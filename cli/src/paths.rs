//! Where a host's server lives. Everything is under one directory per WezTerm
//! pin, so a server built from another pin (another mux protocol) never
//! shares a socket with this one; the app reaches it by the same fixed path.

use std::path::PathBuf;

/// The WezTerm commit the mux protocol comes from (`Cargo.toml`'s `rev`).
pub const WEZTERM_REV: &str = "e68a8ec";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/.local/share/wezzy/server/<pin>`; XDG_DATA_HOME is ignored on purpose:
/// the app names this path over ssh without knowing the remote environment.
pub fn server_dir() -> PathBuf {
    home().join(".local/share/wezzy/server").join(WEZTERM_REV)
}

pub fn socket() -> PathBuf {
    server_dir().join("sock")
}

pub fn pid_file() -> PathBuf {
    server_dir().join("pid")
}

pub fn log_file() -> PathBuf {
    server_dir().join("server.log")
}

/// Where the installer puts this binary on a host, as the remote shell
/// spells it (`$HOME` expands there, not here).
pub fn remote_binary() -> String {
    format!("\"$HOME/.local/share/wezzy/server/{WEZTERM_REV}/bin/wezzy\"")
}

/// `ssh … <host> wezzy proxy`: how a client reaches a host's server. `target`
/// is what you'd type after `ssh` (an alias, `user@host`, `-p 2222 host`, or
/// `ssh://user@host:port`).
pub fn ssh_proxy_argv(target: &str) -> Vec<String> {
    let mut argv: Vec<String> = ["ssh", "-T", "-o", "BatchMode=yes", "-o", "ServerAliveInterval=15"]
        .into_iter()
        .map(String::from)
        .collect();
    argv.extend(ssh_target_args(target));
    argv.push(format!("{} proxy", remote_binary()));
    argv
}

/// `ssh://user@host:port` → `-p port user@host`; anything else is split like a
/// shell would.
pub fn ssh_target_args(target: &str) -> Vec<String> {
    if let Some(rest) = target.strip_prefix("ssh://") {
        let rest = rest.trim_end_matches('/');
        // a port follows the last ':' after the host (no IPv6 brackets support needed yet)
        if let Some((host, port)) = rest.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())) {
            return vec!["-p".into(), port.into(), host.into()];
        }
        return vec![rest.into()];
    }
    split_words(target)
}

/// Whitespace split that honours single and double quotes.
fn split_words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_urls() {
        assert_eq!(ssh_target_args("ssh://dev@box:2222"), ["-p", "2222", "dev@box"]);
        assert_eq!(ssh_target_args("ssh://dev@box"), ["dev@box"]);
        assert_eq!(ssh_target_args("-p 2222 'admin@nas'"), ["-p", "2222", "admin@nas"]);
    }

    #[test]
    fn pin_matches_the_app() {
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains(&format!("rev = \"{WEZTERM_REV}")));
        let app = include_str!("../../Cargo.toml");
        assert!(app.contains(&format!("rev = \"{WEZTERM_REV}")));
    }
}
