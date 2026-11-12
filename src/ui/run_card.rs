//! The run card: floats over the bottom of an agent's pane when its turn
//! finished with changes (runs.rs). Non-modal: keys keep going to the
//! terminal, and typing into the pane hides it until the next turn.

use super::git::counts;
use super::widgets::{menu_row, small_icon_button, term_family};
use super::{icon, Icon, Theme};
use crate::app::TermApp;
use gpui::{div, point, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent, Window};
use mux::pane::PaneId;

const CARD_W: f32 = 380.;
const INSET: f32 = 12.;

#[derive(Clone, Copy)]
enum Action {
    Review,
    Ask,
    Commit,
    Undo,
}

impl TermApp {
    /// A card over each pane of the active tab whose agent has a run to review.
    pub(super) fn run_cards(&self, t: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        let g = self.grid.get();
        let area = point(g.origin.x - g.pad.x, g.origin.y - g.pad.y);
        tab.iter_panes()
            .into_iter()
            .filter_map(|p| {
                let pane = p.pane.pane_id();
                let run = self.run_card(pane)?;
                let b = self.view_bounds(&tab, &p);
                // too small to hold it without covering everything
                if f32::from(b.size.width) < 260. || f32::from(b.size.height) < 220. {
                    return None;
                }
                let w = px(CARD_W.min(f32::from(b.size.width) - INSET * 2.));
                Some(
                    // the pane's box, bottom-right aligned; it takes no mouse
                    // events itself, only the card does
                    div()
                        .absolute()
                        .left(b.origin.x - area.x)
                        .top(b.origin.y - area.y)
                        .w(b.size.width)
                        .h(b.size.height)
                        .p(px(INSET))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .items_end()
                        .child(self.run_card_view(pane, &run, w, t, cx))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn run_card_view(
        &self,
        pane: PaneId,
        run: &crate::activity::Run,
        w: gpui::Pixels,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let stats = run.stats.clone().unwrap_or_default();
        let undoable = run.turns.last().is_some_and(|turn| !turn.files.is_empty());
        let actions = [
            (Action::Review, Icon::Search, "Review changes"),
            (Action::Ask, Icon::Message, "Ask for changes"),
            (Action::Commit, Icon::GitCommit, "Commit"),
            (Action::Undo, Icon::Undo, "Undo turn"),
        ];
        let rows = actions
            .into_iter()
            .filter(|(a, ..)| undoable || !matches!(a, Action::Undo))
            .map(|(a, ic, label)| {
                menu_row(("run-action", a as usize), t)
                    .child(icon(ic, 13., t.fg_subtle))
                    .child(div().flex_1().child(label))
                    .child(icon(Icon::ChevronRight, 12., t.fg_subtle))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.run_card_action(pane, a, window, cx);
                        cx.notify();
                    }))
            });

        div()
            .id(("run-card", pane))
            .w(w)
            .flex()
            .flex_col()
            .rounded_xl()
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .overflow_hidden()
            // the card's clicks and hovers stay off the terminal underneath
            .occlude()
            .on_mouse_down(MouseButton::Left, |_: &MouseDownEvent, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px_4()
                    .pt_3()
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
                            // finished: the done dot
                            .child(div().size(px(6.)).flex_none().rounded_full().bg(t.success))
                            .child(div().w_0p5())
                            .child(icon(Icon::GitBranch, 13., t.fg_subtle))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(run.branch.clone()),
                            )
                            .child(small_icon_button("run-dismiss", Icon::X, t).on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.end_run(pane);
                                    cx.notify();
                                },
                            ))),
                    )
                    .children(run.summary.clone().map(|s| div().line_clamp(3).child(s)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(px(12.))
                            .text_color(t.fg_subtle)
                            .child(counts(stats.added, stats.removed, t))
                            .child(
                                div()
                                    .font_family(term_family())
                                    .text_size(px(11.))
                                    .child(format!("· {}", crate::runs::files(stats.files.len()))),
                            ),
                    ),
            )
            .child(
                div()
                    .p_1()
                    .flex()
                    .flex_col()
                    .text_color(t.fg)
                    .children(rows),
            )
    }

    fn run_card_action(&mut self, pane: PaneId, a: Action, window: &mut Window, cx: &mut Context<Self>) {
        match a {
            Action::Review => self.review_run(pane, false, window, cx),
            Action::Ask => self.review_run(pane, true, window, cx),
            Action::Commit => self.commit_run(pane, window, cx),
            Action::Undo => self.ask_undo(pane, false),
        }
    }
}
