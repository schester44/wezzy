//! Completion for table, column and keyword names, from the loaded schema and
//! the tables the statement already mentions (`u.` → columns of `users u`).

use super::db::{Schema, Table};

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    /// what gets inserted
    pub text: String,
    /// what the menu shows on the right: a column's type, "table", …
    pub detail: String,
}

const KEYWORDS: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "in", "is", "null", "join", "left", "right",
    "inner", "outer", "full", "cross", "on", "as", "group", "by", "order", "having", "limit",
    "offset", "insert", "into", "values", "update", "set", "delete", "returning", "create",
    "table", "alter", "drop", "index", "distinct", "case", "when", "then", "else", "end",
    "count", "sum", "avg", "min", "max", "like", "ilike", "between", "exists", "union", "all",
    "with", "asc", "desc", "true", "false", "coalesce", "now", "explain", "analyze",
];

pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.contains(&word)
}

/// Keywords after which a table name comes next.
const TABLE_CONTEXT: &[&str] = &["from", "join", "update", "into", "table", "describe"];

fn unquote(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}

/// Quote an identifier when Postgres would otherwise fold or reject it.
pub fn ident(s: &str) -> String {
    let plain = s
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if plain {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

fn find_table<'a>(schema: &'a Schema, name: &str) -> Option<&'a Table> {
    let (sch, tbl) = match name.rsplit_once('.') {
        Some((s, t)) => (Some(unquote(s)), unquote(t)),
        None => (None, unquote(name)),
    };
    let matches = |t: &&Table| {
        t.name.to_lowercase() == tbl && sch.as_ref().is_none_or(|s| t.schema.to_lowercase() == *s)
    };
    // an unqualified name resolves through the search path first
    schema
        .tables
        .iter()
        .filter(matches)
        .min_by_key(|t| {
            schema
                .search_path
                .iter()
                .position(|s| *s == t.schema)
                .unwrap_or(usize::MAX)
        })
}

fn tokens(sql: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    for c in sql.chars() {
        if c == '"' {
            quoted = !quoted;
            cur.push(c);
        } else if quoted || c.is_alphanumeric() || c == '_' || c == '.' {
            cur.push(c);
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if c == ',' || c == '(' || c == ')' {
                out.push(c.to_string());
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Tables the statement mentions, with their alias: `from users u join orgs o`.
pub fn table_refs(sql: &str) -> Vec<(String, Option<String>)> {
    let toks = tokens(sql);
    let mut out = vec![];
    let mut i = 0;
    while i < toks.len() {
        let kw = toks[i].to_lowercase();
        let lists = kw == "from"; // from a, b c
        if TABLE_CONTEXT.contains(&kw.as_str()) {
            let mut j = i + 1;
            loop {
                let Some(name) = toks.get(j) else { break };
                if name == "(" || KEYWORDS.contains(&name.to_lowercase().as_str()) {
                    break;
                }
                let mut alias = None;
                let mut k = j + 1;
                if toks.get(k).is_some_and(|t| t.eq_ignore_ascii_case("as")) {
                    k += 1;
                }
                if let Some(a) = toks.get(k) {
                    let al = a.to_lowercase();
                    if a != "," && a != "(" && a != ")" && !KEYWORDS.contains(&al.as_str()) {
                        alias = Some(al);
                        k += 1;
                    }
                }
                out.push((name.clone(), alias));
                if lists && toks.get(k).is_some_and(|t| t == ",") {
                    j = k + 1;
                } else {
                    break;
                }
            }
        }
        i += 1;
    }
    out
}

fn rank(prefix: &str, items: Vec<Candidate>) -> Vec<Candidate> {
    let p = prefix.to_lowercase();
    let bare = |c: &Candidate| c.text.trim_matches('"').to_lowercase();
    let mut starts: Vec<Candidate> = vec![];
    let mut contains: Vec<Candidate> = vec![];
    for c in items {
        let b = bare(&c);
        if b == p {
            continue; // already typed
        }
        if b.starts_with(&p) || b.rsplit('.').next().is_some_and(|l| l.starts_with(&p)) {
            if !starts.iter().any(|s| s.text == c.text) {
                starts.push(c);
            }
        } else if !p.is_empty() && b.contains(&p) && !contains.iter().any(|s| s.text == c.text) {
            contains.push(c);
        }
    }
    starts.extend(contains);
    starts.truncate(50);
    starts
}

/// `before`: the statement up to the cursor. `qualifier`: the word before a
/// `.` (`u` in `u.na`). `prefix`: the identifier being typed.
pub fn complete(
    schema: &Schema,
    statement: &str,
    before: &str,
    qualifier: Option<&str>,
    prefix: &str,
) -> Vec<Candidate> {
    let refs = table_refs(statement);
    let columns_of = |t: &Table| -> Vec<Candidate> {
        t.columns
            .iter()
            .map(|(n, ty)| Candidate {
                text: ident(n),
                detail: ty.clone(),
            })
            .collect()
    };
    let table_candidate = |t: &Table| Candidate {
        text: if schema.search_path.contains(&t.schema) {
            ident(&t.name)
        } else {
            format!("{}.{}", ident(&t.schema), ident(&t.name))
        },
        detail: "table".into(),
    };

    if let Some(q) = qualifier {
        let q = unquote(q);
        let aliased = refs.iter().find_map(|(name, alias)| {
            let hit = alias.as_deref() == Some(q.as_str())
                || unquote(name.rsplit('.').next().unwrap_or(name)) == q;
            hit.then(|| find_table(schema, name)).flatten()
        });
        if let Some(t) = aliased.or_else(|| find_table(schema, &q)) {
            return rank(prefix, columns_of(t));
        }
        // a schema: its tables
        let tables = schema
            .tables
            .iter()
            .filter(|t| t.schema.to_lowercase() == q)
            .map(|t| Candidate {
                text: ident(&t.name),
                detail: "table".into(),
            })
            .collect();
        return rank(prefix, tables);
    }

    if prefix.is_empty() {
        return vec![];
    }
    let prev = tokens(before)
        .into_iter()
        .rev()
        .find(|t| !t.eq_ignore_ascii_case(prefix))
        .map(|t| t.to_lowercase());
    let tables: Vec<Candidate> = schema.tables.iter().map(table_candidate).collect();
    if prev.as_deref().is_some_and(|p| TABLE_CONTEXT.contains(&p)) {
        return rank(prefix, tables);
    }
    let mut items: Vec<Candidate> = refs
        .iter()
        .filter_map(|(name, _)| find_table(schema, name))
        .flat_map(columns_of)
        .collect();
    items.extend(tables);
    let upper = prefix.chars().all(|c| !c.is_lowercase());
    items.extend(KEYWORDS.iter().map(|k| Candidate {
        text: if upper { k.to_uppercase() } else { k.to_string() },
        detail: String::new(),
    }));
    rank(prefix, items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        let t = |schema: &str, name: &str, cols: &[&str]| Table {
            schema: schema.into(),
            name: name.into(),
            columns: cols.iter().map(|c| (c.to_string(), "text".to_string())).collect(),
            ..Default::default()
        };
        Schema {
            tables: vec![
                t("neverwait", "users", &["id", "email", "name"]),
                t("neverwait", "companies", &["id", "name"]),
                t("audit", "events", &["id", "kind"]),
                t("neverwait", "Mixed", &["Id"]),
            ],
            search_path: vec!["neverwait".into()],
            ..Default::default()
        }
    }

    fn texts(c: Vec<Candidate>) -> Vec<String> {
        c.into_iter().map(|c| c.text).collect()
    }

    #[test]
    fn tables_after_from() {
        let s = schema();
        let got = texts(complete(&s, "select * from us", "select * from us", None, "us"));
        assert_eq!(got, vec!["users"]);
        let got = texts(complete(&s, "select * from ev", "select * from ev", None, "ev"));
        assert_eq!(got, vec!["audit.events"]);
    }

    #[test]
    fn columns_through_alias() {
        let s = schema();
        let sql = "select u. from users u join companies c on c.id = u.id";
        let got = texts(complete(&s, sql, "select u.", Some("u"), ""));
        assert_eq!(got, vec!["id", "email", "name"]);
        let got = texts(complete(&s, sql, "select c.na", Some("c"), "na"));
        assert_eq!(got, vec!["name"]);
    }

    #[test]
    fn columns_of_mentioned_tables_come_first() {
        let s = schema();
        let sql = "select em from users";
        let got = texts(complete(&s, sql, "select em", None, "em"));
        assert_eq!(got.first().map(String::as_str), Some("email"));
    }

    #[test]
    fn quotes_mixed_case() {
        let s = schema();
        let got = texts(complete(&s, "select * from mi", "select * from mi", None, "mi"));
        assert_eq!(got, vec!["\"Mixed\""]);
    }

    #[test]
    fn refs_with_lists_and_as() {
        let refs = table_refs("select * from a x, b as y join c on true where 1");
        assert_eq!(
            refs,
            vec![
                ("a".into(), Some("x".into())),
                ("b".into(), Some("y".into())),
                ("c".into(), None)
            ]
        );
    }
}
