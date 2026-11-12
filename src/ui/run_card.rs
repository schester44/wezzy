//! The run card: a slim bar on the bottom edge of an agent's pane when its
//! turn finished with changes (runs.rs): +/−, file count, Review changes. Non-modal: keys keep going to the
//! terminal, and typing into the pane hides it until the next turn. While it
//! shows, the pane's view lifts by the rows it covers (`card_lift`), so the
//! agent's prompt stays visible above it.

use super::git::counts;
use super::widgets::{button, small_icon_button, term_family, ButtonKind};
use super::{Icon, Theme};
use crate::app::TermApp;
use gpui::{div, point, prelude::*, px, AnyElement, ClickEvent, Context, MouseButton, MouseDownEvent};
use mux::pane::PaneId;
use mux::tab::{PositionedPane, Tab};
use wezterm_term::StableRowIndex;

/// Panes smaller than this get no card: it would cover everything.
const MIN_W: f32 = 320.;
const MIN_H: f32 = 160.;
/// The bar's height.
const BAR_H: f32 = 34.;

impl TermApp {
    /// Whether `p` shows a run card: it has a run to review and is big enough.
    fn shows_run_card(&self, tab: &Tab, p: &PositionedPane) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.run_card(p.pane.pane_id())?;
        let b = self.view_bounds(tab, p);
        (f32::from(b.size.width) >= MIN_W && f32::from(b.size.height) >= MIN_H).then_some(b)
    }

    /// How many rows the view of `p` lifts so its last lines show above the
    /// run card: the rows the card covers, less blank rows at the bottom
    /// (below the cursor), which can stay under it.
    pub(crate) fn card_lift(&self, tab: &Tab, p: &PositionedPane) -> usize {
        if crate::views::is_view(&p.pane) {
            return 0;
        }
        let Some(b) = self.shows_run_card(tab, p) else {
            return 0;
        };
        let g = self.grid.get();
        if g.line_h <= px(0.) {
            return 0;
        }
        let bar_top = b.origin.y + b.size.height - px(BAR_H);
        let rows_top = g.origin.y + g.line_h * p.top as f32;
        let fit = ((bar_top - rows_top) / g.line_h).floor().max(0.) as usize;
        let covered = p.height.saturating_sub(fit);
        if covered == 0 {
            return 0;
        }
        let d = p.pane.get_dimensions();
        let top = d.physical_top;
        let (_, lines) = p.pane.get_lines(top..top + d.viewport_rows as StableRowIndex);
        let cursor = (p.pane.get_cursor_position().y - top).max(0) as usize;
        let blank = lines
            .iter()
            .enumerate()
            .rev()
            .take_while(|(i, l)| *i > cursor && l.is_whitespace())
            .count();
        covered.saturating_sub(blank)
    }

    /// The first row shown in `pane`: where you scrolled to, else the bottom
    /// of the screen, lifted above a run card. `None` is the live screen.
    pub(crate) fn view_top(&self, pane: PaneId) -> Option<StableRowIndex> {
        if let Some(top) = self.viewports.get(&pane) {
            return Some(*top);
        }
        let tab = self.active_tab()?;
        let p = tab.iter_panes().into_iter().find(|p| p.pane.pane_id() == pane)?;
        let lift = self.card_lift(&tab, &p);
        (lift > 0).then(|| p.pane.get_dimensions().physical_top + lift as StableRowIndex)
    }

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
                let b = self.shows_run_card(&tab, &p)?;
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
            .h(px(BAR_H))
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
