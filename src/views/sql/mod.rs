//! The SQL view: an editor on top, the last result below.
//!
//! ⌘↵ runs the statement under the cursor (or the selection). esc moves to the
//! result grid (vim keys, `y` copies a cell, `Y` a row); `i`/esc go back.
//! Completion pops up while you type a name; tab or ↵ takes it.
//!
//! The query text is kept per database in `~/.local/state/wezzy/sql/<name>.sql`.

pub mod complete;
pub mod conn;
mod db;

use super::editor::{Editor, Pos};
use super::{PaneView, ViewCx};
use crate::render::Metrics;
use crate::ui::{with_alpha, Theme};
use complete::Candidate;
use conn::Conn;
use db::{Db, Outcome, Status};
use gpui::{div, prelude::*, px, AnyElement, App, ClipboardItem, FontWeight, Hsla, Keystroke, Pixels};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;

/// Queries to run when a view for that database opens ("sql neverwait select …"
/// in ⌘K). Keyed by connection name.
static PENDING: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn run_on_open(name: &str, query: String) {
    PENDING
        .lock()
        .get_or_insert_with(HashMap::new)
        .insert(name.to_string(), query);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Editor,
    Grid,
}

struct Menu {
    items: Vec<Candidate>,
    selected: usize,
    /// where the replaced word starts
    from: Pos,
}

#[derive(Default)]
struct Grid {
    row: usize,
    col: usize,
    top: usize,
    left: usize,
    /// rows that fit, from the last draw (paging)
    page: usize,
    /// result the widths below belong to
    seq: u64,
    widths: Vec<usize>,
}

pub struct SqlView {
    conn: Option<Conn>,
    /// why `conn` is missing
    missing: String,
    db: Option<Db>,
    editor: Editor,
    focus: Focus,
    menu: Option<Menu>,
    grid: Grid,
    scratch: Option<PathBuf>,
    saved: u64,
}

const STATUS_H: f32 = 26.;
const CELL_PAD: f32 = 8.;
const MAX_COL_CHARS: usize = 48;

impl SqlView {
    pub fn new(args: &str) -> Self {
        let conn = conn::resolve(args);
        let name = args.split(' ').next().unwrap_or(args).to_string();
        let scratch = conn
            .as_ref()
            .map(|c| crate::paths::state_dir().join("sql").join(format!("{}.sql", c.name)));
        let text = scratch
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let mut editor = Editor::new(text.trim_end());
        let pending = PENDING.lock().as_mut().and_then(|m| m.remove(&name));
        let db = conn.as_ref().map(|c| Db::connect(c.url.clone()));
        let mut v = Self {
            missing: format!("No database named {name}"),
            conn,
            db,
            editor: Editor::new(""),
            focus: Focus::Editor,
            menu: None,
            grid: Grid::default(),
            scratch,
            saved: 0,
        };
        // append the query as its own statement and run it
        if let Some(q) = pending {
            editor.key(&Keystroke::parse("cmd-down").unwrap());
            if !editor.text().trim().is_empty() {
                let sep = if editor.text().trim_end().ends_with(';') { "\n\n" } else { ";\n\n" };
                editor.insert(sep);
            }
            editor.insert(q.trim());
            v.editor = editor;
            v.run();
        } else {
            v.editor = editor;
        }
        v.saved = v.editor.version;
        v
    }

    fn save(&mut self) {
        if self.editor.version == self.saved {
            return;
        }
        if let Some(p) = &self.scratch {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            std::fs::write(p, self.editor.text() + "\n").ok();
        }
        self.saved = self.editor.version;
    }

    /// The selection, or the statement the cursor is in.
    fn statement(&self) -> String {
        if let Some(s) = self.editor.selected_text() {
            return s;
        }
        let text = self.editor.text();
        let (start, end) = statement_at(&text, self.editor.offset(self.editor.cur));
        text.chars().skip(start).take(end - start).collect()
    }

    fn run(&mut self) {
        self.save();
        let sql = self.statement();
        if sql.trim().is_empty() {
            return;
        }
        if let Some(db) = &self.db {
            db.run(sql);
        }
    }

    // -- completion ----------------------------------------------------------

    fn refresh_menu(&mut self, force: bool) {
        let Some(db) = &self.db else { return };
        let schema = db.shared.lock().schema.clone();
        let (from, prefix) = self.editor.word_before_cursor();
        let line = self.editor.line(from.row);
        // `alias.` / `schema.` before the word
        let qualifier = (from.col > 0 && line[from.col - 1] == '.').then(|| {
            let mut s = from.col - 1;
            while s > 0 && (line[s - 1].is_alphanumeric() || line[s - 1] == '_' || line[s - 1] == '"') {
                s -= 1;
            }
            line[s..from.col - 1].iter().collect::<String>()
        });
        if prefix.is_empty() && qualifier.is_none() && !force {
            self.menu = None;
            return;
        }
        let text = self.editor.text();
        let cursor = self.editor.offset(self.editor.cur);
        let (start, end) = statement_at(&text, cursor);
        let chars: Vec<char> = text.chars().collect();
        let statement: String = chars[start..end].iter().collect();
        let before: String = chars[start..cursor].iter().collect();
        let items = complete::complete(&schema, &statement, &before, qualifier.as_deref(), &prefix);
        self.menu = (!items.is_empty()).then_some(Menu {
            items,
            selected: 0,
            from,
        });
    }

    fn accept(&mut self) {
        if let Some(m) = self.menu.take() {
            if let Some(c) = m.items.get(m.selected) {
                self.editor.replace_before_cursor(m.from, &c.text);
            }
        }
    }

    fn menu_key(&mut self, ks: &Keystroke) -> bool {
        let Some(m) = self.menu.as_mut() else { return false };
        let n = m.items.len();
        let ctrl = ks.modifiers.control;
        match ks.key.as_str() {
            "down" => m.selected = (m.selected + 1) % n,
            "n" if ctrl => m.selected = (m.selected + 1) % n,
            "up" => m.selected = (m.selected + n - 1) % n,
            "p" if ctrl => m.selected = (m.selected + n - 1) % n,
            "tab" | "enter" if !ks.modifiers.platform => self.accept(),
            "escape" => self.menu = None,
            _ => return false,
        }
        true
    }

    // -- result grid ---------------------------------------------------------

    fn grid_key(&mut self, ks: &Keystroke, cx: &mut App) -> bool {
        let Some(db) = &self.db else { return false };
        let shared = db.shared.lock();
        let Some(Outcome::Rows(set)) = shared.last.as_ref().map(|r| &r.outcome) else {
            drop(shared);
            if matches!(ks.key.as_str(), "escape" | "i" | "enter") {
                self.focus = Focus::Editor;
                return true;
            }
            return false;
        };
        let rows = set.rows.len();
        let cols = set.columns.len();
        let g = &mut self.grid;
        let half = (g.page / 2).max(1);
        let m = &ks.modifiers;
        match (ks.key.as_str(), m.control) {
            ("j" | "down", false) => g.row = (g.row + 1).min(rows.saturating_sub(1)),
            ("k" | "up", false) => g.row = g.row.saturating_sub(1),
            ("l" | "right", false) => g.col = (g.col + 1).min(cols.saturating_sub(1)),
            ("h" | "left", false) => g.col = g.col.saturating_sub(1),
            ("d", true) => g.row = (g.row + half).min(rows.saturating_sub(1)),
            ("u", true) => g.row = g.row.saturating_sub(half),
            ("f", true) => g.row = (g.row + g.page.max(1)).min(rows.saturating_sub(1)),
            ("b", true) => g.row = g.row.saturating_sub(g.page.max(1)),
            ("g", false) if m.shift => g.row = rows.saturating_sub(1),
            ("g", false) => g.row = 0,
            ("0" | "^", false) => g.col = 0,
            ("4", false) if m.shift => g.col = cols.saturating_sub(1),
            ("$", false) => g.col = cols.saturating_sub(1),
            ("y", false) => {
                let text = if m.shift {
                    set.rows.get(g.row).map(|r| {
                        r.iter()
                            .map(|v| v.as_deref().unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join("\t")
                    })
                } else {
                    set.rows
                        .get(g.row)
                        .and_then(|r| r.get(g.col))
                        .map(|v| v.clone().unwrap_or_default())
                };
                if let Some(text) = text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            ("escape" | "i" | "enter", false) => self.focus = Focus::Editor,
            _ => return false,
        }
        true
    }

    fn render_grid(&mut self, m: &Metrics, t: &Theme, focused: bool, size: gpui::Size<Pixels>) -> AnyElement {
        let Some(db) = &self.db else {
            return empty_line(&self.missing, t.fg_muted, m);
        };
        let shared = db.shared.lock();
        match &shared.status {
            Status::Failed(e) => return empty_line(e, t.danger, m),
            Status::Connecting => return div().into_any_element(),
            Status::Ready => {}
        }
        let set = match shared.last.as_ref().map(|r| &r.outcome) {
            None => return div().into_any_element(),
            Some(Outcome::Error(e)) => return empty_line(e, t.danger, m),
            Some(Outcome::Done(n)) => {
                return empty_line(
                    &format!("{n} row{} affected", if *n == 1 { "" } else { "s" }),
                    t.fg_muted,
                    m,
                )
            }
            Some(Outcome::Rows(set)) => set,
        };
        let g = &mut self.grid;
        if g.seq != shared.seq {
            g.seq = shared.seq;
            *g = Grid {
                seq: shared.seq,
                page: g.page,
                widths: column_widths(set),
                ..Default::default()
            };
        }
        let row_h = m.line_h + px(6.);
        let page = ((f32::from(size.height - row_h) / f32::from(row_h)).floor() as usize).max(1);
        g.page = page;
        if g.row < g.top {
            g.top = g.row;
        } else if g.row >= g.top + page {
            g.top = g.row + 1 - page;
        }
        let px_w = |chars: usize| m.cell_w * chars as f32 + px(CELL_PAD * 2.);
        // scroll columns so the cursor's column fits
        if g.col < g.left {
            g.left = g.col;
        }
        loop {
            let used: f32 = (g.left..=g.col).map(|c| f32::from(px_w(g.widths[c]))).sum();
            if used <= f32::from(size.width) || g.left == g.col {
                break;
            }
            g.left += 1;
        }
        let mut shown = vec![];
        let mut x = 0.;
        for c in g.left..set.columns.len() {
            shown.push(c);
            x += f32::from(px_w(g.widths[c]));
            if x > f32::from(size.width) {
                break;
            }
        }
        let row_fill = with_alpha(t.fg, 0.04);
        let cell_fill = with_alpha(t.fg, if focused { 0.09 } else { 0.05 });
        let bar = if focused { t.accent } else { with_alpha(t.fg_subtle, 0.5) };
        let cell = |text: String, w: usize, color: Hsla| {
            div()
                .w(px_w(w))
                .flex_none()
                .px(px(CELL_PAD))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(color)
                .child(text)
        };
        let header = div()
            .h(row_h)
            .flex()
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .font_weight(FontWeight::MEDIUM)
            .children(shown.iter().map(|&c| cell(clip(&set.columns[c], g.widths[c]), g.widths[c], t.fg_muted)));
        let end = (g.top + page).min(set.rows.len());
        let rows = (g.top..end).map(|r| {
            let selected = r == g.row;
            div()
                .relative()
                .h(row_h)
                .flex()
                .items_center()
                .when(selected, |d| d.bg(row_fill))
                .when(selected, |d| {
                    d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(bar))
                })
                .children(shown.iter().map(|&c| {
                    let v = set.rows[r].get(c).cloned().flatten();
                    let (text, color) = match v {
                        Some(v) => (clip(&v, g.widths[c]), t.fg),
                        None => ("NULL".to_string(), t.fg_subtle),
                    };
                    cell(text, g.widths[c], color).when(selected && c == g.col, |d| d.h_full().flex().items_center().bg(cell_fill))
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

    fn status_line(&self, m: &Metrics, t: &Theme) -> AnyElement {
        let left = self
            .conn
            .as_ref()
            .map(|c| format!("{} · {}", c.name, c.target()))
            .unwrap_or_default();
        let mut right = div().flex().items_center().gap_2();
        if let Some(db) = &self.db {
            let s = db.shared.lock();
            if let Some(started) = s.running {
                right = right
                    .child(crate::ui::spinner(11., t.fg_muted))
                    .child(format!("{:.1}s", started.elapsed().as_secs_f32()));
            } else if matches!(s.status, Status::Connecting) {
                right = right.child(crate::ui::spinner(11., t.fg_muted));
            } else if let Some(run) = &s.last {
                let ms = run.elapsed.as_millis();
                let what = match &run.outcome {
                    Outcome::Rows(set) if set.total > set.rows.len() => {
                        format!("{} of {} rows", set.rows.len(), set.total)
                    }
                    Outcome::Rows(set) => {
                        format!("{} row{}", set.total, if set.total == 1 { "" } else { "s" })
                    }
                    _ => String::new(),
                };
                right = right.child(if what.is_empty() {
                    format!("{ms} ms")
                } else {
                    format!("{what} · {ms} ms")
                });
            }
        }
        div()
            .h(px(STATUS_H))
            .flex_none()
            .px(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_b_1()
            .border_color(t.border)
            .font_family(m.font.family.clone())
            .text_size(px(11.))
            .text_color(t.fg_subtle)
            .child(left)
            .child(right)
            .into_any_element()
    }

    fn render_menu(&self, m: &Metrics, t: &Theme) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let (x, y) = self.editor.cursor_origin(m);
        let width = menu
            .items
            .iter()
            .map(|c| c.text.chars().count() + c.detail.chars().count() + 4)
            .max()
            .unwrap_or(10)
            .min(60);
        const SHOW: usize = 8;
        let first = menu.selected.saturating_sub(SHOW - 1);
        let rows = menu.items.iter().enumerate().skip(first).take(SHOW).map(|(i, c)| {
            let selected = i == menu.selected;
            div()
                .relative()
                .h(m.line_h + px(4.))
                .px(px(8.))
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .when(selected, |d| {
                    d.bg(with_alpha(t.fg, 0.07))
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
                })
                .child(div().text_color(t.fg).child(c.text.clone()))
                .child(div().text_color(t.fg_subtle).child(c.detail.clone()))
        });
        Some(
            div()
                .absolute()
                .left(x - px(8.))
                .top(y + m.line_h + px(2.))
                .w(m.cell_w * width as f32 + px(16.))
                .py(px(4.))
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .rounded_lg()
                .shadow_md()
                .overflow_hidden()
                .font_family(m.font.family.clone())
                .text_size(m.font_size)
                .children(rows)
                .into_any_element(),
        )
    }
}

impl Drop for SqlView {
    fn drop(&mut self) {
        self.save();
    }
}

impl PaneView for SqlView {
    fn render(&mut self, vcx: &ViewCx, t: &Theme, _cx: &mut App) -> AnyElement {
        let m = &vcx.metrics;
        let editor_h = (vcx.size.height * 0.4).max(m.line_h * 6. + px(10.)).min(vcx.size.height * 0.7);
        let grid_h = vcx.size.height - editor_h - px(STATUS_H);
        let editor_focused = vcx.focused && self.focus == Focus::Editor;
        let grid_focused = vcx.focused && self.focus == Focus::Grid;
        let keyword = t.fg_muted;
        let comment = t.fg_subtle;
        let editor = self.editor.render(m, t, editor_focused, vcx.size.width, editor_h, |_, line| {
            highlight(line, keyword, comment)
        });
        let menu = if editor_focused { self.render_menu(m, t) } else { None };
        let status = self.status_line(m, t);
        let grid = self.render_grid(m, t, grid_focused, gpui::size(vcx.size.width, grid_h));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().relative().h(editor_h).flex_none().child(editor).children(menu))
            .child(status)
            .child(div().flex_1().min_h_0().child(grid))
            .into_any_element()
    }

    fn claims(&self, ks: &Keystroke) -> bool {
        let m = &ks.modifiers;
        self.focus == Focus::Editor
            && m.platform
            && !m.control
            && matches!(ks.key.as_str(), "enter" | "a" | "z" | "x")
    }

    fn key(&mut self, ks: &Keystroke, cx: &mut App) -> bool {
        if self.focus == Focus::Grid {
            return self.grid_key(ks, cx);
        }
        let m = &ks.modifiers;
        if self.menu_key(ks) {
            return true;
        }
        match ks.key.as_str() {
            "enter" if m.platform => {
                self.menu = None;
                self.run();
                return true;
            }
            "x" if m.platform => {
                if let Some(s) = self.editor.cut() {
                    cx.write_to_clipboard(ClipboardItem::new_string(s));
                }
                return true;
            }
            "space" if m.control => {
                self.refresh_menu(true);
                return true;
            }
            "escape" => {
                let db = self.db.as_ref();
                if db.is_some_and(|d| d.shared.lock().running.is_some()) {
                    db.unwrap().cancel();
                } else if db.is_some_and(|d| d.shared.lock().last.is_some()) {
                    self.focus = Focus::Grid;
                }
                return true;
            }
            _ => {}
        }
        let version = self.editor.version;
        if !self.editor.key(ks) {
            return false;
        }
        let typed_name = ks
            .key_char
            .as_deref()
            .is_some_and(|c| c.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.'));
        if self.editor.version != version && (typed_name || (ks.key == "backspace" && self.menu.is_some())) {
            self.refresh_menu(false);
        } else {
            self.menu = None;
        }
        true
    }

    fn paste(&mut self, text: &str) {
        if self.focus == Focus::Editor {
            self.menu = None;
            self.editor.insert(text);
        }
    }

    fn copy(&mut self) -> Option<String> {
        match self.focus {
            Focus::Editor => self.editor.selected_text(),
            Focus::Grid => {
                let s = self.db.as_ref()?.shared.lock();
                match s.last.as_ref().map(|r| &r.outcome) {
                    Some(Outcome::Rows(set)) => set
                        .rows
                        .get(self.grid.row)?
                        .get(self.grid.col)
                        .map(|v| v.clone().unwrap_or_default()),
                    _ => None,
                }
            }
        }
    }
}

fn empty_line(text: &str, color: Hsla, m: &Metrics) -> AnyElement {
    div()
        .px(px(10.))
        .py(px(8.))
        .font_family(m.font.family.clone())
        .text_size(m.font_size)
        .text_color(color)
        .child(text.to_string())
        .into_any_element()
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

fn column_widths(set: &db::ResultSet) -> Vec<usize> {
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

/// Keywords muted, comments and strings subtle; names stay `fg`.
fn highlight(line: &[char], keyword: Hsla, comment: Hsla) -> Vec<(std::ops::Range<usize>, Hsla)> {
    let mut out = vec![];
    let mut i = 0;
    while i < line.len() {
        let c = line[i];
        if c == '-' && line.get(i + 1) == Some(&'-') {
            out.push((i..line.len(), comment));
            break;
        }
        if c == '\'' {
            let start = i;
            i += 1;
            while i < line.len() && line[i] != '\'' {
                i += 1;
            }
            out.push((start..(i + 1).min(line.len()), comment));
            i += 1;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < line.len() && (line[i].is_alphanumeric() || line[i] == '_') {
                i += 1;
            }
            let word: String = line[start..i].iter().collect::<String>().to_lowercase();
            let before_dot = start > 0 && line[start - 1] == '.';
            if !before_dot && complete::is_keyword(&word) {
                out.push((start..i, keyword));
            }
            continue;
        }
        i += 1;
    }
    out
}

/// The `;`-separated statement around char offset `at`, ignoring `;` inside
/// quotes and comments. A cursor past the last `;` belongs to the statement
/// before it when nothing follows.
fn statement_at(text: &str, at: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut bounds = vec![0];
    let mut i = 0;
    let mut quote: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '-' && chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            None if c == ';' => bounds.push(i + 1),
            None => {}
        }
        i += 1;
    }
    bounds.push(chars.len());
    let blank = |a: usize, b: usize| chars[a..b].iter().all(|c| c.is_whitespace() || *c == ';');
    for w in (0..bounds.len() - 1).rev() {
        let (a, b) = (bounds[w], bounds[w + 1]);
        if at >= a && at <= b {
            if blank(a, b) && w > 0 {
                return (bounds[w - 1], a);
            }
            return (a, b);
        }
    }
    (0, chars.len())
}

#[cfg(test)]
mod tests {
    use super::statement_at;

    fn at(text: &str, cursor: usize) -> String {
        let (a, b) = statement_at(text, cursor);
        text.chars().skip(a).take(b - a).collect::<String>().trim().to_string()
    }

    #[test]
    fn picks_statement_under_cursor() {
        let t = "select 1;\nselect ';' from x;\n\nselect 3";
        assert_eq!(at(t, 3), "select 1;");
        assert_eq!(at(t, 14), "select ';' from x;");
        assert_eq!(at(t, t.len()), "select 3");
    }

    #[test]
    fn cursor_after_last_semicolon() {
        let t = "select 1;\n";
        assert_eq!(at(t, t.len()), "select 1;");
    }
}
