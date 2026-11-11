//! Finds AI coding agents running in any pane of the mux, and their status.
//!
//! Status comes from harness hooks when available (hooks.rs: exact). Without
//! hooks we fall back to heuristics: title spinners, recent output, bells.

use crate::app::{clear_alert, has_alert, last_output};
use crate::hooks::{self, HookStatus};
use mux::pane::{CachePolicy, PaneId};
use mux::Mux;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

/// Agents that finished or asked for you while you were looking elsewhere,
/// until you go to their pane. Main thread only, but a static like PANE_ALERTS.
static UNSEEN: Mutex<Option<HashSet<PaneId>>> = Mutex::new(None);

/// True when it wasn't already unseen.
pub fn mark_unseen(pane_id: PaneId) -> bool {
    let added = UNSEEN.lock().get_or_insert_with(HashSet::new).insert(pane_id);
    crate::notify::sync_badge();
    added
}

pub fn mark_seen(pane_id: PaneId) {
    let removed = UNSEEN.lock().as_mut().is_some_and(|s| s.remove(&pane_id));
    if removed {
        crate::notify::withdraw(pane_id);
        crate::notify::sync_badge();
    }
}

/// Agents waiting on you: they asked or finished since you last went to them.
/// Closed panes are dropped. The Dock badge shows this.
pub fn waiting_count() -> usize {
    let Some(mux) = Mux::try_get() else {
        return 0;
    };
    let mut guard = UNSEEN.lock();
    let Some(s) = guard.as_mut() else {
        return 0;
    };
    s.retain(|id| mux.get_pane(*id).is_some());
    s.len()
}

fn is_unseen(pane_id: PaneId) -> bool {
    UNSEEN.lock().as_ref().is_some_and(|s| s.contains(&pane_id))
}

const KNOWN: &[(&str, &str)] = &[
    ("claude", "Claude"),
    ("pi", "Pi"),
    ("codex", "Codex"),
    ("aider", "Aider"),
    ("gemini", "Gemini"),
    ("opencode", "OpenCode"),
    ("amp", "Amp"),
    ("goose", "Goose"),
    ("cursor-agent", "Cursor"),
];

pub fn label_for(name: &str) -> Option<&'static str> {
    KNOWN.iter().find(|(k, _)| *k == name).map(|(_, l)| *l)
}

pub fn is_agent_process(name: &str) -> bool {
    let n = basename(name).trim_end_matches(".js").to_ascii_lowercase();
    KNOWN.iter().any(|(k, _)| *k == n)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Working,
    /// Blocked on you: permission prompt / question.
    NeedsInput,
    /// Finished its turn; your move.
    Done,
    Idle,
}

#[derive(Clone, Debug)]
pub struct Agent {
    pub pane_id: PaneId,
    pub kind: &'static str,
    /// Pane title with spinner glyphs trimmed; agents usually put the current task here.
    pub title: String,
    /// What it's doing / needs right now (from hooks), e.g. "Editing app.rs".
    pub detail: Option<String>,
    pub project: String,
    pub workspace: String,
    pub status: Status,
    /// Finished or asked for you since you last went to its pane.
    pub unseen: bool,
    pub focused: bool,
}

fn basename(s: &str) -> &str {
    Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(s)
}

fn classify(names: &[&str]) -> Option<&'static str> {
    names.iter().find_map(|n| {
        let n = basename(n).trim_end_matches(".js").to_ascii_lowercase();
        label_for(&n)
    })
}

pub fn collect(focused_pane: Option<PaneId>) -> Vec<Agent> {
    let mux = Mux::get();
    let mut out = vec![];
    for pane in mux.iter_panes() {
        let pane_id = pane.pane_id();
        let proc = crate::procs::foreground(&pane);
        let proc_kind = proc.as_ref().and_then(|info| {
            let exe = info.executable.to_string_lossy().to_string();
            let mut names = vec![info.name.as_str(), exe.as_str()];
            names.extend(info.argv.iter().take(2).map(String::as_str));
            classify(&names)
        });
        let hook = hooks::state(pane_id);

        // A hook record outlives a killed agent; trust the process table when it
        // clearly shows something else in the foreground.
        let hook = match (&hook, &proc, proc_kind) {
            (Some(_), Some(_), None) => {
                hooks::forget(pane_id);
                None
            }
            _ => hook,
        };
        let kind = match (proc_kind, &hook) {
            (Some(k), _) => k,
            (None, Some(h)) => label_for(&h.harness).unwrap_or("Agent"),
            (None, None) => continue,
        };

        let workspace = mux
            .resolve_pane_id(pane_id)
            .and_then(|(_, w, _)| mux.get_window(w).map(|w| w.get_workspace().to_string()))
            .unwrap_or_default();
        // the agent's own cwd (from hooks) beats the shell's last OSC 7
        let project = hook
            .as_ref()
            .and_then(|h| h.cwd.as_ref())
            .and_then(|c| c.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .or_else(|| {
                pane.get_current_working_dir(CachePolicy::AllowStale)
                    .map(|u| basename(u.path().trim_end_matches('/')).to_string())
            })
            .unwrap_or_default();
        let raw_title = pane.get_title();
        let title = raw_title
            .trim_start_matches(|c: char| !c.is_alphanumeric())
            .trim()
            .to_string();

        let (status, detail) = match &hook {
            Some(h) => {
                let s = match h.status {
                    HookStatus::Working => Status::Working,
                    HookStatus::NeedsInput => Status::NeedsInput,
                    HookStatus::Done => Status::Done,
                    HookStatus::Idle => Status::Idle,
                };
                if s != Status::NeedsInput {
                    clear_alert(pane_id);
                }
                (s, h.detail.clone())
            }
            None => (guess(pane_id, kind, &raw_title), None),
        };

        // back at work: whatever it finished before is old news
        if status == Status::Working {
            mark_seen(pane_id);
        }
        let unseen = is_unseen(pane_id);

        out.push(Agent {
            pane_id,
            kind,
            title,
            detail,
            project,
            workspace,
            status,
            unseen,
            focused: Some(pane_id) == focused_pane,
        });
    }
    out.sort_by_key(|a| a.pane_id);
    out
}

/// Heuristic status for agents without hooks.
fn guess(pane_id: PaneId, kind: &str, raw_title: &str) -> Status {
    let spinner = raw_title
        .chars()
        .next()
        .is_some_and(|c| ('\u{2801}'..='\u{28FF}').contains(&c));
    let recent = last_output(pane_id).is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
    let working = spinner || (kind != "Claude" && recent);
    if working {
        clear_alert(pane_id);
        Status::Working
    } else if has_alert(pane_id) {
        Status::NeedsInput
    } else {
        Status::Idle
    }
}
