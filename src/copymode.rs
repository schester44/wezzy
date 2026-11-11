//! Copy mode (Leader y): move a cursor through the pane's scrollback with vim
//! keys, select, and copy. Nothing reaches the pane while it's on, and the
//! view is pinned so new output doesn't scroll the text away.
//!
//! hjkl / arrows, w b e, 0 ^ $, gg G, H M L, ⌃U ⌃D ⌃B ⌃F move. v selects by
//! character, V by line. y or ↵ copies the selection and leaves; with nothing
//! selected, y copies the cursor's line. / and ? search down and up (smart
//! case), n and N repeat. esc clears the selection, then leaves; q leaves.

use crate::app::TermApp;
use gpui::{ClipboardItem, Context, KeyDownEvent};
use mux::pane::{Pane, PaneId};
use std::collections::HashMap;
use std::sync::Arc;
use wezterm_term::StableRowIndex;

pub type Pos = (StableRowIndex, usize);

pub struct CopyMode {
    pub pane_id: PaneId,
    pub cursor: Pos,
    /// where the selection started; None when nothing is selected
    pub anchor: Option<Pos>,
    /// V: whole lines
    pub line_wise: bool,
    /// being typed: (query, forward)
    pub search: Option<(String, bool)>,
    last_search: Option<(String, bool)>,
    /// the column j/k try to keep
    want_col: usize,
    pending_g: bool,
}

impl CopyMode {
    /// The selection as (start, end) in reading order, inclusive.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        let (s, e) = if a <= self.cursor { (a, self.cursor) } else { (self.cursor, a) };
        Some(if self.line_wise {
            ((s.0, 0), (e.0, usize::MAX))
        } else {
            (s, e)
        })
    }
}

/// Rows of a pane as cells, fetched once per keystroke.
struct Text<'a> {
    pane: &'a Arc<dyn Pane>,
    cols: usize,
    top: StableRowIndex,
    bottom: StableRowIndex,
    rows: HashMap<StableRowIndex, (Vec<String>, bool)>,
}

impl<'a> Text<'a> {
    fn new(pane: &'a Arc<dyn Pane>) -> Self {
        let d = pane.get_dimensions();
        Self {
            pane,
            cols: d.cols.max(1),
            top: d.scrollback_top,
            bottom: d.physical_top + d.viewport_rows as StableRowIndex - 1,
            rows: HashMap::new(),
        }
    }

    /// (cells, wrapped onto the next row)
    fn row(&mut self, row: StableRowIndex) -> &(Vec<String>, bool) {
        let (pane, cols) = (self.pane, self.cols);
        self.rows.entry(row).or_insert_with(|| {
            let mut cells = vec![" ".to_string(); cols];
            let mut wrapped = false;
            if let Some(line) = pane.get_lines(row..row + 1).1.into_iter().next() {
                for c in line.visible_cells() {
                    if c.cell_index() < cols {
                        cells[c.cell_index()] = c.str().to_string();
                    }
                }
                wrapped = line.last_cell_was_wrapped();
            }
            (cells, wrapped)
        })
    }

    fn class(&mut self, (row, col): Pos) -> u8 {
        let cells = &self.row(row).0;
        match cells.get(col).and_then(|s| s.chars().next()) {
            None => 0,
            Some(c) if c.is_whitespace() => 0,
            Some(c) if c.is_alphanumeric() || c == '_' => 1,
            Some(_) => 2,
        }
    }

    fn next(&self, (row, col): Pos) -> Option<Pos> {
        if col + 1 < self.cols {
            Some((row, col + 1))
        } else if row < self.bottom {
            Some((row + 1, 0))
        } else {
            None
        }
    }

    fn prev(&self, (row, col): Pos) -> Option<Pos> {
        if col > 0 {
            Some((row, col - 1))
        } else if row > self.top {
            Some((row - 1, self.cols - 1))
        } else {
            None
        }
    }

    /// Last non-blank column, or 0.
    fn line_end(&mut self, row: StableRowIndex) -> usize {
        let cells = &self.row(row).0;
        cells.iter().rposition(|s| !s.trim().is_empty()).unwrap_or(0)
    }

    fn first_non_blank(&mut self, row: StableRowIndex) -> usize {
        let cells = &self.row(row).0;
        cells.iter().position(|s| !s.trim().is_empty()).unwrap_or(0)
    }

    fn word_forward(&mut self, mut p: Pos) -> Pos {
        let start = self.class(p);
        while let Some(n) = self.next(p) {
            // a new row starts a new word, like vim's line break
            let broke = n.0 != p.0;
            p = n;
            if (broke || self.class(p) != start) && self.class(p) != 0 {
                return p;
            }
            if self.class(p) == 0 {
                break;
            }
        }
        while self.class(p) == 0 {
            match self.next(p) {
                Some(n) => p = n,
                None => break,
            }
        }
        p
    }

    fn word_back(&mut self, mut p: Pos) -> Pos {
        while let Some(n) = self.prev(p) {
            p = n;
            if self.class(p) != 0 {
                break;
            }
        }
        let c = self.class(p);
        while let Some(n) = self.prev(p) {
            if n.0 != p.0 || self.class(n) != c {
                break;
            }
            p = n;
        }
        p
    }

    fn word_end(&mut self, mut p: Pos) -> Pos {
        if let Some(n) = self.next(p) {
            p = n;
        }
        while self.class(p) == 0 {
            match self.next(p) {
                Some(n) => p = n,
                None => return p,
            }
        }
        let c = self.class(p);
        while let Some(n) = self.next(p) {
            if n.0 != p.0 || self.class(n) != c {
                break;
            }
            p = n;
        }
        p
    }

    /// The selected text: rows joined by newlines except where a row wrapped,
    /// trailing blanks trimmed.
    fn text(&mut self, (s, e): (Pos, Pos)) -> String {
        let mut out = String::new();
        for row in s.0..=e.0 {
            let (cells, wrapped) = self.row(row).clone();
            let from = if row == s.0 { s.1 } else { 0 };
            let to = if row == e.0 { e.1.min(cells.len() - 1) } else { cells.len() - 1 };
            let piece: String = cells[from.min(to)..=to].concat();
            if wrapped && row != e.0 {
                out.push_str(&piece);
            } else {
                out.push_str(piece.trim_end());
                if row != e.0 {
                    out.push('\n');
                }
            }
        }
        out
    }

    /// Next match of `query` after (or before) `from`, wrapping around.
    fn find(&mut self, query: &str, forward: bool, from: Pos) -> Option<Pos> {
        let case = query.chars().any(char::is_uppercase);
        let norm = |s: &str| if case { s.to_string() } else { s.to_lowercase() };
        let q = norm(query);
        let lines = self.pane.get_lines(self.top..self.bottom + 1).1;
        let rows: Vec<(StableRowIndex, Vec<usize>)> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let (text, cols) = crate::quickselect::row_text(line);
                let hay = norm(&text);
                let hits = hay.match_indices(&q).map(|(b, _)| cols[b]).collect();
                (self.top + i as StableRowIndex, hits)
            })
            .collect();
        let mut all: Vec<Pos> = rows
            .into_iter()
            .flat_map(|(r, hits)| hits.into_iter().map(move |c| (r, c)))
            .collect();
        if forward {
            all.iter().find(|p| **p > from).or(all.first()).copied()
        } else {
            all.reverse();
            all.iter().find(|p| **p < from).or(all.first()).copied()
        }
    }
}

impl TermApp {
    pub(crate) fn start_copy_mode(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else {
            return;
        };
        let d = pane.get_dimensions();
        let top = self.viewports.get(&pane.pane_id()).copied().unwrap_or(d.physical_top);
        let c = pane.get_cursor_position();
        // the terminal cursor if it's on screen, else the bottom of the view
        let bottom = top + d.viewport_rows as StableRowIndex - 1;
        let cursor = if (top..=bottom).contains(&c.y) { (c.y, c.x) } else { (bottom, 0) };
        // pin the view: new output shouldn't move the text under the cursor
        self.viewports.insert(pane.pane_id(), top);
        self.copy_mode = Some(CopyMode {
            pane_id: pane.pane_id(),
            cursor,
            anchor: None,
            line_wise: false,
            search: None,
            last_search: None,
            want_col: cursor.1,
            pending_g: false,
        });
        cx.notify();
    }

    pub(crate) fn copy_mode_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(pane) = self.copy_mode.as_ref().and_then(|c| mux::Mux::get().get_pane(c.pane_id))
        else {
            self.copy_mode = None;
            return;
        };
        let ks = &ev.keystroke;
        let key = ks.key.as_str();
        let ctrl = ks.modifiers.control;
        let ch = ks.key_char.clone().unwrap_or_default();
        let mut text = Text::new(&pane);
        let rows = pane.get_dimensions().viewport_rows as StableRowIndex;
        let view_top = self
            .viewports
            .get(&pane.pane_id())
            .copied()
            .unwrap_or(pane.get_dimensions().physical_top);
        let cm = self.copy_mode.as_mut().unwrap();
        let mut flash: Option<&str> = None;

        // typing a search
        if let Some((query, forward)) = cm.search.as_mut() {
            match key {
                "escape" => cm.search = None,
                "enter" => {
                    let (q, f) = (query.clone(), *forward);
                    cm.search = None;
                    if !q.is_empty() {
                        match text.find(&q, f, cm.cursor) {
                            Some(p) => {
                                cm.cursor = p;
                                cm.want_col = p.1;
                            }
                            None => flash = Some("No match"),
                        }
                        cm.last_search = Some((q, f));
                    }
                }
                "backspace" => {
                    if query.pop().is_none() {
                        cm.search = None;
                    }
                }
                _ if !ctrl && !ks.modifiers.platform && !ch.is_empty() => query.push_str(&ch),
                _ => {}
            }
            if let Some(m) = flash {
                self.flash(m);
            }
            self.copy_mode_follow(cx);
            return;
        }

        // y / ↵ / ⌘C: copy the selection (or the cursor's line) and leave
        if (matches!(key, "y" | "enter") && !ctrl) || (key == "c" && ks.modifiers.platform) {
            let (row, _) = cm.cursor;
            let sel = cm.selection().unwrap_or(((row, 0), (row, usize::MAX)));
            cx.write_to_clipboard(ClipboardItem::new_string(text.text(sel)));
            self.exit_copy_mode();
            self.flash("Copied");
            cx.notify();
            return;
        }

        let g = std::mem::take(&mut cm.pending_g);
        let (row, col) = cm.cursor;
        let mut to: Option<Pos> = None;
        let mut keep_col = false;
        let clamp_row = |r: StableRowIndex| r.clamp(text.top, text.bottom);
        match (key, ctrl) {
            ("h" | "left", false) => to = Some((row, col.saturating_sub(1))),
            ("l" | "right", false) => to = Some((row, (col + 1).min(text.cols - 1))),
            ("j" | "down", false) => {
                to = Some((clamp_row(row + 1), cm.want_col));
                keep_col = true;
            }
            ("k" | "up", false) => {
                to = Some((clamp_row(row - 1), cm.want_col));
                keep_col = true;
            }
            ("d", true) | ("f", true) | ("u", true) | ("b", true) => {
                let n = if matches!(key, "d" | "u") { rows / 2 } else { rows };
                let n = if matches!(key, "u" | "b") { -n } else { n };
                to = Some((clamp_row(row + n), cm.want_col));
                keep_col = true;
            }
            ("w", false) => to = Some(text.word_forward(cm.cursor)),
            ("b", false) => to = Some(text.word_back(cm.cursor)),
            ("e", false) => to = Some(text.word_end(cm.cursor)),
            ("0" | "home", false) => to = Some((row, 0)),
            ("^", false) => to = Some((row, text.first_non_blank(row))),
            ("$" | "end", false) => to = Some((row, text.line_end(row))),
            ("g", false) if g => to = Some((text.top, 0)),
            ("g", false) => cm.pending_g = true,
            ("G", false) => to = Some((text.bottom, 0)),
            ("H", false) => to = Some((clamp_row(view_top), 0)),
            ("M", false) => to = Some((clamp_row(view_top + rows / 2), 0)),
            ("L", false) => to = Some((clamp_row(view_top + rows - 1), 0)),
            ("v", false) => {
                if cm.anchor.is_some() && !cm.line_wise {
                    cm.anchor = None;
                } else {
                    cm.anchor.get_or_insert(cm.cursor);
                    cm.line_wise = false;
                }
            }
            ("V", false) => {
                if cm.anchor.is_some() && cm.line_wise {
                    cm.anchor = None;
                } else {
                    cm.anchor.get_or_insert(cm.cursor);
                    cm.line_wise = true;
                }
            }
            ("/", false) => cm.search = Some((String::new(), true)),
            ("?", false) => cm.search = Some((String::new(), false)),
            ("n" | "N", false) => {
                if let Some((q, f)) = cm.last_search.clone() {
                    let forward = if key == "n" { f } else { !f };
                    match text.find(&q, forward, cm.cursor) {
                        Some(p) => to = Some(p),
                        None => flash = Some("No match"),
                    }
                }
            }
            ("escape", _) | ("[", true) => {
                if cm.anchor.take().is_none() {
                    self.exit_copy_mode();
                }
            }
            ("q", false) => self.exit_copy_mode(),
            _ => {}
        }
        if let Some(m) = flash {
            self.flash(m);
        }
        if let (Some(p), Some(cm)) = (to, self.copy_mode.as_mut()) {
            let p = (p.0, p.1.min(text.cols - 1));
            cm.cursor = p;
            if !keep_col {
                cm.want_col = p.1;
            }
        }
        self.copy_mode_follow(cx);
    }

    fn exit_copy_mode(&mut self) {
        if let Some(cm) = self.copy_mode.take() {
            self.viewports.remove(&cm.pane_id);
        }
    }

    /// Scroll so the copy cursor is on screen.
    fn copy_mode_follow(&mut self, cx: &mut Context<Self>) {
        if let Some(cm) = &self.copy_mode {
            if let Some(pane) = mux::Mux::get().get_pane(cm.pane_id) {
                let d = pane.get_dimensions();
                let rows = d.viewport_rows as StableRowIndex;
                let top = self.viewports.get(&cm.pane_id).copied().unwrap_or(d.physical_top);
                let row = cm.cursor.0;
                let new_top = if row < top {
                    row
                } else if row >= top + rows {
                    row - rows + 1
                } else {
                    top
                };
                // stays pinned even at the bottom; leaving copy mode unpins
                let new_top = new_top.clamp(d.scrollback_top, d.physical_top);
                self.viewports.insert(cm.pane_id, new_top);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cm(anchor: Option<Pos>, cursor: Pos, line_wise: bool) -> CopyMode {
        CopyMode {
            pane_id: 0,
            cursor,
            anchor,
            line_wise,
            search: None,
            last_search: None,
            want_col: 0,
            pending_g: false,
        }
    }

    #[test]
    fn selection_is_ordered() {
        assert_eq!(cm(Some((5, 3)), (2, 7), false).selection(), Some(((2, 7), (5, 3))));
        assert_eq!(cm(Some((2, 7)), (5, 3), false).selection(), Some(((2, 7), (5, 3))));
        assert_eq!(cm(None, (5, 3), false).selection(), None);
    }

    #[test]
    fn line_wise_covers_whole_rows() {
        assert_eq!(
            cm(Some((5, 3)), (2, 7), true).selection(),
            Some(((2, 0), (5, usize::MAX)))
        );
    }
}
