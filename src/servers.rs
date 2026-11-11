//! Dev servers: TCP listeners started from a pane in the current workspace.
//!
//! A background thread runs `lsof` every few seconds for all listening
//! sockets; we keep the ones whose process is attached to a pane's tty.

use mux::pane::{Pane, PaneId};
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Listener {
    pub pid: u32,
    pub port: u16,
    pub command: String,
}

#[derive(Clone, Debug)]
pub struct Server {
    pub port: u16,
    pub command: String,
    /// Folder of the pane it was started from, e.g. "neverwait".
    pub folder: String,
    pub pane_id: PaneId,
}

static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());

pub fn start() {
    std::thread::spawn(|| loop {
        if let Some(found) = scan() {
            *LISTENERS.lock() = found;
        }
        std::thread::sleep(Duration::from_secs(3));
    });
}

fn scan() -> Option<Vec<Listener>> {
    let out = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = vec![];
    let (mut pid, mut cmd) = (0u32, String::new());
    for line in text.lines() {
        let (tag, rest) = line.split_at(line.len().min(1));
        match tag {
            "p" => pid = rest.parse().unwrap_or(0),
            "c" => cmd = rest.to_string(),
            "n" => {
                // "*:3000", "127.0.0.1:5173", "[::1]:8080"
                if let Some(port) = rest.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                    if !found
                        .iter()
                        .any(|l: &Listener| l.pid == pid && l.port == port)
                    {
                        found.push(Listener {
                            pid,
                            port,
                            command: cmd.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    Some(found)
}

/// Servers started from any of `panes`, one entry per port.
pub fn for_panes(panes: &[Arc<dyn Pane>]) -> Vec<Server> {
    let listeners = LISTENERS.lock().clone();
    if listeners.is_empty() {
        return vec![];
    }
    let mut by_port: BTreeMap<u16, Server> = BTreeMap::new();
    for pane in panes {
        let pids = crate::procs::pane_pids(pane);
        let folder = pane
            .get_current_working_dir(mux::pane::CachePolicy::AllowStale)
            .and_then(|u| {
                std::path::Path::new(u.path().trim_end_matches('/'))
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
            })
            .unwrap_or_default();
        for l in listeners.iter().filter(|l| pids.contains(&l.pid)) {
            // skip agents' own sockets (MCP/IDE bridges) and privileged ports
            if l.port < 1024 || crate::agents::is_agent_process(&l.command) {
                continue;
            }
            by_port.entry(l.port).or_insert(Server {
                port: l.port,
                command: l.command.clone(),
                folder: folder.clone(),
                pane_id: pane.pane_id(),
            });
        }
    }
    by_port.into_values().collect()
}
