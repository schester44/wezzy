//! wezzy: a gpui shell around WezTerm's engine crates.
//!
//! WezTerm provides: Lua config loading, the mux (windows/tabs/panes/workspaces),
//! PTYs, terminal emulation and key encoding.
//! We provide: rendering, input dispatch, and our own UI (sidebar, overlays).

mod actions;
mod activity;
mod agents;
mod app;
mod board;
mod clipboard;
mod commands;
mod copymode;
mod diff;
mod feedback;
mod gitlive;
mod gitpanel;
mod hooks;
mod hosts;
mod keys;
mod launch;
mod luawin;
mod notify;
mod palette;
mod paneselect;
mod paths;
mod pr;
mod quickselect;
mod recipes;
mod regions;
mod restart;
mod runs;
mod seam;
mod procs;
mod queue;
mod render;
mod sendout;
mod sound;
mod textfield;
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
    let args: Vec<OsString> = std::env::args_os().collect();
    // started through the `handoff` link (board.rs): the board's own command
    if args.first().is_some_and(|a| board::is_handoff(std::path::Path::new(a))) {
        std::process::exit(handoff::cli::run(args));
    }
    if is_cli(&args) {
        std::process::exit(wezzy_cli::main(args));
    }
    paths::migrate_from_dekita();
    import_login_env();
    install_cli();
    // Registers WezTerm's full Lua API (wezterm.mux, wezterm.plugin, wezterm.GLOBAL,
    // wezterm.time, wezterm.format, ...) so a real wezterm.lua can be evaluated.
    env_bootstrap::bootstrap();
    commands::register_lua(); // require("wezzy") in the config
    commands::write_lua_types(); // completion for it in an editor
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

/// `wezzy <command>`, or `wezzy` found on PATH (from a shell, with no
/// subcommand: show the CLI's help rather than open a second app).
fn is_cli(args: &[OsString]) -> bool {
    if wezzy_cli::wants_cli(args) {
        return true;
    }
    let argv0 = args.first().map(|a| std::path::Path::new(a)).unwrap_or(std::path::Path::new(""));
    argv0.components().count() == 1 || argv0.starts_with(paths::data_dir().join("bin"))
}

/// Put the `wezzy` command on PATH: a link to this binary in
/// ~/.local/share/wezzy/bin (beside the agent hook), first on PATH for the
/// session server and everything it starts. A session server outlives app
/// updates, so its panes keep the PATH it started with; for those (and other
/// terminals) it's also linked into ~/.local/bin when that's on your PATH.
fn install_cli() {
    let bin = paths::data_dir().join("bin");
    let link = bin.join("wezzy");
    if let Ok(exe) = std::env::current_exe() {
        relink(&link, &exe);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    // the login shell's PATH (import_login_env), before ours goes in front
    let user_bin = config::HOME_DIR.join(".local/bin");
    if std::env::split_paths(&path).any(|p| p == user_bin) {
        let user_link = user_bin.join("wezzy");
        // only ours: never replace someone else's `wezzy`
        let ours = match std::fs::symlink_metadata(&user_link) {
            Err(_) => true,
            Ok(m) => m.file_type().is_symlink() && std::fs::read_link(&user_link).is_ok_and(|t| t == link),
        };
        if ours {
            relink(&user_link, &link);
        }
    }
    if !std::env::split_paths(&path).any(|p| p == bin) {
        let joined = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&path)));
        if let Ok(joined) = joined {
            std::env::set_var("PATH", joined);
        }
    }
}

/// Point the symlink `link` at `target`, unless it already does.
fn relink(link: &std::path::Path, target: &std::path::Path) {
    if std::fs::read_link(link).ok().as_deref() == Some(target) {
        return;
    }
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::remove_file(link).ok();
    if let Err(err) = std::os::unix::fs::symlink(target, link) {
        log::warn!("linking {}: {err}", link.display());
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
