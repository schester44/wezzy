//! `handoff mcp`: the board as MCP tools over stdio (newline-delimited
//! JSON-RPC). The agent's harness starts it in the project's directory, so
//! `project` defaults to that repository.

use crate::cli::default_project;
use crate::{Actor, CardPatch, LanePatch, Project, Role, Store};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const INSTRUCTIONS: &str = "Handoff is where you leave things for the user while you keep \
working: a board of cards they read, answer and tick off.\n\n\
File a card with create_cards EVERY time one of these comes up, as it comes up, and keep going \
with whatever doesn't depend on it:\n\
- an open question, or a decision you made for them that they might want to reverse\n\
- something only they can do (credentials, a deploy, a manual check, an approval)\n\
- a follow-up or extra work you noticed but didn't do\n\
- something you couldn't finish or couldn't verify\n\n\
Saying it in your reply is not enough: a message scrolls away, a card stays until it's dealt \
with. If your final message would contain a question, a 'you'll need to…' or a 'next steps' \
list, each of those items must already be a card. One card per item.\n\n\
Call get_updates when you start and whenever you finish a piece of work: it returns what the \
user did since you last looked (answers, completed cards, new work) and the cards waiting on \
you. Answers arrive as replies on the card: read them with get_card. When you take a card from \
your queue, move it to the in-progress lane; complete_card it when it's finished, with a note on \
what you did.\n\n\
Your queue and your updates are only the cards that are yours: the ones you filed, the ones \
the user assigned to you, and the ones you took. Other agents work on this board too; don't \
touch their cards. get_board shows everything, including cards nobody owns yet: moving or \
editing one of those makes it yours.\n\n\
Lanes say whose move a card is: the role `you` waits on the user, `agent` waits on you, `done` \
is finished.";

fn tools() -> Value {
    let project = json!({
        "type": "string",
        "description": "Another board, by slug. Leave it out: everything goes to the one shared board, and your cards are tagged with the repository you're in."
    });
    let lane = json!({
        "type": "string",
        "description": "Lane name, role (`you`, `agent`, `done`: the first lane with it) or id."
    });
    json!([
        {
            "name": "list_projects",
            "description": "List the projects on the board, with how many cards wait on you in each.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "create_project",
            "description": "Create a project with the default lanes (Needs you, Agent queue, In progress, Done). Projects are also created on first use, so this is only needed for one that isn't the current repository.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }
        },
        {
            "name": "get_board",
            "description": "The project's lanes and their cards, in order.",
            "inputSchema": { "type": "object", "properties": { "project": project } }
        },
        {
            "name": "get_updates",
            "description": "What the user did since you last called this (answers to your questions, completed action items, new work for you), plus the cards waiting on you now. Call it when you start, and every time you finish a piece of work, before you stop.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project,
                    "peek": { "type": "boolean", "description": "Leave the events unseen." }
                }
            }
        },
        {
            "name": "get_card",
            "description": "A card with its body and its thread of replies.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "integer" } },
                "required": ["id"]
            }
        },
        {
            "name": "create_cards",
            "description": "Leave open questions, action items and follow-ups for the user. Use it every time you have a question for them, need them to do something, made a call they should know about, or noticed work you didn't do, while you carry on with the rest. Never leave these only in your reply. One card per item: a title they can act on without the conversation (\"Which Stripe key should staging use?\", not \"Question about keys\") and the context in the body. Without a lane the cards land in front of the user.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project,
                    "cards": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string" },
                                "body": { "type": "string", "description": "Markdown." },
                                "lane": lane
                            },
                            "required": ["title"]
                        }
                    }
                },
                "required": ["cards"]
            }
        },
        {
            "name": "update_card",
            "description": "Change a card's title or body, and/or move it to a lane (to the bottom, or to `position`, counted from 1). Moving a card to a `done` lane completes it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "title": { "type": "string" },
                    "body": { "type": "string" },
                    "lane": lane,
                    "position": { "type": "integer", "minimum": 1 }
                },
                "required": ["id"]
            }
        },
        {
            "name": "complete_card",
            "description": "Mark a card done, with a note on what was done.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "integer" }, "note": { "type": "string" } },
                "required": ["id"]
            }
        },
        {
            "name": "reply_to_card",
            "description": "Add a reply to a card's thread.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "integer" }, "body": { "type": "string" } },
                "required": ["id", "body"]
            }
        },
        {
            "name": "delete_card",
            "description": "Delete a card.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "integer" } },
                "required": ["id"]
            }
        },
        {
            "name": "create_lane",
            "description": "Add a lane. `role` says whose move its cards are: `you` (the user), `agent`, `done`, or `none`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project,
                    "name": { "type": "string" },
                    "role": { "type": "string", "enum": ["you", "agent", "none", "done"] },
                    "position": { "type": "integer", "minimum": 1 }
                },
                "required": ["name"]
            }
        },
        {
            "name": "update_lane",
            "description": "Rename a lane, change its role, or move it to `position` among the lanes (from 1).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": project,
                    "lane": lane,
                    "name": { "type": "string" },
                    "role": { "type": "string", "enum": ["you", "agent", "none", "done"] },
                    "position": { "type": "integer", "minimum": 1 }
                },
                "required": ["lane"]
            }
        },
        {
            "name": "delete_lane",
            "description": "Delete an empty lane.",
            "inputSchema": {
                "type": "object",
                "properties": { "project": project, "lane": lane },
                "required": ["lane"]
            }
        }
    ])
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().filter(|s| !s.trim().is_empty())
}

fn id_arg(args: &Value) -> Result<i64> {
    args["id"].as_i64().ok_or_else(|| anyhow!("`id` is required"))
}

fn index_arg(args: &Value) -> Option<usize> {
    args["position"].as_u64().map(|n| (n as usize).saturating_sub(1))
}

fn existing(store: &Store, args: &Value) -> Result<Project> {
    let slug = default_project(str_arg(args, "project"))?;
    store.find_project(&slug)?.ok_or_else(|| {
        anyhow!("no project {slug:?} yet: create_cards or create_project makes it")
    })
}

fn ensured(store: &Store, args: &Value) -> Result<Project> {
    store.ensure_project(&default_project(str_arg(args, "project"))?)
}

/// Runs one tool. Everything an MCP session does is the agent's doing.
pub fn call(store: &Store, name: &str, args: &Value) -> Result<Value> {
    let actor = Actor::Agent;
    Ok(match name {
        "list_projects" => {
            let waiting = store.waiting(actor)?;
            let projects: Vec<Value> = store
                .projects()?
                .into_iter()
                .map(|p| {
                    let n = waiting.iter().find(|(w, _)| w.id == p.id).map_or(0, |(_, n)| *n);
                    json!({ "slug": p.slug, "name": p.name, "waiting_on_you": n })
                })
                .collect();
            json!(projects)
        }
        "create_project" => {
            let name = str_arg(args, "name").ok_or_else(|| anyhow!("`name` is required"))?;
            json!(store.board(&store.ensure_project(name)?)?)
        }
        "get_board" => json!(store.board(&existing(store, args)?)?),
        "get_updates" => {
            let peek = args["peek"].as_bool().unwrap_or(false);
            json!(store.updates(&existing(store, args)?, actor, peek)?)
        }
        "get_card" => json!(store.card(id_arg(args)?)?),
        "create_cards" => {
            let cards = args["cards"].as_array().filter(|c| !c.is_empty());
            let cards = cards.ok_or_else(|| anyhow!("`cards` needs at least one card"))?;
            let project = ensured(store, args)?;
            // resolve everything first, so a bad lane adds nothing
            let mut resolved = Vec::new();
            for card in cards {
                let title = str_arg(card, "title").ok_or_else(|| anyhow!("every card needs a `title`"))?;
                let lane = str_arg(card, "lane").map(|l| store.find_lane(project.id, l)).transpose()?;
                resolved.push((title, card["body"].as_str().unwrap_or(""), lane.map(|l| l.id)));
            }
            let mut made = Vec::new();
            for (title, body, lane) in resolved {
                let card = store.add_card(project.id, lane, title, body, actor)?;
                made.push(store.card(card.id)?);
            }
            json!(made)
        }
        "update_card" => {
            let id = id_arg(args)?;
            let patch = CardPatch {
                title: str_arg(args, "title").map(String::from),
                body: args["body"].as_str().map(String::from),
            };
            let lane = str_arg(args, "lane");
            if patch.title.is_none() && patch.body.is_none() && lane.is_none() && index_arg(args).is_none() {
                bail!("nothing to change: pass `title`, `body`, `lane` or `position`");
            }
            let card = store.update_card(id, patch, actor)?;
            if lane.is_some() || index_arg(args).is_some() {
                let lane_id = match lane {
                    Some(l) => store.find_lane(card.project_id, l)?.id,
                    None => card.lane_id,
                };
                store.move_card(id, lane_id, index_arg(args), actor)?;
            }
            json!(store.card(id)?)
        }
        "complete_card" => {
            let id = id_arg(args)?;
            store.complete_card(id, str_arg(args, "note"), actor)?;
            json!(store.card(id)?)
        }
        "reply_to_card" => {
            let id = id_arg(args)?;
            let body = str_arg(args, "body").ok_or_else(|| anyhow!("`body` is required"))?;
            store.reply(id, body, actor)?;
            json!(store.card(id)?)
        }
        "delete_card" => {
            store.delete_card(id_arg(args)?, actor)?;
            json!({ "deleted": true })
        }
        "create_lane" => {
            let name = str_arg(args, "name").ok_or_else(|| anyhow!("`name` is required"))?;
            let role = str_arg(args, "role").map(Role::parse).transpose()?.unwrap_or(Role::None);
            let project = ensured(store, args)?;
            let lane = store.add_lane(project.id, name, role, actor)?;
            if let Some(index) = index_arg(args) {
                store.update_lane(lane.id, LanePatch { index: Some(index), ..Default::default() }, actor)?;
            }
            json!(store.board(&project)?)
        }
        "update_lane" => {
            let project = existing(store, args)?;
            let key = str_arg(args, "lane").ok_or_else(|| anyhow!("`lane` is required"))?;
            let lane = store.find_lane(project.id, key)?;
            let patch = LanePatch {
                name: str_arg(args, "name").map(String::from),
                role: str_arg(args, "role").map(Role::parse).transpose()?,
                index: index_arg(args),
            };
            store.update_lane(lane.id, patch, actor)?;
            json!(store.board(&project)?)
        }
        "delete_lane" => {
            let project = existing(store, args)?;
            let key = str_arg(args, "lane").ok_or_else(|| anyhow!("`lane` is required"))?;
            store.delete_lane(store.find_lane(project.id, key)?.id, actor)?;
            json!(store.board(&project)?)
        }
        other => bail!("unknown tool {other:?}"),
    })
}

/// Answers one JSON-RPC message; `None` for notifications.
pub fn handle(store: &Store, msg: &Value) -> Option<Value> {
    let id = msg.get("id").filter(|id| !id.is_null())?.clone();
    let method = msg["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => {
            let version = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "handoff", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let empty = json!({});
            let args = msg["params"].get("arguments").filter(|a| a.is_object()).unwrap_or(&empty);
            // a failed tool is a result the model can read, not a protocol error
            Ok(match call(store, name, args) {
                Ok(value) => json!({
                    "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }]
                }),
                Err(e) => json!({
                    "content": [{ "type": "text", "text": format!("{e:#}") }],
                    "isError": true
                }),
            })
        }
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

pub fn serve(store: &Store) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(store, &msg),
            Err(e) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": format!("parse error: {e}") }
            })),
        };
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}
