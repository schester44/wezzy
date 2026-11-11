//! Persistent sessions: shells and agents live in a background
//! `wezterm-mux-server`; this app is a client that attaches to it.
//! Quit or kill the app and everything keeps running; the next launch
//! reattaches. `WEZZY_LOCAL=1` runs an in-process mux instead (old behavior).
//!
//! Both processes evaluate the same Lua config. Its `unix_domains` entry named
//! "wezzy" tells the server where to listen and the app where to connect.

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

    // The server evaluates the user's config too; everything session-specific is
    // passed here so the config stays free of plumbing. Panes inherit WEZZY=1
    // from the server's environment (set in main) for the agent hooks.
    let unix = UnixDomain {
        name: DOMAIN.into(),
        socket_path: Some(socket.clone()),
        serve_command: Some(vec![
            mux_server_binary()?,
            "--daemonize".into(),
            "--config-file".into(),
            config_file.to_string_lossy().to_string(),
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
    fn walk(
        node: mux::tab::PaneNode,
        inner: &wezterm_client::domain::ClientInner,
        map: &mut std::collections::HashMap<mux::pane::PaneId, String>,
    ) {
        match node {
            mux::tab::PaneNode::Empty => {}
            mux::tab::PaneNode::Split { left, right, .. } => {
                walk(*left, inner, map);
                walk(*right, inner, map);
            }
            mux::tab::PaneNode::Leaf(entry) => {
                if let (Some(local), Some(tty)) =
                    (inner.remote_to_local_pane_id(entry.pane_id), entry.tty_name)
                {
                    map.insert(local, tty);
                }
            }
        }
    }
    for tab in panes.tabs {
        walk(tab, &inner, &mut map);
    }
    *crate::procs::PANE_TTYS.lock() = Some(map);

    // nvim that was already running when we attached never re-announces
    // IS_NVIM; infer it so smart-splits (and our edge nav) see it.
    for pane in mux.iter_panes() {
        if let Some(cp) = pane.downcast_ref::<wezterm_client::pane::ClientPane>() {
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

/// Our pane id -> the session server's (for `wezterm cli --pane-id`).
pub fn remote_pane_id(local: mux::pane::PaneId) -> Option<mux::pane::PaneId> {
    if !persistent() {
        return Some(local);
    }
    let pane = Mux::try_get()?.get_pane(local)?;
    pane.downcast_ref::<wezterm_client::pane::ClientPane>()
        .map(|p| p.remote_pane_id)
}
