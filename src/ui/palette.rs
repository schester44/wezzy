//! The command palette's list.

use super::widgets::{kbd, status_indicator, with_alpha};
use super::{icon, Icon, Theme};
use crate::app::TermApp;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight};

impl TermApp {
    pub(super) fn palette_view(
        &self,
        state: &crate::palette::PaletteState,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::palette::{Kind, Scope};
        let results = self.palette_results();
        let grouped = state.query.trim().is_empty();
        let selected = state.selected.min(results.len().saturating_sub(1));

        let mut rows: Vec<AnyElement> = vec![];
        let mut last_kind = None;
        for (i, item) in results.into_iter().enumerate() {
            if grouped && last_kind != Some(item.kind) {
                last_kind = Some(item.kind);
                rows.push(
                    div()
                        .px_4()
                        .pt_3()
                        .pb_1p5()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.fg_subtle)
                        .child(item.kind.section())
                        .into_any_element(),
                );
            }
            let is_sel = i == selected;
            let leading: AnyElement = match (item.kind, item.status) {
                (Kind::Agent, Some((status, unseen))) => status_indicator(status, unseen, t).into_any_element(),
                (kind, _) => icon(
                    match kind {
                        Kind::Agent | Kind::Launch => Icon::Bot,
                        Kind::Tab => Icon::Terminal,
                        Kind::Window => Icon::Window,
                        Kind::Workspace => Icon::Layers,
                        Kind::Command => Icon::Bolt,
                        Kind::Recipe => Icon::Sparkles,
                        Kind::Host => Icon::Server,
                        Kind::Database => Icon::Database,
                    },
                    14.,
                    if is_sel { t.fg } else { t.fg_subtle },
                )
                .into_any_element(),
            };
            let action = item.action.clone();
            rows.push(
                // full-width row; selection = faint fill + accent bar (docs/development/design.md)
                div()
                    .id(("palette-row", i))
                    .w_full()
                    .h(px(36.))
                    .pr_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .map(|d| {
                        if is_sel {
                            d.bg(with_alpha(t.fg, 0.07))
                        } else {
                            d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
                        }
                    })
                    .child(
                        div()
                            .w(px(2.))
                            .h_full()
                            .flex_none()
                            .when(is_sel, |d| d.bg(t.accent)),
                    )
                    .child(
                        div()
                            .ml(px(10.))
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(leading),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(px(300.))
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(t.fg)
                                    .child(item.title),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(t.fg_subtle)
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(item.subtitle),
                            ),
                    )
                    .when_some(item.shortcut, |d, sc| d.child(kbd(&sc, t)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.overlay = None;
                        this.run_palette_action(action.clone(), window, cx);
                    }))
                    .into_any_element(),
            );
        }
        let empty = rows.is_empty();

        div()
            .flex()
            .flex_col()
            // search field
            .child(
                div()
                    .h(px(48.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .border_b_1()
                    .border_color(t.border)
                    .child(icon(Icon::Search, 15., t.fg_subtle))
                    .when(state.scope != Scope::All, |d| {
                        d.child(
                            div()
                                .h(px(20.))
                                .px_1p5()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(t.border)
                                .bg(t.hover)
                                .text_color(t.fg_muted)
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(if state.move_tab.is_some() {
                                    "Move tab to"
                                } else {
                                    state.scope.label()
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .text_size(px(14.))
                            .when(state.query.is_empty(), |d| {
                                d.child(div().w(px(1.5)).h(px(17.)).bg(t.accent)).child(
                                    div()
                                        .ml_1()
                                        .text_color(t.fg_subtle)
                                        .child("Search commands, agents, tabs…"),
                                )
                            })
                            .when(!state.query.is_empty(), |d| {
                                d.child(state.query.clone())
                                    .child(div().ml_px().w(px(1.5)).h(px(17.)).bg(t.accent))
                            }),
                    ),
            )
            // results
            .child(
                div()
                    .id("palette-list")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .track_scroll(&state.scroll)
                    .pb_1p5()
                    .children(rows)
                    .when(empty, |d| {
                        d.child(
                            div()
                                .py_8()
                                .flex()
                                .justify_center()
                                .text_size(px(12.))
                                .text_color(t.fg_subtle)
                                .child(match state.query.trim() {
                                    q if state.scope == Scope::Workspaces && !q.is_empty() => {
                                        format!("Press Enter to create workspace “{q}”")
                                    }
                                    _ => "No matches".to_string(),
                                }),
                        )
                    }),
            )
    }
}
