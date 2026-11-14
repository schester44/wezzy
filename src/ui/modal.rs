//! Centered overlays (prompt, confirm, palette, settings) and the toast.

use super::view::CHROME_H;
use super::widgets::{button, term_family, with_alpha, ButtonKind};
use super::Theme;
use crate::app::{Overlay, TermApp};
use config::keyassignment::KeyAssignment;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, MouseButton, MouseDownEvent};

impl TermApp {
    pub(super) fn modal(&self, t: &Theme, window: &gpui::Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let overlay = self.overlay.as_ref()?;
        let card = |w: f32| {
            div()
                .w(px(w))
                .flex()
                .flex_col()
                .rounded_xl()
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .shadow_2xl()
                .overflow_hidden()
        };

        let (top, content): (f32, AnyElement) = match overlay {
            Overlay::Prompt {
                description,
                prompt,
                input,
                ..
            } => (
                72.,
                card(460.)
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(if description.is_empty() {
                                "Input".to_string()
                            } else {
                                description.clone()
                            }),
                    )
                    .child(
                        div()
                            .h(px(38.))
                            .px_3()
                            .flex()
                            .items_center()
                            .rounded_lg()
                            .bg(gpui::hsla(0., 0., 0., 0.25))
                            .border_1()
                            .border_color(with_alpha(t.accent, 0.7))
                            .font_family(term_family())
                            .when(!prompt.trim().is_empty() && prompt.trim() != ">", |d| {
                                d.child(div().text_color(t.fg_subtle).mr_1().child(prompt.clone()))
                            })
                            .child(input.clone())
                            .child(div().ml_px().w(px(2.)).h(px(16.)).bg(t.accent)),
                    )
                    .into_any_element(),
            ),
            Overlay::Confirm {
                message,
                detail,
                verb,
                cancel_verb,
                action,
                ..
            } => {
                let destructive = match action {
                    KeyAssignment::QuitApplication
                    | KeyAssignment::CloseCurrentPane { .. }
                    | KeyAssignment::CloseCurrentTab { .. } => true,
                    // undoing an agent's work throws edits away
                    KeyAssignment::EmitEvent(name) => crate::runs::confirmed_undo(name).is_some(),
                    _ => false,
                };
                (
                    96.,
                    card(340.)
                        .child(
                            div()
                                .px_4()
                                .pt_4()
                                .pb_3()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().font_weight(FontWeight::MEDIUM).child(message.clone()))
                                .when(!detail.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(t.fg_muted)
                                            .child(detail.clone()),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .px_4()
                                .py_3()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .border_t_1()
                                .border_color(t.border)
                                .bg(gpui::hsla(0., 0., 0., 0.14))
                                .child(button(
                                    "confirm-no",
                                    cancel_verb.unwrap_or("Cancel"),
                                    if cancel_verb.is_some() { "n" } else { "esc" },
                                    t,
                                    ButtonKind::Ghost,
                                ).on_click(
                                    cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.answer_confirm(false, window, cx)
                                    }),
                                ))
                                .child(
                                    button(
                                        "confirm-yes",
                                        verb,
                                        "↩",
                                        t,
                                        if destructive { ButtonKind::Danger } else { ButtonKind::Soft },
                                    )
                                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                        this.answer_confirm(true, window, cx)
                                    })),
                                ),
                        )
                        .into_any_element(),
                )
            }
            Overlay::Palette(state) => (
                56.,
                card(620.).child(self.palette_view(state, t, cx)).into_any_element(),
            ),
            Overlay::Settings(state) => (
                72.,
                card(460.).child(self.settings_view(state, t, cx)).into_any_element(),
            ),
            Overlay::Queue(state) => (
                56.,
                card(560.).child(self.queue_view(state, t, cx)).into_any_element(),
            ),
            Overlay::Board(state) => {
                // as wide and tall as the window allows: lanes need the room
                let view = window.viewport_size();
                let width = (f32::from(view.width) - 64.).clamp(480., 1280.);
                let lanes = (f32::from(view.height) - CHROME_H - 40. - 48. - 96.).clamp(240., 720.);
                (
                    40.,
                    card(width).child(self.board_view(state, lanes, t, cx)).into_any_element(),
                )
            }
            Overlay::Shortcuts(_) | Overlay::ViewShortcuts(..) => return None,
        };

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .flex_col()
                .items_center()
                // offsets are measured below the top bar
                .pt(px(top + CHROME_H))
                .bg(gpui::hsla(0., 0., 0., 0.28))
                // keep wheel and hover from reaching the terminal underneath
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, window, cx| {
                        // click on the backdrop dismisses
                        this.overlay = None;
                        this.focus.focus(window);
                        cx.notify();
                    }),
                )
                .child(
                    // swallow clicks inside the card
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(content),
                ),
        )
    }

    pub(super) fn toast(&self, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (msg, _) = self.flash.as_ref()?;
        let agent = self.flash_agent.as_ref();
        let target = agent.map(|a| a.pane);
        // done: green; needs you: amber; anything else: accent
        let dot = match agent {
            Some(a) if a.done => t.success,
            Some(a) if a.body.is_some() => t.warning,
            _ => t.accent,
        };
        let body = agent.and_then(|a| a.body.clone());
        // the turn's changes; stats land a moment after the toast does
        let run = agent
            .filter(|a| a.done)
            .and_then(|a| crate::runs::reviewable(a.pane))
            .and_then(|r| r.stats);
        Some(
            div()
                .id("toast")
                .absolute()
                .bottom_4()
                .right_4()
                .max_w(px(420.))
                .px_3()
                .py_2()
                .flex()
                .items_start()
                .gap_2()
                .rounded_lg()
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .child(
                    // centered on the first line
                    div()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .child(div().size(px(6.)).flex_none().rounded_full().bg(dot)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .truncate()
                                .when(agent.is_some(), |d| d.font_weight(FontWeight::MEDIUM))
                                .child(msg.clone()),
                        )
                        .when_some(body, |d, b| {
                            d.child(div().text_size(px(12.)).text_color(t.fg_muted).line_clamp(3).child(b))
                        })
                        .when_some(run, |d, s| {
                            d.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(super::git::counts(s.added, s.removed, t))
                                    .child(
                                        div()
                                            .font_family(term_family())
                                            .text_size(px(11.))
                                            .text_color(t.fg_subtle)
                                            .child(crate::runs::files(s.files.len())),
                                    ),
                            )
                        }),
                )
                .when_some(target, |d, pane_id| {
                    d.cursor_pointer().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.flash = None;
                        this.flash_agent = None;
                        this.region = crate::regions::Region::Terminal;
                        this.focus_pane(pane_id, window, cx);
                    }))
                }),
        )
    }
}
