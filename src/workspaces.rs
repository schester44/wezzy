//! Saved workspaces: named workspaces you can pick (sidebar menu, ⌘K, ⇧⌘K)
//! before they exist. Opening one that isn't running starts it with a shell in
//! its directory; opening one that is just switches to it.
//!
//! ```lua
//! require("wezzy").setup({
//!   workspaces = {
//!     { name = "neverwait", cwd = "~/work/neverwait" },
//!   },
//! })
//! ```
//!
//! or in settings.json: `"workspaces": [{ "name": "neverwait", "cwd": "~/work/neverwait" }]`.

use config::keyassignment::SpawnCommand;
use mux::Mux;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub name: String,
    /// as written; `~` is expanded when spawning
    pub cwd: String,
}

impl Saved {
    pub fn dir(&self) -> PathBuf {
        expand_tilde(&self.cwd)
    }

    /// What the first pane of a fresh workspace runs.
    pub fn spawn(&self) -> SpawnCommand {
        SpawnCommand {
            cwd: Some(self.dir()),
            ..Default::default()
        }
    }
}

/// A row in a workspace list.
pub struct Entry {
    pub name: String,
    /// tab count if running
    pub tabs: Option<usize>,
    /// the saved directory, `~`-abbreviated
    pub cwd: Option<String>,
}

impl Entry {
    /// Right-hand detail: tab count when running, else where it'll open.
    pub fn detail(&self) -> String {
        match (self.tabs, &self.cwd) {
            (Some(n), _) => format!("{n} {}", if n == 1 { "tab" } else { "tabs" }),
            (None, Some(cwd)) => cwd.clone(),
            (None, None) => String::new(),
        }
    }
}

pub fn saved(name: &str) -> Option<Saved> {
    crate::settings::get().workspaces.into_iter().find(|w| w.name == name)
}

/// Running workspaces (mux order), then saved ones that aren't (config order).
pub fn list() -> Vec<Entry> {
    let mux = Mux::get();
    let saved = crate::settings::get().workspaces;
    let live = mux.iter_workspaces();
    let mut out: Vec<Entry> = live
        .iter()
        .map(|ws| Entry {
            name: ws.clone(),
            tabs: Some(
                mux.iter_windows_in_workspace(ws)
                    .iter()
                    .filter_map(|w| mux.get_window(*w).map(|w| w.count_tabs()))
                    .sum(),
            ),
            cwd: saved.iter().find(|s| s.name == *ws).map(|s| abbreviate(&s.cwd)),
        })
        .collect();
    out.extend(saved.iter().filter(|s| !live.contains(&s.name)).map(|s| Entry {
        name: s.name.clone(),
        tabs: None,
        cwd: Some(abbreviate(&s.cwd)),
    }));
    out
}

fn expand_tilde(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            config::HOME_DIR.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(p),
    }
}

fn abbreviate(p: &str) -> String {
    let home = config::HOME_DIR.to_string_lossy().to_string();
    match p.strip_prefix(&home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => p.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde() {
        let home = config::HOME_DIR.clone();
        assert_eq!(expand_tilde("~/work/x"), home.join("work/x"));
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("/tmp"), PathBuf::from("/tmp"));
        assert_eq!(expand_tilde("~bob/x"), PathBuf::from("~bob/x"));
        let abs = home.join("work/x").to_string_lossy().to_string();
        assert_eq!(abbreviate(&abs), "~/work/x");
        assert_eq!(abbreviate("/tmp"), "/tmp");
    }
}
