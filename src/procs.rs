//! What's running in a pane.
//!
//! Panes owned by the session server (ClientPane) can't say: the mux protocol
//! doesn't carry process info. But the server runs on this machine and
//! `ListPanes` tells us each pane's tty, so we read the foreground process
//! group for that tty from the macOS process table.
//!
//! Reading the table means a `proc_pidinfo` call for every process on the
//! machine, so it's done once per refresh for all panes, on a background
//! thread (`poll`), and published as a `Snapshot`. Everything that runs from
//! render (`foreground`, `is_vim`, `can_close_quietly`, `pane_pids`) only reads
//! that snapshot. The thread idles when nothing has read the snapshot for a
//! while (the window is hidden). `refresh` takes a fresh one right away (after
//! attach, for `views::recover_spec`), and `now` does a synchronous lookup of a
//! few panes for a close confirmation.

use mux::pane::{Pane, PaneId};
use parking_lot::{Condvar, Mutex};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// local pane id -> tty path, for server-owned panes. Filled by session.rs.
pub static PANE_TTYS: Mutex<Option<HashMap<PaneId, String>>> = Mutex::new(None);

/// Ttys of local panes (`WEZZY_LOCAL`), learned when something asks about
/// them, with when they were last asked about.
static LOCAL_TTYS: Mutex<Option<HashMap<PaneId, (String, Instant)>>> = Mutex::new(None);
/// A local pane nobody asked about for this long is dropped from the scan.
const LOCAL_TTL: Duration = Duration::from_secs(10);

/// How often the poller refreshes the snapshot while it's being read.
const POLL: Duration = Duration::from_millis(1000);
/// The poller idles when nothing read the snapshot for this long.
const IDLE: Duration = Duration::from_secs(5);

/// A pane's foreground process: the leader of its tty's foreground group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fg {
    pub pid: u32,
    /// The COMM name (often truncated to 16 bytes).
    pub name: String,
    pub executable: PathBuf,
    pub argv: Vec<String>,
}

impl Fg {
    /// `name`, else the executable's path.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() {
            self.executable.to_string_lossy().to_string()
        } else {
            self.name.clone()
        }
    }
}

/// One pass over the process table, shaped per pane.
#[derive(Default)]
struct Snapshot {
    /// When the scan started; a slower, older scan never replaces a newer one.
    at: Option<Instant>,
    foreground: HashMap<PaneId, Fg>,
    /// Every process attached to the pane's terminal.
    members: HashMap<PaneId, Vec<u32>>,
}

struct State {
    snap: Snapshot,
    last_read: Instant,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// Wakes the poller when a read finds it idle.
static WAKE: Condvar = Condvar::new();
static WOKEN: Mutex<bool> = Mutex::new(false);
static START: std::sync::Once = std::sync::Once::new();

fn state(guard: &mut Option<State>) -> &mut State {
    guard.get_or_insert_with(|| State {
        snap: Snapshot::default(),
        last_read: Instant::now(),
    })
}

/// Read the snapshot, marking it wanted (and starting / waking the poller).
fn read<R>(f: impl FnOnce(&Snapshot) -> R) -> R {
    START.call_once(|| {
        if let Err(err) = std::thread::Builder::new().name("procs".into()).spawn(poll) {
            log::error!("procs poller: {err:#}");
        }
    });
    let mut guard = STATE.lock();
    let state = state(&mut guard);
    let was_idle = state.last_read.elapsed() >= IDLE;
    state.last_read = Instant::now();
    let r = f(&state.snap);
    drop(guard);
    if was_idle {
        *WOKEN.lock() = true;
        WAKE.notify_one();
    }
    r
}

fn poll() {
    loop {
        {
            let mut woken = WOKEN.lock();
            if !*woken {
                WAKE.wait_for(&mut woken, POLL);
            }
            *woken = false;
        }
        let wanted = STATE
            .lock()
            .as_ref()
            .is_some_and(|s| s.last_read.elapsed() < IDLE);
        if wanted {
            refresh_blocking();
        }
    }
}

/// The ttys to scan: the server's panes plus local panes asked about lately.
fn ttys() -> HashMap<PaneId, String> {
    let mut out = PANE_TTYS.lock().clone().unwrap_or_default();
    if let Some(local) = LOCAL_TTYS.lock().as_mut() {
        local.retain(|_, (_, seen)| seen.elapsed() < LOCAL_TTL);
        for (id, (tty, _)) in local.iter() {
            out.entry(*id).or_insert_with(|| tty.clone());
        }
    }
    out
}

/// Scan for `ttys()` and publish the result.
fn refresh_blocking() {
    let at = Instant::now();
    let ttys = ttys();
    let (foreground, members) = if ttys.is_empty() {
        Default::default()
    } else {
        scan(&ttys)
    };
    let mut guard = STATE.lock();
    let state = state(&mut guard);
    if state.snap.at.is_some_and(|prev| prev > at) {
        return;
    }
    state.snap = Snapshot {
        at: Some(at),
        foreground,
        members,
    };
}

/// Take a fresh snapshot now (on a blocking thread) and wait for it.
pub async fn refresh() {
    smol::unblock(refresh_blocking).await;
}

/// How old the snapshot is; `None` before the first one.
pub fn snapshot_age() -> Option<Duration> {
    STATE.lock().as_ref()?.snap.at.map(|at| at.elapsed())
}

/// Foreground processes of `panes`, looked up now rather than read from the
/// snapshot: one scan, on the calling thread. For a user action (closing).
pub fn now(panes: &[Arc<dyn Pane>]) -> Now {
    let ttys: HashMap<PaneId, String> = panes
        .iter()
        .filter_map(|p| Some((p.pane_id(), pane_tty(p)?)))
        .collect();
    Now(if ttys.is_empty() {
        HashMap::new()
    } else {
        scan(&ttys).0
    })
}

/// A synchronous lookup from `now`.
pub struct Now(HashMap<PaneId, Fg>);

impl Now {
    pub fn foreground_name(&self, pane: &Arc<dyn Pane>) -> Option<String> {
        self.0.get(&pane.pane_id()).map(Fg::display_name)
    }

    /// `can_close_quietly`, with this lookup.
    pub fn can_close_quietly(&self, pane: &Arc<dyn Pane>) -> bool {
        quiet(pane, self.foreground_name(pane))
    }
}

/// Foreground process of the pane, from the snapshot.
pub fn foreground(pane: &Arc<dyn Pane>) -> Option<Fg> {
    note_local(pane);
    let id = pane.pane_id();
    read(|s| s.foreground.get(&id).cloned())
}

pub fn foreground_name(pane: &Arc<dyn Pane>) -> Option<String> {
    foreground(pane).map(|f| f.display_name())
}

fn basename(name: String) -> String {
    Path::new(&name)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or(name)
}

/// nvim/vim in the foreground (smart-splits forwards Ctrl+hjkl to it).
pub fn is_vim(pane: &Arc<dyn Pane>) -> bool {
    if pane.copy_user_vars().get("IS_NVIM").map(String::as_str) == Some("true") {
        return true;
    }
    foreground_name(pane).is_some_and(|n| matches!(basename(n).as_str(), "nvim" | "vim" | "vi"))
}

/// WezTerm's rule: closing is quiet when the foreground process is a shell
/// (`skip_close_confirmation_for_processes_named`). From the snapshot; closing
/// a pane asks `now` instead.
pub fn can_close_quietly(pane: &Arc<dyn Pane>) -> bool {
    quiet(pane, foreground_name(pane))
}

fn quiet(pane: &Arc<dyn Pane>, name: Option<String>) -> bool {
    if crate::views::is_view(pane) {
        return true;
    }
    let Some(name) = name else {
        // Unknown. Local panes know better than we do; remote ones: be careful.
        return pane.can_close_without_prompting(mux::pane::CloseReason::Pane);
    };
    let base = basename(name);
    config::configuration()
        .skip_close_confirmation_for_processes_named
        .iter()
        .any(|s| *s == base)
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

/// A local pane's tty goes into the next scans.
fn note_local(pane: &Arc<dyn Pane>) {
    if let Some(tty) = pane.tty_name() {
        LOCAL_TTYS
            .lock()
            .get_or_insert_with(HashMap::new)
            .insert(pane.pane_id(), (tty, Instant::now()));
    }
}

/// Every process attached to the pane's terminal (shell, jobs, servers,
/// agents...), from the snapshot.
pub fn pane_pids(pane: &Arc<dyn Pane>) -> Vec<u32> {
    note_local(pane);
    let id = pane.pane_id();
    read(|s| s.members.get(&id).cloned().unwrap_or_default())
}

// -- the scan ------------------------------------------------------------------

/// One process-table entry: what the shaping needs.
#[derive(Clone, Debug)]
struct Row {
    pid: u32,
    /// controlling tty device
    tdev: u32,
    /// that tty's foreground process group
    tpgid: u32,
    comm: String,
}

type Shaped = (HashMap<PaneId, Row>, HashMap<PaneId, Vec<u32>>);

/// Per pane: its tty's foreground group leader (if it's still alive) and every
/// pid on that tty. `ttys` is pane -> tty device.
fn shape(ttys: &HashMap<PaneId, u32>, rows: &[Row]) -> Shaped {
    let mut fg_group: HashMap<u32, u32> = HashMap::new();
    let mut on_tty: HashMap<u32, Vec<u32>> = HashMap::new();
    for r in rows {
        fg_group.entry(r.tdev).or_insert(r.tpgid);
        on_tty.entry(r.tdev).or_default().push(r.pid);
    }
    let by_pid: HashMap<u32, &Row> = rows.iter().map(|r| (r.pid, r)).collect();
    let mut leaders = HashMap::new();
    let mut members = HashMap::new();
    for (&pane, dev) in ttys {
        if let Some(leader) = fg_group.get(dev).and_then(|g| by_pid.get(g)) {
            leaders.insert(pane, (*leader).clone());
        }
        if let Some(pids) = on_tty.get(dev) {
            members.insert(pane, pids.clone());
        }
    }
    (leaders, members)
}

/// tty path -> device number.
fn rdev(tty: &str) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::metadata(tty).ok()?.rdev() as u32)
}

/// One pass over the process table for `ttys` (pane -> tty path).
fn scan(ttys: &HashMap<PaneId, String>) -> (HashMap<PaneId, Fg>, HashMap<PaneId, Vec<u32>>) {
    let devs: HashMap<PaneId, u32> = ttys
        .iter()
        .filter_map(|(id, t)| Some((*id, rdev(t)?)))
        .collect();
    let (leaders, members) = shape(&devs, &table());
    // panes sharing a tty share a leader; read its argv once
    let mut by_pid: HashMap<u32, Fg> = HashMap::new();
    let foreground = leaders
        .into_iter()
        .map(|(pane, row)| {
            let fg = by_pid
                .entry(row.pid)
                .or_insert_with(|| {
                    let (executable, argv) = exe_and_argv(row.pid).unwrap_or_default();
                    Fg {
                        pid: row.pid,
                        name: row.comm,
                        executable,
                        argv,
                    }
                })
                .clone();
            (pane, fg)
        })
        .collect();
    (foreground, members)
}

/// Every process with a controlling tty.
#[cfg(target_os = "macos")]
fn table() -> Vec<Row> {
    use std::mem::{size_of, zeroed};
    let mut out = vec![];
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
            let comm = unsafe { std::ffi::CStr::from_ptr(info.pbi_comm.as_ptr()) };
            out.push(Row {
                pid: pid as u32,
                tdev: info.e_tdev,
                tpgid: info.e_tpgid,
                comm: comm.to_string_lossy().into_owned(),
            });
        }
    }
    out
}

#[cfg(not(target_os = "macos"))]
fn table() -> Vec<Row> {
    vec![]
}

/// A process's executable and argv (`KERN_PROCARGS2`).
#[cfg(target_os = "macos")]
fn exe_and_argv(pid: u32) -> Option<(PathBuf, Vec<String>)> {
    let mut size: libc::size_t = 64 * 1024;
    let mut buf: Vec<u8> = vec![0; size];
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let r = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as _,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r == -1 {
        return None;
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

#[cfg(not(target_os = "macos"))]
fn exe_and_argv(_pid: u32) -> Option<(PathBuf, Vec<String>)> {
    None
}

/// `KERN_PROCARGS2` is argc (a C int), the executable path, NUL padding, then
/// argc NUL-terminated args (then the environment).
fn parse_procargs(buf: &[u8]) -> Option<(PathBuf, Vec<String>)> {
    const INT: usize = std::mem::size_of::<libc::c_int>();
    let argc = libc::c_int::from_ne_bytes(buf.get(..INT)?.try_into().ok()?);
    let mut rest = &buf[INT..];
    let nul = rest.iter().position(|&c| c == 0)?;
    let exe = PathBuf::from(String::from_utf8_lossy(&rest[..nul]).into_owned());
    rest = &rest[nul..];
    let start = rest.iter().position(|&c| c != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    let mut argv = vec![];
    for _ in 0..argc.max(0) {
        let nul = rest.iter().position(|&c| c == 0)?;
        argv.push(String::from_utf8_lossy(&rest[..nul]).into_owned());
        rest = &rest[nul + 1..];
    }
    Some((exe, argv))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, tdev: u32, tpgid: u32, comm: &str) -> Row {
        Row {
            pid,
            tdev,
            tpgid,
            comm: comm.into(),
        }
    }

    #[test]
    fn shape_finds_each_panes_foreground_leader_and_members() {
        let rows = vec![
            row(10, 1, 12, "zsh"),
            row(12, 1, 12, "nvim"),
            row(13, 1, 12, "node"),
            row(20, 2, 20, "zsh"),
            row(30, 3, 30, "other"),
        ];
        let ttys = HashMap::from([(100, 1), (200, 2), (300, 9)]);
        let (fg, members) = shape(&ttys, &rows);
        assert_eq!(fg.len(), 2, "only panes whose tty has processes");
        assert_eq!(fg[&100].pid, 12);
        assert_eq!(fg[&100].comm, "nvim");
        assert_eq!(fg[&200].comm, "zsh");
        assert_eq!(members[&100], vec![10, 12, 13]);
        assert_eq!(members[&200], vec![20]);
        assert!(!members.contains_key(&300));
    }

    #[test]
    fn shape_skips_a_foreground_group_whose_leader_exited() {
        let rows = vec![row(10, 1, 15, "zsh"), row(16, 1, 15, "make")];
        let (fg, members) = shape(&HashMap::from([(1, 1)]), &rows);
        assert!(fg.is_empty());
        assert_eq!(members[&1], vec![10, 16]);
    }

    fn procargs(argc: i32, exe: &str, pad: usize, args: &[&str]) -> Vec<u8> {
        let mut b = argc.to_ne_bytes().to_vec();
        b.extend(exe.as_bytes());
        b.extend(std::iter::repeat_n(0, 1 + pad));
        for a in args {
            b.extend(a.as_bytes());
            b.push(0);
        }
        b.extend(b"HOME=/x\0");
        b
    }

    #[test]
    fn parse_procargs_reads_exe_and_argv() {
        let b = procargs(3, "/bin/sh", 5, &["/bin/sh", "-c", "cat"]);
        let (exe, argv) = parse_procargs(&b).unwrap();
        assert_eq!(exe, PathBuf::from("/bin/sh"));
        assert_eq!(argv, ["/bin/sh", "-c", "cat"]);
    }

    #[test]
    fn parse_procargs_keeps_empty_args_in_place() {
        // a view placeholder with an empty title: argv[4] is "", argv[5] the spec
        let b = procargs(6, "/bin/sh", 0, &["/bin/sh", "-c", "s", "wezzy-view", "", "c3Fs"]);
        let (_, argv) = parse_procargs(&b).unwrap();
        assert_eq!(argv[4], "");
        assert_eq!(argv[5], "c3Fs");
    }

    #[test]
    fn parse_procargs_rejects_truncated_input() {
        assert!(parse_procargs(&[1, 0]).is_none());
        let mut b = procargs(2, "/bin/x", 0, &["x"]);
        b.truncate(b.len() - b"HOME=/x\0".len());
        assert!(parse_procargs(&b).is_none(), "argc says 2, one arg present");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn exe_and_argv_reads_this_process() {
        let (exe, argv) = exe_and_argv(std::process::id()).unwrap();
        assert!(!exe.as_os_str().is_empty());
        assert_eq!(argv.first().map(String::as_str), std::env::args().next().as_deref());
    }
}
