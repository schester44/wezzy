//! Diff feedback: comments left on hunks in the git panel (`f`), sent to an
//! agent as one review message (`F`). The message carries file:line and the
//! changed lines, so the agent knows exactly which code each note is about.

use crate::app::TermApp;
use crate::diff::{Hunk, LineKind, Section};
use gpui::{Context, Window};
use mux::pane::PaneId;
use mux::Mux;

/// Changed lines quoted per comment before eliding.
const MAX_EXCERPT: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub sec: Section,
    pub path: String,
    /// raw hunk header, to find the hunk again (and mark it) while it's unchanged
    pub header: String,
    /// line range of the changed lines (new-file numbers; old for pure deletions)
    pub start: u32,
    pub end: u32,
    /// the changed lines, with +/- prefixes
    pub excerpt: Vec<String>,
    pub text: String,
}

impl Comment {
    pub fn for_hunk(sec: Section, path: &str, h: &Hunk) -> Self {
        let changed: Vec<_> = h.lines.iter().filter(|l| l.kind != LineKind::Context).collect();
        let nums: Vec<u32> = changed.iter().filter_map(|l| l.new.or(l.old)).collect();
        let start = nums.iter().copied().min().unwrap_or(h.new_start);
        let end = nums.iter().copied().max().unwrap_or(start);
        let mut excerpt: Vec<String> = changed
            .iter()
            .take(MAX_EXCERPT)
            .map(|l| {
                let sign = if l.kind == LineKind::Add { '+' } else { '-' };
                format!("{sign}{}", l.text)
            })
            .collect();
        if changed.len() > MAX_EXCERPT {
            excerpt.push(format!("… {} more changed lines", changed.len() - MAX_EXCERPT));
        }
        Self {
            sec,
            path: path.to_string(),
            header: h.header.clone(),
            start,
            end,
            excerpt,
            text: String::new(),
        }
    }

    /// A comment on lines `lo..=hi` of a hunk (visual selection); quotes every
    /// selected line, context included.
    pub fn for_lines(sec: Section, path: &str, h: &Hunk, lo: usize, hi: usize) -> Self {
        let mut c = Self::for_hunk(sec, path, h);
        let picked = &h.lines[lo.min(h.lines.len())..(hi + 1).min(h.lines.len())];
        let nums: Vec<u32> = picked.iter().filter_map(|l| l.new.or(l.old)).collect();
        c.start = nums.iter().copied().min().unwrap_or(c.start);
        c.end = nums.iter().copied().max().unwrap_or(c.start);
        c.excerpt = picked
            .iter()
            .take(MAX_EXCERPT)
            .map(|l| {
                let sign = match l.kind {
                    LineKind::Add => '+',
                    LineKind::Del => '-',
                    LineKind::Context => ' ',
                };
                format!("{sign}{}", l.text)
            })
            .collect();
        if picked.len() > MAX_EXCERPT {
            c.excerpt.push(format!("… {} more lines", picked.len() - MAX_EXCERPT));
        }
        c
    }

    /// "src/a.rs:12" or "src/a.rs:12-18"
    pub fn location(&self) -> String {
        if self.end > self.start {
            format!("{}:{}-{}", self.path, self.start, self.end)
        } else {
            format!("{}:{}", self.path, self.start)
        }
    }
}

/// The review message an agent receives.
pub fn message(comments: &[Comment]) -> String {
    let mut out = String::from("Review feedback on your changes:\n");
    for c in comments {
        out.push_str(&format!(
            "\n{}\n```diff\n{}\n```\n→ {}\n",
            c.location(),
            c.excerpt.join("\n"),
            c.text.trim()
        ));
    }
    out
}

impl TermApp {
    /// `F`: send every comment to the agent whose run changed those files, else
    /// to the agent in this workspace, or pick one.
    pub(crate) fn send_feedback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.comments.is_empty() {
            self.flash("No comments yet — press f on a hunk");
            return;
        }
        // comments on an agent run's files go back to that agent
        if let Some(pane) = self.run_for_comments() {
            self.send_feedback_to(pane, window, cx);
            return;
        }
        let ws = Mux::get().active_workspace();
        let agents: Vec<PaneId> = crate::agents::collect(None)
            .into_iter()
            .filter(|a| a.workspace == ws)
            .map(|a| a.pane_id)
            .collect();
        match agents.as_slice() {
            [] => self.flash("No agent in this workspace to send feedback to"),
            [one] => self.send_feedback_to(*one, window, cx),
            _ => {
                self.open_palette(crate::palette::Scope::Agents, cx);
                if let Some(crate::app::Overlay::Palette(p)) = self.overlay.as_mut() {
                    p.send_feedback = true;
                }
            }
        }
    }

    /// Paste the review into the agent's prompt (not submitted, so you can add
    /// to it), focus it, and clear the comments.
    pub(crate) fn send_feedback_to(&mut self, to: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = Mux::get().get_pane(to) else { return };
        let msg = message(&self.git.comments);
        if let Err(e) = pane.send_paste(&msg) {
            self.flash(format!("Couldn't send: {e:#}"));
            return;
        }
        let n = self.git.comments.len();
        self.git.comments.clear();
        self.region = crate::regions::Region::Terminal;
        self.focus_pane(to, window, cx);
        self.flash(format!(
            "Sent {n} comment{} — press enter in the agent to submit",
            if n == 1 { "" } else { "s" }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::DiffLine;

    #[test]
    fn builds_message_from_hunk() {
        let line = |kind, old, new, text: &str| DiffLine {
            kind,
            old,
            new,
            text: text.into(),
        };
        let h = Hunk {
            header: "@@ -10,4 +10,4 @@".into(),
            raw: vec![],
            lines: vec![
                line(LineKind::Context, Some(10), Some(10), "a"),
                line(LineKind::Del, Some(11), None, "old"),
                line(LineKind::Add, None, Some(11), "new"),
                line(LineKind::Add, None, Some(12), "more"),
            ],
            new_start: 10,
        };
        let mut c = Comment::for_hunk(Section::Unstaged, "src/x.rs", &h);
        c.text = "keep the old name".into();
        assert_eq!(c.location(), "src/x.rs:11-12");
        assert_eq!(c.excerpt, vec!["-old", "+new", "+more"]);
        let m = message(&[c]);
        assert!(m.contains("src/x.rs:11-12\n```diff\n-old\n+new\n+more\n```\n→ keep the old name"));

        // visual selection: lines 0..=1 (context + the deletion)
        let v = Comment::for_lines(Section::Unstaged, "src/x.rs", &h, 0, 1);
        assert_eq!(v.location(), "src/x.rs:10-11");
        assert_eq!(v.excerpt, vec![" a", "-old"]);
    }
}
