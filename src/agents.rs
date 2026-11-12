//! Finds AI coding agents running in any pane of the mux, and their status.
//!
//! Status comes from harness hooks when available (hooks.rs: exact). Without
//! hooks we fall back to heuristics: title spinners, recent output, bells.

use crate::app::{clear_alert, has_alert, last_output};
use crate::hooks::{self, HookStatus};
use mux::pane::{CachePolicy, PaneId};
use mux::Mux;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

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

/// Each agent's last status and when it took it, so the attention queue can
/// put whoever has waited longest first. Main thread only.
static SINCE: Mutex<Option<HashMap<PaneId, (Status, Instant)>>> = Mutex::new(None);

fn since(pane_id: PaneId, status: Status) -> Instant {
    let mut guard = SINCE.lock();
    let m = guard.get_or_insert_with(HashMap::new);
    match m.get(&pane_id) {
        Some((s, at)) if *s == status => *at,
        _ => {
            let now = Instant::now();
            m.insert(pane_id, (status, now));
            now
        }
    }
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
    /// What you named it ("auth refactor"); see `names`.
    pub name: Option<String>,
    /// Pane title with spinner glyphs trimmed; agents usually put the current task here.
    pub title: String,
    /// What it's doing / needs right now (from hooks), e.g. "Editing app.rs".
    pub detail: Option<String>,
    /// Claude subagents it has running (from hooks).
    pub subagents: Vec<hooks::Subagent>,
    pub project: String,
    pub workspace: String,
    pub status: Status,
    /// Finished or asked for you since you last went to its pane.
    pub unseen: bool,
    pub focused: bool,
    /// The permission dialog it's showing (from hooks).
    pub request: Option<hooks::Request>,
    /// Its hook record's time (unix secs); 0 without hooks. Changes with
    /// every event the agent reports.
    pub hook_ts: u64,
    /// When it took its current status (as first seen by this app).
    pub since: Instant,
}

impl Agent {
    /// How the UI names this agent: your name for it, else its kind ("Claude").
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(self.kind)
    }
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

        let subagents = hook.as_ref().map(|h| h.subagents.clone()).unwrap_or_default();
        let request = hook.as_ref().and_then(|h| h.request.clone());
        let hook_ts = hook.as_ref().map_or(0, |h| h.ts);
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
            name: names::get(pane_id),
            title,
            detail,
            subagents,
            project,
            workspace,
            status,
            unseen,
            focused: Some(pane_id) == focused_pane,
            request,
            hook_ts,
            since: since(pane_id, status),
        });
    }
    if let Some(m) = SINCE.lock().as_mut() {
        m.retain(|id, _| out.iter().any(|a| a.pane_id == *id));
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

/// Names you give agents (Rename agent). A name belongs to the pane, keyed by
/// the session server's pane id, and is kept in
/// `~/.local/state/wezzy/agent-names.json` so it outlives an app restart for as
/// long as the pane lives.
///
/// It can't live on the server pane itself: the mux protocol has no request
/// that stores data there (user vars only flow from the pane's own output to
/// the client). Server pane ids restart from 0 with a new server, so the file
/// records which server it belongs to (its pid) and is ignored under another;
/// `session.rs` prunes ids the server no longer lists.
pub mod names {
    use mux::pane::PaneId;
    use parking_lot::Mutex;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    #[derive(Serialize, Deserialize)]
    struct Stored {
        /// The session server's pid (or ours, with an in-process mux).
        server: String,
        /// Server pane id -> name.
        names: BTreeMap<PaneId, String>,
    }

    /// Loaded on first use once the server is known (its pid file exists).
    static NAMES: Mutex<Option<Stored>> = Mutex::new(None);

    fn file() -> PathBuf {
        crate::paths::state_dir().join("agent-names.json")
    }

    fn server_id() -> Option<String> {
        if !crate::session::persistent() {
            return Some(std::process::id().to_string());
        }
        let pid = std::fs::read_to_string(crate::paths::data_dir().join("pid")).ok()?;
        let pid = pid.trim();
        (!pid.is_empty()).then(|| pid.to_string())
    }

    fn with<R>(f: impl FnOnce(&mut Stored) -> R) -> Option<R> {
        let mut guard = NAMES.lock();
        if guard.is_none() {
            let server = server_id()?;
            let stored = std::fs::read(file())
                .ok()
                .and_then(|b| serde_json::from_slice::<Stored>(&b).ok())
                .filter(|s| s.server == server)
                .unwrap_or(Stored {
                    server,
                    names: BTreeMap::new(),
                });
            *guard = Some(stored);
        }
        guard.as_mut().map(f)
    }

    fn save(s: &Stored) {
        let path = file();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let Ok(json) = serde_json::to_vec_pretty(s) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            std::fs::rename(&tmp, &path).ok();
        }
    }

    /// The name you gave the agent in this (local) pane.
    pub fn get(pane_id: PaneId) -> Option<String> {
        let remote = crate::session::remote_pane_id(pane_id)?;
        with(|s| s.names.get(&remote).cloned()).flatten()
    }

    /// Name the agent in this (local) pane; blank clears it.
    pub fn set(pane_id: PaneId, name: &str) {
        let Some(remote) = crate::session::remote_pane_id(pane_id) else {
            return;
        };
        let name = name.trim().to_string();
        with(|s| {
            let changed = if name.is_empty() {
                s.names.remove(&remote).is_some()
            } else {
                s.names.insert(remote, name.clone()).as_ref() != Some(&name)
            };
            if changed {
                save(s);
            }
        });
    }

    /// Forget names of server panes that no longer exist.
    pub fn retain(live: impl Fn(PaneId) -> bool) {
        with(|s| {
            let before = s.names.len();
            s.names.retain(|id, _| live(*id));
            if s.names.len() != before {
                save(s);
            }
        });
    }
}
