//! Pane commands: `wezzy ls`, `split`, `new`, `send`, `capture`, `focus`,
//! `zoom`, `title`, `kill`. They talk to a session server over the mux
//! protocol, the same requests `wezterm cli` makes.
//!
//! Which server: `-S <host>` reaches a host's server over ssh; otherwise the
//! one this shell runs in (`$WEZTERM_UNIX_SOCKET`), else this machine's.
//! Which pane: `-p <id>`; otherwise your own (`$WEZTERM_PANE`), or with `-S`
//! the pane last focused in a client.

use anyhow::{anyhow, bail, Context as _};
use clap::{Args, Parser};
use mux::pane::PaneId;
use mux::tab::{PaneEntry, PaneNode, SplitDirection, SplitRequest, SplitSize};
use portable_pty::CommandBuilder;
use std::io::Read as _;
use std::path::PathBuf;
use wezterm_client::client::Client;

#[derive(Debug, Args, Clone, Default)]
pub struct Target {
    /// The pane; defaults to your own
    #[arg(short = 'p', long = "pane")]
    pane: Option<PaneId>,
}

#[derive(Debug, Parser)]
pub struct Ls {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
pub struct Split {
    #[command(flatten)]
    target: Target,
    #[arg(long, conflicts_with_all = ["right", "up", "down"])]
    left: bool,
    /// (the default)
    #[arg(long, conflicts_with_all = ["left", "up", "down"])]
    right: bool,
    #[arg(long, conflicts_with_all = ["left", "right", "down"])]
    up: bool,
    #[arg(long, conflicts_with_all = ["left", "right", "up"])]
    down: bool,
    /// Size of the new pane, in percent of the split (default 50)
    #[arg(long)]
    percent: Option<u8>,
    /// Size of the new pane, in cells
    #[arg(long, conflicts_with = "percent")]
    cells: Option<usize>,
    /// Split the whole tab, not just the pane
    #[arg(long)]
    top_level: bool,
    /// Working directory (default: the pane's)
    #[arg(short = 'c', long)]
    cwd: Option<PathBuf>,
    /// Command to run instead of your shell; the pane closes when it exits
    #[arg(last = true)]
    command: Vec<String>,
}

#[derive(Debug, Parser)]
pub struct New {
    #[command(flatten)]
    target: Target,
    /// A new window instead of a tab
    #[arg(long)]
    window: bool,
    /// Workspace for a new window (default: the pane's)
    #[arg(long, requires = "window")]
    workspace: Option<String>,
    #[arg(short = 'c', long)]
    cwd: Option<PathBuf>,
    #[arg(last = true)]
    command: Vec<String>,
}

#[derive(Debug, Parser)]
pub struct Send {
    #[command(flatten)]
    target: Target,
    /// Send as a paste (bracketed) instead of typing it
    #[arg(long)]
    paste: bool,
    /// Press enter after the text
    #[arg(short = 'e', long)]
    enter: bool,
    /// The text; read from stdin when left out
    text: Option<String>,
}

#[derive(Debug, Parser)]
pub struct Capture {
    #[command(flatten)]
    target: Target,
    /// Include this many lines of scrollback above the screen
    #[arg(short = 'n', long, default_value_t = 0)]
    lines: usize,
    /// Keep colors and styles as escape sequences
    #[arg(long)]
    escapes: bool,
}

#[derive(Debug, Parser)]
pub struct Focus {
    pane: PaneId,
}

#[derive(Debug, Parser)]
pub struct Zoom {
    #[command(flatten)]
    target: Target,
    #[arg(long, conflicts_with = "off")]
    on: bool,
    #[arg(long)]
    off: bool,
}

#[derive(Debug, Parser)]
pub struct Title {
    #[command(flatten)]
    target: Target,
    /// The tab's title; empty clears it
    title: String,
}

#[derive(Debug, Parser)]
pub struct Kill {
    /// Panes to close
    #[arg(required = true)]
    panes: Vec<PaneId>,
}

// -- connecting ------------------------------------------------------------------

/// The local server's socket: the one this shell runs in, else Wezzy's on a
/// Mac, else this host's `wezzy server`.
fn local_socket() -> PathBuf {
    if let Some(s) = std::env::var_os("WEZTERM_UNIX_SOCKET").filter(|s| !s.is_empty()) {
        return s.into();
    }
    let app = PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/wezzy/sock");
    if cfg!(target_os = "macos") && app.exists() {
        return app;
    }
    crate::paths::socket()
}

pub fn connect(server: Option<&str>) -> anyhow::Result<Client> {
    let mut ui = mux::connui::ConnectionUI::new_headless();
    let dom = match server {
        Some(target) => config::UnixDomain {
            name: "wezzy".into(),
            proxy_command: Some(crate::paths::ssh_proxy_argv(target)),
            no_serve_automatically: true,
            ..Default::default()
        },
        None => config::UnixDomain {
            name: "wezzy".into(),
            socket_path: Some(local_socket()),
            no_serve_automatically: true,
            ..Default::default()
        },
    };
    Client::new_unix_domain(None, &dom, true, &mut ui, true).map_err(|e| match server {
        Some(s) => anyhow!("{s}: {e:#}"),
        None => anyhow!("no Wezzy session server here ({e:#})"),
    })
}

async fn pane(client: &Client, t: &Target) -> anyhow::Result<PaneId> {
    client.resolve_pane_id(t.pane).await
}

fn entries(node: PaneNode, out: &mut Vec<PaneEntry>) {
    match node {
        PaneNode::Empty => {}
        PaneNode::Split { left, right, .. } => {
            entries(*left, out);
            entries(*right, out);
        }
        PaneNode::Leaf(e) => out.push(e),
    }
}

async fn all_panes(client: &Client) -> anyhow::Result<Vec<(PaneEntry, String)>> {
    let resp = client.list_panes().await?;
    let mut out = vec![];
    for (tab, title) in resp.tabs.into_iter().zip(resp.tab_titles) {
        let mut v = vec![];
        entries(tab, &mut v);
        out.extend(v.into_iter().map(|e| (e, title.clone())));
    }
    Ok(out)
}

async fn find(client: &Client, id: PaneId) -> anyhow::Result<PaneEntry> {
    all_panes(client)
        .await?
        .into_iter()
        .map(|(e, _)| e)
        .find(|e| e.pane_id == id)
        .ok_or_else(|| anyhow!("no pane {id}"))
}

fn cwd_arg(cwd: Option<PathBuf>) -> anyhow::Result<Option<String>> {
    cwd.map(|c| {
        let abs = if c.is_absolute() { c } else { std::env::current_dir()?.join(c) };
        abs.to_str().map(String::from).context("cwd isn't valid UTF-8")
    })
    .transpose()
}

fn command(argv: Vec<String>) -> Option<CommandBuilder> {
    (!argv.is_empty()).then(|| CommandBuilder::from_argv(argv.into_iter().map(Into::into).collect()))
}

// -- commands --------------------------------------------------------------------

#[derive(serde::Serialize)]
struct Row {
    window: mux::window::WindowId,
    tab: mux::tab::TabId,
    pane: PaneId,
    workspace: String,
    title: String,
    tab_title: String,
    cwd: String,
    cols: usize,
    rows: usize,
    active: bool,
    zoomed: bool,
}

pub async fn ls(client: Client, o: Ls) -> anyhow::Result<()> {
    let rows: Vec<Row> = all_panes(&client)
        .await?
        .into_iter()
        .map(|(e, tab_title)| Row {
            window: e.window_id,
            tab: e.tab_id,
            pane: e.pane_id,
            workspace: e.workspace,
            title: e.title,
            tab_title,
            cwd: e.working_dir.map(|u| u.url.path().to_string()).unwrap_or_default(),
            cols: e.size.cols,
            rows: e.size.rows,
            active: e.is_active_pane,
            zoomed: e.is_zoomed_pane,
        })
        .collect();
    if o.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let table: Vec<[String; 6]> = rows
        .iter()
        .map(|r| {
            let cwd = match r.cwd.strip_prefix(&home) {
                Some(rest) if !home.is_empty() => format!("~{rest}"),
                _ => r.cwd.clone(),
            };
            [r.pane.to_string(), r.tab.to_string(), r.window.to_string(), r.workspace.clone(), r.title.clone(), cwd]
        })
        .collect();
    let header = ["PANE", "TAB", "WINDOW", "WORKSPACE", "TITLE", "CWD"].map(String::from);
    let mut widths = [0usize; 6];
    for row in std::iter::once(&header).chain(&table) {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    for row in std::iter::once(&header).chain(&table) {
        let line: Vec<String> = row.iter().zip(widths).map(|(c, w)| format!("{c:<w$}")).collect();
        println!("{}", line.join("  ").trim_end());
    }
    Ok(())
}

pub async fn split(client: Client, o: Split) -> anyhow::Result<()> {
    let pane_id = pane(&client, &o.target).await?;
    let direction = if o.up || o.down { SplitDirection::Vertical } else { SplitDirection::Horizontal };
    let size = match (o.cells, o.percent) {
        (Some(c), _) => SplitSize::Cells(c),
        (_, Some(p)) => SplitSize::Percent(p.clamp(1, 99)),
        _ => SplitSize::Percent(50),
    };
    let spawned = client
        .split_pane(codec::SplitPane {
            pane_id,
            split_request: SplitRequest {
                direction,
                target_is_second: !(o.left || o.up),
                size,
                top_level: o.top_level,
            },
            domain: config::keyassignment::SpawnTabDomain::CurrentPaneDomain,
            command: command(o.command),
            command_dir: cwd_arg(o.cwd)?,
            move_pane_id: None,
        })
        .await?;
    println!("{}", spawned.pane_id);
    Ok(())
}

pub async fn new(client: Client, o: New) -> anyhow::Result<()> {
    // a new window needs no pane to start from (e.g. with -S)
    let from = match pane(&client, &o.target).await {
        Ok(id) => Some(find(&client, id).await?),
        Err(_) if o.window => None,
        Err(e) => return Err(e),
    };
    let spawned = client
        .spawn_v2(codec::SpawnV2 {
            domain: config::keyassignment::SpawnTabDomain::DefaultDomain,
            window_id: if o.window { None } else { from.as_ref().map(|f| f.window_id) },
            command: command(o.command),
            command_dir: cwd_arg(o.cwd)?,
            size: from.as_ref().map(|f| f.size).unwrap_or(wezterm_term::TerminalSize {
                rows: 24,
                cols: 80,
                ..Default::default()
            }),
            // your workspace, not "default": Wezzy shows one at a time
            workspace: o
                .workspace
                .or_else(|| from.map(|f| f.workspace))
                .unwrap_or_else(|| "default".into()),
        })
        .await?;
    println!("{}", spawned.pane_id);
    Ok(())
}

pub async fn send(client: Client, o: Send) -> anyhow::Result<()> {
    let pane_id = pane(&client, &o.target).await?;
    let mut text = match o.text {
        Some(t) => t,
        None => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).context("reading stdin")?;
            s
        }
    };
    if o.paste {
        client.send_paste(codec::SendPaste { pane_id, data: text }).await?;
        if o.enter {
            client.write_to_pane(codec::WriteToPane { pane_id, data: b"\r".to_vec() }).await?;
        }
    } else {
        if o.enter {
            text.push('\r');
        }
        client.write_to_pane(codec::WriteToPane { pane_id, data: text.into_bytes() }).await?;
    }
    Ok(())
}

pub async fn capture(client: Client, o: Capture) -> anyhow::Result<()> {
    let pane_id = pane(&client, &o.target).await?;
    let dims = client.get_dimensions(codec::GetPaneRenderableDimensions { pane_id }).await?.dimensions;
    let top = dims.physical_top.saturating_sub(o.lines as isize).max(dims.scrollback_top);
    let end = dims.physical_top + dims.viewport_rows as isize;
    let resp = client.get_lines(codec::GetLines { pane_id, lines: vec![top..end] }).await?;
    let lines: Vec<_> = resp.lines.extract_data().0.into_iter().map(|(_, l)| l).collect();
    let mut out: Vec<String> = if o.escapes {
        termwiz_funcs::lines_to_escapes(lines)?.lines().map(String::from).collect()
    } else {
        lines.iter().map(|l| l.as_str().trim_end().to_string()).collect()
    };
    // the screen's blank tail isn't output
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    for l in out {
        println!("{l}");
    }
    Ok(())
}

pub async fn focus(client: Client, o: Focus) -> anyhow::Result<()> {
    find(&client, o.pane).await?;
    client.set_focused_pane_id(codec::SetFocusedPane { pane_id: o.pane }).await?;
    Ok(())
}

pub async fn zoom(client: Client, o: Zoom) -> anyhow::Result<()> {
    let pane_id = pane(&client, &o.target).await?;
    let e = find(&client, pane_id).await?;
    let zoomed = if o.on {
        true
    } else if o.off {
        false
    } else {
        !(e.is_zoomed_pane && e.is_active_pane)
    };
    client
        .set_zoomed(codec::SetPaneZoomed { containing_tab_id: e.tab_id, pane_id, zoomed })
        .await?;
    Ok(())
}

pub async fn title(client: Client, o: Title) -> anyhow::Result<()> {
    let e = find(&client, pane(&client, &o.target).await?).await?;
    client.set_tab_title(codec::TabTitleChanged { tab_id: e.tab_id, title: o.title }).await?;
    Ok(())
}

pub async fn kill(client: Client, o: Kill) -> anyhow::Result<()> {
    let own = std::env::var("WEZTERM_PANE").ok().and_then(|p| p.parse::<PaneId>().ok());
    for id in o.panes {
        if Some(id) == own {
            bail!("pane {id} is the one you're running in");
        }
        client.kill_pane(codec::KillPane { pane_id: id }).await?;
    }
    Ok(())
}
