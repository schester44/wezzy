//! The git side panel: header, optional commit composer, the change tree
//! (sections → files → hunk cards), and a footer with context key hints.
//!
//! Visual rules: neutral chrome, color only where it carries meaning (diff
//! lines, status letters, +/− counts), hairline borders, generous rhythm.

use super::widgets::{kbd, term_family, with_alpha};
use super::{icon, spinner, Icon, Theme};
use crate::app::TermApp;
use crate::diff::{LineKind, Section, Snapshot};
use crate::gitpanel::{GitPanel, Row};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, Hsla, SharedString,
};
use std::sync::Arc;

/// Width of the line-number gutter (incl. the left bar) in expanded diffs.
const GUTTER_W: f32 = 58.;

impl TermApp {
    pub(super) fn diff_panel(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let snap = crate::diff::snapshot().filter(|s| s.root.is_some());
        let focused = self.region == crate::regions::Region::Diff;
        let panel = self.side_panel(super::panel::SidePanel::Git, t, cx);

        let Some(snap) = snap else {
            let empty = div().child(empty_state(Icon::GitBranch, "Not a git repository", None, t));
            return panel.child(self.side_card(focused, t, empty));
        };

        let rows = Arc::new(self.git.rows(&snap));
        self.git.sync_list(&rows);
        let cursor = self.git.cursor.min(rows.len().saturating_sub(1));
        let cur_row = rows.get(cursor).copied();

        let body = if snap.is_clean() {
            empty_state(Icon::Check, "No changes", Some("Working tree clean"), t).into_any_element()
        } else {
            let (snap, rows, tc, mono) = (snap.clone(), rows.clone(), *t, term_family());
            gpui::list(
                self.git.list.clone(),
                cx.processor(move |this, i: usize, _, cx| {
                    let ctx = RowCtx {
                        git: &this.git,
                        snap: &snap,
                        rows: &rows,
                        cur_row,
                        cursor,
                        focused,
                        mono: &mono,
                        t: &tc,
                    };
                    ctx.render(i, cx)
                }),
            )
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        let content = div()
            .child(self.git_header(&snap, t, cx))
            .children(self.git_composer(&snap, t, cx))
            .child(body)
            .when(
                self.git.confirm.is_some()
                    || self.git.committing
                    || self.git.syncing.is_some()
                    || self.git.creating_pr
                    || self.git.visual.is_some(),
                |d| {
                d.child(self.git_footer(t, cx))
                },
            );
        panel.child(self.side_card(focused, t, content))
    }

    fn git_header(&self, snap: &Snapshot, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (adds, dels) = Section::ALL
            .iter()
            .flat_map(|s| snap.files(*s))
            .fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        let repo = snap
            .root
            .as_ref()
            .and_then(|r| r.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let (hash, subject) = snap.head.split_once(' ').unwrap_or((&snap.head, ""));
        let pr = crate::pr::cached(snap);
        div()
            .flex_none()
            .px_4()
            .pt(px(14.))
            .pb_3()
            .flex()
            .flex_col()
            .gap_1p5()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(icon(Icon::GitBranch, 13., t.fg_subtle))
                    .child(div().text_color(t.fg_muted).child(repo))
                    .child(div().text_color(t.fg_subtle).child("/"))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::MEDIUM)
                            .child(snap.branch.clone()),
                    )
                    .children(tracking(snap, t))
                    .children(pr.map(|pr| pr_chip(pr, t, cx)))
                    .child(div().flex_1())
                    .when(adds + dels > 0, |d| d.child(counts(adds, dels, t)))
                    .child(
                        div()
                            .id("git-help")
                            .ml_1()
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border_1()
                            .border_color(t.border)
                            .text_size(px(10.))
                            .text_color(t.fg_subtle)
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.fg).bg(t.hover))
                            .child("?")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.git.help = true;
                                this.region = crate::regions::Region::Diff;
                                this.focus.focus(window);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(12.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(icon(Icon::GitCommit, 13., t.fg_subtle))
                    .map(|d| {
                        if snap.head.is_empty() {
                            d.child(div().text_color(t.fg_subtle).child("No commits yet"))
                        } else {
                            d.child(
                                div()
                                    .flex_none()
                                    .font_family(term_family())
                                    .text_size(px(11.))
                                    .text_color(t.fg_subtle)
                                    .child(hash.to_string()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(t.fg_muted)
                                    .child(subject.to_string()),
                            )
                        }
                    }),
            )
    }

    fn git_composer(&self, snap: &Snapshot, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let c = self.git.composer.as_ref()?;
        let subject_len = c.text.lines().next().unwrap_or("").chars().count();
        let mut lines: Vec<&str> = c.text.split('\n').collect();
        let hidden = lines.len().saturating_sub(10);
        lines.drain(..hidden);
        let n = lines.len();
        let caret = || div().ml_px().w(px(1.5)).h(px(15.)).flex_none().bg(t.accent);
        let context = if let Some(pr) = &c.pr {
            let base = pr.plan.base.as_deref().unwrap_or("");
            match pr.plan.commits {
                Some(n) => format!("{n} commit{} → {base}", if n == 1 { "" } else { "s" }),
                None => format!("{} → {base}", pr.plan.branch),
            }
        } else if c.amend {
            format!("Amending {}", snap.head.split(' ').next().unwrap_or(""))
        } else {
            let k = snap.staged.len();
            format!("{k} file{} staged", if k == 1 { "" } else { "s" })
        };
        Some(
            div()
                .flex_none()
                .m_3()
                .flex()
                .flex_col()
                .rounded_lg()
                .bg(gpui::hsla(0., 0., 0., 0.2))
                .border_1()
                .border_color(with_alpha(t.accent, 0.55))
                .child(
                    div()
                        .px_3()
                        .pt_2p5()
                        .pb_2()
                        .min_h(px(64.))
                        .flex()
                        .flex_col()
                        .map(|d| {
                            if c.text.is_empty() {
                                d.child(
                                    div()
                                        .h(px(18.))
                                        .flex()
                                        .items_center()
                                        .child(caret())
                                        .child(div().ml_1().text_color(t.fg_subtle).child(if c.pr.is_some() {
                                            "Pull request title"
                                        } else {
                                            "Commit message"
                                        })),
                                )
                            } else {
                                d.children(lines.into_iter().enumerate().map(|(i, l)| {
                                    div()
                                        .min_h(px(18.))
                                        .flex()
                                        .items_center()
                                        .when(i == 0 && hidden == 0, |d| d.font_weight(FontWeight::MEDIUM))
                                        .when(i > 0 || hidden > 0, |d| d.text_color(t.fg_muted))
                                        .child(SharedString::from(l.to_string()))
                                        .when(i + 1 == n, |d| d.child(caret()))
                                }))
                            }
                        }),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_t_1()
                        .border_color(t.border)
                        .text_size(px(12.))
                        .text_color(t.fg_subtle)
                        .child(div().flex_1().child(context))
                        .when(subject_len > 50 && c.pr.is_none(), |d| {
                            d.child(
                                div()
                                    .font_family(term_family())
                                    .text_size(px(11.))
                                    .text_color(if subject_len > 72 { t.warning } else { t.fg_subtle })
                                    .child(subject_len.to_string()),
                            )
                        })
                        .children(c.pr.as_ref().map(|pr| {
                            let draft = pr.draft;
                            div()
                                .id("pr-draft")
                                .h(px(24.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .rounded_md()
                                .cursor_pointer()
                                .border_1()
                                .border_color(t.border)
                                .text_color(if draft { t.fg } else { t.fg_muted })
                                .hover(|s| s.bg(t.hover))
                                .when(draft, |d| d.child(icon(Icon::Check, 11., t.fg)))
                                .child("Draft")
                                .child(div().text_color(t.fg_subtle).child("^D"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if let Some(pr) = this.git.composer.as_mut().and_then(|c| c.pr.as_mut()) {
                                        pr.draft = !pr.draft;
                                    }
                                    cx.notify();
                                }))
                        }))
                        .child(
                            div()
                                .id("commit-submit")
                                .h(px(24.))
                                .px_2p5()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(with_alpha(t.fg, 0.07))
                                .border_1()
                                .border_color(t.border)
                                .text_color(t.fg)
                                .font_weight(FontWeight::MEDIUM)
                                .hover(|s| s.bg(with_alpha(t.fg, 0.12)))
                                .child(if c.pr.is_some() {
                                    "Create"
                                } else if c.amend {
                                    "Amend"
                                } else {
                                    "Commit"
                                })
                                .child(div().text_color(t.fg_subtle).child("↩"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    if this.git.composer.as_ref().is_some_and(|c| c.pr.is_some()) {
                                        this.submit_pull_request();
                                    } else {
                                        this.submit_commit();
                                    }
                                    cx.notify();
                                })),
                        ),
                ),
        )
    }

    /// Only shown for a pending discard or a running commit.
    fn git_footer(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let footer = div()
            .flex_none()
            .h(px(36.))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_t_1()
            .border_color(t.border)
            .text_size(px(12.))
            .text_color(t.fg_subtle)
            .overflow_hidden()
            .whitespace_nowrap();

        if let Some(p) = &self.git.confirm {
            let choice = |id: &'static str, label: &'static str, key: &'static str, danger: bool| {
                div()
                    .id(id)
                    .flex_none()
                    .h(px(24.))
                    .px_2p5()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .font_weight(FontWeight::MEDIUM)
                    .map(|d| {
                        if danger {
                            d.bg(t.danger).text_color(gpui::white()).hover(|s| s.opacity(0.88))
                        } else {
                            d.border_1()
                                .border_color(t.border)
                                .text_color(t.fg)
                                .hover(|s| s.bg(t.hover))
                        }
                    })
                    .child(label)
                    .child(div().opacity(0.55).child(key))
            };
            return footer
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(t.fg)
                        .child(format!("Discard {}?", p.label)),
                )
                .child(choice("discard-no", "Cancel", "esc", false).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.answer_discard(false);
                        cx.notify();
                    },
                )))
                .child(choice("discard-yes", "Discard", "d", true).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        this.answer_discard(true);
                        cx.notify();
                    },
                )));
        }
        if self.git.committing {
            return footer.gap_2().child(spinner(12., t.fg_muted)).child("Committing…");
        }
        if let Some(op) = self.git.syncing {
            let label = match op {
                crate::diff::Sync::Pull => "Pulling…",
                crate::diff::Sync::Push => "Pushing…",
            };
            return footer.gap_2().child(spinner(12., t.fg_muted)).child(label);
        }
        if self.git.creating_pr {
            return footer.gap_2().child(spinner(12., t.fg_muted)).child("Creating pull request…");
        }
        if let Some(v) = self.git.visual {
            let (lo, hi) = v.range();
            let n = hi - lo + 1;
            return footer
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .text_color(t.fg)
                        .child(format!("{n} line{} selected", if n == 1 { "" } else { "s" })),
                )
                .child(kbd("f", t))
                .child("comment")
                .child(div().w_1())
                .child(kbd("esc", t))
                .child("cancel");
        }
        footer
    }
}

/// Everything a row needs to render itself.
struct RowCtx<'a> {
    git: &'a GitPanel,
    snap: &'a Snapshot,
    rows: &'a [Row],
    cur_row: Option<Row>,
    cursor: usize,
    focused: bool,
    mono: &'a SharedString,
    t: &'a Theme,
}

impl RowCtx<'_> {
    fn render(&self, i: usize, cx: &mut Context<TermApp>) -> AnyElement {
        let t = self.t;
        let row = self.rows[i];
        let is_cur = i == self.cursor;
        // every row spans the panel edge to edge; the cursor is a faint fill
        // plus the accent bar on the left
        let base = div()
            .id(("git-row", i))
            .relative()
            .w_full()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden()
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.region = crate::regions::Region::Diff;
                this.git.cursor = i;
                if let Some(snap) = crate::diff::snapshot() {
                    let rows = this.git.rows(&snap);
                    this.git.clamp(&rows);
                    if matches!(row, Row::Section(_) | Row::File(..) | Row::Feedback) {
                        let tab = gpui::Keystroke::parse("tab").expect("valid keystroke");
                        this.diff_key(&tab, window, cx);
                    }
                }
                this.focus.focus(window);
                cx.notify();
            }))
            .when(is_cur && !matches!(row, Row::Line(..)), |d| d.bg(self.cursor_bg()));
        // the hunk the cursor is in (header or any of its lines): the bar spans it
        let in_cursor_hunk = |sec, fi, hi| match self.cur_row {
            Some(Row::Hunk(s, f, h) | Row::Line(s, f, h, _)) => (s, f, h) == (sec, fi, hi),
            _ => false,
        };

        match row {
            Row::Section(sec) => {
                let open = !self.git.folded.contains(&sec);
                base.h(px(32.))
                    .mt_2()
                    .child(self.bar(is_cur))
                    .pl(px(14.))
                    .gap_1p5()
                    .text_size(px(12.))
                    .child(chevron(open, 11., t))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_cur { t.fg } else { t.fg_muted })
                            .child(sec.title()),
                    )
                    .child(
                        div()
                            .font_family(self.mono.clone())
                            .text_size(px(11.))
                            .text_color(t.fg_subtle)
                            .child(self.snap.files(sec).len().to_string()),
                    )
                    .into_any_element()
            }
            Row::File(sec, fi) => {
                let f = &self.snap.files(sec)[fi];
                let open = self.git.expanded.contains(&(sec, f.path.clone()));
                let (dir, name) = match f.path.rsplit_once('/') {
                    Some((d, n)) => (format!("{d}/"), n.to_string()),
                    None => (String::new(), f.path.clone()),
                };
                let (letter, color) = match f.status {
                    "?" => ("U", t.success),
                    "A" => ("A", t.success),
                    "D" => ("D", t.danger),
                    "R" => ("R", t.accent),
                    "U" => ("!", t.danger),
                    _ => ("M", t.warning),
                };
                base.h(px(30.))
                    .cursor_pointer()
                    .when(!is_cur, |d| d.hover(|s| s.bg(with_alpha(t.fg, 0.03))))
                    .child(self.bar(is_cur))
                    .pl(px(26.))
                    .pr_4()
                    .gap_2()
                    .child(chevron(open, 12., t))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap_1p5()
                            .overflow_hidden()
                            .child(div().flex_none().text_color(t.fg).child(name))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(t.fg_subtle)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(dir),
                            ),
                    )
                    .when_some(f.note, |d, note| {
                        d.child(div().text_size(px(11.)).text_color(t.fg_subtle).child(note))
                    })
                    .map(|d| {
                        let n = self.git.comments.iter().filter(|c| c.path == f.path).count();
                        d.when(!open && n > 0, |d| d.child(comment_badge(n, t)))
                    })
                    .when(f.added + f.removed > 0, |d| d.child(counts(f.added, f.removed, t)))
                    .child(
                        div()
                            .w(px(10.))
                            .flex_none()
                            .flex()
                            .justify_end()
                            .font_family(self.mono.clone())
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(with_alpha(color, 0.9))
                            .child(letter),
                    )
                    .into_any_element()
            }
            Row::Hunk(sec, fi, hi) => {
                let h = &self.snap.files(sec)[fi].hunks[hi];
                let range = h.header.splitn(3, "@@").nth(1).unwrap_or("").trim().to_string();
                base.h(px(26.))
                    .when(!is_cur, |d| d.bg(gpui::hsla(0., 0., 0., 0.14)))
                    .border_t_1()
                    .border_color(t.border)
                    .child(self.bar(self.git.visual.is_none() && in_cursor_hunk(sec, fi, hi)))
                    .pl(px(GUTTER_W - 2.))
                    .pr_4()
                    .gap_2()
                    .font_family(self.mono.clone())
                    .text_size(px(11.))
                    .child(div().flex_none().text_color(t.fg_subtle).child(range))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if is_cur { t.fg_muted } else { t.fg_subtle })
                            .child(h.context().to_string()),
                    )
                    .map(|d| {
                        let n = self.git.comments_on(&self.snap.files(sec)[fi].path, &h.header);
                        d.when(n > 0, |d| d.child(comment_badge(n, t)))
                    })
                    .into_any_element()
            }
            Row::Line(sec, fi, hi, li) => {
                let l = &self.snap.files(sec)[fi].hunks[hi].lines[li];
                // during a visual selection the bar marks just the selected lines
                let selected = self.git.visual.is_some_and(|v| v.contains(sec, fi, hi, li));
                let in_cur = match (self.git.visual, self.cur_row) {
                    (Some(_), _) => selected,
                    (None, Some(Row::File(s, f))) => (s, f) == (sec, fi),
                    (None, _) => in_cursor_hunk(sec, fi, hi),
                };
                let (bg, sign, sign_color, text_color): (Option<Hsla>, &str, Hsla, Hsla) = match l.kind {
                    LineKind::Add => (Some(with_alpha(t.success, 0.1)), "+", t.success, t.fg),
                    LineKind::Del => (Some(with_alpha(t.danger, 0.1)), "-", t.danger, t.fg),
                    LineKind::Context => (None, "", t.fg_subtle, t.fg_muted),
                };
                let number = match l.kind {
                    LineKind::Del => l.old,
                    _ => l.new,
                };
                // the last line of a file leaves a little air before the next row
                let last = !matches!(self.rows.get(i + 1), Some(Row::Hunk(..) | Row::Line(..)));
                base.h(px(20.))
                    .when(last, |d| d.mb_1p5())
                    .font_family(self.mono.clone())
                    .text_size(px(12.))
                    .text_color(text_color)
                    .when_some(bg, |d, bg| d.bg(bg))
                    .when(selected, |d| d.bg(with_alpha(t.accent, 0.16)))
                    // cursor line: a gray wash over whatever the line's own color is
                    .when(is_cur, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(with_alpha(t.fg, if self.focused { 0.08 } else { 0.05 })),
                        )
                    })
                    .child(self.bar(in_cur))
                    .child(
                        div()
                            .w(px(GUTTER_W - 16.))
                            .flex_none()
                            .pr_2()
                            .flex()
                            .justify_end()
                            .text_size(px(11.))
                            .text_color(with_alpha(t.fg_subtle, 0.7))
                            .children(number.map(|n| n.to_string())),
                    )
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .text_color(with_alpha(sign_color, 0.8))
                            .child(sign),
                    )
                    .child(SharedString::from(l.text.clone()))
                    .into_any_element()
            }
            Row::Feedback => {
                let open = !self.git.feedback_folded;
                let n = self.git.comments.len();
                base.h(px(36.))
                    .mt_1p5()
                    .child(self.bar(is_cur))
                    .pl(px(14.))
                    .pr_4()
                    .gap_1p5()
                    .text_size(px(12.))
                    .child(chevron(open, 11., t))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_cur { t.fg } else { t.fg_muted })
                            .child("Feedback"),
                    )
                    .child(
                        div()
                            .font_family(self.mono.clone())
                            .text_size(px(11.))
                            .text_color(t.fg_subtle)
                            .child(n.to_string()),
                    )
                    .child(div().flex_1())
                    .when(n > 0, |d| {
                        d.child(
                            small_button("feedback-send", "Send to agent", "⇧F", t).on_click(cx.listener(
                                |this, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.send_feedback(window, cx);
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .into_any_element()
            }
            Row::Comment(ci) => {
                let Some(c) = self.git.comments.get(ci) else {
                    return div().into_any_element();
                };
                let last = !matches!(self.rows.get(i + 1), Some(Row::Comment(_) | Row::Draft));
                base.items_start()
                    .min_h(px(30.))
                    .when(last, |d| d.mb_1p5())
                    .cursor_pointer()
                    .when(!is_cur, |d| d.hover(|s| s.bg(with_alpha(t.fg, 0.03))))
                    .relative()
                    // full-height bar even though the row wraps
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).when(is_cur, |d| {
                        d.bg(if self.focused { t.accent } else { with_alpha(t.fg_subtle, 0.5) })
                    }))
                    .pl(px(14.))
                    .pr_4()
                    .py_2()
                    .gap_2()
                    .child(div().pt(px(3.)).child(icon(Icon::Message, 12., t.fg_subtle)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .font_family(self.mono.clone())
                                    .text_size(px(11.))
                                    .text_color(t.fg_subtle)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(c.location()),
                            )
                            .child(
                                div()
                                    .whitespace_normal()
                                    .text_color(t.fg)
                                    .child(c.text.trim().to_string()),
                            ),
                    )
                    .into_any_element()
            }
            Row::Draft => {
                let Some(d) = self.git.comment_draft.as_ref() else {
                    return div().into_any_element();
                };
                let text = &d.comment.text;
                let lines: Vec<&str> = text.split('\n').collect();
                let n = lines.len();
                let caret = || div().ml_px().w(px(1.5)).h(px(15.)).flex_none().bg(t.accent);
                base.py_1p5()
                    .pl(px(14.))
                    .pr_3()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .rounded_lg()
                            .bg(gpui::hsla(0., 0., 0., 0.2))
                            .border_1()
                            .border_color(with_alpha(t.accent, 0.55))
                            .whitespace_normal()
                            .child(
                                div()
                                    .px_3()
                                    .pt_2()
                                    .pb_1p5()
                                    .min_h(px(44.))
                                    .flex()
                                    .flex_col()
                                    .map(|b| {
                                        if text.is_empty() {
                                            b.child(
                                                div()
                                                    .h(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .child(caret())
                                                    .child(
                                                        div()
                                                            .ml_1()
                                                            .text_color(t.fg_subtle)
                                                            .child("What should change?"),
                                                    ),
                                            )
                                        } else {
                                            b.children(lines.into_iter().enumerate().map(|(i, l)| {
                                                div()
                                                    .min_h(px(18.))
                                                    .flex()
                                                    .items_center()
                                                    .child(SharedString::from(l.to_string()))
                                                    .when(i + 1 == n, |d| d.child(caret()))
                                            }))
                                        }
                                    }),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1p5()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .border_t_1()
                                    .border_color(t.border)
                                    .text_size(px(11.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .font_family(self.mono.clone())
                                            .text_color(t.fg_subtle)
                                            .child(d.comment.location()),
                                    )
                                    .child(
                                        small_button("comment-save", "Comment", "↩", t).on_click(cx.listener(
                                            |this, _: &ClickEvent, _, cx| {
                                                cx.stop_propagation();
                                                this.save_comment();
                                                cx.notify();
                                            },
                                        )),
                                    ),
                            ),
                    )
                    .into_any_element()
            }
        }
    }

    fn cursor_bg(&self) -> Hsla {
        with_alpha(self.t.fg, if self.focused { 0.07 } else { 0.04 })
    }

    /// The 2px bar on the left edge: marks the cursor row, and the lines `s` / `d`
    /// will act on.
    /// Absolutely positioned (rows are `relative`), so it always sits on the
    /// panel's left edge regardless of the row's padding.
    fn bar(&self, on: bool) -> impl IntoElement {
        div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(2.))
            .when(on, |d| {
                d.bg(if self.focused {
                    self.t.accent
                } else {
                    with_alpha(self.t.fg_subtle, 0.5)
                })
            })
    }
}

fn chevron(open: bool, size: f32, t: &Theme) -> impl IntoElement {
    icon(
        if open { Icon::ChevronDown } else { Icon::ChevronRight },
        size,
        t.fg_subtle,
    )
}

/// The panel's button: soft fill + hairline, label plus its shortcut, dimmed.
fn small_button(id: &'static str, label: &'static str, key: &'static str, t: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(22.))
        .px_2()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_md()
        .cursor_pointer()
        .bg(with_alpha(t.fg, 0.07))
        .border_1()
        .border_color(t.border)
        .text_color(t.fg)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.bg(with_alpha(t.fg, 0.12)))
        .child(label)
        .child(div().text_color(t.fg_subtle).child(key))
}

fn comment_badge(n: usize, t: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(11.))
        .text_color(t.fg_muted)
        .child(icon(Icon::Message, 11., t.fg_muted))
        .child(n.to_string())
}

/// Ahead/behind the upstream (nothing when in step), or a crossed-out cloud
/// when the branch has no upstream yet.
fn tracking(snap: &Snapshot, t: &Theme) -> Option<AnyElement> {
    if !snap.on_branch || !snap.has_head {
        return None;
    }
    let Some(u) = &snap.upstream else {
        return Some(icon(Icon::CloudOff, 12., t.fg_subtle).into_any_element());
    };
    if u.ahead == 0 && u.behind == 0 {
        return None;
    }
    Some(
        div()
            .flex_none()
            .flex()
            .gap_1()
            .font_family(term_family())
            .text_size(px(11.))
            .text_color(t.fg_subtle)
            .when(u.ahead > 0, |d| d.child(format!("↑{}", u.ahead)))
            .when(u.behind > 0, |d| d.child(format!("↓{}", u.behind)))
            .into_any_element(),
    )
}

/// The branch's pull request: state icon + number; click opens it.
fn pr_chip(pr: crate::pr::PullRequest, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    use crate::pr::PrState;
    let (ic, color) = match pr.state {
        PrState::Open => (Icon::PullRequest, t.success),
        PrState::Draft => (Icon::PullRequestDraft, t.fg_subtle),
        PrState::Merged => (Icon::GitMerge, t.fg_muted),
        PrState::Closed => (Icon::PullRequestClosed, t.fg_subtle),
    };
    let url = pr.url.clone();
    div()
        .id("git-pr")
        .flex_none()
        .ml_1()
        .flex()
        .items_center()
        .gap_1()
        .font_family(term_family())
        .text_size(px(11.))
        .text_color(t.fg_muted)
        .cursor_pointer()
        .hover(|s| s.text_color(t.fg))
        .child(icon(ic, 12., color))
        .child(format!("#{}", pr.number))
        .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| cx.open_url(&url)))
}

fn counts(adds: u32, dels: u32, t: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .flex()
        .gap_1p5()
        .font_family(term_family())
        .text_size(px(11.))
        .when(adds > 0, |d| d.child(div().text_color(t.success).child(format!("+{adds}"))))
        .when(dels > 0, |d| d.child(div().text_color(t.danger).child(format!("−{dels}"))))
}

impl TermApp {
    /// The git panel's shortcut sheet (`?`), drawn over the whole window so it
    /// has room regardless of how narrow the panel is. Any key or click closes it.
    pub(super) fn git_help_modal(&self, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !(self.git.help && self.diff_open) {
            return None;
        }
        use super::help::group;
        let left = vec![
            group(
                "Navigate",
                &[
                    ("j k", "Move (lines too)"),
                    ("] [", "Next / previous hunk or file"),
                    ("tab", "Expand / collapse"),
                    ("enter", "Expand, or open hunk in editor"),
                    ("o", "Open in editor"),
                ],
            ),
            group(
                "Changes",
                &[
                    ("s", "Stage / unstage"),
                    ("S", "Stage / unstage everything"),
                    ("u", "Unstage"),
                    ("U", "Unstage everything"),
                    ("x d", "Discard"),
                ],
            ),
            group(
                "Panel",
                &[
                    ("< >", "Narrower / wider"),
                    ("R", "Refresh"),
                    ("esc", "Back to terminal"),
                    ("q", "Close"),
                ],
            ),
        ];
        let right = vec![
            group(
                "Feedback",
                &[
                    ("f", "Comment on line (or hunk header)"),
                    ("V", "Select lines from here, then f"),
                    ("F", "Send comments to agent"),
                    ("enter", "Edit comment"),
                    ("d", "Delete comment"),
                ],
            ),
            group(
                "Commit & agents",
                &[
                    ("c", "Commit staged"),
                    ("a", "Amend last commit"),
                    ("p", "Pull"),
                    ("P", "Push"),
                    ("N", "Open or create pull request"),
                    ("r", "Run a recipe (review, simplify…)"),
                ],
            ),
        ];
        Some(super::help::sheet(
            "Git panel shortcuts",
            vec![left, right],
            600.,
            t,
            cx,
            |this| this.git.help = false,
        ))
    }
}

fn empty_state(ic: Icon, title: &'static str, detail: Option<&'static str>, t: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .child(icon(ic, 14., t.fg_subtle)),
        )
        .child(div().text_color(t.fg_muted).child(title))
        .children(detail.map(|d| div().text_size(px(12.)).text_color(t.fg_subtle).child(d)))
}
