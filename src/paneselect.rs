//! PaneSelect (Leader m swaps by default): a label on every pane of the tab;
//! type one to activate it, swap it with the active pane, or move it out to
//! its own tab or window. Esc cancels.
//!
//! Wezzy adds a close mode (Leader x, `AppCommand::ClosePane`): the labels
//! turn danger-colored with a ×, and the picked pane closes without moving
//! focus. It asks first like ⌘W does (`close_confirmation`).
//!
//! Swaps happen on the session server, so they survive a resync. The protocol
//! has no swap, only "move this pane into a split of that one"
//! (`SplitSource::MovePane`), so a swap is built from moves:
//!
//! - Two panes: move A to B's far side, sized to B. One step.
//! - More: a placeholder T borrows B's spot while the two trade places.
//!   Split T off B; move B next to A (T now fills B's old spot); move A next
//!   to T (B now fills A's old spot); kill T (A fills it). Removing a pane
//!   hands its area to its sibling, so each pane ends with the other's exact
//!   rectangle.

use crate::app::{send_event, AppEvent, Overlay, TermApp};
use config::keyassignment::{KeyAssignment, PaneSelectArguments, PaneSelectMode, SpawnTabDomain};
use std::sync::Arc;
use gpui::{Context, KeyDownEvent};
use mux::domain::SplitSource;
use mux::pane::{Pane, PaneId};
use mux::tab::{SplitDirection, SplitRequest, SplitSize, Tab, TabId};
use mux::Mux;
use portable_pty::CommandBuilder;

pub struct PaneSelect {
    pub tab_id: TabId,
    pub mode: PaneSelectMode,
    /// (pane, label) for every pane you can pick
    pub labels: Vec<(PaneId, String)>,
    pub typed: String,
    pub show_ids: bool,
    /// close the picked pane instead of `mode`
    pub close: bool,
    was_zoomed: bool,
}

impl TermApp {
    pub(crate) fn start_pane_select(&mut self, args: &PaneSelectArguments, cx: &mut Context<Self>) {
        self.open_pane_select(args, false, cx);
    }

    pub(crate) fn start_close_pane_select(&mut self, cx: &mut Context<Self>) {
        self.open_pane_select(&PaneSelectArguments::default(), true, cx);
    }

    fn open_pane_select(&mut self, args: &PaneSelectArguments, close: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        let was_zoomed = tab.get_zoomed_pane().is_some();
        let panes = tab.iter_panes_ignoring_zoom();
        if panes.len() < 2 {
            self.flash("Only one pane");
            cx.notify();
            return;
        }
        let swap = !close
            && matches!(
                args.mode,
                PaneSelectMode::SwapWithActive | PaneSelectMode::SwapWithActiveKeepFocus
            );
        let picks: Vec<PaneId> = panes
            .iter()
            .filter(|p| !(swap && p.is_active))
            .map(|p| p.pane.pane_id())
            .collect();
        let alphabet = if args.alphabet.is_empty() {
            config::configuration().quick_select_alphabet.clone()
        } else {
            args.alphabet.clone()
        };
        let labels = crate::quickselect::labels(picks.len(), &alphabet);
        if was_zoomed {
            tab.set_zoomed(false);
        }
        self.pane_select = Some(PaneSelect {
            tab_id: tab.tab_id(),
            mode: args.mode,
            labels: picks.into_iter().zip(labels).collect(),
            typed: String::new(),
            show_ids: args.show_pane_ids,
            close,
            was_zoomed,
        });
        cx.notify();
    }

    pub(crate) fn pane_select_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let Some(ps) = self.pane_select.as_mut() else {
            return;
        };
        if ks.key == "escape" || ks.modifiers.control || ks.modifiers.platform {
            self.cancel_pane_select();
            return;
        }
        if ks.key == "backspace" {
            ps.typed.pop();
            return;
        }
        let Some(c) = ks.key.chars().next().filter(|c| ks.key.len() == 1 && c.is_ascii_alphabetic())
        else {
            return;
        };
        ps.typed.push(c.to_ascii_lowercase());
        let hit = ps.labels.iter().find(|(_, l)| *l == ps.typed).map(|(p, _)| *p);
        let any_prefix = ps.labels.iter().any(|(_, l)| l.starts_with(&ps.typed));
        if let Some(pane_id) = hit {
            let ps = self.pane_select.take().unwrap();
            self.pane_selected(ps, pane_id);
        } else if !any_prefix {
            self.cancel_pane_select();
        }
        cx.notify();
    }

    fn cancel_pane_select(&mut self) {
        if let Some(ps) = self.pane_select.take() {
            if ps.was_zoomed {
                if let Some(tab) = Mux::get().get_tab(ps.tab_id) {
                    tab.set_zoomed(true);
                }
            }
        }
    }

    fn pane_selected(&mut self, ps: PaneSelect, picked: PaneId) {
        let mux = Mux::get();
        let (Some(tab), Some(pane)) = (mux.get_tab(ps.tab_id), mux.get_pane(picked)) else {
            return;
        };
        if ps.close {
            self.close_picked_pane(&ps, &tab, &pane);
            return;
        }
        match ps.mode {
            PaneSelectMode::Activate => {
                tab.set_active_pane(&pane);
                if ps.was_zoomed {
                    tab.set_zoomed(true);
                }
            }
            PaneSelectMode::SwapWithActive | PaneSelectMode::SwapWithActiveKeepFocus => {
                let Some(active) = tab.get_active_pane() else {
                    return;
                };
                let keep_focus = ps.mode == PaneSelectMode::SwapWithActiveKeepFocus;
                let (a, b) = (active.pane_id(), picked);
                let focus = if keep_focus { a } else { b };
                let tab_id = ps.tab_id;
                promise::spawn::spawn(async move {
                    match swap(tab_id, a, b).await {
                        Ok(()) => send_event(AppEvent::PanesSwapped { tab: tab_id, focus }),
                        Err(e) => send_event(AppEvent::Flash(format!("swap failed: {e:#}"))),
                    }
                })
                .detach();
            }
            PaneSelectMode::MoveToNewTab | PaneSelectMode::MoveToNewWindow => {
                // a new window still belongs to this workspace
                let (window, workspace) = if ps.mode == PaneSelectMode::MoveToNewTab {
                    (self.mux_window_id, None)
                } else {
                    (None, Some(mux.active_workspace()))
                };
                promise::spawn::spawn(async move {
                    match Mux::get().move_pane_to_new_tab(picked, window, workspace).await {
                        Ok((tab, window)) => send_event(AppEvent::TabMoved {
                            window,
                            tab: tab.tab_id(),
                            follow: true,
                        }),
                        Err(e) => send_event(AppEvent::Flash(format!("move failed: {e:#}"))),
                    }
                })
                .detach();
            }
        }
    }
}

impl TermApp {
    /// Close mode: kill `pane`, asking first if it's running something.
    fn close_picked_pane(&mut self, ps: &PaneSelect, tab: &Arc<Tab>, pane: &Arc<dyn Pane>) {
        let picked = pane.pane_id();
        let closing_active = tab.get_active_pane().is_some_and(|a| a.pane_id() == picked);
        // the zoomed pane was the active one; zoom back unless it's the one going
        if ps.was_zoomed && !closing_active {
            tab.set_zoomed(true);
        }
        let now = crate::procs::now(std::slice::from_ref(pane));
        if crate::app::needs_confirmation(|| now.can_close_quietly(pane)) {
            self.overlay = Some(Overlay::Confirm {
                message: "Close this pane?".into(),
                detail: crate::app::running_detail(&now, std::slice::from_ref(pane)),
                verb: "Close",
                cancel_verb: None,
                action: KeyAssignment::CloseCurrentPane { confirm: false },
                cancel: None,
                pane_id: picked,
            });
        } else {
            crate::app::close_pane(tab, pane);
            Mux::get().prune_dead_windows();
        }
    }
}

/// Trade the places of panes `a` and `b` in one tab (see the module comment).
async fn swap(tab_id: TabId, a: PaneId, b: PaneId) -> anyhow::Result<()> {
    let mux = Mux::get();
    let tab = mux
        .get_tab(tab_id)
        .ok_or_else(|| anyhow::anyhow!("tab {tab_id} is gone"))?;
    let panes = tab.iter_panes_ignoring_zoom();
    let find = |id: PaneId| {
        panes
            .iter()
            .find(|p| p.pane.pane_id() == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("pane {id} isn't in this tab"))
    };
    let (pa, pb) = (find(a)?, find(b)?);
    let request = |direction, target_is_second, size| SplitRequest {
        direction,
        target_is_second,
        top_level: false,
        size,
    };
    let half = |direction| request(direction, true, SplitSize::Percent(50));
    let domain = SpawnTabDomain::CurrentPaneDomain;

    if panes.len() == 2 {
        // siblings: with A gone, B fills the tab; put A back on the far side
        let side_by_side = pa.top == pb.top && pa.height == pb.height;
        let (direction, a_first, extent) = if side_by_side {
            (SplitDirection::Horizontal, pa.left < pb.left, pb.width)
        } else {
            (SplitDirection::Vertical, pa.top < pb.top, pb.height)
        };
        mux.split_pane(
            b,
            request(direction, a_first, SplitSize::Cells(extent)),
            SplitSource::MovePane(a),
            domain,
        )
        .await?;
        return Ok(());
    }

    let dir = |p: &mux::tab::PositionedPane| {
        if p.width >= p.height * 2 {
            SplitDirection::Horizontal
        } else {
            SplitDirection::Vertical
        }
    };
    let mut sleep = CommandBuilder::new("sleep");
    sleep.arg("600");
    let (placeholder, _) = mux
        .split_pane(
            b,
            half(dir(&pb)),
            SplitSource::Spawn {
                command: Some(sleep),
                command_dir: None,
            },
            domain.clone(),
        )
        .await?;
    let t = placeholder.pane_id();
    let result = async {
        mux.split_pane(a, half(dir(&pa)), SplitSource::MovePane(b), domain.clone())
            .await?;
        mux.split_pane(t, half(dir(&pb)), SplitSource::MovePane(a), domain.clone())
            .await?;
        anyhow::Ok(())
    }
    .await;
    // the placeholder goes either way
    if let Some(tab) = mux.resolve_pane_id(t).and_then(|(_, _, tab)| mux.get_tab(tab)) {
        crate::app::close_pane(&tab, &placeholder);
    }
    result
}
