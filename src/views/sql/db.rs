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

    while let Ok(job) = rx.recv() {
        match job {
            Job::Query(sql) => {
                let started = Instant::now();
                shared.lock().running = Some(started);
                send_event(AppEvent::Redraw);
                let outcome = run(&mut client, &sql);
                let schema_changed = matches!(outcome, Outcome::Done(_))
                    && sql.to_lowercase().split_whitespace().any(|w| {
                        matches!(w, "create" | "alter" | "drop")
                    });
                {
                    let mut s = shared.lock();
                    s.running = None;
                    s.seq += 1;
                    s.last = Some(Run {
                        outcome,
                        elapsed: started.elapsed(),
                    });
                }
                if schema_changed {
                    if let Ok(schema) = load_schema(&mut client) {
                        shared.lock().schema = Arc::new(schema);
                    }
                }
                send_event(AppEvent::Redraw);
            }
        }
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
