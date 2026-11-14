//! The `handoff` command.

use crate::{hook, mcp, project_for_dir, Actor, Board, CardDetail, CardPatch, LanePatch, Role, Store, Updates};
use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "handoff", version, about = "A board shared by you and your coding agents")]
struct Args {
    /// Board (default: the one shared board)
    #[arg(short, long, global = true, env = "HANDOFF_PROJECT")]
    project: Option<String>,

    /// Who's acting: you or agent (default: agent inside an agent's shell)
    #[arg(long = "as", global = true, env = "HANDOFF_ACTOR", value_name = "WHO")]
    actor: Option<String>,

    /// Print JSON
    #[arg(long, global = true)]
    json: bool,

    /// The database file
    #[arg(long, global = true, env = "HANDOFF_DB", value_name = "FILE")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the board (the default)
    Board,
    /// List projects
    Projects,
    /// Create, rename or delete a project
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Add, change or delete a lane
    Lane {
        #[command(subcommand)]
        command: LaneCommand,
    },
    /// Add a card. Without --lane it goes to the other side: yours to the
    /// agent's queue, an agent's in front of you
    Add {
        title: String,
        #[arg(short, long)]
        body: Option<String>,
        /// Lane name, role (you, agent, done) or id
        #[arg(short, long)]
        lane: Option<String>,
    },
    /// Show a card and its thread
    Show { id: i64 },
    /// Change a card's title or body
    Edit {
        id: i64,
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long)]
        body: Option<String>,
    },
    /// Move a card to a lane
    Mv {
        id: i64,
        /// Lane name, role (you, agent, done) or id
        lane: String,
        /// Position in the lane, from 1 (default: the bottom)
        #[arg(long)]
        at: Option<usize>,
    },
    /// Mark cards done
    Done {
        #[arg(required = true)]
        ids: Vec<i64>,
        /// A closing note, added to the thread
        #[arg(short = 'm', long)]
        note: Option<String>,
    },
    /// Reply on a card's thread
    Reply { id: i64, text: String },
    /// Delete cards
    Rm {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// What the other side did since you last asked, and what's waiting on you
    Updates {
        /// Don't mark them seen
        #[arg(long)]
        peek: bool,
    },
    /// Serve the board to an agent over MCP (stdio)
    Mcp,
    /// Agent hooks: bring the board into a session
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Create a project
    Add { name: String },
    /// Rename the project (its slug stays)
    Rename { name: String },
    /// Delete the project and everything in it
    Rm { slug: String },
}

#[derive(Subcommand)]
enum LaneCommand {
    /// Add a lane
    Add {
        name: String,
        /// Whose move its cards are: you, agent, none or done
        #[arg(short, long, default_value = "none")]
        role: String,
        /// Position among the lanes, from 1 (default: last)
        #[arg(long)]
        at: Option<usize>,
    },
    /// Rename a lane, change its role or position
    Set {
        lane: String,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(short, long)]
        role: Option<String>,
        #[arg(long)]
        at: Option<usize>,
    },
    /// Delete an empty lane
    Rm { lane: String },
}

#[derive(Subcommand)]
enum HookCommand {
    /// SessionStart: print what's waiting for the agent
    SessionStart,
    /// Stop: keep the agent going when you've handed it something new
    Stop,
}

/// Runs the command and returns the exit code.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let args = Args::parse_from(args);
    match dispatch(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("handoff: {e:#}");
            1
        }
    }
}

/// The actor when none is named: an agent inside an agent's shell, else you.
pub fn default_actor() -> Actor {
    let agent = ["CLAUDECODE", "CODEX_SANDBOX", "PI_CODING_AGENT"]
        .iter()
        .any(|v| std::env::var_os(v).is_some_and(|v| !v.is_empty()));
    if agent {
        Actor::Agent
    } else {
        Actor::You
    }
}

/// The board everything goes to unless one is named.
pub const DEFAULT_PROJECT: &str = "Board";

/// The project named on the command line (or `$HANDOFF_PROJECT`), else the one
/// shared board: agents in every repository file to the same place, and their
/// cards are tagged with where they came from.
pub fn default_project(named: Option<&str>) -> Result<String> {
    Ok(named.filter(|p| !p.trim().is_empty()).unwrap_or(DEFAULT_PROJECT).to_string())
}

/// The tag for cards made from here: the repository of the current directory.
pub fn tag_here() -> Option<String> {
    let dir = std::env::current_dir().ok()?;
    Some(project_for_dir(&dir)).filter(|t| !t.is_empty())
}

fn dispatch(args: Args) -> Result<()> {
    let mut store = match &args.db {
        Some(path) => Store::open(path)?,
        None => Store::open_default()?,
    };
    store.agent = crate::agent_from_env();
    store.tag = tag_here();
    let actor = match &args.actor {
        Some(a) => Actor::parse(a)?,
        None => default_actor(),
    };
    let json = args.json;
    let named = args.project.as_deref();
    // reads need the project to exist; writes make it
    let existing = || store.project(&default_project(named)?);
    let ensured = || store.ensure_project(&default_project(named)?);

    match args.command.unwrap_or(Command::Board) {
        Command::Board => {
            let board = store.board(&existing()?)?;
            out(json, &board, || print_board(&board));
        }
        Command::Projects => {
            let projects = store.projects()?;
            out(json, &projects, || {
                for p in &projects {
                    println!("{}", p.slug);
                }
            });
        }
        Command::Project { command } => match command {
            ProjectCommand::Add { name } => {
                let p = store.ensure_project(&name)?;
                out(json, &p, || println!("{}", p.slug));
            }
            ProjectCommand::Rename { name } => {
                let p = store.rename_project(&existing()?.slug, &name)?;
                out(json, &p, || println!("{}", p.name));
            }
            ProjectCommand::Rm { slug } => store.delete_project(&slug)?,
        },
        Command::Lane { command } => match command {
            LaneCommand::Add { name, role, at } => {
                let p = ensured()?;
                let mut lane = store.add_lane(p.id, &name, Role::parse(&role)?, actor)?;
                if let Some(at) = at {
                    let patch = LanePatch { index: Some(at.saturating_sub(1)), ..Default::default() };
                    lane = store.update_lane(lane.id, patch, actor)?;
                }
                out(json, &lane, || println!("{}", lane.name));
            }
            LaneCommand::Set { lane, name, role, at } => {
                let lane = store.find_lane(existing()?.id, &lane)?;
                let patch = LanePatch {
                    name,
                    role: role.as_deref().map(Role::parse).transpose()?,
                    index: at.map(|n| n.saturating_sub(1)),
                };
                let lane = store.update_lane(lane.id, patch, actor)?;
                out(json, &lane, || println!("{}", lane.name));
            }
            LaneCommand::Rm { lane } => {
                let lane = store.find_lane(existing()?.id, &lane)?;
                store.delete_lane(lane.id, actor)?;
            }
        },
        Command::Add { title, body, lane } => {
            let p = ensured()?;
            let lane = lane.map(|l| store.find_lane(p.id, &l)).transpose()?.map(|l| l.id);
            let card = store.add_card(p.id, lane, &title, body.as_deref().unwrap_or(""), actor)?;
            let card = store.card(card.id)?;
            out(json, &card, || println!("#{} {} · {}", card.card.id, card.card.title, card.lane));
        }
        Command::Show { id } => {
            let card = store.card(id)?;
            out(json, &card, || print_card(&card));
        }
        Command::Edit { id, title, body } => {
            store.update_card(id, CardPatch { title, body }, actor)?;
            let card = store.card(id)?;
            out(json, &card, || print_card(&card));
        }
        Command::Mv { id, lane, at } => {
            let card = store.card(id)?;
            let project = store.project(&card.project)?;
            let lane = store.find_lane(project.id, &lane)?;
            store.move_card(id, lane.id, at.map(|n| n.saturating_sub(1)), actor)?;
            let card = store.card(id)?;
            out(json, &card, || println!("#{} {} · {}", card.card.id, card.card.title, card.lane));
        }
        Command::Done { ids, note } => {
            let mut done = Vec::new();
            for id in ids {
                store.complete_card(id, note.as_deref(), actor)?;
                done.push(store.card(id)?);
            }
            out(json, &done, || {
                for card in &done {
                    println!("#{} {} · {}", card.card.id, card.card.title, card.lane);
                }
            });
        }
        Command::Reply { id, text } => {
            store.reply(id, &text, actor)?;
            let card = store.card(id)?;
            out(json, &card, || print_card(&card));
        }
        Command::Rm { ids } => {
            for id in ids {
                store.delete_card(id, actor)?;
            }
        }
        Command::Updates { peek } => {
            let updates = store.updates(&existing()?, actor, peek)?;
            out(json, &updates, || print!("{}", updates_text(&updates, actor)));
        }
        Command::Mcp => mcp::serve(&store)?,
        Command::Hook { command } => match command {
            HookCommand::SessionStart => hook::session_start(&store, named)?,
            HookCommand::Stop => hook::stop(&store, named)?,
        },
    }
    Ok(())
}

fn out<T: Serialize>(json: bool, value: &T, text: impl FnOnce()) {
    if json {
        println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
    } else {
        text();
    }
}

fn print_board(board: &Board) {
    println!("{}", board.project.name);
    for lane in &board.lanes {
        println!("\n{} ({})", lane.lane.name, lane.cards.len());
        for card in &lane.cards {
            let replies = if card.replies > 0 { format!("  ↩ {}", card.replies) } else { String::new() };
            let tag = card.tag.as_deref().map(|t| format!("  [{t}]")).unwrap_or_default();
            println!("  #{:<4} {}{tag}{replies}", card.id, card.title);
        }
    }
}

fn print_card(card: &CardDetail) {
    println!("#{} {}", card.card.id, card.card.title);
    println!("{} · {} · {}", card.project, card.lane, ago(card.card.updated_at));
    if !card.card.body.is_empty() {
        println!("\n{}", card.card.body);
    }
    for reply in &card.thread {
        println!("\n{} · {}\n{}", reply.author.as_str(), ago(reply.at), reply.body);
    }
}

/// The updates as an agent (or you) would read them.
pub fn updates_text(updates: &Updates, reader: Actor) -> String {
    let mut s = String::new();
    let other = match reader {
        Actor::Agent => "the user",
        Actor::You => "the agent",
    };
    if !updates.events.is_empty() {
        s.push_str(&format!("Since you last checked the handoff board ({}), {other}:\n", updates.project));
        for e in &updates.events {
            let card = e.card.map(|id| format!("#{id} ")).unwrap_or_default();
            let what = match e.kind.as_str() {
                "card.created" => "added",
                "card.updated" => "edited",
                "card.moved" => "moved",
                "card.completed" => "completed",
                "card.reopened" => "reopened",
                "card.replied" => "replied on",
                "card.deleted" => "deleted",
                "card.assigned" => "assigned you",
                "lane.created" => "added the lane",
                "lane.updated" => "changed the lane",
                "lane.deleted" => "deleted the lane",
                other => other,
            };
            s.push_str(&format!("- {what} {card}\"{}\"", e.title));
            if !e.detail.is_empty() {
                let sep = if e.kind == "card.replied" { ": " } else { " — " };
                s.push_str(sep);
                s.push_str(&e.detail);
            }
            s.push('\n');
        }
    }
    if !updates.queue.is_empty() {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(&format!("Waiting on you ({}):\n", updates.project));
        for card in &updates.queue {
            s.push_str(&format!("- #{} {}\n", card.id, card.title));
        }
    }
    s
}

fn ago(at: i64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(at);
    let secs = (now - at).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}
