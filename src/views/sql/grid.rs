//! The result grid: a page of rows with a cursor cell, a row selection, and
//! copying. Only the rows that fit are drawn.

use super::db::ResultSet;
use crate::render::Metrics;
use crate::ui::{with_alpha, Theme};
use gpui::{div, prelude::*, px, AnyElement, FontWeight, Hsla, Keystroke, Modifiers, Pixels, Point, Size};
use std::collections::BTreeSet;

const CELL_PAD: f32 = 8.;
const MAX_COL_CHARS: usize = 48;

/// What the view should do after a key or click.
pub enum Action {
    /// copy this text, and toast this
    Copy(String, String),
    /// previous (-1) or next (+1) page
    Page(isize),
    /// back to the editor
    Editor,
}

#[derive(Default)]
struct Layout {
    row_h: f32,
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
}

fn tsv_value(v: &Option<String>) -> String {
    v.as_deref()
        .unwrap_or("")
        .chars()
        .map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
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
        let widths = column_widths(set);
        let same_shape = widths.len() == self.widths.len();
        *self = Grid {
            seq,
            col: if same_shape { self.col } else { 0 },
            left: if same_shape { self.left } else { 0 },
            page_rows: self.page_rows,
            widths,
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
            let v = set.rows.get(self.row)?.get(self.col)?;
            return Some(Action::Copy(v.clone().unwrap_or_default(), "Copied".into()));
        }
        let rows = self.target_rows();
        let mut lines = vec![];
        if with_header {
            lines.push(set.columns.join("\t"));
        }
        for r in &rows {
            if let Some(row) = set.rows.get(*r) {
                lines.push(row.iter().map(tsv_value).collect::<Vec<_>>().join("\t"));
            }
        }
        Some(Action::Copy(lines.join("\n"), format!("Copied {}", plural(rows.len(), "row"))))
    }

    pub fn select_all(&mut self, set: &ResultSet) {
        self.anchor = None;
        self.selected = (0..set.rows.len()).collect();
    }

    /// `None` if the key isn't a grid key.
    pub fn key(&mut self, ks: &Keystroke, set: &ResultSet) -> Option<Option<Action>> {
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
            ("i" | "enter", false) => return Some(Some(Action::Editor)),
            _ => return None,
        }
        self.moved();
        Some(None)
    }

    /// A click at `pos`, relative to the grid's top-left.
    pub fn click(&mut self, pos: Point<Pixels>, mods: &Modifiers, set: &ResultSet) {
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
        let header = div()
            .h(row_h)
            .flex()
            .border_b_1()
            .border_color(t.border)
            .font_weight(FontWeight::MEDIUM)
            .child(gutter(String::new()))
            .children(shown.iter().map(|&(c, _, _)| {
                cell(clip(&set.columns[c], self.widths[c]), self.widths[c], t.fg_muted)
            }));
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
                    let (text, color) = match set.rows[r].get(c).cloned().flatten() {
                        Some(v) => (clip(&v, self.widths[c]), t.fg),
                        None => ("NULL".to_string(), t.fg_subtle),
                    };
                    cell(text, self.widths[c], color).when(cursor && c == self.col, |d| {
                        d.bg(with_alpha(t.fg, if focused { 0.12 } else { 0.06 }))
                    })
                }))
        });
        div()
            .size_full()
            .overflow_hidden()
            .font_family(m.font.family.clone())
            .text_size(m.font_size)
            .child(header)
            .children(rows)
            .into_any_element()
    }
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
        g.key(&key("shift-v"), &s);
        g.key(&key("j"), &s);
        g.key(&key("j"), &s);
        assert_eq!(g.selected.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);
        g.key(&key("k"), &s);
        assert_eq!(g.selected.iter().copied().collect::<Vec<_>>(), vec![0, 1]);
    }
}
