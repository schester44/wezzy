//! Exact agent status from harness hooks.
//!
//! `assets/hooks/agent-hook.sh` is installed to ~/.local/share/wezzy/bin and
//! registered as a Claude Code hook. On every lifecycle event it writes the
//! payload to ~/.local/state/wezzy/agents/<WEZTERM_PANE>.json. We poll that
//! directory and turn the latest event per pane into a `HookState`.
//!
//! WEZTERM_PANE is the session server's pane id, so it's mapped to our local
//! pane id via the client domain.

use crate::app::{send_event, AppEvent};
use mux::pane::PaneId;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const HOOK_SCRIPT: &str = include_str!("../assets/hooks/agent-hook.sh");

/// Claude Code events we subscribe to.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "PreCompact",
    "Stop",
    "SessionEnd",
];

#[derive(Clone, Debug, PartialEq)]
pub enum HookStatus {
    Working,
    /// Blocked on you: a permission prompt or a question.
    NeedsInput,
    /// Finished its turn; your move.
    Done,
    Idle,
}

#[derive(Clone, Debug)]
pub struct HookState {
    pub harness: String,
    pub status: HookStatus,
    /// Human-readable detail: "Editing app.rs", "needs permission to use Bash", ...
    pub detail: Option<String>,
    /// The session's current directory, as the harness reports it (may differ
    /// from the pane's: an agent that moved into a worktree).
    pub cwd: Option<PathBuf>,
}

/// local pane id -> latest hook state
static STATES: Mutex<Option<HashMap<PaneId, HookState>>> = Mutex::new(None);

pub fn state(pane_id: PaneId) -> Option<HookState> {
    STATES.lock().as_ref()?.get(&pane_id).cloned()
}

/// Forget a pane's state (e.g. the agent process is gone).
pub fn forget(pane_id: PaneId) {
    if let Some(m) = STATES.lock().as_mut() {
        m.remove(&pane_id);
    }
}

fn data_dir() -> PathBuf {
    crate::paths::data_dir()
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::HOME_DIR.join(".local/state"))
        .join("wezzy/agents")
}

pub fn script_path() -> PathBuf {
    data_dir().join("bin/agent-hook")
}

/// Keep the installed script in sync with this build (it's ours to overwrite).
pub fn write_script() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = script_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    if std::fs::read_to_string(&path).ok().as_deref() != Some(HOOK_SCRIPT) {
        std::fs::write(&path, HOOK_SCRIPT)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn claude_settings_path() -> PathBuf {
    config::HOME_DIR.join(".claude/settings.json")
}

fn hook_command() -> String {
    format!("{} claude", script_path().display())
}

/// Are our hooks registered in ~/.claude/settings.json? (cached for a few seconds)
pub fn claude_hooks_installed() -> bool {
    static CACHE: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
    let mut c = CACHE.lock();
    if let Some((at, v)) = *c {
        if at.elapsed() < Duration::from_secs(3) {
            return v;
        }
    }
    let v = check_claude_hooks();
    *c = Some((std::time::Instant::now(), v));
    v
}

fn check_claude_hooks() -> bool {
    let Ok(text) = std::fs::read_to_string(claude_settings_path()) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let cmd = hook_command();
    CLAUDE_EVENTS.iter().all(|ev| has_command(&v["hooks"][ev], &cmd))
}

fn has_command(groups: &Value, cmd: &str) -> bool {
    groups.as_array().is_some_and(|groups| {
        groups.iter().any(|g| {
            g["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|h| h["command"].as_str() == Some(cmd)))
        })
    })
}

/// Merge our hook into every event in ~/.claude/settings.json, keeping
/// everything else as-is. Writes a one-time backup first.
pub fn install_claude_hooks() -> anyhow::Result<()> {
    write_script()?;
    let path = claude_settings_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".into());
    let mut root: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("{} isn't valid JSON: {e}", path.display()))?;
    let backup = path.with_extension("json.wezzy-backup");
    if path.exists() && !backup.exists() {
        std::fs::copy(&path, &backup)?;
    }
    let cmd = hook_command();
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json isn't an object"))?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()));
    for ev in CLAUDE_EVENTS {
        let groups = hooks
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("settings.json `hooks` isn't an object"))?
            .entry(*ev)
            .or_insert_with(|| Value::Array(vec![]));
        // drop entries from older installs (e.g. a previous app name/path)
        if let Some(arr) = groups.as_array_mut() {
            arr.retain(|g| {
                !g["hooks"].as_array().is_some_and(|hs| {
                    hs.iter().any(|h| {
                        h["command"]
                            .as_str()
                            .is_some_and(|c| c.contains("/bin/agent-hook") && c != cmd)
                    })
                })
            });
        }
        if !has_command(groups, &cmd) {
            groups
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("hooks.{ev} isn't an array"))?
                .push(serde_json::json!({
                    "hooks": [{ "type": "command", "command": cmd, "timeout": 5 }]
                }));
        }
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.wezzy-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)? + "\n")?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

// -- watching ----------------------------------------------------------------

pub fn start() {
    if let Err(e) = write_script() {
        log::warn!("couldn't write agent hook script: {e:#}");
    }
    // hooks registered before the dekita -> wezzy rename point at the old path;
    // re-register them at the new one (install replaces our old entries)
    if std::fs::read_to_string(claude_settings_path())
        .is_ok_and(|t| t.contains("/.local/share/dekita/bin/agent-hook"))
    {
        if let Err(e) = install_claude_hooks() {
            log::warn!("couldn't move agent hooks to the new path: {e:#}");
        }
    }
    std::thread::spawn(|| loop {
        poll();
        std::thread::sleep(Duration::from_millis(400));
    });
}

fn poll() {
    let Ok(entries) = std::fs::read_dir(state_dir()) else {
        return;
    };
    let mut next: HashMap<PaneId, HookState> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(remote) = v["pane"].as_u64() else {
            continue;
        };
        let Some(local) = crate::session::local_pane_id(remote as PaneId) else {
            continue;
        };
        let harness = v["harness"].as_str().unwrap_or("agent").to_string();
        let Some((status, detail)) = interpret(&harness, &v["payload"]) else {
            // session ended: drop the record
            std::fs::remove_file(&path).ok();
            continue;
        };
        next.insert(
            local,
            HookState {
                harness,
                status,
                detail,
                cwd: v["payload"]["cwd"].as_str().map(PathBuf::from),
            },
        );
    }

    let mut guard = STATES.lock();
    let prev = guard.take().unwrap_or_default();
    for (pane, st) in &next {
        let before = prev.get(pane).map(|p| &p.status);
        if before != Some(&st.status) {
            send_event(AppEvent::AgentStatusChanged {
                pane_id: *pane,
                status: st.status.clone(),
                detail: st.detail.clone(),
            });
        }
    }
    *guard = Some(next);
}

/// Map a harness event to a status. `None` means the session ended.
fn interpret(harness: &str, p: &Value) -> Option<(HookStatus, Option<String>)> {
    if harness != "claude" {
        return Some((HookStatus::Idle, None));
    }
    let event = p["hook_event_name"].as_str().unwrap_or("");
    Some(match event {
        "SessionEnd" => return None,
        "SessionStart" => (HookStatus::Idle, None),
        "UserPromptSubmit" => (HookStatus::Working, Some("Thinking…".into())),
        "PreToolUse" => (HookStatus::Working, Some(describe_tool(p))),
        "PostToolUse" => (HookStatus::Working, Some("Thinking…".into())),
        "PreCompact" => (HookStatus::Working, Some("Compacting context…".into())),
        "Stop" => (HookStatus::Done, None),
        "Notification" => {
            let msg = p["message"].as_str().unwrap_or("").to_string();
            let kind = p["notification_type"].as_str().unwrap_or("");
            let lower = msg.to_lowercase();
            if kind == "idle_prompt" || lower.contains("waiting for your input") {
                // Claude's "still waiting" nudge after a finished turn
                (HookStatus::Done, None)
            } else {
                (HookStatus::NeedsInput, (!msg.is_empty()).then_some(msg))
            }
        }
        _ => (HookStatus::Working, None),
    })
}

fn describe_tool(p: &Value) -> String {
    let tool = p["tool_name"].as_str().unwrap_or("tool");
    let input = &p["tool_input"];
    let file = || {
        input["file_path"]
            .as_str()
            .or(input["notebook_path"].as_str())
            .and_then(|f| std::path::Path::new(f).file_name())
            .map(|f| f.to_string_lossy().to_string())
    };
    let clip = |s: &str| {
        let s = s.lines().next().unwrap_or("").trim();
        if s.chars().count() > 48 {
            format!("{}…", s.chars().take(47).collect::<String>())
        } else {
            s.to_string()
        }
    };
    match tool {
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            format!("Editing {}", file().unwrap_or_else(|| "files".into()))
        }
        "Read" => format!("Reading {}", file().unwrap_or_else(|| "files".into())),
        "Bash" => input["description"]
            .as_str()
            .map(clip)
            .or_else(|| input["command"].as_str().map(|c| format!("$ {}", clip(c))))
            .unwrap_or_else(|| "Running a command".into()),
        "Grep" | "Glob" => "Searching code".into(),
        "WebFetch" | "WebSearch" => "Browsing the web".into(),
        "Task" | "Agent" => input["description"]
            .as_str()
            .map(|d| format!("Subagent: {}", clip(d)))
            .unwrap_or_else(|| "Running a subagent".into()),
        "TodoWrite" => "Updating todos".into(),
        other => format!("Using {other}"),
    }
}
