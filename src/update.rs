//! Automatic updates for the installed app.
//!
//! A release (scripts/release.sh) uploads `latest.json` next to a notarized
//! `Wezzy-<v>.zip`. Every few hours, and on ⌘K → Check for Updates, we fetch
//! the feed; if it names a newer version we download the zip, check its hash,
//! check it's signed by the same Developer ID team as the running app and
//! accepted by Gatekeeper, then swap it in place of the bundle on disk. The
//! running app keeps going on the old code; the next launch (or ⌘K → Restart)
//! runs the new one. Shells live in the session server, so restarting loses
//! nothing.
//!
//! Except when the release was built against a different WezTerm commit: its
//! mux protocol may not match the session server that's already running, so we
//! don't install it automatically (that needs the server restarted, which
//! ends every shell). We say so once and leave it to the user.
//!
//! Only a signed release bundle updates itself. Dev builds (`target/`), bare
//! binaries and ad-hoc `scripts/bundle.sh` builds never do.

use crate::app::{send_event, AppEvent};
use anyhow::{bail, Context as _};
use parking_lot::Mutex;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FEED: &str = "https://github.com/schester44/wezzy/releases/latest/download/latest.json";
const BUNDLE_ID: &str = "dev.wezzy.app";
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Deserialize)]
struct Feed {
    version: String,
    wezterm: String,
    url: String,
    sha256: String,
}

/// Version installed on disk, waiting for a restart.
static STAGED: Mutex<Option<String>> = Mutex::new(None);
/// Version we already told the user needs a manual install.
static NOTIFIED: Mutex<Option<String>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

pub fn staged() -> Option<String> {
    STAGED.lock().clone()
}

/// Background checks, when this is a signed release bundle and the setting is on.
pub fn start() {
    if bundle().is_none() {
        return;
    }
    std::thread::spawn(|| {
        std::thread::sleep(FIRST_CHECK);
        loop {
            if crate::settings::get().auto_update {
                check(false);
            }
            std::thread::sleep(INTERVAL);
        }
    });
}

/// ⌘K → Check for Updates: works with the setting off, and reports the outcome.
pub fn check_now() {
    std::thread::spawn(|| check(true));
}

fn check(manual: bool) {
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let result = run(manual);
    BUSY.store(false, Ordering::Release);
    let msg = match result {
        Ok(Some(msg)) => msg,
        Ok(None) => return,
        Err(e) => {
            log::warn!("update check failed: {e:#}");
            if !manual {
                return;
            }
            format!("Update failed: {e:#}")
        }
    };
    send_event(AppEvent::Flash(msg));
}

/// What to tell the user, if anything.
fn run(manual: bool) -> anyhow::Result<Option<String>> {
    let Some(app) = bundle() else {
        return Ok(manual.then(|| "This build doesn't update itself".into()));
    };
    let Some(team) = team_id(&app) else {
        return Ok(manual.then(|| "Only signed releases update themselves".into()));
    };

    let text = output(Command::new("curl").args(["-fsSL", "--max-time", "30", FEED]))
        .context("couldn't fetch the update feed")?;
    let feed: Feed = serde_json::from_str(&text).context("bad update feed")?;

    let current = staged().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    if !newer(&feed.version, &current) {
        return Ok(manual.then(|| match staged() {
            Some(v) => format!("Wezzy {v} is installed · restart to use it"),
            None => "Wezzy is up to date".into(),
        }));
    }
    if feed.wezterm != wezterm_rev() {
        let mut told = NOTIFIED.lock();
        if !manual && told.as_deref() == Some(&feed.version) {
            return Ok(None);
        }
        *told = Some(feed.version.clone());
        return Ok(Some(format!(
            "Wezzy {} needs a session server restart · install it by hand",
            feed.version
        )));
    }

    install(&app, &team, &feed)?;
    *STAGED.lock() = Some(feed.version.clone());
    Ok(Some(format!("Wezzy {} installed · restart to use it", feed.version)))
}

/// Download, verify and swap in the new bundle.
fn install(app: &Path, team: &str, feed: &Feed) -> anyhow::Result<()> {
    let parent = app.parent().context("bundle has no parent")?;
    // stage next to the bundle so the final swap is a rename on one volume
    let work = parent.join(".wezzy-update");
    std::fs::remove_dir_all(&work).ok();
    std::fs::create_dir_all(&work)
        .with_context(|| format!("can't write to {}", parent.display()))?;
    let result = (|| {
        let zip = work.join("Wezzy.zip");
        output(
            Command::new("curl")
                .args(["-fsSL", "--max-time", "900", "-o"])
                .arg(&zip)
                .arg(&feed.url),
        )
        .context("download failed")?;

        let sum = output(Command::new("shasum").args(["-a", "256"]).arg(&zip))?;
        if sum.split_whitespace().next() != Some(feed.sha256.as_str()) {
            bail!("download doesn't match its checksum");
        }

        output(Command::new("ditto").args(["-x", "-k"]).arg(&zip).arg(&work))?;
        let new = work.join(app.file_name().context("bundle has no name")?);
        let new = if new.exists() { new } else { work.join("Wezzy.app") };

        let requirement = format!(
            "=anchor apple generic and identifier \"{BUNDLE_ID}\" and certificate leaf[subject.OU] = \"{team}\""
        );
        output(
            Command::new("codesign")
                .args(["--verify", "--deep", "--strict", "-R"])
                .arg(requirement)
                .arg(&new),
        )
        .context("update isn't signed by the same developer")?;
        output(Command::new("spctl").args(["--assess", "--type", "execute"]).arg(&new))
            .context("Gatekeeper rejected the update")?;
        let version = output(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", "Print :CFBundleShortVersionString"])
                .arg(new.join("Contents/Info.plist")),
        )?;
        if version.trim() != feed.version {
            bail!("update is {} but the feed says {}", version.trim(), feed.version);
        }

        // swap: the running process keeps its open files, and the session
        // server keeps running the binary it started from
        let old = work.join("old.app");
        std::fs::rename(app, &old).context("couldn't move the old app aside")?;
        if let Err(e) = std::fs::rename(&new, app) {
            std::fs::rename(&old, app).ok();
            return Err(e).context("couldn't move the new app into place");
        }
        Ok(())
    })();
    std::fs::remove_dir_all(&work).ok();
    result
}

/// The .app we're running from, if it's an installed bundle (not a dev build).
fn bundle() -> Option<PathBuf> {
    if crate::restart::source_dir().is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(PathBuf::from)
}

/// Developer ID team of a signed bundle; None for ad-hoc or unsigned.
fn team_id(app: &Path) -> Option<String> {
    let out = Command::new("codesign").args(["-dv", "--verbose=2"]).arg(app).output().ok()?;
    // codesign -d writes its report to stderr
    let text = String::from_utf8_lossy(&out.stderr);
    text.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "not set")
        .map(str::to_string)
}

/// The WezTerm commit this build is pinned to (Cargo.toml).
fn wezterm_rev() -> &'static str {
    let toml = include_str!("../Cargo.toml");
    toml.split("rev = \"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or("")
}

/// Is `a` a later version than `b`? Dotted numbers; anything else compares as 0.
fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    let n = a.len().max(b.len());
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    (0..n)
        .map(|i| at(&a, i).cmp(&at(&b, i)))
        .find(|o| o.is_ne())
        .is_some_and(|o| o.is_gt())
}

fn output(cmd: &mut Command) -> anyhow::Result<String> {
    let out = cmd.output().with_context(|| format!("couldn't run {:?}", cmd.get_program()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{}", err.lines().last().unwrap_or("failed").trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.2.0", "0.1.9"));
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0", "0.99.99"));
        assert!(newer("v0.1.1", "0.1.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn pinned_rev() {
        assert_eq!(wezterm_rev().len(), 40);
    }
}
