//! One worker thread per SQL view: owns the Postgres connection, loads the
//! schema for completion, and runs queries. The view reads `Shared` when it
//! draws; the worker pokes the UI with `AppEvent::Redraw` when it changes.

use crate::app::{send_event, AppEvent};
use parking_lot::Mutex;
use postgres::{CancelToken, Client, SimpleQueryMessage};
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
}

pub struct Run {
    pub outcome: Outcome,
    pub elapsed: Duration,
    /// set when the query is read-only and fetched a page at a time
    pub page: Option<Page>,
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

    pub fn page(&self, offset: usize) {
        self.tx.send(Job::Page(offset)).ok();
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
                        publish(Run {
                            outcome,
                            elapsed: started.elapsed(),
                            page: Some(Page {
                                sql: inner.clone(),
                                offset: 0,
                                has_more,
                                total: (!has_more).then_some(shown as u64),
                            }),
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
                let schema_changed = matches!(outcome, Outcome::Done(_))
                    && sql.to_lowercase().split_whitespace().any(|w| {
                        matches!(w, "create" | "alter" | "drop")
                    });
                publish(Run {
                    outcome,
                    elapsed: started.elapsed(),
                    page: None,
                });
                if schema_changed {
                    if let Ok(schema) = load_schema(&mut client) {
                        shared.lock().schema = Arc::new(schema);
                    }
                }
                send_event(AppEvent::Redraw);
            }
            Job::Page(offset) => {
                let page = shared
                    .lock()
                    .last
                    .as_ref()
                    .and_then(|r| r.page.as_ref())
                    .map(|p| (p.sql.clone(), p.total));
                let Some((sql, total)) = page else {
                    shared.lock().running = None;
                    continue;
                };
                let outcome = fetch_page(&mut client, &sql, offset);
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
                });
            }
        }
    }
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
fn run(client: &mut Client, sql: &str) -> Outcome {
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
