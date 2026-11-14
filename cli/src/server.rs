//! `wezzy server`: a host's session server. The mux half is WezTerm's own
//! (`wezterm-mux-server-impl`, as in `wezterm-mux-server`); this file is
//! the process around it: one instance per pin, a pid lock, a fixed socket, and
//! a systemd user service so it outlives logout and starts at boot.

use crate::paths;
use anyhow::{bail, Context as _};
use clap::{Parser, Subcommand};
use mux::domain::{Domain, LocalDomain};
use mux::Mux;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Subcommand)]
pub enum ServerCmd {
    /// Run the server in the foreground
    Run(RunOpts),
    /// Start the server in the background if it isn't running
    Start(RunOpts),
    /// Whether the server is running
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Stop the server. Every shell and agent on it ends
    Stop,
    /// Start the server at boot and keep it running after you log out (systemd)
    Install,
    /// Undo `install`. A running server keeps running
    Uninstall,
    /// Show the server's log
    Logs {
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: usize,
    },
}

#[derive(Debug, Parser, Clone, Default)]
pub struct RunOpts {
    /// Don't load wezterm.lua
    #[arg(long, short = 'n')]
    skip_config: bool,
}

pub fn run(cmd: ServerCmd) -> anyhow::Result<()> {
    match cmd {
        ServerCmd::Run(o) => serve(o),
        ServerCmd::Start(o) => start(o),
        ServerCmd::Status { json } => status(json),
        ServerCmd::Stop => stop(),
        ServerCmd::Install => install(),
        ServerCmd::Uninstall => uninstall(),
        ServerCmd::Logs { follow, lines } => logs(follow, lines),
    }
}

fn running() -> bool {
    UnixStream::connect(paths::socket()).is_ok()
}

fn pid() -> Option<i32> {
    std::fs::read_to_string(paths::pid_file()).ok()?.trim().parse().ok()
}

// -- run ---------------------------------------------------------------------

fn serve(opts: RunOpts) -> anyhow::Result<()> {
    private_server_dir()?;
    let _lock = lock_pid_file()?;

    config::common_init(None, &[], opts.skip_config)?;
    let config = config::configuration();
    config.update_ulimit()?;
    if let Some(value) = &config.default_ssh_auth_sock {
        std::env::set_var("SSH_AUTH_SOCK", value);
    }

    for name in ["OLDPWD", "PWD", "SHLVL", "WEZTERM_PANE", "WEZTERM_UNIX_SOCKET", "_"] {
        std::env::remove_var(name);
    }
    for name in &config.mux_env_remove {
        std::env::remove_var(name);
    }
    // panes know they're in Wezzy (agent hooks check it)
    std::env::set_var("WEZZY", "1");

    wezterm_blob_leases::register_storage(Arc::new(
        wezterm_blob_leases::simple_tempdir::SimpleTempDir::new_in(&*config::CACHE_DIR)?,
    ))?;

    let domain: Arc<dyn Domain> = Arc::new(LocalDomain::new("local")?);
    let mux = Arc::new(Mux::new(Some(domain)));
    Mux::set_mux(&mux);

    let executor = promise::spawn::SimpleExecutor::new();
    // Our socket, whatever wezterm.lua's unix_domains say: the app and `-S`
    // find the server by this path. (A `--config unix_domains=…` override
    // doesn't take when there's no wezterm.lua, so it's set here instead.)
    let unix_dom = config::UnixDomain {
        name: "wezzy".into(),
        socket_path: Some(paths::socket()),
        ..config.unix_domains.first().cloned().unwrap_or_default()
    };
    std::env::set_var("WEZTERM_UNIX_SOCKET", unix_dom.socket_path());
    let mut listener = wezterm_mux_server_impl::local::LocalListener::with_domain(&unix_dom)?;
    std::thread::spawn(move || listener.run());
    log::info!("wezzy server {} listening on {}", paths::WEZTERM_REV, unix_dom.socket_path().display());

    let activity = mux::activity::Activity::new();
    promise::spawn::spawn(async move {
        if let Err(err) = startup().await {
            log::error!("{err:#}; terminating");
            std::process::exit(1);
        }
        drop(activity);
    })
    .detach();

    loop {
        executor.tick()?;
    }
}

async fn startup() -> anyhow::Result<()> {
    let mux = Mux::get();
    let config = config::configuration();
    wezterm_mux_server_impl::update_mux_domains_for_server(&config)?;
    let _sub = config::subscribe_to_config_reload(move || {
        promise::spawn::spawn_into_main_thread(async move {
            if let Err(err) = wezterm_mux_server_impl::update_mux_domains_for_server(&config::configuration()) {
                log::error!("updating mux domains: {err:#}");
            }
        })
        .detach();
        true
    });
    if let Err(err) = config::with_lua_config_on_main_thread(mux_startup).await {
        log::error!("mux-startup: {err:#}");
    }
    // a first shell, so a fresh server has something to attach to
    let domain = mux.default_domain();
    if !mux.iter_panes().iter().any(|p| p.domain_id() == domain.domain_id()) {
        let window = mux.new_empty_window(None, None);
        domain.attach(Some(*window)).await?;
        domain.spawn(config.initial_size(0, None), None, None, *window).await?;
    }
    Ok(())
}

async fn mux_startup(lua: Option<Rc<mlua::Lua>>) -> anyhow::Result<()> {
    if let Some(lua) = lua {
        let args = lua.pack_multi(())?;
        config::lua::emit_event(&lua, ("mux-startup".to_string(), args)).await?;
    }
    Ok(())
}

/// The server's folder, only ours: WezTerm won't put its socket in a folder
/// others can write to, and with a umask of 002 (Ubuntu's) a new folder is
/// group-writable.
fn private_server_dir() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = paths::server_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("making {} private", dir.display()))
}

/// One server per pin: hold an exclusive lock on the pid file for our lifetime.
fn lock_pid_file() -> anyhow::Result<std::fs::File> {
    use std::io::Write;
    let path = paths::pid_file();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("already running (pid {})", pid().map(|p| p.to_string()).unwrap_or_else(|| "?".into()));
    }
    file.set_len(0)?;
    writeln!(file, "{}", std::process::id())?;
    Ok(file)
}

// -- start / status / stop -----------------------------------------------------

fn start(opts: RunOpts) -> anyhow::Result<()> {
    if running() {
        return Ok(());
    }
    private_server_dir()?;
    // under systemd, let it start the unit so the server lives in the service
    if unit_installed() && systemctl(&["start", UNIT]).is_ok() && wait_for_socket() {
        return Ok(());
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).open(paths::log_file())?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["server", "run"]);
    if opts.skip_config {
        cmd.arg("-n");
    }
    cmd.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    unsafe {
        use std::os::unix::process::CommandExt;
        // its own session: it outlives the ssh connection that started it
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().context("starting wezzy server run")?;
    for _ in 0..100 {
        if running() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            bail!("the server exited ({status}); see {}", paths::log_file().display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("the server didn't come up; see {}", paths::log_file().display())
}

fn wait_for_socket() -> bool {
    (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        running()
    })
}

fn status(json: bool) -> anyhow::Result<()> {
    let up = running();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "running": up,
                "pid": if up { pid() } else { None },
                "socket": paths::socket(),
                "wezterm": paths::WEZTERM_REV,
                "service": unit_installed(),
            })
        );
    } else if up {
        println!("running, pid {}", pid().map(|p| p.to_string()).unwrap_or_else(|| "?".into()));
    } else {
        println!("not running");
    }
    if !up {
        std::process::exit(1);
    }
    Ok(())
}

fn stop() -> anyhow::Result<()> {
    if !running() {
        return Ok(());
    }
    if unit_installed() && systemctl(&["stop", UNIT]).is_ok() {
        return Ok(());
    }
    let pid = pid().context("no pid file")?;
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        bail!("stopping pid {pid}: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

// -- systemd ---------------------------------------------------------------------

const UNIT: &str = "wezzy.service";

fn unit_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("systemd/user").join(UNIT)
}

fn unit_installed() -> bool {
    cfg!(target_os = "linux") && unit_path().exists()
}

fn systemctl(args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("systemctl").arg("--user").args(args).status()?;
    anyhow::ensure!(status.success(), "systemctl --user {} failed", args.join(" "));
    Ok(())
}

fn install() -> anyhow::Result<()> {
    if !cfg!(target_os = "linux") || Command::new("systemctl").arg("--version").output().is_err() {
        bail!("`server install` needs systemd; without it the server starts on first connect and runs until reboot");
    }
    let exe = std::env::current_exe()?;
    let unit = format!(
        "[Unit]\n\
         Description=Wezzy session server\n\
         \n\
         [Service]\n\
         ExecStart={} server run\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe.display()
    );
    let path = unit_path();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, unit)?;
    systemctl(&["daemon-reload"])?;
    if running() {
        // a server started by a connection keeps its sessions; the unit takes
        // over the next time it starts
        systemctl(&["enable", UNIT])?;
        println!("installed; the running server (pid {}) moves under systemd when it next starts",
            pid().map(|p| p.to_string()).unwrap_or_else(|| "?".into()));
    } else {
        systemctl(&["enable", "--now", UNIT])?;
        println!("installed and started");
    }
    // keep the user manager (and the server) running while logged out
    let user = std::env::var("USER").unwrap_or_default();
    let linger = Command::new("loginctl").args(["enable-linger", &user]).status();
    if !linger.is_ok_and(|s| s.success()) {
        eprintln!("couldn't enable lingering; run `sudo loginctl enable-linger {user}` so the server survives logout");
    }
    Ok(())
}

fn uninstall() -> anyhow::Result<()> {
    if !unit_installed() {
        return Ok(());
    }
    systemctl(&["disable", UNIT]).ok();
    std::fs::remove_file(unit_path())?;
    systemctl(&["daemon-reload"]).ok();
    Ok(())
}

fn logs(follow: bool, lines: usize) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let n = lines.to_string();
    let mut cmd = if unit_installed() {
        let mut c = Command::new("journalctl");
        c.args(["--user", "-u", UNIT, "-n", &n]);
        c
    } else {
        let mut c = Command::new("tail");
        c.args(["-n", &n]).arg(paths::log_file());
        c
    };
    if follow {
        cmd.arg("-f");
    }
    Err(cmd.exec().into())
}
