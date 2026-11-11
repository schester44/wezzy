//! Keyboard focus beyond the terminal: the left panel (agents sidebar, or the
//! git panel in its place), reached with Ctrl+h at the terminal's left edge
//! and left with Ctrl+l / esc; vim keys inside.

use crate::app::TermApp;
use gpui::{Context, KeyDownEvent, Window};
use mux::pane::PaneId;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Region {
    Terminal,
    Sidebar,
    Diff,
}

#[derive(Clone, Debug)]
pub enum SidebarItem {
    Agent(PaneId),
    Server { port: u16, pane_id: PaneId },
}

impl TermApp {
    /// The active tab has a zoomed pane: it fills the window, side panels hidden.
    pub(crate) fn pane_zoomed(&self) -> bool {
        self.active_tab().is_some_and(|t| t.get_zoomed_pane().is_some())
    }

    /// Sidebar entries in display order (agents, then servers).
    pub(crate) fn sidebar_items(&self) -> Vec<SidebarItem> {
        let mut items: Vec<SidebarItem> = crate::agents::collect(None)
            .into_iter()
            .map(|a| SidebarItem::Agent(a.pane_id))
            .collect();
        items.extend(
            self.workspace_servers()
                .into_iter()
                .map(|s| SidebarItem::Server {
                    port: s.port,
                    pane_id: s.pane_id,
                }),
        );
        items
    }

    /// Ctrl+h/l at the terminal's outer edge moves into the side panels.
    /// Returns true if the key was used.
    pub(crate) fn edge_nav(&mut self, ks: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let m = &ks.modifiers;
        if !(m.control && !m.alt && !m.platform && !m.shift) {
            return false;
        }
        if self.pane_zoomed() {
            return false;
        }
        let (dir, region, open) = match ks.key.as_str() {
            "h" if self.diff_open => (config::keyassignment::PaneDirection::Left, Region::Diff, true),
            "h" => (config::keyassignment::PaneDirection::Left, Region::Sidebar, self.sidebar_open),
            _ => return false,
        };
        if !open {
            return false;
        }
        let (Some(tab), Some(pane)) = (self.active_tab(), self.active_pane()) else {
            return false;
        };
        // nvim handles its own splits (smart-splits); leave the key to it
        if crate::procs::is_vim(&pane) {
            return false;
        }
        if tab.get_pane_direction(dir, false).is_some() {
            return false;
        }
        self.region = region;
        cx.notify();
        true
    }

    pub(crate) fn region_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        // the commit composer and discard prompt own every key while open
        if self.region == Region::Diff && self.git.composer.is_some() && self.git.comment_draft.is_none() {
            self.composer_key(ks, cx);
            cx.notify();
            return;
        }
        if self.region == Region::Diff
            && (self.git.confirm.is_some()
                || self.git.help
                || self.git.comment_draft.is_some()
                || self.git.visual.is_some())
        {
            self.diff_key(ks, window, cx);
            cx.notify();
            return;
        }
        let back = ks.key == "escape"
            || (m.control && ks.key == "[")
            || (m.control && self.region == Region::Sidebar && ks.key == "l")
            || (m.control && self.region == Region::Diff && ks.key == "l");
        // resize whichever left panel has focus: < / > (vim's ^w< ^w>), or ⌃← / ⌃→
        // when macOS isn't using those for switching Spaces
        let ch = ks.key_char.as_deref();
        let grow = (m.control && ks.key == "right") || (!m.control && !m.platform && ch == Some(">"));
        let shrink = (m.control && ks.key == "left") || (!m.control && !m.platform && ch == Some("<"));
        if !m.alt && !m.platform && (grow || shrink) {
            let step = if grow { 24. } else { -24. };
            let max = f32::from(window.viewport_size().width) * 0.8;
            match self.region {
                Region::Sidebar => self.sidebar_width = (self.sidebar_width + step).clamp(180., max),
                Region::Diff => self.diff_width = (self.diff_width + step).clamp(260., max),
                Region::Terminal => {}
            }
            cx.notify();
            return;
        }
        if back {
            self.region = crate::regions::Region::Terminal;
            cx.notify();
            return;
        }
        match self.region {
            Region::Terminal => {}
            Region::Sidebar => self.sidebar_key(ks, window, cx),
            Region::Diff => self.diff_key(ks, window, cx),
        }
        cx.notify();
    }

    fn sidebar_key(&mut self, ks: &gpui::Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.sidebar_items();
        if items.is_empty() {
            self.region = Region::Terminal;
            return;
        }
        let n = items.len();
        self.sidebar_cursor = self.sidebar_cursor.min(n - 1);
        match ks.key.as_str() {
            "j" | "down" => self.sidebar_cursor = (self.sidebar_cursor + 1).min(n - 1),
            "k" | "up" => self.sidebar_cursor = self.sidebar_cursor.saturating_sub(1),
            "g" if !ks.modifiers.shift => self.sidebar_cursor = 0,
            "g" | "G" => self.sidebar_cursor = n - 1,
            "enter" | "o" => match items[self.sidebar_cursor].clone() {
                SidebarItem::Agent(pane_id) => {
                    self.region = Region::Terminal;
                    self.focus_pane(pane_id, window, cx);
                }
                SidebarItem::Server { port, pane_id } => {
                    if ks.key == "o" {
                        cx.open_url(&format!("http://localhost:{port}"));
                    } else {
                        self.region = Region::Terminal;
                        self.focus_pane(pane_id, window, cx);
                    }
                }
            },
            _ => {}
        }
    }
}
