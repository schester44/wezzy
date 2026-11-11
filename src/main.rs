//! wezzy: a gpui shell around WezTerm's engine crates.
//!
//! WezTerm provides: Lua config loading, the mux (windows/tabs/panes/workspaces),
//! PTYs, terminal emulation and key encoding.
//! We provide: rendering, input dispatch, and our own UI (sidebar, overlays).

mod actions;
mod agents;
mod app;
mod clipboard;
mod commands;
mod copymode;
mod diff;
mod feedback;
mod gitpanel;
mod hooks;
mod hosts;
mod keys;
mod launch;
mod luawin;
mod palette;
mod paneselect;
mod paths;
mod quickselect;
mod recipes;
mod regions;
mod restart;
mod procs;
mod render;
mod sendout;
mod sound;
mod servers;
mod session;
mod settings;
mod selection;
mod settings_ui;
mod sidebar;
mod shortcuts;
mod ui;
mod update;
mod views;
mod workspaces;

use anyhow::Context as _;
use mux::domain::{Domain, LocalDomain};
use mux::Mux;
use std::ffi::OsString;
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    paths::migrate_from_dekita();
    import_login_env();
    // Registers WezTerm's full Lua API (wezterm.mux, wezterm.plugin, wezterm.GLOBAL,
    // wezterm.time, wezterm.format, ...) so a real wezterm.lua can be evaluated.
    env_bootstrap::bootstrap();
    commands::register_lua(); // require("wezzy") in the config
    config::designate_this_as_the_main_thread();
    install_schedulers();

    // inherited by the session server and every pane: lets agent hooks know
    // they're running inside Wezzy
    std::env::set_var("WEZZY", "1");
    settings::load();
    let config_file = config_path();
    log::info!("loading config from {}", config_file.to_string_lossy());
    config::common_init(Some(&config_file), &[], false).context("loading config")?;
    let config = config::configuration();
    config.update_ulimit().ok();

    wezterm_blob_leases::register_storage(Arc::new(
        wezterm_blob_leases::simple_tempdir::SimpleTempDir::new_in(&*config::CACHE_DIR)?,
    ))?;

    let domain: Arc<dyn Domain> = Arc::new(LocalDomain::new("local")?);
    let mux = Arc::new(Mux::new(Some(domain)));
    Mux::set_mux(&mux);
    // The mux tracks the active workspace per client; without an identity,
    // set_active_workspace is a silent no-op. Same as wezterm-gui's startup.
    let client_id = Arc::new(mux::client::ClientId::new());
    mux.register_client(client_id.clone());
    mux.replace_identity(Some(client_id));

    if session::persistent() {
        session::install_domain(&config_file)?;
    }

    gpui::Application::new()
        .with_assets(ui::Assets)
        .run(app::open_main_window);
    Ok(())
}

/// Launched from Finder/Dock, the app gets launchd's bare environment. Take
/// PATH and EDITOR from the user's login shell so harnesses, editors and the
/// session server find what a terminal would.
fn import_login_env() {
    if std::env::var_os("TERM").is_some() {
        return; // started from a terminal: already has the real environment
    }
    let shell = unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_shell.is_null() {
            "/bin/zsh".to_string()
        } else {
            std::ffi::CStr::from_ptr((*pw).pw_shell)
                .to_string_lossy()
                .into_owned()
        }
    };
    let marker = "__WEZZY_ENV__";
    let script = format!(r#"printf '{marker}%s\n%s\n' "$PATH" "$EDITOR""#);
    let Ok(out) = std::process::Command::new(&shell)
        .args(["-lic", &script])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(rest) = text.rsplit(marker).next().filter(|_| text.contains(marker)) else {
        return;
    };
    let mut lines = rest.lines();
    if let Some(path) = lines.next().filter(|p| !p.is_empty()) {
        std::env::set_var("PATH", path);
    }
    if let Some(editor) = lines.next().filter(|e| !e.is_empty()) {
        std::env::set_var("EDITOR", editor);
    }
}

const DEFAULT_CONFIG: &str = include_str!("../config/wezzy.lua");

/// `$WEZZY_CONFIG`, else ~/.config/wezzy/wezzy.lua (written from the
/// built-in default on first run).
fn config_path() -> OsString {
    if let Some(p) = std::env::var_os("WEZZY_CONFIG") {
        return p;
    }
    let path = paths::config_dir().join("wezzy.lua");
    if !path.exists() {
        std::fs::create_dir_all(paths::config_dir()).ok();
        if let Err(e) = std::fs::write(&path, DEFAULT_CONFIG) {
            log::error!("couldn't write {}: {e}", path.display());
        }
    }
    path.into_os_string()
}

/// WezTerm's crates schedule their main-thread futures through `promise::spawn`.
/// gpui owns the macOS run loop, which drains the GCD main queue, so route
/// everything there.
fn install_schedulers() {
    fn on_main(runnable: promise::spawn::Runnable) {
        dispatch::Queue::main().exec_async(move || {
            runnable.run();
        });
    }
    promise::spawn::set_schedulers(Box::new(on_main), Box::new(on_main));
}
