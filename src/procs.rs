//! What's running in a pane.
//!
//! Local panes answer directly. Panes owned by the session server (ClientPane)
//! can't: the mux protocol doesn't carry process info. But the server runs on
//! this machine and `ListPanes` tells us each pane's tty, so we read the
//! foreground process group for that tty from the macOS process table.

use mux::pane::{CachePolicy, Pane, PaneId};
use parking_lot::Mutex;
use procinfo::LocalProcessInfo;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// local pane id -> tty path, for server-owned panes. Filled by session.rs.
pub static PANE_TTYS: Mutex<Option<HashMap<PaneId, String>>> = Mutex::new(None);

struct Cached {
    at: Instant,
    info: Option<LocalProcessInfo>,
}

static CACHE: Mutex<Option<HashMap<PaneId, Cached>>> = Mutex::new(None);
/// One scan of the process table: tty device -> (foreground pgid, all pids on it).
struct TtyScan {
    foreground: HashMap<u32, u32>,
    members: HashMap<u32, Vec<u32>>,
}
static FOREGROUND: Mutex<Option<(Instant, TtyScan)>> = Mutex::new(None);
const TTL: Duration = Duration::from_millis(1000);

/// Foreground process of the pane, from whichever source works.
pub fn foreground(pane: &Arc<dyn Pane>) -> Option<LocalProcessInfo> {
    if let Some(info) = pane.get_foreground_process_info(CachePolicy::AllowStale) {
        return Some(info);
    }
    let id = pane.pane_id();
    if let Some(c) = CACHE.lock().as_ref().and_then(|m| m.get(&id)) {
        if c.at.elapsed() < TTL {
            return c.info.clone();
        }
    }
    let tty = pane_tty(pane);
    let info = tty
        .and_then(|t| foreground_pgid(&t))
        .and_then(LocalProcessInfo::with_root_pid);
    CACHE.lock().get_or_insert_with(HashMap::new).insert(
        id,
        Cached {
            at: Instant::now(),
            info: info.clone(),
        },
    );
    info
}

pub fn foreground_name(pane: &Arc<dyn Pane>) -> Option<String> {
    foreground(pane).map(|i| {
        if i.name.is_empty() {
            i.executable.to_string_lossy().to_string()
        } else {
            i.name
        }
    })
}

/// nvim/vim in the foreground (smart-splits forwards Ctrl+hjkl to it).
pub fn is_vim(pane: &Arc<dyn Pane>) -> bool {
    if pane.copy_user_vars().get("IS_NVIM").map(String::as_str) == Some("true") {
        return true;
    }
    foreground_name(pane).is_some_and(|n| {
        let base = std::path::Path::new(&n)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or(n);
        matches!(base.as_str(), "nvim" | "vim" | "vi")
    })
}

/// WezTerm's rule: closing is quiet when the foreground process is a shell
/// (`skip_close_confirmation_for_processes_named`).
pub fn can_close_quietly(pane: &Arc<dyn Pane>) -> bool {
    if crate::views::is_view(pane) {
        return true;
    }
    let Some(name) = foreground_name(pane) else {
        // Unknown. Local panes know better than we do; remote ones: be careful.
        return pane.can_close_without_prompting(mux::pane::CloseReason::Pane);
    };
    let base = std::path::Path::new(&name)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or(name);
    config::configuration()
        .skip_close_confirmation_for_processes_named
        .iter()
        .any(|s| *s == base)
}

fn with_scan<R>(tty: &str, f: impl FnOnce(u32, &TtyScan) -> R) -> Option<R> {
    use std::os::unix::fs::MetadataExt;
    let rdev = std::fs::metadata(tty).ok()?.rdev() as u32;
    let mut guard = FOREGROUND.lock();
    let fresh = guard.as_ref().is_some_and(|(at, _)| at.elapsed() < TTL);
    if !fresh {
        *guard = Some((Instant::now(), scan_ttys()));
    }
    Some(f(rdev, &guard.as_ref()?.1))
}

/// tty path -> foreground process group id (tpgid).
fn foreground_pgid(tty: &str) -> Option<u32> {
    with_scan(tty, |rdev, scan| scan.foreground.get(&rdev).copied()).flatten()
}

/// The pane's tty, whether it's local or owned by the session server.
pub fn pane_tty(pane: &Arc<dyn Pane>) -> Option<String> {
    pane.tty_name().or_else(|| {
        PANE_TTYS
            .lock()
            .as_ref()
            .and_then(|m| m.get(&pane.pane_id()).cloned())
    })
}

/// Every process attached to the pane's terminal (shell, jobs, servers, agents...).
pub fn pane_pids(pane: &Arc<dyn Pane>) -> Vec<u32> {
    pane_tty(pane)
        .and_then(|t| with_scan(&t, |rdev, scan| scan.members.get(&rdev).cloned()))
        .flatten()
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn scan_ttys() -> TtyScan {
    use std::mem::{size_of, zeroed};
    let mut out = TtyScan {
        foreground: HashMap::new(),
        members: HashMap::new(),
    };
    let mut pids = vec![0i32; 8192];
    let n = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr() as *mut libc::c_void,
            (pids.len() * size_of::<i32>()) as i32,
        )
    };
    if n <= 0 {
        return out;
    }
    for &pid in &pids[..(n as usize).min(pids.len())] {
        if pid <= 0 {
            continue;
        }
        let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as i32;
        let r = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        // e_tdev: controlling tty; e_tpgid: that tty's foreground process group
        if r == size && info.e_tpgid != 0 && info.e_tdev != u32::MAX && info.e_tdev != 0 {
            out.foreground.entry(info.e_tdev).or_insert(info.e_tpgid);
            out.members.entry(info.e_tdev).or_default().push(pid as u32);
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn scan_ttys() -> TtyScan {
    TtyScan {
        foreground: HashMap::new(),
        members: HashMap::new(),
    }
}
