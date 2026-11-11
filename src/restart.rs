//! Restarting Dekita from inside Dekita. Sessions live in the session server,
//! so panes and agents survive; only the window process is replaced.

use crate::app::{send_event, AppEvent, TermApp};
use gpui::{Context, Window};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The source tree this binary was built from, when running a dev build out
/// of its own `target/` (so "Rebuild & Restart" makes sense).
pub fn source_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe = std::env::current_exe().ok()?;
    (dir.join("Cargo.toml").exists() && exe.starts_with(dir.join("target"))).then_some(dir)
}

/// Start a fresh instance once this one has exited.
fn relaunch() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    // inside Dekita.app: relaunch the bundle so it's a proper app instance
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(PathBuf::from);
    let (script, target) = match bundle {
        Some(app) => (r#"sleep 0.4; open -n "$0""#, app),
        None => (r#"sleep 0.4; exec "$0""#, exe),
    };
    Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0) // outlive us
        .spawn()?;
    Ok(())
}

impl TermApp {
    pub(crate) fn restart(&mut self, cx: &mut Context<Self>) {
        match relaunch() {
            Ok(()) => cx.quit(),
            Err(e) => {
                self.flash(format!("Restart failed: {e}"));
                cx.notify();
            }
        }
    }

    /// `cargo build`, then restart on success; on failure show the errors in a split.
    pub(crate) fn rebuild_and_restart(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = source_dir() else {
            self.restart(cx);
            return;
        };
        self.flash("Building…");
        cx.notify();
        std::thread::spawn(move || {
            let ok = Command::new("cargo")
                .arg("build")
                .current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            send_event(AppEvent::RebuildFinished { ok, dir });
        });
    }

    pub(crate) fn rebuild_finished(
        &mut self,
        ok: bool,
        dir: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ok {
            self.restart(cx);
            return;
        }
        self.flash("Build failed");
        // re-run it where you can read the errors
        let cmd = format!("cd {} && cargo build", shell_words::quote(&dir.to_string_lossy()));
        let harness = crate::settings::Harness {
            name: "cargo".into(),
            argv: vec!["sh".into(), "-c".into(), cmd],
        };
        self.run_command_in_split(&harness, window, cx);
    }
}
