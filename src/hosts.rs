//! SSH hosts: servers you can open a shell on from the palette (⌘⇧S, or
//! "ssh <name>" in ⌘K). ↵ opens it in a split, ⌘↵ in a new tab.
//!
//! Hosts come from the config and from `~/.ssh/config` (every `Host` without
//! wildcards, so aliases you already have just show up):
//!
//! ```lua
//! require("wezzy").setup({
//!   hosts = {
//!     { name = "devbox", ssh = "dev@10.0.0.5" },
//!     { name = "nas", ssh = "-p 2222 admin@nas.local" },  -- anything ssh takes
//!   },
//! })
//! ```
//!
//! or in settings.json: `"hosts": [{ "name": "devbox", "ssh": "dev@10.0.0.5" }]`.
//!
//! The palette matches on the target too, so "ssh 0.5" finds devbox. Typing a
//! target nobody configured ("ssh pi@10.0.0.9") still connects.
//!
//! These are plain `ssh` sessions in a local pane. The same list is what
//! "Connect <host>" attaches as a session domain (`session::connect_host`),
//! and choosing a host that hasn't got it asks to install `wezzy` there
//! (`pick_host`, `set_up_host`).

use crate::app::TermApp;
use crate::launch::Placement;
use crate::settings::{self, Harness};
use gpui::{Context, Window};
use config::keyassignment::KeyAssignment;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub struct Host {
    pub name: String,
    /// ssh arguments, as written: `user@host`, `-p 2222 user@host`, or an ssh_config alias
    pub ssh: String,
}

impl Host {
    /// `ssh …` argv.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = vec!["ssh".to_string()];
        argv.extend(shell_words::split(&self.ssh).unwrap_or_else(|_| vec![self.ssh.clone()]));
        argv
    }
}

// -- asking to set up ----------------------------------------------------------

const SET_UP: &str = "wezzy-host.set-up";
const JUST_SSH: &str = "wezzy-host.ssh";

/// The host `pick_host` is asking about, and where its shell would open.
static ASKING: Mutex<Option<(Host, Placement)>> = Mutex::new(None);
/// Hosts you chose a plain shell for: not asked again this run.
static DECLINED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Whether an `EmitEvent` is the answer to `pick_host`'s question.
pub fn is_prompt_answer(event: &str) -> bool {
    event == SET_UP || event == JUST_SSH
}

// -- set-up hosts ---------------------------------------------------------------

/// Hosts with `wezzy` installed, one name per line: the ones "Connect" is
/// offered for. Added when Set up finishes or a connection works, dropped when
/// a connection finds no `wezzy` there (so Set up comes back).
fn set_up_file() -> PathBuf {
    crate::paths::data_dir().join("set-up-hosts")
}

fn set_up_hosts() -> Vec<String> {
    std::fs::read_to_string(set_up_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub fn is_set_up(name: &str) -> bool {
    set_up_hosts().iter().any(|h| h == name)
}

pub fn mark_set_up(name: &str, set_up: bool) {
    let mut hosts = set_up_hosts();
    let had = hosts.iter().any(|h| h == name);
    if had == set_up {
        return;
    }
    hosts.retain(|h| h != name);
    if set_up {
        hosts.push(name.to_string());
    }
    std::fs::create_dir_all(crate::paths::data_dir()).ok();
    std::fs::write(set_up_file(), hosts.join("\n") + "\n").ok();
}

/// A palette entry: what to show and what to run.
pub struct Entry {
    pub host: Host,
    /// where it goes, e.g. `dev@10.0.0.5`
    pub target: String,
}

/// Configured hosts first (in config order), then ~/.ssh/config aliases they
/// don't shadow.
pub fn all() -> Vec<Entry> {
    let mut out: Vec<Entry> = settings::get()
        .hosts
        .into_iter()
        .map(|h| Entry {
            target: h.ssh.clone(),
            host: h,
        })
        .collect();
    let home = config::HOME_DIR.clone();
    for (alias, target) in ssh_config_hosts(&home.join(".ssh/config"), &home) {
        if !out.iter().any(|e| e.host.name == alias) {
            out.push(Entry {
                host: Host {
                    name: alias.clone(),
                    ssh: alias,
                },
                target,
            });
        }
    }
    out
}

/// `(alias, user@hostname[:port])` for each non-wildcard `Host` in an
/// ssh_config, following `Include`.
fn ssh_config_hosts(path: &Path, home: &Path) -> Vec<(String, String)> {
    let mut out = vec![];
    parse_into(path, home, 0, &mut out);
    out
}

fn parse_into(path: &Path, home: &Path, depth: u8, out: &mut Vec<(String, String)>) {
    if depth > 4 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    #[derive(Default)]
    struct Block {
        aliases: Vec<String>,
        hostname: Option<String>,
        user: Option<String>,
        port: Option<String>,
    }
    let flush = |b: &mut Block, out: &mut Vec<(String, String)>| {
        for alias in b.aliases.drain(..) {
            if out.iter().any(|(a, _)| *a == alias) {
                continue;
            }
            let host = b.hostname.clone().unwrap_or_else(|| alias.clone());
            let mut target = match &b.user {
                Some(u) => format!("{u}@{host}"),
                None => host,
            };
            if let Some(p) = b.port.as_ref().filter(|p| *p != "22") {
                target.push(':');
                target.push_str(p);
            }
            out.push((alias, target));
        }
        *b = Block::default();
    };
    let mut block = Block::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c == '=' || c.is_whitespace()) {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                flush(&mut block, out);
                block.aliases = value
                    .split_whitespace()
                    .filter(|p| !p.contains(['*', '?', '!']))
                    .map(str::to_string)
                    .collect();
            }
            // conditional blocks: not something you'd pick by name
            "match" => flush(&mut block, out),
            "hostname" => block.hostname = Some(value.to_string()),
            "user" => block.user = Some(value.to_string()),
            "port" => block.port = Some(value.to_string()),
            "include" => {
                for pattern in value.split_whitespace() {
                    for inc in expand_include(pattern, home) {
                        parse_into(&inc, home, depth + 1, out);
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut block, out);
}

/// `~/x`, relative-to-`~/.ssh`, and a `*` in the file name.
fn expand_include(pattern: &str, home: &Path) -> Vec<PathBuf> {
    let p = match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if pattern.starts_with('/') => PathBuf::from(pattern),
        None => home.join(".ssh").join(pattern),
    };
    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let Some((pre, post)) = name.split_once('*') else {
        return vec![p];
    };
    let Some(dir) = p.parent() else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            f.file_name()
                .map(|n| n.to_string_lossy())
                .is_some_and(|n| n.len() >= pre.len() + post.len() && n.starts_with(pre) && n.ends_with(post))
        })
        .collect();
    files.sort();
    files
}

/// Installs `wezzy` on a host (`scripts/install-server.sh`), piped over ssh.
const INSTALL_SCRIPT: &str = include_str!("../scripts/install-server.sh");
/// Sends the host our Linux build first, then runs the installer there.
const SET_UP_SCRIPT: &str = include_str!("../scripts/set-up-host.sh");

/// The Linux builds of `wezzy` this app carries (`wezzy-linux-<arch>.tar.gz`):
/// Wezzy.app/Contents/Resources/server, or target/server in a dev build
/// (`scripts/build-server.sh`).
fn server_builds() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundled = exe.parent()?.parent()?.join("Resources/server");
    if bundled.is_dir() {
        return Some(bundled);
    }
    crate::restart::source_dir()
        .map(|d| d.join("target/server"))
        .filter(|d| d.is_dir())
}

impl TermApp {
    /// Set the host up in the background (it's on this Mac whatever pane
    /// you're in): send it our build of `wezzy` (no internet needed there),
    /// install the server's service, remember it's set up. A toast says how it
    /// went; the whole run is in hosts/<name>-setup.log.
    /// `connect`: attach it once it's set up.
    pub(crate) fn set_up_host(&mut self, host: &Host, connect: bool, _window: &mut Window, cx: &mut Context<Self>) {
        let bin = crate::paths::data_dir().join("bin");
        let installer = bin.join("install-server.sh");
        let set_up = bin.join("set-up-host.sh");
        std::fs::create_dir_all(&bin).ok();
        let written = std::fs::write(&installer, INSTALL_SCRIPT).and_then(|_| std::fs::write(&set_up, SET_UP_SCRIPT));
        if let Err(err) = written {
            self.flash(format!("writing the installer: {err}"));
            return;
        }
        let log = crate::paths::data_dir().join("hosts").join(format!("{}-setup.log", host.name));
        let path = |p: &Path| p.to_string_lossy().to_string();
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg(&set_up)
            .arg(server_builds().as_deref().map(path).unwrap_or_default())
            .arg(&installer)
            .arg(set_up_file())
            .arg(&host.name)
            .arg(wezzy_cli::paths::WEZTERM_REV)
            // nobody to answer a prompt
            .args(["-o", "BatchMode=yes"])
            .args(host.argv().into_iter().skip(1));
        let name = host.name.clone();
        let host = host.clone();
        self.flash(format!("Setting up {name}"));
        cx.notify();
        std::thread::spawn(move || {
            let out = log
                .parent()
                .map(|d| std::fs::create_dir_all(d))
                .and_then(|_| std::fs::File::create(&log).ok());
            let Some(out) = out else {
                return;
            };
            let status = cmd
                .stdin(std::process::Stdio::null())
                .stdout(out.try_clone().map(Into::into).unwrap_or(std::process::Stdio::null()))
                .stderr(out)
                .status();
            let msg = match status {
                Ok(s) if s.success() && connect => {
                    promise::spawn::spawn_into_main_thread(async move { crate::session::connect_host(host) }).detach();
                    format!("{name} is set up, connecting")
                }
                Ok(s) if s.success() => format!("{name} is set up"),
                _ => {
                    let text = std::fs::read_to_string(&log).unwrap_or_default();
                    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("set up failed");
                    format!("{name}: {}", last.trim().trim_start_matches("error: "))
                }
            };
            crate::app::send_event(crate::app::AppEvent::Flash(msg));
        });
    }

    /// ↵ on a host in the palette. A configured host without Wezzy's server
    /// asks first: Set up installs it and connects; SSH opens a plain shell
    /// and stops asking about that host until the app restarts.
    pub(crate) fn pick_host(
        &mut self,
        host: &Host,
        placement: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = all().into_iter().find(|e| e.host == *host);
        let pane = self.active_pane();
        let ask = !is_set_up(&host.name) && !DECLINED.lock().unwrap().contains(&host.name);
        let (Some(entry), Some(pane), true) = (entry, pane, ask) else {
            return self.open_ssh(host, placement, window, cx);
        };
        *ASKING.lock().unwrap() = Some((host.clone(), placement));
        self.overlay = Some(crate::app::Overlay::Confirm {
            message: format!("Set up {}?", host.name),
            detail: entry.target,
            verb: "Set up",
            cancel_verb: Some("SSH"),
            action: KeyAssignment::EmitEvent(SET_UP.into()),
            cancel: Some(KeyAssignment::EmitEvent(JUST_SSH.into())),
            pane_id: pane.pane_id(),
        });
        cx.notify();
    }

    /// The answer to `pick_host`'s question (from `perform`).
    pub(crate) fn answer_host_prompt(&mut self, event: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((host, placement)) = ASKING.lock().unwrap().take() else {
            return;
        };
        if event == SET_UP {
            self.set_up_host(&host, true, window, cx);
        } else {
            DECLINED.lock().unwrap().push(host.name.clone());
            self.open_ssh(&host, placement, window, cx);
        }
    }

    pub(crate) fn open_ssh(
        &mut self,
        host: &Host,
        placement: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = Harness {
            name: "ssh".into(),
            argv: host.argv(),
        };
        self.region = crate::regions::Region::Terminal;
        self.spawn_harness(&session, placement, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ssh_config() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-ssh");
        std::fs::create_dir_all(dir.join(".ssh/conf.d")).unwrap();
        std::fs::write(
            dir.join(".ssh/config"),
            "Include conf.d/*.conf\n\
             Host *\n  IdentityAgent x\n\
             Host backup vb\n  HostName 10.0.0.5\n  User dev\n\
             Host box\n  Port=2222\n\
             Match host foo\n  User nope\n",
        )
        .unwrap();
        std::fs::write(dir.join(".ssh/conf.d/a.conf"), "Host orb\n  Hostname 127.0.0.1\n  Port 32222\n").unwrap();
        let hosts = ssh_config_hosts(&dir.join(".ssh/config"), &dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            hosts,
            vec![
                ("orb".into(), "127.0.0.1:32222".into()),
                ("backup".into(), "dev@10.0.0.5".into()),
                ("vb".into(), "dev@10.0.0.5".into()),
                ("box".into(), "box:2222".into()),
            ]
        );
    }

    #[test]
    fn argv_splits_options() {
        let h = Host {
            name: "nas".into(),
            ssh: "-p 2222 admin@nas.local".into(),
        };
        assert_eq!(h.argv(), ["ssh", "-p", "2222", "admin@nas.local"]);
    }
}
