//! The git panel's live layer: where agents are reading and editing, hunks
//! that just appeared, and follow mode (`w`).
//!
//! - **Markers:** `activity::touches` within `TRAIL` mark their file rows,
//!   fading by age, so an agent's path reads as a short trail.
//! - **Glow:** `diff::freshness` (each snapshot compared with the one before).
//! - **Follow:** a new Edit touch in the panel's repo moves the cursor to that
//!   file once it's in the diff, and onto the hunk that appeared after the edit
//!   when the next snapshot brings it. Moving the cursor yourself stops it.
//!
//! Fades are drawn from timestamps at render time; `tick` keeps the panel
//! repainting while any of them is still running, then stops.

use crate::activity::{self, TouchKind};
use crate::app::TermApp;
use crate::diff::{self, Section, Snapshot};
use crate::gitpanel::Row;
use gpui::Context;
use mux::pane::PaneId;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a touch marks its file.
pub const TRAIL: Duration = Duration::from_secs(10);
/// The one-off pulse when an agent edits a file.
pub const PULSE: Duration = Duration::from_millis(700);
/// Repaint interval while something fades.
const FRAME: Duration = Duration::from_millis(50);
/// Follow gives up on an edit whose file never shows up in the diff.
const FOLLOW_WAIT: Duration = Duration::from_secs(10);
/// ...and settles for the file's first hunk if no fresh one arrives.
const FOLLOW_SETTLE: Duration = Duration::from_secs(4);
/// How far a fresh hunk may predate the edit's touch and still be its result.
const TOUCH_SLACK: Duration = Duration::from_millis(1500);

/// The newest touch of a file in the panel's repo.
#[derive(Clone, Debug)]
pub struct Marker {
    pub kind: TouchKind,
    pub at: Instant,
    /// the agent pane's label letter, when more than one agent is at work here
    pub label: Option<String>,
}

impl Marker {
    /// 1 when fresh, fading to 0 at `TRAIL`.
    pub fn strength(&self) -> f32 {
        fade(self.at, TRAIL)
    }
}

/// 1 → 0 over `span` since `at`, easing out so the tail lingers.
pub fn fade(at: Instant, span: Duration) -> f32 {
    let x = (at.elapsed().as_secs_f32() / span.as_secs_f32()).clamp(0., 1.);
    (1. - x) * (1. - x * 0.5)
}

/// Markers by path relative to `root`.
pub fn markers(root: &Path) -> HashMap<String, Marker> {
    let touches = activity::touches();
    let panes: HashSet<PaneId> = touches
        .iter()
        .filter(|t| t.path.starts_with(root))
        .map(|t| t.pane)
        .collect();
    let labels: HashMap<PaneId, String> = if panes.len() > 1 {
        panes.iter().filter_map(|p| Some((*p, pane_label(*p)?))).collect()
    } else {
        HashMap::new()
    };
    let mut out = HashMap::new();
    // newest last: later touches overwrite
    for t in touches.iter().filter(|t| t.at.elapsed() < TRAIL) {
        let Ok(rel) = t.path.strip_prefix(root) else { continue };
        out.insert(
            rel.to_string_lossy().to_string(),
            Marker {
                kind: t.kind,
                at: t.at,
                label: labels.get(&t.pane).cloned(),
            },
        );
    }
    out
}

/// The letter PaneSelect shows on the pane: its position among the tab's
/// panes, labeled from `quick_select_alphabet`.
fn pane_label(pane: PaneId) -> Option<String> {
    let mux = mux::Mux::get();
    let (_, _, tab_id) = mux.resolve_pane_id(pane)?;
    let panes = mux.get_tab(tab_id)?.iter_panes_ignoring_zoom();
    let i = panes.iter().position(|p| p.pane.pane_id() == pane)?;
    let labels = crate::quickselect::labels(panes.len(), &config::configuration().quick_select_alphabet);
    labels.get(i).map(|l| l.to_uppercase())
}

/// Something in the panel is still fading.
fn live(root: &Path) -> bool {
    diff::freshness().live()
        || activity::touches()
            .iter()
            .any(|t| t.at.elapsed() < TRAIL && t.path.starts_with(root))
}

/// Follow mode's state: on while `GitPanel::follow` is `Some`.
#[derive(Clone, Debug, Default)]
pub struct Follow {
    /// the file the agent last edited (relative) and when
    pending: Option<(String, Instant)>,
    /// the newest touch already looked at
    seen: Option<Instant>,
}

impl TermApp {
    /// Keep repainting the panel while a marker or glow fades; stops on its own.
    pub(crate) fn git_tick(&self, snap: &Snapshot, cx: &mut Context<Self>) {
        let Some(root) = snap.root.clone() else { return };
        if self.git.ticking.get() || !live(&root) {
            return;
        }
        self.git.ticking.set(true);
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(FRAME).await;
            let go = this.update(cx, |this, cx| {
                let root = diff::snapshot().and_then(|s| s.root.clone());
                let go = this.diff_open && root.is_some_and(|r| live(&r));
                if go {
                    cx.notify();
                } else {
                    this.git.ticking.set(false);
                    // the last frame, at zero
                    cx.notify();
                }
                go
            });
            if !matches!(go, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// `w`: follow the agent's edits, or stop. Starting jumps to its latest edit.
    pub(crate) fn toggle_follow(&mut self) {
        if self.git.follow.take().is_some() {
            return;
        }
        let root = diff::snapshot().and_then(|s| s.root.clone());
        let touches = activity::touches();
        let pending = root.and_then(|root| {
            touches.iter().rev().find_map(|t| {
                let rel = t.path.strip_prefix(&root).ok()?;
                (t.kind == TouchKind::Edit).then(|| (rel.to_string_lossy().to_string(), t.at))
            })
        });
        self.git.follow = Some(Follow {
            pending,
            seen: touches.last().map(|t| t.at),
        });
        self.follow_step();
    }

    /// Pick up new edits and move the cursor to the latest one once the diff
    /// has it. Runs on every redraw event (new touches, new snapshots).
    pub(crate) fn follow_step(&mut self) {
        if !self.diff_open {
            return;
        }
        let Some(snap) = diff::snapshot() else { return };
        let Some(root) = snap.root.clone() else { return };
        let Some(f) = self.git.follow.as_mut() else { return };
        let touches = activity::touches();
        for t in touches.iter().filter(|t| f.seen.is_none_or(|s| t.at > s)) {
            if t.kind == TouchKind::Edit {
                if let Ok(rel) = t.path.strip_prefix(&root) {
                    f.pending = Some((rel.to_string_lossy().to_string(), t.at));
                }
            }
        }
        if let Some(last) = touches.last() {
            f.seen = Some(last.at);
        }
        let Some((path, at)) = f.pending.clone() else { return };
        // don't pull the cursor out from under something you're doing
        let g = &self.git;
        if g.composer.is_some() || g.comment_draft.is_some() || g.visual.is_some() || g.confirm.is_some() || g.help {
            return;
        }
        let found = [Section::Unstaged, Section::Untracked, Section::Staged]
            .into_iter()
            .find_map(|sec| Some((sec, snap.files(sec).iter().position(|f| f.path == path)?)));
        let Some((sec, fi)) = found else {
            if at.elapsed() > FOLLOW_WAIT {
                if let Some(f) = self.git.follow.as_mut() {
                    f.pending = None;
                }
            }
            return;
        };
        // the hunk that appeared after the edit, newest first. A touch is
        // stamped when the log is polled, which can be a moment after the write
        // landed in a snapshot.
        let fresh = diff::freshness();
        let file = &snap.files(sec)[fi];
        let since = at.checked_sub(TOUCH_SLACK).unwrap_or(at);
        let fresh_hi = (0..file.hunks.len())
            .filter_map(|hi| Some((hi, fresh.hunk(sec, fi, hi).filter(|t| *t >= since)?)))
            .max_by_key(|(_, t)| *t)
            .map(|(hi, _)| hi);
        if fresh_hi.is_some() || at.elapsed() > FOLLOW_SETTLE {
            if let Some(f) = self.git.follow.as_mut() {
                f.pending = None;
            }
        }
        self.git.folded.remove(&sec);
        self.git.expanded.insert((sec, path));
        let rows = self.git.rows(&snap);
        let target = match fresh_hi.or((!file.hunks.is_empty()).then_some(0)) {
            // untracked files have no hunk header: land on the first line
            Some(hi) => rows
                .iter()
                .position(|r| *r == Row::Hunk(sec, fi, hi))
                .or_else(|| rows.iter().position(|r| *r == Row::Line(sec, fi, hi, 0))),
            None => None,
        }
        .or_else(|| rows.iter().position(|r| *r == Row::File(sec, fi)));
        if let Some(i) = target {
            if i != self.git.cursor {
                self.git.cursor = i;
                self.git.reveal_cursor();
            }
        }
    }
}
