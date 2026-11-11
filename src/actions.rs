//! GitHub Actions runs for the active pane's repo and branch: the "Actions"
//! sidebar source (opt-in, see sidebar.rs).
//!
//! Everything goes through the `gh` CLI, so its login (and GitHub Enterprise
//! hosts) apply and we never hold a token. A background thread polls while the
//! sidebar is open with Actions in it (`set_target`): every 10s while a run is
//! queued or in progress, every 60s otherwise. A repo `gh` can't resolve (not
//! on GitHub, gh missing or logged out) is retried every few minutes.

use crate::app::{send_event, AppEvent};
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ACTIVE_EVERY: Duration = Duration::from_secs(10);
const IDLE_EVERY: Duration = Duration::from_secs(60);
const UNRESOLVED_RETRY: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Queued,
    Running,
    /// Waiting on an approval or a manual step.
    Waiting,
    Success,
    Failure,
    /// Cancelled, skipped, neutral, stale.
    Other,
}

impl State {
    pub fn active(self) -> bool {
        matches!(self, State::Queued | State::Running | State::Waiting)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub id: u64,
    pub workflow: String,
    pub state: State,
    pub url: String,
    /// `owner/repo`, for `gh -R`.
    pub repo: String,
    /// Unix seconds.
    pub created: i64,
}

impl Run {
    /// The command ↵ runs: follow a live run, or show what failed.
    pub fn view_command(&self) -> Vec<String> {
        let id = self.id.to_string();
        let mut argv: Vec<String> = match self.state {
            s if s.active() => vec!["gh".into(), "run".into(), "watch".into(), id],
            State::Failure => vec!["gh".into(), "run".into(), "view".into(), id, "--log-failed".into()],
            _ => vec!["gh".into(), "run".into(), "view".into(), id],
        };
        argv.extend(["-R".into(), self.repo.clone()]);
        argv
    }
}

/// Repo root + branch (or commit, when detached) the runs belong to.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key {
    root: PathBuf,
    rev: Rev,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Rev {
    Branch(String),
    Commit(String),
}

static TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Runs for the current target, newest first, one per workflow.
static RUNS: Mutex<Vec<Run>> = Mutex::new(Vec::new());

/// Directory to show runs for; `None` stops polling.
pub fn set_target(cwd: Option<PathBuf>) {
    let mut t = TARGET.lock();
    if *t != cwd {
        *t = cwd;
    }
}

pub fn runs() -> Vec<Run> {
    if TARGET.lock().is_none() {
        return vec![];
    }
    RUNS.lock().clone()
}

fn publish(runs: Vec<Run>) {
    let mut cur = RUNS.lock();
    if *cur != runs {
        *cur = runs;
        drop(cur);
        send_event(AppEvent::Redraw);
    }
}

pub fn start() {
    std::thread::spawn(|| {
        // owner/repo per repo root; None = gh couldn't resolve it (retry later)
        let mut repos: HashMap<PathBuf, (Option<String>, Instant)> = HashMap::new();
        // last fetch per key, so switching back shows runs right away
        let mut cache: HashMap<Key, (Vec<Run>, Instant)> = HashMap::new();
        let mut resolved: Option<(PathBuf, Option<Key>, Instant)> = None;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let Some(cwd) = TARGET.lock().clone() else {
                resolved = None;
                continue;
            };
            // where HEAD is: re-read every few seconds (checkouts, cd)
            let stale = match &resolved {
                Some((c, _, at)) => *c != cwd || at.elapsed() > Duration::from_secs(3),
                None => true,
            };
            if stale {
                resolved = Some((cwd.clone(), key_for(&cwd), Instant::now()));
            }
            let Some(key) = resolved.as_ref().and_then(|(_, k, _)| k.clone()) else {
                publish(vec![]);
                continue;
            };

            let repo = match repos.get(&key.root) {
                Some((r, at)) if r.is_some() || at.elapsed() < UNRESOLVED_RETRY => r.clone(),
                _ => {
                    let r = repo_name(&key.root);
                    repos.insert(key.root.clone(), (r.clone(), Instant::now()));
                    r
                }
            };
            let Some(repo) = repo else {
                publish(vec![]);
                continue;
            };

            let due = match cache.get(&key) {
                Some((runs, at)) => {
                    publish(runs.clone());
                    let every = if runs.iter().any(|r| r.state.active()) {
                        ACTIVE_EVERY
                    } else {
                        IDLE_EVERY
                    };
                    at.elapsed() >= every
                }
                None => {
                    publish(vec![]);
                    true
                }
            };
            if !due {
                continue;
            }
            // on failure keep what we had, and try again next interval
            let runs = list_runs(&repo, &key.rev)
                .or_else(|| cache.get(&key).map(|(r, _)| r.clone()))
                .unwrap_or_default();
            if TARGET.lock().as_deref() == Some(cwd.as_path()) {
                publish(runs.clone());
            }
            cache.insert(key, (runs, Instant::now()));
        }
    });
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn key_for(cwd: &Path) -> Option<Key> {
    let root = PathBuf::from(git(cwd, &["rev-parse", "--show-toplevel"])?);
    let rev = match git(&root, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        Some(b) if !b.is_empty() => Rev::Branch(b),
        _ => Rev::Commit(git(&root, &["rev-parse", "HEAD"])?),
    };
    Some(Key { root, rev })
}

fn gh(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("gh")
        .current_dir(dir)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// `owner/repo` for the repo's GitHub remote, if gh can see it.
fn repo_name(root: &Path) -> Option<String> {
    let name = gh(root, &["repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"])?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRun {
    database_id: u64,
    workflow_name: String,
    status: String,
    conclusion: String,
    url: String,
    created_at: String,
}

fn list_runs(repo: &str, rev: &Rev) -> Option<Vec<Run>> {
    let (flag, value) = match rev {
        Rev::Branch(b) => ("--branch", b.as_str()),
        Rev::Commit(c) => ("--commit", c.as_str()),
    };
    let json = gh(
        Path::new("/"),
        &[
            "run",
            "list",
            "-R",
            repo,
            flag,
            value,
            "--limit",
            "20",
            "--json",
            "databaseId,workflowName,status,conclusion,url,createdAt",
        ],
    )?;
    let raw: Vec<GhRun> = serde_json::from_str(&json).ok()?;
    Some(latest_per_workflow(raw, repo))
}

/// gh lists newest first; keep each workflow's newest run.
fn latest_per_workflow(raw: Vec<GhRun>, repo: &str) -> Vec<Run> {
    let mut out: Vec<Run> = vec![];
    for r in raw {
        if out.iter().any(|x| x.workflow == r.workflow_name) {
            continue;
        }
        out.push(Run {
            id: r.database_id,
            state: state(&r.status, &r.conclusion),
            workflow: r.workflow_name,
            url: r.url,
            repo: repo.to_string(),
            created: parse_time(&r.created_at).unwrap_or(0),
        });
    }
    out
}

fn state(status: &str, conclusion: &str) -> State {
    match status {
        "completed" => match conclusion {
            "success" => State::Success,
            "failure" | "timed_out" | "startup_failure" => State::Failure,
            "action_required" => State::Waiting,
            _ => State::Other,
        },
        "in_progress" => State::Running,
        "waiting" | "action_required" => State::Waiting,
        _ => State::Queued,
    }
}

/// "2026-11-11T21:18:02Z" → unix seconds.
fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|p| p.parse::<f64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh as i64 * 3600 + mm as i64 * 60 + ss as i64)
}

/// "now", "4m", "3h", "2d".
pub fn age(created: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let s = (now - created).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_timestamps() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-11T21:18:02Z"), Some(1794431882));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951868800));
        assert_eq!(parse_time("garbage"), None);
    }

    #[test]
    fn maps_status() {
        assert_eq!(state("completed", "success"), State::Success);
        assert_eq!(state("completed", "timed_out"), State::Failure);
        assert_eq!(state("completed", "cancelled"), State::Other);
        assert_eq!(state("in_progress", ""), State::Running);
        assert_eq!(state("queued", ""), State::Queued);
        assert_eq!(state("waiting", ""), State::Waiting);
    }

    #[test]
    fn keeps_newest_run_per_workflow() {
        let run = |id, wf: &str| GhRun {
            database_id: id,
            workflow_name: wf.into(),
            status: "completed".into(),
            conclusion: "success".into(),
            url: String::new(),
            created_at: "2026-11-11T00:00:00Z".into(),
        };
        let out = latest_per_workflow(vec![run(3, "CI"), run(2, "Lint"), run(1, "CI")], "o/r");
        let ids: Vec<u64> = out.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3, 2]);
    }

    #[test]
    fn view_command_follows_state() {
        let mut r = Run {
            id: 7,
            workflow: "CI".into(),
            state: State::Running,
            url: String::new(),
            repo: "o/r".into(),
            created: 0,
        };
        assert_eq!(r.view_command().join(" "), "gh run watch 7 -R o/r");
        r.state = State::Failure;
        assert_eq!(r.view_command().join(" "), "gh run view 7 --log-failed -R o/r");
    }
}
