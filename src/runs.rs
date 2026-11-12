//! Agent runs in the UI (the data lives in `activity.rs`): the card over an
//! agent's pane when a turn finishes with changes (`ui/run_card.rs`), the git
//! panel's run filter, reviewing, committing, undoing and dismissing a run.

use crate::activity::{self, Run};
use crate::app::{send_event, AppEvent, Overlay, TermApp};
use crate::diff::{self, Section, Snapshot};
use crate::gitpanel::Row;
use config::keyassignment::KeyAssignment;
use gpui::{Context, Window};
use mux::pane::PaneId;
use mux::Mux;
use std::path::{Path, PathBuf};

/// What the undo confirmation performs. Not app commands: these act without
/// asking.
const UNDO_TURN: &str = "wezzy-run.undo-turn";
const UNDO_RUN: &str = "wezzy-run.undo-run";

/// For a confirmed undo's event name: `Some(true)` undoes the whole run,
/// `Some(false)` its last turn.
pub fn confirmed_undo(name: &str) -> Option<bool> {
    match name {
        UNDO_TURN => Some(false),
        UNDO_RUN => Some(true),
        _ => None,
    }
}

/// Undo on a background thread (it runs git); the toast reports.
pub fn undo(pane: PaneId, whole: bool) {
    std::thread::spawn(move || {
        let r = if whole { activity::undo_run(pane) } else { activity::undo_turn(pane) };
        send_event(AppEvent::Flash(match r {
            Ok(n) => format!("Restored {}", files(n)),
            Err(e) => e,
        }));
    });
}

/// A finished run with changes: what the card and ⌘G look for.
pub fn reviewable(pane: PaneId) -> Option<Run> {
    activity::run(pane).filter(|r| r.done && r.has_changes())
}

pub fn files(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

fn rel(root: &Path, f: &Path) -> Option<String> {
    Some(f.strip_prefix(root).ok()?.to_string_lossy().to_string())
}

impl TermApp {
    /// The run whose card shows over `pane`: finished with changes, and you
    /// haven't typed into the pane since that turn finished.
    pub(crate) fn run_card(&self, pane: PaneId) -> Option<Run> {
        reviewable(pane).filter(|r| self.run_card_hidden.get(&pane) != Some(&r.finished_at))
    }

    /// You typed into the pane: hide its card until the agent's next turn
    /// finishes. The run itself stays.
    pub(crate) fn hide_run_card(&mut self, pane: PaneId) {
        if let Some(r) = reviewable(pane) {
            self.run_card_hidden.insert(pane, r.finished_at);
        }
    }

    /// The active pane, when it has a run.
    fn active_run_pane(&self) -> Option<PaneId> {
        let pane = self.active_pane()?.pane_id();
        activity::run(pane).map(|_| pane)
    }

    /// End the run: you've seen its work.
    pub(crate) fn end_run(&mut self, pane: PaneId) {
        activity::dismiss(pane);
        self.run_card_hidden.remove(&pane);
        if self.git.run_filter == Some(pane) {
            self.git.run_filter = None;
        }
    }

    /// Drop the runs (and hidden cards) of panes that closed.
    pub(crate) fn forget_closed_runs(&mut self) {
        let mux = Mux::get();
        for (pane, _) in activity::runs() {
            if mux.get_pane(pane).is_none() {
                activity::forget(pane);
                if self.git.run_filter == Some(pane) {
                    self.git.run_filter = None;
                }
            }
        }
        self.run_card_hidden.retain(|p, _| mux.get_pane(*p).is_some());
    }

    /// Open the git panel filtered to `pane`'s run, on the agent's repo.
    fn open_git_on_run(&mut self, pane: PaneId, root: PathBuf) {
        self.diff_open = true;
        self.sidebar_open = false;
        self.region = crate::regions::Region::Diff;
        self.git.run_filter = Some(pane);
        self.git.land = Some(root);
        // render would set it too; set it now so the refresh looks at the right repo
        if let Some(p) = Mux::get().get_pane(pane) {
            diff::set_target(crate::gitpanel::git_dir(&p));
        }
        diff::mark_dirty();
    }

    /// Review changes: go to the agent's pane, open the git panel on just its
    /// run's files, cursor on the first one.
    pub(crate) fn review_run(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = activity::run(pane) else {
            self.flash("No agent changes to review");
            return;
        };
        if self.active_pane().map(|p| p.pane_id()) != Some(pane) {
            self.focus_pane(pane, window, cx);
        }
        self.focus.focus(window);
        self.open_git_on_run(pane, run.root);
    }

    /// ⌘G opening the panel: filtered when the active pane's agent finished a
    /// run with changes you haven't reviewed, else everything.
    pub(crate) fn open_git_panel(&mut self) {
        self.diff_open = true;
        self.sidebar_open = false;
        self.region = crate::regions::Region::Diff;
        self.git.run_filter = None;
        let pane = self.active_pane().map(|p| p.pane_id());
        if let Some((pane, run)) = pane.and_then(|p| reviewable(p).map(|r| (p, r))) {
            self.open_git_on_run(pane, run.root);
        }
    }

    /// `A` in the git panel: cycle everything → each agent's run in this repo
    /// (in pane order, the active tab's first) → everything again.
    pub(crate) fn toggle_run_filter(&mut self) {
        let panes = self.run_panes_here();
        if panes.is_empty() {
            self.git.run_filter = None;
            self.flash("No agent run here");
            return;
        }
        let next = match self.git.run_filter.and_then(|p| panes.iter().position(|q| *q == p)) {
            Some(i) => panes.get(i + 1).copied(),
            // unfiltered: start with the active pane's run when it has one
            None => Some(self.active_run_pane().filter(|p| panes.contains(p)).unwrap_or(panes[0])),
        };
        self.git.run_filter = next;
        if let Some(run) = next.and_then(activity::run) {
            self.git.land = Some(run.root);
        }
    }

    /// Panes whose runs changed files in the git panel's repo: the active
    /// tab's in layout order, then any others.
    pub(crate) fn run_panes_here(&self) -> Vec<PaneId> {
        let Some(root) = diff::snapshot().and_then(|s| s.root.clone()) else {
            return vec![];
        };
        let mut runs: Vec<PaneId> = activity::runs()
            .into_iter()
            .filter(|(_, r)| r.root == root && !r.files().is_empty())
            .map(|(p, _)| p)
            .collect();
        let order: Vec<PaneId> = self
            .active_tab()
            .map(|t| t.iter_panes().into_iter().map(|p| p.pane.pane_id()).collect())
            .unwrap_or_default();
        runs.sort_by_key(|p| (order.iter().position(|q| q == p).unwrap_or(usize::MAX), *p));
        runs
    }

    /// Once the reviewed repo's snapshot is in, put the cursor on the first
    /// file. Called from render.
    pub(crate) fn land_git_cursor(&mut self) {
        let Some(root) = self.git.land.clone() else { return };
        let Some(snap) = diff::snapshot() else { return };
        if snap.root.as_ref() != Some(&root) {
            return;
        }
        self.git.land = None;
        let rows = self.git.rows(&snap);
        let Some(i) = rows.iter().position(|r| matches!(r, Row::File(..))) else { return };
        self.git.cursor = i;
        self.scroll_to_cursor();
    }

    /// The run a commit finishes: the panel is filtered to it, everything
    /// staged is the run's, and none of its files are left unstaged.
    pub(crate) fn commit_ends_run(&self, snap: &Snapshot) -> Option<PaneId> {
        let pane = self.git.run_filter?;
        let files = activity::run(pane)?.files();
        let root = snap.root.as_ref()?;
        let is_run = |f: &diff::FileDiff| files.contains(&root.join(&f.path));
        let all_staged = !snap.staged.is_empty()
            && snap.staged.iter().all(is_run)
            && ![Section::Unstaged, Section::Untracked]
                .iter()
                .any(|s| snap.files(*s).iter().any(is_run));
        all_staged.then_some(pane)
    }

    /// Ask before undoing the last turn (or the whole run) of `pane`'s agent.
    pub(crate) fn ask_undo(&mut self, pane: PaneId, whole: bool) {
        let Some(run) = activity::run(pane) else {
            self.flash("Nothing to undo");
            return;
        };
        let changed: Vec<PathBuf> = if whole {
            run.files().into_iter().collect()
        } else {
            run.turns.last().map(|t| t.files.iter().cloned().collect()).unwrap_or_default()
        };
        if changed.is_empty() {
            self.flash("Nothing to undo");
            return;
        }
        // name a few files; past that, count them
        let detail = if changed.len() <= 3 {
            changed
                .iter()
                .filter_map(|f| rel(&run.root, f))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            files(changed.len())
        };
        self.overlay = Some(Overlay::Confirm {
            message: if whole { "Undo the agent's run?" } else { "Undo the agent's last turn?" }.into(),
            detail,
            verb: "Undo",
            action: KeyAssignment::EmitEvent(if whole { UNDO_RUN } else { UNDO_TURN }.into()),
            cancel: None,
            pane_id: pane,
        });
    }

    /// The palette's run commands act on the active pane's run.
    pub(crate) fn run_command(&mut self, c: crate::commands::AppCommand, window: &mut Window, cx: &mut Context<Self>) {
        use crate::commands::AppCommand;
        let Some(pane) = self.active_run_pane() else {
            self.flash("No agent run here");
            return;
        };
        match c {
            AppCommand::ReviewAgentRun => self.review_run(pane, window, cx),
            AppCommand::UndoAgentTurn => self.ask_undo(pane, false),
            AppCommand::UndoAgentRun => self.ask_undo(pane, true),
            AppCommand::DismissAgentRun => self.end_run(pane),
            _ => {}
        }
    }

    /// Where `F` sends comments without asking: the agent whose run edited
    /// every commented file (the one the panel is filtered to, if several).
    pub(crate) fn run_for_comments(&self) -> Option<PaneId> {
        let snap = diff::snapshot()?;
        let root = snap.root.as_ref()?;
        let paths: Vec<PathBuf> = self.git.comments.iter().map(|c| root.join(&c.path)).collect();
        let mux = Mux::get();
        let hits: Vec<PaneId> = activity::runs()
            .into_iter()
            .filter(|(p, r)| mux.get_pane(*p).is_some() && paths.iter().all(|f| r.touched(f)))
            .map(|(p, _)| p)
            .collect();
        match (self.git.run_filter, hits.as_slice()) {
            (Some(p), _) if hits.contains(&p) => Some(p),
            (_, [one]) => Some(*one),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_events() {
        assert_eq!(confirmed_undo(UNDO_TURN), Some(false));
        assert_eq!(confirmed_undo(UNDO_RUN), Some(true));
        assert_eq!(confirmed_undo("wezzy.toggle-sidebar"), None);
        assert!(crate::commands::AppCommand::from_event(UNDO_TURN).is_none());
    }

    #[test]
    fn file_counts() {
        assert_eq!(files(1), "1 file");
        assert_eq!(files(3), "3 files");
    }
}
