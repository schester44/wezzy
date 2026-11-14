//! The agents overlay (⌘J): what's waiting on you, and what each agent has
//! done this session.
//!
//! **Waiting** comes first: agents blocked on you (a permission dialog, a
//! question), then the ones that finished while you were elsewhere; each group
//! oldest first. Below it, each agent's timeline (`activity::timeline`),
//! newest first, one line per piece of work.
//!
//! A Claude or Codex permission dialog can be answered from here: approving
//! sends ↩ to the agent's pane (its first option, "Yes"), rejecting sends esc.
//! The dialog's full input (from the `PermissionRequest` hook, `hooks.rs`) is
//! shown on the selected row, and a risky one (`Request::is_risky`) asks once
//! more before it goes.

use crate::activity;
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

/// A selectable row, stable while others come and go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Wait(PaneId),
    /// a pane's run, by its start time
    Run(PaneId, u64),
    /// an agent's header, above its runs
    Agent(PaneId),
}

/// How much of an agent's timeline shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fold {
    /// just its header
    None,
    /// its newest runs (`RECENT`): how an agent starts, never chosen
    #[default]
    Recent,
    All,
}

impl Fold {
    /// Folding is open or shut: `Recent` is only where an agent starts,
    /// before you've touched it.
    pub fn toggle(self) -> Self {
        match self {
            Fold::All => Fold::None,
            _ => Fold::All,
        }
    }
}

/// Runs shown per agent until you ask for all of them.
const RECENT: usize = 2;

pub struct QueueState {
    /// The selected row; follows it when others come and go.
    pub selected: Option<Key>,
    /// Waiting for a second ↩ before approving this pane's risky request.
    pub confirm: Option<PaneId>,
    /// Agents whose timeline isn't at the default (`Fold::Recent`).
    pub folds: HashMap<PaneId, Fold>,
    /// The shortcut sheet (`?`) is up, with what's typed to filter it.
    pub help: Option<String>,
    pub scroll: ScrollHandle,
}

impl QueueState {
    /// Show more or less of an agent's timeline. A cursor on a run that
    /// goes away moves to the agent's header.
    pub fn set_fold(&mut self, pane: PaneId, fold: Fold) {
        if fold == Fold::default() {
            self.folds.remove(&pane);
        } else {
            self.folds.insert(pane, fold);
        }
        if matches!(self.selected, Some(Key::Run(p, _)) if p == pane) && fold != Fold::All {
            self.selected = Some(Key::Agent(pane));
        }
    }

    pub fn new() -> Self {
        Self {
            selected: None,
            confirm: None,
            folds: HashMap::new(),
            help: None,
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

/// One line of an agent's history.
#[derive(Clone)]
pub struct RunRow {
    pub pane_id: PaneId,
    pub started: u64,
    pub title: String,
    /// what you asked, when the title is something else
    pub prompt: Option<String>,
    /// (added, removed, files)
    pub stats: Option<(u32, u32, usize)>,
    /// unix secs, once a turn of it finished
    pub ended: Option<u64>,
    /// the agent is on it now
    pub working: bool,
    pub reviewable: bool,
}

#[derive(Clone)]
pub enum Entry {
    /// "Waiting", above the items
    WaitHeader(usize),
    Wait(Item),
    /// an agent's name and project, above its runs: how many it has, and
    /// how many of them show
    Agent { pane_id: PaneId, name: String, project: String, runs: usize, fold: Fold },
    Run(RunRow),
}

impl Entry {
    pub fn key(&self) -> Option<Key> {
        match self {
            Entry::Wait(it) => Some(Key::Wait(it.pane_id)),
            Entry::Run(r) => Some(Key::Run(r.pane_id, r.started)),
            Entry::Agent { pane_id, .. } => Some(Key::Agent(*pane_id)),
            _ => None,
        }
    }
}

/// The waiting items, in order.
pub fn items() -> Vec<Item> {
    let mut agents: Vec<Agent> = agents::collect(None).into_iter().filter(queued).collect();
    agents.sort_by_key(|a| (a.status != Status::NeedsInput, a.since));
    agents.into_iter().map(item).collect()
}

/// Everything the overlay lists, in order. Each agent shows its newest runs,
/// all of them, or only its header, per `folds`.
pub fn entries(folds: &HashMap<PaneId, Fold>) -> Vec<Entry> {
    let items = items();
    let mut out = vec![];
    if !items.is_empty() {
        out.push(Entry::WaitHeader(items.len()));
        out.extend(items.into_iter().map(Entry::Wait));
    }
    let mut agents = agents::collect(None);
    // the agent in the pane you're in first
    agents.sort_by_key(|a| !a.focused);
    for a in agents {
        let rows = runs(&a);
        if rows.is_empty() {
            continue;
        }
        let fold = folds.get(&a.pane_id).copied().unwrap_or_default();
        out.push(Entry::Agent {
            pane_id: a.pane_id,
            name: a.display_name().to_string(),
            project: a.project.clone(),
            runs: rows.len(),
            fold,
        });
        let shown = match fold {
            Fold::None => 0,
            Fold::Recent => RECENT,
            Fold::All => usize::MAX,
        };
        out.extend(rows.into_iter().take(shown).map(Entry::Run));
    }
    out
}

/// The agent's timeline, newest first.
fn runs(a: &Agent) -> Vec<RunRow> {
    // Review changes works on the run the bar shows
    let reviewable = activity::run(a.pane_id).is_some_and(|r| r.done && r.has_changes());
    activity::timeline(a.pane_id)
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            // an interrupted turn never reports; trust the agent's status
            let working = l.working && matches!(a.status, Status::Working | Status::NeedsInput);
            let title = if working {
                l.prompt.clone().or_else(|| a.detail.clone())
            } else {
                l.summary.clone().or_else(|| l.prompt.clone())
            };
            RunRow {
                pane_id: a.pane_id,
                started: l.started,
                prompt: l.prompt.clone().filter(|p| Some(p) != title.as_ref()),
                title: title.unwrap_or_else(|| a.display_name().to_string()),
                stats: (l.files > 0).then_some((l.added, l.removed, l.files)),
                ended: (!working && l.ended > 0).then_some(l.ended),
                working,
                reviewable: i == 0 && reviewable,
            }
        })
        .collect()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "now", "12m", "3h", "2d" since `secs`.
pub fn ago(secs: u64) -> String {
    let d = now().saturating_sub(secs);
    match d {
        0..60 => "now".into(),
        60..3600 => format!("{}m", d / 60),
        3600..86400 => format!("{}h", d / 3600),
        _ => format!("{}d", d / 86400),
    }
}

/// In the queue: blocked on you (and not just answered here), or finished
/// while you were elsewhere.
pub fn queued(a: &Agent) -> bool {
    match a.status {
        Status::NeedsInput => !answered(a),
        Status::Done => a.unseen,
        _ => false,
    }
}

/// (blocked, finished) agents in the queue, for the top bar.
pub fn counts(agents: &[Agent]) -> (usize, usize) {
    agents.iter().filter(|a| queued(a)).fold((0, 0), |(b, f), a| {
        if a.status == Status::NeedsInput {
            (b + 1, f)
        } else {
            (b, f + 1)
        }
    })
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

/// The selected row's index in `entries`: the selection, else the first
/// selectable row. `None` when nothing can be selected.
pub fn selected_index(state: &QueueState, entries: &[Entry]) -> Option<usize> {
    state
        .selected
        .and_then(|k| entries.iter().position(|e| e.key() == Some(k)))
        .or_else(|| entries.iter().position(|e| e.key().is_some()))
}

/// The selectable row `step` rows from `from` (clamped).
fn step(entries: &[Entry], from: usize, down: bool) -> Option<usize> {
    let sel = |i: &usize| entries[*i].key().is_some();
    if down {
        (from + 1..entries.len()).find(sel)
    } else {
        (0..from).rev().find(sel)
    }
}

impl TermApp {
    pub(crate) fn open_queue(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Queue(QueueState::new()));
        cx.notify();
    }

    pub(crate) fn queue_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let Some(Overlay::Queue(state)) = self.overlay.as_mut() else {
            return;
        };
        if let Some(query) = state.help.as_mut() {
            if crate::shortcuts::sheet_key(query, ks, cx) {
                state.help = None;
            }
            return;
        }
        if ks.key_char.as_deref() == Some("?") || ks.key == "?" {
            state.help = Some(String::new());
            return;
        }
        let entries = entries(&state.folds);
        let idx = selected_index(state, &entries);
        let current = idx.map(|i| entries[i].clone());
        let item = match &current {
            Some(Entry::Wait(it)) => Some(it.clone()),
            _ => None,
        };

        // a risky approval waits for ↩ (or a again); anything else backs out
        if let Some(pane) = state.confirm.take() {
            if matches!(ks.key.as_str(), "enter" | "a" | "y") {
                if let Some(it) = items().into_iter().find(|i| i.pane_id == pane) {
                    self.answer(&it, true, cx);
                }
            }
            return;
        }

        let select = |state: &mut QueueState, i: Option<usize>| {
            if let Some(i) = i {
                state.selected = entries[i].key();
                state.scroll.scroll_to_item(i);
            }
        };
        let first = entries.iter().position(|e| e.key().is_some());
        let last = entries.iter().rposition(|e| e.key().is_some());
        match ks.key.as_str() {
            "escape" | "q" => {
                self.overlay = None;
                self.focus.focus(window);
            }
            "j" | "down" => select(state, idx.and_then(|i| step(&entries, i, true)).or(idx)),
            "k" | "up" => select(state, idx.and_then(|i| step(&entries, i, false)).or(idx)),
            // the agent the cursor is in: all of its work, none, or the other
            "l" | "right" | "h" | "left" | "tab" => {
                let pane = match current.as_ref().and_then(Entry::key) {
                    Some(Key::Agent(p) | Key::Run(p, _)) => p,
                    _ => return,
                };
                let fold = state.folds.get(&pane).copied().unwrap_or_default();
                let to = match ks.key.as_str() {
                    "l" | "right" => Fold::All,
                    "h" | "left" => Fold::None,
                    _ => fold.toggle(),
                };
                state.set_fold(pane, to);
            }
            // every agent down to its header, to see them all; again opens
            // them all
            "z" => {
                let panes: Vec<PaneId> = entries
                    .iter()
                    .filter_map(|e| match e {
                        Entry::Agent { pane_id, .. } => Some(*pane_id),
                        _ => None,
                    })
                    .collect();
                let all_folded = panes.iter().all(|p| state.folds.get(p) == Some(&Fold::None));
                let to = if all_folded { Fold::All } else { Fold::None };
                for p in panes {
                    state.set_fold(p, to);
                }
            }
            "g" | "home" => select(state, first),
            "G" | "end" => select(state, last),
            k if k.len() == 1 && k.as_bytes()[0].is_ascii_digit() && k != "0" => {
                let n = (k.as_bytes()[0] - b'1') as usize;
                let i = entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| matches!(e, Entry::Wait(_)))
                    .nth(n)
                    .map(|(i, _)| i);
                select(state, i)
            }
            "enter" | "o" => match current.as_ref().and_then(Entry::key) {
                Some(Key::Wait(p) | Key::Run(p, _) | Key::Agent(p)) => self.open_queue_item(p, window, cx),
                None => {}
            },
            "v" => {
                if let Some(Entry::Run(r)) = current {
                    if r.reviewable {
                        self.overlay = None;
                        self.review_run(r.pane_id, window, cx);
                    }
                }
            }
            "a" | "y" => {
                if let Some(it @ Item { ask: Ask::Approve(_), .. }) = item {
                    if it.risky {
                        state.confirm = Some(it.pane_id);
                    } else {
                        self.answer(&it, true, cx);
                    }
                }
            }
            "r" | "n" => {
                if let Some(it @ Item { ask: Ask::Approve(_), .. }) = item {
                    self.answer(&it, false, cx);
                }
            }
            "d" | "x" => {
                if let Some(Item { ask: Ask::Done, pane_id, .. }) = item {
                    agents::mark_seen(pane_id);
                    self.after_answer(idx.unwrap_or(0), cx);
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
        let idx = entries(&self.queue_folds())
            .iter()
            .position(|e| e.key() == Some(Key::Wait(it.pane_id)))
            .unwrap_or(0);
        let ts = agents::collect(None)
            .into_iter()
            .find(|a| a.pane_id == it.pane_id)
            .map_or(0, |a| a.hook_ts);
        ANSWERED.lock().get_or_insert_with(HashMap::new).insert(it.pane_id, ts);
        agents::mark_seen(it.pane_id);
        self.after_answer(idx, cx);
    }

    /// An item went away: select the row now in its place, or close an
    /// empty overlay.
    pub(crate) fn after_answer(&mut self, idx: usize, cx: &mut Context<Self>) {
        let entries = entries(&self.queue_folds());
        if entries.is_empty() {
            self.overlay = None;
        } else if let Some(Overlay::Queue(state)) = self.overlay.as_mut() {
            let still = state.selected.is_some_and(|k| entries.iter().any(|e| e.key() == Some(k)));
            if !still {
                let i = idx.min(entries.len() - 1);
                let i = (i..entries.len()).chain((0..i).rev()).find(|&i| entries[i].key().is_some());
                state.selected = i.and_then(|i| entries[i].key());
            }
        }
        cx.notify();
    }

    /// The open overlay's folds.
    fn queue_folds(&self) -> HashMap<PaneId, Fold> {
        match &self.overlay {
            Some(Overlay::Queue(s)) => s.folds.clone(),
            _ => HashMap::new(),
        }
    }

    /// Go to the agent's pane and close the queue.
    pub(crate) fn open_queue_item(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = None;
        self.region = crate::regions::Region::Terminal;
        self.focus_pane(pane_id, window, cx);
    }
}
