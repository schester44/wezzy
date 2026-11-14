//! The `wezzy` command line.
//!
//! - **Pane commands** (`ls`, `split`, `new`, `send`, `capture`, …) drive the
//!   session server you're in, or a host's with `-S <host>`.
//! - **`server`** runs a host's session server (Linux; the Mac app's is the
//!   bundled `wezterm-mux-server`). Built with the `server` feature.
//! - **`proxy`** (hidden) is what the app and `-S` run over ssh.
//!
//! The Mac app links this crate and calls [`main`] when its first argument is
//! one of [`COMMANDS`], so `Wezzy.app/Contents/MacOS/wezzy split` works.

pub mod paths;
mod panes;
#[cfg(feature = "server")]
mod proxy;
#[cfg(feature = "server")]
mod server;

use clap::{Parser, Subcommand};
use std::ffi::OsString;

#[derive(Debug, Parser)]
#[command(name = "wezzy", about = "Wezzy's command line", disable_version_flag = true)]
struct Opt {
    /// Act on a host's server: an ssh_config alias, user@host or ssh://user@host:port
    #[arg(short = 'S', long, global = true, env = "WEZZY_SERVER", allow_hyphen_values = true)]
    server: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// List panes
    Ls(panes::Ls),
    /// Split a pane; prints the new pane's id
    Split(panes::Split),
    /// Open a tab (or --window); prints the new pane's id
    New(panes::New),
    /// Type text into a pane
    Send(panes::Send),
    /// Print a pane's screen, and scrollback with -n
    Capture(panes::Capture),
    /// Focus a pane
    Focus(panes::Focus),
    /// Zoom or unzoom a pane
    Zoom(panes::Zoom),
    /// Set the tab's title
    Title(panes::Title),
    /// Close panes
    Kill(panes::Kill),
    /// This host's session server
    #[cfg(feature = "server")]
    #[command(subcommand)]
    Server(server::ServerCmd),
    #[cfg(feature = "server")]
    #[command(hide = true)]
    Proxy,
    /// Version and WezTerm pin
    Version {
        #[arg(long)]
        json: bool,
    },
}

/// Subcommands, for the app to tell a CLI call from a launch.
pub const COMMANDS: &[&str] = &[
    "ls", "split", "new", "send", "capture", "focus", "zoom", "title", "kill", "server", "proxy", "version",
    "help", "-S", "--server", "-h", "--help",
];

/// Whether `args` (with argv[0]) asks for the CLI rather than the app.
pub fn wants_cli(args: &[OsString]) -> bool {
    args.get(1).and_then(|a| a.to_str()).is_some_and(|a| COMMANDS.contains(&a))
}

/// Run the CLI; returns the exit code.
pub fn main(args: Vec<OsString>) -> i32 {
    let opt = match Opt::try_parse_from(args) {
        Ok(o) => o,
        Err(e) => {
            e.print().ok();
            return e.exit_code();
        }
    };
    match run(opt) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("wezzy: {e:#}");
            1
        }
    }
}

fn run(opt: Opt) -> anyhow::Result<()> {
    match opt.cmd {
        Cmd::Version { json } => {
            let version = env!("CARGO_PKG_VERSION");
            if json {
                println!(
                    "{}",
                    serde_json::json!({ "version": version, "wezterm": paths::WEZTERM_REV, "codec": codec::CODEC_VERSION })
                );
            } else {
                println!("wezzy {version} (wezterm {}, codec {})", paths::WEZTERM_REV, codec::CODEC_VERSION);
            }
            Ok(())
        }
        #[cfg(feature = "server")]
        Cmd::Proxy => proxy::run(),
        #[cfg(feature = "server")]
        Cmd::Server(cmd) => {
            env_bootstrap::bootstrap();
            config::designate_this_as_the_main_thread();
            let _umask = umask::UmaskSaver::new();
            server::run(cmd)
        }
        cmd => {
            // your pane id means nothing on another host
            if opt.server.is_some() {
                std::env::remove_var("WEZTERM_PANE");
            }
            config::designate_this_as_the_main_thread();
            let client = panes::connect(opt.server.as_deref())?;
            let executor = promise::spawn::ScopedExecutor::new();
            promise::spawn::block_on(executor.run(async move {
                match cmd {
                    Cmd::Ls(o) => panes::ls(client, o).await,
                    Cmd::Split(o) => panes::split(client, o).await,
                    Cmd::New(o) => panes::new(client, o).await,
                    Cmd::Send(o) => panes::send(client, o).await,
                    Cmd::Capture(o) => panes::capture(client, o).await,
                    Cmd::Focus(o) => panes::focus(client, o).await,
                    Cmd::Zoom(o) => panes::zoom(client, o).await,
                    Cmd::Title(o) => panes::title(client, o).await,
                    Cmd::Kill(o) => panes::kill(client, o).await,
                    _ => unreachable!(),
                }
            }))
        }
    }
}
