//! "Copy as": result rows as CSV, TSV, JSON, a Markdown table, an `insert`
//! statement, or one column's values. Pure functions over the rows to copy
//! (pending edits already applied); `grid.rs` has the menu.

use super::db::{quote_ident, quote_literal, Editable};

/// The table an `insert` names when the rows don't all come from one table.
pub const PLACEHOLDER_TABLE: &str = "table_name";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Csv,
    Tsv,
    Json,
    Markdown,
    Insert,
    Column,
}

impl Format {
    pub const ALL: [Format; 6] = [
        Format::Csv,
        Format::Tsv,
        Format::Json,
        Format::Markdown,
        Format::Insert,
        Format::Column,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Format::Csv => "CSV",
            Format::Tsv => "TSV",
            Format::Json => "JSON",
            Format::Markdown => "Markdown",
            Format::Insert => "SQL insert",
            Format::Column => "Column values",
        }
    }

    /// The letter that picks it in the menu.
    pub fn key(self) -> &'static str {
        match self {
            Format::Csv => "c",
            Format::Tsv => "t",
            Format::Json => "o",
            Format::Markdown => "m",
            Format::Insert => "s",
            Format::Column => "v",
        }
    }
}

/// Rows to copy, values as they appear in the grid.
pub struct Rows<'a> {
    pub columns: &'a [String],
    pub rows: Vec<Vec<Option<String>>>,
}

/// Format `rows`. `col` is the cursor's column (for `Column`), `edit` where
/// the columns come from (for `Insert`).
pub fn format(f: Format, rows: &Rows, col: usize, edit: Option<&Editable>) -> String {
    match f {
        Format::Csv => csv(rows),
        Format::Tsv => tsv(rows),
        Format::Json => json(rows),
        Format::Markdown => markdown(rows),
        Format::Insert => insert(rows, edit),
        Format::Column => column(rows, col),
    }
}

/// RFC 4180 quoting: fields with `,` `"` or a line break are quoted, `"`
/// doubled. NULL is an empty field. Lines end with `\n`.
fn csv_field(v: Option<&str>) -> String {
    let s = v.unwrap_or("");
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn csv(r: &Rows) -> String {
    let mut lines = vec![r.columns.iter().map(|c| csv_field(Some(c))).collect::<Vec<_>>().join(",")];
    for row in &r.rows {
        lines.push(row.iter().map(|v| csv_field(v.as_deref())).collect::<Vec<_>>().join(","));
    }
    lines.join("\n")
}

/// A value on one TSV line: tabs and line breaks become spaces, NULL is empty.
pub fn tsv_value(v: &Option<String>) -> String {
    v.as_deref()
        .unwrap_or("")
        .chars()
        .map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

pub fn tsv(r: &Rows) -> String {
    let mut lines = vec![r.columns.iter().map(|c| tsv_value(&Some(c.clone()))).collect::<Vec<_>>().join("\t")];
    for row in &r.rows {
        lines.push(row.iter().map(tsv_value).collect::<Vec<_>>().join("\t"));
    }
    lines.join("\n")
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// An array of objects keyed by column name, one per line. Values stay
/// strings (Postgres sends text); NULL is `null`.
pub fn json(r: &Rows) -> String {
    if r.rows.is_empty() {
        return "[]".into();
    }
    let objects: Vec<String> = r
        .rows
        .iter()
        .map(|row| {
            let fields: Vec<String> = r
                .columns
                .iter()
                .zip(row)
                .map(|(c, v)| {
                    let v = v.as_deref().map_or("null".to_string(), json_string);
                    format!("{}: {v}", json_string(c))
                })
                .collect();
            format!("  {{{}}}", fields.join(", "))
        })
        .collect();
    format!("[\n{}\n]", objects.join(",\n"))
}

/// `|` escaped, line breaks as `<br>`, NULL empty.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace("\r\n", "<br>").replace(['\n', '\r'], "<br>")
}

pub fn markdown(r: &Rows) -> String {
    let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    let mut lines = vec![
        line(r.columns.iter().map(|c| md_cell(c)).collect()),
        line(r.columns.iter().map(|_| "---".to_string()).collect()),
    ];
    for row in &r.rows {
        lines.push(line(row.iter().map(|v| md_cell(v.as_deref().unwrap_or(""))).collect()));
    }
    lines.join("\n")
}

/// The table to insert into and its column names: the real table when every
/// column comes straight from one, else `PLACEHOLDER_TABLE` and the result's
/// column names.
fn insert_target(columns: &[String], edit: Option<&Editable>) -> (String, Vec<String>) {
    let from_one = edit.and_then(|e| {
        let mut table = None;
        let mut names = vec![];
        for c in 0..columns.len() {
            let (t, name) = e.columns.get(c)?.as_ref()?;
            if *table.get_or_insert(*t) != *t {
                return None;
            }
            names.push(name.clone());
        }
        let t = &e.tables[table?];
        Some((format!("{}.{}", quote_ident(&t.schema), quote_ident(&t.name)), names))
    });
    from_one.unwrap_or_else(|| (PLACEHOLDER_TABLE.to_string(), columns.to_vec()))
}

pub fn insert(r: &Rows, edit: Option<&Editable>) -> String {
    let (table, names) = insert_target(r.columns, edit);
    let cols: Vec<String> = names.iter().map(|n| quote_ident(n)).collect();
    let values: Vec<String> = r
        .rows
        .iter()
        .map(|row| format!("  ({})", row.iter().map(quote_literal).collect::<Vec<_>>().join(", ")))
        .collect();
    format!("insert into {table} ({}) values\n{};", cols.join(", "), values.join(",\n"))
}

/// One value per line, exactly as it is; NULL is an empty line.
pub fn column(r: &Rows, col: usize) -> String {
    r.rows
        .iter()
        .map(|row| row.get(col).cloned().flatten().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::sql::db::EditTable;

    fn cols() -> Vec<String> {
        vec!["id".into(), "note".into()]
    }

    fn rows(columns: &[String]) -> Rows<'_> {
        Rows {
            columns,
            rows: vec![
                vec![Some("1".into()), Some("a, \"b\"".into())],
                vec![Some("2".into()), None],
                vec![Some("3".into()), Some("x|y\nz".into())],
            ],
        }
    }

    #[test]
    fn csv_quotes_only_what_needs_it() {
        let c = cols();
        assert_eq!(csv(&rows(&c)), "id,note\n1,\"a, \"\"b\"\"\"\n2,\n3,\"x|y\nz\"");
    }

    #[test]
    fn tsv_has_a_header_and_flattens() {
        let c = cols();
        assert_eq!(tsv(&rows(&c)), "id\tnote\n1\ta, \"b\"\n2\t\n3\tx|y z");
    }

    #[test]
    fn json_keeps_strings_and_null() {
        let c = cols();
        assert_eq!(
            json(&rows(&c)),
            "[\n  {\"id\": \"1\", \"note\": \"a, \\\"b\\\"\"},\n  {\"id\": \"2\", \"note\": null},\n  {\"id\": \"3\", \"note\": \"x|y\\nz\"}\n]"
        );
        let parsed: serde_json::Value = serde_json::from_str(&json(&rows(&c))).unwrap();
        assert_eq!(parsed[1]["note"], serde_json::Value::Null);
        assert_eq!(json(&Rows { columns: &c, rows: vec![] }), "[]");
    }

    #[test]
    fn markdown_escapes_pipes() {
        let c = cols();
        assert_eq!(
            markdown(&rows(&c)),
            "| id | note |\n| --- | --- |\n| 1 | a, \"b\" |\n| 2 |  |\n| 3 | x\\|y<br>z |"
        );
    }

    #[test]
    fn insert_names_the_table_when_there_is_one() {
        let c = vec!["id".to_string(), "who".to_string()];
        let r = Rows {
            columns: &c,
            rows: vec![vec![Some("1".into()), Some("it's".into())], vec![Some("2".into()), None]],
        };
        let edit = Editable {
            columns: vec![Some((0, "id".into())), Some((0, "firstName".into()))],
            tables: vec![EditTable {
                schema: "public".into(),
                name: "users".into(),
                key: None,
            }],
        };
        assert_eq!(
            insert(&r, Some(&edit)),
            "insert into \"public\".\"users\" (\"id\", \"firstName\") values\n  ('1', 'it''s'),\n  ('2', NULL);"
        );
        assert_eq!(
            insert(&r, None),
            "insert into table_name (\"id\", \"who\") values\n  ('1', 'it''s'),\n  ('2', NULL);"
        );
        // a computed column, or a second table: no single table
        let mixed = Editable {
            columns: vec![Some((0, "id".into())), None],
            ..edit
        };
        assert!(insert(&r, Some(&mixed)).starts_with("insert into table_name "));
    }

    #[test]
    fn column_values_one_per_line() {
        let c = cols();
        assert_eq!(column(&rows(&c), 0), "1\n2\n3");
        assert_eq!(column(&rows(&c), 1), "a, \"b\"\n\nx|y\nz");
    }
}
