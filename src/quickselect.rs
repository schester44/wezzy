//! QuickSelect hints and ⌘-click links.
//!
//! Both scan the visible text with the same patterns: URLs, paths (with
//! optional :line:col), git hashes, IPs, long numbers, plus the config's
//! `quick_select_patterns`.
//!
//! QuickSelect: every match gets a label; type it to copy (lowercase) or to
//! copy + paste into the pane (uppercase). Esc cancels.
//!
//! Error select (⌘⇧E) uses the same labels, but each hint is an error and
//! selecting it copies the whole block around it: the error line through the
//! end of its paragraph (rustc's `-->` and code lines, a Python traceback, a
//! panic message), stopping at a blank line, the next error, or a prompt.

use crate::app::TermApp;
use gpui::{ClipboardItem, Context, KeyDownEvent, Window};
use mux::pane::{CachePolicy, Pane, PaneId};
use regex::Regex;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use wezterm_term::{SemanticType, StableRowIndex};

const DEFAULT_PATTERNS: &[&str] = &[
    // URLs
    r#"\bhttps?://[^\s<>"'`]+[^\s<>"'`.,;:)\]}]"#,
    // paths with a slash, optional :line(:col)
    r"(?:~|\.{1,2})?(?:/?[\w.@+\-]+)*/[\w.@+\-]+(?::\d+(?::\d+)?)?",
    // bare file.ext:line(:col)
    r"\b[\w.\-]+\.[A-Za-z][\w]{0,7}:\d+(?::\d+)?\b",
    // git hashes
    r"\b[0-9a-f]{7,40}\b",
    // IPv4 (+port)
    r"\b\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?\b",
    // UUIDs
    r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
    // long numbers
    r"\b\d{4,}\b",
];

const LINK_PATTERNS: &[usize] = &[0, 1, 2];

fn default_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        DEFAULT_PATTERNS
            .iter()
            .map(|p| Regex::new(p).expect("valid pattern"))
            .collect()
    })
}

fn regexes(extra: &[String]) -> Vec<Regex> {
    let config = config::configuration();
    let mut out: Vec<Regex> = extra
        .iter()
        .chain(config.quick_select_patterns.iter())
        .filter_map(|p| Regex::new(p).ok())
        .collect();
    if !config.disable_default_quick_select_patterns {
        out.extend(default_regexes().iter().cloned());
    }
    out
}

/// Lines that start an error block.
const ERROR_PATTERNS: &[&str] = &[
    r"(?i)\berror\b",
    r"\w+(?:Error|Exception)\b",
    r"(?i)\b(?:exception|fatal)\b",
    r"^\s*Traceback \(most recent call last\)",
    r"\bpanicked at\b",
    r"\bFAIL(?:ED)?\b",
];

/// Longest block, in logical lines (a log full of errors shouldn't become one).
const MAX_BLOCK: usize = 60;
/// Rows read past each edge of the viewport so blocks aren't cut at the screen edge.
const CONTEXT_ROWS: StableRowIndex = 100;

fn error_regexes() -> &'static [Regex] {
    static RE: OnceLock<Vec<Regex>> = OnceLock::new();
    RE.get_or_init(|| {
        ERROR_PATTERNS
            .iter()
            .map(|p| Regex::new(p).expect("valid pattern"))
            .collect()
    })
}

#[derive(Clone, Debug)]
pub struct Match {
    /// row within the pane viewport
    pub row: usize,
    pub col: usize,
    pub cells: usize,
    pub text: String,
    pub label: String,
}

pub struct QuickSelect {
    pub pane_id: PaneId,
    pub matches: Vec<Match>,
    pub typed: String,
}

/// A viewport row as text, plus the cell column of every byte offset.
pub(crate) fn row_text(line: &wezterm_term::Line) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut cols = vec![];
    for cell in line.visible_cells() {
        let s = cell.str();
        for _ in 0..s.len() {
            cols.push(cell.cell_index());
        }
        text.push_str(s);
    }
    cols.push(cols.last().map(|c| c + 1).unwrap_or(0));
    (text, cols)
}

fn viewport_lines(pane: &Arc<dyn Pane>, top: Option<StableRowIndex>) -> Vec<wezterm_term::Line> {
    let dims = pane.get_dimensions();
    let top = top.unwrap_or(dims.physical_top);
    pane.get_lines(top..top + dims.viewport_rows as StableRowIndex).1
}

/// Non-overlapping matches, earlier patterns win.
fn find(lines: &[wezterm_term::Line], res: &[Regex]) -> Vec<Match> {
    let mut out: Vec<Match> = vec![];
    for (row, line) in lines.iter().enumerate() {
        let (text, cols) = row_text(line);
        let mut taken: Vec<(usize, usize)> = vec![];
        for re in res {
            for m in re.find_iter(&text) {
                let (s, e) = (m.start(), m.end());
                if e - s < 3 || taken.iter().any(|(a, b)| s < *b && e > *a) {
                    continue;
                }
                taken.push((s, e));
                let col = cols[s];
                let end_col = cols[e.min(cols.len() - 1)];
                out.push(Match {
                    row,
                    col,
                    cells: end_col.saturating_sub(col).max(1),
                    text: m.as_str().to_string(),
                    label: String::new(),
                });
            }
        }
    }
    out.sort_by_key(|m| (m.row, m.col));
    out
}

/// Nearest-to-bottom matches get the first (easiest) letters.
fn assign_labels(matches: &mut [Match], alphabet: &str) {
    let labels = labels(matches.len(), alphabet);
    for (m, label) in matches.iter_mut().rev().zip(labels) {
        m.label = label;
    }
}

/// `n` equal-length labels from `alphabet`'s lowercase letters, so none is a
/// prefix of another. Also used by PaneSelect.
pub(crate) fn labels(n: usize, alphabet: &str) -> Vec<String> {
    let letters: Vec<char> = alphabet.chars().filter(|c| c.is_ascii_lowercase()).collect();
    let letters = if letters.len() < 2 {
        "asdfqwerzxcvjklmiuopghtybn".chars().collect()
    } else {
        letters
    };
    let mut len = 1;
    while letters.len().pow(len as u32) < n {
        len += 1;
    }
    (0..n)
        .map(|i| {
            let mut label = String::new();
            let mut k = i;
            for _ in 0..len {
                label.insert(0, letters[k % letters.len()]);
                k /= letters.len();
            }
            label
        })
        .collect()
}

/// A wrapped run of rows, as one line of text.
struct Logical {
    text: String,
    /// first row (index into the rows it was built from)
    row: usize,
    /// last row
    last: usize,
    /// shell prompt or typed command (OSC 133): ends a block
    prompt: bool,
}

fn logical_lines(rows: &[(String, bool, bool)]) -> Vec<Logical> {
    let mut out: Vec<Logical> = vec![];
    let mut joining = false;
    for (i, (text, wrapped, prompt)) in rows.iter().enumerate() {
        match out.last_mut() {
            Some(l) if joining => {
                l.text.push_str(text);
                l.last = i;
                l.prompt |= *prompt;
            }
            _ => out.push(Logical { text: text.clone(), row: i, last: i, prompt: *prompt }),
        }
        joining = *wrapped;
    }
    for l in &mut out {
        l.text.truncate(l.text.trim_end().len());
    }
    out
}

/// Error blocks as (first, last) logical-line indices.
fn error_blocks(lines: &[Logical]) -> Vec<(usize, usize)> {
    let res = error_regexes();
    let anchor = |l: &Logical| !l.prompt && res.iter().any(|re| re.is_match(&l.text));
    let top_level = |l: &Logical| !l.text.starts_with(char::is_whitespace);
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        if !anchor(&lines[i]) {
            i += 1;
            continue;
        }
        // a traceback runs through its exception line, which is itself an error
        let mut traceback = lines[i].text.trim_start().starts_with("Traceback");
        let mut end = i;
        for (j, l) in lines.iter().enumerate().skip(i + 1) {
            if j - i >= MAX_BLOCK || l.prompt || l.text.trim().is_empty() {
                break;
            }
            if top_level(l) && anchor(l) {
                if !traceback {
                    break;
                }
                traceback = false;
            }
            end = j;
        }
        out.push((i, end));
        i = end + 1;
    }
    out
}

impl TermApp {
    /// QuickSelect over error blocks: the label copies the whole block.
    pub(crate) fn start_error_select(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else {
            return;
        };
        let dims = pane.get_dimensions();
        let top = self
            .viewports
            .get(&pane.pane_id())
            .copied()
            .unwrap_or(dims.physical_top);
        let bottom = dims.physical_top + dims.viewport_rows as StableRowIndex;
        let start = (top - CONTEXT_ROWS).max(dims.scrollback_top);
        let end = (top + dims.viewport_rows as StableRowIndex + CONTEXT_ROWS).min(bottom);
        let (first, lines) = pane.get_lines(start..end);
        let is_prompt = |l: &wezterm_term::Line| {
            l.visible_cells()
                .any(|c| matches!(c.attrs().semantic_type(), SemanticType::Prompt | SemanticType::Input))
        };
        let rows: Vec<(String, bool, bool)> = lines
            .iter()
            .map(|l| (row_text(l).0, l.last_cell_was_wrapped(), is_prompt(l)))
            .collect();
        let logical = logical_lines(&rows);
        // viewport rows, as indices into `rows`
        let vis_start = (top - first).max(0) as usize;
        let vis_end = vis_start + dims.viewport_rows;

        let mut matches = vec![];
        for (a, b) in error_blocks(&logical) {
            let (row0, row1) = (logical[a].row, logical[b].last);
            if row1 < vis_start || row0 >= vis_end {
                continue;
            }
            let row = row0.max(vis_start);
            let (text, cols) = row_text(&lines[row]);
            let s = text.len() - text.trim_start().len();
            let e = text.trim_end().len().max(s);
            let col = cols[s];
            matches.push(Match {
                row: row - vis_start,
                col,
                cells: cols[e].saturating_sub(col).max(1),
                text: logical[a..=b]
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
                label: String::new(),
            });
        }
        if matches.is_empty() {
            self.flash("No errors");
            cx.notify();
            return;
        }
        assign_labels(&mut matches, &config::configuration().quick_select_alphabet);
        self.quick_select = Some(QuickSelect {
            pane_id: pane.pane_id(),
            matches,
            typed: String::new(),
        });
        cx.notify();
    }

    pub(crate) fn start_quick_select(&mut self, extra_patterns: &[String], cx: &mut Context<Self>) {
        let Some(pane) = self.active_pane() else {
            return;
        };
        let lines = viewport_lines(&pane, self.viewports.get(&pane.pane_id()).copied());
        let mut matches = find(&lines, &regexes(extra_patterns));
        if matches.is_empty() {
            self.flash("Nothing to select");
            cx.notify();
            return;
        }
        assign_labels(&mut matches, &config::configuration().quick_select_alphabet);
        self.quick_select = Some(QuickSelect {
            pane_id: pane.pane_id(),
            matches,
            typed: String::new(),
        });
        cx.notify();
    }

    pub(crate) fn quick_select_key(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        let Some(qs) = self.quick_select.as_mut() else {
            return;
        };
        if ks.key == "escape" || ks.modifiers.control || ks.modifiers.platform {
            self.quick_select = None;
            return;
        }
        if ks.key == "backspace" {
            qs.typed.pop();
            return;
        }
        let Some(c) = ks.key.chars().next().filter(|c| ks.key.len() == 1 && c.is_ascii_alphabetic())
        else {
            return;
        };
        let paste = ks.modifiers.shift;
        qs.typed.push(c.to_ascii_lowercase());
        let hit = qs.matches.iter().find(|m| m.label == qs.typed).cloned();
        let any_prefix = qs.matches.iter().any(|m| m.label.starts_with(&qs.typed));
        if let Some(m) = hit {
            let pane_id = qs.pane_id;
            self.quick_select = None;
            cx.write_to_clipboard(ClipboardItem::new_string(m.text.clone()));
            if paste {
                if let Some(pane) = mux::Mux::get().get_pane(pane_id) {
                    pane.send_paste(&m.text).ok();
                }
            } else {
                match m.text.lines().count() {
                    n if n > 1 => self.flash(format!("Copied {n} lines")),
                    _ => self.flash(format!("Copied {}", m.text)),
                }
            }
        } else if !any_prefix {
            self.quick_select = None;
        }
    }

    /// ⌘-click: open the URL or file:line under the mouse.
    pub(crate) fn open_link_at(
        &mut self,
        pane: &Arc<dyn Pane>,
        row: usize,
        col: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let lines = viewport_lines(pane, self.viewports.get(&pane.pane_id()).copied());
        let Some(line) = lines.get(row) else {
            return false;
        };
        let res: Vec<Regex> = LINK_PATTERNS
            .iter()
            .map(|i| default_regexes()[*i].clone())
            .collect();
        let Some(m) = find(std::slice::from_ref(line), &res)
            .into_iter()
            .find(|m| col >= m.col && col < m.col + m.cells)
        else {
            return false;
        };
        if m.text.starts_with("http://") || m.text.starts_with("https://") {
            cx.open_url(&m.text);
            return true;
        }
        let (path, line_no, col_no) = split_location(&m.text);
        let cwd = pane
            .get_current_working_dir(CachePolicy::AllowStale)
            .map(|u| PathBuf::from(u.path()));
        let full = resolve(&path, cwd.as_deref());
        match full {
            Some(p) if p.is_dir() => {
                cx.open_url(&format!("file://{}", p.display()));
                true
            }
            Some(p) => {
                self.open_in_editor(&p, line_no, col_no, window, cx);
                true
            }
            None => {
                self.flash(format!("Not found: {path}"));
                cx.notify();
                true
            }
        }
    }
}

/// "src/app.rs:42:7" -> ("src/app.rs", Some(42), Some(7))
fn split_location(s: &str) -> (String, Option<u32>, Option<u32>) {
    let mut parts: Vec<&str> = s.rsplitn(3, ':').collect();
    parts.reverse();
    match parts.as_slice() {
        [p, l, c] if l.parse::<u32>().is_ok() && c.parse::<u32>().is_ok() => {
            (p.to_string(), l.parse().ok(), c.parse().ok())
        }
        [p, l] if l.parse::<u32>().is_ok() => (p.to_string(), l.parse().ok(), None),
        [a, b, l] if l.parse::<u32>().is_ok() => (format!("{a}:{b}"), l.parse().ok(), None),
        _ => (s.to_string(), None, None),
    }
}

fn resolve(path: &str, cwd: Option<&std::path::Path>) -> Option<PathBuf> {
    let p = if let Some(rest) = path.strip_prefix("~/") {
        config::HOME_DIR.join(rest)
    } else {
        let p = PathBuf::from(path);
        match (p.is_absolute(), cwd) {
            (true, _) => p,
            (false, Some(cwd)) => cwd.join(p),
            (false, None) => return None,
        }
    };
    p.exists().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(line: &str) -> Vec<String> {
        let mut taken: Vec<(usize, usize)> = vec![];
        let mut out = vec![];
        for re in default_regexes() {
            for m in re.find_iter(line) {
                if m.end() - m.start() < 3 || taken.iter().any(|(a, b)| m.start() < *b && m.end() > *a) {
                    continue;
                }
                taken.push((m.start(), m.end()));
                out.push(m.as_str().to_string());
            }
        }
        out
    }

    #[test]
    fn finds_compiler_locations() {
        let h = hits("error[E0308]: mismatched types --> src/palette.rs:120:23");
        assert!(h.contains(&"src/palette.rs:120:23".to_string()), "{h:?}");
    }

    #[test]
    fn finds_bare_file_line_and_urls() {
        let h = hits("at main.rs:42 see https://example.com/a?b=1.");
        assert!(h.contains(&"main.rs:42".to_string()), "{h:?}");
        assert!(h.contains(&"https://example.com/a?b=1".to_string()), "{h:?}");
    }

    #[test]
    fn finds_hashes() {
        let h = hits("commit e68a8ec27727fbbda15e07f0a0737cd5e16481f6 (HEAD)");
        assert!(h.iter().any(|x| x.starts_with("e68a8ec")), "{h:?}");
    }

    #[test]
    fn splits_locations() {
        assert_eq!(split_location("src/a.rs:4:2"), ("src/a.rs".into(), Some(4), Some(2)));
        assert_eq!(split_location("src/a.rs:4"), ("src/a.rs".into(), Some(4), None));
        assert_eq!(split_location("src/a.rs"), ("src/a.rs".into(), None, None));
    }

    fn blocks(text: &str) -> Vec<String> {
        let rows: Vec<(String, bool, bool)> = text
            .lines()
            .map(|l| (l.to_string(), false, l.starts_with('$')))
            .collect();
        let ls = logical_lines(&rows);
        error_blocks(&ls)
            .into_iter()
            .map(|(a, b)| ls[a..=b].iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n"))
            .collect()
    }

    #[test]
    fn rustc_error_block() {
        let b = blocks(
            "   Compiling wezzy v0.1.0\nerror[E0308]: mismatched types\n  --> src/a.rs:1:2\n   |\n12 |     x\n\nerror: could not compile `wezzy`\n$ ",
        );
        assert_eq!(b.len(), 2, "{b:?}");
        assert!(b[0].starts_with("error[E0308]") && b[0].ends_with("12 |     x"), "{b:?}");
        assert_eq!(b[1], "error: could not compile `wezzy`");
    }

    #[test]
    fn python_traceback_is_one_block() {
        let b = blocks(
            "Traceback (most recent call last):\n  File \"a.py\", line 1, in <module>\n    f()\nValueError: bad\nnext line\nKeyError: other",
        );
        assert_eq!(b.len(), 2, "{b:?}");
        assert!(b[0].starts_with("Traceback") && b[0].ends_with("next line"), "{b:?}");
        assert_eq!(b[1], "KeyError: other");
    }

    #[test]
    fn wrapped_rows_join() {
        let rows = vec![
            ("error: a very long".to_string(), true, false),
            (" message".to_string(), false, false),
            ("  detail".to_string(), false, false),
        ];
        let ls = logical_lines(&rows);
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].text, "error: a very long message");
        assert_eq!(error_blocks(&ls), vec![(0, 1)]);
    }

    #[test]
    fn labels_are_prefix_free() {
        let mut ms: Vec<Match> = (0..40)
            .map(|i| Match { row: i, col: 0, cells: 3, text: String::new(), label: String::new() })
            .collect();
        assert_labels(&mut ms);
    }

    fn assert_labels(ms: &mut [Match]) {
        assign_labels(ms, "asdfqwerzxcvjklmiuopghtybn");
        for a in ms.iter() {
            for b in ms.iter() {
                if !std::ptr::eq(a, b) {
                    assert!(!b.label.starts_with(&a.label), "{} / {}", a.label, b.label);
                }
            }
        }
    }
}
