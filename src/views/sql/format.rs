//! Format SQL (⌘⇧F): one clause per line, select lists one item per line,
//! `and` / `or` leading their lines, subqueries and CTEs indented.
//!
//! Only whitespace and keyword case change. Text is split into Postgres tokens
//! first (strings, quoted names, dollar quotes, comments and operators are
//! kept byte for byte), then laid out again. Keywords follow the input's
//! case: lowercase unless most of them were typed in capitals. Formatting
//! formatted text gives the same text back.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Word,
    /// `"name"`, `U&"name"`
    Quoted,
    /// `'…'`, `E'…'`, `$tag$…$tag$`
    Str,
    Num,
    /// `$1`
    Param,
    Op,
    Comma,
    Dot,
    /// `::`
    Cast,
    Colon,
    /// `(` or `[`
    Open,
    /// `)` or `]`
    Close,
    Semi,
    LineComment,
    BlockComment,
    Other,
}

#[derive(Clone, Debug)]
struct Tok {
    kind: Kind,
    text: String,
    /// a line break before it in the input (or it's first)
    nl_before: bool,
    /// a line break after it in the input (or it's last)
    nl_after: bool,
    /// no whitespace before it in the input
    glued: bool,
}

impl Tok {
    fn is_comment(&self) -> bool {
        matches!(self.kind, Kind::LineComment | Kind::BlockComment)
    }
}

const OP_CHARS: &str = "+-*/<>=~!@#%^&|`?";

fn tokenize(s: &str) -> Vec<Tok> {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let at = |i: usize| c.get(i).copied().unwrap_or('\0');
    let mut out: Vec<Tok> = vec![];
    let mut i = 0;
    let mut nl = true;
    let mut glued = false;
    while i < n {
        let ch = c[i];
        if ch.is_whitespace() {
            nl |= ch == '\n';
            glued = false;
            i += 1;
            continue;
        }
        let start = i;
        let kind = match ch {
            '-' if at(i + 1) == '-' => {
                while i < n && c[i] != '\n' {
                    i += 1;
                }
                Kind::LineComment
            }
            '/' if at(i + 1) == '*' => {
                let mut depth = 0;
                while i < n {
                    if c[i] == '/' && at(i + 1) == '*' {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && at(i + 1) == '/' {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                Kind::BlockComment
            }
            '\'' => {
                i = string_end(&c, i, false);
                Kind::Str
            }
            '"' => {
                i = quoted_end(&c, i);
                Kind::Quoted
            }
            '$' if at(i + 1).is_ascii_digit() => {
                i += 1;
                while i < n && c[i].is_ascii_digit() {
                    i += 1;
                }
                Kind::Param
            }
            '$' => match dollar_tag(&c, i) {
                Some(end) => {
                    let tag = &c[i..end];
                    i = end;
                    loop {
                        if i >= n {
                            break;
                        }
                        if c[i..].starts_with(tag) {
                            i += tag.len();
                            break;
                        }
                        i += 1;
                    }
                    Kind::Str
                }
                None => {
                    i += 1;
                    Kind::Other
                }
            },
            '0'..='9' => {
                i = number_end(&c, i);
                Kind::Num
            }
            '.' if at(i + 1).is_ascii_digit() => {
                i = number_end(&c, i);
                Kind::Num
            }
            '.' => {
                i += 1;
                Kind::Dot
            }
            ',' => {
                i += 1;
                Kind::Comma
            }
            ';' => {
                i += 1;
                Kind::Semi
            }
            '(' | '[' => {
                i += 1;
                Kind::Open
            }
            ')' | ']' => {
                i += 1;
                Kind::Close
            }
            ':' if at(i + 1) == ':' => {
                i += 2;
                Kind::Cast
            }
            ':' => {
                i += 1;
                Kind::Colon
            }
            _ if OP_CHARS.contains(ch) => {
                let mut j = i;
                while j < n && OP_CHARS.contains(c[j]) {
                    if j > i && ((c[j] == '-' && at(j + 1) == '-') || (c[j] == '/' && at(j + 1) == '*')) {
                        break;
                    }
                    j += 1;
                }
                // like Postgres: a multi-char operator can't end in + or -
                // unless it has one of ~!@#%^&|`?
                if j - i > 1 && !c[i..j].iter().any(|c| "~!@#%^&|`?".contains(*c)) {
                    while j - i > 1 && matches!(c[j - 1], '+' | '-') {
                        j -= 1;
                    }
                }
                i = j;
                Kind::Op
            }
            _ if matches!(ch, 'e' | 'E' | 'b' | 'B' | 'x' | 'X' | 'n' | 'N') && at(i + 1) == '\'' => {
                i = string_end(&c, i + 1, matches!(ch, 'e' | 'E'));
                Kind::Str
            }
            _ if matches!(ch, 'u' | 'U') && at(i + 1) == '&' && matches!(at(i + 2), '\'' | '"') => {
                if at(i + 2) == '\'' {
                    i = string_end(&c, i + 2, false);
                    Kind::Str
                } else {
                    i = quoted_end(&c, i + 2);
                    Kind::Quoted
                }
            }
            _ if ch.is_alphabetic() || ch == '_' => {
                while i < n && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == '$') {
                    i += 1;
                }
                Kind::Word
            }
            _ => {
                i += 1;
                Kind::Other
            }
        };
        if let Some(prev) = out.last_mut() {
            prev.nl_after = nl;
        }
        let mut text: String = c[start..i].iter().collect();
        if kind == Kind::LineComment {
            text.truncate(text.trim_end().len());
        }
        out.push(Tok {
            kind,
            text,
            nl_before: nl,
            nl_after: true,
            glued,
        });
        nl = false;
        glued = true;
    }
    out
}

/// Past the closing quote of the string opening at `i` (or the end).
fn string_end(c: &[char], mut i: usize, backslash: bool) -> usize {
    i += 1;
    while i < c.len() {
        match c[i] {
            '\\' if backslash => i += 2,
            '\'' if c.get(i + 1) == Some(&'\'') => i += 2,
            '\'' => return i + 1,
            _ => i += 1,
        }
    }
    c.len()
}

fn quoted_end(c: &[char], mut i: usize) -> usize {
    i += 1;
    while i < c.len() {
        if c[i] == '"' {
            if c.get(i + 1) == Some(&'"') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    c.len()
}

/// The end of a `$tag$` opening at `i`.
fn dollar_tag(c: &[char], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if j < c.len() && !(c[j].is_alphabetic() || c[j] == '_' || c[j] == '$') {
        return None;
    }
    while j < c.len() && (c[j].is_alphanumeric() || c[j] == '_') {
        j += 1;
    }
    (c.get(j) == Some(&'$')).then_some(j + 1)
}

fn number_end(c: &[char], mut i: usize) -> usize {
    let start = i;
    let hex = c.get(i) == Some(&'0') && matches!(c.get(i + 1), Some('x' | 'X'));
    while i < c.len() {
        let ch = c[i];
        if ch.is_alphanumeric() || ch == '_' {
            i += 1;
        } else if ch == '.' && c.get(i + 1) != Some(&'.') {
            i += 1;
        } else if matches!(ch, '+' | '-')
            && !hex
            && i > start
            && matches!(c[i - 1], 'e' | 'E')
            && c.get(i + 1).is_some_and(|d| d.is_ascii_digit())
        {
            i += 1;
        } else {
            break;
        }
    }
    i
}

// -- tree ---------------------------------------------------------------------

#[derive(Debug)]
enum Node {
    T(Tok),
    G { open: Tok, body: Vec<Node>, close: Option<Tok> },
}

fn tree(toks: Vec<Tok>) -> Vec<Node> {
    // stack of (open token, nodes so far)
    let mut stack: Vec<(Option<Tok>, Vec<Node>)> = vec![(None, vec![])];
    for t in toks {
        match t.kind {
            Kind::Open => stack.push((Some(t), vec![])),
            Kind::Close
                if stack.len() > 1
                    && stack.last().unwrap().0.as_ref().is_some_and(|o| {
                        (o.text == "(" && t.text == ")") || (o.text == "[" && t.text == "]")
                    }) =>
            {
                let (open, body) = stack.pop().unwrap();
                stack.last_mut().unwrap().1.push(Node::G {
                    open: open.unwrap(),
                    body,
                    close: Some(t),
                });
            }
            _ => stack.last_mut().unwrap().1.push(Node::T(t)),
        }
    }
    while stack.len() > 1 {
        let (open, body) = stack.pop().unwrap();
        stack.last_mut().unwrap().1.push(Node::G {
            open: open.unwrap(),
            body,
            close: None,
        });
    }
    stack.pop().unwrap().1
}

/// A plain word, lowercased (`None` for anything else).
fn word(n: Option<&Node>) -> Option<String> {
    match n {
        Some(Node::T(t)) if t.kind == Kind::Word => Some(t.text.to_lowercase()),
        _ => None,
    }
}

fn is_comment(n: &Node) -> bool {
    matches!(n, Node::T(t) if t.is_comment())
}

fn is_comma(n: &Node) -> bool {
    matches!(n, Node::T(t) if t.kind == Kind::Comma)
}

fn is_paren(n: &Node) -> bool {
    matches!(n, Node::G { open, .. } if open.text == "(")
}

/// The first word that isn't a comment.
fn first_word(nodes: &[Node]) -> Option<String> {
    word(nodes.iter().find(|n| !is_comment(n)))
}

fn is_query(body: &[Node]) -> bool {
    first_word(body).is_some_and(|w| QUERY_START.contains(&w.as_str()))
}

const QUERY_START: &[&str] = &["select", "with", "values", "insert", "update", "delete"];

const KEYWORDS: &[&str] = &[
    "add", "all", "alter", "analyze", "and", "any", "array", "as", "asc", "begin", "between", "by",
    "cascade", "case", "check", "column", "commit", "conflict", "constraint", "create", "cross",
    "default", "delete", "desc", "distinct", "do", "drop", "else", "end", "except", "exists",
    "explain", "false", "fetch", "filter", "first", "for", "foreign", "from", "full", "grant",
    "group", "having", "if", "ilike", "in", "index", "inner", "insert", "intersect", "into", "is",
    "join", "key", "last", "lateral", "left", "like", "limit", "locked", "materialized", "natural",
    "next", "not", "nothing", "nowait", "null", "nulls", "of", "offset", "on", "only", "or",
    "order", "outer", "over", "partition", "primary", "recursive", "references", "replace",
    "restrict", "returning", "revoke", "right", "rollback", "row", "rows", "select", "set",
    "share", "similar", "skip", "some", "table", "temp", "temporary", "then", "to", "true",
    "truncate", "union", "unique", "update", "using", "values", "verbose", "view", "when",
    "where", "window", "with", "within",
];

fn is_kw(w: &str) -> bool {
    KEYWORDS.contains(&w)
}

/// Keywords that read like functions: no space before their `(`.
const CALL_KEYWORDS: &[&str] = &["any", "some", "all", "array", "left", "right"];

// -- writer -------------------------------------------------------------------

struct Prev {
    kind: Kind,
    text: String,
    lower: String,
    /// a prefix `-` / `+`
    unary: bool,
}

struct W {
    out: String,
    /// indent of the current line
    indent: usize,
    /// nothing but indent on the current line yet
    line_start: bool,
    /// where the current line (after its `\n`) begins
    line_begin: usize,
    /// whether `nl` pushed the `\n` before `line_begin`
    pushed_nl: bool,
    prev: Option<Prev>,
    /// a line comment was written: the next token starts a line
    force_nl: bool,
    /// the next `(` gets a space before it (`insert into t (a, b)`)
    paren_space: bool,
    upper: bool,
}

impl W {
    fn new(upper: bool) -> Self {
        W {
            out: String::new(),
            indent: 0,
            line_start: true,
            line_begin: 0,
            pushed_nl: false,
            prev: None,
            force_nl: false,
            paren_space: false,
            upper,
        }
    }

    fn nl(&mut self, indent: usize) {
        if self.line_start {
            self.out.truncate(self.line_begin);
        } else {
            self.out.push('\n');
            self.pushed_nl = true;
            self.line_begin = self.out.len();
        }
        if self.out.is_empty() {
            self.pushed_nl = false;
        }
        self.out.push_str(&" ".repeat(indent));
        self.indent = indent;
        self.line_start = true;
        self.force_nl = false;
    }

    /// Drop an empty last line.
    fn finish(mut self) -> String {
        if self.line_start {
            self.out.truncate(self.line_begin);
            if self.pushed_nl {
                self.out.pop();
            }
        }
        self.out
    }

    fn tok(&mut self, t: &Tok) {
        if t.is_comment() {
            if (t.nl_before || self.force_nl) && !self.line_start {
                self.nl(self.indent);
            }
            if !self.line_start {
                self.out.push(' ');
            }
            self.out.push_str(&t.text);
            self.line_start = false;
            self.force_nl = t.kind == Kind::LineComment || t.nl_after;
            self.prev = Some(Prev {
                kind: t.kind,
                text: String::new(),
                lower: String::new(),
                unary: false,
            });
            return;
        }
        if self.force_nl {
            self.nl(self.indent);
        }
        // 'a'\n'b' is one string in Postgres; 'a' 'b' is an error
        let continued = t.kind == Kind::Str && t.nl_before && self.prev.as_ref().is_some_and(|p| p.kind == Kind::Str);
        if continued && !self.line_start {
            self.nl(self.indent);
        }
        let lower = t.text.to_lowercase();
        if !self.line_start && (self.space_before(t) || !self.apart(t)) {
            self.out.push(' ');
        }
        let after_dot = self.prev.as_ref().is_some_and(|p| p.kind == Kind::Dot);
        if t.kind == Kind::Word && !after_dot && is_kw(&lower) {
            self.out.push_str(&if self.upper { t.text.to_uppercase() } else { lower.clone() });
        } else {
            self.out.push_str(&t.text);
        }
        let unary = t.kind == Kind::Op
            && matches!(t.text.as_str(), "-" | "+")
            && match &self.prev {
                None => true,
                Some(p) => {
                    matches!(p.kind, Kind::Op | Kind::Open | Kind::Comma | Kind::Colon)
                        || (p.kind == Kind::Word
                            && is_kw(&p.lower)
                            && !matches!(p.lower.as_str(), "end" | "null" | "true" | "false"))
                }
            };
        if t.kind == Kind::Open {
            self.paren_space = false;
        }
        self.line_start = false;
        self.prev = Some(Prev {
            kind: t.kind,
            text: t.text.clone(),
            lower,
            unary,
        });
    }

    /// `t` written right after the last token reads back as the same two
    /// tokens (`:` then `::` would read as `::` then `:`).
    fn apart(&self, t: &Tok) -> bool {
        let Some(p) = &self.prev else { return true };
        let both = tokenize(&format!("{}{}", p.text, t.text));
        both.len() == 2 && both[0].text == p.text && both[1].text == t.text
    }

    fn space_before(&self, t: &Tok) -> bool {
        let Some(p) = &self.prev else { return false };
        if matches!(p.kind, Kind::LineComment | Kind::BlockComment) {
            return true;
        }
        if (t.kind == Kind::Other || p.kind == Kind::Other) && t.glued {
            return false;
        }
        if matches!(t.kind, Kind::Comma | Kind::Semi | Kind::Dot | Kind::Cast | Kind::Colon | Kind::Close) {
            return false;
        }
        if matches!(p.kind, Kind::Dot | Kind::Cast | Kind::Colon | Kind::Open) {
            return false;
        }
        if p.unary {
            // `- -1`, never `--1`
            return t.kind == Kind::Op;
        }
        if t.kind == Kind::Open {
            if t.text == "[" {
                return !matches!(p.kind, Kind::Word | Kind::Quoted | Kind::Close | Kind::Param);
            }
            if self.paren_space {
                return true;
            }
            return match p.kind {
                Kind::Word => is_kw(&p.lower) && !CALL_KEYWORDS.contains(&p.lower.as_str()),
                Kind::Quoted => false,
                _ => true,
            };
        }
        true
    }

    fn inline(&mut self, nodes: &[Node]) {
        for n in nodes {
            self.node(n);
        }
    }

    fn node(&mut self, n: &Node) {
        match n {
            Node::T(t) => self.tok(t),
            Node::G { open, body, close } => {
                if open.text == "(" && is_query(body) {
                    self.subquery(open, body, close.as_ref());
                } else {
                    self.tok(open);
                    self.inline(body);
                    if let Some(c) = close {
                        self.tok(c);
                    }
                }
            }
        }
    }

    /// `(`, the query indented on its own lines, `)`.
    fn subquery(&mut self, open: &Tok, body: &[Node], close: Option<&Tok>) {
        self.tok(open);
        let base = self.indent;
        self.nl(base + 2);
        query(body, base + 2, self);
        self.nl(base);
        if let Some(c) = close {
            self.tok(c);
        }
    }

    /// `(`, one item per line, `)` (`create table` columns).
    fn expanded(&mut self, open: &Tok, body: &[Node], close: Option<&Tok>) {
        if body.is_empty() {
            self.tok(open);
            if let Some(c) = close {
                self.tok(c);
            }
            return;
        }
        self.tok(open);
        let base = self.indent;
        self.list_items(body, base + 2);
        self.nl(base);
        if let Some(c) = close {
            self.tok(c);
        }
    }

    /// Comma-separated items, each on its own line at `indent`. Comments
    /// that trailed a comma stay on its line.
    fn list_items(&mut self, nodes: &[Node], indent: usize) {
        for item in split_commas(nodes) {
            let lead = item
                .iter()
                .take_while(|n| matches!(n, Node::T(t) if t.is_comment() && !t.nl_before))
                .count();
            self.inline(&item[..lead]);
            self.nl(indent);
            self.inline(&item[lead..]);
            if let Some(comma) = item.comma {
                self.node(comma);
            }
        }
    }
}

struct Item<'a> {
    nodes: &'a [Node],
    comma: Option<&'a Node>,
}

impl std::ops::Deref for Item<'_> {
    type Target = [Node];
    fn deref(&self) -> &[Node] {
        self.nodes
    }
}

fn split_commas(nodes: &[Node]) -> Vec<Item<'_>> {
    let mut out = vec![];
    let mut start = 0;
    for (i, n) in nodes.iter().enumerate() {
        if is_comma(n) {
            out.push(Item {
                nodes: &nodes[start..i],
                comma: Some(n),
            });
            start = i + 1;
        }
    }
    if start < nodes.len() || out.is_empty() {
        out.push(Item {
            nodes: &nodes[start..],
            comma: None,
        });
    }
    out
}

// -- layout -------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    /// on one line
    Inline,
    /// head, then comma-separated items one per line when there are several
    List,
    /// `and` / `or` start lines
    Cond,
    /// `insert into t (a, b)`: a space before the column list
    Insert,
}

struct Clause {
    start: usize,
    layout: Layout,
    head: usize,
}

/// Where the clauses of a query at this level start.
fn clauses(nodes: &[Node]) -> Vec<Clause> {
    let w = |i: usize| word(nodes.get(i));
    let first = w(0);
    let first = first.as_deref();
    let mut out: Vec<Clause> = vec![];
    let mut cur: Option<String> = None;
    let mut i = 0;
    while i < nodes.len() {
        let Some(word) = w(i) else {
            i += 1;
            continue;
        };
        let prev = i.checked_sub(1).and_then(w);
        let prev = prev.as_deref();
        let next = w(i + 1);
        let next = next.as_deref();
        let found: Option<(Layout, usize)> = match word.as_str() {
            "select" => {
                let mut head = 1;
                match next {
                    Some("distinct") => {
                        head += 1;
                        if w(i + 2).as_deref() == Some("on") && nodes.get(i + 3).is_some_and(is_paren) {
                            head += 2;
                        }
                    }
                    Some("all") => head += 1,
                    _ => {}
                }
                Some((Layout::List, head))
            }
            "insert" | "update" | "delete" if i == 0 => {
                let two = matches!((word.as_str(), next), ("insert", Some("into")) | ("delete", Some("from")));
                Some((if word == "insert" { Layout::Insert } else { Layout::Inline }, 1 + two as usize))
            }
            "from" if prev != Some("distinct") => Some((Layout::List, 1)),
            "where" | "having" => Some((Layout::Cond, 1)),
            "group" | "order" if next == Some("by") => Some((Layout::Inline, 2)),
            "limit" | "offset" | "window" => Some((Layout::Inline, 1)),
            "fetch" if matches!(next, Some("first" | "next")) => Some((Layout::Inline, 1)),
            "for" if matches!(next, Some("update" | "share" | "no" | "key")) => Some((Layout::Inline, 2)),
            "into" if cur.as_deref() == Some("select") => Some((Layout::Inline, 1)),
            "union" | "intersect" | "except" => {
                Some((Layout::Inline, 1 + matches!(next, Some("all" | "distinct")) as usize))
            }
            "default" if next == Some("values") => Some((Layout::Inline, 2)),
            "values" => Some((Layout::List, 1)),
            "set" if first == Some("update") || prev == Some("update") => Some((Layout::List, 1)),
            "on" if next == Some("conflict") => Some((Layout::Inline, 2)),
            "returning" => Some((Layout::List, 1)),
            "using" if first == Some("delete") && cur.as_deref() == Some("delete") => Some((Layout::Inline, 1)),
            "join" | "natural" | "inner" | "left" | "right" | "full" | "cross" => {
                let mut j = i;
                while matches!(w(j).as_deref(), Some("natural" | "inner" | "left" | "right" | "full" | "cross" | "outer")) {
                    j += 1;
                }
                (w(j).as_deref() == Some("join")).then_some((Layout::Cond, j + 1 - i))
            }
            _ => None,
        };
        match found {
            Some((layout, head)) => {
                out.push(Clause { start: i, layout, head });
                cur = Some(word);
                i += head;
            }
            None => i += 1,
        }
    }
    out
}

/// Lay out a query at `base` indent, starting where the writer is.
fn query(nodes: &[Node], base: usize, w: &mut W) {
    let lead = nodes.iter().take_while(|n| is_comment(n)).count();
    w.inline(&nodes[..lead]);
    let nodes = &nodes[lead..];
    if word(nodes.first()).as_deref() == Some("with") {
        return cte(nodes, base, w);
    }
    let cs = clauses(nodes);
    let pre = cs.first().map_or(nodes.len(), |c| c.start);
    w.inline(&nodes[..pre]);
    for (k, c) in cs.iter().enumerate() {
        let end = cs.get(k + 1).map_or(nodes.len(), |n| n.start);
        if k > 0 || pre > 0 {
            w.nl(base);
        }
        clause(&nodes[c.start..end], c, base, w);
    }
}

fn clause(nodes: &[Node], c: &Clause, base: usize, w: &mut W) {
    let head = c.head.min(nodes.len());
    match c.layout {
        Layout::Inline => w.inline(nodes),
        Layout::Insert => {
            for n in nodes {
                if is_paren(n) {
                    w.paren_space = true;
                }
                w.node(n);
            }
        }
        Layout::List => {
            w.inline(&nodes[..head]);
            let rest = &nodes[head..];
            if split_commas(rest).len() > 1 {
                w.list_items(rest, base + 2);
            } else {
                w.inline(rest);
            }
        }
        Layout::Cond => {
            w.inline(&nodes[..head]);
            let mut between = false;
            let mut case = 0usize;
            for n in &nodes[head..] {
                match word(Some(n)).as_deref() {
                    Some("between") => between = true,
                    Some("case") => case += 1,
                    Some("end") if case > 0 => case -= 1,
                    Some("and") if between => between = false,
                    Some("and" | "or") if case == 0 => w.nl(base + 2),
                    _ => {}
                }
                w.node(n);
            }
        }
    }
}

/// `with a as (…), b as (…)` then the query.
fn cte(nodes: &[Node], base: usize, w: &mut W) {
    let mut i = 1;
    w.node(&nodes[0]);
    if word(nodes.get(1)).as_deref() == Some("recursive") {
        w.node(&nodes[1]);
        i = 2;
    }
    loop {
        // the body: a ( after `as` or `materialized`
        let Some(body) = (i..nodes.len()).find(|&j| {
            is_paren(&nodes[j]) && matches!(word(j.checked_sub(1).and_then(|p| nodes.get(p))).as_deref(), Some("as" | "materialized"))
        }) else {
            w.inline(&nodes[i..]);
            return;
        };
        w.inline(&nodes[i..body]);
        if let Node::G { open, body: inner, close } = &nodes[body] {
            w.subquery(open, inner, close.as_ref());
        }
        i = body + 1;
        match nodes.get(i) {
            Some(n) if is_comma(n) => {
                w.node(n);
                i += 1;
                w.nl(base);
            }
            _ => break,
        }
    }
    // `search` / `cycle` and anything else before the query
    let rest = &nodes[i..];
    let q = rest
        .iter()
        .position(|n| word(Some(n)).is_some_and(|w| QUERY_START.contains(&w.as_str())))
        .unwrap_or(rest.len());
    w.inline(&rest[..q]);
    if q < rest.len() {
        w.nl(base);
        query(&rest[q..], base, w);
    }
}

/// One statement, without its `;`.
fn statement(nodes: &[Node], w: &mut W) {
    let lead = nodes.iter().take_while(|n| is_comment(n)).count();
    w.inline(&nodes[..lead]);
    let nodes = &nodes[lead..];
    let at = |i: usize| word(nodes.get(i));
    match at(0).as_deref() {
        Some(q) if QUERY_START.contains(&q) => query(nodes, 0, w),
        Some("explain") => {
            let q = nodes
                .iter()
                .position(|n| word(Some(n)).is_some_and(|w| QUERY_START.contains(&w.as_str())))
                .unwrap_or(nodes.len());
            w.inline(&nodes[..q]);
            query(&nodes[q..], 0, w);
        }
        Some("create") => {
            // create view v as select …
            let as_query = (0..nodes.len()).find(|&i| {
                at(i).as_deref() == Some("as") && at(i + 1).is_some_and(|w| QUERY_START.contains(&w.as_str()))
            });
            if let Some(a) = as_query {
                w.inline(&nodes[..=a]);
                w.nl(0);
                query(&nodes[a + 1..], 0, w);
                return;
            }
            // create table t (columns)
            let g = nodes.iter().position(is_paren);
            let table = g.filter(|&g| {
                g > 0
                    && (0..g).any(|i| at(i).as_deref() == Some("table"))
                    && match &nodes[g - 1] {
                        Node::T(t) => t.kind == Kind::Quoted || (t.kind == Kind::Word && !is_kw(&t.text.to_lowercase())),
                        _ => false,
                    }
            });
            match table {
                Some(g) => {
                    w.inline(&nodes[..g]);
                    if let Node::G { open, body, close } = &nodes[g] {
                        w.paren_space = true;
                        w.expanded(open, body, close.as_ref());
                    }
                    w.inline(&nodes[g + 1..]);
                }
                None => w.inline(nodes),
            }
        }
        _ => w.inline(nodes),
    }
}

/// Format `sql`: each statement laid out on its own, separated by a blank line.
pub fn format(sql: &str) -> String {
    let toks = tokenize(sql);
    let (up, low) = toks
        .iter()
        .filter(|t| t.kind == Kind::Word && is_kw(&t.text.to_lowercase()))
        .fold((0, 0), |(u, l), t| {
            if t.text.chars().any(char::is_lowercase) {
                (u, l + 1)
            } else {
                (u + 1, l)
            }
        });
    let upper = up > low;

    // split on `;`; comments on the line after a `;` stay with it
    struct Stmt {
        body: Vec<Tok>,
        semi: Option<Tok>,
        tail: Vec<Tok>,
    }
    let mut stmts: Vec<Stmt> = vec![];
    let mut cur = Stmt { body: vec![], semi: None, tail: vec![] };
    for t in toks {
        if cur.semi.is_some() {
            if t.is_comment() && !t.nl_before {
                cur.tail.push(t);
                continue;
            }
            stmts.push(std::mem::replace(&mut cur, Stmt { body: vec![], semi: None, tail: vec![] }));
        }
        if t.kind == Kind::Semi {
            cur.semi = Some(t);
        } else {
            cur.body.push(t);
        }
    }
    stmts.push(cur);

    let mut parts = vec![];
    for mut s in stmts {
        if s.body.is_empty() && s.tail.is_empty() {
            continue;
        }
        // `select 1 -- one` + `;` → `select 1; -- one`
        let mut moved = vec![];
        if s.semi.is_some() && s.tail.is_empty() {
            while s.body.last().is_some_and(|t| t.is_comment() && !t.nl_before) {
                moved.insert(0, s.body.pop().unwrap());
            }
        }
        let mut w = W::new(upper);
        statement(&tree(s.body), &mut w);
        if let Some(semi) = &s.semi {
            w.tok(semi);
        }
        for t in moved.iter().chain(&s.tail) {
            w.tok(t);
        }
        let out = w.finish();
        if !out.is_empty() {
            parts.push(out);
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Postgres sees: tokens without comments, keywords folded.
    fn meaning(sql: &str) -> Vec<String> {
        tokenize(sql)
            .into_iter()
            .filter(|t| !t.is_comment())
            .map(|t| if t.kind == Kind::Word { t.text.to_lowercase() } else { t.text })
            .collect()
    }

    fn comments(sql: &str) -> Vec<String> {
        tokenize(sql).into_iter().filter(Tok::is_comment).map(|t| t.text).collect()
    }

    /// Format, and check it keeps meaning and comments and is idempotent.
    fn f(sql: &str) -> String {
        let out = format(sql);
        assert_eq!(meaning(sql), meaning(&out), "meaning changed:\n{out}");
        assert_eq!(comments(sql), comments(&out), "comments changed:\n{out}");
        assert_eq!(format(&out), out, "not idempotent:\n{out}");
        assert!(!out.lines().any(|l| l.ends_with(' ')), "trailing space:\n{out}");
        out
    }

    #[test]
    fn select_layout() {
        assert_eq!(
            f("SELECT id, name FROM users WHERE active = true AND age > 18 ORDER BY name LIMIT 10"),
            "SELECT\n  id,\n  name\nFROM users\nWHERE active = TRUE\n  AND age > 18\nORDER BY name\nLIMIT 10"
        );
        assert_eq!(f("select * from users where id = 1"), "select *\nfrom users\nwhere id = 1");
        assert_eq!(
            f("select u.id, count(*) as n from users u group by u.id having count(*) > 1"),
            "select\n  u.id,\n  count(*) as n\nfrom users u\ngroup by u.id\nhaving count(*) > 1"
        );
    }

    #[test]
    fn keyword_case_follows_input() {
        assert_eq!(f("Select a From t"), "select a\nfrom t");
        assert_eq!(f("SELECT a FROM t where x"), "SELECT a\nFROM t\nWHERE x");
        // names keep their case
        assert_eq!(f("select Name from Users"), "select Name\nfrom Users");
    }

    #[test]
    fn joins() {
        assert_eq!(
            f("select * from users u left outer join orders o on o.user_id = u.id and o.paid join items i using (id)"),
            "select *\nfrom users u\nleft outer join orders o on o.user_id = u.id\n  and o.paid\njoin items i using (id)"
        );
        // left() the function isn't a join
        assert_eq!(f("select left(name, 3) from t"), "select left(name, 3)\nfrom t");
    }

    #[test]
    fn where_between_and_case() {
        assert_eq!(
            f("select * from t where a between 1 and 5 and b = 2 or case when x and y then true end"),
            "select *\nfrom t\nwhere a between 1 and 5\n  and b = 2\n  or case when x and y then true end"
        );
        assert_eq!(f("select * from t where a is distinct from b"), "select *\nfrom t\nwhere a is distinct from b");
    }

    #[test]
    fn subqueries() {
        assert_eq!(
            f("select * from users where id in (select user_id from orders where total > 10) and active"),
            "select *\nfrom users\nwhere id in (\n  select user_id\n  from orders\n  where total > 10\n)\n  and active"
        );
        assert_eq!(
            f("select a, (select count(*) from b where b.a = a.id) as n from a"),
            "select\n  a,\n  (\n    select count(*)\n    from b\n    where b.a = a.id\n  ) as n\nfrom a"
        );
        assert_eq!(
            f("select * from (select 1 as x) as t"),
            "select *\nfrom (\n  select 1 as x\n) as t"
        );
    }

    #[test]
    fn ctes() {
        assert_eq!(
            f("with a as (select 1 as x), b as materialized (select x from a) select * from b"),
            "with a as (\n  select 1 as x\n),\nb as materialized (\n  select x\n  from a\n)\nselect *\nfrom b"
        );
        assert_eq!(
            f("WITH RECURSIVE t(n) AS (VALUES (1) UNION ALL SELECT n+1 FROM t WHERE n < 100) SELECT sum(n) FROM t"),
            "WITH RECURSIVE t(n) AS (\n  VALUES (1)\n  UNION ALL\n  SELECT n + 1\n  FROM t\n  WHERE n < 100\n)\nSELECT sum(n)\nFROM t"
        );
    }

    #[test]
    fn insert_update_delete() {
        assert_eq!(
            f("insert into users(name, email) values ('a', 'a@x'), ('b', 'b@x') returning id"),
            "insert into users (name, email)\nvalues\n  ('a', 'a@x'),\n  ('b', 'b@x')\nreturning id"
        );
        assert_eq!(f("insert into t (a) values (1)"), "insert into t (a)\nvalues (1)");
        assert_eq!(f("insert into t default values"), "insert into t\ndefault values");
        assert_eq!(
            f("insert into t (id, a) select id, a from s on conflict (id) do update set a = excluded.a, b = 2"),
            "insert into t (id, a)\nselect\n  id,\n  a\nfrom s\non conflict (id) do update\nset\n  a = excluded.a,\n  b = 2"
        );
        assert_eq!(
            f("update neverwait.online_profiles set \"firstName\" = 'hello', age = age + 1 where id = 3;"),
            "update neverwait.online_profiles\nset\n  \"firstName\" = 'hello',\n  age = age + 1\nwhere id = 3;"
        );
        assert_eq!(f("update t set a = 1 from s where s.id = t.id"), "update t\nset a = 1\nfrom s\nwhere s.id = t.id");
        assert_eq!(f("delete from t using s where s.id = t.id"), "delete from t\nusing s\nwhere s.id = t.id");
    }

    #[test]
    fn preserves_literals() {
        let s = "select 'from x where; -- not a comment', \"Select\" , E'it\\'s', $$ select ; $$, $fn$ a $ b $fn$ from \"My Table\"";
        let out = f(s);
        for lit in ["'from x where; -- not a comment'", "\"Select\"", "E'it\\'s'", "$$ select ; $$", "$fn$ a $ b $fn$", "\"My Table\""] {
            assert!(out.contains(lit), "{lit} missing from {out}");
        }
        assert_eq!(f("select 'it''s'"), "select 'it''s'");
        assert_eq!(f("select U&'d\\0061t' , x'1F', b'101'"), "select\n  U&'d\\0061t',\n  x'1F',\n  b'101'");
        // adjacent strings across a line break are one string
        assert_eq!(f("select 'a'\n'b'"), "select 'a'\n'b'");
    }

    #[test]
    fn operators_and_casts() {
        assert_eq!(f("select a::int, b :: text[], c->>'k', d#>>'{a}', -1, - -1, a - -b from t"),
            "select\n  a::int,\n  b::text[],\n  c ->> 'k',\n  d #>> '{a}',\n  -1,\n  - -1,\n  a - -b\nfrom t");
        assert_eq!(f("select a=-1"), "select a = -1");
        assert_eq!(f("select x[1:2], arr[1], array[1,2]"), "select\n  x[1:2],\n  arr[1],\n  array[1, 2]");
        assert_eq!(f("select 1.5e-3, .5, $1, a@>b, a<>b, a!=b"), "select\n  1.5e-3,\n  .5,\n  $1,\n  a @> b,\n  a <> b,\n  a != b");
        assert_eq!(f("select count(distinct x) filter (where y) over (partition by z order by w) from t"),
            "select count(distinct x) filter (where y) over (partition by z order by w)\nfrom t");
    }

    #[test]
    fn comments_kept() {
        assert_eq!(
            f("-- users\nselect a, -- the a\n b /* bee */ from t -- end\n;"),
            "-- users\nselect\n  a, -- the a\n  b /* bee */\nfrom t; -- end"
        );
        assert_eq!(f("select a -- x\nfrom t"), "select a -- x\nfrom t");
        assert_eq!(f("select a\n-- why\nfrom t"), "select a\n-- why\nfrom t");
        assert_eq!(f("/* header\n  two */\nselect 1"), "/* header\n  two */\nselect 1");
        assert_eq!(f("select /* a /* nested */ b */ 1"), "select /* a /* nested */ b */ 1");
        assert_eq!(f("select 1\n-- note\n;"), "select 1\n-- note\n;");
    }

    #[test]
    fn several_statements() {
        assert_eq!(
            f("select 1; select 2 from t;\n\n\nupdate t set a = 1;"),
            "select 1;\n\nselect 2\nfrom t;\n\nupdate t\nset a = 1;"
        );
        assert_eq!(f("select 1; -- one\nselect 2"), "select 1; -- one\n\nselect 2");
        // an empty statement is dropped
        assert_eq!(format("select 1;;"), "select 1;");
        assert_eq!(f("  \n "), "");
    }

    #[test]
    fn other_statements() {
        assert_eq!(
            f("create table if not exists t (id serial primary key, name text not null, check (length(name) > 0))"),
            "create table if not exists t (\n  id serial primary key,\n  name text not null,\n  check (length(name) > 0)\n)"
        );
        assert_eq!(f("create view v as select a, b from t"), "create view v as\nselect\n  a,\n  b\nfrom t");
        assert_eq!(f("explain analyze select a from t where x"), "explain analyze select a\nfrom t\nwhere x");
        assert_eq!(f("grant select on t to bob"), "grant select on t to bob");
        assert_eq!(f("set search_path = public, audit"), "set search_path = public, audit");
        assert_eq!(f("select x::timestamp with time zone from t"), "select x::timestamp with time zone\nfrom t");
        assert_eq!(
            f("create function f() returns int as $$\nbegin\n  return 1;\nend\n$$ language plpgsql"),
            "create function f() returns int as $$\nbegin\n  return 1;\nend\n$$ language plpgsql"
        );
    }

    #[test]
    fn locking_and_paging() {
        assert_eq!(
            f("select * from t order by a desc nulls last offset 10 fetch first 5 rows only for update skip locked"),
            "select *\nfrom t\norder by a desc nulls last\noffset 10\nfetch first 5 rows only\nfor update skip locked"
        );
        assert_eq!(f("select a from t union all select b from s"), "select a\nfrom t\nunion all\nselect b\nfrom s");
    }

    #[test]
    fn broken_input_is_kept() {
        for s in [
            "select (1, 2",
            "select 'unterminated",
            "select \"open",
            "select $$ never closed",
            "select /* open",
            "select a) from t",
            "select ] [ from",
            "with x as select",
            "select \\x {}",
            "insert into",
            "-- only a comment",
        ] {
            f(s);
        }
    }

    /// Random token soup: whatever comes in, meaning, comments and
    /// idempotency hold.
    #[test]
    fn fuzz() {
        let parts = [
            "select", "from", "where", "and", "or", "between", "case", "when", "end", "join", "left",
            "on", "with", "as", "(", ")", "[", "]", ",", ";", "a", "t.b", "'s'", "\"Q\"", "$$x$$",
            "1", "-", "+", "::", ":", "*", "=", "<>", "-- c", "/* b */", "values", "insert", "into",
            "update", "set", "union", "all", "distinct", "group", "by", "order", "limit", "in",
            "exists", "create", "table", "returning", "E'\\''", "$1", "not", "is", "null",
            "e", "u", "&", "'a'", ".", "1.5", "1e", "->>", "#", "/", "$", "\\", "x", "--", "/*", "*/",
            "?", "!", "|", "%", "0x", "fetch", "first", "for", "delete", "using", "do", "conflict",
            "recursive", "materialized", "explain", "default",
        ];
        let seps = [" ", "", "\n", "  \n\n "];
        let mut seed: u64 = 0x9e3779b97f4a7c15;
        let mut rand = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..3000 {
            let mut s = String::new();
            for _ in 0..rand(30) {
                s.push_str(parts[rand(parts.len())]);
                s.push_str(seps[rand(seps.len())]);
            }
            let out = format(&s);
            let keep = |sql: &str| {
                let mut m = meaning(sql);
                m.dedup_by(|a, b| a == ";" && b == ";");
                while m.first().is_some_and(|t| t == ";") {
                    m.remove(0);
                }
                m
            };
            assert_eq!(keep(&s), keep(&out), "meaning changed:\n{s:?}\n{out}");
            assert_eq!(comments(&s), comments(&out), "comments changed:\n{s:?}\n{out}");
            assert_eq!(format(&out), out, "not idempotent:\n{s:?}\n{out}");
        }
    }

    #[test]
    fn idempotent_on_a_corpus() {
        let corpus = [
            "select a,b,c from x join y on x.id=y.id where x.a in (1,2,3) and (y.b or y.c) group by a,b order by c desc",
            "with a as (select * from (select 1) s), b as (select * from a where exists (select 1 from a)) select * from a, b",
            "update t set (a, b) = (select a, b from s where s.id = t.id) where t.id in (select id from u)",
            "insert into t select * from s where not exists (select 1 from t where t.id = s.id) returning *",
            "select case when a > 0 then 'pos' when a < 0 then 'neg' else 'zero' end as sign, coalesce(b, 0) from t",
            "SELECT DISTINCT ON (a) a, b FROM t ORDER BY a, b DESC",
            "select * from t where a ilike '%x%' and b similar to 'y' and c is not null or d = any(array[1,2])",
            "select json_build_object('a', 1)->>'a', now() - interval '1 day', date '2020-01-01'",
            "select * from t cross join lateral (select * from s where s.t_id = t.id limit 1) x",
            "delete from t where id = $1 returning id, name",
        ];
        for s in corpus {
            f(s);
        }
    }
}
