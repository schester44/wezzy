//! Agent activity: what each agent read and changed, turn by turn.
//!
//! The hook script appends prompts, stops and file-tool calls to
//! `<state>/agents/<server pane>.log` (append-only, so nothing is lost between
//! polls the way the single status record loses a PreToolUse that's followed
//! right away by its PostToolUse). `poll` reads the new lines and keeps:
//!
//! - **Touches:** recent reads and edits per file, for the live markers.
//! - **Runs:** the turns since you last reviewed an agent's work. Each turn
//!   starts with a snapshot of the working tree (a tree object written through
//!   a temporary index, so the real index is never touched) and collects the
//!   files the agent edited. A run's stats are the diff between its first
//!   snapshot and that snapshot with only the agent's files updated, so your
//!   own edits elsewhere don't count. Undo restores a turn's files from its
//!   snapshot.
//!
//! Git work happens on short-lived threads; everything here is safe to call
//! from the main thread (it only locks and clones).

use crate::app::{send_event, AppEvent};
use crate::diff::{git, git_run};
use mux::pane::PaneId;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Touches older than this are forgotten.
const TOUCH_TTL: Duration = Duration::from_secs(60);
/// A log is emptied once we've read this far into it.
const LOG_TRIM: u64 = 4 * 1024 * 1024;
/// Live stats while an agent is editing are recomputed at most this often.
const STATS_EVERY: Duration = Duration::from_secs(3);
/// Logs of panes that no longer exist are deleted after this long.
const LOG_ORPHAN: Duration = Duration::from_secs(24 * 3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchKind {
    Read,
    Edit,
}

/// An agent read or edited a file.
#[derive(Clone, Debug)]
pub struct Touch {
    /// absolute
    pub path: PathBuf,
    pub pane: PaneId,
    pub kind: TouchKind,
    pub at: Instant,
}

#[derive(Clone, Debug)]
pub struct Turn {
    /// the working tree when the turn started; `None` until the snapshot lands
    pub base: Option<String>,
    /// files the agent edited this turn (absolute)
    pub files: BTreeSet<PathBuf>,
    pub done: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileStat {
    /// relative to the repo root
    pub path: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub added: u32,
    pub removed: u32,
    pub files: Vec<FileStat>,
}

/// An agent's turns since you last reviewed its work (`dismiss`).
#[derive(Clone, Debug)]
pub struct Run {
    pub root: PathBuf,
    pub branch: String,
    pub turns: Vec<Turn>,
    /// first sentence of the agent's last message, once a turn finishes
    pub summary: Option<String>,
    /// changes to the agent's files since the run started; `None` until computed
    pub stats: Option<Stats>,
    /// the latest turn has finished
    pub done: bool,
    /// when the run last finished a turn, for ordering
    pub finished_at: Option<Instant>,
    stale: bool,
    computed: Option<Instant>,
    computing: bool,
}

impl Run {
    /// Every file the agent edited during the run (absolute).
    pub fn files(&self) -> BTreeSet<PathBuf> {
        self.turns.iter().flat_map(|t| t.files.iter().cloned()).collect()
    }

    pub fn touched(&self, path: &Path) -> bool {
        self.turns.iter().any(|t| t.files.contains(path))
    }

    /// The tree the run started from.
    pub fn base(&self) -> Option<&str> {
        self.turns.first()?.base.as_deref()
    }

    /// Something to show: a finished turn that changed files.
    pub fn has_changes(&self) -> bool {
        self.stats.as_ref().is_some_and(|s| !s.files.is_empty())
    }
}

struct State {
    runs: HashMap<PaneId, Run>,
    touches: Vec<Touch>,
    /// read offset per log file
    offsets: HashMap<PathBuf, u64>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock();
    let st = g.get_or_insert_with(|| State {
        runs: HashMap::new(),
        touches: vec![],
        offsets: HashMap::new(),
    });
    f(st)
}

// -- queries -----------------------------------------------------------------

pub fn run(pane: PaneId) -> Option<Run> {
    with(|s| s.runs.get(&pane).cloned())
}

pub fn runs() -> Vec<(PaneId, Run)> {
    with(|s| s.runs.iter().map(|(p, r)| (*p, r.clone())).collect())
}

/// Recent touches, newest last.
pub fn touches() -> Vec<Touch> {
    with(|s| s.touches.clone())
}

/// The most recent touch of `path`, if it's younger than `within`.
pub fn last_touch(path: &Path, within: Duration) -> Option<Touch> {
    with(|s| {
        s.touches
            .iter()
            .rev()
            .find(|t| t.path == path && t.at.elapsed() < within)
            .cloned()
    })
}

/// You've seen the run's work: the next prompt starts a new run.
pub fn dismiss(pane: PaneId) {
    with(|s| s.runs.remove(&pane));
    send_event(AppEvent::Redraw);
}

/// Forget a closed pane.
pub fn forget(pane: PaneId) {
    with(|s| {
        s.runs.remove(&pane);
        s.touches.retain(|t| t.pane != pane);
    });
}

// -- undo --------------------------------------------------------------------

/// Restore the files the run's latest turn edited to how they were when it
/// started, and drop the turn. Files the agent created are deleted. Anything
/// else that changed those files since (your edits too) is lost.
pub fn undo_turn(pane: PaneId) -> Result<usize, String> {
    let (root, turn) = with(|s| {
        let run = s.runs.get(&pane)?;
        Some((run.root.clone(), run.turns.last()?.clone()))
    })
    .ok_or("Nothing to undo")?;
    let base = turn.base.clone().ok_or("The turn's snapshot isn't ready")?;
    let n = restore(&root, &base, &turn.files)?;
    with(|s| {
        if let Some(run) = s.runs.get_mut(&pane) {
            run.turns.pop();
            if run.turns.is_empty() {
                s.runs.remove(&pane);
            } else {
                run.stale = true;
                run.computed = None;
            }
        }
    });
    crate::diff::mark_dirty();
    send_event(AppEvent::Redraw);
    Ok(n)
}

/// Undo every turn of the run: its files go back to how they were before it.
pub fn undo_run(pane: PaneId) -> Result<usize, String> {
    let run = run(pane).ok_or("Nothing to undo")?;
    let base = run.base().ok_or("The run's snapshot isn't ready")?.to_string();
    let n = restore(&run.root, &base, &run.files())?;
    with(|s| s.runs.remove(&pane));
    crate::diff::mark_dirty();
    send_event(AppEvent::Redraw);
    Ok(n)
}

fn restore(root: &Path, tree: &str, files: &BTreeSet<PathBuf>) -> Result<usize, String> {
    let mut n = 0;
    for abs in files {
        let Ok(rel) = abs.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy();
        let in_base = git(root, &["ls-tree", "--name-only", tree, "--", &rel])
            .is_some_and(|o| !o.trim().is_empty());
        if in_base {
            git_run(root, &["restore", &format!("--source={tree}"), "--worktree", "--", &rel], None)?;
            n += 1;
        } else if abs.exists() {
            std::fs::remove_file(abs).map_err(|e| format!("{rel}: {e}"))?;
            n += 1;
        }
    }
    Ok(n)
}

// -- snapshots ---------------------------------------------------------------

/// A fresh temp index file path; removed by the caller.
fn temp_index() -> PathBuf {
    let name = format!(
        "wezzy-index-{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    std::env::temp_dir().join(name)
}

fn git_with_index(root: &Path, index: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The whole working tree (tracked and untracked, ignored files excluded) as a
/// tree object. Starts from a copy of the real index so unchanged files aren't
/// rehashed.
pub fn snapshot_worktree(root: &Path) -> Option<String> {
    let index = temp_index();
    if let Some(real) = git(root, &["rev-parse", "--git-path", "index"]) {
        let real = root.join(real.trim());
        std::fs::copy(&real, &index).ok();
    }
    let tree = git_with_index(root, &index, &["add", "-A"])
        .and_then(|_| git_with_index(root, &index, &["write-tree"]));
    std::fs::remove_file(&index).ok();
    tree
}

/// `base` with only `files` taken from the working tree.
fn snapshot_files(root: &Path, base: &str, files: &BTreeSet<PathBuf>) -> Option<String> {
    let index = temp_index();
    let rels: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(root).ok())
        .map(|r| r.to_string_lossy().to_string())
        .collect();
    let mut tree = git_with_index(root, &index, &["read-tree", base]);
    if tree.is_some() && !rels.is_empty() {
        let mut args = vec!["add", "-A", "--"];
        args.extend(rels.iter().map(String::as_str));
        tree = git_with_index(root, &index, &args);
    }
    let tree = tree.and_then(|_| git_with_index(root, &index, &["write-tree"]));
    std::fs::remove_file(&index).ok();
    tree
}

fn numstat(root: &Path, from: &str, to: &str) -> Stats {
    let mut stats = Stats::default();
    let Some(out) = git(root, &["diff", "--numstat", "--no-renames", from, to]) else {
        return stats;
    };
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(a), Some(r), Some(path)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        // binary files show "-"
        let added = a.parse().unwrap_or(0);
        let removed = r.parse().unwrap_or(0);
        stats.added += added;
        stats.removed += removed;
        stats.files.push(FileStat {
            path: path.to_string(),
            added,
            removed,
        });
    }
    stats
}

fn compute_stats(pane: PaneId) {
    let Some((root, base, files)) = with(|s| {
        let run = s.runs.get_mut(&pane)?;
        let base = run.base()?.to_string();
        if run.computing {
            return None;
        }
        run.computing = true;
        run.stale = false;
        run.computed = Some(Instant::now());
        Some((run.root.clone(), base, run.files()))
    }) else {
        return;
    };
    std::thread::spawn(move || {
        let stats = snapshot_files(&root, &base, &files).map(|after| numstat(&root, &base, &after));
        with(|s| {
            if let Some(run) = s.runs.get_mut(&pane) {
                run.computing = false;
                if let Some(stats) = stats {
                    run.stats = Some(stats);
                }
            }
        });
        send_event(AppEvent::Redraw);
    });
}

// -- the log -----------------------------------------------------------------

fn logs_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::HOME_DIR.join(".local/state"))
        .join("wezzy/agents")
}

/// Read new log lines and update runs and touches. Called from the hooks
/// poller's thread.
pub fn poll() {
    let Ok(entries) = std::fs::read_dir(logs_dir()) else {
        return;
    };
    let mut events: Vec<(PaneId, Value)> = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Some(remote) = path.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let Some(local) = crate::session::local_pane_id(remote as PaneId) else {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|m| m.elapsed().is_ok_and(|e| e > LOG_ORPHAN));
            if old {
                std::fs::remove_file(&path).ok();
            }
            continue;
        };
        for line in read_new(&path) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                events.push((local, v));
            }
        }
    }
    let changed = !events.is_empty();
    for (pane, v) in events {
        apply(pane, &v);
    }
    // live stats for runs whose files changed
    let due: Vec<PaneId> = with(|s| {
        s.touches.retain(|t| t.at.elapsed() < TOUCH_TTL);
        s.runs
            .iter()
            .filter(|(_, r)| r.stale && !r.computing && r.computed.is_none_or(|c| c.elapsed() > STATS_EVERY))
            .map(|(p, _)| *p)
            .collect()
    });
    for pane in due {
        compute_stats(pane);
    }
    if changed {
        send_event(AppEvent::Redraw);
    }
}

/// Lines appended since the last read. The first time we see a log we start
/// at its end: what happened before the app started isn't replayed.
fn read_new(path: &Path) -> Vec<String> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return vec![];
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = with(|s| *s.offsets.entry(path.to_path_buf()).or_insert(len));
    // truncated under us
    let start = if start > len { 0 } else { start };
    if start == len {
        return vec![];
    }
    let mut buf = String::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_string(&mut buf).is_err() {
        return vec![];
    }
    // keep a partial last line for next time
    let complete = buf.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let mut end = start + complete as u64;
    if end >= LOG_TRIM && end == len {
        // read everything: empty it (an append racing this loses one event)
        if std::fs::OpenOptions::new().write(true).open(path).and_then(|f| f.set_len(0)).is_ok() {
            end = 0;
        }
    }
    with(|s| s.offsets.insert(path.to_path_buf(), end));
    buf[..complete].lines().map(str::to_string).collect()
}

fn apply(pane: PaneId, v: &Value) {
    let p = &v["payload"];
    let cwd = p["cwd"].as_str().filter(|c| !c.is_empty()).map(PathBuf::from);
    match p["hook_event_name"].as_str().unwrap_or("") {
        "UserPromptSubmit" => {
            if p["agent_id"].is_string() {
                return;
            }
            let Some(cwd) = cwd else { return };
            start_turn(pane, cwd);
        }
        "PreToolUse" => {
            let kind = match p["tool_name"].as_str().unwrap_or("").to_ascii_lowercase().as_str() {
                "read" => TouchKind::Read,
                _ => TouchKind::Edit,
            };
            let base = cwd.unwrap_or_default();
            for path in tool_paths(&p["tool_input"]) {
                let abs = if path.is_absolute() { path } else { base.join(path) };
                touch(pane, abs, kind);
            }
        }
        "Stop" => finish_turn(pane, v),
        _ => {}
    }
}

fn start_turn(pane: PaneId, cwd: PathBuf) {
    let root = with(|s| s.runs.get(&pane).map(|r| r.root.clone()));
    // a run lives in one repo; prompting from another starts over
    let repo = git(&cwd, &["rev-parse", "--show-toplevel"]).map(|s| PathBuf::from(s.trim()));
    let Some(repo) = repo else {
        return;
    };
    let branch = git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let idx = with(|s| {
        if root.as_ref() != Some(&repo) {
            s.runs.remove(&pane);
        }
        let run = s.runs.entry(pane).or_insert_with(|| Run {
            root: repo.clone(),
            branch: branch.clone(),
            turns: vec![],
            summary: None,
            stats: None,
            done: false,
            finished_at: None,
            stale: false,
            computed: None,
            computing: false,
        });
        run.branch = branch;
        run.done = false;
        run.turns.push(Turn {
            base: None,
            files: BTreeSet::new(),
            done: false,
        });
        run.turns.len() - 1
    });
    std::thread::spawn(move || {
        let tree = snapshot_worktree(&repo);
        with(|s| {
            if let Some(turn) = s.runs.get_mut(&pane).and_then(|r| r.turns.get_mut(idx)) {
                turn.base = tree;
            }
        });
    });
}

fn touch(pane: PaneId, path: PathBuf, kind: TouchKind) {
    with(|s| {
        if kind == TouchKind::Edit {
            if let Some(run) = s.runs.get_mut(&pane) {
                if path.starts_with(&run.root) {
                    if let Some(turn) = run.turns.last_mut() {
                        turn.files.insert(path.clone());
                        run.stale = true;
                    }
                }
            }
        }
        s.touches.push(Touch {
            path,
            pane,
            kind,
            at: Instant::now(),
        });
    });
}

fn finish_turn(pane: PaneId, v: &Value) {
    let summary = summary(&v["payload"]);
    with(|s| {
        if let Some(run) = s.runs.get_mut(&pane) {
            run.done = true;
            run.finished_at = Some(Instant::now());
            if let Some(turn) = run.turns.last_mut() {
                turn.done = true;
            }
            if summary.is_some() {
                run.summary = summary;
            }
            // final numbers right away, not after the throttle
            run.stale = false;
            run.computed = None;
        }
    });
    // the edit's PreToolUse fires before the file is written; give the last
    // write a moment
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        compute_stats(pane);
    });
}

// -- payload parsing ---------------------------------------------------------

/// Files a file tool names: `file_path` (Claude), `notebook_path`, or every
/// file in an apply_patch body (Codex). Possibly relative to the cwd.
fn tool_paths(input: &Value) -> Vec<PathBuf> {
    for k in ["file_path", "filePath", "notebook_path", "path"] {
        if let Some(p) = input[k].as_str().filter(|s| !s.is_empty()) {
            return vec![PathBuf::from(p)];
        }
    }
    let body = match &input["command"] {
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
        _ => input["input"]
            .as_str()
            .or(input["patch"].as_str())
            .or(input["command"].as_str())
            .unwrap_or("")
            .to_string(),
    };
    body.lines()
        .filter_map(|l| {
            ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "]
                .iter()
                .find_map(|p| l.strip_prefix(p))
        })
        .map(|f| PathBuf::from(f.trim()))
        .collect()
}

/// The agent's closing words: `last_assistant_message` when the harness sends
/// it, else the last assistant text in Claude's transcript.
fn summary(p: &Value) -> Option<String> {
    let text = p["last_assistant_message"]
        .as_str()
        .map(str::to_string)
        .or_else(|| p["transcript_path"].as_str().and_then(|t| last_assistant_text(Path::new(t))))?;
    first_sentence(&text)
}

fn last_assistant_text(transcript: &Path) -> Option<String> {
    let mut f = std::fs::File::open(transcript).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(512 * 1024);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    text.lines().rev().find_map(|line| {
        let v: Value = serde_json::from_str(line).ok()?;
        if v["type"].as_str() != Some("assistant") || v["isSidechain"].as_bool() == Some(true) {
            return None;
        }
        let parts = v["message"]["content"].as_array()?;
        let text: Vec<&str> = parts
            .iter()
            .filter(|c| c["type"].as_str() == Some("text"))
            .filter_map(|c| c["text"].as_str())
            .collect();
        let text = text.join("\n");
        (!text.trim().is_empty()).then_some(text)
    })
}

/// The first sentence of the first paragraph, markdown stripped, clipped.
fn first_sentence(text: &str) -> Option<String> {
    let para = text
        .split("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty() && !p.starts_with('#') && !p.starts_with("```"))?;
    let flat: String = para
        .lines()
        .map(|l| l.trim().trim_start_matches(['-', '*', '>']).trim())
        .collect::<Vec<_>>()
        .join(" ")
        .replace("**", "")
        .replace('`', "");
    let end = flat.find(". ").map(|i| i + 1).unwrap_or(flat.len());
    let mut s = flat[..end].trim().to_string();
    if s.chars().count() > 160 {
        s = s.chars().take(159).collect::<String>().trim_end().to_string() + "…";
    }
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths_from_tools() {
        assert_eq!(tool_paths(&json!({"file_path": "/a/b.rs"})), vec![PathBuf::from("/a/b.rs")]);
        assert_eq!(
            tool_paths(&json!({"command": "*** Begin Patch\n*** Update File: src/a.rs\n@@\n*** Add File: src/b.rs\n+x"})),
            vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]
        );
        assert!(tool_paths(&json!({})).is_empty());
    }

    #[test]
    fn sentences() {
        assert_eq!(
            first_sentence("Fixed the **duplicate** notifications. Also added tests.\n\nMore"),
            Some("Fixed the duplicate notifications.".into())
        );
        assert_eq!(first_sentence("## Summary\n\n- Did `x` and y"), Some("Did x and y".into()));
        assert_eq!(first_sentence("   "), None);
    }

    #[test]
    fn snapshot_and_stats() {
        let dir = std::env::temp_dir().join(format!("wezzy-activity-test-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
        };
        run(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("mine.txt"), "x\n").unwrap();
        let base = snapshot_worktree(&dir).expect("base tree");
        // the agent edits a.txt and adds b.txt; someone else edits mine.txt
        std::fs::write(dir.join("a.txt"), "one\n2\nthree\n").unwrap();
        std::fs::write(dir.join("b.txt"), "new\n").unwrap();
        std::fs::write(dir.join("mine.txt"), "y\n").unwrap();
        let files: BTreeSet<PathBuf> = [dir.join("a.txt"), dir.join("b.txt")].into();
        let after = snapshot_files(&dir, &base, &files).expect("after tree");
        let stats = numstat(&dir, &base, &after);
        assert_eq!((stats.added, stats.removed, stats.files.len()), (3, 1, 2));
        // undo puts the agent's files back and leaves the rest
        assert_eq!(restore(&dir, &base, &files).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\ntwo\n");
        assert!(!dir.join("b.txt").exists());
        assert_eq!(std::fs::read_to_string(dir.join("mine.txt")).unwrap(), "y\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}
