//! Agent overview (⌘O, Leader 0): every agent in every workspace as a live
//! card, so you can approve, deny, reply or interrupt without going there.
//! A window-level mode, not an overlay or a pane view: it replaces the
//! sidebar/terminal row until you close it or jump into an agent. State, keys
//! and actions live here; the view is `ui/overview.rs`.
//!
//! Cards follow `agents::collect` order (workspace, then pane id), the same as
//! the sidebar and Leader 1–9, and never reorder by status.

use crate::agents::{Agent, Status};
use crate::app::{clear_alert, TermApp, UI_STATE};
use crate::keys::{self, Lookup};
use gpui::{Context, KeyDownEvent, Keystroke, ScrollHandle, Window};
use mux::pane::PaneId;
use mux::Mux;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use termwiz::input::KeyCode as TermKey;
use wezterm_input_types::Modifiers as Mods;

/// A prompt you just answered keeps its buttons hidden this long, or until the
/// hook reports the agent moving on, so a second `y` can't land in its input.
const ANSWERED_FOR: Duration = Duration::from_secs(3);

pub struct OverviewState {
    /// The card under the keyboard cursor.
    pub selected: Option<PaneId>,
    /// The inline reply under the selected card: (pane, text).
    pub reply: Option<(PaneId, String)>,
    pub scroll: ScrollHandle,
    /// Scroll the selected card into view on the next render.
    pub reveal: bool,
    /// Cards per row, as last laid out.
    pub cols: usize,
    answered: HashMap<PaneId, Instant>,
}

impl OverviewState {
    fn new(selected: Option<PaneId>) -> Self {
        Self {
            selected,
            reply: None,
            scroll: ScrollHandle::new(),
            reveal: true,
            cols: 1,
            answered: HashMap::new(),
        }
    }

    /// The selected agent's index, falling back to the first card when it's gone.
    pub fn index(&self, agents: &[Agent]) -> usize {
        self.selected
            .and_then(|id| agents.iter().position(|a| a.pane_id == id))
            .unwrap_or(0)
    }

    fn select(&mut self, agents: &[Agent], i: usize) {
        if let Some(a) = agents.get(i) {
            self.selected = Some(a.pane_id);
            self.reveal = true;
        }
    }

    /// A permission prompt the card can answer: not one you just answered.
    pub fn can_answer(&self, a: &Agent) -> bool {
        a.permission
            && keymap(a.kind).allow.is_some()
            && self.answered.get(&a.pane_id).is_none_or(|t| t.elapsed() > ANSWERED_FOR)
    }

    pub fn prune(&mut self) {
        self.answered.retain(|_, t| t.elapsed() <= ANSWERED_FOR);
    }
}

/// A row of the grid: a workspace label, or up to `cols` cards (agent indexes).
#[derive(Debug, PartialEq)]
pub enum Row {
    Workspace(String, usize),
    Cards(Vec<usize>),
}

/// Group agents by workspace (they arrive sorted), each group's cards wrapped
/// at `cols`.
pub fn rows(agents: &[Agent], cols: usize) -> Vec<Row> {
    let workspaces: Vec<&str> = agents.iter().map(|a| a.workspace.as_str()).collect();
    rows_for(&workspaces, cols)
}

fn rows_for(workspaces: &[&str], cols: usize) -> Vec<Row> {
    let cols = cols.max(1);
    let mut out = vec![];
    let mut i = 0;
    while i < workspaces.len() {
        let ws = workspaces[i];
        let end = workspaces[i..].iter().position(|w| *w != ws).map_or(workspaces.len(), |n| i + n);
        out.push(Row::Workspace(ws.to_string(), end - i));
        let idx: Vec<usize> = (i..end).collect();
        out.extend(idx.chunks(cols).map(|c| Row::Cards(c.to_vec())));
        i = end;
    }
    out
}

/// The card row and column holding agent `i`.
fn position(rows: &[Row], i: usize) -> Option<(usize, usize)> {
    rows.iter().enumerate().find_map(|(r, row)| match row {
        Row::Cards(c) => c.iter().position(|x| *x == i).map(|col| (r, col)),
        _ => None,
    })
}

/// The card above (`up`) or below agent `i`, keeping the column where it can.
fn vertical(rows: &[Row], i: usize, up: bool) -> Option<usize> {
    let (r, col) = position(rows, i)?;
    let mut candidates: Box<dyn Iterator<Item = &Row>> = if up {
        Box::new(rows[..r].iter().rev())
    } else {
        Box::new(rows[r + 1..].iter())
    };
    candidates.find_map(|row| match row {
        Row::Cards(c) => c.get(col.min(c.len() - 1)).copied(),
        _ => None,
    })
}

/// The grid row index of agent `i` (for scrolling it into view).
pub fn row_of(rows: &[Row], i: usize) -> Option<usize> {
    position(rows, i).map(|(r, _)| r)
}

/// Agents that want you, for Tab: those waiting on input, else finished ones
/// you haven't seen.
fn wanting(agents: &[Agent]) -> Vec<usize> {
    let needs: Vec<usize> = (0..agents.len())
        .filter(|&i| agents[i].status == Status::NeedsInput)
        .collect();
    if !needs.is_empty() {
        return needs;
    }
    (0..agents.len())
        .filter(|&i| agents[i].status == Status::Done && agents[i].unseen)
        .collect()
}

/// The next (or previous) of `among` after `from`, wrapping.
fn cycle(among: &[usize], from: usize, forward: bool) -> Option<usize> {
    if forward {
        among.iter().copied().find(|&i| i > from).or(among.first().copied())
    } else {
        among.iter().rev().copied().find(|&i| i < from).or(among.last().copied())
    }
}

/// How to drive a harness's prompts with keys.
pub struct Keymap {
    /// Allow / always allow / deny on a permission prompt.
    pub allow: Option<(TermKey, TermKey, TermKey)>,
    pub interrupt: (TermKey, Mods),
}

pub fn keymap(kind: &str) -> Keymap {
    match kind {
        // option 1 is highlighted; 2 is "Yes, and don't ask again"; esc cancels
        "Claude" => Keymap {
            allow: Some((TermKey::Enter, TermKey::Char('2'), TermKey::Escape)),
            interrupt: (TermKey::Escape, Mods::NONE),
        },
        _ => Keymap {
            allow: None,
            interrupt: (TermKey::Char('c'), Mods::CTRL),
        },
    }
}

#[derive(Clone, Copy)]
pub enum Answer {
    Allow,
    Always,
    Deny,
}

/// "40s", "3m", "2h", "4d".
pub fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// The branch checked out at `dir` (short commit when detached), read from
/// the repo's HEAD file. Cached a few seconds per directory.
pub fn branch(dir: &Path) -> Option<String> {
    static CACHE: Mutex<Option<HashMap<PathBuf, (Instant, Option<String>)>>> = Mutex::new(None);
    let mut guard = CACHE.lock();
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some((at, b)) = cache.get(dir) {
        if at.elapsed() < Duration::from_secs(5) {
            return b.clone();
        }
    }
    let b = read_branch(dir);
    cache.insert(dir.to_path_buf(), (Instant::now(), b.clone()));
    b
}

fn read_branch(dir: &Path) -> Option<String> {
    let dot_git = dir.ancestors().map(|d| d.join(".git")).find(|p| p.exists())?;
    // a worktree's .git is a file: "gitdir: <path>"
    let git_dir = if dot_git.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let path = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
        if path.is_absolute() {
            path
        } else {
            dot_git.parent()?.join(path)
        }
    } else {
        dot_git
    };
    parse_head(&std::fs::read_to_string(git_dir.join("HEAD")).ok()?)
}

fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    match head.strip_prefix("ref:") {
        Some(r) => Some(r.trim().trim_start_matches("refs/heads/").to_string()),
        None => (head.len() >= 7).then(|| head[..7].to_string()),
    }
}

impl TermApp {
    pub(crate) fn toggle_overview(&mut self) {
        if self.overview.take().is_none() {
            // start on the agent you're in, if you're in one
            self.overview = Some(OverviewState::new(self.active_pane().map(|p| p.pane_id())));
        }
    }

    /// Close and focus that agent's pane.
    pub(crate) fn overview_jump(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        self.overview = None;
        self.region = crate::regions::Region::Terminal;
        self.focus_pane(pane_id, window, cx);
    }

    pub(crate) fn overview_select(&mut self, pane_id: PaneId) {
        if let Some(o) = self.overview.as_mut() {
            o.selected = Some(pane_id);
        }
    }

    /// Select the next agent matching `pred` after the selected one (header counts).
    pub(crate) fn overview_cycle(&mut self, pred: impl Fn(&Agent) -> bool) {
        let agents = crate::agents::collect(None);
        let Some(o) = self.overview.as_mut() else { return };
        let among: Vec<usize> = (0..agents.len()).filter(|&i| pred(&agents[i])).collect();
        if let Some(i) = cycle(&among, o.index(&agents), true) {
            o.select(&agents, i);
        }
    }

    pub(crate) fn overview_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let agents = crate::agents::collect(None);
        if self.overview.as_ref().is_some_and(|o| o.reply.is_some()) {
            self.overview_reply_key(ks, cx);
            cx.notify();
            return;
        }
        let plain = !self.keys.leader_active() && !ks.modifiers.platform;
        if plain && self.overview_nav(ks, &agents, window, cx) {
            cx.notify();
            return;
        }
        // ⌘ chords and the leader still work; nothing reaches a pane
        if let Some(input) = keys::translate(ks) {
            match self.keys.lookup(&input.key, input.mods) {
                Lookup::Action(action) => self.perform(action, None, window, cx),
                Lookup::Agent(i) => match agents.get(i) {
                    Some(a) => self.overview_jump(a.pane_id, window, cx),
                    None => self.flash(format!("No agent {}", i + 1)),
                },
                Lookup::LeaderArmed | Lookup::Swallow | Lookup::Unbound => {}
            }
        }
        let mut ui = UI_STATE.lock();
        ui.leader_active = self.keys.leader_active();
        ui.active_key_table = self.keys.active_table().map(str::to_string);
        drop(ui);
        cx.notify();
    }

    /// The overview's own keys. False for keys it leaves to the keymap.
    fn overview_nav(&mut self, ks: &Keystroke, agents: &[Agent], window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &ks.modifiers;
        if m.control && ks.key == "c" {
            if let Some(a) = self.overview_selected(agents) {
                self.overview_interrupt(&a);
            }
            return true;
        }
        if m.control || m.alt {
            return false;
        }
        let Some(o) = self.overview.as_mut() else { return false };
        let cur = o.index(agents);
        let rows = rows(agents, o.cols);
        let typed = ks.key_char.as_deref().unwrap_or("");
        match (ks.key.as_str(), typed) {
            ("escape", _) | (_, "q") => self.overview = None,
            ("enter", _) => {
                if let Some(a) = agents.get(cur) {
                    self.overview_jump(a.pane_id, window, cx);
                }
            }
            ("tab", _) => {
                if let Some(i) = cycle(&wanting(agents), cur, !m.shift) {
                    o.select(agents, i);
                }
            }
            ("left", _) | (_, "h") => o.select(agents, cur.saturating_sub(1)),
            ("right", _) | (_, "l") => o.select(agents, (cur + 1).min(agents.len().saturating_sub(1))),
            ("up", _) | (_, "k") => {
                if let Some(i) = vertical(&rows, cur, true) {
                    o.select(agents, i);
                }
            }
            ("down", _) | (_, "j") => {
                if let Some(i) = vertical(&rows, cur, false) {
                    o.select(agents, i);
                }
            }
            (_, "g") => o.select(agents, 0),
            (_, "G") => o.select(agents, agents.len().saturating_sub(1)),
            (_, d @ ("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")) => {
                let i = d.parse::<usize>().unwrap_or(1) - 1;
                if i < agents.len() {
                    o.select(agents, i);
                }
            }
            (_, k @ ("y" | "Y" | "n")) => {
                let answer = match k {
                    "y" => Answer::Allow,
                    "Y" => Answer::Always,
                    _ => Answer::Deny,
                };
                if let Some(a) = agents.get(cur).cloned() {
                    self.overview_answer(&a, answer);
                }
            }
            (_, "r") => {
                if let Some(a) = agents.get(cur) {
                    o.reply = Some((a.pane_id, String::new()));
                }
            }
            // everything else typed is swallowed: the overview isn't a terminal
            _ => {}
        }
        true
    }

    fn overview_selected(&self, agents: &[Agent]) -> Option<Agent> {
        let o = self.overview.as_ref()?;
        agents.get(o.index(agents)).cloned()
    }

    /// `y` / `Y` / `n` (and the card's buttons) on a permission prompt.
    pub(crate) fn overview_answer(&mut self, a: &Agent, answer: Answer) {
        let Some(o) = self.overview.as_mut() else { return };
        let (Some((allow, always, deny)), true) = (keymap(a.kind).allow, o.can_answer(a)) else {
            self.flash("No permission prompt");
            return;
        };
        let Some(pane) = Mux::get().get_pane(a.pane_id) else { return };
        let (key, verb) = match answer {
            Answer::Allow => (allow, "Allowed"),
            Answer::Always => (always, "Always allowed"),
            Answer::Deny => (deny, "Denied"),
        };
        if let Err(e) = pane.key_down(key, Mods::NONE) {
            self.flash(format!("Couldn't send: {e:#}"));
            return;
        }
        o.answered.insert(a.pane_id, Instant::now());
        clear_alert(a.pane_id);
        self.flash(format!("{verb} · {}", a.display_name()));
    }

    /// ⌃C: stop what the agent is doing (Claude takes Esc; ⌃C would quit it).
    fn overview_interrupt(&mut self, a: &Agent) {
        let Some(pane) = Mux::get().get_pane(a.pane_id) else { return };
        let (key, mods) = keymap(a.kind).interrupt;
        match pane.key_down(key, mods) {
            Ok(()) => self.flash(format!("Interrupted · {}", a.display_name())),
            Err(e) => self.flash(format!("Couldn't send: {e:#}")),
        }
    }

    /// The inline reply: enter sends, shift+enter adds a line, esc cancels.
    fn overview_reply_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let m = &ks.modifiers;
        let Some(o) = self.overview.as_mut() else { return };
        let Some((pane_id, input)) = o.reply.as_mut() else { return };
        match ks.key.as_str() {
            "escape" => o.reply = None,
            "[" if m.control => o.reply = None,
            "enter" if m.shift || m.alt => input.push('\n'),
            "enter" => {
                let (pane_id, text) = (*pane_id, std::mem::take(input));
                o.reply = None;
                self.overview_send(pane_id, text, cx);
            }
            "backspace" if m.alt || m.platform => input.clear(),
            "backspace" => {
                input.pop();
            }
            "v" if m.platform => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    input.push_str(&t);
                }
            }
            _ if !m.control && !m.platform => {
                if let Some(c) = &ks.key_char {
                    input.push_str(c);
                }
            }
            _ => {}
        }
    }

    /// Paste the reply into the agent's prompt and submit it.
    fn overview_send(&mut self, pane_id: PaneId, text: String, cx: &mut Context<Self>) {
        let text = text.trim_end().to_string();
        if text.is_empty() {
            return;
        }
        let Some(pane) = Mux::get().get_pane(pane_id) else { return };
        if let Err(e) = pane.send_paste(&text) {
            self.flash(format!("Couldn't send: {e:#}"));
            return;
        }
        clear_alert(pane_id);
        // a beat after the paste, so the agent doesn't take enter as part of it
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(Duration::from_millis(80)).await;
            pane.key_down(TermKey::Enter, Mods::NONE).ok();
        })
        .detach();
        let name = crate::agents::collect(None)
            .into_iter()
            .find(|a| a.pane_id == pane_id)
            .map(|a| a.display_name().to_string())
            .unwrap_or_default();
        self.flash(format!("Sent · {name}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_wraps() {
        let rows = rows_for(&["api", "api", "api", "web"], 2);
        assert_eq!(
            rows,
            vec![
                Row::Workspace("api".into(), 3),
                Row::Cards(vec![0, 1]),
                Row::Cards(vec![2]),
                Row::Workspace("web".into(), 1),
                Row::Cards(vec![3]),
            ]
        );
        // down from the second column lands on the shorter row's last card
        assert_eq!(vertical(&rows, 1, false), Some(2));
        assert_eq!(vertical(&rows, 2, false), Some(3));
        assert_eq!(vertical(&rows, 3, true), Some(2));
        assert_eq!(vertical(&rows, 0, true), None);
        assert_eq!(row_of(&rows, 3), Some(4));
    }

    #[test]
    fn cycles() {
        assert_eq!(cycle(&[1, 4], 1, true), Some(4));
        assert_eq!(cycle(&[1, 4], 4, true), Some(1));
        assert_eq!(cycle(&[1, 4], 1, false), Some(4));
        assert_eq!(cycle(&[], 0, true), None);
    }

    #[test]
    fn heads() {
        assert_eq!(parse_head("ref: refs/heads/fix/parser\n"), Some("fix/parser".into()));
        assert_eq!(parse_head("e5c448f77174f89502ff4e7c90cf5fb28b6e36dd"), Some("e5c448f".into()));
        assert_eq!(parse_head(""), None);
    }
}
