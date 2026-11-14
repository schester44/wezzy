//! Agent hooks (Claude Code's `SessionStart` and `Stop`): the board comes to
//! the agent instead of the agent having to remember to look.
//!
//! Both are silent, and exit 0, when the directory has no project or there's
//! nothing to say: a hook must never get in an agent's way.

use crate::cli::{default_project, updates_text};
use crate::{Actor, Store};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::Read;

fn news(store: &Store, named: Option<&str>) -> Result<Option<(String, bool)>> {
    let Ok(slug) = default_project(named) else { return Ok(None) };
    let Some(project) = store.find_project(&slug)? else { return Ok(None) };
    let updates = store.updates(&project, Actor::Agent, false)?;
    let text = updates_text(&updates, Actor::Agent);
    Ok((!text.is_empty()).then_some((text, !updates.events.is_empty())))
}

/// Prints the agent's queue and the user's changes; Claude Code adds a
/// SessionStart hook's stdout to the session's context.
pub fn session_start(store: &Store, named: Option<&str>) -> Result<()> {
    if let Some((text, _)) = news(store, named)? {
        println!(
            "{text}\nUse the handoff tools (or the `handoff` command) to read cards, reply and mark work done. \
             File your own open questions, action items for the user and follow-ups there too, as cards."
        );
    }
    Ok(())
}

/// Blocks the stop when the user changed the board since the agent last
/// looked, so answers and new work reach an agent that was about to go idle.
/// A queue with nothing new in it doesn't block: the agent has seen it.
pub fn stop(store: &Store, named: Option<&str>) -> Result<()> {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let input: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    // already continuing because of a stop hook: don't loop
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(());
    }
    if let Some((text, true)) = news(store, named)? {
        println!("{}", json!({ "decision": "block", "reason": text }));
    }
    Ok(())
}
