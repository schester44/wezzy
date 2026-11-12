//! The attention queue (⌘J): every agent waiting on you, in one list.
//!
//! Agents blocked on you (a permission dialog, a question) come first, then
//! the ones that finished while you were elsewhere; each group oldest first.
//! A Claude or Codex permission dialog can be answered from here: approving
//! sends ↩ to the agent's pane (its first option, "Yes"), rejecting sends esc.
//! The dialog's full input (from the `PermissionRequest` hook, `hooks.rs`) is
//! shown on the selected row, and a risky one (`Request::is_risky`) asks once
//! more before it goes.

use crate::agents::{self, Agent, Status};
use crate::app::{Overlay, TermApp};
use crate::hooks::Request;
use gpui::{Context, KeyDownEvent, ScrollHandle, Window};
use mux::pane::PaneId;
use mux::Mux;
use parking_lot::Mutex;
use std::collections::HashMap;

/// Dialogs answered from the queue: pane -> the agent's hook record time when
/// we answered. The item stays hidden until the agent reports anything newer,
/// so it doesn't linger for the poll interval (or, after a rejection that
/// fires no hook, until the next prompt). Main thread only.
static ANSWERED: Mutex<Option<HashMap<PaneId, u64>>> = Mutex::new(None);

fn answered(a: &Agent) -> bool {
    ANSWERED.lock().as_ref().and_then(|m| m.get(&a.pane_id)) == Some(&a.hook_ts)
}

pub struct QueueState {
    /// The selected item's pane; follows the item when others come and go.
    pub selected: Option<PaneId>,
    /// Waiting for a second ↩ before approving this pane's risky request.
    pub confirm: Option<PaneId>,
    pub scroll: ScrollHandle,
}

impl QueueState {
    pub fn new() -> Self {
        Self {
            selected: None,
            confirm: None,
            scroll: ScrollHandle::new(),
        }
    }
}

#[derive(Clone)]
pub enum Ask {
    /// A permission dialog you can approve or reject from here.
    Approve(Request),
    /// Blocked on you some other way (a question, a plan, a dialog we can't
    /// answer): open the pane. The request, when there is one, is shown.
    Question(Option<Request>),
    /// Finished its turn while you were elsewhere.
    Done,
}

#[derive(Clone)]
pub struct Item {
    pub pane_id: PaneId,
    pub ask: Ask,
    pub title: String,
    /// project · agent · more
    pub subtitle: String,
    /// The text to read before answering, on the selected row.
    pub detail: Option<String>,
    pub risky: bool,
}

/// The queue's items, in order.
pub fn items() -> Vec<Item> {
    let mut agents: Vec<Agent> = agents::collect(None)
        .into_iter()
        .filter(|a| match a.status {
            Status::NeedsInput => !answered(a),
            Status::Done => a.unseen,
            _ => false,
        })
        .collect();
    agents.sort_by_key(|a| (a.status != Status::NeedsInput, a.since));
    agents.into_iter().map(item).collect()
}

fn item(a: Agent) -> Item {
    let mut sub = vec![];
    if !a.project.is_empty() {
        sub.push(a.project.clone());
    }
    sub.push(a.display_name().to_string());
    let ask = match (a.status, a.request.clone()) {
        (Status::Done, _) => Ask::Done,
        (_, Some(r)) if r.is_approval() && approvable(a.kind) => Ask::Approve(r),
        (_, r) => Ask::Question(r),
    };
    let (title, detail, risky) = match &ask {
        Ask::Approve(r) | Ask::Question(Some(r)) => {
            let full = r.full();
            let title = r.summary();
            // the command's first line, when the title is its description
            let first = full.lines().next().unwrap_or("").trim();
            if !first.is_empty() && !title.contains(first) && first.chars().count() <= 60 {
                sub.push(first.to_string());
            }
            (title, Some(full), matches!(ask, Ask::Approve(_)) && r.is_risky())
        }
        Ask::Question(None) => (
            a.detail.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| fallback_title(&a)),
            None,
            false,
        ),
        Ask::Done => (fallback_title(&a), None, false),
    };
    Item {
        pane_id: a.pane_id,
        ask,
        title,
        subtitle: sub.join(" · "),
        detail,
        risky,
    }
}

/// The pane title (agents put their task there), else the agent's name.
fn fallback_title(a: &Agent) -> String {
    if a.title.is_empty() || a.title.eq_ignore_ascii_case(a.kind) {
        a.display_name().to_string()
    } else {
        a.title.clone()
    }
}

/// Harnesses whose permission dialog takes ↩ for "Yes" and esc for "No".
fn approvable(kind: &str) -> bool {
    matches!(kind, "Claude" | "Codex")
}

/// The selected item's index in `items`.
pub fn selected_index(state: &QueueState, items: &[Item]) -> usize {
    state
        .selected
        .and_then(|p| items.iter().position(|i| i.pane_id == p))
        .unwrap_or(0)
}

impl TermApp {
    pub(crate) fn open_queue(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Queue(QueueState::new()));
        cx.notify();
    }

    pub(crate) fn queue_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let items = items();
        let Some(Overlay::Queue(state)) = self.overlay.as_mut() else {
            return;
        };
        let idx = selected_index(state, &items);
        let current = items.get(idx).cloned();

        // a risky approval waits for ↩ (or a again); anything else backs out
        if let Some(pane) = state.confirm.take() {
            if matches!(ks.key.as_str(), "enter" | "a" | "y") {
                if let Some(it) = items.iter().find(|i| i.pane_id == pane) {
                    self.answer(it, true, cx);
                }
            }
            return;
        }

        let select = |state: &mut QueueState, i: usize| {
            if let Some(it) = items.get(i) {
                state.selected = Some(it.pane_id);
                state.scroll.scroll_to_item(i);
            }
        };
        match ks.key.as_str() {
            "escape" | "q" => {
                self.overlay = None;
                self.focus.focus(window);
            }
            "j" | "down" => select(state, (idx + 1).min(items.len().saturating_sub(1))),
            "k" | "up" => select(state, idx.saturating_sub(1)),
            "g" | "home" => select(state, 0),
            "G" | "end" => select(state, items.len().saturating_sub(1)),
            k if k.len() == 1 && k.as_bytes()[0].is_ascii_digit() && k != "0" => {
                select(state, (k.as_bytes()[0] - b'1') as usize)
            }
            "enter" | "o" => {
                if let Some(it) = current {
                    self.open_queue_item(it.pane_id, window, cx);
                }
            }
            "a" | "y" => match current {
                Some(it @ Item { ask: Ask::Approve(_), .. }) => {
                    if it.risky {
                        state.confirm = Some(it.pane_id);
                    } else {
                        self.answer(&it, true, cx);
                    }
                }
                _ => {}
            },
            "r" | "n" => {
                if let Some(it @ Item { ask: Ask::Approve(_), .. }) = current {
                    self.answer(&it, false, cx);
                }
            }
            "d" | "x" => {
                if let Some(Item { ask: Ask::Done, pane_id, .. }) = current {
                    agents::mark_seen(pane_id);
                    self.after_answer(idx, cx);
                }
            }
            _ => {}
        }
    }

    /// Answer a permission dialog in its pane: ↩ approves, esc rejects.
    pub(crate) fn answer(&mut self, it: &Item, approve: bool, cx: &mut Context<Self>) {
        let Ask::Approve(_) = it.ask else {
            return;
        };
        let Some(pane) = Mux::get().get_pane(it.pane_id) else {
            return;
        };
        let key = if approve {
            termwiz::input::KeyCode::Enter
        } else {
            termwiz::input::KeyCode::Escape
        };
        if let Err(e) = pane.key_down(key, termwiz::input::Modifiers::NONE) {
            self.flash(format!("Couldn't reach the agent: {e:#}"));
            return;
        }
        let idx = items().iter().position(|i| i.pane_id == it.pane_id).unwrap_or(0);
        let ts = agents::collect(None)
            .into_iter()
            .find(|a| a.pane_id == it.pane_id)
            .map_or(0, |a| a.hook_ts);
        ANSWERED.lock().get_or_insert_with(HashMap::new).insert(it.pane_id, ts);
        agents::mark_seen(it.pane_id);
        self.after_answer(idx, cx);
    }

    /// An item went away: select the one after it, or close an empty queue.
    fn after_answer(&mut self, idx: usize, cx: &mut Context<Self>) {
        let items = items();
        if items.is_empty() {
            self.overlay = None;
        } else if let Some(Overlay::Queue(state)) = self.overlay.as_mut() {
            let still = state.selected.is_some_and(|p| items.iter().any(|i| i.pane_id == p));
            if !still {
                let i = idx.min(items.len() - 1);
                state.selected = Some(items[i].pane_id);
            }
        }
        cx.notify();
    }

    /// Go to the agent's pane and close the queue.
    pub(crate) fn open_queue_item(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.region = crate::regions::Region::Terminal;
        self.focus_pane(pane_id, window, cx);
    }
}
