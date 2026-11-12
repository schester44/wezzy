//! The schema browser: a column at the left of the SQL view (⌘O) listing
//! schemas → tables → columns, with each table's keys and indexes.
//!
//! The tree is rebuilt from the loaded `Schema` on every draw and key
//! (`rows`), so a reload after DDL just shows up. What's expanded is kept by
//! name, so it survives reloads too. `/` types a filter over table names.

use super::complete::ident;
use super::db::{Schema, Table};
use crate::render::Metrics;
use crate::ui::{icon, with_alpha, Icon, Theme};
use gpui::{div, prelude::*, px, AnyElement, FontWeight, Keystroke, Pixels, Point};
use std::collections::HashSet;

/// the browser's width, before it's capped to part of the pane
pub const WIDTH: f32 = 260.;
const ROW_H: f32 = 24.;
const FILTER_H: f32 = 34.;
const INDENT: f32 = 12.;

/// What the view should do after a key or click.
pub enum Act {
    /// used, nothing else to do
    Nothing,
    /// put this statement in the editor and run it
    Open(String),
    /// copy this text, and toast this
    Copy(String, String),
    /// back to the editor
    Editor,
    /// show the shortcut sheet
    Shortcuts,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Schema(String),
    /// index into `Schema::tables`
    Table(usize),
    /// (table, column)
    Column(usize, usize),
    /// the "Indexes" group of a table
    Indexes(usize),
    /// (table, index)
    Index(usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub node: Node,
    pub depth: usize,
    /// Some(expanded) for rows that open
    pub open: Option<bool>,
}

#[derive(Default)]
pub struct Browser {
    pub open: bool,
    cursor: usize,
    top: usize,
    /// keep the cursor in sight on the next draw
    follow: bool,
    /// expanded nodes, by name (`key`)
    expanded: HashSet<String>,
    /// the search path's schemas start expanded, once
    seeded: bool,
    pub filter: String,
    /// typing into the filter
    pub typing: bool,
    /// where the rows start, from the last draw, for clicks
    list_top: f32,
}

fn key(schema: &Schema, node: &Node) -> Option<String> {
    let t = |i: usize| &schema.tables[i];
    match node {
        Node::Schema(s) => Some(format!("s\0{s}")),
        Node::Table(i) => Some(format!("t\0{}\0{}", t(*i).schema, t(*i).name)),
        Node::Indexes(i) => Some(format!("i\0{}\0{}", t(*i).schema, t(*i).name)),
        _ => None,
    }
}

/// Every letter of `pattern`, in order, case ignored.
pub fn fuzzy(pattern: &str, text: &str) -> bool {
    let mut it = text.chars().flat_map(char::to_lowercase);
    pattern
        .chars()
        .flat_map(char::to_lowercase)
        .all(|p| it.any(|c| c == p))
}

/// Schemas in search-path order, then the rest by name.
fn schemas(schema: &Schema) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for t in &schema.tables {
        if !out.contains(&t.schema) {
            out.push(t.schema.clone());
        }
    }
    let rank = |s: &String| schema.search_path.iter().position(|p| p == s).unwrap_or(usize::MAX);
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    out
}

/// The visible tree. With a filter, only matching tables show, and their
/// schemas are open.
pub fn rows(schema: &Schema, expanded: &HashSet<String>, filter: &str) -> Vec<Row> {
    let filter = filter.trim();
    let matches = |t: &Table| {
        filter.is_empty()
            || fuzzy(filter, &t.name)
            || (filter.contains('.') && fuzzy(filter, &format!("{}.{}", t.schema, t.name)))
    };
    let is_open = |n: &Node| key(schema, n).is_some_and(|k| expanded.contains(&k));
    let mut out = vec![];
    for s in schemas(schema) {
        let tables: Vec<usize> = (0..schema.tables.len())
            .filter(|&i| schema.tables[i].schema == s && matches(&schema.tables[i]))
            .collect();
        if tables.is_empty() {
            continue;
        }
        let node = Node::Schema(s);
        let open = !filter.is_empty() || is_open(&node);
        out.push(Row {
            node,
            depth: 0,
            open: Some(open),
        });
        if !open {
            continue;
        }
        for i in tables {
            let node = Node::Table(i);
            let open = is_open(&node);
            out.push(Row {
                node,
                depth: 1,
                open: Some(open),
            });
            if !open {
                continue;
            }
            let t = &schema.tables[i];
            out.extend((0..t.columns.len()).map(|c| Row {
                node: Node::Column(i, c),
                depth: 2,
                open: None,
            }));
            if !t.indexes.is_empty() {
                let node = Node::Indexes(i);
                let open = is_open(&node);
                out.push(Row {
                    node,
                    depth: 2,
                    open: Some(open),
                });
                if open {
                    out.extend((0..t.indexes.len()).map(|x| Row {
                        node: Node::Index(i, x),
                        depth: 3,
                        open: None,
                    }));
                }
            }
        }
    }
    out
}

/// How to name the table in a query: bare when the search path finds it,
/// with its schema otherwise.
pub fn table_ref(schema: &Schema, t: &Table) -> String {
    let pos = |s: &str| schema.search_path.iter().position(|p| p == s);
    let found = pos(&t.schema).is_some_and(|mine| {
        // an earlier schema with a table of the same name would win
        !schema
            .tables
            .iter()
            .any(|o| o.name == t.name && pos(&o.schema).is_some_and(|p| p < mine))
    });
    if found {
        ident(&t.name)
    } else {
        format!("{}.{}", ident(&t.schema), ident(&t.name))
    }
}

/// What a column's row shows after its type: `pk`, and `→ users.id` for
/// each foreign key it's part of.
pub fn column_meta(schema: &Schema, t: &Table, column: &str) -> Vec<String> {
    let mut out = vec![];
    if t.primary_key.iter().any(|c| c == column) {
        out.push("pk".to_string());
    }
    for fk in &t.foreign_keys {
        if let Some(k) = fk.columns.iter().position(|c| c == column) {
            let target = schema
                .tables
                .iter()
                .find(|o| o.schema == fk.ref_schema && o.name == fk.ref_table)
                .map(|o| table_ref(schema, o))
                .unwrap_or_else(|| ident(&fk.ref_table));
            let col = fk.ref_columns.get(k).map(|c| ident(c)).unwrap_or_default();
            out.push(format!("→ {target}.{col}"));
        }
    }
    out
}

/// Postgres's long type names, as people write them.
fn short_type(ty: &str) -> &str {
    match ty {
        "character varying" => "varchar",
        "character" => "char",
        "timestamp with time zone" => "timestamptz",
        "timestamp without time zone" => "timestamp",
        "time with time zone" => "timetz",
        "time without time zone" => "time",
        "double precision" => "float8",
        "USER-DEFINED" => "enum",
        "ARRAY" => "array",
        other => other,
    }
}

impl Browser {
    /// Open the search path's schemas the first time there's a schema.
    fn seed(&mut self, schema: &Schema) {
        if self.seeded || !schema.loaded {
            return;
        }
        self.seeded = true;
        for s in &schema.search_path {
            self.expanded.insert(format!("s\0{s}"));
        }
    }

    fn rows(&mut self, schema: &Schema) -> Vec<Row> {
        self.seed(schema);
        let rows = rows(schema, &self.expanded, &self.filter);
        self.cursor = self.cursor.min(rows.len().saturating_sub(1));
        rows
    }

    fn set_filter(&mut self, schema: &Schema, f: String) {
        self.filter = f;
        let rows = rows(schema, &self.expanded, &self.filter);
        self.cursor = rows
            .iter()
            .position(|r| matches!(r.node, Node::Table(_)))
            .unwrap_or(0);
        self.top = 0;
        self.follow = true;
    }

    fn toggle(&mut self, schema: &Schema, row: &Row, open: bool) {
        if let Some(k) = key(schema, &row.node) {
            if open {
                self.expanded.insert(k);
            } else {
                self.expanded.remove(&k);
            }
        }
    }

    fn select_sql(schema: &Schema, i: usize) -> Act {
        Act::Open(format!("select * from {};", table_ref(schema, &schema.tables[i])))
    }

    fn copy(schema: &Schema, row: &Row) -> Act {
        let name = match &row.node {
            Node::Schema(s) => ident(s),
            Node::Table(i) => table_ref(schema, &schema.tables[*i]),
            Node::Column(i, c) => ident(&schema.tables[*i].columns[*c].0),
            Node::Indexes(_) => return Act::Nothing,
            Node::Index(i, x) => ident(&schema.tables[*i].indexes[*x].name),
        };
        Act::Copy(name.clone(), format!("Copied {name}"))
    }

    fn move_by(&mut self, n: usize, d: isize) {
        self.cursor = (self.cursor as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
        self.follow = true;
    }

    /// The text this view's ⌘C copies.
    pub fn copy_selected(&mut self, schema: &Schema) -> Option<String> {
        let rows = self.rows(schema);
        match Self::copy(schema, rows.get(self.cursor)?) {
            Act::Copy(text, _) => Some(text),
            _ => None,
        }
    }

    pub fn paste(&mut self, schema: &Schema, text: &str) {
        if self.typing {
            let f = format!("{}{}", self.filter, text.trim());
            self.set_filter(schema, f);
        }
    }

    /// A key while the browser has focus; None if it didn't use it.
    pub fn key(&mut self, ks: &Keystroke, schema: &Schema) -> Option<Act> {
        let rows = self.rows(schema);
        let m = &ks.modifiers;
        let n = rows.len();
        if self.typing {
            match ks.key.as_str() {
                "escape" => {
                    self.typing = false;
                    self.set_filter(schema, String::new());
                }
                "enter" => self.typing = false,
                "backspace" if m.platform || m.alt => self.set_filter(schema, String::new()),
                "backspace" => {
                    let mut f = self.filter.clone();
                    f.pop();
                    self.set_filter(schema, f);
                }
                "down" => self.move_by(n, 1),
                "n" if m.control => self.move_by(n, 1),
                "up" => self.move_by(n, -1),
                "p" if m.control => self.move_by(n, -1),
                _ if m.platform || m.control => return None,
                _ => {
                    let typed = ks.key_char.as_deref().filter(|c| !c.chars().any(char::is_control))?;
                    let f = format!("{}{typed}", self.filter);
                    self.set_filter(schema, f);
                }
            }
            return Some(Act::Nothing);
        }
        if m.platform || m.control || m.alt {
            return None;
        }
        if ks.key_char.as_deref() == Some("?") {
            return Some(Act::Shortcuts);
        }
        if ks.key_char.as_deref() == Some("/") {
            self.typing = true;
            return Some(Act::Nothing);
        }
        let row = rows.get(self.cursor).cloned();
        match ks.key.as_str() {
            "j" | "down" => self.move_by(n, 1),
            "k" | "up" => self.move_by(n, -1),
            "g" if m.shift => self.move_by(n, n as isize),
            "g" => self.move_by(n, -(n as isize)),
            "l" | "right" | "enter" | "tab" => {
                let row = row?;
                match (row.open, ks.key.as_str()) {
                    (Some(false), _) => self.toggle(schema, &row, true),
                    (Some(true), "enter" | "tab") => self.toggle(schema, &row, false),
                    (Some(true), _) => self.move_by(n, 1),
                    _ => {}
                }
            }
            "s" => match row?.node {
                Node::Table(i) => return Some(Self::select_sql(schema, i)),
                _ => {}
            },
            "h" | "left" => {
                let row = row?;
                if row.open == Some(true) && (self.filter.is_empty() || row.depth > 0) {
                    self.toggle(schema, &row, false);
                } else if let Some(p) = rows[..self.cursor].iter().rposition(|r| r.depth < row.depth) {
                    self.cursor = p;
                    self.follow = true;
                    let parent = rows[p].clone();
                    self.toggle(schema, &parent, false);
                }
            }
            "y" => return Some(Self::copy(schema, &row?)),
            "escape" if !self.filter.is_empty() => self.set_filter(schema, String::new()),
            "escape" => return Some(Act::Editor),
            _ => return None,
        }
        Some(Act::Nothing)
    }

    /// A click at `pos`, relative to the browser's top-left.
    pub fn mouse_down(&mut self, pos: Point<Pixels>, clicks: usize, schema: &Schema) -> Act {
        let y = f32::from(pos.y);
        if y < self.list_top && self.list_top > ROW_H {
            self.typing = true;
            return Act::Nothing;
        }
        self.typing = false;
        let rows = self.rows(schema);
        let i = self.top + ((y - self.list_top) / ROW_H) as usize;
        let Some(row) = rows.get(i).cloned() else { return Act::Nothing };
        self.cursor = i;
        match (clicks, row.open) {
            // a double-click's first click already toggled it
            (1, Some(open)) => {
                self.toggle(schema, &row, !open);
                Act::Nothing
            }
            _ => Act::Nothing,
        }
    }

    pub fn scroll(&mut self, lines: isize) {
        self.top = (self.top as isize - lines).max(0) as usize;
    }

    pub fn render(
        &mut self,
        schema: &Schema,
        m: &Metrics,
        t: &Theme,
        focused: bool,
        width: Pixels,
        height: Pixels,
    ) -> AnyElement {
        let rows = self.rows(schema);
        let show_filter = self.typing || !self.filter.is_empty();
        self.list_top = if show_filter { FILTER_H } else { 4. };
        let page = ((f32::from(height) - self.list_top) / ROW_H).floor().max(1.) as usize;
        if self.follow {
            if self.cursor < self.top {
                self.top = self.cursor;
            } else if self.cursor >= self.top + page {
                self.top = self.cursor + 1 - page;
            }
            self.follow = false;
        }
        self.top = self.top.min(rows.len().saturating_sub(page));

        let mono = m.font.family.clone();
        let bar = if focused { t.accent } else { with_alpha(t.fg_subtle, 0.5) };
        let fill = with_alpha(t.fg, if focused { 0.07 } else { 0.04 });
        let list = rows.iter().enumerate().skip(self.top).take(page).map(|(i, r)| {
            let selected = i == self.cursor;
            let (name, meta, name_color, weight) = match &r.node {
                Node::Schema(s) => {
                    let n = schema.tables.iter().filter(|x| x.schema == *s).count();
                    (s.clone(), n.to_string(), t.fg_muted, FontWeight::MEDIUM)
                }
                Node::Table(x) => (schema.tables[*x].name.clone(), String::new(), t.fg, FontWeight::NORMAL),
                Node::Column(x, c) => {
                    let tb = &schema.tables[*x];
                    let (col, ty) = &tb.columns[*c];
                    let mut meta = vec![short_type(ty).to_string()];
                    meta.extend(column_meta(schema, tb, col));
                    (col.clone(), meta.join(" "), t.fg_muted, FontWeight::NORMAL)
                }
                Node::Indexes(x) => (
                    "Indexes".to_string(),
                    schema.tables[*x].indexes.len().to_string(),
                    t.fg_subtle,
                    FontWeight::NORMAL,
                ),
                Node::Index(x, ix) => {
                    let idx = &schema.tables[*x].indexes[*ix];
                    let mut meta = idx.columns.join(", ");
                    if idx.primary {
                        meta.push_str(" pk");
                    } else if idx.unique {
                        meta.push_str(" unique");
                    }
                    (idx.name.clone(), meta, t.fg_muted, FontWeight::NORMAL)
                }
            };
            let chevron = r.open.map(|open| {
                icon(
                    if open { Icon::ChevronDown } else { Icon::ChevronRight },
                    11.,
                    if selected { t.fg } else { t.fg_subtle },
                )
            });
            div()
                .relative()
                .h(px(ROW_H))
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .pl(px(8. + INDENT * r.depth as f32))
                .pr(px(10.))
                .when(selected, |d| {
                    d.bg(fill)
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(bar))
                })
                .child(div().w(px(12.)).flex_none().flex().justify_center().children(chevron))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .font_weight(weight)
                        .text_color(if selected { t.fg } else { name_color })
                        .child(name),
                )
                .when(!meta.is_empty(), |d| {
                    d.child(
                        div()
                            .flex_shrink()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.))
                            .text_color(t.fg_subtle)
                            .child(meta),
                    )
                })
        });

        let filter = show_filter.then(|| {
            let caret = div().w(px(1.5)).h(px(13.)).flex_none().bg(t.accent);
            div().h(px(FILTER_H)).flex_none().px(px(8.)).flex().items_center().child(
                div()
                    .flex_1()
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_lg()
                    .bg(gpui::hsla(0., 0., 0., 0.18))
                    .border_1()
                    .border_color(if self.typing { with_alpha(t.accent, 0.6) } else { t.border })
                    .text_size(px(12.))
                    .child(icon(Icon::Search, 11., t.fg_subtle))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(!self.filter.is_empty(), |d| d.text_color(t.fg).child(self.filter.clone()))
                            .when(self.typing, |d| d.child(caret))
                            .when(self.filter.is_empty(), |d| {
                                d.child(div().ml_1().text_color(t.fg_subtle).child("Filter tables"))
                            }),
                    ),
            )
        });

        let body = if !schema.loaded {
            div()
                .px(px(10.))
                .py(px(8.))
                .child(crate::ui::spinner(11., t.fg_muted))
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .when(!show_filter, |d| d.pt(px(4.)))
                .children(list)
                .into_any_element()
        };

        div()
            .w(width)
            .h(height)
            .flex_none()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(t.chrome)
            .border_r_1()
            .border_color(t.border)
            .font_family(mono)
            .children(filter)
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::super::db::{ForeignKey, Index};
    use super::*;

    fn schema() -> Schema {
        let t = |schema: &str, name: &str, cols: &[&str]| Table {
            schema: schema.into(),
            name: name.into(),
            columns: cols.iter().map(|c| (c.to_string(), "integer".to_string())).collect(),
            ..Default::default()
        };
        let mut posts = t("app", "posts", &["id", "user_id"]);
        posts.primary_key = vec!["id".into()];
        posts.foreign_keys = vec![ForeignKey {
            columns: vec!["user_id".into()],
            ref_schema: "app".into(),
            ref_table: "users".into(),
            ref_columns: vec!["id".into()],
        }];
        posts.indexes = vec![Index {
            name: "posts_pkey".into(),
            columns: vec!["id".into()],
            unique: true,
            primary: true,
        }];
        Schema {
            tables: vec![
                t("audit", "events", &["id"]),
                posts,
                t("app", "users", &["id"]),
                t("public", "users", &["id"]),
                t("Odd", "Things", &["id"]),
            ],
            search_path: vec!["app".into(), "public".into()],
            loaded: true,
        }
    }

    fn nodes(rows: &[Row]) -> Vec<Node> {
        rows.iter().map(|r| r.node.clone()).collect()
    }

    #[test]
    fn search_path_schemas_first() {
        assert_eq!(schemas(&schema()), vec!["app", "public", "Odd", "audit"]);
    }

    #[test]
    fn tree_follows_expansion() {
        let s = schema();
        let mut open = HashSet::new();
        assert_eq!(rows(&s, &open, "").len(), 4);
        open.insert("s\0app".to_string());
        open.insert("t\0app\0posts".to_string());
        let got = nodes(&rows(&s, &open, ""));
        assert_eq!(
            got[..6],
            [
                Node::Schema("app".into()),
                Node::Table(1),
                Node::Column(1, 0),
                Node::Column(1, 1),
                Node::Indexes(1),
                Node::Table(2),
            ]
        );
    }

    #[test]
    fn filter_shows_matching_tables_in_open_schemas() {
        let s = schema();
        let got = nodes(&rows(&s, &HashSet::new(), "usr"));
        assert_eq!(
            got,
            vec![
                Node::Schema("app".into()),
                Node::Table(2),
                Node::Schema("public".into()),
                Node::Table(3)
            ]
        );
        let got = nodes(&rows(&s, &HashSet::new(), "aud.ev"));
        assert_eq!(got, vec![Node::Schema("audit".into()), Node::Table(0)]);
    }

    #[test]
    fn fuzzy_is_ordered_subsequence() {
        assert!(fuzzy("oprf", "online_profiles"));
        assert!(fuzzy("USR", "users"));
        assert!(!fuzzy("sru", "users"));
    }

    #[test]
    fn table_refs_qualify_outside_or_shadowed() {
        let s = schema();
        assert_eq!(table_ref(&s, &s.tables[1]), "posts");
        assert_eq!(table_ref(&s, &s.tables[2]), "users");
        // app.users comes first in the search path
        assert_eq!(table_ref(&s, &s.tables[3]), "public.users");
        assert_eq!(table_ref(&s, &s.tables[0]), "audit.events");
        assert_eq!(table_ref(&s, &s.tables[4]), "\"Odd\".\"Things\"");
    }

    #[test]
    fn column_meta_marks_keys() {
        let s = schema();
        let posts = &s.tables[1];
        assert_eq!(column_meta(&s, posts, "id"), vec!["pk"]);
        assert_eq!(column_meta(&s, posts, "user_id"), vec!["→ users.id"]);
    }

    #[test]
    fn enter_toggles_a_table_and_s_selects_from_it() {
        let s = schema();
        let mut b = Browser::default();
        let ks = |k: &str| Keystroke::parse(k).unwrap();
        // app is open from the search path; the cursor starts on it
        b.key(&ks("j"), &s);
        b.key(&ks("enter"), &s);
        assert!(b.expanded.contains("t\0app\0posts"));
        b.key(&ks("enter"), &s);
        assert!(!b.expanded.contains("t\0app\0posts"));
        b.key(&ks("tab"), &s);
        assert!(b.expanded.contains("t\0app\0posts"));
        b.key(&ks("tab"), &s);
        assert!(!b.expanded.contains("t\0app\0posts"));
        match b.key(&ks("s"), &s) {
            Some(Act::Open(sql)) => assert_eq!(sql, "select * from posts;"),
            _ => panic!("expected Open"),
        }
        // l opens the table, h from a column closes it again
        b.key(&ks("l"), &s);
        b.key(&ks("j"), &s);
        b.key(&ks("h"), &s);
        assert_eq!(b.cursor, 1);
        assert!(!b.expanded.contains("t\0app\0posts"));
    }
}
