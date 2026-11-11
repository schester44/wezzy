//! Where Dekita keeps things.
//!
//!   ~/.config/dekita/        dekita.lua (config), settings.json
//!   ~/.local/share/dekita/   session socket, pid, server log, hook script
//!   ~/.local/state/dekita/   agent status records

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    config::HOME_DIR.join(".config/dekita")
}

pub fn data_dir() -> PathBuf {
    config::HOME_DIR.join(".local/share/dekita")
}
