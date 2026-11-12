//! One worker thread per SQL view: owns the Postgres connection, loads the
//! schema for completion, and runs queries. The view reads `Shared` when it
//! draws; the worker pokes the UI with `AppEvent::Redraw` when it changes.

use crate::app::{send_event, AppEvent};
use parking_lot::Mutex;
use postgres::{CancelToken, Client, SimpleQueryMessage};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Rows kept per result; the count goes on past it.
const MAX_ROWS: usize = 10_000;

#[derive(Clone, Debug)]
pub struct Table {
    pub schema: String,
    pub name: String,
    /// (name, type)
    pub columns: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Schema {
    pub tables: Vec<Table>,
    /// schemas whose tables don't need qualifying
    pub search_path: Vec<String>,
}

pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// rows the query returned, including any past MAX_ROWS
    pub total: usize,
}

pub enum Outcome {
    Rows(ResultSet),
    /// a statement without rows: "3 rows affected"
    Done(u64),
    Error(String),
    /// `explain (format json …)` output
    Plan(super::explain::Plan),
}

pub struct Run {
    pub outcome: Outcome,
    pub elapsed: Duration,
    /// set when the query is read-only and fetched a page at a time
    pub page: Option<Page>,
    /// which cells can be changed, for a paged result
    pub edit: Option<Arc<Editable>>,
}

/// Where a result's columns come from, as Postgres reports them, so a cell
/// can be written back with `update … where <primary key> = …`.
#[derive(Debug, Default)]
pub struct Editable {
    /// per result column: (index into `tables`, the column's real name)
    pub columns: Vec<Option<(usize, String)>>,
    pub tables: Vec<EditTable>,
}

#[derive(Debug)]
pub struct EditTable {
    pub schema: String,
    pub name: String,
    /// primary key columns: (result column, name); None if the result lacks
    /// one of them (or the table has no primary key)
    pub key: Option<Vec<(usize, String)>>,
}

impl Editable {
    /// Why column `c` can't be edited, if it can't.
    pub fn check(&self, c: usize) -> Result<(), &'static str> {
        let Some(Some((t, _))) = self.columns.get(c) else {
            return Err("Not a table column");
        };
        match &self.tables[*t].key {
            Some(_) => Ok(()),
            None => Err("Needs the table's primary key in the result"),
        }
    }
}

pub(super) fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub(super) fn quote_literal(v: &Option<String>) -> String {
    match v {
        Some(s) => format!("'{}'", s.replace('\'', "''")),
        None => "NULL".into(),
    }
}

/// One `update` per changed (table, row). `changes`: (row, column) → new value.
pub fn update_sql(
    edit: &Editable,
    set: &ResultSet,
    changes: &BTreeMap<(usize, usize), Option<String>>,
) -> Result<Vec<String>, String> {
    let mut by_row: BTreeMap<(usize, usize), Vec<(String, &Option<String>)>> = BTreeMap::new();
    for ((row, col), value) in changes {
        let Some(Some((t, name))) = edit.columns.get(*col) else {
            return Err("Not a table column".into());
        };
        by_row.entry((*t, *row)).or_default().push((name.clone(), value));
    }
    let mut out = vec![];
    for ((t, row), sets) in by_row {
        let table = &edit.tables[t];
        let key = table.key.as_ref().ok_or("Needs the table's primary key in the result")?;
        let mut cond = vec![];
        for (c, name) in key {
            match set.rows.get(row).and_then(|r| r.get(*c)).cloned().flatten() {
                Some(v) => cond.push(format!("{} = {}", quote_ident(name), quote_literal(&Some(v)))),
                None => return Err(format!("Row {} has no {name}", row + 1)),
            }
        }
        let assigns: Vec<String> = sets
            .iter()
            .map(|(n, v)| format!("{} = {}", quote_ident(n), quote_literal(v)))
            .collect();
        out.push(format!(
            "update {}.{} set {} where {}",
            quote_ident(&table.schema),
            quote_ident(&table.name),
            assigns.join(", "),
            cond.join(" and ")
        ));
    }
    Ok(out)
}

/// Ask Postgres where each column of `sql`'s result comes from.
fn editable(client: &mut Client, sql: &str) -> Option<Editable> {
    let stmt = client
        .prepare(&format!("select * from (\n{sql}\n) as _wezzy_page"))
        .ok()?;
    let origins: Vec<Option<(u32, i16)>> = stmt
        .columns()
        .iter()
        .map(|c| match (c.table_oid(), c.column_id()) {
            (Some(t), Some(a)) if t != 0 && a > 0 => Some((t, a)),
            _ => None,
        })
        .collect();
    let mut oids: Vec<u32> = origins.iter().flatten().map(|(t, _)| *t).collect();
    oids.sort();
    oids.dedup();
    if oids.is_empty() {
        return None;
    }
    let rows = client
        .query(
            "select c.oid, n.nspname::text, c.relname::text, a.attnum, a.attname::text,
                    coalesce(a.attnum = any(i.indkey), false)
             from pg_class c
             join pg_namespace n on n.oid = c.relnamespace
             join pg_attribute a on a.attrelid = c.oid and a.attnum > 0 and not a.attisdropped
             left join pg_index i on i.indrelid = c.oid and i.indisprimary
             where c.oid = any($1) and c.relkind in ('r', 'p')",
            &[&oids],
        )
        .ok()?;
    // oid → (schema, name, attnum → (name, is_pk))
    let mut info: BTreeMap<u32, (String, String, BTreeMap<i16, (String, bool)>)> = BTreeMap::new();
    for r in rows {
        let oid: u32 = r.get(0);
        let e = info.entry(oid).or_insert_with(|| (r.get(1), r.get(2), BTreeMap::new()));
        e.2.insert(r.get(3), (r.get(4), r.get(5)));
    }
    let mut ed = Editable::default();
    let mut table_idx: BTreeMap<u32, usize> = BTreeMap::new();
    for (oid, (schema, name, attrs)) in &info {
        let pk: Vec<i16> = attrs.iter().filter(|(_, (_, pk))| *pk).map(|(a, _)| *a).collect();
        let key: Option<Vec<(usize, String)>> = (!pk.is_empty())
            .then(|| {
                pk.iter()
                    .map(|a| {
                        let c = origins.iter().position(|o| *o == Some((*oid, *a)))?;
                        Some((c, attrs[a].0.clone()))
                    })
                    .collect()
            })
            .flatten();
        table_idx.insert(*oid, ed.tables.len());
        ed.tables.push(EditTable {
            schema: schema.clone(),
            name: name.clone(),
            key,
        });
    }
    ed.columns = origins
        .iter()
        .map(|o| {
            let (oid, a) = (*o)?;
            let t = *table_idx.get(&oid)?;
            Some((t, info[&oid].2.get(&a)?.0.clone()))
        })
        .collect();
    Some(ed)
}

/// Rows per page.
pub const PAGE: usize = 500;

pub struct Page {
    /// the query as typed, wrapped for each page
    sql: String,
    /// index of the first row shown
    pub offset: usize,
    pub has_more: bool,
    /// rows in all, once counted (gives up after a few seconds)
    pub total: Option<u64>,
}

pub enum Status {
    Connecting,
    Ready,
    Failed(String),
}

pub struct Shared {
    pub status: Status,
    pub schema: Arc<Schema>,
    pub last: Option<Run>,
    /// bumped with every new `last`
    pub seq: u64,
    /// when the running query started
    pub running: Option<Instant>,
    cancel: Option<CancelToken>,
}

enum Job {
    Query(String),
    /// another page of the last result, from this row
    Page(usize),
    /// run these updates in one transaction, then reload the page
    Save(Vec<String>),
    /// explain one statement; with ANALYZE when `true`
    Explain(String, bool),
}

pub struct Db {
    pub shared: Arc<Mutex<Shared>>,
    tx: mpsc::Sender<Job>,
    verify_tls: bool,
}

fn tls(verify: bool) -> postgres_native_tls::MakeTlsConnector {
    // sslmode=require doesn't verify certificates in libpq either; verify-*
    // modes do
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(!verify)
        .danger_accept_invalid_hostnames(!verify)
        .build()
        .expect("tls connector");
    postgres_native_tls::MakeTlsConnector::new(connector)
}

fn error_text(e: &postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => match db.hint() {
            Some(h) => format!("{} ({h})", db.message()),
            None => db.message().to_string(),
        },
        None => {
            let s = e.to_string();
            match std::error::Error::source(e) {
                Some(src) => format!("{s}: {src}"),
                None => s,
            }
        }
    }
}

impl Db {
    pub fn connect(url: String) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            status: Status::Connecting,
            schema: Arc::new(Schema::default()),
            last: None,
            seq: 0,
            running: None,
            cancel: None,
        }));
        let verify_tls = url.contains("sslmode=verify");
        let (tx, rx) = mpsc::channel();
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("sql".into())
            .spawn(move || worker(url, verify_tls, sh, rx))
            .ok();
        Self { shared, tx, verify_tls }
    }

    pub fn run(&self, sql: String) {
        self.tx.send(Job::Query(sql)).ok();
    }

    pub fn explain(&self, sql: String, analyze: bool) {
        self.tx.send(Job::Explain(sql, analyze)).ok();
    }

    pub fn page(&self, offset: usize) {
        self.tx.send(Job::Page(offset)).ok();
    }

    pub fn save(&self, updates: Vec<String>) {
        self.tx.send(Job::Save(updates)).ok();
    }

    pub fn cancel(&self) {
        let token = self.shared.lock().cancel.clone();
        if let Some(token) = token {
            let verify = self.verify_tls;
            std::thread::spawn(move || {
                token.cancel_query(tls(verify)).ok();
            });
        }
    }
}

fn worker(url: String, verify: bool, shared: Arc<Mutex<Shared>>, rx: mpsc::Receiver<Job>) {
    let connected = url
        .parse::<postgres::Config>()
        .and_then(|mut cfg| {
            if cfg.get_connect_timeout().is_none() {
                cfg.connect_timeout(Duration::from_secs(5));
            }
            cfg.application_name("wezzy");
            cfg.connect(tls(verify))
        });
    let mut client = match connected {
        Ok(c) => c,
        Err(e) => {
            shared.lock().status = Status::Failed(error_text(&e));
            send_event(AppEvent::Redraw);
            return;
        }
    };
    {
        let mut s = shared.lock();
        s.status = Status::Ready;
        s.cancel = Some(client.cancel_token());
    }
    send_event(AppEvent::Redraw);
    match load_schema(&mut client) {
        Ok(schema) => shared.lock().schema = Arc::new(schema),
        Err(e) => log::warn!("sql: schema: {}", error_text(&e)),
    }
    send_event(AppEvent::Redraw);

    let publish = |run: Run| {
        let mut s = shared.lock();
        s.running = None;
        s.seq += 1;
        s.last = Some(run);
        drop(s);
        send_event(AppEvent::Redraw);
    };
    while let Ok(job) = rx.recv() {
        let started = Instant::now();
        shared.lock().running = Some(started);
        send_event(AppEvent::Redraw);
        match job {
            Job::Query(sql) => {
                if let Some(inner) = pageable(&sql) {
                    let outcome = fetch_page(&mut client, &inner, 0);
                    // the wrapped query can fail where the plain one wouldn't;
                    // fall back to running it as typed
                    if !matches!(outcome, Outcome::Error(_)) {
                        let (has_more, shown) = match &outcome {
                            Outcome::Rows(set) => (set.total > PAGE, set.rows.len()),
                            _ => (false, 0),
                        };
                        let edit = matches!(outcome, Outcome::Rows(_))
                            .then(|| editable(&mut client, &inner))
                            .flatten()
                            .map(Arc::new);
                        publish(Run {
                            outcome,
                            elapsed: started.elapsed(),
                            page: Some(Page {
                                sql: inner.clone(),
                                offset: 0,
                                has_more,
                                total: (!has_more).then_some(shown as u64),
                            }),
                            edit,
                        });
                        if has_more {
                            let total = count(&mut client, &inner);
                            if let Some(Some(p)) = shared.lock().last.as_mut().map(|r| r.page.as_mut()) {
                                if p.sql == inner {
                                    p.total = total;
                                }
                            }
                            send_event(AppEvent::Redraw);
                        }
                        continue;
                    }
                }
                let outcome = run(&mut client, &sql);
                let outcome = if starts_with_word(&sql, "explain") { as_plan(outcome) } else { outcome };
                let schema_changed = matches!(outcome, Outcome::Done(_))
                    && sql.to_lowercase().split_whitespace().any(|w| {
                        matches!(w, "create" | "alter" | "drop")
                    });
                publish(Run {
                    outcome,
                    elapsed: started.elapsed(),
                    page: None,
                    edit: None,
                });
                if schema_changed {
                    if let Ok(schema) = load_schema(&mut client) {
                        shared.lock().schema = Arc::new(schema);
                    }
                }
                send_event(AppEvent::Redraw);
            }
            Job::Explain(sql, analyze) => {
                let outcome = explain(&mut client, &sql, analyze);
                publish(Run {
                    outcome,
                    elapsed: started.elapsed(),
                    page: None,
                    edit: None,
                });
            }
            Job::Page(offset) => reload(&mut client, &shared, offset, started, &publish),
            Job::Save(updates) => {
                let saved = (|| -> Result<(), String> {
                    let mut tx = client.transaction().map_err(|e| error_text(&e))?;
                    for u in &updates {
                        let n = tx.execute(u.as_str(), &[]).map_err(|e| error_text(&e))?;
                        if n == 0 {
                            return Err("A row you changed is gone; nothing was saved".into());
                        }
                    }
                    tx.commit().map_err(|e| error_text(&e))
                })();
                match saved {
                    Ok(()) => {
                        let n = updates.len();
                        send_event(AppEvent::Flash(format!(
                            "Saved {n} row{}",
                            if n == 1 { "" } else { "s" }
                        )));
                        let offset = shared
                            .lock()
                            .last
                            .as_ref()
                            .and_then(|r| r.page.as_ref())
                            .map_or(0, |p| p.offset);
                        reload(&mut client, &shared, offset, started, &publish);
                    }
                    Err(e) => {
                        shared.lock().running = None;
                        send_event(AppEvent::Flash(e));
                    }
                }
            }
        }
    }
}

/// Fetch the last result's query again from row `offset`, keeping its count
/// and what's editable.
fn reload(
    client: &mut Client,
    shared: &Mutex<Shared>,
    offset: usize,
    started: Instant,
    publish: &dyn Fn(Run),
) {
    let page = {
        let s = shared.lock();
        s.last.as_ref().and_then(|r| {
            let p = r.page.as_ref()?;
            Some((p.sql.clone(), p.total, r.edit.clone()))
        })
    };
    let Some((sql, total, edit)) = page else {
        shared.lock().running = None;
        send_event(AppEvent::Redraw);
        return;
    };
    let outcome = fetch_page(client, &sql, offset);
    let has_more = matches!(&outcome, Outcome::Rows(set) if set.total > PAGE);
    publish(Run {
        outcome,
        elapsed: started.elapsed(),
        page: Some(Page {
            sql,
            offset,
            has_more,
            total,
        }),
        edit,
    });
}

/// One statement only reads, so EXPLAIN ANALYZE can run it.
pub fn read_only(sql: &str) -> bool {
    pageable(sql).is_some()
}

fn starts_with_word(sql: &str, word: &str) -> bool {
    sql.trim_start()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case(word))
}

/// A one-cell result that holds a JSON plan, as a plan.
fn as_plan(outcome: Outcome) -> Outcome {
    if let Outcome::Rows(set) = &outcome {
        if let ([_], [row]) = (set.columns.as_slice(), set.rows.as_slice()) {
            if let Some(plan) = row.first().cloned().flatten().and_then(|raw| super::explain::parse(&raw)) {
                return Outcome::Plan(plan);
            }
        }
    }
    outcome
}

/// `explain (format json)`, or with `analyze, buffers`, in a read-only
/// transaction that's rolled back. Preparing it first refuses more than one
/// statement, so nothing after a `;` can run.
fn explain(client: &mut Client, sql: &str, analyze: bool) -> Outcome {
    let options = if analyze { "analyze, buffers, format json" } else { "format json" };
    let q = format!("explain ({options}) {}", sql.trim().trim_end_matches(';'));
    let mut tx = match client.build_transaction().read_only(true).start() {
        Ok(tx) => tx,
        Err(e) => return Outcome::Error(error_text(&e)),
    };
    if let Err(e) = tx.prepare(&q) {
        return Outcome::Error(error_text(&e));
    }
    let outcome = as_plan(run(&mut tx, &q));
    tx.rollback().ok();
    outcome
}

/// The query, ready to wrap in `select * from (…)`, if it only reads and is
/// one statement.
fn pageable(sql: &str) -> Option<String> {
    let q = sql.trim().trim_end_matches(';').trim_end();
    if q.contains(';') {
        return None;
    }
    let lower = q.to_lowercase();
    let first = lower.split_whitespace().next()?;
    let writes = lower.split(|c: char| !c.is_alphanumeric() && c != '_').any(|w| {
        matches!(w, "insert" | "update" | "delete" | "merge" | "into" | "for")
    });
    (matches!(first, "select" | "with" | "table" | "values") && !writes).then(|| q.to_string())
}

/// One page, plus one row to tell whether there's another. `total` on the
/// result is the rows fetched (so `> PAGE` means more).
fn fetch_page(client: &mut Client, sql: &str, offset: usize) -> Outcome {
    let wrapped = format!(
        "select * from (\n{sql}\n) as _wezzy_page limit {} offset {offset}",
        PAGE + 1
    );
    match run(client, &wrapped) {
        Outcome::Rows(mut set) => {
            set.rows.truncate(PAGE);
            Outcome::Rows(set)
        }
        other => other,
    }
}

/// How many rows the query returns in all, unless counting takes over 3s.
fn count(client: &mut Client, sql: &str) -> Option<u64> {
    let q = format!(
        "set local statement_timeout = '3s'; select count(*) from (\n{sql}\n) as _wezzy_count"
    );
    match run(client, &q) {
        Outcome::Rows(set) => set.rows.first()?.first()?.as_ref()?.parse().ok(),
        _ => None,
    }
}

/// Runs everything in `sql`; the result is the last statement's.
fn run(client: &mut impl postgres::GenericClient, sql: &str) -> Outcome {
    let messages = match client.simple_query(sql) {
        Ok(m) => m,
        Err(e) => return Outcome::Error(error_text(&e)),
    };
    let mut last = Outcome::Done(0);
    let mut set: Option<ResultSet> = None;
    for m in messages {
        match m {
            SimpleQueryMessage::RowDescription(cols) => {
                set = Some(ResultSet {
                    columns: cols.iter().map(|c| c.name().to_string()).collect(),
                    rows: vec![],
                    total: 0,
                });
            }
            SimpleQueryMessage::Row(row) => {
                let set = set.get_or_insert_with(|| ResultSet {
                    columns: row.columns().iter().map(|c| c.name().to_string()).collect(),
                    rows: vec![],
                    total: 0,
                });
                set.total += 1;
                if set.rows.len() < MAX_ROWS {
                    set.rows
                        .push((0..row.len()).map(|i| row.get(i).map(str::to_string)).collect());
                }
            }
            SimpleQueryMessage::CommandComplete(n) => {
                last = match set.take() {
                    Some(s) => Outcome::Rows(s),
                    None => Outcome::Done(n),
                };
            }
            _ => {}
        }
    }
    last
}

fn load_schema(client: &mut Client) -> Result<Schema, postgres::Error> {
    let search_path = client
        .query("select unnest(current_schemas(false))::text", &[])?
        .iter()
        .map(|r| r.get::<_, String>(0))
        .collect();
    let rows = client.query(
        "select table_schema::text, table_name::text, column_name::text, data_type::text
         from information_schema.columns
         where table_schema not in ('pg_catalog', 'information_schema')
           and table_schema not like 'pg_toast%'
         order by table_schema, table_name, ordinal_position",
        &[],
    )?;
    let mut tables: Vec<Table> = vec![];
    for r in rows {
        let (schema, name): (String, String) = (r.get(0), r.get(1));
        let col = (r.get::<_, String>(2), r.get::<_, String>(3));
        match tables.last_mut() {
            Some(t) if t.schema == schema && t.name == name => t.columns.push(col),
            _ => tables.push(Table {
                schema,
                name,
                columns: vec![col],
            }),
        }
    }
    Ok(Schema {
        tables,
        search_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_only_for_reads_and_plans_detected() {
        assert!(read_only("select * from users where id = 1;"));
        assert!(read_only("with x as (select 1) select * from x"));
        assert!(!read_only("delete from users"));
        assert!(!read_only("with d as (delete from users returning *) select * from d"));
        assert!(!read_only("select 1; delete from users"));
        assert!(!read_only("select * from users for update"));
        assert!(starts_with_word("  EXPLAIN (format json) select 1", "explain"));
        assert!(!starts_with_word("explained", "explain"));
        let cell = |v: &str| {
            Outcome::Rows(ResultSet {
                columns: vec!["QUERY PLAN".into()],
                rows: vec![vec![Some(v.into())]],
                total: 1,
            })
        };
        let json = r#"[{"Plan": {"Node Type": "Result", "Total Cost": 0.01, "Plan Rows": 1}}]"#;
        assert!(matches!(as_plan(cell(json)), Outcome::Plan(_)));
        assert!(matches!(as_plan(cell("Result  (cost=0.00..0.01 rows=1 width=4)")), Outcome::Rows(_)));
    }

    #[test]
    fn updates_group_by_row_and_quote() {
        let edit = Editable {
            columns: vec![Some((0, "id".into())), Some((0, "firstName".into())), Some((0, "note".into()))],
            tables: vec![EditTable {
                schema: "neverwait".into(),
                name: "online_profiles".into(),
                key: Some(vec![(0, "id".into())]),
            }],
        };
        let set = ResultSet {
            columns: vec!["id".into(), "firstName".into(), "note".into()],
            rows: vec![vec![Some("3".into()), Some("a".into()), None]],
            total: 1,
        };
        let changes: BTreeMap<_, _> = [((0, 1), Some("it's".to_string())), ((0, 2), None)].into_iter().collect();
        assert_eq!(
            update_sql(&edit, &set, &changes).unwrap(),
            vec![r#"update "neverwait"."online_profiles" set "firstName" = 'it''s', "note" = NULL where "id" = '3'"#]
        );
    }
}
