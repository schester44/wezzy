//! Git changes for the active pane's repo, for the diff side panel.
//!
//! A background thread polls `git` for whatever the UI is pointing at
//! (`set_target`) and publishes snapshots: staged, unstaged and untracked
//! files, each with parsed hunks. GIT_OPTIONAL_LOCKS=0 keeps the poller from
//! ever contending for index.lock with an agent that's committing.
//!
//! The panel stages/unstages/discards whole files or single hunks (via
//! `git apply [--cached] [-R]` on a one-hunk patch) and commits.

use crate::app::{send_event, AppEvent};
use parking_lot::Mutex;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Files with bigger diffs show up without hunks (file-level ops still work).
const MAX_FILE_LINES: usize = 5000;
/// Untracked files bigger than this aren't read for preview.
const MAX_UNTRACKED_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Untracked,
    Unstaged,
    Staged,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Untracked, Section::Unstaged, Section::Staged];

    pub fn title(self) -> &'static str {
        match self {
            Section::Untracked => "Untracked",
            Section::Unstaged => "Unstaged",
            Section::Staged => "Staged",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    Add,
    Del,
    Context,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// line number in the old / new file
    pub old: Option<u32>,
    pub new: Option<u32>,
    /// display text (tabs expanded, no +/- prefix)
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk {
    /// raw "@@ -a,b +c,d @@ ctx" line
    pub header: String,
    /// raw body lines, prefixes included ("\ No newline" too); used to build patches
    pub raw: Vec<String>,
    pub lines: Vec<DiffLine>,
    pub new_start: u32,
}

impl Hunk {
    /// The function/context text after the second "@@".
    pub fn context(&self) -> &str {
        self.header.splitn(3, "@@").nth(2).unwrap_or("").trim()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileDiff {
    pub path: String,
    /// rename source
    pub old_path: Option<String>,
    /// "M", "A", "D", "R", "U" (conflict), "?" (untracked)
    pub status: &'static str,
    /// everything before the first "@@" (diff --git, index, ---, +++)
    pub header: String,
    pub hunks: Vec<Hunk>,
    pub added: u32,
    pub removed: u32,
    /// binary, conflicted, or too big: no hunk-level ops
    pub note: Option<&'static str>,
}

/// The branch's upstream and how far apart they are.
#[derive(Clone, Debug, PartialEq)]
pub struct Upstream {
    pub ahead: u32,
    pub behind: u32,
    /// upstream commit, so a push is noticed
    pub sha: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub root: Option<PathBuf>,
    /// branch name, or the short hash when HEAD is detached
    pub branch: String,
    pub on_branch: bool,
    /// `None` on a branch with no upstream (or a detached HEAD)
    pub upstream: Option<Upstream>,
    /// "abc1234 subject" of HEAD, empty on an unborn branch
    pub head: String,
    pub has_head: bool,
    pub untracked: Vec<FileDiff>,
    pub unstaged: Vec<FileDiff>,
    pub staged: Vec<FileDiff>,
}

impl Snapshot {
    pub fn files(&self, s: Section) -> &[FileDiff] {
        match s {
            Section::Untracked => &self.untracked,
            Section::Unstaged => &self.unstaged,
            Section::Staged => &self.staged,
        }
    }

    pub fn is_clean(&self) -> bool {
        Section::ALL.iter().all(|s| self.files(*s).is_empty())
    }
}

static TARGET: Mutex<Option<PathBuf>> = Mutex::new(None);
static SNAPSHOT: Mutex<Option<Arc<Snapshot>>> = Mutex::new(None);
/// Set after we change the repo ourselves, so the poller refreshes right away.
static DIRTY: Mutex<bool> = Mutex::new(false);

/// Directory the panel is looking at; `None` when it's closed.
pub fn set_target(cwd: Option<PathBuf>) {
    let mut t = TARGET.lock();
    if *t != cwd {
        *t = cwd;
    }
}

pub fn snapshot() -> Option<Arc<Snapshot>> {
    SNAPSHOT.lock().clone()
}

/// Recompute now (on the calling thread) and publish.
pub fn refresh() {
    let Some(cwd) = TARGET.lock().clone() else {
        return;
    };
    publish(compute(&cwd));
}

fn publish(snap: Snapshot) {
    let mut cur = SNAPSHOT.lock();
    if cur.as_deref() != Some(&snap) {
        *cur = Some(Arc::new(snap));
        drop(cur);
        send_event(AppEvent::Redraw);
    }
}

pub fn start() {
    std::thread::spawn(|| {
        let mut last: Option<(PathBuf, Instant)> = None;
        loop {
            std::thread::sleep(Duration::from_millis(200));
            let Some(target) = TARGET.lock().clone() else {
                last = None;
                continue;
            };
            let dirty = std::mem::take(&mut *DIRTY.lock());
            // recompute immediately when the target changes, else every 2s
            let due = dirty
                || match &last {
                    Some((t, at)) => *t != target || at.elapsed() > Duration::from_secs(2),
                    None => true,
                };
            if !due {
                continue;
            }
            let snap = compute(&target);
            last = Some((target, Instant::now()));
            crate::pr::poll(&snap);
            publish(snap);
        }
    });
}

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args(["-c", "core.quotePath=false", "-c", "color.ui=false"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_EDITOR", "true")
        .env("GIT_TERMINAL_PROMPT", "0");
    c
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git_cmd(dir).args(args).stderr(Stdio::null()).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run git for a write op; Err carries git's message.
pub(crate) fn git_run(dir: &Path, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut child = git_cmd(dir)
        .args(args)
        .env_remove("GIT_OPTIONAL_LOCKS")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("git: {e}"))?;
    if let Some(input) = stdin {
        if let Some(mut w) = child.stdin.take() {
            w.write_all(input.as_bytes()).map_err(|e| format!("git: {e}"))?;
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = stderr
            .lines()
            .chain(stdout.lines())
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("hint:"))
            .unwrap_or("git failed")
            .to_string();
        Err(msg)
    }
}

pub(crate) fn compute(cwd: &Path) -> Snapshot {
    let Some(root) = git(cwd, &["rev-parse", "--show-toplevel"]).map(|s| PathBuf::from(s.trim()))
    else {
        return Snapshot::default();
    };
    let head = git(&root, &["log", "-1", "--format=%h %s"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let symbolic = git(&root, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let on_branch = symbolic.is_some();
    let branch = symbolic
        .or_else(|| git(&root, &["rev-parse", "--short", "HEAD"]))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let upstream = on_branch.then(|| upstream(&root)).flatten();

    let staged = parse_diff(&git(&root, &["diff", "--cached", "--no-ext-diff"]).unwrap_or_default());
    let unstaged = parse_diff(&git(&root, &["diff", "--no-ext-diff"]).unwrap_or_default());
    let untracked = git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])
        .unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(|p| untracked_file(&root, p))
        .collect();

    Snapshot {
        root: Some(root),
        branch,
        on_branch,
        upstream,
        has_head: !head.is_empty(),
        head,
        untracked,
        unstaged,
        staged,
    }
}

fn upstream(root: &Path) -> Option<Upstream> {
    let sha = git(root, &["rev-parse", "--verify", "-q", "@{upstream}"])?.trim().to_string();
    let counts = git(root, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"])?;
    let mut it = counts.split_whitespace().map(|n| n.parse::<u32>().unwrap_or(0));
    let behind = it.next()?;
    let ahead = it.next()?;
    Some(Upstream { ahead, behind, sha })
}

fn untracked_file(root: &Path, path: &str) -> FileDiff {
    let full = root.join(path);
    let mut f = FileDiff {
        path: path.to_string(),
        old_path: None,
        status: "?",
        header: String::new(),
        hunks: vec![],
        added: 0,
        removed: 0,
        note: None,
    };
    let small = std::fs::metadata(&full).is_ok_and(|m| m.len() <= MAX_UNTRACKED_BYTES);
    let bytes = small.then(|| std::fs::read(&full).ok()).flatten();
    match bytes {
        None => f.note = Some("large file"),
        Some(b) if b.contains(&0) => f.note = Some("binary"),
        Some(b) => {
            let text = String::from_utf8_lossy(&b);
            let lines: Vec<DiffLine> = text
                .lines()
                .take(MAX_FILE_LINES)
                .enumerate()
                .map(|(i, l)| DiffLine {
                    kind: LineKind::Add,
                    old: None,
                    new: Some(i as u32 + 1),
                    text: l.replace('\t', "    "),
                })
                .collect();
            f.added = text.lines().count() as u32;
            if !lines.is_empty() {
                f.hunks.push(Hunk {
                    header: String::new(),
                    raw: vec![],
                    lines,
                    new_start: 1,
                });
            }
        }
    }
    f
}

/// "@@ -12,7 +12,9 @@ fn x" -> (12, 12)
fn hunk_starts(h: &str) -> Option<(u32, u32)> {
    let mut it = h.split_whitespace().skip(1);
    let old = it.next()?.trim_start_matches('-').split(',').next()?.parse().ok()?;
    let new = it.next()?.trim_start_matches('+').split(',').next()?.parse().ok()?;
    Some((old, new))
}

/// "diff --git a/P b/P" -> P (no rename, so both halves are the same length)
fn path_from_diff_git(l: &str) -> String {
    let rest = l.trim_start_matches("diff --git ");
    let rest = rest.strip_prefix("a/").unwrap_or(rest);
    let n = rest.len().saturating_sub(3) / 2;
    rest.get(..n).unwrap_or(rest).to_string()
}

/// Parse `git diff` output into files with hunks.
fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = vec![];
    // (old, new) line counters inside the current hunk
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for l in text.lines() {
        let combined = l.starts_with("diff --cc ") || l.starts_with("diff --combined ");
        if l.starts_with("diff --git ") || combined {
            files.push(FileDiff {
                path: if combined {
                    l.splitn(3, ' ').nth(2).unwrap_or("").to_string()
                } else {
                    path_from_diff_git(l)
                },
                old_path: None,
                status: if combined { "U" } else { "M" },
                header: format!("{l}\n"),
                hunks: vec![],
                added: 0,
                removed: 0,
                note: combined.then_some("conflict"),
            });
            in_hunk = false;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if f.note == Some("conflict") {
            continue;
        }
        if l.starts_with("@@") {
            in_hunk = true;
            (old, new) = hunk_starts(l).unwrap_or((0, 0));
            f.hunks.push(Hunk {
                header: l.to_string(),
                raw: vec![],
                lines: vec![],
                new_start: new,
            });
            continue;
        }
        if !in_hunk {
            f.header.push_str(l);
            f.header.push('\n');
            if l.starts_with("new file mode") {
                f.status = "A";
            } else if l.starts_with("deleted file mode") {
                f.status = "D";
            } else if let Some(p) = l.strip_prefix("rename from ") {
                f.status = "R";
                f.old_path = Some(p.to_string());
            } else if let Some(p) = l.strip_prefix("rename to ") {
                f.path = p.to_string();
            } else if let Some(p) = l.strip_prefix("+++ b/") {
                f.path = p.trim_end_matches('\t').to_string();
            } else if l.starts_with("Binary files ") {
                f.note = Some("binary");
            }
            continue;
        }
        let h = f.hunks.last_mut().expect("in hunk");
        h.raw.push(l.to_string());
        let mut push = |kind, o, n, t: &str| {
            h.lines.push(DiffLine {
                kind,
                old: o,
                new: n,
                text: t.replace('\t', "    "),
            })
        };
        if let Some(rest) = l.strip_prefix('+') {
            push(LineKind::Add, None, Some(new), rest);
            new += 1;
            f.added += 1;
        } else if let Some(rest) = l.strip_prefix('-') {
            push(LineKind::Del, Some(old), None, rest);
            old += 1;
            f.removed += 1;
        } else if l.starts_with('\\') {
            // "\ No newline at end of file": kept in raw only
        } else {
            push(LineKind::Context, Some(old), Some(new), l.strip_prefix(' ').unwrap_or(l));
            old += 1;
            new += 1;
        }
    }
    for f in files.iter_mut() {
        if f.hunks.iter().map(|h| h.lines.len()).sum::<usize>() > MAX_FILE_LINES {
            f.hunks.clear();
            f.note = Some("large diff");
        }
    }
    files
}

// -- operations --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Stage,
    Unstage,
    Discard,
}

/// Stage / unstage / discard a file, or one of its hunks. Returns a short
/// message for the user on failure or when there's nothing to do.
pub fn apply(
    root: &Path,
    op: Op,
    sec: Section,
    file: &FileDiff,
    hunk: Option<usize>,
    has_head: bool,
) -> Result<(), String> {
    let hunk = hunk
        .filter(|_| sec != Section::Untracked && file.note.is_none())
        .and_then(|i| file.hunks.get(i));
    let patch = |h: &Hunk| {
        let mut p = file.header.clone();
        p.push_str(&h.header);
        p.push('\n');
        for l in &h.raw {
            p.push_str(l);
            p.push('\n');
        }
        p
    };
    let mut paths: Vec<&str> = vec![&file.path];
    paths.extend(file.old_path.as_deref());
    let with_paths = |args: &[&'static str]| {
        let mut v: Vec<&str> = args.to_vec();
        v.push("--");
        v.extend(paths.iter().copied());
        v
    };
    let r = match (op, sec) {
        (Op::Stage, Section::Staged) => return Err("Already staged".into()),
        (Op::Unstage, Section::Unstaged | Section::Untracked) => {
            return Err("Not staged".into())
        }
        (Op::Discard, Section::Staged) => {
            return Err("Unstage it first (s), then discard".into())
        }
        (Op::Stage, _) => match hunk {
            Some(h) => git_run(root, &["apply", "--cached", "--whitespace=nowarn", "-"], Some(&patch(h))),
            None => git_run(root, &with_paths(&["add", "-A"]), None),
        },
        (Op::Unstage, Section::Staged) => match hunk {
            Some(h) => git_run(
                root,
                &["apply", "--cached", "-R", "--whitespace=nowarn", "-"],
                Some(&patch(h)),
            ),
            None if has_head => git_run(root, &with_paths(&["restore", "--staged"]), None),
            None => git_run(root, &with_paths(&["rm", "--cached", "-r", "-q"]), None),
        },
        (Op::Discard, Section::Unstaged) => match hunk {
            Some(h) => git_run(root, &["apply", "-R", "--whitespace=nowarn", "-"], Some(&patch(h))),
            None => git_run(root, &with_paths(&["restore"]), None),
        },
        (Op::Discard, Section::Untracked) => std::fs::remove_file(root.join(&file.path))
            .map(|_| String::new())
            .map_err(|e| format!("Couldn't delete {}: {e}", file.path)),
    };
    *DIRTY.lock() = true;
    r.map(|_| ())
}

/// Stage or unstage everything.
pub fn apply_all(root: &Path, op: Op, has_head: bool) -> Result<(), String> {
    let r = match op {
        Op::Stage => git_run(root, &["add", "-A"], None),
        Op::Unstage if has_head => git_run(root, &["reset", "-q"], None),
        Op::Unstage => git_run(root, &["rm", "--cached", "-r", "-q", "."], None),
        Op::Discard => return Err("Discard files one at a time".into()),
    };
    *DIRTY.lock() = true;
    r.map(|_| ())
}

/// Message of HEAD, for amending.
pub fn head_message(root: &Path) -> String {
    git(root, &["log", "-1", "--format=%B"])
        .map(|s| s.trim_end().to_string())
        .unwrap_or_default()
}

/// Commit in the background (hooks may take a while); reports via AppEvent::Committed.
pub fn commit(root: PathBuf, message: String, amend: bool) {
    std::thread::spawn(move || {
        let mut args = vec!["commit", "-q", "--cleanup=strip", "-F", "-"];
        if amend {
            args.push("--amend");
        }
        let r = git_run(&root, &args, Some(&message))
            .map(|_| {
                git(&root, &["log", "-1", "--format=%h %s"])
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            })
            .map_err(|e| (e, message.clone()));
        *DIRTY.lock() = true;
        send_event(AppEvent::Committed { result: r, amend });
    });
}

/// Pull or push the current branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    Pull,
    Push,
}

/// Pull / push in the background (the network may take a while); reports via
/// AppEvent::Synced with a short summary or git's error.
pub fn sync(root: PathBuf, op: Sync) {
    std::thread::spawn(move || {
        let r = match op {
            Sync::Pull => pull(&root),
            Sync::Push => push(&root),
        };
        *DIRTY.lock() = true;
        send_event(AppEvent::Synced { op, result: r });
    });
}

fn count(root: &Path, range: &str) -> usize {
    git(root, &["rev-list", "--count", range])
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn commits(n: usize) -> String {
    if n == 1 { "1 commit".into() } else { format!("{n} commits") }
}

fn pull(root: &Path) -> Result<String, String> {
    if git(root, &["rev-parse", "--verify", "-q", "@{upstream}"]).is_none() {
        return Err("No upstream to pull from".into());
    }
    let before = git(root, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
    git_run(root, &["pull", "-q"], None)?;
    let n = before.map(|b| count(root, &format!("{b}..HEAD"))).unwrap_or(0);
    Ok(if n == 0 { "Already up to date".into() } else { format!("Pulled {}", commits(n)) })
}

fn push(root: &Path) -> Result<String, String> {
    let Some(branch) = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).map(|s| s.trim().to_string())
    else {
        return Err("Not on a branch".into());
    };
    if git(root, &["rev-parse", "--verify", "-q", "@{upstream}"]).is_none() {
        // first push: set the upstream, like `p` in the pull request flow
        let remote = crate::pr::remote_for(root, &branch).ok_or("No remote to push to")?;
        git_run(root, &["push", "-q", "-u", &remote, "HEAD"], None)?;
        return Ok(format!("Pushed to {remote}/{branch}"));
    }
    let n = count(root, "@{upstream}..HEAD");
    if n == 0 {
        return Ok("Nothing to push".into());
    }
    git_run(root, &["push", "-q"], None)?;
    Ok(format!("Pushed {}", commits(n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunk_headers() {
        assert_eq!(hunk_starts("@@ -12,7 +14,9 @@ fn main()"), Some((12, 14)));
        assert_eq!(hunk_starts("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(hunk_starts("@@ -0,0 +1,3 @@"), Some((0, 1)));
    }

    #[test]
    fn parses_paths() {
        assert_eq!(path_from_diff_git("diff --git a/src/a b.rs b/src/a b.rs"), "src/a b.rs");
    }

    #[test]
    fn parses_diff() {
        let text = "diff --git a/x.rs b/x.rs\nindex 1..2 100644\n--- a/x.rs\n+++ b/x.rs\n@@ -1,2 +1,2 @@ fn a\n ctx\n-old\n+new\n@@ -10 +10,2 @@\n x\n+y\n\\ No newline at end of file\ndiff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+hi\n";
        let f = parse_diff(text);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].path, "x.rs");
        assert_eq!(f[0].hunks.len(), 2);
        assert_eq!((f[0].added, f[0].removed), (2, 1));
        assert_eq!(f[0].hunks[0].context(), "fn a");
        assert_eq!(f[0].hunks[1].raw.len(), 3);
        assert_eq!(f[0].hunks[1].lines.len(), 2);
        assert_eq!(f[1].status, "A");
        assert!(f[1].header.ends_with("+++ b/n\n"));
    }

    /// End to end against a throwaway repo: hunk stage/unstage/discard, file ops, commit.
    #[test]
    fn hunk_ops_roundtrip() {
        let dir = std::env::temp_dir().join(format!("wezzy-diff-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sh = |args: &[&str]| git_run(&dir, args, None).unwrap();
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@t"]);
        sh(&["config", "user.name", "t"]);
        let body: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("a.txt"), &body).unwrap();

        // unborn branch: stage + unstage a whole new file
        let s = compute(&dir);
        assert!(!s.has_head);
        assert_eq!(s.untracked.len(), 1);
        apply(&dir, Op::Stage, Section::Untracked, &s.untracked[0], None, false).unwrap();
        let s = compute(&dir);
        assert_eq!(s.staged.len(), 1);
        apply(&dir, Op::Unstage, Section::Staged, &s.staged[0], None, false).unwrap();
        assert_eq!(compute(&dir).untracked.len(), 1);
        sh(&["add", "a.txt"]);
        sh(&["commit", "-q", "-m", "init"]);

        // two separate hunks
        let edited = body.replace("line 2\n", "line two\n").replace("line 38\n", "line 38\nadded\n");
        std::fs::write(dir.join("a.txt"), &edited).unwrap();
        let s = compute(&dir);
        assert_eq!(s.unstaged[0].hunks.len(), 2);

        // stage only the second hunk
        apply(&dir, Op::Stage, Section::Unstaged, &s.unstaged[0], Some(1), true).unwrap();
        let s = compute(&dir);
        assert_eq!(s.staged[0].hunks.len(), 1);
        assert_eq!(s.unstaged[0].hunks.len(), 1);
        assert_eq!(s.staged[0].added, 1);

        // unstage it again
        apply(&dir, Op::Unstage, Section::Staged, &s.staged[0], Some(0), true).unwrap();
        let s = compute(&dir);
        assert!(s.staged.is_empty());
        assert_eq!(s.unstaged[0].hunks.len(), 2);

        // discard the first hunk from the worktree
        apply(&dir, Op::Discard, Section::Unstaged, &s.unstaged[0], Some(0), true).unwrap();
        let now = std::fs::read_to_string(dir.join("a.txt")).unwrap();
        assert!(now.contains("line 2\n") && now.contains("added\n"));

        // discarding staged is refused
        apply(&dir, Op::Stage, Section::Unstaged, &compute(&dir).unstaged[0], None, true).unwrap();
        let s = compute(&dir);
        assert!(apply(&dir, Op::Discard, Section::Staged, &s.staged[0], None, true).is_err());

        // commit through the same path the panel uses
        git_run(&dir, &["commit", "-q", "--cleanup=strip", "-F", "-"], Some("second\n\nbody")).unwrap();
        let s = compute(&dir);
        assert!(s.is_clean());
        assert!(s.head.ends_with("second"));
        assert_eq!(head_message(&dir), "second\n\nbody");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
