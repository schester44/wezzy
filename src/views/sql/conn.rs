//! Where databases come from: the `databases` setting, and `.env` files in the
//! project you're in (`DATABASE_URL`, or `DB_HOST`/`DB_NAME`/`DB_USER`/
//! `DB_PASS` like a Prisma or Rails app has).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Conn {
    pub name: String,
    pub url: String,
    /// the `.env` it was read from; None = configured
    pub source: Option<PathBuf>,
}

impl Conn {
    /// `host/db`, without user or password, for display.
    pub fn target(&self) -> String {
        let Ok(cfg) = self.url.parse::<postgres::Config>() else {
            return String::new();
        };
        let host = cfg
            .get_hosts()
            .first()
            .map(|h| match h {
                postgres::config::Host::Tcp(h) => h.clone(),
                postgres::config::Host::Unix(p) => p.display().to_string(),
            })
            .unwrap_or_default();
        format!("{host}/{}", cfg.get_dbname().unwrap_or(""))
    }

    /// The view spec args: the name, plus the `.env` to re-read on restart.
    pub fn spec_args(&self) -> String {
        match &self.source {
            Some(p) => format!("{} {}", self.name, p.display()),
            None => self.name.clone(),
        }
    }
}

/// Configured databases, then ones found in the projects of `cwds`, in order.
pub fn all(cwds: &[PathBuf]) -> Vec<Conn> {
    let mut out: Vec<Conn> = crate::settings::get()
        .databases
        .into_iter()
        .map(|d| Conn {
            name: d.name,
            url: d.url,
            source: None,
        })
        .collect();
    for cwd in cwds {
        for c in discover(cwd) {
            if !out.iter().any(|o| o.url == c.url || o.name == c.name) {
                out.push(c);
            }
        }
    }
    out
}

/// Turn view spec args back into a connection.
pub fn resolve(args: &str) -> Option<Conn> {
    let (name, source) = match args.split_once(' ') {
        Some((n, p)) => (n, Some(PathBuf::from(p))),
        None => (args, None),
    };
    match source {
        Some(path) => Some(Conn {
            name: name.into(),
            url: url_from_env(&path)?,
            source: Some(path),
        }),
        None => all(&[]).into_iter().find(|c| c.name == name),
    }
}

fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// `.env` files at the root and two levels down (`apps/api/.env`).
fn env_files(root: &Path) -> Vec<PathBuf> {
    const SKIP: &[&str] = &["node_modules", "target", "dist", "build", "vendor"];
    let mut out = vec![];
    let mut dirs = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        for name in [".env", ".env.local", ".env.development"] {
            let f = dir.join(name);
            if f.is_file() {
                out.push(f);
            }
        }
        if depth == 2 {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with('.') || SKIP.contains(&n.as_str()) {
                continue;
            }
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push((e.path(), depth + 1));
            }
        }
    }
    out.sort();
    out
}

/// Found databases per project root, for a few seconds: the palette asks on
/// every keystroke.
type Cache = HashMap<PathBuf, (std::time::Instant, Vec<Conn>)>;
static CACHE: parking_lot::Mutex<Option<Cache>> = parking_lot::Mutex::new(None);

pub fn discover(cwd: &Path) -> Vec<Conn> {
    let root = project_root(cwd);
    // a shell in ~ isn't in a project; don't walk the home folder
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if root.parent().is_none() || Some(&root) == home.as_ref() {
        return vec![];
    }
    let mut cache = CACHE.lock();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((at, conns)) = cache.get(&root) {
        if at.elapsed() < std::time::Duration::from_secs(10) {
            return conns.clone();
        }
    }
    let conns = scan(&root);
    cache.insert(root, (std::time::Instant::now(), conns.clone()));
    conns
}

fn scan(root: &Path) -> Vec<Conn> {
    let root = root.to_path_buf();
    let project = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "db".into());
    let mut found: Vec<(PathBuf, String)> = vec![];
    for f in env_files(&root) {
        if let Some(url) = url_from_env(&f) {
            if !found.iter().any(|(_, u)| *u == url) {
                found.push((f, url));
            }
        }
    }
    let single = found.len() == 1;
    found
        .into_iter()
        .map(|(f, url)| {
            // one database: the project's name; several: project-<dir>
            let name = if single || f.parent() == Some(root.as_path()) {
                project.clone()
            } else {
                let dir = f
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                format!("{project}-{dir}")
            };
            Conn {
                name,
                url,
                source: Some(f),
            }
        })
        .collect()
}

fn parse_env(text: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let mut v = v.trim();
        if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
            v = v[1..].split(q).next().unwrap_or("");
        } else if let Some(i) = v.find(" #") {
            v = v[..i].trim_end();
        }
        vars.insert(k.trim().to_string(), v.to_string());
    }
    vars
}

fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

pub fn url_from_env(path: &Path) -> Option<String> {
    let vars = parse_env(&std::fs::read_to_string(path).ok()?);
    url_from_vars(&vars)
}

fn url_from_vars(vars: &HashMap<String, String>) -> Option<String> {
    for k in ["DATABASE_URL", "POSTGRES_URL", "DB_URL", "PG_URL"] {
        if let Some(u) = vars.get(k).filter(|u| is_postgres(u)) {
            return Some(u.clone());
        }
    }
    let get = |keys: &[&str]| keys.iter().find_map(|k| vars.get(*k).filter(|v| !v.is_empty()).cloned());
    let db = get(&["DB_NAME", "DB_DATABASE", "PGDATABASE", "POSTGRES_DB"])?;
    let host = get(&["DB_HOST", "PGHOST", "POSTGRES_HOST"]).unwrap_or_else(|| "localhost".into());
    let port = get(&["DB_PORT", "PGPORT", "POSTGRES_PORT"]).unwrap_or_else(|| "5432".into());
    let user = get(&["DB_USER", "DB_USERNAME", "PGUSER", "POSTGRES_USER"]).unwrap_or_default();
    let pass = get(&["DB_PASS", "DB_PASSWORD", "PGPASSWORD", "POSTGRES_PASSWORD"]).unwrap_or_default();
    let auth = match (user.is_empty(), pass.is_empty()) {
        (true, _) => String::new(),
        (false, true) => format!("{}@", encode(&user)),
        (false, false) => format!("{}:{}@", encode(&user), encode(&pass)),
    };
    Some(format!("postgresql://{auth}{host}:{port}/{}", encode(&db)))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_from_db_vars() {
        let vars = parse_env("DB_HOST=db.local\nDB_NAME=app\nDB_USER=me\nDB_PASS=\"p@ss w\"\n# DB_PORT=1\n");
        assert_eq!(
            url_from_vars(&vars).as_deref(),
            Some("postgresql://me:p%40ss%20w@db.local:5432/app")
        );
    }

    #[test]
    fn database_url_wins() {
        let vars = parse_env("export DATABASE_URL='postgres://a@b/c'\nDB_NAME=x");
        assert_eq!(url_from_vars(&vars).as_deref(), Some("postgres://a@b/c"));
    }

    #[test]
    fn non_postgres_url_is_ignored() {
        let vars = parse_env("DATABASE_URL=mysql://a@b/c");
        assert_eq!(url_from_vars(&vars), None);
    }
}
