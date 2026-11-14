//! Exact agent status from harness hooks.
//!
//! Every supported harness reports its lifecycle events into one record per
//! pane, ~/.local/state/wezzy/agents/<WEZTERM_PANE>.json:
//!
//! ```json
//! {"harness":"claude","pane":3,"ts":1731340000,"payload":{...}}
//! ```
//!
//! How the record gets written depends on the harness (`Wiring`):
//!
//! - **Claude Code, Codex**: `assets/hooks/agent-hook.sh` is installed to
//!   ~/.local/share/wezzy/bin and registered as a command hook in the
//!   harness's JSON hooks file. The payload is the harness's own hook input.
//! - **pi, opencode**: a small extension/plugin of ours in the harness's
//!   plugin directory writes the record itself. The payload is
//!   `{"event", "cwd", "tool", "input", "message", ...}` with the harness's
//!   native event names.
//!
//! We poll the directory and map the latest event per pane to a `HookState`
//! with a per-harness `interpret_*`; polling and pane id mapping are shared.
//!
//! Events from inside a Claude subagent carry an `agent_id`; the script writes
//! those to <WEZTERM_PANE>.<agent_id>.json instead, and they show up as the
//! pane's `subagents`. Claude's own per-subagent meta file
//! (<session>/subagents/agent-<id>.meta.json) gives the Agent tool's
//! description and says when it finished; it's read best-effort.
//!
//! WEZTERM_PANE is the session server's pane id, so it's mapped to our local
//! pane id via the client domain.

use crate::app::{send_event, AppEvent};
use mux::pane::PaneId;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HOOK_SCRIPT: &str = include_str!("../assets/hooks/agent-hook.sh");
const PI_EXTENSION: &str = include_str!("../assets/hooks/pi-extension.ts");
const OPENCODE_PLUGIN: &str = include_str!("../assets/hooks/opencode-plugin.js");
/// First line of every plugin file we write; how we recognize ours.
const PLUGIN_MARKER: &str = "wezzy agent status";

/// Claude Code events we subscribe to.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Notification",
    "PreCompact",
    "Stop",
    "SessionEnd",
    "SubagentStart",
    "SubagentStop",
];

/// A subagent record with no events for this long is assumed dead (killed
/// before SubagentStop fired).
const SUBAGENT_STALE: Duration = Duration::from_secs(30 * 60);

/// Codex events we subscribe to.
const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "Stop",
    "Interrupt",
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
    /// The agent's working directory, as the harness reports it (may differ
    /// from the pane's: an agent that moved into a worktree).
    pub cwd: Option<PathBuf>,
    /// Subagents this agent has running, oldest first.
    pub subagents: Vec<Subagent>,
    /// The permission dialog it's showing, while `NeedsInput` (Claude, Codex).
    pub request: Option<Request>,
    /// When the record was written (unix secs): changes with every event.
    pub ts: u64,
}

/// A permission dialog an agent is showing: the tool it wants to run and its
/// full input, from the harness's `PermissionRequest` hook.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub tool: String,
    pub input: Value,
}

impl Request {
    fn from_payload(p: &Value) -> Option<Self> {
        let tool = p["tool_name"].as_str().filter(|t| !t.is_empty())?;
        Some(Request {
            tool: tool.to_string(),
            input: p["tool_input"].clone(),
        })
    }

    /// One line: what it wants to do ("Run database migration", "$ git push").
    pub fn summary(&self) -> String {
        if let Some(q) = self.input["questions"][0]["question"].as_str() {
            return clip(q);
        }
        if self.tool == "ExitPlanMode" {
            return "Plan ready".into();
        }
        self.input["description"]
            .as_str()
            .filter(|d| !d.trim().is_empty())
            .map(clip)
            .unwrap_or_else(|| describe_tool(&Value::String(self.tool.clone()), &self.input))
    }

    /// A yes/no dialog whose first option runs the tool (so ↩ approves and
    /// esc rejects). Claude's question and plan prompts aren't: they're
    /// answered in the pane.
    pub fn is_approval(&self) -> bool {
        !matches!(self.tool.as_str(), "AskUserQuestion" | "ExitPlanMode")
    }

    /// Everything you'd want to read before approving: the whole command,
    /// the file and the new text, the URL; otherwise the input as JSON.
    pub fn full(&self) -> String {
        let i = &self.input;
        let s = |k: &str| i[k].as_str().filter(|v| !v.is_empty());
        if let Some(plan) = s("plan") {
            return plan.to_string();
        }
        if let Some(qs) = i["questions"].as_array() {
            let mut out = vec![];
            for q in qs {
                out.push(q["question"].as_str().unwrap_or("").to_string());
                for o in q["options"].as_array().into_iter().flatten() {
                    out.push(format!("  · {}", o["label"].as_str().unwrap_or("")));
                }
            }
            return out.join("\n");
        }
        if let Some(cmd) = command_text(i) {
            return cmd;
        }
        if let Some(file) = s("file_path").or(s("filePath")).or(s("path")).or(s("notebook_path")) {
            return match s("new_string").or(s("content")).or(s("new_source")) {
                Some(text) => format!("{file}\n\n{text}"),
                None => file.to_string(),
            };
        }
        if let Some(url) = s("url") {
            return url.to_string();
        }
        match i {
            Value::Null => self.tool.clone(),
            v => serde_json::to_string_pretty(v).unwrap_or_default(),
        }
    }

    /// Worth a second look before approving: deletes, force pushes, database
    /// and infrastructure changes, privilege escalation, publishing. A
    /// heuristic over the command; anything else that isn't a plain file edit
    /// or read is approved directly.
    pub fn is_risky(&self) -> bool {
        let Some(cmd) = command_text(&self.input) else {
            // MCP tools act on the outside world in ways we can't read
            return self.tool.starts_with("mcp__");
        };
        let c = format!(" {} ", cmd.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "));
        const WORDS: &[&str] = &[
            " rm ", " rmdir ", " sudo ", " doas ", " su ", " dd ", " mkfs", " shred ", " chmod ", " chown ",
            " kill ", " pkill ", " killall ", "git push", "--force", "push -f", "reset --hard", "clean -f",
            "branch -d", "checkout -- ", "restore ", " drop ", "truncate ", "delete from", " delete ",
            "migrate", "prisma ", "terraform ", "pulumi ", "kubectl ", "helm ", " aws ", "gcloud ", " az ",
            "flyctl ", "heroku ", "vercel ", "deploy", " publish", "| sh", "| bash", "| zsh", "> /", "prod",
        ];
        WORDS.iter().any(|w| c.contains(w))
    }
}

/// A shell tool's command, joined if it came as argv.
fn command_text(input: &Value) -> Option<String> {
    match &input["command"] {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Array(a) if !a.is_empty() => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subagent {
    pub id: String,
    /// "Explore", "general-purpose", ...
    pub kind: String,
    /// What it was asked to do, as the Agent tool described it.
    pub description: Option<String>,
    pub status: HookStatus,
    pub detail: Option<String>,
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

// -- harnesses ---------------------------------------------------------------

/// Harnesses that report exact status.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Harness {
    Claude,
    Codex,
    Pi,
    OpenCode,
}

pub const HARNESSES: &[Harness] = &[Harness::Claude, Harness::Codex, Harness::Pi, Harness::OpenCode];

/// How a harness gets our events into the state directory.
enum Wiring {
    /// Our script as a command hook in a Claude-style JSON hooks file.
    HookFile {
        path: PathBuf,
        events: &'static [&'static str],
        timeout: u64,
    },
    /// A plugin file of ours in the harness's plugin directory.
    Plugin { path: PathBuf, source: &'static str },
}

impl Harness {
    /// The `harness` field in records, and the binary name.
    pub fn id(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::OpenCode => "opencode",
        }
    }

    pub fn label(self) -> &'static str {
        crate::agents::label_for(self.id()).unwrap_or("Agent")
    }

    fn from_id(id: &str) -> Option<Self> {
        HARNESSES.iter().copied().find(|h| h.id() == id)
    }

    fn wiring(self) -> Wiring {
        let home = &*config::HOME_DIR;
        let env_dir = |var: &str, default: PathBuf| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(default)
        };
        match self {
            Harness::Claude => Wiring::HookFile {
                path: home.join(".claude/settings.json"),
                events: CLAUDE_EVENTS,
                timeout: 5,
            },
            Harness::Codex => Wiring::HookFile {
                path: env_dir("CODEX_HOME", home.join(".codex")).join("hooks.json"),
                events: CODEX_EVENTS,
                // Codex caps SessionEnd and Interrupt hooks at 3s
                timeout: 3,
            },
            Harness::Pi => Wiring::Plugin {
                path: env_dir("PI_CODING_AGENT_DIR", home.join(".pi/agent")).join("extensions/wezzy.ts"),
                source: PI_EXTENSION,
            },
            Harness::OpenCode => Wiring::Plugin {
                path: env_dir("XDG_CONFIG_HOME", home.join(".config")).join("opencode/plugins/wezzy.js"),
                source: OPENCODE_PLUGIN,
            },
        }
    }

    fn hook_command(self) -> String {
        format!("{} {}", script_path().display(), self.id())
    }

    fn is_installed(self) -> bool {
        match self.wiring() {
            Wiring::HookFile { path, events, .. } => read_json(&path)
                .ok()
                .flatten()
                .is_some_and(|v| has_hooks(&v, events, &self.hook_command())),
            Wiring::Plugin { path, .. } => is_our_plugin(&path),
        }
    }

    fn install(self) -> anyhow::Result<()> {
        match self.wiring() {
            Wiring::HookFile { path, events, timeout } => {
                write_script()?;
                let cmd = self.hook_command();
                edit_json(&path, |root| merge_hooks(root, events, &cmd, timeout))
            }
            Wiring::Plugin { path, source } => install_plugin(&path, source),
        }
    }

    fn uninstall(self) -> anyhow::Result<()> {
        match self.wiring() {
            Wiring::HookFile { path, .. } => {
                if read_json(&path)?.is_some_and(|v| has_any_hook(&v)) {
                    edit_json(&path, remove_hooks)?;
                }
                Ok(())
            }
            Wiring::Plugin { path, .. } => {
                if is_our_plugin(&path) {
                    std::fs::remove_file(&path)?;
                }
                Ok(())
            }
        }
    }
}

/// A harness's hooks as the settings row shows them.
#[derive(Clone, Copy, Debug)]
pub struct HarnessHooks {
    pub harness: Harness,
    /// Its binary is on PATH.
    pub available: bool,
    pub installed: bool,
}

static STATUS_CACHE: Mutex<Option<(Instant, Vec<HarnessHooks>)>> = Mutex::new(None);

/// Every harness's hook state (cached for a few seconds).
pub fn harness_hooks() -> Vec<HarnessHooks> {
    let mut c = STATUS_CACHE.lock();
    if let Some((at, v)) = &*c {
        if at.elapsed() < Duration::from_secs(3) {
            return v.clone();
        }
    }
    let v: Vec<HarnessHooks> = HARNESSES
        .iter()
        .map(|&h| HarnessHooks {
            harness: h,
            available: crate::settings::on_path(h.id()),
            installed: h.is_installed(),
        })
        .collect();
    *c = Some((Instant::now(), v.clone()));
    v
}

/// Hooks are in for every harness on PATH (and at least one).
pub fn all_installed() -> bool {
    let hs = harness_hooks();
    hs.iter().any(|h| h.installed) && hs.iter().all(|h| h.installed || !h.available)
}

/// Install hooks for every supported harness on PATH. Returns the harnesses
/// done; fails if none could be.
pub fn install_all() -> anyhow::Result<Vec<Harness>> {
    let targets: Vec<Harness> = harness_hooks()
        .into_iter()
        .filter(|h| h.available || h.installed)
        .map(|h| h.harness)
        .collect();
    if targets.is_empty() {
        anyhow::bail!("no supported agent on PATH");
    }
    let r = for_each(&targets, Harness::install);
    *STATUS_CACHE.lock() = None;
    r
}

/// Remove our hooks from every harness that has them.
pub fn uninstall_all() -> anyhow::Result<Vec<Harness>> {
    let targets: Vec<Harness> = harness_hooks()
        .into_iter()
        .filter(|h| h.installed)
        .map(|h| h.harness)
        .collect();
    let r = for_each(&targets, Harness::uninstall);
    *STATUS_CACHE.lock() = None;
    r
}

fn for_each(hs: &[Harness], f: fn(Harness) -> anyhow::Result<()>) -> anyhow::Result<Vec<Harness>> {
    let mut done = vec![];
    let mut errors = vec![];
    for &h in hs {
        match f(h) {
            Ok(()) => done.push(h),
            Err(e) => errors.push(format!("{}: {e:#}", h.label())),
        }
    }
    if errors.is_empty() {
        Ok(done)
    } else {
        Err(anyhow::anyhow!(errors.join("; ")))
    }
}

// -- files -------------------------------------------------------------------

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::HOME_DIR.join(".local/state"))
        .join("wezzy/agents")
}

pub fn script_path() -> PathBuf {
    crate::paths::data_dir().join("bin/agent-hook")
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

/// `None` when the file doesn't exist.
fn read_json(path: &Path) -> anyhow::Result<Option<Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Some(Value::Object(Default::default()))),
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("{} isn't valid JSON: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Copy `path` aside once, before the first time we change it.
fn backup(path: &Path) -> anyhow::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".wezzy-backup");
    let backup = path.with_file_name(name);
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup)?;
    }
    Ok(())
}

/// Atomic write, via a sibling temp file.
fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".wezzy-tmp");
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Read-modify-write a JSON config, keeping everything we don't touch.
/// Leaves the file alone when nothing changed.
pub(crate) fn edit_json(path: &Path, f: impl FnOnce(&mut Value) -> anyhow::Result<()>) -> anyhow::Result<()> {
    let before = read_json(path)?;
    let mut root = before.clone().unwrap_or_else(|| Value::Object(Default::default()));
    f(&mut root)?;
    if before.as_ref() == Some(&root) {
        return Ok(());
    }
    backup(path)?;
    write_atomic(path, &(serde_json::to_string_pretty(&root)? + "\n"))
}

fn is_our_plugin(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .is_ok_and(|t| t.lines().next().is_some_and(|l| l.contains(PLUGIN_MARKER)))
}

fn install_plugin(path: &Path, source: &str) -> anyhow::Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(source) {
        return Ok(());
    }
    if !is_our_plugin(path) {
        backup(path)?;
    }
    write_atomic(path, source)
}

// -- hook file merging -------------------------------------------------------

/// Ours: any command running an `agent-hook` script from a `/bin/` dir,
/// including older installs under a previous app name.
fn is_our_command(c: &str) -> bool {
    c.contains("/bin/agent-hook")
}

pub(crate) fn has_command(groups: &Value, cmd: &str) -> bool {
    groups.as_array().is_some_and(|groups| {
        groups.iter().any(|g| {
            g["hooks"]
                .as_array()
                .is_some_and(|hs| hs.iter().any(|h| h["command"].as_str() == Some(cmd)))
        })
    })
}

fn has_hooks(root: &Value, events: &[&str], cmd: &str) -> bool {
    events.iter().all(|ev| has_command(&root["hooks"][ev], cmd))
}

fn has_any_hook(root: &Value) -> bool {
    root["hooks"].as_object().is_some_and(|evs| {
        evs.values().any(|groups| {
            groups.as_array().is_some_and(|gs| {
                gs.iter().any(|g| {
                    g["hooks"].as_array().is_some_and(|hs| {
                        hs.iter().any(|h| h["command"].as_str().is_some_and(is_our_command))
                    })
                })
            })
        })
    })
}

/// Add our command to every event in `root.hooks`, keeping everything else.
/// Our entries from older installs (another path) are replaced.
fn merge_hooks(root: &mut Value, events: &[&str], cmd: &str, timeout: u64) -> anyhow::Result<()> {
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("the file isn't a JSON object"))?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("`hooks` isn't an object"))?;
    for ev in events {
        let groups = hooks
            .entry(*ev)
            .or_insert_with(|| Value::Array(vec![]))
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("hooks.{ev} isn't an array"))?;
        strip_ours(groups, |c| c != cmd);
        if !has_command(&Value::Array(groups.clone()), cmd) {
            groups.push(serde_json::json!({
                "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }]
            }));
        }
    }
    Ok(())
}

/// Add one command hook to one event in `root.hooks`, for a hook that isn't
/// the status script (the board's, `board.rs`). Everything else is kept.
pub(crate) fn add_hook(root: &mut Value, event: &str, cmd: &str, timeout: u64) -> anyhow::Result<()> {
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("the file isn't a JSON object"))?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("`hooks` isn't an object"))?;
    let groups = hooks
        .entry(event)
        .or_insert_with(|| Value::Array(vec![]))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.{event} isn't an array"))?;
    if !has_command(&Value::Array(groups.clone()), cmd) {
        groups.push(serde_json::json!({
            "hooks": [{ "type": "command", "command": cmd, "timeout": timeout }]
        }));
    }
    Ok(())
}

/// Remove exactly `cmd` from one event; a group or event left empty goes too.
pub(crate) fn remove_hook(root: &mut Value, event: &str, cmd: &str) {
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
        return;
    };
    groups.retain_mut(|g| {
        let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hs.len();
        hs.retain(|h| h["command"].as_str() != Some(cmd));
        !(hs.is_empty() && before > 0)
    });
    if groups.is_empty() {
        hooks.remove(event);
    }
}

/// Remove every hook of ours; drop groups and events left empty by that.
fn remove_hooks(root: &mut Value) -> anyhow::Result<()> {
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    hooks.retain(|_, groups| {
        let Some(arr) = groups.as_array_mut() else {
            return true;
        };
        let before = arr.len();
        strip_ours(arr, |_| true);
        !(arr.is_empty() && before > 0)
    });
    Ok(())
}

/// Drop our hooks matching `which` from a list of hook groups; a group left
/// with no hooks goes too. Other people's hooks are never touched.
fn strip_ours(groups: &mut Vec<Value>, which: impl Fn(&str) -> bool) {
    groups.retain_mut(|g| {
        let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hs.len();
        hs.retain(|h| !h["command"].as_str().is_some_and(|c| is_our_command(c) && which(c)));
        !(hs.is_empty() && before > 0)
    });
}

// -- watching ----------------------------------------------------------------

pub fn start() {
    if let Err(e) = write_script() {
        log::warn!("couldn't write agent hook script: {e:#}");
    }
    // Keep installed plugins in sync with this build, and re-register hooks
    // that point at an old path (e.g. from before the dekita -> wezzy rename).
    for &h in HARNESSES {
        let refresh = match h.wiring() {
            Wiring::Plugin { path, source } => {
                is_our_plugin(&path) && std::fs::read_to_string(&path).ok().as_deref() != Some(source)
            }
            Wiring::HookFile { path, .. } => read_json(&path)
                .ok()
                .flatten()
                .is_some_and(|v| has_any_hook(&v) && !h.is_installed()),
        };
        if refresh {
            if let Err(e) = h.install() {
                log::warn!("couldn't update {} hooks: {e:#}", h.label());
            }
        }
    }
    std::thread::spawn(|| {
        // records written before launch are old news: show them, don't alert
        let mut since = unix_now();
        loop {
            let started = unix_now();
            poll(since);
            crate::activity::poll();
            since = started;
            std::thread::sleep(Duration::from_millis(400));
        }
    });
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read every hook record. A status change is announced (`AgentStatusChanged`:
/// chime, toast, notification) when it differs from the last poll, or for a
/// pane seen for the first time, only when its record was written at or after
/// `since` (unix secs). Otherwise relaunching the app, or a record coming back
/// after `forget`, would alert about an agent that finished long ago.
fn poll(since: u64) {
    let Ok(entries) = std::fs::read_dir(state_dir()) else {
        return;
    };
    let now = unix_now();
    let mut next: HashMap<PaneId, HookState> = HashMap::new();
    // main agent's session per pane, to drop subagents left over from an older one
    let mut sessions: HashMap<PaneId, String> = HashMap::new();
    let mut ended: Vec<u64> = vec![];
    // when each pane's record was written
    let mut written: HashMap<PaneId, u64> = HashMap::new();
    // payloads of finished turns, for the agent's closing words
    let mut finished: HashMap<PaneId, Value> = HashMap::new();
    // (remote pane, session, started, file, subagent)
    let mut subs: Vec<(u64, String, std::time::SystemTime, PathBuf, Subagent)> = vec![];
    // a subagent's pending permission request and its record time, by subagent id
    let mut sub_requests: HashMap<String, (Option<Request>, u64)> = HashMap::new();
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
        let p = &v["payload"];
        let harness = v["harness"].as_str().unwrap_or("agent").to_string();
        let session = p["session_id"].as_str().unwrap_or("").to_string();

        if let Some(id) = p["agent_id"].as_str() {
            let meta = subagent_meta(p, id);
            let ts = v["ts"].as_u64().unwrap_or(0);
            let gone = p["hook_event_name"].as_str() == Some("SubagentStop")
                || meta.as_ref().is_some_and(|m| m.finished)
                || now.saturating_sub(ts) > SUBAGENT_STALE.as_secs();
            if gone {
                std::fs::remove_file(&path).ok();
                std::fs::remove_file(path.with_extension("req")).ok();
                continue;
            }
            let (status, detail) = interpret(&harness, p).unwrap_or((HookStatus::Working, None));
            let request = (status == HookStatus::NeedsInput).then(|| read_request(&path)).flatten();
            let ts = v["ts"].as_u64().unwrap_or(0);
            let meta = meta.unwrap_or_default();
            let kind = meta
                .kind
                .or_else(|| p["agent_type"].as_str().map(str::to_string))
                .unwrap_or_else(|| "agent".into());
            let started = meta
                .started
                .or_else(|| entry.metadata().and_then(|m| m.created()).ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            sub_requests.insert(id.to_string(), (request, ts));
            subs.push((
                remote,
                session,
                started,
                path,
                Subagent {
                    id: id.to_string(),
                    kind,
                    description: meta.description,
                    status,
                    detail,
                },
            ));
            continue;
        }

        let Some(local) = crate::session::local_pane_id(remote as PaneId) else {
            continue;
        };
        let Some((status, detail)) = interpret(&harness, p) else {
            // session ended: drop the record, and its subagents' below
            std::fs::remove_file(&path).ok();
            std::fs::remove_file(path.with_extension("req")).ok();
            ended.push(remote);
            continue;
        };
        sessions.insert(local, session);
        let ts = v["ts"].as_u64().unwrap_or(0);
        written.insert(local, ts);
        let cwd = v["payload"]["cwd"].as_str().filter(|c| !c.is_empty()).map(PathBuf::from);
        let request = (status == HookStatus::NeedsInput).then(|| read_request(&path)).flatten();
        if status == HookStatus::Done {
            finished.insert(local, p.clone());
        }
        next.insert(
            local,
            HookState {
                harness,
                status,
                detail,
                subagents: vec![],
                cwd,
                request,
                ts,
            },
        );
    }

    subs.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.4.id.cmp(&b.4.id)));
    for (remote, session, _, path, sub) in subs {
        let ended = ended.contains(&remote);
        let Some(pane) = crate::session::local_pane_id(remote as PaneId) else {
            continue;
        };
        let stale = sessions.get(&pane).is_some_and(|s| !s.is_empty() && !session.is_empty() && *s != session);
        if ended || stale {
            std::fs::remove_file(&path).ok();
            std::fs::remove_file(path.with_extension("req")).ok();
            continue;
        }
        if let Some(st) = next.get_mut(&pane) {
            st.subagents.push(sub);
        }
    }
    // a subagent waiting on you means the agent is
    for st in next.values_mut() {
        if st.status != HookStatus::NeedsInput {
            if let Some(sub) = st.subagents.iter().find(|s| s.status == HookStatus::NeedsInput) {
                st.status = HookStatus::NeedsInput;
                st.detail = sub.detail.clone();
                if let Some((request, ts)) = sub_requests.remove(&sub.id) {
                    st.request = request;
                    st.ts = st.ts.max(ts);
                }
            }
        }
    }

    let mut guard = STATES.lock();
    let prev = guard.take().unwrap_or_default();
    for (pane, st) in &next {
        let changed = match prev.get(pane) {
            Some(p) => p.status != st.status,
            None => written.get(pane).is_some_and(|&ts| ts >= since),
        };
        if changed {
            // only on the change: it may read the transcript
            let said = (st.status == HookStatus::Done)
                .then(|| finished.get(pane).and_then(crate::activity::summary))
                .flatten();
            send_event(AppEvent::AgentStatusChanged {
                pane_id: *pane,
                status: st.status.clone(),
                detail: st.detail.clone(),
                said,
            });
        } else if prev
            .get(pane)
            .is_none_or(|p| p.subagents != st.subagents || p.request != st.request || p.ts != st.ts)
        {
            send_event(AppEvent::MuxChanged);
        }
    }
    *guard = Some(next);
}

/// The permission request saved beside a record (`<record>.req`, written by
/// the hook script while the dialog is up).
fn read_request(record: &Path) -> Option<Request> {
    let text = std::fs::read_to_string(record.with_extension("req")).ok()?;
    let p: Value = serde_json::from_str(&text).ok()?;
    Request::from_payload(&p)
}

#[derive(Default)]
struct SubagentMeta {
    kind: Option<String>,
    description: Option<String>,
    finished: bool,
    started: Option<std::time::SystemTime>,
}

/// Claude's meta file for a subagent, next to the session transcript:
/// <transcript dir>/<session>/subagents/agent-<id>.meta.json. Not a documented
/// format, so everything in it is optional.
fn subagent_meta(p: &Value, id: &str) -> Option<SubagentMeta> {
    let transcript = p["transcript_path"].as_str()?;
    let session = p["session_id"].as_str()?;
    let dir = std::path::Path::new(transcript)
        .parent()?
        .join(session)
        .join("subagents");
    let meta_path = dir.join(format!("agent-{id}.meta.json"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&meta_path).ok()?).ok()?;
    let started = std::fs::metadata(dir.join(format!("agent-{id}.jsonl")))
        .or_else(|_| std::fs::metadata(&meta_path))
        .and_then(|m| m.created())
        .ok();
    Some(SubagentMeta {
        kind: v["agentType"].as_str().map(str::to_string),
        description: v["description"].as_str().map(str::to_string),
        finished: v["finished"].as_bool().unwrap_or(false),
        started,
    })
}

// -- event -> status ---------------------------------------------------------

type Interpreted = Option<(HookStatus, Option<String>)>;

/// Map a harness event to a status. `None` means the session ended.
fn interpret(harness: &str, p: &Value) -> Interpreted {
    match Harness::from_id(harness) {
        Some(Harness::Claude) => interpret_claude(p),
        Some(Harness::Codex) => interpret_codex(p),
        Some(Harness::Pi) => interpret_pi(p),
        Some(Harness::OpenCode) => interpret_opencode(p),
        None => Some((HookStatus::Idle, None)),
    }
}

fn thinking() -> (HookStatus, Option<String>) {
    (HookStatus::Working, Some("Thinking…".into()))
}

fn compacting() -> (HookStatus, Option<String>) {
    (HookStatus::Working, Some("Compacting context…".into()))
}

fn interpret_claude(p: &Value) -> Interpreted {
    let event = p["hook_event_name"].as_str().unwrap_or("");
    Some(match event {
        "SessionEnd" => return None,
        "SessionStart" => (HookStatus::Idle, None),
        "UserPromptSubmit" | "PostToolUse" => thinking(),
        "PreToolUse" => (HookStatus::Working, Some(describe_tool(&p["tool_name"], &p["tool_input"]))),
        "PermissionRequest" => (HookStatus::NeedsInput, Some(permission_detail(p))),
        "PreCompact" => compacting(),
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

fn interpret_codex(p: &Value) -> Interpreted {
    let event = p["hook_event_name"].as_str().unwrap_or("");
    Some(match event {
        "SessionEnd" => return None,
        "SessionStart" => (HookStatus::Idle, None),
        "UserPromptSubmit" | "PostToolUse" | "PostCompact" => thinking(),
        "PreToolUse" => (HookStatus::Working, Some(describe_tool(&p["tool_name"], &p["tool_input"]))),
        "PermissionRequest" => (HookStatus::NeedsInput, Some(permission_detail(p))),
        "PreCompact" => compacting(),
        // an interrupted turn is over too: it's your move
        "Stop" | "Interrupt" => (HookStatus::Done, None),
        _ => (HookStatus::Working, None),
    })
}

fn interpret_pi(p: &Value) -> Interpreted {
    Some(match p["event"].as_str().unwrap_or("") {
        "session_shutdown" => return None,
        "session_start" => (HookStatus::Idle, None),
        "agent_start" | "tool_execution_end" => thinking(),
        "tool_execution_start" => (HookStatus::Working, Some(describe_tool(&p["tool"], &p["input"]))),
        "session_before_compact" => compacting(),
        "ui_prompt_start" => (HookStatus::NeedsInput, message(p)),
        "ui_prompt_end" if p["idle"].as_bool() == Some(true) => (HookStatus::Done, None),
        "ui_prompt_end" => thinking(),
        "agent_settled" => (HookStatus::Done, None),
        _ => (HookStatus::Working, None),
    })
}

fn interpret_opencode(p: &Value) -> Interpreted {
    Some(match p["event"].as_str().unwrap_or("") {
        "exit" => return None,
        "start" => (HookStatus::Idle, None),
        "session.busy" | "tool.execute.after" | "permission.replied" => thinking(),
        "tool.execute.before" => (HookStatus::Working, Some(describe_tool(&p["tool"], &p["input"]))),
        "permission.asked" => {
            let detail = message(p).or_else(|| {
                p["tool"]
                    .as_str()
                    .filter(|t| !t.is_empty())
                    .map(|t| format!("Permission to use {t}"))
            });
            (HookStatus::NeedsInput, detail)
        }
        "session.idle" | "session.error" => (HookStatus::Done, None),
        _ => (HookStatus::Working, None),
    })
}

/// A permission request's detail: the tool's own description of the call
/// (Bash's "Delete build output"), else what the call does.
fn permission_detail(p: &Value) -> String {
    p["tool_input"]["description"]
        .as_str()
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| describe_tool(&p["tool_name"], &p["tool_input"]))
}

fn message(p: &Value) -> Option<String> {
    p["message"].as_str().filter(|m| !m.is_empty()).map(str::to_string)
}

fn clip(s: &str) -> String {
    let s = s.lines().next().unwrap_or("").trim();
    if s.chars().count() > 48 {
        format!("{}…", s.chars().take(47).collect::<String>())
    } else {
        s.to_string()
    }
}

/// "Editing app.rs", "$ cargo test", ... from a tool call. Tool names differ
/// per harness (`Edit`, `edit`, `apply_patch`, ...), so match loosely.
fn describe_tool(tool: &Value, input: &Value) -> String {
    let tool = tool.as_str().unwrap_or("tool");
    let str_field = |keys: &[&str]| keys.iter().find_map(|k| input[*k].as_str().filter(|s| !s.is_empty()));
    let file = || {
        str_field(&["file_path", "filePath", "path", "notebook_path"])
            .or_else(|| patch_file(input))
            .and_then(|f| Path::new(f).file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "files".into())
    };
    let command = || match &input["command"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    };
    match tool.to_ascii_lowercase().as_str() {
        "edit" | "multiedit" | "write" | "notebookedit" | "apply_patch" | "patch" | "replace"
        | "write_file" => format!("Editing {}", file()),
        "read" | "read_file" | "view" => format!("Reading {}", file()),
        "bash" | "shell" | "exec_command" | "local_shell" | "run_shell_command" => str_field(&["description"])
            .map(clip)
            .or_else(|| command().filter(|c| !c.is_empty()).map(|c| format!("$ {}", clip(&c))))
            .unwrap_or_else(|| "Running a command".into()),
        "grep" | "glob" | "find" | "ls" | "list" | "search" => "Searching code".into(),
        "webfetch" | "websearch" | "web_search" | "fetch" => "Browsing the web".into(),
        "task" | "agent" => str_field(&["description"])
            .map(|d| format!("Subagent: {}", clip(d)))
            .unwrap_or_else(|| "Running a subagent".into()),
        "todowrite" | "update_plan" => "Updating todos".into(),
        _ => format!("Using {tool}"),
    }
}

/// The first file named in an apply_patch body ("*** Update File: src/a.rs").
fn patch_file(input: &Value) -> Option<&str> {
    let body = input["input"].as_str().or(input["patch"].as_str()).or(input["command"].as_str())?;
    body.lines().find_map(|l| {
        ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
            .iter()
            .find_map(|p| l.strip_prefix(p))
            .map(str::trim)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn st(harness: &str, p: Value) -> Interpreted {
        interpret(harness, &p)
    }

    #[test]
    fn claude_events() {
        assert_eq!(st("claude", json!({"hook_event_name": "SessionEnd"})), None);
        assert_eq!(
            st("claude", json!({"hook_event_name": "PreToolUse", "tool_name": "Edit", "tool_input": {"file_path": "/x/src/app.rs"}})),
            Some((HookStatus::Working, Some("Editing app.rs".into())))
        );
        assert_eq!(
            st("claude", json!({"hook_event_name": "Notification", "message": "Claude needs your permission to use Bash"})),
            Some((HookStatus::NeedsInput, Some("Claude needs your permission to use Bash".into())))
        );
        assert_eq!(
            st("claude", json!({"hook_event_name": "Notification", "notification_type": "idle_prompt", "message": "x"})),
            Some((HookStatus::Done, None))
        );
        assert_eq!(st("claude", json!({"hook_event_name": "Stop"})), Some((HookStatus::Done, None)));
    }

    #[test]
    fn codex_events() {
        assert_eq!(st("codex", json!({"hook_event_name": "SessionStart", "source": "startup"})), Some((HookStatus::Idle, None)));
        assert_eq!(st("codex", json!({"hook_event_name": "UserPromptSubmit"})), Some(thinking()));
        assert_eq!(
            st("codex", json!({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "cargo test --all"}})),
            Some((HookStatus::Working, Some("$ cargo test --all".into())))
        );
        assert_eq!(
            st("codex", json!({"hook_event_name": "PreToolUse", "tool_name": "apply_patch",
                "tool_input": {"command": "*** Begin Patch\n*** Update File: src/hooks.rs\n@@"}})),
            Some((HookStatus::Working, Some("Editing hooks.rs".into())))
        );
        assert_eq!(
            st("codex", json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
                "tool_input": {"command": "rm -rf target", "description": "Delete build output"}})),
            Some((HookStatus::NeedsInput, Some("Delete build output".into())))
        );
        assert_eq!(
            st("codex", json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
                "tool_input": {"command": "git push", "description": null}})),
            Some((HookStatus::NeedsInput, Some("$ git push".into())))
        );
        assert_eq!(st("codex", json!({"hook_event_name": "Stop"})), Some((HookStatus::Done, None)));
        assert_eq!(st("codex", json!({"hook_event_name": "Interrupt"})), Some((HookStatus::Done, None)));
        assert_eq!(st("codex", json!({"hook_event_name": "SessionEnd"})), None);
    }

    #[test]
    fn pi_events() {
        assert_eq!(st("pi", json!({"event": "session_start", "cwd": "/w"})), Some((HookStatus::Idle, None)));
        assert_eq!(st("pi", json!({"event": "agent_start"})), Some(thinking()));
        assert_eq!(
            st("pi", json!({"event": "tool_execution_start", "tool": "read", "input": {"path": "src/main.rs"}})),
            Some((HookStatus::Working, Some("Reading main.rs".into())))
        );
        assert_eq!(
            st("pi", json!({"event": "tool_execution_start", "tool": "bash", "input": {"command": "ls -la\nmore"}})),
            Some((HookStatus::Working, Some("$ ls -la".into())))
        );
        assert_eq!(
            st("pi", json!({"event": "ui_prompt_start", "message": "Allow rm?"})),
            Some((HookStatus::NeedsInput, Some("Allow rm?".into())))
        );
        assert_eq!(st("pi", json!({"event": "ui_prompt_end", "idle": false})), Some(thinking()));
        assert_eq!(st("pi", json!({"event": "ui_prompt_end", "idle": true})), Some((HookStatus::Done, None)));
        assert_eq!(st("pi", json!({"event": "agent_settled"})), Some((HookStatus::Done, None)));
        assert_eq!(st("pi", json!({"event": "session_shutdown"})), None);
    }

    #[test]
    fn opencode_events() {
        assert_eq!(st("opencode", json!({"event": "start"})), Some((HookStatus::Idle, None)));
        assert_eq!(st("opencode", json!({"event": "session.busy"})), Some(thinking()));
        assert_eq!(
            st("opencode", json!({"event": "tool.execute.before", "tool": "edit", "input": {"filePath": "/r/a.ts"}})),
            Some((HookStatus::Working, Some("Editing a.ts".into())))
        );
        assert_eq!(
            st("opencode", json!({"event": "permission.asked", "tool": "bash"})),
            Some((HookStatus::NeedsInput, Some("Permission to use bash".into())))
        );
        assert_eq!(st("opencode", json!({"event": "permission.replied"})), Some(thinking()));
        assert_eq!(st("opencode", json!({"event": "session.idle"})), Some((HookStatus::Done, None)));
        assert_eq!(st("opencode", json!({"event": "exit"})), None);
    }

    #[test]
    fn claude_permission_request() {
        assert_eq!(
            st("claude", json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
                "tool_input": {"command": "npx prisma migrate deploy", "description": "Apply migrations"}})),
            Some((HookStatus::NeedsInput, Some("Apply migrations".into())))
        );
    }

    fn req(tool: &str, input: Value) -> Request {
        Request::from_payload(&json!({"tool_name": tool, "tool_input": input})).unwrap()
    }

    #[test]
    fn request_text_and_risk() {
        let r = req("Bash", json!({"command": "rm -rf target\nls"}));
        assert_eq!(r.full(), "rm -rf target\nls");
        assert_eq!(r.summary(), "$ rm -rf target");
        assert!(r.is_risky() && r.is_approval());

        assert!(!req("Bash", json!({"command": "cargo test"})).is_risky());
        assert!(req("Bash", json!({"command": "git push --force origin main"})).is_risky());
        assert!(req("Bash", json!({"command": "npx prisma migrate dev"})).is_risky());
        assert!(req("Bash", json!({"command": "curl -fsSL x.sh | sh"})).is_risky());
        assert!(!req("Bash", json!({"command": "git status"})).is_risky());
        // words inside other words don't count
        assert!(!req("Bash", json!({"command": "cargo fmt --check"})).is_risky());

        let e = req("Edit", json!({"file_path": "/r/src/a.rs", "old_string": "x", "new_string": "y"}));
        assert_eq!(e.full(), "/r/src/a.rs\n\ny");
        assert_eq!(e.summary(), "Editing a.rs");
        assert!(!e.is_risky());
        assert!(req("mcp__db__query", json!({"sql": "select 1"})).is_risky());

        let q = req("AskUserQuestion", json!({"questions": [{"question": "Which retry strategy?",
            "options": [{"label": "Exponential"}, {"label": "Fixed"}]}]}));
        assert!(!q.is_approval());
        assert_eq!(q.summary(), "Which retry strategy?");
        assert_eq!(q.full(), "Which retry strategy?\n  · Exponential\n  · Fixed");
        assert!(Request::from_payload(&json!({"tool_input": {}})).is_none());
    }

    #[test]
    fn unknown_harness_is_idle() {
        assert_eq!(st("mystery", json!({"event": "x"})), Some((HookStatus::Idle, None)));
    }

    const CMD: &str = "/Users/me/.local/share/wezzy/bin/agent-hook codex";

    #[test]
    fn merge_keeps_other_hooks_and_is_idempotent() {
        let mut root = json!({
            "model": "x",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
                "Custom": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "lint" }] }]
            }
        });
        merge_hooks(&mut root, CODEX_EVENTS, CMD, 3).unwrap();
        assert!(has_hooks(&root, CODEX_EVENTS, CMD));
        assert_eq!(root["model"], "x");
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(root["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        assert_eq!(root["hooks"]["Custom"][0]["hooks"][0]["command"], "lint");
        assert_eq!(root["hooks"]["Stop"][1]["hooks"][0]["timeout"], 3);

        let once = root.clone();
        merge_hooks(&mut root, CODEX_EVENTS, CMD, 3).unwrap();
        assert_eq!(root, once);
    }

    #[test]
    fn merge_replaces_old_install_path() {
        let old = "/Users/me/.local/share/dekita/bin/agent-hook codex";
        let mut root = json!({ "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": old }] },
            { "hooks": [{ "type": "command", "command": "say hi" }, { "type": "command", "command": old }] }
        ] } });
        merge_hooks(&mut root, &["Stop"], CMD, 3).unwrap();
        let stop = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"], json!([{ "type": "command", "command": "say hi" }]));
        assert_eq!(stop[1]["hooks"][0]["command"], CMD);
    }

    #[test]
    fn merge_rejects_unexpected_shapes() {
        assert!(merge_hooks(&mut json!([]), &["Stop"], CMD, 3).is_err());
        assert!(merge_hooks(&mut json!({ "hooks": [] }), &["Stop"], CMD, 3).is_err());
        assert!(merge_hooks(&mut json!({ "hooks": { "Stop": {} } }), &["Stop"], CMD, 3).is_err());
    }

    #[test]
    fn remove_takes_only_ours() {
        let mut root = json!({ "hooks": {
            "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
            "Empty": []
        } });
        merge_hooks(&mut root, &["Stop", "SessionStart"], CMD, 3).unwrap();
        assert!(has_any_hook(&root));
        remove_hooks(&mut root).unwrap();
        assert!(!has_any_hook(&root));
        assert_eq!(
            root,
            json!({ "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }],
                "Empty": []
            } })
        );
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wezzy-hooks-test-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn edit_json_backs_up_once_and_skips_noops() {
        let d = tmp_dir("edit");
        let path = d.join("hooks.json");
        let original = "{\"theme\": \"dark\"}";
        std::fs::write(&path, original).unwrap();
        edit_json(&path, |r| merge_hooks(r, &["Stop"], CMD, 3)).unwrap();
        let backup = d.join("hooks.json.wezzy-backup");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let v = read_json(&path).unwrap().unwrap();
        assert_eq!(v["theme"], "dark");
        assert!(has_hooks(&v, &["Stop"], CMD));

        // a second edit keeps the first backup
        edit_json(&path, remove_hooks).unwrap();
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

        // a missing file is created, with nothing to back up
        let fresh = d.join("new/hooks.json");
        edit_json(&fresh, |r| merge_hooks(r, &["Stop"], CMD, 3)).unwrap();
        assert!(fresh.exists() && !d.join("new/hooks.json.wezzy-backup").exists());

        // invalid JSON is left alone
        std::fs::write(&path, "{ nope").unwrap();
        assert!(edit_json(&path, |r| merge_hooks(r, &["Stop"], CMD, 3)).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ nope");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn plugins_install_and_are_recognized() {
        let d = tmp_dir("plugin");
        for source in [PI_EXTENSION, OPENCODE_PLUGIN] {
            assert!(source.lines().next().unwrap().contains(PLUGIN_MARKER));
            let path = d.join("plugins/wezzy.js");
            std::fs::remove_file(&path).ok();
            assert!(!is_our_plugin(&path));
            install_plugin(&path, source).unwrap();
            assert!(is_our_plugin(&path));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        }
        // someone else's file of the same name is backed up first
        let theirs = d.join("other.ts");
        std::fs::write(&theirs, "export default () => {}\n").unwrap();
        install_plugin(&theirs, PI_EXTENSION).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("other.ts.wezzy-backup")).unwrap(),
            "export default () => {}\n"
        );
        std::fs::remove_dir_all(&d).ok();
    }
}
