//! Handoff: a board shared by you and your coding agents.
//!
//! A project has ordered lanes; a lane has a role that says whose move its
//! cards are (`you`, `agent`), that they're finished (`done`), or nothing
//! (`none`). Moving a card between lanes is the handoff. Every change lands in
//! an event log, and a reader asks for what the other side did since it last
//! looked (`Store::updates`).
//!
//! Everything goes through [`Store`], one SQLite file in WAL mode, so the CLI,
//! the MCP server and a frontend can all have it open at once.

pub mod cli;
pub mod hook;
pub mod mcp;
mod store;

pub use store::{
    agent_from_env, default_db_path, project_for_dir, slugify, Actor, Board, Card, CardDetail, CardPatch, Event,
    Lane, LaneCards, LanePatch, Project, Reply, Role, Store, Updates,
};
