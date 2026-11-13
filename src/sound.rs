//! A subtle chime when an agent needs you or finishes (like Pi's pi-chime).

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// Sounds offered in Settings (macOS system sounds), plus "off".
pub const CHOICES: &[&str] = &[
    "Funk", "Glass", "Ping", "Pop", "Tink", "Purr", "Bottle", "Hero", "Submarine", "Blow", "off",
];

fn path(name: &str) -> String {
    format!("/System/Library/Sounds/{name}.aiff")
}

/// Play `name` now (Settings preview).
pub fn play(name: &str) {
    if name == "off" {
        return;
    }
    let p = path(name);
    if std::path::Path::new(&p).exists() {
        let child = std::process::Command::new("/usr/bin/afplay")
            .arg(p)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        // reap it, or every chime leaves a zombie behind
        if let Ok(mut child) = child {
            std::thread::spawn(move || child.wait().ok());
        }
    }
}

/// Play the configured chime, at most once every couple of seconds.
pub fn chime() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let Some(name) = crate::settings::get().sound() else {
        return;
    };
    let mut last = LAST.lock();
    if last.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
        return;
    }
    *last = Some(Instant::now());
    play(&name);
}
