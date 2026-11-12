//! The run card: a slim bar on the bottom edge of an agent's pane when its
//! turn finished with changes (runs.rs): +/−, file count, Review changes. Non-modal: keys keep going to the
//! terminal, and typing into the pane hides it until the next turn.

use super::git::counts;
use super::widgets::{button, small_icon_button, term_family, ButtonKind};
use super::{Icon, Theme};
use crate::app::TermApp;
use gpui::{div, point, prelude::*, px, AnyElement, ClickEvent, Context, MouseButton, MouseDownEvent};
use mux::pane::PaneId;

/// Panes smaller than this get no card: it would cover everything.
const MIN_W: f32 = 320.;
const MIN_H: f32 = 160.;

impl TermApp {
    /// A run card docked to the bottom of each pane of the active tab whose
    /// agent has a run to review.
    pub(super) fn run_cards(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        let g = self.grid.get();
        let area = point(g.origin.x - g.pad.x, g.origin.y - g.pad.y);
        // flush with the pane: its bottom corners when panes are cards
        let radius = if crate::settings::get().pane_cards {
            g.card_radius() - px(1.)
        } else {
            px(0.)
        };
        tab.iter_panes()
            .into_iter()
            .filter_map(|p| {
                let pane = p.pane.pane_id();
                let run = self.run_card(pane)?;
                let b = self.view_bounds(&tab, &p);
                // too small to hold it without covering everything
                if f32::from(b.size.width) < MIN_W || f32::from(b.size.height) < MIN_H {
                    return None;
                }
                Some(
                    // the pane's box, the card at its bottom edge; the box
                    // takes no mouse events itself, only the card does
                    div()
                        .absolute()
                        .left(b.origin.x - area.x)
                        .top(b.origin.y - area.y)
                        .w(b.size.width)
                        .h(b.size.height)
                        .flex()
                        .flex_col()
                        .justify_end()
                        .child(self.run_card_view(pane, &run, radius, t, cx))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// A slim bar: the run's +/− and file count, Review changes, ×.
    fn run_card_view(
        &self,
        pane: PaneId,
        run: &crate::activity::Run,
        radius: gpui::Pixels,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let stats = run.stats.clone().unwrap_or_default();
        div()
            .id(("run-card", pane))
            .w_full()
            .h(px(34.))
            .pl_3()
            .pr_1p5()
            .flex()
            .items_center()
            .gap_2()
            .bg(t.elevated)
            .border_t_1()
            .border_color(t.border)
            .rounded_b(radius)
            .text_size(px(12.))
            // the bar's clicks and hovers stay off the terminal underneath
            .occlude()
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| cx.stop_propagation())
            // finished: the done dot
            .child(div().size(px(6.)).flex_none().rounded_full().bg(t.success))
            .child(counts(stats.added, stats.removed, t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(term_family())
                    .text_size(px(11.))
                    .text_color(t.fg_subtle)
                    .child(crate::runs::files(stats.files.len())),
            )
            .child(
                button("run-review", "Review changes", "", t, ButtonKind::Ghost)
                    .h(px(22.))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.review_run(pane, window, cx);
                        cx.notify();
                    })),
            )
            .child(small_icon_button("run-dismiss", Icon::X, t).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.end_run(pane);
                    cx.notify();
                },
            )))
    }
}
