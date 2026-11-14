//! Persistent sessions: shells and agents live in a background
//! `wezterm-mux-server`; this app is a client that attaches to it.
//! Quit or kill the app and everything keeps running; the next launch
//! reattaches. `WEZZY_LOCAL=1` runs an in-process mux instead (old behavior).
//!
//! Both processes evaluate the same Lua config. Its `unix_domains` entry named
//! "wezzy" tells the server where to listen and the app where to connect.
//!
//! **Hosts.** Each connected host (`hosts.rs`) is another client domain,
//! `host:<name>`, reached with the system `ssh` running `wezzy proxy` on the
//! far side (`wezzy_cli::paths::ssh_proxy_argv`). Its workspaces show up as
//! `<name>/<workspace>` (the vendored client adds and strips the prefix).
//!
//! **Pane ids.** Every server numbers its own panes, and `$WEZTERM_PANE` is
//! that number, so a bare id only means something with its server:
//! [`ServerPane`]. Hook files and notifications only come from this Mac's
//! server, so [`local_pane_id`] / [`remote_pane_id`] are about that one.

use crate::app::{send_event, AppEvent};
use anyhow::Context as _;
use config::UnixDomain;
use mux::domain::{Domain, DomainState};
use mux::Mux;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use wezterm_client::domain::{ClientDomain, ClientDomainConfig};

pub const DOMAIN: &str = "wezzy";

/// Domain names of hosts' servers: `host:devbox`.
const HOST_PREFIX: &str = "host:";

pub fn host_domain_name(host: &str) -> String {
    format!("{HOST_PREFIX}{host}")
}

/// A pane as its session server knows it. `host` is None for this Mac's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ServerPane {
    pub host: Option<String>,
    pub id: mux::pane::PaneId,
}

impl std::fmt::Display for ServerPane {
    /// `3`, or `devbox:3` on a host
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self.host {
            Some(h) => write!(f, "{h}:{}", self.id),
            None => write!(f, "{}", self.id),
        }
    }
}

pub fn persistent() -> bool {
    std::env::var_os("WEZZY_LOCAL").is_none()
}

fn data_dir() -> PathBuf {
    crate::paths::data_dir()
}

/// Add the session-server domain to the mux and make it the default.
pub fn install_domain(config_file: &OsString) -> anyhow::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).ok();
    let socket = dir.join("sock");
    // Lua long strings: paths need no escaping
    let lua_path = |p: PathBuf| format!("[[{}]]", p.display());

    // The server evaluates the user's config too, through an entry file that
    // gives it the wezzy module; everything session-specific is passed here so
    // the config stays free of plumbing. Panes inherit WEZZY=1 from the
    // server's environment (set in main) for the agent hooks.
    let entry = server_config(&dir, config_file)?;
    let unix = UnixDomain {
        name: DOMAIN.into(),
        socket_path: Some(socket.clone()),
        serve_command: Some(vec![
            mux_server_binary()?,
            "--daemonize".into(),
            "--config-file".into(),
            entry.to_string_lossy().to_string(),
            "--config".into(),
            format!(
                "unix_domains={{{{name='{DOMAIN}',socket_path={}}}}}",
                lua_path(socket.clone())
            ),
            "--config".into(),
            format!(
                "daemon_options={{pid_file={},stdout={},stderr={}}}",
                lua_path(dir.join("pid")),
                lua_path(dir.join("server.log")),
                lua_path(dir.join("server.log")),
            ),
        ]),
        ..Default::default()
    };
    log::info!("session server: {:?} via {:?}", unix.socket_path, unix.serve_command);

    let domain: Arc<dyn Domain> = Arc::new(ClientDomain::new(ClientDomainConfig::Unix(unix)));
    let mux = Mux::get();
    mux.add_domain(&domain);
    mux.set_default_domain(&domain);
    Ok(())
}

/// Write the session server's config file: it preloads the `wezzy` module
/// (WezTerm options only, no app settings) and then runs the user's config, so
/// `require("wezzy")` works in both processes. Rewritten only when it changes,
/// since the server reloads its config when the file does.
fn server_config(dir: &std::path::Path, user_config: &OsString) -> anyhow::Result<PathBuf> {
    const FENCE: &str = "]=====]";
    anyhow::ensure!(!crate::commands::LUA_MODULE.contains(FENCE), "wezzy.lua contains {FENCE}");
    let user = std::path::Path::new(user_config).display().to_string();
    anyhow::ensure!(!user.contains("]]"), "config path contains ]]: {user}");
    let text = format!(
        "-- Written by Wezzy: the session server's config. It loads the wezzy module\n\
         -- and then your config ({user}).\n\
         local wezterm = require(\"wezterm\")\n\
         local user = [[{user}]]\n\
         package.preload.wezzy = function()\n\
         \treturn assert(load([=====[{module}]=====], \"=wezzy\"))({{ actions = {actions} }})\n\
         end\n\
         package.preload.dekita = function() return require(\"wezzy\") end\n\
         wezterm.add_to_config_reload_watch_list(user)\n\
         return dofile(user)\n",
        module = crate::commands::LUA_MODULE,
        actions = crate::commands::lua_actions_literal(),
    );
    let path = dir.join("server.lua");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(path)
}

/// The bundled server (Wezzy.app/Contents/MacOS), else $WEZZY_MUX_SERVER,
/// else an installed WezTerm.
fn mux_server_binary() -> anyhow::Result<String> {
    if let Some(p) = std::env::current_exe()
        .ok()
        .map(|e| e.with_file_name("wezterm-mux-server"))
        .filter(|p| p.exists())
    {
        return Ok(p.to_string_lossy().to_string());
    }
    if let Ok(p) = std::env::var("WEZZY_MUX_SERVER") {
        return Ok(p);
    }
    let candidates = [
        PathBuf::from("/Applications/WezTerm.app/Contents/MacOS/wezterm-mux-server"),
        config::HOME_DIR.join("Applications/WezTerm.app/Contents/MacOS/wezterm-mux-server"),
    ];
    if let Some(p) = candidates.iter().find(|p| p.exists()) {
        return Ok(p.to_string_lossy().to_string());
    }
    // fall back to PATH
    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|d| d.join("wezterm-mux-server"))
                .find(|p| p.exists())
        })
        .map(|p| p.to_string_lossy().to_string())
        .context("couldn't find wezterm-mux-server; set WEZZY_MUX_SERVER")
}

/// Attach to the server (starting it if needed), pick the window to show,
/// and spawn a first shell if the server is empty.
pub fn start() {
    promise::spawn::spawn(async {
        if let Err(err) = attach_and_restore().await {
            log::error!("session start failed: {err:#}");
            send_event(AppEvent::Flash(format!("session server: {err:#}")));
        }
        reconnect_hosts();
    })
    .detach();
    promise::spawn::spawn(refresh_ttys_forever()).detach();
}

async fn attach_and_restore() -> anyhow::Result<()> {
    let mux = Mux::get();
    let domain = mux.default_domain();
    let workspace = last_workspace().unwrap_or_else(|| "default".into());
    mux.set_active_workspace(&workspace);

    // Placeholder window: hosts the brief "Connecting…" output and adopts the
    // server's window for this workspace, if there is one.
    let placeholder = *mux.new_empty_window(Some(workspace.clone()), None);
    send_event(AppEvent::WindowReady(placeholder));

    if domain.state() == DomainState::Detached {
        domain.attach(Some(placeholder)).await?;
    }

    let has_tabs = |w| mux.get_window(w).is_some_and(|w| !w.is_empty());
    let server_empty = !mux
        .iter_panes()
        .iter()
        .any(|p| p.domain_id() == domain.domain_id());

    if server_empty {
        let size = config::configuration().initial_size(0, None);
        let cwd = crate::workspaces::saved(&workspace)
            .map(|w| w.dir().to_string_lossy().to_string());
        domain.spawn(size, None, cwd, placeholder).await?;
        send_event(AppEvent::WindowReady(placeholder));
        return Ok(());
    }

    // Reattached. Show the remembered workspace if it still exists, else any.
    let target = mux
        .iter_windows_in_workspace(&workspace)
        .into_iter()
        .find(|w| has_tabs(*w))
        .or_else(|| mux.iter_windows().into_iter().find(|w| has_tabs(*w)));
    if let Some(w) = target {
        if let Some(ws) = mux.get_window(w).map(|w| w.get_workspace().to_string()) {
            mux.set_active_workspace(&ws);
        }
        send_event(AppEvent::WindowReady(w));
    }
    // ttys and the views' user vars now, not at the 2s poll: a reattached
    // view pane is blank until its spec is recovered
    if let Err(err) = refresh_ttys().await {
        log::trace!("tty refresh: {err:#}");
    }
    // Drop the placeholder if it didn't adopt anything (the connecting UI tab
    // closes on its own; give it a moment first).
    smol::Timer::after(Duration::from_millis(1500)).await;
    if !has_tabs(placeholder) {
        mux.kill_window(placeholder);
    }
    send_event(AppEvent::MuxChanged);
    Ok(())
}

/// Keep `procs::PANE_TTYS` current so agent detection and smart-close work
/// for server-owned panes.
async fn refresh_ttys_forever() {
    loop {
        if let Err(err) = refresh_ttys().await {
            log::trace!("tty refresh: {err:#}");
        }
        smol::Timer::after(Duration::from_secs(2)).await;
    }
}

async fn refresh_ttys() -> anyhow::Result<()> {
    let mux = Mux::get();
    let Some(domain) = mux.get_domain_by_name(DOMAIN) else {
        return Ok(());
    };
    let inner = ClientDomain::get_client_inner_for_domain(domain.domain_id())?;
    let panes = inner.client.list_panes().await?;
    let mut map = std::collections::HashMap::new();
    let mut live = std::collections::HashSet::new();
    fn walk(
        node: mux::tab::PaneNode,
        inner: &wezterm_client::domain::ClientInner,
        map: &mut std::collections::HashMap<mux::pane::PaneId, String>,
        live: &mut std::collections::HashSet<mux::pane::PaneId>,
    ) {
        match node {
            mux::tab::PaneNode::Empty => {}
            mux::tab::PaneNode::Split { left, right, .. } => {
                walk(*left, inner, map, live);
                walk(*right, inner, map, live);
            }
            mux::tab::PaneNode::Leaf(entry) => {
                live.insert(entry.pane_id);
                if let (Some(local), Some(tty)) =
                    (inner.remote_to_local_pane_id(entry.pane_id), entry.tty_name)
                {
                    map.insert(local, tty);
                }
            }
        }
    }
    for tab in panes.tabs {
        walk(tab, &inner, &mut map, &mut live);
    }
    let changed = {
        let mut ttys = crate::procs::PANE_TTYS.lock();
        let changed = ttys.as_ref() != Some(&map);
        *ttys = Some(map);
        changed
    };
    crate::agents::names::retain(|id| live.contains(&id));

    // nvim that was already running when we attached never re-announces
    // IS_NVIM; infer it so smart-splits (and our edge nav) see it. Views'
    // placeholders don't re-announce WEZZY_VIEW either. New ttys (attach,
    // new panes) get a fresh process snapshot; otherwise the poller's is
    // enough, unless it's idle and its snapshot stale.
    if changed {
        crate::procs::refresh().await;
    } else if crate::procs::snapshot_age().is_none_or(|age| age > Duration::from_secs(3)) {
        return Ok(());
    }
    for pane in mux.iter_panes() {
        if let Some(cp) = pane.downcast_ref::<wezterm_client::pane::ClientPane>() {
            if !crate::views::is_view(&pane) {
                if let Some(spec) = crate::views::recover_spec(&pane) {
                    cp.seed_user_var(crate::views::USER_VAR, &spec);
                    crate::app::send_event(crate::app::AppEvent::Redraw);
                    continue;
                }
            }
            if !pane.copy_user_vars().contains_key("IS_NVIM") && crate::procs::is_vim(&pane) {
                cp.seed_user_var("IS_NVIM", "true");
            }
        }
    }
    Ok(())
}

// -- remembered workspace ----------------------------------------------------

fn state_file() -> PathBuf {
    data_dir().join("last-workspace")
}

pub fn last_workspace() -> Option<String> {
    let s = std::fs::read_to_string(state_file()).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

pub fn remember_workspace(name: &str) {
    std::fs::create_dir_all(data_dir()).ok();
    std::fs::write(state_file(), name).ok();
}

/// Session-server pane id (what panes see as $WEZTERM_PANE) -> our pane id.
pub fn local_pane_id(remote: mux::pane::PaneId) -> Option<mux::pane::PaneId> {
    if !persistent() {
        return Some(remote);
    }
    let domain = Mux::try_get()?.get_domain_by_name(DOMAIN)?;
    ClientDomain::get_client_inner_for_domain(domain.domain_id())
        .ok()?
        .remote_to_local_pane_id(remote)
}

/// Our pane id -> this Mac's session server's (`$WEZTERM_PANE` in it). None
/// for a pane on a host: its id belongs to another server.
pub fn remote_pane_id(local: mux::pane::PaneId) -> Option<mux::pane::PaneId> {
    server_pane(local).filter(|p| p.host.is_none()).map(|p| p.id)
}

/// Which server a pane lives on, and its id there.
pub fn server_pane(local: mux::pane::PaneId) -> Option<ServerPane> {
    if !persistent() {
        return Some(ServerPane { host: None, id: local });
    }
    let mux = Mux::try_get()?;
    let pane = mux.get_pane(local)?;
    let id = pane.downcast_ref::<wezterm_client::pane::ClientPane>()?.remote_pane_id;
    Some(ServerPane {
        host: pane_host(&mux, &pane),
        id,
    })
}

fn pane_host(mux: &Mux, pane: &Arc<dyn mux::pane::Pane>) -> Option<String> {
    let domain = mux.get_domain(pane.domain_id())?;
    domain.domain_name().strip_prefix(HOST_PREFIX).map(String::from)
}

/// Whether a pane runs on a host. Its cwd is a path there, so anything that
/// reads the local disk at it (git, files, recipes) must leave it alone.
pub fn on_host(pane: &Arc<dyn mux::pane::Pane>) -> bool {
    Mux::try_get().is_some_and(|mux| pane_host(&mux, pane).is_some())
}

// -- hosts -----------------------------------------------------------------------

fn host_log(name: &str) -> PathBuf {
    data_dir().join("hosts").join(format!("{name}.log"))
}

/// `ssh … <host> wezzy proxy`, its stderr kept in hosts/<name>.log so a
/// failed connection can say why.
fn host_proxy_argv(host: &crate::hosts::Host) -> Vec<String> {
    let log = host_log(&host.name);
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let mut ssh = wezzy_cli::paths::ssh_proxy_argv(&host.ssh);
    // fail fast on a host that's down instead of a minute of "Connecting…"
    ssh.splice(1..1, ["-o".to_string(), "ConnectTimeout=10".to_string()]);
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!("exec \"$@\" 2>{}", shell_words::quote(&log.to_string_lossy())),
        "sh".to_string(),
    ];
    argv.extend(ssh);
    argv
}

const NOT_SET_UP: &str = "wezzy isn't set up there";

/// The last thing ssh (or the far side) said on stderr.
fn host_error(name: &str) -> Option<String> {
    let text = std::fs::read_to_string(host_log(name)).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?.trim().to_string();
    if line.contains("not found") || line.contains("No such file") {
        return Some(NOT_SET_UP.into());
    }
    Some(line)
}

/// Whether a host's server is attached.
pub fn host_connected(name: &str) -> bool {
    Mux::try_get()
        .and_then(|m| m.get_domain_by_name(&host_domain_name(name)))
        .is_some_and(|d| d.state() == DomainState::Attached)
}

/// The host whose workspace this is (`devbox/default` → devbox), if attached.
pub fn workspace_host(workspace: &str) -> Option<String> {
    let (host, _) = workspace.split_once('/')?;
    host_connected(host).then(|| host.to_string())
}

/// Whether a workspace is a host's (`devbox/…`, for a host Wezzy knows).
pub fn is_host_workspace(workspace: &str) -> bool {
    workspace.split_once('/').is_some_and(|(host, _)| {
        Mux::try_get().is_some_and(|m| m.get_domain_by_name(&host_domain_name(host)).is_some())
    })
}

/// Connect a host's server and show its workspace. Already connected: just
/// show it.
pub fn connect_host(host: crate::hosts::Host) {
    promise::spawn::spawn(async move {
        match attach_host(&host).await {
            Ok(window) => {
                remember_host(&host.name, true);
                crate::hosts::mark_set_up(&host.name, true);
                send_event(AppEvent::HostConnected { name: host.name, window });
            }
            Err(err) => {
                log::error!("host {}: {err:#}", host.name);
                let why = host_error(&host.name).unwrap_or_else(|| format!("{err:#}"));
                if why == NOT_SET_UP {
                    crate::hosts::mark_set_up(&host.name, false);
                }
                send_event(AppEvent::Flash(format!("{}: {why}", host.name)));
            }
        }
    })
    .detach();
}

/// Attach (creating the domain the first time) and return a window to show.
async fn attach_host(host: &crate::hosts::Host) -> anyhow::Result<Option<mux::window::WindowId>> {
    let mux = Mux::get();
    let name = host_domain_name(&host.name);
    let domain = match mux.get_domain_by_name(&name) {
        Some(d) => d,
        None => {
            let unix = UnixDomain {
                name: name.clone(),
                proxy_command: Some(host_proxy_argv(host)),
                no_serve_automatically: true,
                ..Default::default()
            };
            let client = ClientDomain::new(ClientDomainConfig::Unix(unix))
                .with_workspace_prefix(format!("{}/", host.name));
            let domain: Arc<dyn Domain> = Arc::new(client);
            mux.add_domain(&domain);
            domain
        }
    };
    if domain.state() == DomainState::Detached {
        domain.attach(None).await?;
    }
    let id = domain.domain_id();
    let window = mux
        .iter_panes()
        .into_iter()
        .filter(|p| p.domain_id() == id)
        .find_map(|p| mux.resolve_pane_id(p.pane_id()).map(|(_, window, _)| window));
    Ok(window)
}

/// Close a host's connection. Its shells keep running there.
pub fn disconnect_host(name: &str) {
    remember_host(name, false);
    let mux = Mux::get();
    if let Some(domain) = mux.get_domain_by_name(&host_domain_name(name)) {
        domain.detach().ok();
        send_event(AppEvent::MuxChanged);
    }
}

fn hosts_file() -> PathBuf {
    data_dir().join("connected-hosts")
}

fn connected_hosts() -> Vec<String> {
    std::fs::read_to_string(hosts_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// Hosts connected when the app quit reconnect at launch.
fn remember_host(name: &str, connected: bool) {
    let mut hosts = connected_hosts();
    hosts.retain(|h| h != name);
    if connected {
        hosts.push(name.to_string());
    }
    std::fs::create_dir_all(data_dir()).ok();
    std::fs::write(hosts_file(), hosts.join("\n")).ok();
}

/// Reconnect remembered hosts, in the background after the local server.
fn reconnect_hosts() {
    let all = crate::hosts::all();
    for name in connected_hosts() {
        if let Some(e) = all.iter().find(|e| e.host.name == name) {
            let host = e.host.clone();
            promise::spawn::spawn(async move {
                if let Err(err) = attach_host(&host).await {
                    log::warn!("reconnecting {}: {err:#}", host.name);
                } else {
                    send_event(AppEvent::MuxChanged);
                }
            })
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::server_config;
    use mlua::FromLua;

    /// Evaluate `user` the way the session server does: through its entry file.
    fn eval(name: &str, user: &str) -> anyhow::Result<config::Config> {
        let dir = std::env::temp_dir().join(format!("wezzy-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let user_path = dir.join("wezzy.lua");
        std::fs::write(&user_path, user)?;
        let entry = server_config(&dir, &user_path.clone().into_os_string())?;
        let lua = config::lua::make_lua_context(&entry)?;
        let value: mlua::Value = lua.load(std::fs::read_to_string(&entry)?).eval()?;
        let cfg = config::Config::from_lua(value, &lua)?;
        std::fs::remove_dir_all(&dir).ok();
        Ok(cfg)
    }

    #[test]
    fn declarative_config() {
        let cfg = eval(
            "ok",
            r#"
            local wezzy = require("wezzy")
            local config = wezzy.setup({
              window = { padding = 30, opacity = 0.8 },
              font = { size = 15 },
              leader = "ctrl+b",
              wezterm = { enable_kitty_keyboard = true },
              keys = {
                { "cmd+shift+a", wezzy.action.FindAgent },
                { "cmd++", wezzy.action.IncreaseFontSize },
                { "leader+space", wezzy.action.CommandPalette },
                { "ctrl+shift+f5", wezzy.action.ShowShortcuts },
                { "alt+pageup", wezzy.action.ScrollByPage(-1) },
              },
              workspaces = { name = "w", cwd = "~" },
            })
            wezzy.bind("cmd+d", function(window, pane)
              window:perform_action(wezzy.action.SpawnCommandInNewTab({ args = { "nvim" } }))
            end)
            return config
            "#,
        )
        .unwrap();
        assert_eq!(cfg.window_background_opacity, 0.8);
        assert_eq!(cfg.font_size, 15.0);
        assert!(cfg.enable_kitty_keyboard);
        assert_eq!(cfg.keys.len(), 6);
        assert!(cfg.leader.is_some());
        let tables = cfg.key_bindings();
        assert_eq!(tables.default.len(), 6);
    }

    #[test]
    fn default_config() {
        let cfg = eval("default", include_str!("../config/wezzy.lua")).unwrap();
        assert_eq!(cfg.color_scheme.as_deref(), Some("Tokyo Night"));
        assert!(cfg.enable_kitty_keyboard);
    }

    /// The app's side: repeated setup() calls add up before reaching settings.rs.
    #[test]
    fn setup_merges_app_options() {
        let dir = std::env::temp_dir().join(format!("wezzy-test-{}-merge", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wezzy.lua");
        std::fs::write(&path, "").unwrap();
        let lua = config::lua::make_lua_context(&path).unwrap();
        let got: (i64, String, bool) = lua
            .load(format!(
                r#"
                local seen
                local wezzy = load([=====[{}]=====])({{ actions = {{}}, settings = function(t) seen = t end }})
                wezzy.setup({{ workspaces = {{ name = "a", cwd = "~" }}, pane_cards = false }})
                wezzy.setup({{ workspaces = {{ {{ name = "b", cwd = "~" }} }}, editor = {{ "nvim" }} }})
                return #seen.workspaces, seen.workspaces[2].name, seen.pane_cards
                "#,
                crate::commands::LUA_MODULE
            ))
            .eval()
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(got, (2, "b".into(), false));
    }

    #[test]
    fn rejects_typos() {
        let err = eval("typo", r#"return require("wezzy").setup({ windw = {} })"#).unwrap_err();
        assert!(format!("{err:#}").contains("unknown option setup.windw"), "{err:#}");
        let err = eval("mods", r#"return require("wezzy").setup({ keys = { { "hyper+a", "x" } } })"#)
            .unwrap_err();
        assert!(format!("{err:#}").contains("unknown modifier"), "{err:#}");
    }
}
