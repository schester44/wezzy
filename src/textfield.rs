//! A multi-line text field: text, a cursor and a selection anchor (byte
//! offsets on char boundaries), with macOS editing keys, the clipboard, mouse
//! selection and undo. The git panel's commit / pull request composer and its
//! comment box use it; `ui/textfield.rs` draws it and hit-tests the mouse.
//!
//! Keys go through `key` from TermApp's own key routing (there's no IME input
//! handler): typing comes from `key_char`, so shifted and ⌥ characters work.
//! ↑/↓ move by wrapped (visual) rows once the field has been painted; the view
//! leaves its row layout in `Shared`.

use gpui::{App, ClipboardItem, Keystroke};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

/// What a key did, for the box that owns the field.
#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    Submit,
    Cancel,
    Changed,
}

/// One visual (wrapped) row of the laid-out text, in byte offsets of the
/// whole text. `stops` are the caret positions on it (every char boundary
/// from `start` to `end`) with x relative to the row's left edge.
#[derive(Clone, Debug, Default)]
pub struct VRow {
    pub start: usize,
    pub end: usize,
    /// the last row of its logical line (`end` is the line's end, not a wrap)
    pub last: bool,
    pub stops: Vec<(usize, f32)>,
}

impl VRow {
    /// Whether the caret at `i` sits on this row. A wrap offset belongs to
    /// the row it starts.
    fn holds(&self, i: usize) -> bool {
        self.start <= i && (i < self.end || (self.last && i == self.end))
    }

    pub fn x(&self, i: usize) -> f32 {
        self.stops
            .iter()
            .find(|(s, _)| *s == i)
            .map(|(_, x)| *x)
            .unwrap_or_else(|| self.stops.last().map(|s| s.1).unwrap_or(0.))
    }

    /// The caret position nearest to `x`.
    pub fn index_at(&self, x: f32) -> usize {
        self.stops
            .iter()
            .filter(|(s, _)| self.last || *s < self.end || self.start == self.end)
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map(|s| s.0)
            .unwrap_or(self.start)
    }
}

/// What the view and the field share across frames: the layout of the last
/// paint (for ↑/↓ and hit-testing), the scroll offset of a capped box, and
/// whether a mouse drag is selecting.
#[derive(Default)]
pub struct Shared {
    /// the `rev` the rows were laid out for
    pub rev: u64,
    pub rows: Vec<VRow>,
    /// first visible row, when the box caps its height
    pub scroll: usize,
    /// (cursor, rev) the scroll was last fitted to
    pub fitted: Option<(usize, u64)>,
    pub dragging: bool,
    /// leftover scroll-wheel pixels
    pub wheel: f32,
}

impl Shared {
    pub fn row_of(&self, i: usize) -> usize {
        self.rows.iter().position(|r| r.holds(i)).unwrap_or(self.rows.len().saturating_sub(1))
    }
}

/// A mouse event the view hit-tested to a text offset.
#[derive(Clone, Copy, Debug)]
pub enum FieldMouse {
    Down { index: usize, clicks: usize, shift: bool },
    Drag { index: usize },
    Up,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Insert,
    Delete,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Char,
    Word,
    Line,
}

struct Snapshot {
    text: String,
    cursor: usize,
    anchor: usize,
}

const MAX_UNDO: usize = 100;

pub struct TextField {
    text: String,
    cursor: usize,
    anchor: usize,
    /// x kept across ↑/↓ so the column survives short rows
    goal_x: Option<f32>,
    /// bumped on every text change (layout staleness)
    rev: u64,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// the kind of the last edit, to coalesce typing into one undo step
    group: Option<Group>,
    /// a drag selecting by word / line keeps the unit it started on
    drag: Option<(Unit, Range<usize>)>,
    pub shared: Rc<RefCell<Shared>>,
}

impl Default for TextField {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl TextField {
    /// A field holding `text`, with the cursor at its end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let end = text.len();
        Self {
            text,
            cursor: end,
            anchor: end,
            goal_x: None,
            rev: 0,
            undo: vec![],
            redo: vec![],
            group: None,
            drag: None,
            shared: Default::default(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn into_text(self) -> String {
        self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// The selected range (empty when nothing is selected).
    pub fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    // ---- editing ---------------------------------------------------------

    fn checkpoint(&mut self, g: Group) {
        if self.group != Some(g) || g == Group::Other {
            self.undo.push(Snapshot { text: self.text.clone(), cursor: self.cursor, anchor: self.anchor });
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.group = Some(g);
    }

    fn replace(&mut self, range: Range<usize>, with: &str, g: Group) {
        if range.is_empty() && with.is_empty() {
            return;
        }
        self.checkpoint(g);
        self.text.replace_range(range.clone(), with);
        self.cursor = range.start + with.len();
        self.anchor = self.cursor;
        self.goal_x = None;
        self.rev += 1;
    }

    /// Type `s` over the selection.
    pub fn insert(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        // a paste or a newline is its own undo step
        let g = if s.chars().count() == 1 && s != "\n" { Group::Insert } else { Group::Other };
        self.replace(self.selection(), &s, g);
    }

    /// Delete the selection, or from the cursor to `to`.
    fn delete_to(&mut self, to: usize) {
        let r = if self.has_selection() {
            self.selection()
        } else {
            self.cursor.min(to)..self.cursor.max(to)
        };
        self.replace(r, "", Group::Delete);
    }

    fn restore(&mut self, s: Snapshot) -> Snapshot {
        let now = Snapshot { text: std::mem::replace(&mut self.text, s.text), cursor: self.cursor, anchor: self.anchor };
        self.cursor = s.cursor.min(self.text.len());
        self.anchor = s.anchor.min(self.text.len());
        self.goal_x = None;
        self.group = None;
        self.rev += 1;
        now
    }

    pub fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            let now = self.restore(s);
            self.redo.push(now);
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            let now = self.restore(s);
            self.undo.push(now);
        }
    }

    // ---- movement --------------------------------------------------------

    /// Move the cursor to `i`; with `extend` the anchor stays (shift).
    fn move_to(&mut self, i: usize, extend: bool) {
        self.cursor = i.min(self.text.len());
        if !extend {
            self.anchor = self.cursor;
        }
        self.group = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
        self.group = None;
    }

    /// ↑ / ↓: one visual row when the layout is current, else one logical line.
    fn vertical(&mut self, down: bool, extend: bool) {
        let shared = self.shared.clone();
        let sh = shared.borrow();
        let target = if sh.rev == self.rev && !sh.rows.is_empty() {
            let r = sh.row_of(self.cursor);
            let x = self.goal_x.unwrap_or_else(|| sh.rows[r].x(self.cursor));
            self.goal_x = Some(x);
            match (down, r) {
                (false, 0) => 0,
                (false, r) => sh.rows[r - 1].index_at(x),
                (true, r) if r + 1 >= sh.rows.len() => self.text.len(),
                (true, r) => sh.rows[r + 1].index_at(x),
            }
        } else {
            logical_vertical(&self.text, self.cursor, down)
        };
        drop(sh);
        let goal = self.goal_x;
        self.move_to(target, extend);
        self.goal_x = goal;
    }

    /// Handle a key. Every key is consumed; `Submit` / `Cancel` are for the owner.
    pub fn key(&mut self, ks: &Keystroke, cx: &mut App) -> Edit {
        let m = &ks.modifiers;
        let shift = m.shift;
        let key = ks.key.as_str();
        if !matches!(key, "up" | "down") {
            self.goal_x = None;
        }
        let (c, t) = (self.cursor, &self.text);
        let sel = self.selection();
        match key {
            "escape" => return Edit::Cancel,
            "enter" if shift => self.insert("\n"),
            "enter" => return Edit::Submit,
            "left" if m.platform => self.move_to(line_start(t, c), shift),
            "right" if m.platform => self.move_to(line_end(t, c), shift),
            "left" if m.alt => self.move_to(word_left(t, c), shift),
            "right" if m.alt => self.move_to(word_right(t, c), shift),
            "left" if !shift && !sel.is_empty() => self.move_to(sel.start, false),
            "right" if !shift && !sel.is_empty() => self.move_to(sel.end, false),
            "left" => self.move_to(prev_char(t, c), shift),
            "right" => self.move_to(next_char(t, c), shift),
            "up" if m.platform => self.move_to(0, shift),
            "down" if m.platform => self.move_to(t.len(), shift),
            "up" => self.vertical(false, shift),
            "down" => self.vertical(true, shift),
            "home" => self.move_to(line_start(t, c), shift),
            "end" => self.move_to(line_end(t, c), shift),
            "backspace" if m.platform => {
                // to the line start; at the start, join with the line above
                let s = line_start(t, c);
                self.delete_to(if s == c { prev_char(t, c) } else { s })
            }
            "backspace" if m.alt => self.delete_to(word_left(t, c)),
            "backspace" => self.delete_to(prev_char(t, c)),
            "delete" if m.platform => self.delete_to(line_end(t, c)),
            "delete" if m.alt => self.delete_to(word_right(t, c)),
            "delete" => self.delete_to(next_char(t, c)),
            "a" if m.platform => self.select_all(),
            "c" if m.platform => {
                if !sel.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(t[sel].to_string()));
                }
            }
            "x" if m.platform => {
                if !sel.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(t[sel.clone()].to_string()));
                    self.replace(sel, "", Group::Other);
                }
            }
            "v" if m.platform => {
                if let Some(s) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    self.insert(&s);
                }
            }
            "z" if m.platform && shift => self.redo(),
            "z" if m.platform => self.undo(),
            // the emacs keys every macOS text field has
            "a" if m.control => self.move_to(line_start(t, c), shift),
            "e" if m.control => self.move_to(line_end(t, c), shift),
            "k" if m.control => {
                let e = line_end(t, c);
                self.delete_to(if e == c { next_char(t, c) } else { e })
            }
            "tab" => self.insert("    "),
            _ if !m.control && !m.platform => {
                if let Some(ch) = ks.key_char.clone() {
                    self.insert(&ch);
                }
            }
            _ => {}
        }
        Edit::Changed
    }

    // ---- mouse -----------------------------------------------------------

    pub fn mouse(&mut self, ev: FieldMouse) {
        match ev {
            FieldMouse::Down { index, shift: true, .. } => {
                self.move_to(index, true);
                self.drag = Some((Unit::Char, index..index));
            }
            FieldMouse::Down { index, clicks, .. } => {
                let (unit, r) = match clicks {
                    0 | 1 => (Unit::Char, index..index),
                    2 => (Unit::Word, word_at(&self.text, index)),
                    _ => (Unit::Line, line_start(&self.text, index)..line_end(&self.text, index)),
                };
                self.anchor = r.start;
                self.cursor = r.end;
                self.group = None;
                self.drag = Some((unit, r));
            }
            FieldMouse::Drag { index } => {
                let Some((unit, origin)) = self.drag.clone() else { return };
                let t = &self.text;
                let (lo, hi) = match unit {
                    Unit::Char => (index, index),
                    Unit::Word => {
                        let w = word_at(t, index);
                        (w.start, w.end)
                    }
                    Unit::Line => (line_start(t, index), line_end(t, index)),
                };
                if unit == Unit::Char {
                    self.cursor = index;
                } else if lo < origin.start {
                    self.anchor = origin.end;
                    self.cursor = lo;
                } else {
                    self.anchor = origin.start;
                    self.cursor = hi.max(origin.end);
                }
            }
            FieldMouse::Up => self.drag = None,
        }
        self.goal_x = None;
        self.shared.borrow_mut().dragging = self.drag.is_some();
    }
}

// ---- text navigation (pure) -----------------------------------------------

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn prev_char(t: &str, i: usize) -> usize {
    t[..i].char_indices().next_back().map(|(j, _)| j).unwrap_or(0)
}

pub fn next_char(t: &str, i: usize) -> usize {
    t[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(t.len())
}

pub fn line_start(t: &str, i: usize) -> usize {
    t[..i].rfind('\n').map(|j| j + 1).unwrap_or(0)
}

pub fn line_end(t: &str, i: usize) -> usize {
    t[i..].find('\n').map(|j| i + j).unwrap_or(t.len())
}

/// ⌥←: back over non-word characters, then to the start of the word.
pub fn word_left(t: &str, i: usize) -> usize {
    let mut it = t[..i].char_indices().rev().peekable();
    let mut at = i;
    while let Some(&(j, c)) = it.peek() {
        if is_word(c) {
            break;
        }
        at = j;
        it.next();
    }
    while let Some(&(j, c)) = it.peek() {
        if !is_word(c) {
            break;
        }
        at = j;
        it.next();
    }
    at
}

/// ⌥→: over non-word characters, then to the end of the word.
pub fn word_right(t: &str, i: usize) -> usize {
    let mut at = i;
    let mut it = t[i..].chars().peekable();
    while let Some(&c) = it.peek() {
        if is_word(c) {
            break;
        }
        at += c.len_utf8();
        it.next();
    }
    while let Some(&c) = it.peek() {
        if !is_word(c) {
            break;
        }
        at += c.len_utf8();
        it.next();
    }
    at
}

/// The word (or run of spaces, or single other character) at `i`, for a
/// double-click.
pub fn word_at(t: &str, i: usize) -> Range<usize> {
    // past the end, the last character's word
    let probe = if i < t.len() {
        i
    } else if i > 0 {
        prev_char(t, i)
    } else {
        return i..i;
    };
    let Some(c) = t[probe..].chars().next() else { return i..i };
    let same = |d: char| if is_word(c) { is_word(d) } else if c == ' ' { d == ' ' } else { false };
    if !is_word(c) && c != ' ' {
        return probe..probe + c.len_utf8();
    }
    let start = t[..probe]
        .char_indices()
        .rev()
        .take_while(|&(_, d)| same(d))
        .last()
        .map(|(j, _)| j)
        .unwrap_or(probe);
    let end = probe
        + t[probe..]
            .chars()
            .take_while(|&d| same(d))
            .map(char::len_utf8)
            .sum::<usize>();
    start..end
}

/// ↑/↓ by logical line, keeping the char column (before the first paint).
fn logical_vertical(t: &str, i: usize, down: bool) -> usize {
    let start = line_start(t, i);
    let col = t[start..i].chars().count();
    let target = if down {
        let end = line_end(t, i);
        if end == t.len() {
            return t.len();
        }
        end + 1
    } else {
        if start == 0 {
            return 0;
        }
        line_start(t, start - 1)
    };
    let end = line_end(t, target);
    t[target..end]
        .char_indices()
        .nth(col)
        .map(|(j, _)| target + j)
        .unwrap_or(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str, cursor: usize) -> TextField {
        let mut f = TextField::new(text);
        f.cursor = cursor;
        f.anchor = cursor;
        f
    }

    #[test]
    fn chars_step_over_multibyte() {
        let t = "aé😀b";
        assert_eq!(next_char(t, 1), 3);
        assert_eq!(next_char(t, 3), 7);
        assert_eq!(prev_char(t, 7), 3);
        assert_eq!(prev_char(t, 0), 0);
        assert_eq!(next_char(t, t.len()), t.len());
    }

    #[test]
    fn word_boundaries() {
        let t = "fix the  foo_bar, now";
        assert_eq!(word_left(t, t.len()), 18);
        assert_eq!(word_left(t, 18), 9);
        assert_eq!(word_left(t, 9), 4);
        assert_eq!(word_left(t, 2), 0);
        assert_eq!(word_right(t, 0), 3);
        assert_eq!(word_right(t, 3), 7);
        assert_eq!(word_right(t, 7), 16);
        assert_eq!(word_right(t, 16), t.len());
        assert_eq!(word_left("über café", 11), 6);
    }

    #[test]
    fn word_at_click() {
        let t = "fix the  bug.";
        assert_eq!(word_at(t, 1), 0..3);
        assert_eq!(word_at(t, 3), 3..4);
        assert_eq!(word_at(t, 11), 9..12);
        assert_eq!(word_at(t, 7), 7..9);
        assert_eq!(word_at(t, 12), 12..13);
        assert_eq!(word_at(t, t.len()), 12..13);
        assert_eq!(word_at("", 0), 0..0);
    }

    #[test]
    fn lines() {
        let t = "one\ntwo\n\nfour";
        assert_eq!(line_start(t, 6), 4);
        assert_eq!(line_end(t, 5), 7);
        assert_eq!(line_start(t, 8), 8);
        assert_eq!(line_end(t, 8), 8);
        assert_eq!(logical_vertical(t, 6, false), 2);
        assert_eq!(logical_vertical(t, 6, true), 8);
        assert_eq!(logical_vertical(t, 1, false), 0);
        assert_eq!(logical_vertical(t, 11, true), t.len());
    }

    #[test]
    fn typing_replaces_selection() {
        let mut f = field("hello world", 0);
        f.anchor = 6;
        f.cursor = 11;
        f.insert("there");
        assert_eq!(f.text(), "hello there");
        assert_eq!(f.cursor(), 11);
        assert!(f.selection().is_empty());
    }

    #[test]
    fn delete_prefers_selection() {
        let mut f = field("abcdef", 4);
        f.anchor = 1;
        f.delete_to(5);
        assert_eq!(f.text(), "aef");
        assert_eq!(f.cursor(), 1);
        let mut f = field("añb", 3);
        f.delete_to(prev_char(f.text(), 3));
        assert_eq!(f.text(), "ab");
    }

    #[test]
    fn select_all_then_type() {
        let mut f = field("old", 1);
        f.select_all();
        assert_eq!(f.selection(), 0..3);
        f.insert("n");
        assert_eq!(f.text(), "n");
    }

    #[test]
    fn undo_coalesces_typing() {
        let mut f = TextField::new("");
        for c in ["a", "b", "c"] {
            f.insert(c);
        }
        f.insert("\n");
        f.insert("d");
        f.undo();
        assert_eq!(f.text(), "abc\n");
        f.undo();
        assert_eq!(f.text(), "abc");
        f.undo();
        assert_eq!(f.text(), "");
        f.redo();
        assert_eq!(f.text(), "abc");
    }

    #[test]
    fn mouse_word_drag() {
        let mut f = TextField::new("one two three");
        f.mouse(FieldMouse::Down { index: 5, clicks: 2, shift: false });
        assert_eq!(f.selection(), 4..7);
        f.mouse(FieldMouse::Drag { index: 10 });
        assert_eq!(f.selection(), 4..13);
        f.mouse(FieldMouse::Drag { index: 1 });
        assert_eq!(f.selection(), 0..7);
        f.mouse(FieldMouse::Up);
        assert!(!f.shared.borrow().dragging);
        f.mouse(FieldMouse::Down { index: 2, clicks: 1, shift: false });
        f.mouse(FieldMouse::Down { index: 9, clicks: 1, shift: true });
        assert_eq!(f.selection(), 2..9);
    }

    #[test]
    fn visual_rows_drive_up_down() {
        // "aaaa bbbb" wrapped after the space: rows [0,5) and [5,9]
        let mut f = field("aaaa bbbb", 7);
        let stops = |s: usize, e: usize| (s..=e).map(|i| (i, (i - s) as f32 * 10.)).collect();
        {
            let mut sh = f.shared.borrow_mut();
            sh.rev = f.rev;
            sh.rows = vec![
                VRow { start: 0, end: 5, last: false, stops: stops(0, 5) },
                VRow { start: 5, end: 9, last: true, stops: stops(5, 9) },
            ];
        }
        f.vertical(false, false);
        assert_eq!(f.cursor(), 2);
        f.vertical(true, false);
        assert_eq!(f.cursor(), 7);
        f.vertical(true, true);
        assert_eq!(f.selection(), 7..9);
        // the wrap offset belongs to the second row; clicking past the first
        // row's end stays on it
        assert_eq!(f.shared.borrow().row_of(5), 1);
        assert_eq!(f.shared.borrow().rows[0].index_at(100.), 4);
    }
}
