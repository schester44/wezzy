//! Where Wezzy keeps things.
//!
//!   ~/.config/wezzy/        wezzy.lua (config), settings.json
//!   ~/.local/share/wezzy/   session socket, pid, server log, hook script
//!   ~/.local/state/wezzy/   agent status records
//!
//! Before the rename these were …/dekita; `migrate_from_dekita` moves them.

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    config::HOME_DIR.join(".config/wezzy")
}

pub fn data_dir() -> PathBuf {
    config::HOME_DIR.join(".local/share/wezzy")
}

pub fn state_dir() -> PathBuf {
    config::HOME_DIR.join(".local/state/wezzy")
}

/// One-time move from the app's old name. Each ~/.…/dekita directory is renamed
/// to wezzy and a symlink is left behind, so everything that still points at
/// the old paths keeps working: the running session server (its socket moves
/// with the directory; its --config-file is the old path), panes it spawned,
/// and agent hooks registered under the old path.
pub fn migrate_from_dekita() {
    let home = &*config::HOME_DIR;
    for (old, new) in [
        (home.join(".config/dekita"), config_dir()),
        (home.join(".local/share/dekita"), data_dir()),
        (home.join(".local/state/dekita"), state_dir()),
    ] {
        let is_real_dir = std::fs::symlink_metadata(&old).is_ok_and(|m| m.is_dir());
        if !is_real_dir || new.exists() {
            continue;
        }
        if let Some(parent) = new.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match std::fs::rename(&old, &new) {
            Ok(()) => {
                std::os::unix::fs::symlink(&new, &old).ok();
            }
            Err(e) => eprintln!("couldn't move {} to {}: {e}", old.display(), new.display()),
        }
    }
    // the config file itself was dekita.lua
    let (old, new) = (config_dir().join("dekita.lua"), config_dir().join("wezzy.lua"));
    if std::fs::symlink_metadata(&old).is_ok_and(|m| m.is_file()) && !new.exists() {
        if std::fs::rename(&old, &new).is_ok() {
            std::os::unix::fs::symlink("wezzy.lua", &old).ok();
        }
    }
}
