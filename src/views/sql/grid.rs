//! The result grid: a page of rows with a cursor cell, a row selection, and
//! copying. Only the rows that fit are drawn.

use super::db::{Editable, ResultSet};
use super::export::{self, Format};
use crate::views::editor::Editor;
use crate::render::Metrics;
use crate::ui::{with_alpha, Theme};
use gpui::{div, prelude::*, px, AnyElement, FontWeight, Hsla, Keystroke, Modifiers, Pixels, Point, Size};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const CELL_PAD: f32 = 8.;
const MAX_COL_CHARS: usize = 48;
/// how far from a column's edge a press grabs it
const GRAB: f32 = 4.;

/// What the view should do after a key or click.
pub enum Action {
    /// copy this text, and toast this
    Copy(String, String),
    /// previous (-1) or next (+1) page
    Page(isize),
    /// back to the editor
    Editor,
    /// toast this
    Flash(String),
    /// write the pending changes
    Save,
}

/// A cell being typed into.
struct CellEdit {
    row: usize,
    col: usize,
    editor: Editor,
}

#[derive(Default)]
struct Layout {
    row_h: f32,
    cell_w: f32,
    /// (column, left, right), relative to the grid's left edge
    cols: Vec<(usize, f32, f32)>,
}

#[derive(Default)]
pub struct Grid {
    pub row: usize,
    pub col: usize,
    top: usize,
    left: usize,
    /// rows that fit, from the last draw
    page_rows: usize,
    /// the result these belong to
    seq: u64,
    widths: Vec<usize>,
    /// selected rows (indexes into the page)
    pub selected: BTreeSet<usize>,
    /// `V`: the row a range selection started from, and what was selected before
    anchor: Option<(usize, BTreeSet<usize>)>,
    layout: Layout,
    /// changed cells, (row, column) → new value (None = NULL), not saved yet
    pub pending: BTreeMap<(usize, usize), Option<String>>,
    /// each change, with what the cell's pending value was before it, for `u`
    history: Vec<((usize, usize), Option<Option<String>>)>,
    edit: Option<CellEdit>,
    /// widths that fit each column's content, before any resizing
    fitted: Vec<usize>,
    /// widths you dragged, by column name; kept across queries
    manual: HashMap<String, usize>,
    /// a column edge being dragged: (column, where the drag started, width then)
    resizing: Option<(usize, f32, usize)>,
    /// `c`: the "Copy as" menu, and its highlighted item
    copy_menu: Option<usize>,
}

fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

impl Grid {
    /// Start over for a new result; a new page of the same query keeps the column.
    pub fn sync(&mut self, seq: u64, set: &ResultSet) {
        if self.seq == seq {
            return;
        }
        let fitted = column_widths(set);
        let widths = fitted
            .iter()
            .zip(&set.columns)
            .map(|(w, name)| self.manual.get(name).copied().unwrap_or(*w))
            .collect::<Vec<_>>();
        let same_shape = widths.len() == self.widths.len();
        *self = Grid {
            seq,
            col: if same_shape { self.col } else { 0 },
            left: if same_shape { self.left } else { 0 },
            page_rows: self.page_rows,
            widths,
            fitted,
            manual: std::mem::take(&mut self.manual),
            ..Default::default()
        };
    }

    fn last_row(set: &ResultSet) -> usize {
        set.rows.len().saturating_sub(1)
    }

    fn moved(&mut self) {
        if let Some((from, base)) = &self.anchor {
            let (a, b) = (*from.min(&self.row), *from.max(&self.row));
            let mut s = base.clone();
            s.extend(a..=b);
            self.selected = s;
        }
    }

    /// Rows a copy acts on: the selection, or the cursor's row.
    fn target_rows(&self) -> Vec<usize> {
        if self.selected.is_empty() {
            vec![self.row]
        } else {
            self.selected.iter().copied().collect()
        }
    }

    /// `y`: the cell, or the selected rows. `Y`: rows, with column names.
    pub fn copy(&self, set: &ResultSet, with_header: bool) -> Option<Action> {
        if set.rows.is_empty() {
            return None;
        }
        if !with_header && self.selected.is_empty() {
            let v = self.value(set, self.row, self.col)?;
            return Some(Action::Copy(v.clone().unwrap_or_default(), "Copied".into()));
        }
        let rows = self.target_rows();
        let mut lines = vec![];
        if with_header {
            lines.push(set.columns.join("\t"));
        }
        for r in &rows {
            if set.rows.get(*r).is_some() {
                let cells: Vec<String> = (0..set.columns.len())
                    .map(|c| self.value(set, *r, c).map(export::tsv_value).unwrap_or_default())
                    .collect();
                lines.push(cells.join("\t"));
            }
        }
        Some(Action::Copy(lines.join("\n"), format!("Copied {}", plural(rows.len(), "row"))))
    }

    /// Leaving the grid closes the "Copy as" menu.
    pub fn close_menu(&mut self) {
        self.copy_menu = None;
    }

    /// The target rows in `f`, as `Action::Copy`.
    pub fn copy_as(&self, set: &ResultSet, f: Format, edit: Option<&Editable>) -> Option<Action> {
        let targets: Vec<usize> = self.target_rows().into_iter().filter(|r| *r < set.rows.len()).collect();
        if targets.is_empty() {
            return None;
        }
        let rows = export::Rows {
            columns: &set.columns,
            rows: targets
                .iter()
                .map(|r| (0..set.columns.len()).map(|c| self.value(set, *r, c).cloned().flatten()).collect())
                .collect(),
        };
        let text = export::format(f, &rows, self.col, edit);
        let n = targets.len();
        let toast = match f {
            Format::Column => format!(
                "Copied {} of {}",
                plural(n, "value"),
                set.columns.get(self.col).map_or("", |c| c.as_str())
            ),
            _ => format!("Copied {} as {}", plural(n, "row"), f.label()),
        };
        Some(Action::Copy(text, toast))
    }

    /// Keys while the "Copy as" menu is open: it takes them all.
    fn menu_key(&mut self, ks: &Keystroke, set: &ResultSet, edit: Option<&Editable>) -> Option<Action> {
        let i = self.copy_menu?;
        let last = Format::ALL.len() - 1;
        let pick = match ks.key.as_str() {
            "j" | "down" => {
                self.copy_menu = Some((i + 1).min(last));
                None
            }
            "k" | "up" => {
                self.copy_menu = Some(i.saturating_sub(1));
                None
            }
            "g" if ks.modifiers.shift => {
                self.copy_menu = Some(last);
                None
            }
            "g" => {
                self.copy_menu = Some(0);
                None
            }
            "enter" => Some(Format::ALL[i]),
            k if !ks.modifiers.shift && !ks.modifiers.control && !ks.modifiers.platform => {
                let f = Format::ALL.into_iter().find(|f| f.key() == k);
                if f.is_none() {
                    self.copy_menu = None;
                }
                f
            }
            _ => {
                self.copy_menu = None;
                None
            }
        };
        let f = pick?;
        self.copy_menu = None;
        self.copy_as(set, f, edit)
    }

    /// The column whose right edge is under `pos` in the header.
    fn edge_at(&self, pos: Point<Pixels>) -> Option<usize> {
        let l = &self.layout;
        if l.row_h <= 0. || f32::from(pos.y) >= l.row_h {
            return None;
        }
        let x = f32::from(pos.x);
        l.cols.iter().find(|(_, _, x1)| (x - x1).abs() <= GRAB).map(|(c, _, _)| *c)
    }

    /// A press on a header edge starts resizing that column; a double-click
    /// fits it to its content. `true` if the press was on an edge.
    pub fn header_down(&mut self, pos: Point<Pixels>, clicks: usize, set: &ResultSet) -> bool {
        let Some(c) = self.edge_at(pos) else { return false };
        if clicks == 2 {
            self.widths[c] = self.fitted[c];
            self.manual.remove(&set.columns[c]);
            self.resizing = None;
        } else {
            self.resizing = Some((c, f32::from(pos.x), self.widths[c]));
        }
        true
    }

    pub fn drag(&mut self, pos: Point<Pixels>, set: &ResultSet) {
        let Some((c, x0, w0)) = self.resizing else { return };
        let cell = self.layout.cell_w.max(1.);
        let w = (w0 as f32 + (f32::from(pos.x) - x0) / cell).round().clamp(3., 400.) as usize;
        if let Some(slot) = self.widths.get_mut(c) {
            *slot = w;
            self.manual.insert(set.columns[c].clone(), w);
        }
    }

    pub fn end_drag(&mut self) {
        self.resizing = None;
    }

    pub fn editing(&self) -> bool {
        self.edit.is_some()
    }

    /// The value shown for a cell: its pending change, or what came back.
    fn value<'a>(&'a self, set: &'a ResultSet, row: usize, col: usize) -> Option<&'a Option<String>> {
        self.pending.get(&(row, col)).or_else(|| set.rows.get(row)?.get(col))
    }

    /// Start typing into the cursor's cell.
    fn begin_edit(&mut self, set: &ResultSet, edit: Option<&Editable>) -> Option<Action> {
        let Some(edit) = edit else {
            return Some(Action::Flash("Only results of a select can be edited".into()));
        };
        if let Err(why) = edit.check(self.col) {
            return Some(Action::Flash(why.into()));
        }
        let text = self.value(set, self.row, self.col)?.clone().unwrap_or_default();
        let mut editor = Editor::new(&text);
        editor.inset = (CELL_PAD, 3.);
        editor.key(&Keystroke::parse("cmd-down").unwrap());
        self.edit = Some(CellEdit {
            row: self.row,
            col: self.col,
            editor,
        });
        None
    }

    fn set_cell(&mut self, set: &ResultSet, row: usize, col: usize, value: Option<String>) {
        let before = self.pending.get(&(row, col)).cloned();
        let original = set.rows.get(row).and_then(|r| r.get(col));
        let unchanged = match &before {
            Some(v) => *v == value,
            None => original == Some(&value),
        };
        if unchanged {
            return;
        }
        self.history.push(((row, col), before));
        if original == Some(&value) {
            self.pending.remove(&(row, col));
        } else {
            self.pending.insert((row, col), value);
        }
    }

    fn commit_edit(&mut self, set: &ResultSet) {
        if let Some(e) = self.edit.take() {
            self.set_cell(set, e.row, e.col, Some(e.editor.text()));
        }
    }

    /// Keys while a cell is being typed into: ↵ keeps it, esc drops it, tab
    /// keeps it and moves on.
    fn edit_key(&mut self, ks: &Keystroke, set: &ResultSet, edit: Option<&Editable>) -> Option<Action> {
        let e = self.edit.as_mut()?;
        match ks.key.as_str() {
            "enter" if !ks.modifiers.platform => self.commit_edit(set),
            "escape" => self.edit = None,
            "tab" => {
                self.commit_edit(set);
                let next = if ks.modifiers.shift {
                    self.col.checked_sub(1)
                } else {
                    (self.col + 1 < set.columns.len()).then_some(self.col + 1)
                };
                if let Some(c) = next {
                    self.col = c;
                    if edit.is_some_and(|ed| ed.check(c).is_ok()) {
                        return self.begin_edit(set, edit);
                    }
                }
            }
            _ => {
                // one line: no newlines from ↵ variants
                if ks.key != "enter" {
                    e.editor.key(ks);
                }
            }
        }
        None
    }

    /// `u`: take back the last change, wherever it was, and go to its cell.
    fn undo(&mut self) {
        let Some(((row, col), before)) = self.history.pop() else { return };
        match before {
            Some(v) => self.pending.insert((row, col), v),
            None => self.pending.remove(&(row, col)),
        };
        self.row = row;
        self.col = col;
    }

    /// Discard: every change, and the history of them.
    pub fn discard(&mut self) {
        self.pending.clear();
        self.history.clear();
    }

    pub fn paste(&mut self, text: &str) -> bool {
        match self.edit.as_mut() {
            Some(e) => {
                e.editor.insert(text.lines().next().unwrap_or(""));
                true
            }
            None => false,
        }
    }

    pub fn select_all(&mut self, set: &ResultSet) {
        self.anchor = None;
        self.selected = (0..set.rows.len()).collect();
    }

    /// `None` if the key isn't a grid key.
    pub fn key(&mut self, ks: &Keystroke, set: &ResultSet, edit: Option<&Editable>) -> Option<Option<Action>> {
        if self.edit.is_some() {
            return Some(self.edit_key(ks, set, edit));
        }
        if self.copy_menu.is_some() {
            return Some(self.menu_key(ks, set, edit));
        }
        let rows = set.rows.len();
        let cols = set.columns.len();
        let half = (self.page_rows / 2).max(1);
        let full = self.page_rows.max(1);
        let m = &ks.modifiers;
        let last = Self::last_row(set);
        match (ks.key.as_str(), m.control) {
            ("j" | "down", false) => self.row = (self.row + 1).min(last),
            ("k" | "up", false) => self.row = self.row.saturating_sub(1),
            ("l" | "right", false) => self.col = (self.col + 1).min(cols.saturating_sub(1)),
            ("h" | "left", false) => self.col = self.col.saturating_sub(1),
            ("d", true) => self.row = (self.row + half).min(last),
            ("u", true) => self.row = self.row.saturating_sub(half),
            ("f", true) => self.row = (self.row + full).min(last),
            ("b", true) => self.row = self.row.saturating_sub(full),
            ("g", false) if m.shift => self.row = last,
            ("g", false) => self.row = 0,
            ("0" | "^", false) => self.col = 0,
            ("$", false) => self.col = cols.saturating_sub(1),
            ("4", false) if m.shift => self.col = cols.saturating_sub(1),
            ("v", false) if m.shift => {
                self.anchor = match self.anchor.take() {
                    Some(_) => None,
                    None => Some((self.row, self.selected.clone())),
                };
                self.moved();
            }
            ("space", false) if rows > 0 => {
                self.anchor = None;
                if !self.selected.remove(&self.row) {
                    self.selected.insert(self.row);
                }
            }
            ("y", false) => return Some(self.copy(set, m.shift)),
            ("c", false) if !m.shift && rows > 0 => self.copy_menu = Some(0),
            ("e" | "enter", false) if !m.shift => return Some(self.begin_edit(set, edit)),
            ("n", false) if m.shift => {
                if let Some(Err(why)) = edit.map(|e| e.check(self.col)) {
                    return Some(Some(Action::Flash(why.into())));
                }
                if edit.is_some() {
                    self.set_cell(set, self.row, self.col, None);
                }
            }
            ("u", false) if m.shift => {
                self.pending.clear();
                self.history.clear();
            }
            ("u", false) => self.undo(),
            ("[", false) => return Some(Some(Action::Page(-1))),
            ("]", false) => return Some(Some(Action::Page(1))),
            ("escape", false) => {
                if self.anchor.is_some() || !self.selected.is_empty() {
                    self.anchor = None;
                    self.selected.clear();
                } else {
                    return Some(Some(Action::Editor));
                }
            }
            ("i", false) => return Some(Some(Action::Editor)),
            _ => return None,
        }
        self.moved();
        Some(None)
    }

    /// A click at `pos`, relative to the grid's top-left.
    pub fn click(
        &mut self,
        pos: Point<Pixels>,
        mods: &Modifiers,
        clicks: usize,
        set: &ResultSet,
        edit: Option<&Editable>,
    ) -> Option<Action> {
        self.copy_menu = None;
        // clicking elsewhere keeps what was typed
        if self.edit.is_some() {
            self.commit_edit(set);
        }
        self.click_cell(pos, mods, set);
        if clicks == 2 && !mods.shift && !mods.platform {
            return self.begin_edit(set, edit);
        }
        None
    }

    fn click_cell(&mut self, pos: Point<Pixels>, mods: &Modifiers, set: &ResultSet) {
        let l = &self.layout;
        if l.row_h <= 0. {
            return;
        }
        let y = f32::from(pos.y) - l.row_h; // below the header
        if y < 0. || set.rows.is_empty() {
            return;
        }
        let row = (self.top + (y / l.row_h) as usize).min(Self::last_row(set));
        let x = f32::from(pos.x);
        if let Some((c, _, _)) = l.cols.iter().find(|(_, a, b)| x >= *a && x < *b) {
            self.col = *c;
        }
        if mods.shift {
            let (a, b) = (self.row.min(row), self.row.max(row));
            self.selected.extend(a..=b);
        } else if mods.platform {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
        } else {
            self.selected.clear();
        }
        self.anchor = None;
        self.row = row;
    }

    /// The wheel: scroll by `lines` (up is positive), keeping the cursor in view.
    pub fn scroll(&mut self, lines: isize, set: &ResultSet) {
        let max_top = set.rows.len().saturating_sub(self.page_rows.max(1));
        self.top = (self.top as isize - lines).clamp(0, max_top as isize) as usize;
        let bottom = self.top + self.page_rows.max(1) - 1;
        self.row = self.row.clamp(self.top, bottom.min(Self::last_row(set)));
        self.moved();
    }

    pub fn render(
        &mut self,
        set: &ResultSet,
        first_row: usize,
        m: &Metrics,
        t: &Theme,
        focused: bool,
        size: Size<Pixels>,
    ) -> AnyElement {
        let row_h = m.line_h + px(6.);
        let page = ((f32::from(size.height - row_h) / f32::from(row_h)).floor() as usize).max(1);
        self.page_rows = page;
        if self.row < self.top {
            self.top = self.row;
        } else if self.row >= self.top + page {
            self.top = self.row + 1 - page;
        }

        // row numbers on the left
        let digits = (first_row + set.rows.len()).max(1).to_string().len();
        let gutter_w = m.cell_w * digits as f32 + px(CELL_PAD * 2.);
        let col_w = |chars: usize| m.cell_w * chars as f32 + px(CELL_PAD * 2.);
        let room = f32::from(size.width - gutter_w);
        // scroll columns so the cursor's fits
        if self.col < self.left {
            self.left = self.col;
        }
        while self.left < self.col
            && (self.left..=self.col).map(|c| f32::from(col_w(self.widths[c]))).sum::<f32>() > room
        {
            self.left += 1;
        }
        let mut shown = vec![];
        let mut x = f32::from(gutter_w);
        for c in self.left..set.columns.len() {
            let w = f32::from(col_w(self.widths[c]));
            shown.push((c, x, x + w));
            x += w;
            if x > f32::from(size.width) {
                break;
            }
        }
        self.layout = Layout {
            row_h: f32::from(row_h),
            cell_w: f32::from(m.cell_w),
            cols: shown.clone(),
        };

        let bar = if focused { t.accent } else { with_alpha(t.fg_subtle, 0.5) };
        let cell = |text: String, chars: usize, color: Hsla| {
            div()
                .h_full()
                .w(col_w(chars))
                .flex_none()
                .flex()
                .items_center()
                .px(px(CELL_PAD))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(color)
                .child(text)
        };
        let gutter = |text: String| {
            div()
                .h_full()
                .w(gutter_w)
                .flex_none()
                .flex()
                .items_center()
                .justify_end()
                .px(px(CELL_PAD))
                .text_size(m.font_size * 0.85)
                .text_color(t.fg_subtle)
                .child(text)
        };
        let resizing = self.resizing.map(|(c, _, _)| c);
        let handles = shown.iter().map(|&(c, _, x1)| {
            div()
                .absolute()
                .top_0()
                .left(px(x1 - GRAB))
                .w(px(GRAB * 2.))
                .h(row_h)
                .cursor(gpui::CursorStyle::ResizeLeftRight)
                .flex()
                .justify_center()
                .child(
                    div()
                        .w(px(1.))
                        .h_full()
                        .when(resizing == Some(c), |d| d.bg(with_alpha(t.accent, 0.6)))
                        .when(resizing != Some(c), |d| d.bg(t.border)),
                )
        });
        let header = div()
            .relative()
            .h(row_h)
            .flex()
            .border_b_1()
            .border_color(t.border)
            .font_weight(FontWeight::MEDIUM)
            .child(gutter(String::new()))
            .children(shown.iter().map(|&(c, _, _)| {
                cell(clip(&set.columns[c], self.widths[c]), self.widths[c], t.fg_muted)
            }))
            .children(handles);
        let end = (self.top + page).min(set.rows.len());
        let rows = (self.top..end).map(|r| {
            let cursor = r == self.row;
            let selected = self.selected.contains(&r);
            let fill = if selected {
                Some(with_alpha(t.fg, 0.08))
            } else if cursor {
                Some(with_alpha(t.fg, 0.04))
            } else {
                None
            };
            div()
                .relative()
                .h(row_h)
                .flex()
                .when_some(fill, |d, f| d.bg(f))
                .when(cursor || selected, |d| {
                    d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(bar))
                })
                .child(gutter((first_row + r + 1).to_string()))
                .children(shown.iter().map(|&(c, _, _)| {
                    let changed = self.pending.contains_key(&(r, c));
                    let (text, color) = match self.value(set, r, c).cloned().flatten() {
                        Some(v) => (clip(&v, self.widths[c]), t.fg),
                        None => ("NULL".to_string(), t.fg_subtle),
                    };
                    cell(text, self.widths[c], color)
                        .when(changed, |d| d.bg(with_alpha(t.warning, 0.16)))
                        .when(cursor && c == self.col, |d| {
                            d.bg(with_alpha(
                                if changed { t.warning } else { t.fg },
                                if focused { 0.24 } else { 0.12 },
                            ))
                        })
                }))
        }).collect::<Vec<_>>();
        // the cell being typed into, drawn over the grid
        let top = self.top;
        let input = self.edit.as_mut().and_then(|e| {
            let &(_, x0, x1) = shown.iter().find(|(c, _, _)| *c == e.col)?;
            if e.row < top || e.row >= top + page {
                return None;
            }
            let y = f32::from(row_h) * (1 + e.row - top) as f32;
            let w = (x1 - x0).max(f32::from(m.cell_w) * 24. + CELL_PAD * 2.);
            let w = w.min(f32::from(size.width) - x0).max(x1 - x0);
            Some(
                div()
                    .absolute()
                    .left(px(x0))
                    .top(px(y))
                    .w(px(w))
                    .h(row_h)
                    .bg(t.elevated)
                    .border_1()
                    .border_color(with_alpha(t.accent, 0.65))
                    .rounded_sm()
                    .child(e.editor.render(m, t, focused, px(w), row_h, |_, _| vec![])),
            )
        });
        let menu = self.copy_menu.filter(|_| focused).map(|sel| {
            // under the cursor cell, or over it when there's no room below
            let (x0, _) = shown
                .iter()
                .find(|(c, _, _)| *c == self.col)
                .map_or((f32::from(gutter_w), 0.), |&(_, a, b)| (a, b));
            let h = f32::from(size.height);
            let menu_h = MENU_ROW * Format::ALL.len() as f32 + 8. + 2.;
            let cell_y = f32::from(row_h) * (1 + self.row.saturating_sub(self.top)) as f32;
            let below = cell_y + f32::from(row_h) + 2.;
            let y = if below + menu_h <= h { below } else { (cell_y - menu_h - 2.).max(0.) };
            let x = x0.min(f32::from(size.width) - MENU_W).max(0.);
            copy_menu(sel, t).absolute().left(px(x)).top(px(y))
        });
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family(m.font.family.clone())
            .text_size(m.font_size)
            .child(header)
            .children(rows)
            .children(input)
            .children(menu)
            .into_any_element()
    }
}

const MENU_W: f32 = 200.;
const MENU_ROW: f32 = 28.;

/// The "Copy as" menu: one row per format, its letter as a key cap.
fn copy_menu(selected: usize, t: &Theme) -> gpui::Div {
    let items = Format::ALL.iter().enumerate().map(|(i, f)| {
        let sel = i == selected;
        div()
            .relative()
            .h(px(MENU_ROW))
            .px(px(12.))
            .flex()
            .items_center()
            .justify_between()
            .when(sel, |d| {
                d.bg(with_alpha(t.fg, 0.07))
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
            })
            .child(div().text_color(if sel { t.fg } else { t.fg_muted }).child(f.label()))
            .child(key_cap(f.key(), t))
    });
    div()
        .w(px(MENU_W))
        .py(px(4.))
        .bg(t.elevated)
        .border_1()
        .border_color(t.border)
        .rounded_lg()
        .shadow_md()
        .overflow_hidden()
        .font_family(".SystemUIFont")
        .text_size(px(13.))
        .children(items)
}

/// A key cap, as `kbd()` in the main view draws them.
fn key_cap(label: &'static str, t: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .min_w(px(18.))
        .h(px(18.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(t.border)
        .bg(t.hover)
        .font_family(crate::ui::term_family())
        .text_size(px(10.))
        .text_color(t.fg_muted)
        .child(label)
}

/// One line of a value, cut to `w` chars.
fn clip(s: &str, w: usize) -> String {
    let one: String = s.chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).collect();
    if one.chars().count() > w {
        one.chars().take(w.saturating_sub(1)).collect::<String>() + "…"
    } else {
        one
    }
}

fn column_widths(set: &ResultSet) -> Vec<usize> {
    (0..set.columns.len())
        .map(|c| {
            let data = set
                .rows
                .iter()
                .take(500)
                .map(|r| r.get(c).and_then(|v| v.as_deref()).map_or(4, |v| v.chars().count()))
                .max()
                .unwrap_or(0);
            data.max(set.columns[c].chars().count()).clamp(4, MAX_COL_CHARS)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> ResultSet {
        ResultSet {
            columns: vec!["id".into(), "email".into()],
            rows: vec![
                vec![Some("1".into()), Some("a@x".into())],
                vec![Some("2".into()), None],
                vec![Some("3".into()), Some("c\td".into())],
            ],
            total: 3,
        }
    }

    fn copied(a: Option<Action>) -> String {
        match a {
            Some(Action::Copy(text, _)) => text,
            _ => panic!("no copy"),
        }
    }

    #[test]
    fn copies_cell_then_selected_rows() {
        let s = set();
        let mut g = Grid::default();
        g.sync(1, &s);
        g.col = 1;
        assert_eq!(copied(g.copy(&s, false)), "a@x");
        g.selected = [0, 2].into_iter().collect();
        assert_eq!(copied(g.copy(&s, false)), "1\ta@x\n3\tc d");
        assert_eq!(copied(g.copy(&s, true)), "id\temail\n1\ta@x\n3\tc d");
    }

    #[test]
    fn range_selection_follows_cursor() {
        let s = set();
        let mut g = Grid::default();
        g.sync(1, &s);
        let key = |k: &str| Keystroke::parse(k).unwrap();
        g.key(&key("shift-v"), &s, None);
        g.key(&key("j"), &s, None);
        g.key(&key("j"), &s, None);
        assert_eq!(g.selected.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
        g.key(&key("k"), &s, None);
        assert_eq!(g.selected.iter().copied().collect::<Vec<_>>(), vec![0, 1]);
    }
}

#[cfg(test)]
mod resize_tests {
    use super::*;

    #[test]
    fn widths_survive_a_rerun_by_name() {
        let set = ResultSet {
            columns: vec!["id".into(), "email".into()],
            rows: vec![vec![Some("1".into()), Some("a@x".into())]],
            total: 1,
        };
        let mut g = Grid::default();
        g.sync(1, &set);
        g.layout.cell_w = 10.;
        g.resizing = Some((1, 100., g.widths[1]));
        g.drag(gpui::point(px(150.), px(5.)), &set);
        assert_eq!(g.widths[1], 10);
        g.sync(2, &set);
        assert_eq!(g.widths[1], 10);
    }
}

#[cfg(test)]
mod undo_tests {
    use super::*;

    #[test]
    fn u_takes_back_the_last_change_anywhere() {
        let set = ResultSet {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Some("1".into()), Some("a".into())],
                vec![Some("2".into()), Some("b".into())],
            ],
            total: 2,
        };
        let mut g = Grid::default();
        g.sync(1, &set);
        g.set_cell(&set, 0, 1, Some("x".into()));
        g.set_cell(&set, 0, 1, Some("y".into()));
        g.set_cell(&set, 1, 1, None);
        g.row = 0;
        g.col = 0;
        let u = Keystroke::parse("u").unwrap();
        g.key(&u, &set, None);
        assert_eq!((g.row, g.col), (1, 1));
        assert!(!g.pending.contains_key(&(1, 1)));
        g.key(&u, &set, None);
        assert_eq!(g.pending.get(&(0, 1)), Some(&Some("x".into())));
        g.key(&u, &set, None);
        assert!(g.pending.is_empty());
    }
}

#[cfg(test)]
mod copy_as_tests {
    use super::*;

    #[test]
    fn menu_copies_selection_with_pending_edits() {
        let set = ResultSet {
            columns: vec!["id".into(), "name".into()],
            rows: vec![
                vec![Some("1".into()), Some("a".into())],
                vec![Some("2".into()), Some("b".into())],
                vec![Some("3".into()), Some("c".into())],
            ],
            total: 3,
        };
        let mut g = Grid::default();
        g.sync(1, &set);
        g.set_cell(&set, 2, 1, None);
        g.selected = [0, 2].into_iter().collect();
        let key = |k: &str| Keystroke::parse(k).unwrap();
        assert!(matches!(g.key(&key("c"), &set, None), Some(None)));
        g.key(&key("j"), &set, None);
        match g.key(&key("enter"), &set, None) {
            Some(Some(Action::Copy(text, toast))) => {
                assert_eq!(text, "id\tname\n1\ta\n3\t");
                assert_eq!(toast, "Copied 2 rows as TSV");
            }
            _ => panic!("no copy"),
        }
        assert!(g.copy_menu.is_none());
        // a letter picks directly; the cursor row when nothing is selected
        g.selected.clear();
        g.row = 1;
        g.col = 1;
        g.key(&key("c"), &set, None);
        match g.key(&key("v"), &set, None) {
            Some(Some(Action::Copy(text, toast))) => {
                assert_eq!(text, "b");
                assert_eq!(toast, "Copied 1 value of name");
            }
            _ => panic!("no copy"),
        }
        // esc closes without copying
        g.key(&key("c"), &set, None);
        assert!(matches!(g.key(&key("escape"), &set, None), Some(None)));
        assert!(g.copy_menu.is_none());
    }
}
