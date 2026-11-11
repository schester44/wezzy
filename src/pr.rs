//! The branch's pull request, for the git panel: looking it up with `gh` and
//! creating one (push, then `gh pr create`).
//!
//! Lookups are cached per (repo, branch). The diff poller calls `poll` after
//! every snapshot, but `gh` only runs when there's no entry yet, the
//! upstream moved (a push), or a minute has passed. Everything that shells
//! out runs off the main thread and reports back with `AppEvent::PullRequest`.
//! When `gh` is missing or not logged in, lookups find nothing and the panel
//! shows no PR.

use crate::app::{send_event, AppEvent};
use crate::diff::{git, git_run, Snapshot};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PullRequest {
    pub number: u64,
    pub state: PrState,
    pub url: String,
}

impl PullRequest {
    pub fn is_open(&self) -> bool {
        matches!(self.state, PrState::Open | PrState::Draft)
    }
}

struct Entry {
    pr: Option<PullRequest>,
    at: Instant,
    /// upstream commit when we looked; a push moves it
    upstream: Option<String>,
}

type Key = (PathBuf, String);

static CACHE: Mutex<Option<HashMap<Key, Entry>>> = Mutex::new(None);
static IN_FLIGHT: Mutex<Option<HashSet<Key>>> = Mutex::new(None);

fn key(snap: &Snapshot) -> Option<Key> {
    let root = snap.root.clone()?;
    snap.on_branch.then(|| (root, snap.branch.clone()))
}

/// The PR for the snapshot's branch, if we've found one.
pub fn cached(snap: &Snapshot) -> Option<PullRequest> {
    let key = key(snap)?;
    CACHE.lock().as_ref()?.get(&key)?.pr.clone()
}

/// Drop the cached lookup so the next poll asks `gh` again.
pub fn forget(snap: &Snapshot) {
    if let (Some(key), Some(m)) = (key(snap), CACHE.lock().as_mut()) {
        m.remove(&key);
    }
}

/// Called by the diff poller after each snapshot: refresh the lookup in the
/// background when it's missing, stale, or the upstream moved.
pub fn poll(snap: &Snapshot) {
    let Some(key) = key(snap) else { return };
    let upstream = snap.upstream.as_ref().map(|u| u.sha.clone());
    let due = CACHE
        .lock()
        .as_ref()
        .and_then(|m| m.get(&key))
        .is_none_or(|e| e.at.elapsed() > TTL || e.upstream != upstream);
    if !due || !IN_FLIGHT.lock().get_or_insert_with(HashSet::new).insert(key.clone()) {
        return;
    }
    std::thread::spawn(move || {
        store(&key, lookup(&key.0), upstream);
        if let Some(s) = IN_FLIGHT.lock().as_mut() {
            s.remove(&key);
        }
    });
}

fn store(key: &Key, pr: Option<PullRequest>, upstream: Option<String>) {
    let mut guard = CACHE.lock();
    let m = guard.get_or_insert_with(HashMap::new);
    let changed = m.get(key).is_none_or(|e| e.pr != pr);
    m.insert(
        key.clone(),
        Entry {
            pr,
            at: Instant::now(),
            upstream,
        },
    );
    drop(guard);
    if changed {
        send_event(AppEvent::Redraw);
    }
}

fn gh(dir: &Path) -> Command {
    let mut c = Command::new("gh");
    c.current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    c
}

/// `gh pr view` for the branch checked out in `root`.
fn lookup(root: &Path) -> Option<PullRequest> {
    let out = gh(root)
        .args(["pr", "view", "--json", "number,state,isDraft,url"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_view(&String::from_utf8_lossy(&out.stdout))
}

fn parse_view(json: &str) -> Option<PullRequest> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let state = match (v["state"].as_str()?, v["isDraft"].as_bool().unwrap_or(false)) {
        ("OPEN", true) => PrState::Draft,
        ("OPEN", false) => PrState::Open,
        ("MERGED", _) => PrState::Merged,
        _ => PrState::Closed,
    };
    Some(PullRequest {
        number: v["number"].as_u64()?,
        state,
        url: v["url"].as_str()?.to_string(),
    })
}

fn has_gh() -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("gh").is_file()))
}

// -- creating ----------------------------------------------------------------

/// What `p` found out about the branch, in the background.
#[derive(Debug)]
pub enum PrEvent {
    /// the branch already has an open PR: open it
    Open(String),
    /// ready for the composer
    Compose(Plan),
    /// couldn't start (no remote, gh missing, on the default branch, ...)
    Error(String),
    Created { url: String, draft: bool },
    /// creating failed; the composer reopens with what was written
    Failed { error: String, plan: Plan, text: String, draft: bool },
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub root: PathBuf,
    pub branch: String,
    pub remote: String,
    /// the default branch the PR targets; `None` lets gh pick
    pub base: Option<String>,
    /// commits on the branch that aren't on the base
    pub commits: Option<usize>,
    /// default title
    pub title: String,
}

/// Refresh the panel's snapshot, look the branch over and decide what `p`
/// does; reports a `PrEvent`.
pub fn begin() {
    std::thread::spawn(|| {
        crate::diff::refresh();
        let ev = match crate::diff::snapshot() {
            Some(snap) => prepare(&snap),
            None => PrEvent::Error("Not a git repository".into()),
        };
        send_event(AppEvent::PullRequest(ev));
    });
}

fn prepare(snap: &Snapshot) -> PrEvent {
    let Some(root) = snap.root.clone() else {
        return PrEvent::Error("Not a git repository".into());
    };
    if !snap.on_branch {
        return PrEvent::Error("Not on a branch".into());
    }
    if !has_gh() {
        return PrEvent::Error("gh isn't installed".into());
    }
    // the poller may not have looked yet (panel just opened)
    let known = key(snap).is_some_and(|k| CACHE.lock().as_ref().is_some_and(|m| m.contains_key(&k)));
    let pr = if known {
        cached(snap)
    } else {
        let pr = lookup(&root);
        if let Some(k) = key(snap) {
            store(&k, pr.clone(), snap.upstream.as_ref().map(|u| u.sha.clone()));
        }
        pr
    };
    if let Some(pr) = pr.filter(PullRequest::is_open) {
        return PrEvent::Open(pr.url);
    }

    let branch = snap.branch.clone();
    let Some(remote) = remote_for(&root, &branch) else {
        return PrEvent::Error("No git remote".into());
    };
    let base = default_branch(&root, &remote);
    if base.as_deref() == Some(branch.as_str()) {
        return PrEvent::Error("On the default branch".into());
    }
    let commits = base.as_ref().and_then(|b| {
        git(&root, &["rev-list", "--count", &format!("{remote}/{b}..HEAD")])?
            .trim()
            .parse::<usize>()
            .ok()
    });
    if commits == Some(0) {
        return PrEvent::Error(format!("No commits ahead of {}", base.unwrap_or_default()));
    }
    let subject = snap.head.split_once(' ').map(|(_, s)| s).unwrap_or("");
    let title = if commits == Some(1) && !subject.is_empty() {
        subject.to_string()
    } else {
        humanize(&branch)
    };
    PrEvent::Compose(Plan {
        root,
        branch,
        remote,
        base,
        commits,
        title,
    })
}

/// The branch's push remote: its configured one, else `origin`, else the only one.
fn remote_for(root: &Path, branch: &str) -> Option<String> {
    if let Some(r) = git(root, &["config", &format!("branch.{branch}.remote")]) {
        let r = r.trim();
        if !r.is_empty() && r != "." {
            return Some(r.to_string());
        }
    }
    let remotes = git(root, &["remote"])?;
    let remotes: Vec<&str> = remotes.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if remotes.contains(&"origin") {
        Some("origin".into())
    } else {
        remotes.first().map(|r| r.to_string())
    }
}

/// `origin/HEAD`'s branch, else `main` or `master` if the remote has one.
fn default_branch(root: &Path, remote: &str) -> Option<String> {
    if let Some(head) = git(root, &["symbolic-ref", "--short", "-q", &format!("refs/remotes/{remote}/HEAD")]) {
        if let Some(b) = head.trim().strip_prefix(&format!("{remote}/")) {
            return Some(b.to_string());
        }
    }
    ["main", "master"]
        .into_iter()
        .find(|b| git(root, &["rev-parse", "--verify", "-q", &format!("refs/remotes/{remote}/{b}")]).is_some())
        .map(str::to_string)
}

/// "steve/fix-pr_info" -> "Fix pr info"
pub fn humanize(branch: &str) -> String {
    let leaf = branch.rsplit('/').next().unwrap_or(branch);
    let words: Vec<&str> = leaf.split(['-', '_']).filter(|w| !w.is_empty()).collect();
    let s = words.join(" ");
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => branch.to_string(),
    }
}

/// Push the branch (setting its upstream if it has none) and open the PR.
/// `text`: first line is the title, the rest the body.
pub fn create(plan: Plan, text: String, draft: bool) {
    std::thread::spawn(move || {
        let ev = match run_create(&plan, &text, draft) {
            Ok(url) => {
                // show it in the header right away
                let upstream = git(&plan.root, &["rev-parse", "--verify", "-q", "@{upstream}"])
                    .map(|s| s.trim().to_string());
                store(&(plan.root.clone(), plan.branch.clone()), lookup(&plan.root), upstream);
                PrEvent::Created { url, draft }
            }
            Err(error) => PrEvent::Failed { error, plan, text, draft },
        };
        send_event(AppEvent::PullRequest(ev));
    });
}

fn run_create(plan: &Plan, text: &str, draft: bool) -> Result<String, String> {
    let root = &plan.root;
    let has_upstream = git(root, &["rev-parse", "--verify", "-q", "@{upstream}"]).is_some();
    if !has_upstream {
        git_run(root, &["push", "-q", "-u", &plan.remote, "HEAD"], None)?;
    } else {
        let ahead = git(root, &["rev-list", "--count", "@{upstream}..HEAD"])
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if ahead > 0 {
            git_run(root, &["push", "-q"], None)?;
        }
    }

    let (title, body) = split_message(text);
    let mut cmd = gh(root);
    cmd.args(["pr", "create", "--title", &title, "--body", &body]);
    if draft {
        cmd.arg("--draft");
    }
    if let Some(base) = &plan.base {
        cmd.args(["--base", base]);
    }
    let out = cmd.output().map_err(|e| format!("gh: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("gh pr create failed")
            .to_string());
    }
    Ok(stdout
        .lines()
        .map(str::trim)
        .rfind(|l| l.starts_with("http"))
        .unwrap_or("")
        .to_string())
}

/// First line is the title; the body is the rest, trimmed.
pub fn split_message(text: &str) -> (String, String) {
    let (title, body) = text.split_once('\n').unwrap_or((text, ""));
    (title.trim().to_string(), body.trim().to_string())
}

/// "https://github.com/o/r/pull/12" -> "#12"
pub fn short_ref(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
        .map(|n| format!("#{n}"))
        .unwrap_or_else(|| "pull request".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_branch_names() {
        assert_eq!(humanize("steve/fix-pr_info"), "Fix pr info");
        assert_eq!(humanize("add-thing"), "Add thing");
        assert_eq!(humanize("main"), "Main");
    }

    #[test]
    fn parses_gh_view() {
        let pr = parse_view(r#"{"isDraft":true,"number":12,"state":"OPEN","url":"https://x/pull/12"}"#).unwrap();
        assert_eq!((pr.number, pr.state), (12, PrState::Draft));
        let pr = parse_view(r#"{"isDraft":false,"number":3,"state":"MERGED","url":"u"}"#).unwrap();
        assert_eq!(pr.state, PrState::Merged);
        assert!(!pr.is_open());
        assert!(parse_view("not json").is_none());
    }

    /// Remote, default branch and upstream tracking against a throwaway bare remote.
    #[test]
    fn finds_remote_and_base() {
        let tmp = std::env::temp_dir().join(format!("wezzy-pr-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (remote, work) = (tmp.join("remote.git"), tmp.join("work"));
        std::fs::create_dir_all(&work).unwrap();
        let run = |dir: &Path, args: &[&str]| git_run(dir, args, None).unwrap();
        run(&tmp, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        run(&work, &["init", "-q", "-b", "main"]);
        run(&work, &["config", "user.email", "t@t"]);
        run(&work, &["config", "user.name", "t"]);
        run(&work, &["remote", "add", "origin", remote.to_str().unwrap()]);
        std::fs::write(work.join("a"), "a\n").unwrap();
        run(&work, &["add", "a"]);
        run(&work, &["commit", "-q", "-m", "init"]);
        run(&work, &["push", "-q", "-u", "origin", "main"]);

        run(&work, &["checkout", "-q", "-b", "steve/add-thing"]);
        std::fs::write(work.join("b"), "b\n").unwrap();
        run(&work, &["add", "b"]);
        run(&work, &["commit", "-q", "-m", "Add b"]);
        assert_eq!(remote_for(&work, "steve/add-thing").as_deref(), Some("origin"));
        assert_eq!(default_branch(&work, "origin").as_deref(), Some("main"));
        let snap = crate::diff::compute(&work);
        assert!(snap.on_branch && snap.upstream.is_none());

        run(&work, &["push", "-q", "-u", "origin", "HEAD"]);
        std::fs::write(work.join("c"), "c\n").unwrap();
        run(&work, &["add", "c"]);
        run(&work, &["commit", "-q", "-m", "Add c"]);
        let u = crate::diff::compute(&work).upstream.unwrap();
        assert_eq!((u.ahead, u.behind), (1, 0));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn splits_messages() {
        assert_eq!(split_message("Title\n\nBody\nmore\n"), ("Title".into(), "Body\nmore".into()));
        assert_eq!(split_message("Only"), ("Only".into(), String::new()));
        assert_eq!(short_ref("https://github.com/o/r/pull/12"), "#12");
    }
}
