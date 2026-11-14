//! `wezzy proxy`: what the app runs over ssh. Connects stdio to this host's
//! server socket, starting the server first if nothing answers. Same as
//! `wezterm cli proxy`, without needing the rest of wezterm on the host.

use crate::paths;
use anyhow::Context as _;
use codec::{Pdu, SetClientId};
use mux::client::ClientId;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn run() -> anyhow::Result<()> {
    let mut stream = match UnixStream::connect(paths::socket()) {
        Ok(s) => s,
        Err(_) => {
            start_server()?;
            connect_with_retry()?
        }
    };

    // Tell the server this connection is a proxy, so it doesn't count as a
    // client of its own (the real client says hello through us next).
    Pdu::SetClientId(SetClientId {
        client_id: ClientId::new(),
        is_proxy: true,
    })
    .encode(&mut stream, 1)?;
    Pdu::decode(&mut stream)?;

    let mut up = stream.try_clone()?;
    let down = std::thread::spawn(move || {
        copy(&mut stream, &mut std::io::stdout().lock());
    });
    std::thread::spawn(move || {
        copy(&mut std::io::stdin().lock(), &mut up);
        up.shutdown(std::net::Shutdown::Write).ok();
    });
    down.join().ok();
    Ok(())
}

fn copy(from: &mut impl Read, to: &mut impl Write) {
    let mut buf = [0u8; 16384];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if to.write_all(&buf[..n]).and_then(|_| to.flush()).is_err() {
                    return;
                }
            }
        }
    }
}

/// `wezzy server start` with this binary.
fn start_server() -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("finding wezzy")?;
    let status = std::process::Command::new(exe)
        .args(["server", "start"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status()
        .context("starting the server")?;
    anyhow::ensure!(status.success(), "the server didn't start; see {}", paths::log_file().display());
    Ok(())
}

fn connect_with_retry() -> anyhow::Result<UnixStream> {
    let sock = paths::socket();
    let mut last = None;
    for i in 0..40 {
        match UnixStream::connect(&sock) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(50 + i * 10));
    }
    Err(last.unwrap()).with_context(|| format!("connecting to {}", sock.display()))
}
