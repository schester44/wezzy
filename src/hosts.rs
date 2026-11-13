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
//! These are plain `ssh` sessions in a local pane. The same `name`/`ssh` list is
//! what docs/roadmap/remote-compute.md (phase 1) turns into remote mux domains.

use crate::app::TermApp;
use crate::launch::Placement;
use crate::settings::{self, Harness};
use gpui::{Context, Window};
use std::path::{Path, PathBuf};

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

impl TermApp {
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
