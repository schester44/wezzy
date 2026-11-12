//! The git side panel's state and keyboard handling: a neogit-like tree of
//! sections (Untracked / Unstaged / Staged) → files → hunks, with a cursor
//! over sections, files and hunks, plus an inline commit composer.
//!
//! Keys (panel focused):
//!   j/k ↓/↑   move (through diff lines too)   ]/[  next/prev hunk or file
//!   g/G       top/bottom    ^d/^u    half page
//!   tab/space fold/unfold   enter    fold, or open hunk in editor
//!   s         toggle staged for file/hunk/section (stages if unstaged, unstages if staged)
//!   S         stage everything; if everything's already staged, unstage it all
//!   d         discard (press d again, y or enter to confirm)
//!   c         commit        a        amend
//!   p         open the branch's pull request, or compose one (push + gh pr create)
//!   f         comment on the hunk     F        send comments to an agent
//!   enter/d   on a comment: edit / delete
//!   f on a line comments on just that line; on a hunk header, the whole hunk
//!   V         select lines from the cursor line (j/k extend, f comments, esc exits)
//!   A         only the files the agent's run changed / everything (runs.rs)
//!   o         open in editor          r        recipes     R  refresh     q  close
//!   w         follow the agent's edits (moving the cursor stops it)
//!   ?         shortcut sheet
//!
//! Commit / comment / pull request boxes are `TextField`s (src/textfield.rs,
//! macOS editing keys and mouse selection): enter submits, shift+enter adds a
//! line, esc cancels (the commit box keeps a draft). ^D toggles a PR's draft.

use crate::app::TermApp;
use crate::diff::{self, Op, Section, Snapshot};
use crate::diff::FileDiff;
use crate::feedback::Comment;
use crate::textfield::{Edit, TextField};
use gpui::{Context, Keystroke, Window};
use mux::pane::{Pane, PaneId};
use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Where the git panel looks for a pane: the directory its agent reports
/// working in (it may have moved into a worktree), else the pane's cwd. A
/// hook record outlives a killed agent, so it only counts while something
/// other than the shell is in the foreground.
pub fn git_dir(pane: &Arc<dyn Pane>) -> Option<PathBuf> {
    crate::hooks::state(pane.pane_id())
        .and_then(|h| h.cwd)
        .filter(|d| d.is_dir() && !crate::procs::can_close_quietly(pane))
        .or_else(|| {
            pane.get_current_working_dir(mux::pane::CachePolicy::AllowStale)
                .map(|u| PathBuf::from(u.path()))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Section(Section),
    File(Section, usize),
    Hunk(Section, usize, usize),
    Line(Section, usize, usize, usize),
    /// "Feedback" header above the tree, when there are comments
    Feedback,
    /// a saved comment (index into `GitPanel::comments`)
    Comment(usize),
    /// the comment being written
    Draft,
}

impl Row {
    pub fn navigable(self) -> bool {
        !matches!(self, Row::Draft)
    }
}

/// Visual line selection inside one hunk (`V`). `anchor` / `head` index the
/// hunk's lines; `head` moves with j/k.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visual {
    pub sec: Section,
    pub fi: usize,
    pub hi: usize,
    pub anchor: usize,
    pub head: usize,
}

impl Visual {
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    pub fn contains(&self, sec: Section, fi: usize, hi: usize, li: usize) -> bool {
        let (lo, hi_) = self.range();
        (self.sec, self.fi, self.hi) == (sec, fi, hi) && (lo..=hi_).contains(&li)
    }
}

/// A comment being written: new (shown under its hunk) or an edit of `editing`.
pub struct CommentDraft {
    pub comment: Comment,
    pub editing: Option<usize>,
    /// the text being written; goes into `comment.text` on save
    pub field: TextField,
}

impl CommentDraft {
    fn new(comment: Comment, editing: Option<usize>) -> Self {
        let field = TextField::new(comment.text.clone());
        Self { comment, editing, field }
    }
}

pub struct Composer {
    pub field: TextField,
    pub amend: bool,
    /// composing a pull request rather than a commit
    pub pr: Option<PrCompose>,
}

pub struct PrCompose {
    pub plan: crate::pr::Plan,
    pub draft: bool,
}

/// A discard waiting for y/n.
pub struct PendingDiscard {
    pub row: Row,
    pub label: String,
}

pub struct GitPanel {
    pub expanded: HashSet<(Section, String)>,
    pub folded: HashSet<Section>,
    pub cursor: usize,
    /// variable-height list; `sync_list` keeps its items in step with `rows()`
    pub list: gpui::ListState,
    list_rows: std::cell::RefCell<Vec<Row>>,
    /// scroll the cursor into view on the next render (after the list is synced)
    reveal: std::cell::Cell<bool>,
    pub composer: Option<Composer>,
    /// message kept when the composer is dismissed
    pub draft: String,
    pub confirm: Option<PendingDiscard>,
    /// the `?` shortcut sheet, with what's typed to filter it
    pub help: Option<String>,
    pub committing: bool,
    /// a pull or push running in the background
    pub syncing: Option<diff::Sync>,
    /// pushing + `gh pr create` in the background
    pub creating_pr: bool,
    /// review comments waiting to be sent (`F`)
    pub comments: Vec<Comment>,
    pub comment_draft: Option<CommentDraft>,
    pub feedback_folded: bool,
    /// scroll the comment box into view on the next render
    reveal_draft: std::cell::Cell<bool>,
    pub visual: Option<Visual>,
    /// following the agent's edits (`w`); see `gitlive`
    pub follow: Option<crate::gitlive::Follow>,
    /// a repaint loop is running for fading markers / glows
    pub ticking: std::cell::Cell<bool>,
    /// show only the files this agent pane's run edited (`A` cycles through
    /// the repo's runs: `TermApp::run_panes_here`)
    pub run_filter: Option<PaneId>,
    /// move the cursor to the first file once the snapshot of this repo lands
    /// (opening a review)
    pub land: Option<PathBuf>,
    /// the commit in flight is of this run's files: end the run when it lands
    pub committing_run: Option<PaneId>,
}

impl Default for GitPanel {
    fn default() -> Self {
        Self {
            expanded: HashSet::new(),
            folded: HashSet::new(),
            cursor: 0,
            list: gpui::ListState::new(0, gpui::ListAlignment::Top, gpui::px(400.)),
            list_rows: Default::default(),
            reveal: Default::default(),
            composer: None,
            draft: String::new(),
            confirm: None,
            help: None,
            committing: false,
            syncing: None,
            creating_pr: false,
            comments: vec![],
            comment_draft: None,
            feedback_folded: false,
            reveal_draft: Default::default(),
            visual: None,
            follow: None,
            ticking: Default::default(),
            run_filter: None,
            land: None,
            committing_run: None,
        }
    }
}

/// The files a run filter lets through (absolute), or `None` for everything.
pub type RunFiles = Option<BTreeSet<PathBuf>>;

/// Whether `f` passes the run filter.
pub fn shown(only: &RunFiles, snap: &Snapshot, f: &FileDiff) -> bool {
    match (only, &snap.root) {
        (None, _) => true,
        (Some(only), Some(root)) => only.contains(&root.join(&f.path)),
        (Some(_), None) => false,
    }
}

impl GitPanel {
    /// Comments on this hunk (matched by path + header, any section).
    pub fn comments_on(&self, path: &str, header: &str) -> usize {
        self.comments
            .iter()
            .filter(|c| c.path == path && c.header == header)
            .count()
    }

    /// The run filter's files, while it's on and the run still exists.
    pub fn run_files(&self) -> RunFiles {
        self.run_filter.and_then(crate::activity::run).map(|r| r.files())
    }

    /// A section's files that pass the run filter, with their indices.
    pub fn section_files<'a>(&self, snap: &'a Snapshot, sec: Section) -> Vec<(usize, &'a FileDiff)> {
        let only = self.run_files();
        snap.files(sec)
            .iter()
            .enumerate()
            .filter(|(_, f)| shown(&only, snap, f))
            .collect()
    }

    pub fn rows(&self, snap: &Snapshot) -> Vec<Row> {
        let mut rows = vec![];
        let only = self.run_files();
        // a new comment's box sits under its hunk while that hunk is on screen
        let new_draft = self
            .comment_draft
            .as_ref()
            .filter(|d| d.editing.is_none())
            .map(|d| &d.comment);
        let mut draft_placed = false;
        for sec in Section::ALL {
            let files = snap.files(sec);
            if !files.iter().any(|f| shown(&only, snap, f)) {
                continue;
            }
            rows.push(Row::Section(sec));
            if self.folded.contains(&sec) {
                continue;
            }
            for (fi, f) in files.iter().enumerate() {
                if !shown(&only, snap, f) {
                    continue;
                }
                rows.push(Row::File(sec, fi));
                if !self.expanded.contains(&(sec, f.path.clone())) {
                    continue;
                }
                for (hi, h) in f.hunks.iter().enumerate() {
                    // untracked files have one synthetic hunk with no header
                    if !h.header.is_empty() {
                        rows.push(Row::Hunk(sec, fi, hi));
                    }
                    rows.extend((0..h.lines.len()).map(|li| Row::Line(sec, fi, hi, li)));
                    if let Some(c) = new_draft {
                        if !draft_placed && c.sec == sec && c.path == f.path && c.header == h.header {
                            rows.push(Row::Draft);
                            draft_placed = true;
                        }
                    }
                }
            }
        }

        // the Feedback section goes on top
        let unplaced = new_draft.is_some() && !draft_placed;
        if self.comments.is_empty() && !unplaced {
            return rows;
        }
        let mut top = vec![Row::Feedback];
        if !self.feedback_folded || self.comment_draft.is_some() {
            let editing = self.comment_draft.as_ref().and_then(|d| d.editing);
            for i in 0..self.comments.len() {
                top.push(if editing == Some(i) { Row::Draft } else { Row::Comment(i) });
            }
            if unplaced {
                top.push(Row::Draft);
            }
        }
        top.extend(rows);
        top
    }

    /// Tell the list which rows changed (only the differing middle, so the
    /// scroll position survives), then reveal the cursor if a key moved it.
    pub fn sync_list(&self, rows: &[Row]) {
        let mut old = self.list_rows.borrow_mut();
        if *old != rows {
            let pre = old.iter().zip(rows).take_while(|(a, b)| a == b).count();
            let max_suf = old.len().min(rows.len()) - pre;
            let suf = old
                .iter()
                .rev()
                .zip(rows.iter().rev())
                .take(max_suf)
                .take_while(|(a, b)| a == b)
                .count();
            self.list.splice(pre..old.len() - suf, rows.len() - pre - suf);
            *old = rows.to_vec();
        }
        if self.reveal.take() {
            self.list.scroll_to_reveal_item(self.cursor.min(rows.len().saturating_sub(1)));
        }
        if self.reveal_draft.take() {
            if let Some(i) = rows.iter().position(|r| *r == Row::Draft) {
                self.list.scroll_to_reveal_item(i);
            }
        }
    }

    /// Scroll the cursor into view on the next render.
    pub fn reveal_cursor(&self) {
        self.reveal.set(true);
    }

    /// Clamp the cursor onto a navigable row.
    pub fn clamp(&mut self, rows: &[Row]) {
        if rows.is_empty() {
            self.cursor = 0;
            return;
        }
        self.cursor = self.cursor.min(rows.len() - 1);
        while self.cursor > 0 && !rows[self.cursor].navigable() {
            self.cursor -= 1;
        }
    }

    fn step(&mut self, rows: &[Row], down: bool, count: usize) {
        let mut moved = 0;
        let mut i = self.cursor;
        let mut last_ok = i;
        while moved < count {
            let next = if down { i.checked_add(1).filter(|n| *n < rows.len()) } else { i.checked_sub(1) };
            let Some(n) = next else { break };
            i = n;
            if rows[i].navigable() {
                last_ok = i;
                moved += 1;
            } else if count > 1 {
                // half-page jumps count every row, not just navigable ones
                moved += 1;
            }
        }
        self.cursor = last_ok;
    }

    fn toggle(&mut self, snap: &Snapshot, rows: &[Row]) {
        match rows[self.cursor] {
            Row::Section(sec) => {
                if !self.folded.remove(&sec) {
                    self.folded.insert(sec);
                }
            }
            Row::File(sec, fi) => {
                let key = (sec, snap.files(sec)[fi].path.clone());
                if !self.expanded.remove(&key) {
                    self.expanded.insert(key);
                }
            }
            // folding from inside a hunk collapses its file and lands on it
            Row::Hunk(sec, fi, _) | Row::Line(sec, fi, ..) => {
                self.expanded.remove(&(sec, snap.files(sec)[fi].path.clone()));
                if let Some(i) = rows.iter().position(|r| *r == Row::File(sec, fi)) {
                    self.cursor = i;
                }
            }
            Row::Feedback => self.feedback_folded = !self.feedback_folded,
            Row::Comment(_) | Row::Draft => {}
        }
    }
}

/// `s` stages what's unstaged and unstages what's staged.
pub fn toggle_op(row: Row) -> Op {
    match row {
        Row::Section(Section::Staged)
        | Row::File(Section::Staged, _)
        | Row::Hunk(Section::Staged, ..)
        | Row::Line(Section::Staged, ..) => Op::Unstage,
        _ => Op::Stage,
    }
}

impl TermApp {
    pub(crate) fn diff_key(&mut self, ks: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.git.cursor;
        self.diff_key_inner(ks, window, cx);
        // moving the cursor yourself stops following the agent
        if self.git.cursor != before && ks.key != "w" {
            self.git.follow = None;
        }
    }

    fn diff_key_inner(&mut self, ks: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snap) = diff::snapshot().filter(|s| s.root.is_some()) else {
            if ks.key == "q" {
                self.close_diff_panel();
            }
            return;
        };

        if self.git.comment_draft.is_some() {
            self.comment_key(ks, cx);
            return;
        }
        if let Some(query) = self.git.help.as_mut() {
            if crate::shortcuts::sheet_key(query, ks, cx) {
                self.git.help = None;
            }
            return;
        }
        if ks.key_char.as_deref() == Some("?") || ks.key == "?" {
            self.git.help = Some(String::new());
            return;
        }
        if self.git.visual.is_some() {
            self.visual_key(ks, &snap);
            // the cursor follows the selection head
            if let Some(v) = self.git.visual {
                let rows = self.git.rows(&snap);
                if let Some(i) = rows.iter().position(|r| *r == Row::Line(v.sec, v.fi, v.hi, v.head)) {
                    self.git.cursor = i;
                }
            }
            self.scroll_to_cursor();
            return;
        }
        if self.git.confirm.is_some() {
            // d again, y or enter confirms; anything else cancels
            self.answer_discard(matches!(ks.key.as_str(), "d" | "y" | "enter"));
            return;
        }

        let rows = self.git.rows(&snap);
        self.git.clamp(&rows);
        let cur = rows.get(self.git.cursor).copied();
        let m = &ks.modifiers;
        if m.control || m.platform || m.alt {
            if m.control && !m.platform && !m.alt && !rows.is_empty() {
                match ks.key.as_str() {
                    "d" => self.git.step(&rows, true, 15),
                    "u" => self.git.step(&rows, false, 15),
                    "n" => self.git.step(&rows, true, 1),
                    "p" => self.git.step(&rows, false, 1),
                    _ => return,
                }
                self.scroll_to_cursor();
            }
            return;
        }
        let key = ks.key.as_str();
        let shift = m.shift;
        match key {
            "j" | "down" if !rows.is_empty() => self.git.step(&rows, true, 1),
            "k" | "up" if !rows.is_empty() => self.git.step(&rows, false, 1),
            "g" if !shift => self.git.cursor = 0,
            "g" | "G" if !rows.is_empty() => self.git.cursor = rows.len() - 1,
            "tab" | "space" | "l" | "h" if cur.is_some() => {
                let is_open = match cur {
                    Some(Row::Section(s)) => !self.git.folded.contains(&s),
                    Some(Row::File(s, fi)) => self.git.expanded.contains(&(s, snap.files(s)[fi].path.clone())),
                    _ => true,
                };
                let wants = match key {
                    "l" => !is_open,
                    "h" => is_open,
                    _ => true,
                };
                if wants {
                    self.git.toggle(&snap, &rows);
                }
            }
            "enter" => match cur {
                Some(Row::Comment(i)) => self.edit_comment(i),
                Some(Row::Hunk(..) | Row::Line(..)) => self.open_cursor_in_editor(&snap, cur, window, cx),
                Some(_) => self.git.toggle(&snap, &rows),
                None => {}
            },
            "o" => self.open_cursor_in_editor(&snap, cur, window, cx),
            "s" | "S" if shift || key == "S" => {
                let all_staged = self.git.section_files(&snap, Section::Unstaged).is_empty()
                    && self.git.section_files(&snap, Section::Untracked).is_empty();
                self.git_all(&snap, if all_staged { Op::Unstage } else { Op::Stage })
            }
            "]" | "[" if !rows.is_empty() => {
                // jump between headers (sections, files, hunks, feedback), skipping lines
                let down = key == "]";
                let mut i = self.git.cursor;
                loop {
                    let next = if down { i.checked_add(1).filter(|n| *n < rows.len()) } else { i.checked_sub(1) };
                    let Some(n) = next else { break };
                    i = n;
                    if !matches!(rows[i], Row::Line(..) | Row::Draft) {
                        self.git.cursor = i;
                        break;
                    }
                }
            }
            "v" | "V" if shift || key == "V" => match cur {
                Some(Row::Line(sec, fi, hi, li)) => {
                    self.git.visual = Some(Visual { sec, fi, hi, anchor: li, head: li });
                }
                Some(Row::Hunk(sec, fi, hi)) => {
                    let lines = &snap.files(sec)[fi].hunks[hi].lines;
                    // start on the first changed line; context above it is rarely the point
                    let first = lines
                        .iter()
                        .position(|l| l.kind != diff::LineKind::Context)
                        .unwrap_or(0);
                    self.git.visual = Some(Visual { sec, fi, hi, anchor: first, head: first });
                    if let Some(i) = rows.iter().position(|r| *r == Row::Line(sec, fi, hi, first)) {
                        self.git.cursor = i;
                    }
                }
                _ => self.flash("Move to a hunk to select lines"),
            },
            "f" | "F" if shift || key == "F" => self.send_feedback(window, cx),
            "f" => match cur {
                Some(Row::Hunk(sec, fi, hi)) => {
                    let f = &snap.files(sec)[fi];
                    self.git.comment_draft = Some(CommentDraft::new(Comment::for_hunk(sec, &f.path, &f.hunks[hi]), None));
                    self.git.reveal_draft.set(true);
                }
                Some(Row::Line(sec, fi, hi, li)) => {
                    let f = &snap.files(sec)[fi];
                    self.git.comment_draft = Some(CommentDraft::new(Comment::for_lines(sec, &f.path, &f.hunks[hi], li, li), None));
                    self.git.reveal_draft.set(true);
                }
                Some(Row::Comment(i)) => self.edit_comment(i),
                _ => self.flash("Expand a file and move to a hunk to comment on it"),
            },
            "s" => match cur {
                Some(Row::Feedback | Row::Comment(_) | Row::Draft) | None => {}
                Some(r) => self.git_op(&snap, toggle_op(r), r),
            },
            "u" | "U" if shift || key == "U" => self.git_all(&snap, Op::Unstage),
            "u" => match cur {
                Some(r) if toggle_op(r) == Op::Unstage => self.git_op(&snap, Op::Unstage, r),
                _ => {}
            },
            "x" => match cur {
                Some(Row::Feedback | Row::Comment(_) | Row::Draft) | None => {}
                Some(r) => self.ask_discard(&snap, r),
            },
            "d" => match cur {
                Some(Row::Comment(i)) => {
                    self.git.comments.remove(i);
                    self.flash("Comment deleted");
                }
                Some(Row::Feedback | Row::Draft) | None => {}
                Some(r) => self.ask_discard(&snap, r),
            },
            "a" | "A" if shift || key == "A" => self.toggle_run_filter(),
            "c" | "a" => self.open_composer(&snap, key == "a"),
            "p" | "P" if shift || key == "P" => self.sync(&snap, diff::Sync::Push),
            "p" => self.sync(&snap, diff::Sync::Pull),
            "n" | "N" if shift || key == "N" => self.pull_request(),
            "r" | "R" if shift || key == "R" => {
                crate::pr::forget(&snap);
                std::thread::spawn(diff::refresh);
            }
            // recipes (review / simplify / …) run against this repo; {file} = cursor file
            "r" => self.open_palette(crate::palette::Scope::Recipes, cx),
            "q" => self.close_diff_panel(),
            "w" => self.toggle_follow(),
            _ => {}
        }
        // re-clamp against the same rows (the snapshot only changes on refresh)
        let rows = self.git.rows(&snap);
        self.git.clamp(&rows);
        self.scroll_to_cursor();
    }

    pub(crate) fn scroll_to_cursor(&self) {
        self.git.reveal.set(true);
    }

    pub(crate) fn close_diff_panel(&mut self) {
        self.diff_open = false;
        self.git.composer = None;
        self.git.comment_draft = None;
        self.git.visual = None;
        self.git.confirm = None;
        self.git.help = None;
        self.git.follow = None;
        self.git.run_filter = None;
        self.git.land = None;
        self.region = crate::regions::Region::Terminal;
    }

    fn open_cursor_in_editor(&mut self, snap: &Snapshot, row: Option<Row>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(root), Some(row)) = (&snap.root, row) else { return };
        let (sec, fi, line) = match row {
            Row::Section(_) | Row::Feedback | Row::Draft => return,
            Row::Comment(i) => {
                let Some(c) = self.git.comments.get(i) else { return };
                let path = root.join(&c.path);
                let line = Some(c.start);
                self.region = crate::regions::Region::Terminal;
                self.open_in_editor(&path, line, None, window, cx);
                return;
            }
            Row::File(s, fi) => (s, fi, None),
            Row::Hunk(s, fi, hi) => {
                let h = &snap.files(s)[fi].hunks[hi];
                // first changed line is a better landing spot than the context above it
                let first = h
                    .lines
                    .iter()
                    .find(|l| l.kind != diff::LineKind::Context)
                    .and_then(|l| l.new.or(l.old));
                (s, fi, first.or(Some(h.new_start)))
            }
            Row::Line(s, fi, hi, li) => {
                let l = &snap.files(s)[fi].hunks[hi].lines[li];
                (s, fi, l.new.or(l.old))
            }
        };
        let path = root.join(&snap.files(sec)[fi].path);
        self.region = crate::regions::Region::Terminal;
        self.open_in_editor(&path, line, None, window, cx);
    }

    /// Stage / unstage / discard whatever the row points at.
    fn git_op(&mut self, snap: &Snapshot, op: Op, row: Row) {
        let Some(root) = &snap.root else { return };
        let result = match row {
            Row::Section(sec) => {
                // a whole section: every file in it (that the run filter shows)
                let mut r = Ok(());
                for (_, f) in self.git.section_files(snap, sec) {
                    r = diff::apply(root, op, sec, f, None, snap.has_head);
                    if r.is_err() {
                        break;
                    }
                }
                r
            }
            Row::File(sec, fi) => diff::apply(root, op, sec, &snap.files(sec)[fi], None, snap.has_head),
            Row::Hunk(sec, fi, hi) | Row::Line(sec, fi, hi, _) => {
                diff::apply(root, op, sec, &snap.files(sec)[fi], Some(hi), snap.has_head)
            }
            Row::Feedback | Row::Comment(_) | Row::Draft => return,
        };
        if let Err(e) = result {
            self.flash(e);
        }
        std::thread::spawn(diff::refresh);
    }

    fn git_all(&mut self, snap: &Snapshot, op: Op) {
        let Some(root) = &snap.root else { return };
        let result = if self.git.run_files().is_some() {
            // filtered to a run: just its files
            let secs: &[Section] = match op {
                Op::Unstage => &[Section::Staged],
                _ => &[Section::Unstaged, Section::Untracked],
            };
            secs.iter()
                .flat_map(|sec| self.git.section_files(snap, *sec).into_iter().map(move |(_, f)| (*sec, f)))
                .try_for_each(|(sec, f)| diff::apply(root, op, sec, f, None, snap.has_head))
        } else {
            diff::apply_all(root, op, snap.has_head)
        };
        if let Err(e) = result {
            self.flash(e);
        }
        std::thread::spawn(diff::refresh);
    }

    pub(crate) fn answer_discard(&mut self, yes: bool) {
        let Some(pending) = self.git.confirm.take() else { return };
        if let (true, Some(snap)) = (yes, diff::snapshot()) {
            self.git_op(&snap, Op::Discard, pending.row);
        }
    }

    fn ask_discard(&mut self, snap: &Snapshot, row: Row) {
        let label = match row {
            Row::Section(Section::Staged) | Row::File(Section::Staged, _) | Row::Hunk(Section::Staged, ..) | Row::Line(Section::Staged, ..) => {
                self.flash("Unstage it first (s), then discard");
                return;
            }
            Row::Section(sec) => format!(
                "all {} {}",
                self.git.section_files(snap, sec).len(),
                sec.title().to_lowercase()
            ),
            Row::File(Section::Untracked, fi) => format!("delete {}", snap.untracked[fi].path),
            Row::File(sec, fi) => format!("changes to {}", snap.files(sec)[fi].path),
            Row::Hunk(sec, fi, _) | Row::Line(sec, fi, ..) if sec == Section::Untracked => {
                format!("delete {}", snap.files(sec)[fi].path)
            }
            Row::Hunk(sec, fi, _) | Row::Line(sec, fi, ..) => format!("hunk in {}", snap.files(sec)[fi].path),
            Row::Feedback | Row::Comment(_) | Row::Draft => return,
        };
        self.git.confirm = Some(PendingDiscard { row, label });
    }

    fn open_composer(&mut self, snap: &Snapshot, amend: bool) {
        if self.git.committing {
            self.flash("Commit in progress…");
            return;
        }
        if amend && !snap.has_head {
            self.flash("Nothing to amend yet");
            return;
        }
        if !amend && snap.staged.is_empty() {
            self.flash("Nothing staged — press s to stage, S for everything");
            return;
        }
        let text = if amend {
            snap.root.as_deref().map(diff::head_message).unwrap_or_default()
        } else {
            std::mem::take(&mut self.git.draft)
        };
        self.git.composer = Some(Composer { field: TextField::new(text), amend, pr: None });
    }

    /// Keys while the commit composer is open. Always consumes the key.
    pub(crate) fn composer_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let Some(c) = self.git.composer.as_mut() else { return };
        if let Some(pr) = c.pr.as_mut() {
            let m = &ks.modifiers;
            if ks.key == "d" && m.control && !m.platform && !m.alt {
                pr.draft = !pr.draft;
                return;
            }
        }
        match c.field.key(ks, cx) {
            Edit::Cancel => {
                let c = self.git.composer.take().expect("open");
                if !c.amend && c.pr.is_none() {
                    self.git.draft = c.field.into_text();
                }
            }
            Edit::Submit if c.pr.is_some() => self.submit_pull_request(),
            Edit::Submit => self.submit_commit(),
            Edit::Changed => {}
        }
    }

    /// Keys during visual line selection. Always consumes the key.
    fn visual_key(&mut self, ks: &Keystroke, snap: &Snapshot) {
        let Some(mut v) = self.git.visual else { return };
        // the hunk can vanish under us (refresh after an agent edit)
        let Some(hunk) = snap.files(v.sec).get(v.fi).and_then(|f| f.hunks.get(v.hi)) else {
            self.git.visual = None;
            return;
        };
        let last = hunk.lines.len().saturating_sub(1);
        let shift = ks.modifiers.shift;
        match ks.key.as_str() {
            "j" | "down" => v.head = (v.head + 1).min(last),
            "k" | "up" => v.head = v.head.saturating_sub(1),
            "g" if !shift => v.head = 0,
            "g" | "G" => v.head = last,
            "f" | "enter" => {
                let (lo, hi) = v.range();
                let path = &snap.files(v.sec)[v.fi].path;
                self.git.comment_draft = Some(CommentDraft::new(Comment::for_lines(v.sec, path, hunk, lo, hi), None));
                self.git.visual = None;
                self.git.reveal_draft.set(true);
                return;
            }
            "escape" | "v" | "V" | "q" => {
                self.git.visual = None;
                return;
            }
            _ => return,
        }
        self.git.visual = Some(v);
    }

    /// Keys while a comment is being written. Always consumes the key.
    pub(crate) fn comment_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) {
        let Some(d) = self.git.comment_draft.as_mut() else { return };
        match d.field.key(ks, cx) {
            Edit::Cancel => self.git.comment_draft = None,
            Edit::Submit => self.save_comment(),
            Edit::Changed => {}
        }
    }

    pub(crate) fn save_comment(&mut self) {
        let Some(mut d) = self.git.comment_draft.take() else { return };
        d.comment.text = d.field.into_text();
        let empty = d.comment.text.trim().is_empty();
        match (d.editing, empty) {
            // clearing an existing comment's text deletes it
            (Some(i), true) if i < self.git.comments.len() => {
                self.git.comments.remove(i);
            }
            (Some(i), false) if i < self.git.comments.len() => self.git.comments[i] = d.comment,
            (_, true) => {}
            (_, false) => self.git.comments.push(d.comment),
        }
    }

    fn edit_comment(&mut self, i: usize) {
        if let Some(c) = self.git.comments.get(i) {
            self.git.comment_draft = Some(CommentDraft::new(c.clone(), Some(i)));
            self.git.reveal_draft.set(true);
        }
    }

    pub(crate) fn submit_commit(&mut self) {
        let Some(snap) = diff::snapshot() else { return };
        let Some(root) = snap.root.clone() else { return };
        let Some(c) = self.git.composer.as_ref() else { return };
        if c.field.text().trim().is_empty() {
            self.flash("Write a commit message first");
            return;
        }
        if !c.amend && snap.staged.is_empty() {
            self.flash("Nothing staged");
            return;
        }
        let c = self.git.composer.take().expect("open");
        self.git.committing = true;
        self.git.committing_run = (!c.amend).then(|| self.commit_ends_run(&snap)).flatten();
        diff::commit(root, c.field.into_text(), c.amend);
    }

    /// `p` / `P`: pull or push the branch in the background (`on_synced`).
    fn sync(&mut self, snap: &Snapshot, op: diff::Sync) {
        let Some(root) = snap.root.clone() else { return };
        if self.git.syncing.is_some() || self.git.creating_pr {
            return;
        }
        self.git.syncing = Some(op);
        diff::sync(root, op);
    }

    pub(crate) fn on_synced(&mut self, op: diff::Sync, result: Result<String, String>) {
        self.git.syncing = None;
        match result {
            Ok(summary) => self.flash(summary),
            Err(e) => self.flash(match op {
                diff::Sync::Pull => format!("Pull failed: {e}"),
                diff::Sync::Push => format!("Push failed: {e}"),
            }),
        }
        std::thread::spawn(diff::refresh);
    }

    pub(crate) fn on_committed(&mut self, result: Result<String, (String, String)>, amend: bool) {
        self.git.committing = false;
        if result.is_err() {
            self.git.committing_run = None;
        }
        match result {
            Ok(summary) => {
                self.git.draft.clear();
                self.flash(format!("{} {summary}", if amend { "Amended" } else { "Committed" }));
                // the run's work is in a commit: it's reviewed
                if let Some(pane) = self.git.committing_run.take() {
                    self.end_run(pane);
                }
            }
            Err((err, text)) => {
                // reopen with the message so nothing is lost
                self.git.composer = Some(Composer { field: TextField::new(text), amend, pr: None });
                self.flash(format!("Commit failed: {err}"));
            }
        }
        std::thread::spawn(diff::refresh);
    }
}

impl TermApp {
    /// `N`: open the branch's pull request, or start composing one. The
    /// checks shell out, so they run in the background (`on_pull_request`).
    pub(crate) fn pull_request(&mut self) {
        if self.git.creating_pr {
            self.flash("Creating pull request…");
            return;
        }
        crate::pr::begin();
    }

    /// The palette's "Create Pull Request": open the git panel on the active
    /// pane's repo, then do what `N` does.
    pub(crate) fn pull_request_command(&mut self) {
        let Some(dir) = self.active_pane().and_then(|p| git_dir(&p)) else {
            self.flash("Not a git repository");
            return;
        };
        self.diff_open = true;
        self.sidebar_open = false;
        self.region = crate::regions::Region::Diff;
        // render would set it too; set it now so `begin` refreshes the right repo
        diff::set_target(Some(dir));
        self.pull_request();
    }

    pub(crate) fn on_pull_request(&mut self, ev: crate::pr::PrEvent, cx: &mut Context<Self>) {
        use crate::pr::PrEvent;
        match ev {
            PrEvent::Open(url) => cx.open_url(&url),
            PrEvent::Error(e) => self.flash(e),
            PrEvent::Compose(plan) => {
                if self.git.composer.is_some() {
                    return;
                }
                self.git.composer = Some(Composer {
                    field: TextField::new(plan.title.clone()),
                    amend: false,
                    pr: Some(PrCompose { plan, draft: false }),
                });
            }
            PrEvent::Created { url, draft } => {
                self.git.creating_pr = false;
                let n = crate::pr::short_ref(&url);
                self.flash(if draft { format!("Created draft {n}") } else { format!("Created {n}") });
                std::thread::spawn(diff::refresh);
            }
            PrEvent::Failed { error, plan, text, draft } => {
                self.git.creating_pr = false;
                self.git.composer = Some(Composer {
                    field: TextField::new(text),
                    amend: false,
                    pr: Some(PrCompose { plan, draft }),
                });
                self.flash(format!("Pull request failed: {error}"));
                std::thread::spawn(diff::refresh);
            }
        }
    }

    pub(crate) fn submit_pull_request(&mut self) {
        let Some(c) = self.git.composer.as_ref() else { return };
        if crate::pr::split_message(c.field.text()).0.is_empty() {
            self.flash("Write a title first");
            return;
        }
        let c = self.git.composer.take().expect("open");
        let Some(pr) = c.pr else { return };
        self.git.creating_pr = true;
        crate::pr::create(pr.plan, c.field.into_text(), pr.draft);
    }
}
