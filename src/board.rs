//! The board (⌘⇧J): Handoff's lanes and cards for the project you're in.
//!
//! Handoff (`handoff/`) is its own crate: a SQLite file, a library and the
//! `handoff` command agents use (CLI, MCP, hooks). This module is one frontend
//! for it and holds no data of its own: every change goes through
//! `handoff::Store`, and the overlay re-reads the board when the file changes
//! under it (an agent filing a card while you look).
//!
//! `install` wires agents up: `handoff` on PATH (a link to this binary, like
//! `wezzy`), the MCP server registered with Claude Code, and its
//! `SessionStart` / `Stop` hooks.

use crate::app::{Overlay, TermApp};
use crate::textfield::{Edit, TextField};
use gpui::{Context, KeyDownEvent, Keystroke, ScrollHandle, Window};
use handoff::{Actor, Board, Card, CardDetail, CardPatch, LanePatch, Project, Role, Store};
use crate::agents::Status;
use mux::pane::PaneId;
use mux::Mux;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often an open board checks whether the file changed.
const POLL: Duration = Duration::from_millis(700);
/// How often the top bar's count is refreshed.
const WAITING_POLL: Duration = Duration::from_secs(2);
/// Between pasting into an agent's prompt and pressing ↩ on it.
const SUBMIT_DELAY: Duration = Duration::from_millis(250);

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Cards waiting on you on every board, by where they came from (a
/// repository, or a board's name), most first; kept by `start`'s thread.
static WAITING: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());

/// What waits on you: the top bar's chip (the total) and its tooltip.
pub fn waiting() -> Vec<(String, usize)> {
    WAITING.lock().clone()
}

/// Watch the boards for cards that need you. A new one repaints the top bar
/// and chimes like an agent asking for you does.
pub fn start() {
    std::thread::Builder::new()
        .name("board".into())
        .spawn(|| {
            let mut first = true;
            loop {
                // no board yet: don't make one just to count it
                if !handoff::default_db_path().exists() {
                    std::thread::sleep(WAITING_POLL);
                    continue;
                }
                let now = with_store(|s| s.waiting_by_source(Actor::You));
                if let Ok(now) = now {
                    let now: Vec<(String, usize)> = now.into_iter().map(|(s, n)| (s, n.max(0) as usize)).collect();
                    let total = |v: &[(String, usize)]| v.iter().map(|(_, n)| n).sum::<usize>();
                    let before = std::mem::replace(&mut *WAITING.lock(), now.clone());
                    if now != before {
                        crate::app::send_event(crate::app::AppEvent::Redraw);
                        if total(&now) > total(&before) && !first {
                            if let Some(sound) = crate::settings::get().sound() {
                                crate::sound::play(&sound);
                            }
                        }
                    }
                    first = false;
                }
                std::thread::sleep(WAITING_POLL);
            }
        })
        .ok();
}

/// Run `f` against the store, opened on first use.
pub fn with_store<R>(f: impl FnOnce(&Store) -> anyhow::Result<R>) -> anyhow::Result<R> {
    let mut guard = STORE.lock();
    if guard.is_none() {
        *guard = Some(Store::open_default()?);
    }
    f(guard.as_ref().expect("opened above"))
}

/// What the text box is for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    /// a new card in this lane: the first line is its title, the rest its body
    NewCard(i64),
    Title(i64),
    Body(i64),
    Reply(i64),
    NewLane,
    RenameLane(i64),
}

pub struct Input {
    pub what: Editing,
    pub field: TextField,
}

/// A card being dragged.
#[derive(Clone)]
pub struct DragCard {
    pub id: i64,
    pub title: String,
}

/// The list of agents to give a card to; the row after the last is "nobody".
pub struct Picker {
    pub card: i64,
    /// Handoff id and label of every running agent
    pub agents: Vec<(String, String)>,
    pub cursor: usize,
}

pub struct BoardState {
    pub project: Project,
    pub board: Board,
    /// the cursor: a lane, and a card in it
    pub lane: usize,
    pub card: usize,
    pub input: Option<Input>,
    /// the card that's open, over the lanes
    pub detail: Option<CardDetail>,
    /// waiting for a second `d` before deleting this card
    pub deleting: Option<i64>,
    /// The agent in the pane the board was opened from: cards you add to an
    /// agent's lane are its, unless you say otherwise (`a`).
    pub from_agent: Option<String>,
    /// The tag for cards you add: the repository of that pane.
    pub tag: Option<String>,
    /// The shortcut sheet (`?`) is up, with what's typed to filter it.
    pub help: Option<String>,
    /// Choosing whose card this is (`a`).
    pub picking: Option<Picker>,
    /// per lane id
    pub scrolls: HashMap<i64, ScrollHandle>,
    pub detail_scroll: ScrollHandle,
    version: i64,
}

impl BoardState {
    fn open(slug: &str) -> anyhow::Result<Self> {
        let (project, board, version) = with_store(|s| {
            let project = s.ensure_project(slug)?;
            let board = s.board(&project)?;
            Ok((project, board, s.data_version()?))
        })?;
        let mut state = Self {
            project,
            board,
            lane: 0,
            card: 0,
            input: None,
            detail: None,
            deleting: None,
            from_agent: None,
            tag: None,
            picking: None,
            help: None,
            scrolls: HashMap::new(),
            detail_scroll: ScrollHandle::new(),
            version,
        };
        // start where you're needed
        state.lane = state
            .board
            .lanes
            .iter()
            .position(|l| l.lane.role == Role::You && !l.cards.is_empty())
            .unwrap_or(0);
        state.sync_scrolls();
        Ok(state)
    }

    fn sync_scrolls(&mut self) {
        for lane in &self.board.lanes {
            self.scrolls.entry(lane.lane.id).or_insert_with(ScrollHandle::new);
        }
    }

    pub fn cards(&self) -> &[Card] {
        self.board.lanes.get(self.lane).map(|l| l.cards.as_slice()).unwrap_or(&[])
    }

    pub fn selected(&self) -> Option<&Card> {
        self.cards().get(self.card)
    }

    fn lane_id(&self) -> Option<i64> {
        self.board.lanes.get(self.lane).map(|l| l.lane.id)
    }

    /// Put the cursor on a card, wherever it is now.
    pub fn select(&mut self, id: i64) -> bool {
        for (li, lane) in self.board.lanes.iter().enumerate() {
            if let Some(ci) = lane.cards.iter().position(|c| c.id == id) {
                (self.lane, self.card) = (li, ci);
                self.reveal();
                return true;
            }
        }
        false
    }

    fn reveal(&self) {
        if let Some(scroll) = self.lane_id().and_then(|id| self.scrolls.get(&id)) {
            scroll.scroll_to_item(self.card);
        }
    }

    /// Re-read the board, keeping the cursor on its card (or its place).
    fn reload(&mut self) -> anyhow::Result<()> {
        let keep = self.selected().map(|c| c.id);
        let lane = self.lane_id();
        let detail = self.detail.as_ref().map(|d| d.card.id);
        let (board, detail, version) = with_store(|s| {
            // the open card may have been deleted from under us
            let detail = detail.and_then(|id| s.card(id).ok());
            Ok((s.board(&self.project)?, detail, s.data_version()?))
        })?;
        self.board = board;
        self.detail = detail;
        self.version = version;
        self.sync_scrolls();
        if !keep.is_some_and(|id| self.select(id)) {
            let lanes = self.board.lanes.len();
            self.lane = lane
                .and_then(|id| self.board.lanes.iter().position(|l| l.lane.id == id))
                .unwrap_or(self.lane.min(lanes.saturating_sub(1)));
            self.card = self.card.min(self.cards().len().saturating_sub(1));
        }
        if self.deleting.is_some() && self.deleting != self.selected().map(|c| c.id) {
            self.deleting = None;
        }
        Ok(())
    }

    /// Whether another process wrote since we last read.
    fn stale(&self) -> bool {
        with_store(|s| s.data_version()).is_ok_and(|v| v != self.version)
    }

    fn edit(&mut self, what: Editing, text: &str) {
        self.deleting = None;
        self.input = Some(Input { what, field: TextField::new(text) });
    }

    /// Apply the text box.
    fn submit(&mut self) -> anyhow::Result<()> {
        let Some(Input { what, field }) = self.input.take() else { return Ok(()) };
        let text = field.into_text();
        let text = text.trim();
        let you = Actor::You;
        let project = self.project.id;
        let mut select = None;
        let mut pick = None;
        with_store(|s| {
            match what {
                Editing::NewCard(lane) if !text.is_empty() => {
                    let (title, body) = text.split_once('\n').unwrap_or((text, ""));
                    let card = s.add_card(project, Some(lane), title, body, you)?;
                    s.tag_card(card.id, self.tag.as_deref())?;
                    // work for an agent needs one: the agent you came from, or ask
                    if s.lane(lane)?.role == Role::Agent {
                        match self.from_agent.as_deref() {
                            Some(agent) => drop(s.assign(card.id, Some(agent), you)?),
                            None => pick = Some(card.id),
                        }
                    }
                    select = Some(card.id);
                }
                Editing::Title(id) if !text.is_empty() => {
                    s.update_card(id, CardPatch { title: Some(text.into()), body: None }, you)?;
                }
                Editing::Body(id) => {
                    s.update_card(id, CardPatch { title: None, body: Some(text.into()) }, you)?;
                }
                Editing::Reply(id) if !text.is_empty() => {
                    s.reply(id, text, you)?;
                }
                Editing::NewLane if !text.is_empty() => {
                    s.add_lane(project, text, Role::None, you)?;
                }
                Editing::RenameLane(id) if !text.is_empty() => {
                    s.update_lane(id, LanePatch { name: Some(text.into()), ..Default::default() }, you)?;
                }
                _ => {}
            }
            Ok(())
        })?;
        self.reload()?;
        if let Some(id) = select {
            self.select(id);
        }
        if let Some(id) = pick {
            self.pick_agent(id);
        }
        if what == Editing::NewLane {
            self.lane = self.board.lanes.len().saturating_sub(1);
            self.card = 0;
        }
        if matches!(what, Editing::Reply(_)) {
            self.detail_scroll.scroll_to_bottom();
        }
        Ok(())
    }

    pub fn open_detail(&mut self, id: i64) -> anyhow::Result<()> {
        self.detail = Some(with_store(|s| s.card(id))?);
        self.detail_scroll = ScrollHandle::new();
        Ok(())
    }

    /// Move the card under the cursor a lane left or right, staying on it.
    fn shift_card(&mut self, right: bool) -> anyhow::Result<()> {
        let Some(id) = self.selected().map(|c| c.id) else { return Ok(()) };
        let to = if right { self.lane + 1 } else { self.lane.wrapping_sub(1) };
        let Some(lane) = self.board.lanes.get(to).map(|l| l.lane.id) else { return Ok(()) };
        self.move_card(id, lane, None)
    }

    /// Move the card under the cursor up or down its lane.
    fn nudge_card(&mut self, down: bool) -> anyhow::Result<()> {
        let (Some(id), Some(lane)) = (self.selected().map(|c| c.id), self.lane_id()) else {
            return Ok(());
        };
        let to = if down { self.card + 1 } else { self.card.wrapping_sub(1) };
        if to >= self.cards().len() {
            return Ok(());
        }
        self.move_card(id, lane, Some(to))
    }

    pub fn move_card(&mut self, id: i64, lane: i64, index: Option<usize>) -> anyhow::Result<()> {
        with_store(|s| s.move_card(id, lane, index, Actor::You))?;
        self.reload()?;
        self.select(id);
        Ok(())
    }

    /// Mark a card done; the cursor stays in place, on the next card.
    pub fn complete(&mut self, id: i64) -> anyhow::Result<()> {
        with_store(|s| s.complete_card(id, None, Actor::You))?;
        if self.selected().is_some_and(|c| c.id == id) {
            // don't follow it into Done
            let (lane, card) = (self.lane, self.card);
            self.reload()?;
            self.lane = lane.min(self.board.lanes.len().saturating_sub(1));
            self.card = card.min(self.cards().len().saturating_sub(1));
            return Ok(());
        }
        self.reload()
    }

    /// Answer the pending delete.
    pub fn confirm_delete(&mut self, yes: bool) -> anyhow::Result<()> {
        match self.deleting.take() {
            Some(id) if yes => self.delete(id),
            _ => Ok(()),
        }
    }

    fn delete(&mut self, id: i64) -> anyhow::Result<()> {
        with_store(|s| s.delete_card(id, Actor::You))?;
        self.deleting = None;
        if self.detail.as_ref().is_some_and(|d| d.card.id == id) {
            self.detail = None;
        }
        self.reload()
    }

    /// Open the list of agents for a card, on its current owner.
    fn pick_agent(&mut self, card: i64) {
        let agents = running_agents();
        let owner = self.board.lanes.iter().flat_map(|l| &l.cards).find(|c| c.id == card).and_then(|c| c.agent.clone());
        let cursor = owner.and_then(|o| agents.iter().position(|(id, _)| *id == o)).unwrap_or(0);
        self.picking = Some(Picker { card, agents, cursor });
    }

    /// Keys in the agent list. Always consumes the key.
    fn picker_key(&mut self, key: &str) -> anyhow::Result<()> {
        let Some(p) = self.picking.as_mut() else { return Ok(()) };
        // one row per agent, then "nobody"
        let rows = p.agents.len() + 1;
        let choice = match key {
            "j" | "down" => {
                p.cursor = (p.cursor + 1).min(rows - 1);
                return Ok(());
            }
            "k" | "up" => {
                p.cursor = p.cursor.saturating_sub(1);
                return Ok(());
            }
            "enter" | "a" => p.cursor,
            k if k.len() == 1 && k.as_bytes()[0].is_ascii_digit() && k != "0" => {
                let n = (k.as_bytes()[0] - b'1') as usize;
                if n >= rows {
                    return Ok(());
                }
                n
            }
            _ => {
                self.picking = None;
                return Ok(());
            }
        };
        self.assign_row(choice)
    }

    /// Give the picker's card to the agent on `row` (past the last: nobody).
    pub fn assign_row(&mut self, row: usize) -> anyhow::Result<()> {
        let Some(p) = self.picking.take() else { return Ok(()) };
        let agent = p.agents.get(row).map(|(id, _)| id.as_str());
        with_store(|s| s.assign(p.card, agent, Actor::You))?;
        self.reload()
    }

    fn lane_edit(&mut self, patch: impl FnOnce(usize, Role) -> LanePatch) -> anyhow::Result<()> {
        let Some(lane) = self.board.lanes.get(self.lane).map(|l| l.lane.clone()) else {
            return Ok(());
        };
        let patch = patch(self.lane, lane.role);
        let to = patch.index;
        with_store(|s| s.update_lane(lane.id, patch, Actor::You))?;
        self.reload()?;
        if let Some(to) = to {
            self.lane = to.min(self.board.lanes.len().saturating_sub(1));
        }
        Ok(())
    }

    /// Another project's board; `step` walks the ones with cards on them.
    fn switch_project(&mut self, step: isize) -> anyhow::Result<()> {
        let projects = with_store(|s| {
            let mut out = vec![];
            for p in s.projects()? {
                let used = s.board(&p)?.lanes.iter().any(|l| !l.cards.is_empty());
                if used || p.id == self.project.id {
                    out.push(p);
                }
            }
            Ok(out)
        })?;
        let at = projects.iter().position(|p| p.id == self.project.id).unwrap_or(0) as isize;
        let next = &projects[(at + step).rem_euclid(projects.len().max(1) as isize) as usize];
        if next.id != self.project.id {
            let (from_agent, tag) = (self.from_agent.take(), self.tag.take());
            *self = Self::open(&next.slug)?;
            (self.from_agent, self.tag) = (from_agent, tag);
        }
        Ok(())
    }

    /// A key with no text box open. `Ok(true)` closes the board.
    fn key(&mut self, key: &str) -> anyhow::Result<bool> {
        let selected = self.selected().map(|c| (c.id, c.title.clone(), c.body.clone()));
        // a pending delete takes `d` / ↩; anything else backs out of it
        if let Some(id) = self.deleting.take() {
            if matches!(key, "d" | "enter" | "y") {
                self.delete(id)?;
            }
            return Ok(false);
        }
        if self.picking.is_some() {
            self.picker_key(key)?;
            return Ok(false);
        }
        if key == "a" {
            if let Some(id) = self.detail.as_ref().map(|d| d.card.id).or(selected.as_ref().map(|s| s.0)) {
                self.pick_agent(id);
            }
            return Ok(false);
        }
        if let Some(detail) = &self.detail {
            let (id, title, body) = (detail.card.id, detail.card.title.clone(), detail.card.body.clone());
            match key {
                "escape" | "q" | "h" | "left" => self.detail = None,
                "enter" | "r" => self.edit(Editing::Reply(id), ""),
                "e" => self.edit(Editing::Title(id), &title),
                "b" => self.edit(Editing::Body(id), &body),
                "x" | "space" => {
                    self.detail = None;
                    self.complete(id)?;
                }
                "d" | "backspace" => self.deleting = Some(id),
                "j" | "down" => scroll_by(&self.detail_scroll, -40.),
                "k" | "up" => scroll_by(&self.detail_scroll, 40.),
                _ => {}
            }
            return Ok(false);
        }
        let lanes = self.board.lanes.len();
        match key {
            "escape" | "q" => return Ok(true),
            "h" | "left" if self.lane > 0 => {
                self.lane -= 1;
                self.card = self.card.min(self.cards().len().saturating_sub(1));
            }
            "l" | "right" if self.lane + 1 < lanes => {
                self.lane += 1;
                self.card = self.card.min(self.cards().len().saturating_sub(1));
            }
            "j" | "down" if self.card + 1 < self.cards().len() => self.card += 1,
            "k" | "up" => self.card = self.card.saturating_sub(1),
            "g" | "home" => self.card = 0,
            "G" | "end" => self.card = self.cards().len().saturating_sub(1),
            "c" | "n" | "o" => {
                if let Some(lane) = self.lane_id() {
                    self.edit(Editing::NewCard(lane), "");
                    if let Some(scroll) = self.scrolls.get(&lane) {
                        scroll.scroll_to_bottom();
                    }
                }
            }
            "enter" => {
                if let Some((id, ..)) = selected {
                    self.open_detail(id)?;
                }
            }
            "r" => {
                if let Some((id, ..)) = selected {
                    self.open_detail(id)?;
                    self.edit(Editing::Reply(id), "");
                }
            }
            "e" => {
                if let Some((id, title, _)) = selected {
                    self.edit(Editing::Title(id), &title);
                }
            }
            "x" | "space" => {
                if let Some((id, ..)) = selected {
                    self.complete(id)?;
                }
            }
            "d" | "backspace" => self.deleting = selected.map(|(id, ..)| id),
            "H" => self.shift_card(false)?,
            "L" => self.shift_card(true)?,
            "J" => self.nudge_card(true)?,
            "K" => self.nudge_card(false)?,
            "[" => self.switch_project(-1)?,
            "]" => self.switch_project(1)?,
            "N" => self.edit(Editing::NewLane, ""),
            "R" => {
                if let Some(lane) = self.board.lanes.get(self.lane).map(|l| l.lane.clone()) {
                    self.edit(Editing::RenameLane(lane.id), &lane.name);
                }
            }
            // whose move the lane's cards are
            "T" => self.lane_edit(|_, role| LanePatch {
                role: Some(match role {
                    Role::You => Role::Agent,
                    Role::Agent => Role::None,
                    Role::None => Role::Done,
                    Role::Done => Role::You,
                }),
                ..Default::default()
            })?,
            "<" if self.lane > 0 => {
                self.lane_edit(|at, _| LanePatch { index: Some(at - 1), ..Default::default() })?
            }
            ">" if self.lane + 1 < lanes => {
                self.lane_edit(|at, _| LanePatch { index: Some(at + 1), ..Default::default() })?
            }
            "X" => {
                if let Some(lane) = self.lane_id() {
                    with_store(|s| s.delete_lane(lane, Actor::You))?;
                    self.reload()?;
                }
            }
            _ => return Ok(false),
        }
        self.reveal();
        Ok(false)
    }
}

/// Handoff's name for the agent in a pane of ours.
fn agent_id(pane: PaneId) -> Option<String> {
    crate::session::remote_pane_id(pane).map(|id| format!("pane:{id}"))
}

/// What to call an agent on a card.
fn agent_label(a: &crate::agents::Agent) -> String {
    let name = a.name.clone().unwrap_or_else(|| crate::agents::label_for(a.kind).unwrap_or("Agent").to_string());
    if a.project.is_empty() {
        name
    } else {
        format!("{name} · {}", a.project)
    }
}

/// Every running agent: its Handoff id and its label.
pub fn agent_labels() -> HashMap<String, String> {
    crate::agents::collect(None)
        .iter()
        .filter_map(|a| Some((agent_id(a.pane_id)?, agent_label(a))))
        .collect()
}

/// Every running agent, in pane order: its Handoff id and its label.
fn running_agents() -> Vec<(String, String)> {
    crate::agents::collect(None)
        .iter()
        .filter_map(|a| Some((agent_id(a.pane_id)?, agent_label(a))))
        .collect()
}

fn scroll_by(scroll: &ScrollHandle, dy: f32) {
    let mut at = scroll.offset();
    at.y = (at.y + gpui::px(dy)).min(gpui::px(0.));
    scroll.set_offset(at);
}

/// The key as typed: `G` for shift+g, `<` for shift+comma.
fn typed(ks: &Keystroke) -> &str {
    match ks.key_char.as_deref() {
        Some(c) if c.chars().count() == 1 && !c.starts_with(|c: char| c.is_control() || c == ' ') => c,
        _ => ks.key.as_str(),
    }
}

impl TermApp {
    /// The tag for cards made from the pane you're in: its git repository, or
    /// its directory.
    fn board_tag(&self) -> Option<String> {
        let dir = self
            .active_pane()
            .and_then(|p| p.get_current_working_dir(mux::pane::CachePolicy::AllowStale))
            .map(|u| PathBuf::from(u.path()))?;
        Some(handoff::project_for_dir(&dir)).filter(|s| !s.is_empty())
    }

    pub(crate) fn open_board(&mut self, cx: &mut Context<Self>) {
        // one board for everything; `[` `]` reach the others
        self.open_board_named(handoff::cli::DEFAULT_PROJECT, cx)
    }

    /// From the top bar's chip: the board, or when nothing waits on you there,
    /// the first board where something does.
    pub(crate) fn open_board_waiting(&mut self, cx: &mut Context<Self>) {
        let main = handoff::cli::DEFAULT_PROJECT;
        let slug = with_store(|s| {
            let waiting = s.waiting(Actor::You)?;
            let on_main = waiting.iter().any(|(p, _)| p.slug == handoff::slugify(main));
            Ok(if on_main { None } else { waiting.into_iter().next().map(|(p, _)| p.slug) })
        });
        let slug = slug.ok().flatten().unwrap_or_else(|| main.to_string());
        self.open_board_named(&slug, cx)
    }

    fn open_board_named(&mut self, board: &str, cx: &mut Context<Self>) {
        match BoardState::open(board) {
            Ok(mut state) => {
                state.tag = self.board_tag();
                state.from_agent = self.active_pane().and_then(|p| {
                    let here = crate::agents::collect(None).iter().any(|a| a.pane_id == p.pane_id());
                    here.then(|| agent_id(p.pane_id())).flatten()
                });
                self.overlay = Some(Overlay::Board(Box::new(state)));
                self.board_watch(cx);
            }
            Err(e) => self.flash(format!("Board: {e:#}")),
        }
        cx.notify();
    }

    /// Re-read the board when something else changes it, for as long as it's open.
    fn board_watch(&mut self, cx: &mut Context<Self>) {
        if self.board_watching {
            return;
        }
        self.board_watching = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(POLL).await;
            let mut nudged = vec![];
            let open = this.update(cx, |this, cx| {
                let Some(Overlay::Board(state)) = this.overlay.as_mut() else {
                    this.board_watching = false;
                    // closed: what you did goes to the agents it concerns
                    nudged = this.board_nudge();
                    if !nudged.is_empty() {
                        this.flash("Sent to agent");
                        cx.notify();
                    }
                    return false;
                };
                // not under a text box or a drag: the rows would move
                if state.input.is_none() && !cx.has_active_drag() && state.stale() && state.reload().is_ok() {
                    cx.notify();
                }
                true
            });
            if !matches!(open, Ok(true)) {
                // ↩ after the paste has landed, so the agent takes it as a prompt
                if !nudged.is_empty() {
                    cx.background_executor().timer(SUBMIT_DELAY).await;
                    for pane in nudged.into_iter().filter_map(|id| Mux::get().get_pane(id)) {
                        pane.writer().write_all(b"\r").ok();
                    }
                }
                break;
            }
        })
        .detach();
    }

    /// Type what you changed into the sessions of the agents it's for, when
    /// they're idle: a reply to a stopped agent would otherwise wait for its
    /// next session. A working agent is left alone (its Stop hook delivers).
    /// Only a card's owner hears about it. Returns the panes pasted into, still to be submitted.
    fn board_nudge(&mut self) -> Vec<PaneId> {
        let pending = with_store(|s| s.pending()).unwrap_or_default();
        if pending.is_empty() {
            return vec![];
        }
        let mux = Mux::get();
        let agents = crate::agents::collect(None);
        let mut sent = vec![];
        for (project, owner) in pending {
            let target = match owner.as_deref() {
                Some(owner) => {
                    // identities of another kind aren't ours to deliver to
                    let Some(remote) = owner.strip_prefix("pane:").and_then(|n| n.parse::<PaneId>().ok()) else {
                        continue;
                    };
                    match crate::session::local_pane_id(remote).filter(|id| mux.get_pane(*id).is_some()) {
                        Some(local) => local,
                        None => {
                            // its pane is closed: the cards are anyone's now
                            with_store(|s| s.release(owner)).ok();
                            continue;
                        }
                    }
                }
                // a card nobody owns is nobody's news
                None => continue,
            };
            let idle = agents
                .iter()
                .find(|a| a.pane_id == target)
                .is_some_and(|a| matches!(a.status, Status::Done | Status::Idle));
            if !idle || sent.contains(&target) {
                continue;
            }
            let Some(id) = crate::session::remote_pane_id(target).map(|id| format!("pane:{id}")) else {
                continue;
            };
            let Ok(updates) = with_store(|s| s.updates_as(&project, Actor::Agent, Some(&id), false)) else {
                continue;
            };
            if updates.events.is_empty() {
                continue;
            }
            let text = handoff::cli::updates_text(&updates, Actor::Agent);
            if let Some(pane) = mux.get_pane(target) {
                if pane.send_paste(text.trim_end()).is_ok() {
                    sent.push(target);
                }
            }
        }
        sent
    }

    pub(crate) fn board_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let Some(Overlay::Board(state)) = self.overlay.as_mut() else { return };
        if let Some(query) = state.help.as_mut() {
            if crate::shortcuts::sheet_key(query, ks, cx) {
                state.help = None;
            }
            return;
        }
        if state.input.is_none() && typed(ks) == "?" {
            state.help = Some(String::new());
            return;
        }
        let result = if let Some(input) = state.input.as_mut() {
            match input.field.key(ks, cx) {
                Edit::Cancel => {
                    state.input = None;
                    Ok(false)
                }
                Edit::Submit => state.submit().map(|_| false),
                Edit::Changed => Ok(false),
            }
        } else if ks.modifiers.platform || ks.modifiers.control {
            Ok(false)
        } else {
            state.key(typed(ks))
        };
        match result {
            Ok(true) => {
                self.overlay = None;
                self.focus.focus(window);
            }
            Ok(false) => {}
            Err(e) => self.flash(format!("{e:#}")),
        }
    }

    /// Run `f` on the open board; errors become a toast.
    pub(crate) fn with_board(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut BoardState) -> anyhow::Result<()>,
    ) {
        let Some(Overlay::Board(state)) = self.overlay.as_mut() else { return };
        if let Err(e) = f(state) {
            self.flash(format!("{e:#}"));
        }
        cx.notify();
    }

    /// A card dropped on a lane (`index` `None`: its end) or on another card.
    pub(crate) fn board_drop(&mut self, card: i64, lane: i64, index: Option<usize>, cx: &mut Context<Self>) {
        self.with_board(cx, |s| {
            // dropped below itself in its own lane: the slot it leaves shifts the rest up
            let from = s.board.lanes.iter().find(|l| l.lane.id == lane).and_then(|l| l.cards.iter().position(|c| c.id == card));
            let index = match (from, index) {
                (Some(from), Some(to)) if from < to => Some(to - 1),
                (_, to) => to,
            };
            s.move_card(card, lane, index)
        });
    }

    pub(crate) fn board_click(&mut self, card: i64, open: bool, cx: &mut Context<Self>) {
        self.with_board(cx, |s| {
            s.input = None;
            s.deleting = None;
            let already = s.selected().is_some_and(|c| c.id == card);
            s.select(card);
            if open || already {
                s.open_detail(card)?;
            }
            Ok(())
        });
    }
}

// -- wiring agents up --------------------------------------------------------

const HOOKS: [(&str, &str); 2] = [("SessionStart", "session-start"), ("Stop", "stop")];

/// The `handoff` command: a link to this binary in ~/.local/share/wezzy/bin,
/// which is on every pane's PATH (`main::install_cli`).
pub fn link_path() -> PathBuf {
    crate::paths::data_dir().join("bin/handoff")
}

/// Whether we were started as `handoff` (through that link).
pub fn is_handoff(argv0: &Path) -> bool {
    argv0.file_name().is_some_and(|n| n == "handoff")
}

/// ~/.local/bin/handoff, when that directory is on PATH and the name is free
/// or already ours: never someone else's `handoff`.
fn user_link() -> Option<PathBuf> {
    let dir = config::HOME_DIR.join(".local/bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    if !std::env::split_paths(&path).any(|p| p == dir) {
        return None;
    }
    let link = dir.join("handoff");
    let ours = match std::fs::symlink_metadata(&link) {
        Err(_) => true,
        Ok(m) => m.file_type().is_symlink() && std::fs::read_link(&link).is_ok_and(|t| t == link_path()),
    };
    ours.then_some(link)
}

fn claude_settings() -> PathBuf {
    config::HOME_DIR.join(".claude/settings.json")
}

fn hook_command(arg: &str) -> String {
    format!("{} hook {arg}", link_path().display())
}

fn claude() -> std::process::Command {
    let mut c = std::process::Command::new("claude");
    c.stdin(std::process::Stdio::null());
    c
}

pub fn installed() -> bool {
    link_path().exists()
        && std::fs::read_to_string(claude_settings())
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .is_some_and(|root| HOOKS.iter().all(|(ev, arg)| crate::hooks::has_command(&root["hooks"][ev], &hook_command(arg))))
}

/// Link `handoff`, add its hooks to Claude Code's settings and register its
/// MCP server (through `claude mcp`, which owns that file).
pub fn install() -> anyhow::Result<()> {
    let link = link_path();
    let exe = std::env::current_exe()?;
    if std::fs::read_link(&link).ok().as_deref() != Some(exe.as_path()) {
        if let Some(dir) = link.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::remove_file(&link).ok();
        std::os::unix::fs::symlink(&exe, &link)?;
    }
    // panes of a session server started before this keep its PATH, and other
    // terminals never had ours: link it where `wezzy` is linked too
    if let Some(user) = user_link() {
        std::fs::remove_file(&user).ok();
        std::os::unix::fs::symlink(&link, &user)?;
    }
    crate::hooks::edit_json(&claude_settings(), |root| {
        for (ev, arg) in HOOKS {
            crate::hooks::add_hook(root, ev, &hook_command(arg), 10)?;
        }
        Ok(())
    })?;
    // replace an older registration (another path) rather than fail on it
    claude().args(["mcp", "remove", "--scope", "user", "handoff"]).output().ok();
    let out = claude()
        .args(["mcp", "add", "--scope", "user", "handoff", "--"])
        .arg(&link)
        .arg("mcp")
        .output()
        .map_err(|e| anyhow::anyhow!("running `claude mcp add`: {e}"))?;
    if !out.status.success() {
        anyhow::bail!("claude mcp add: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Take all of that out again. The board's data stays.
pub fn uninstall() -> anyhow::Result<()> {
    crate::hooks::edit_json(&claude_settings(), |root| {
        for (ev, arg) in HOOKS {
            crate::hooks::remove_hook(root, ev, &hook_command(arg));
        }
        Ok(())
    })?;
    claude().args(["mcp", "remove", "--scope", "user", "handoff"]).output().ok();
    if let Some(user) = user_link() {
        std::fs::remove_file(user).ok();
    }
    std::fs::remove_file(link_path()).ok();
    Ok(())
}
