//! Mouse selection: drag over a pane to select text, double-click for a word,
//! triple-click for a line, shift-click to extend. ⌘C copies it (`CopyTo`).
//! Typing into the pane or clicking without dragging clears it. Dragging past
//! the top or bottom edge scrolls.
//!
//! Positions are stable rows, so the highlight stays on its text as output
//! scrolls by. Text extraction is copy mode's (`copymode::Text`).

use crate::app::TermApp;
use crate::copymode::{Pos, Text};
use gpui::{ClipboardItem, Context, Pixels, Point};
use mux::pane::{Pane, PaneId};
use std::sync::Arc;
use wezterm_term::StableRowIndex;

/// WezTerm's default `selection_word_boundary`.
const WORD_BOUNDARY: &str = " \t\n{[}]()\"'`";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Unit {
    Cell,
    Word,
    Line,
}

pub struct MouseSelection {
    pub pane_id: PaneId,
    unit: Unit,
    /// Cell: the column boundary the drag started at (end exclusive).
    /// Word / Line: the unit first clicked, inclusive.
    anchor: (Pos, Pos),
    /// The unit (or boundary) under the mouse now.
    head: (Pos, Pos),
    pub dragging: bool,
}

impl MouseSelection {
    /// The selection as (start, end) in reading order, inclusive; None when empty.
    pub fn range(&self) -> Option<(Pos, Pos)> {
        if self.unit == Unit::Cell {
            let (a, h) = (self.anchor.0, self.head.0);
            let (s, e) = if a <= h { (a, h) } else { (h, a) };
            if s == e {
                return None;
            }
            // `e` is a boundary: the last selected cell is just before it
            let end = if e.1 > 0 { (e.0, e.1 - 1) } else { (e.0 - 1, usize::MAX) };
            return Some((s, end));
        }
        let s = self.anchor.0.min(self.head.0);
        let e = self.anchor.1.max(self.head.1);
        Some((s, e))
    }
}

/// The word (WezTerm's boundaries) or blank run around `pos` on its row.
fn word_at(text: &mut Text, (row, col): Pos) -> (Pos, Pos) {
    let cells = &text.row(row).0;
    let col = col.min(cells.len().saturating_sub(1));
    let class = |c: usize| {
        let s = cells[c].as_str();
        s.trim().is_empty() || s.chars().any(|ch| WORD_BOUNDARY.contains(ch))
    };
    let boundary = class(col);
    if boundary {
        return ((row, col), (row, col));
    }
    let mut s = col;
    while s > 0 && !class(s - 1) {
        s -= 1;
    }
    let mut e = col;
    while e + 1 < cells.len() && !class(e + 1) {
        e += 1;
    }
    ((row, s), (row, e))
}

fn unit_at(unit: Unit, text: &mut Text, (row, col): Pos, boundary: usize) -> (Pos, Pos) {
    match unit {
        Unit::Cell => ((row, boundary), (row, boundary)),
        Unit::Word => word_at(text, (row, col)),
        Unit::Line => ((row, 0), (row, usize::MAX)),
    }
}

/// Where the mouse is over a pane.
struct Hit {
    pane: Arc<dyn Pane>,
    /// the cell under the mouse
    pos: Pos,
    /// the nearest column boundary
    boundary: usize,
    /// rows above (< 0) or below (> 0) the pane, when dragging past it
    beyond: isize,
}

impl TermApp {
    /// The pane cell under `at`. With `only`, the mouse is clamped into that
    /// pane (a drag that leaves it keeps selecting in it).
    fn selection_hit(&self, at: Point<Pixels>, only: Option<PaneId>) -> Option<Hit> {
        let g = self.grid.get();
        if g.cell_w <= gpui::px(0.) || g.line_h <= gpui::px(0.) {
            return None;
        }
        let x = (at.x - g.origin.x) / g.cell_w;
        let y = ((at.y - g.origin.y) / g.line_h).floor();
        let tab = self.active_tab()?;
        let p = tab.iter_panes().into_iter().find(|p| match only {
            Some(id) => p.pane.pane_id() == id,
            None => {
                x >= 0.
                    && y >= 0.
                    && (x as usize) >= p.left
                    && (x as usize) < p.left + p.width
                    && (y as usize) >= p.top
                    && (y as usize) < p.top + p.height
            }
        })?;
        if crate::views::is_view(&p.pane) {
            return None;
        }
        let col_f = (x - p.left as f32).clamp(0., p.width as f32);
        let row = y as isize - p.top as isize;
        let beyond = if row < 0 {
            row
        } else if row >= p.height as isize {
            row - p.height as isize + 1
        } else {
            0
        };
        let row = row.clamp(0, p.height as isize - 1);
        let d = p.pane.get_dimensions();
        let top = self.viewports.get(&p.pane.pane_id()).copied().unwrap_or(d.physical_top);
        Some(Hit {
            pos: (top + row as StableRowIndex, (col_f as usize).min(p.width - 1)),
            boundary: col_f.round() as usize,
            beyond,
            pane: p.pane.clone(),
        })
    }

    /// Left button down on a pane (no ⌘): start or extend a selection.
    pub(crate) fn start_selection(&mut self, at: Point<Pixels>, clicks: usize, shift: bool) {
        let Some(hit) = self.selection_hit(at, None) else {
            self.selection = None;
            return;
        };
        let pane_id = hit.pane.pane_id();
        let mut text = Text::new(&hit.pane);
        if shift {
            if let Some(sel) = self.selection.as_mut().filter(|s| s.pane_id == pane_id) {
                sel.head = unit_at(sel.unit, &mut text, hit.pos, hit.boundary);
                sel.dragging = true;
                return;
            }
        }
        let unit = match clicks {
            0 | 1 => Unit::Cell,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let anchor = unit_at(unit, &mut text, hit.pos, hit.boundary);
        self.selection = Some(MouseSelection { pane_id, unit, anchor, head: anchor, dragging: true });
    }

    /// Mouse moved with the left button down. Returns false when not selecting.
    pub(crate) fn drag_selection(&mut self, ev: &gpui::MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(pane_id) = self.selection.as_ref().filter(|s| s.dragging).map(|s| s.pane_id) else {
            return false;
        };
        if ev.pressed_button != Some(gpui::MouseButton::Left) {
            self.end_selection();
            cx.notify();
            return true;
        }
        let Some(hit) = self.selection_hit(ev.position, Some(pane_id)) else {
            return true;
        };
        let mut hit = hit;
        if hit.beyond != 0 {
            // past the edge: scroll a line and select up to the newly revealed row
            self.scroll_by(&hit.pane, hit.beyond.signum());
            hit = match self.selection_hit(ev.position, Some(pane_id)) {
                Some(h) => h,
                None => return true,
            };
            if hit.beyond < 0 {
                hit.boundary = 0;
            }
        }
        let mut text = Text::new(&hit.pane);
        if let Some(sel) = self.selection.as_mut() {
            sel.head = unit_at(sel.unit, &mut text, hit.pos, hit.boundary);
        }
        cx.notify();
        true
    }

    /// Left button up: a click that didn't select anything clears the selection.
    pub(crate) fn end_selection(&mut self) {
        if let Some(sel) = self.selection.as_mut() {
            sel.dragging = false;
            if sel.range().is_none() {
                self.selection = None;
            }
        }
    }

    /// ⌘C: copy the selection. Returns false when there isn't one.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((pane_id, range)) = self.selection.as_ref().and_then(|s| Some((s.pane_id, s.range()?)))
        else {
            return false;
        };
        let Some(pane) = mux::Mux::get().get_pane(pane_id) else {
            self.selection = None;
            return false;
        };
        let text = Text::new(&pane).text(range);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(unit: Unit, anchor: (Pos, Pos), head: (Pos, Pos)) -> MouseSelection {
        MouseSelection { pane_id: 0, unit, anchor, head, dragging: false }
    }

    #[test]
    fn cell_selection_is_between_boundaries() {
        let b = |r, c| ((r, c), (r, c));
        assert_eq!(sel(Unit::Cell, b(3, 4), b(3, 4)).range(), None);
        assert_eq!(sel(Unit::Cell, b(3, 4), b(3, 7)).range(), Some(((3, 4), (3, 6))));
        assert_eq!(sel(Unit::Cell, b(3, 7), b(3, 4)).range(), Some(((3, 4), (3, 6))));
        assert_eq!(sel(Unit::Cell, b(2, 5), b(4, 0)).range(), Some(((2, 5), (3, usize::MAX))));
    }

    #[test]
    fn word_selection_extends_by_word() {
        let w = sel(Unit::Word, ((5, 2), (5, 6)), ((3, 0), (3, 4)));
        assert_eq!(w.range(), Some(((3, 0), (5, 6))));
        let w = sel(Unit::Word, ((5, 2), (5, 6)), ((7, 1), (7, 9)));
        assert_eq!(w.range(), Some(((5, 2), (7, 9))));
    }
}
