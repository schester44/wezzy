//! The SQL view: an editor on top, the last result below.
//!
//! ⌘↵ runs the statement under the cursor (or the selection). esc moves to the
//! result grid (`grid.rs`); `i`/esc go back. Read-only queries are fetched a
//! page at a time (`db.rs`); `[` `]` or the ‹ › buttons turn pages.
//! Completion pops up while you type a name; tab or ↵ takes it.
//!
//! The query text is kept per database in `~/.local/state/wezzy/sql/<name>.sql`.

pub mod complete;
pub mod conn;
mod db;
mod grid;

use super::editor::{Editor, Pos};
use super::{PaneView, ViewCx};
use crate::render::Metrics;
use crate::ui::{with_alpha, Theme};
use complete::Candidate;
use conn::Conn;
use db::{Db, Outcome, Status};
use gpui::{div, prelude::*, px, AnyElement, App, ClipboardItem, Hsla, Keystroke, Modifiers, Pixels, Point};
use grid::{Action, Grid};
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
    /// from the last draw, for mouse hits
    layout: Option<Layout>,
}

struct Layout {
    metrics: Metrics,
    width: f32,
    editor_h: f32,
}

const STATUS_H: f32 = 26.;
/// the ‹ › page buttons at the right of the status line
const PAGE_BTN: f32 = 22.;

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
            layout: None,
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

    fn act(&mut self, action: Option<Action>, cx: &mut App) {
        match action {
            Some(Action::Copy(text, toast)) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                crate::app::send_event(crate::app::AppEvent::Flash(toast));
            }
            Some(Action::Page(d)) => self.turn_page(d),
            Some(Action::Editor) => self.focus = Focus::Editor,
            None => {}
        }
    }

    fn turn_page(&mut self, d: isize) {
        let Some(db) = &self.db else { return };
        let s = db.shared.lock();
        if s.running.is_some() {
            return;
        }
        let Some(page) = s.last.as_ref().and_then(|r| r.page.as_ref()) else { return };
        let offset = match d {
            1 if page.has_more => page.offset + db::PAGE,
            -1 if page.offset > 0 => page.offset.saturating_sub(db::PAGE),
            _ => return,
        };
        drop(s);
        db.page(offset);
    }

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
        if ks.modifiers.platform && ks.key == "a" {
            self.grid.select_all(set);
            return true;
        }
        let action = self.grid.key(ks, set);
        drop(shared);
        match action {
            Some(a) => {
                self.act(a, cx);
                true
            }
            None => false,
        }
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
        let Some(run) = shared.last.as_ref() else {
            return div().into_any_element();
        };
        let set = match &run.outcome {
            Outcome::Error(e) => return empty_line(e, t.danger, m),
            Outcome::Done(n) => {
                return empty_line(
                    &format!("{n} row{} affected", if *n == 1 { "" } else { "s" }),
                    t.fg_muted,
                    m,
                )
            }
            Outcome::Rows(set) => set,
        };
        self.grid.sync(shared.seq, set);
        let first_row = run.page.as_ref().map_or(0, |p| p.offset);
        self.grid.render(set, first_row, m, t, focused, size)
    }

    fn status_line(&self, m: &Metrics, t: &Theme) -> AnyElement {
        let left = self
            .conn
            .as_ref()
            .map(|c| format!("{} · {}", c.name, c.target()))
            .unwrap_or_default();
        let mut right = div().flex().items_center().gap_2();
        let mut pages: Option<(bool, bool)> = None;
        if let Some(db) = &self.db {
            let s = db.shared.lock();
            if let Some(started) = s.running {
                right = right
                    .child(crate::ui::spinner(11., t.fg_muted))
                    .child(format!("{:.1}s", started.elapsed().as_secs_f32()));
            } else if matches!(s.status, Status::Connecting) {
                right = right.child(crate::ui::spinner(11., t.fg_muted));
            } else if let Some(run) = &s.last {
                let mut parts = vec![];
                if !self.grid.selected.is_empty() {
                    parts.push(format!("{} selected", self.grid.selected.len()));
                }
                match (&run.outcome, &run.page) {
                    (Outcome::Rows(set), Some(p)) if !set.rows.is_empty() => {
                        let (a, b) = (p.offset + 1, p.offset + set.rows.len());
                        parts.push(match p.total {
                            Some(n) => format!("{a}–{b} of {n}"),
                            None if p.has_more => format!("{a}–{b} of many"),
                            None => format!("{a}–{b}"),
                        });
                        pages = Some((p.offset > 0, p.has_more));
                    }
                    (Outcome::Rows(set), Some(p)) if p.offset > 0 => {
                        parts.push(format!("no rows past {}", p.offset));
                        pages = Some((true, false));
                    }
                    (Outcome::Rows(set), _) if set.total > set.rows.len() => {
                        parts.push(format!("{} of {} rows", set.rows.len(), set.total))
                    }
                    (Outcome::Rows(set), _) => {
                        parts.push(format!("{} row{}", set.total, if set.total == 1 { "" } else { "s" }))
                    }
                    _ => {}
                }
                parts.push(format!("{} ms", run.elapsed.as_millis()));
                right = right.child(parts.join(" · "));
            }
        }
        let button = |icon: crate::ui::Icon, enabled: bool| {
            div()
                .w(px(PAGE_BTN))
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .when(enabled, |d| d.hover(|s| s.bg(t.hover)))
                .child(crate::ui::icon(icon, 12., if enabled { t.fg_muted } else { with_alpha(t.fg_subtle, 0.4) }))
        };
        if let Some((prev, next)) = pages {
            right = right.child(
                div()
                    .flex()
                    .child(button(crate::ui::Icon::ChevronLeft, prev))
                    .child(button(crate::ui::Icon::ChevronRight, next)),
            );
        }
        div()
            .h(px(STATUS_H))
            .flex_none()
            .pl(px(10.))
            .pr(px(6.))
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
        self.layout = Some(Layout {
            metrics: m.clone(),
            width: f32::from(vcx.size.width),
            editor_h: f32::from(editor_h),
        });
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
        if !m.platform || m.control {
            return false;
        }
        match self.focus {
            Focus::Editor => matches!(ks.key.as_str(), "enter" | "a" | "z" | "x"),
            Focus::Grid => ks.key == "a",
        }
    }

    fn mouse_down(&mut self, pos: Point<Pixels>, mods: &Modifiers, _clicks: usize, cx: &mut App) {
        let Some(l) = &self.layout else { return };
        let y = f32::from(pos.y);
        if y < l.editor_h {
            self.focus = Focus::Editor;
            self.menu = None;
            let m = l.metrics.clone();
            self.editor.click(&m, pos, mods.shift);
            return;
        }
        if y < l.editor_h + STATUS_H {
            // the ‹ › buttons sit at the right end
            let from_right = l.width - 6. - f32::from(pos.x);
            if (0. ..PAGE_BTN).contains(&from_right) {
                self.act(Some(Action::Page(1)), cx);
            } else if (PAGE_BTN..PAGE_BTN * 2.).contains(&from_right) {
                self.act(Some(Action::Page(-1)), cx);
            }
            return;
        }
        let grid_pos = gpui::point(pos.x, pos.y - px(l.editor_h + STATUS_H));
        let Some(db) = &self.db else { return };
        let s = db.shared.lock();
        if let Some(Outcome::Rows(set)) = s.last.as_ref().map(|r| &r.outcome) {
            self.focus = Focus::Grid;
            self.grid.click(grid_pos, mods, set);
        }
    }

    fn scroll(&mut self, pos: Point<Pixels>, lines: isize) {
        let Some(l) = &self.layout else { return };
        if f32::from(pos.y) < l.editor_h + STATUS_H {
            return;
        }
        let Some(db) = &self.db else { return };
        let s = db.shared.lock();
        if let Some(Outcome::Rows(set)) = s.last.as_ref().map(|r| &r.outcome) {
            self.grid.scroll(lines, set);
        }
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
                    Some(Outcome::Rows(set)) => match self.grid.copy(set, false)? {
                        Action::Copy(text, toast) => {
                            crate::app::send_event(crate::app::AppEvent::Flash(toast));
                            Some(text)
                        }
                        _ => None,
                    },
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
