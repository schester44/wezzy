//! A small multi-line text editor for views: cursor, selection, undo, and the
//! usual macOS / emacs movement keys. Text is drawn in the terminal font on its
//! cell grid, so a column is one cell wide.

use crate::render::Metrics;
use crate::ui::Theme;
use gpui::{div, prelude::*, px, AnyElement, Hsla, Keystroke, Pixels, Point};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone)]
struct Snap {
    lines: Vec<Vec<char>>,
    cur: Pos,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    None,
    Typing,
    Other,
}

pub struct Editor {
    lines: Vec<Vec<char>>,
    pub cur: Pos,
    anchor: Option<Pos>,
    /// column kept while moving up/down through shorter lines
    goal: Option<usize>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    last: Edit,
    scroll_row: usize,
    scroll_col: usize,
    /// bumped on every change, so owners can tell when to save
    pub version: u64,
    /// text inset from the left and top
    pub inset: (f32, f32),
    /// rows drawn with a faint band behind them (the statement ⌘↵ runs)
    pub band: Option<(usize, usize)>,
}


fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn new(text: &str) -> Self {
        let mut e = Self {
            lines: vec![vec![]],
            cur: Pos::default(),
            anchor: None,
            goal: None,
            undo: vec![],
            redo: vec![],
            last: Edit::None,
            scroll_row: 0,
            scroll_col: 0,
            version: 0,
            inset: (10., 5.),
            band: None,
        };
        e.lines = split_lines(text);
        e
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn line(&self, row: usize) -> &[char] {
        self.lines.get(row).map(Vec::as_slice).unwrap_or(&[])
    }

    // -- positions -----------------------------------------------------------

    /// Char offset of `p` in `text()`.
    pub fn offset(&self, p: Pos) -> usize {
        self.lines[..p.row].iter().map(|l| l.len() + 1).sum::<usize>() + p.col
    }

    fn clamp(&self, p: Pos) -> Pos {
        let row = p.row.min(self.lines.len() - 1);
        Pos {
            row,
            col: p.col.min(self.lines[row].len()),
        }
    }

    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        (a != self.cur).then(|| if a < self.cur { (a, self.cur) } else { (self.cur, a) })
    }

    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection()?;
        let text: Vec<char> = self.text().chars().collect();
        Some(text[self.offset(a)..self.offset(b)].iter().collect())
    }

    /// The identifier being typed: its start and text (`us` in `from us|`).
    pub fn word_before_cursor(&self) -> (Pos, String) {
        let line = &self.lines[self.cur.row];
        let mut start = self.cur.col;
        while start > 0 && is_word(line[start - 1]) {
            start -= 1;
        }
        (
            Pos {
                row: self.cur.row,
                col: start,
            },
            line[start..self.cur.col].iter().collect(),
        )
    }

    // -- editing -------------------------------------------------------------

    fn snapshot(&mut self, kind: Edit) {
        if !(kind == Edit::Typing && self.last == Edit::Typing) {
            self.undo.push(Snap {
                lines: self.lines.clone(),
                cur: self.cur,
            });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last = kind;
        self.version += 1;
    }

    fn delete_range(&mut self, a: Pos, b: Pos) {
        let tail: Vec<char> = self.lines[b.row][b.col..].to_vec();
        self.lines[a.row].truncate(a.col);
        self.lines[a.row].extend(tail);
        self.lines.drain(a.row + 1..=b.row);
        self.cur = a;
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((a, b)) => {
                self.delete_range(a, b);
                self.anchor = None;
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    fn insert_raw(&mut self, s: &str) {
        let mut parts = split_lines(s).into_iter();
        let first = parts.next().unwrap_or_default();
        let tail: Vec<char> = self.lines[self.cur.row].split_off(self.cur.col);
        self.lines[self.cur.row].extend(&first);
        self.cur.col += first.len();
        for part in parts {
            self.cur.row += 1;
            self.cur.col = part.len();
            self.lines.insert(self.cur.row, part);
        }
        self.lines[self.cur.row].extend(tail);
    }

    pub fn insert(&mut self, s: &str) {
        let typing = s.chars().count() == 1 && !s.starts_with(char::is_whitespace);
        self.snapshot(if typing { Edit::Typing } else { Edit::Other });
        self.delete_selection();
        self.insert_raw(s);
        self.goal = None;
    }

    /// Replace `from..cursor` on the cursor's line (completion).
    pub fn replace_before_cursor(&mut self, from: Pos, with: &str) {
        self.snapshot(Edit::Other);
        self.anchor = None;
        self.delete_range(from, self.cur);
        self.insert_raw(with);
    }

    fn newline(&mut self) {
        let indent: String = self.lines[self.cur.row]
            .iter()
            .take_while(|c| **c == ' ' || **c == '\t')
            .collect();
        self.snapshot(Edit::Other);
        self.delete_selection();
        self.insert_raw(&format!("\n{indent}"));
    }

    fn backspace(&mut self, to: Pos) {
        self.snapshot(Edit::Other);
        if !self.delete_selection() && to != self.cur {
            self.delete_range(to, self.cur);
        }
    }

    fn delete_forward(&mut self) {
        self.snapshot(Edit::Other);
        if !self.delete_selection() {
            let to = self.right(self.cur);
            if to != self.cur {
                self.delete_range(self.cur, to);
            }
        }
    }

    pub fn cut(&mut self) -> Option<String> {
        let s = self.selected_text()?;
        self.snapshot(Edit::Other);
        self.delete_selection();
        Some(s)
    }

    fn restore(&mut self, from_undo: bool) {
        let (pop, push) = if from_undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        if let Some(s) = pop.pop() {
            push.push(Snap {
                lines: std::mem::replace(&mut self.lines, s.lines),
                cur: self.cur,
            });
            self.cur = self.clamp(s.cur);
            self.anchor = None;
            self.last = Edit::None;
            self.version += 1;
        }
    }

    // -- movement ------------------------------------------------------------

    fn left(&self, p: Pos) -> Pos {
        if p.col > 0 {
            Pos { col: p.col - 1, ..p }
        } else if p.row > 0 {
            Pos {
                row: p.row - 1,
                col: self.lines[p.row - 1].len(),
            }
        } else {
            p
        }
    }

    fn right(&self, p: Pos) -> Pos {
        if p.col < self.lines[p.row].len() {
            Pos { col: p.col + 1, ..p }
        } else if p.row + 1 < self.lines.len() {
            Pos { row: p.row + 1, col: 0 }
        } else {
            p
        }
    }

    fn word_left(&self, mut p: Pos) -> Pos {
        if p.col == 0 {
            return self.left(p);
        }
        let l = &self.lines[p.row];
        while p.col > 0 && !is_word(l[p.col - 1]) {
            p.col -= 1;
        }
        while p.col > 0 && is_word(l[p.col - 1]) {
            p.col -= 1;
        }
        p
    }

    fn word_right(&self, mut p: Pos) -> Pos {
        let l = &self.lines[p.row];
        if p.col == l.len() {
            return self.right(p);
        }
        while p.col < l.len() && !is_word(l[p.col]) {
            p.col += 1;
        }
        while p.col < l.len() && is_word(l[p.col]) {
            p.col += 1;
        }
        p
    }

    fn vertical(&mut self, down: bool) -> Pos {
        let goal = *self.goal.get_or_insert(self.cur.col);
        let row = if down {
            (self.cur.row + 1).min(self.lines.len() - 1)
        } else {
            self.cur.row.saturating_sub(1)
        };
        if row == self.cur.row {
            return Pos {
                row,
                col: if down { self.lines[row].len() } else { 0 },
            };
        }
        self.clamp(Pos { row, col: goal })
    }

    fn move_to(&mut self, p: Pos, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cur);
        } else {
            self.anchor = None;
        }
        self.cur = p;
        self.last = Edit::None;
    }

    /// Put the cursor where the mouse went down (`pos` relative to the
    /// editor); `extend` (shift) selects up to it.
    pub fn click(&mut self, m: &Metrics, pos: Point<Pixels>, extend: bool) {
        let (ix, iy) = self.inset;
        let col = ((f32::from(pos.x) - ix) / f32::from(m.cell_w)).round().max(0.) as usize;
        let row = ((f32::from(pos.y) - iy) / f32::from(m.line_h)).floor().max(0.) as usize;
        let p = self.clamp(Pos {
            row: row + self.scroll_row,
            col: col + self.scroll_col,
        });
        self.goal = None;
        self.move_to(p, extend);
    }

    /// The position of char offset `at` in `text()`.
    pub fn pos_of(&self, mut at: usize) -> Pos {
        for (row, l) in self.lines.iter().enumerate() {
            if at <= l.len() {
                return Pos { row, col: at };
            }
            at -= l.len() + 1;
        }
        let row = self.lines.len() - 1;
        Pos { row, col: self.lines[row].len() }
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(Pos::default());
        let row = self.lines.len() - 1;
        self.cur = Pos {
            row,
            col: self.lines[row].len(),
        };
    }

    /// Editing and movement keys. `false` if the key isn't an editor key.
    pub fn key(&mut self, ks: &Keystroke) -> bool {
        let m = &ks.modifiers;
        let sel = m.shift;
        let line_end = Pos {
            row: self.cur.row,
            col: self.lines[self.cur.row].len(),
        };
        let line_start = Pos {
            row: self.cur.row,
            col: 0,
        };
        let vertical = matches!(ks.key.as_str(), "up" | "down") && !m.platform;
        if !vertical {
            self.goal = None;
        }
        match ks.key.as_str() {
            "left" if m.platform => self.move_to(line_start, sel),
            "left" if m.alt => self.move_to(self.word_left(self.cur), sel),
            "left" => match self.selection() {
                Some((a, _)) if !sel => self.move_to(a, false),
                _ => self.move_to(self.left(self.cur), sel),
            },
            "right" if m.platform => self.move_to(line_end, sel),
            "right" if m.alt => self.move_to(self.word_right(self.cur), sel),
            "right" => match self.selection() {
                Some((_, b)) if !sel => self.move_to(b, false),
                _ => self.move_to(self.right(self.cur), sel),
            },
            "up" if m.platform => self.move_to(Pos::default(), sel),
            "down" if m.platform => {
                let row = self.lines.len() - 1;
                self.move_to(Pos { row, col: self.lines[row].len() }, sel)
            }
            "up" | "down" => {
                let p = self.vertical(ks.key == "down");
                self.move_to(p, sel)
            }
            "home" => self.move_to(line_start, sel),
            "end" => self.move_to(line_end, sel),
            "a" if m.control => self.move_to(line_start, sel),
            "e" if m.control => self.move_to(line_end, sel),
            "a" if m.platform => self.select_all(),
            "z" if m.platform && m.shift => self.restore(false),
            "z" if m.platform => self.restore(true),
            "k" if m.control => {
                let to = if self.cur == line_end { self.right(self.cur) } else { line_end };
                self.anchor = Some(to);
                self.backspace(self.cur);
            }
            "backspace" => {
                let to = if m.platform {
                    line_start
                } else if m.alt {
                    self.word_left(self.cur)
                } else {
                    self.left(self.cur)
                };
                self.backspace(to);
            }
            "delete" => self.delete_forward(),
            "enter" if !m.platform && !m.control => self.newline(),
            "tab" if !m.platform && !m.control => self.insert("  "),
            _ if !m.platform && !m.control => match &ks.key_char {
                Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => self.insert(c),
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    // -- drawing -------------------------------------------------------------

    /// Scroll so the cursor stays inside `rows` × `cols`.
    fn follow(&mut self, rows: usize, cols: usize) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if self.cur.row < self.scroll_row {
            self.scroll_row = self.cur.row;
        } else if self.cur.row >= self.scroll_row + rows {
            self.scroll_row = self.cur.row + 1 - rows;
        }
        if self.cur.col < self.scroll_col {
            self.scroll_col = self.cur.col;
        } else if self.cur.col >= self.scroll_col + cols {
            self.scroll_col = self.cur.col + 1 - cols;
        }
    }

    /// Where the cursor is drawn, relative to the editor's top-left.
    pub fn cursor_origin(&self, m: &Metrics) -> (Pixels, Pixels) {
        (
            px(self.inset.0) + m.cell_w * (self.cur.col.saturating_sub(self.scroll_col)) as f32,
            px(self.inset.1) + m.line_h * (self.cur.row.saturating_sub(self.scroll_row)) as f32,
        )
    }

    pub fn render(
        &mut self,
        m: &Metrics,
        t: &Theme,
        focused: bool,
        width: Pixels,
        height: Pixels,
        color: impl Fn(usize, &[char]) -> Vec<(std::ops::Range<usize>, Hsla)>,
    ) -> AnyElement {
        let (ix, iy) = self.inset;
        let rows = (f32::from(height - px(iy * 2.)) / f32::from(m.line_h)).floor() as usize;
        let cols = (f32::from(width - px(ix * 2.)) / f32::from(m.cell_w)).floor() as usize;
        self.follow(rows, cols);
        let sel = self.selection();
        let sel_fill = crate::ui::with_alpha(t.accent, if focused { 0.28 } else { 0.14 });
        let x = |col: usize| px(ix) + m.cell_w * col.saturating_sub(self.scroll_col) as f32;
        let mut layer = vec![];
        let mut text = vec![];
        let end = (self.scroll_row + rows + 1).min(self.lines.len());
        for row in self.scroll_row..end {
            let y = px(iy) + m.line_h * (row - self.scroll_row) as f32;
            if self.band.is_some_and(|(a, b)| row >= a && row <= b) {
                layer.push(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(y)
                        .h(m.line_h)
                        .bg(crate::ui::with_alpha(t.fg, 0.045))
                        .into_any_element(),
                );
            }
            let line = &self.lines[row];
            if let Some((a, b)) = sel {
                if row >= a.row && row <= b.row {
                    let from = if row == a.row { a.col } else { 0 };
                    let to = if row == b.row { b.col } else { line.len() + 1 };
                    if to > from {
                        layer.push(
                            div()
                                .absolute()
                                .left(x(from))
                                .top(y)
                                .w(m.cell_w * (to - from) as f32)
                                .h(m.line_h)
                                .bg(sel_fill)
                                .into_any_element(),
                        );
                    }
                }
            }
            let visible: Vec<char> = line.iter().skip(self.scroll_col).copied().collect();
            let spans = color(row, line);
            let mut row_el = div()
                .absolute()
                .left(px(ix))
                .top(y)
                .h(m.line_h)
                .flex()
                .whitespace_nowrap();
            // split the visible part into colored runs
            let mut col = 0;
            while col < visible.len() {
                let abs = col + self.scroll_col;
                let c = spans
                    .iter()
                    .find(|(r, _)| r.contains(&abs))
                    .map(|(_, c)| *c)
                    .unwrap_or(t.fg);
                let mut end = col + 1;
                while end < visible.len() {
                    let a2 = end + self.scroll_col;
                    let c2 = spans
                        .iter()
                        .find(|(r, _)| r.contains(&a2))
                        .map(|(_, c)| *c)
                        .unwrap_or(t.fg);
                    if c2 != c {
                        break;
                    }
                    end += 1;
                }
                let s: String = visible[col..end].iter().collect();
                row_el = row_el.child(div().text_color(c).child(s));
                col = end;
            }
            text.push(row_el.into_any_element());
        }
        if focused {
            let (cx_, cy) = self.cursor_origin(m);
            layer.push(
                div()
                    .absolute()
                    .left(cx_)
                    .top(cy)
                    .w(px(1.5))
                    .h(m.line_h)
                    .bg(t.accent)
                    .into_any_element(),
            );
        }
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family(m.font.family.clone())
            .text_size(m.font_size)
            .line_height(m.line_h)
            .children(layer)
            .children(text)
            .into_any_element()
    }
}

fn split_lines(s: &str) -> Vec<Vec<char>> {
    s.replace("\r\n", "\n")
        .split('\n')
        .map(|l| l.replace('\t', "  ").chars().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_delete_across_lines() {
        let mut e = Editor::new("ab\ncd");
        e.cur = Pos { row: 0, col: 1 };
        e.insert("X\nY");
        assert_eq!(e.text(), "aX\nYb\ncd");
        e.anchor = Some(Pos { row: 0, col: 0 });
        e.cur = Pos { row: 2, col: 1 };
        e.insert("");
        assert_eq!(e.text(), "d");
        e.restore(true);
        assert_eq!(e.text(), "aX\nYb\ncd");
    }

    #[test]
    fn word_before_cursor() {
        let mut e = Editor::new("select u.na");
        e.cur = Pos { row: 0, col: 11 };
        assert_eq!(e.word_before_cursor().1, "na");
    }
}
