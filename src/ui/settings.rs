//! The settings overlay's rows.

use super::widgets::{term_family, with_alpha};
use super::{icon, Icon, Theme};
use crate::app::TermApp;
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight};

impl TermApp {
    pub(super) fn settings_view(
        &self,
        state: &crate::settings_ui::SettingsState,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use crate::settings_ui::{Row, ROWS};
        let rows = ROWS.iter().enumerate().map(|(i, row)| {
            let is_cur = i == state.cursor;
            let editing = is_cur && *row == Row::Editor && state.editing.is_some();
            let value: AnyElement = if editing {
                let text = state.editing.clone().unwrap_or_default();
                div()
                    .h(px(26.))
                    .min_w(px(180.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_lg()
                    .bg(gpui::hsla(0., 0., 0., 0.2))
                    .border_1()
                    .border_color(with_alpha(t.accent, 0.55))
                    .font_family(term_family())
                    .text_size(px(12.))
                    .child(text)
                    .child(div().ml_px().w(px(1.5)).h(px(14.)).bg(t.accent))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_size(px(12.))
                    .text_color(if is_cur { t.fg } else { t.fg_muted })
                    .when(*row == Row::Editor, |d| d.font_family(term_family()))
                    .child(row.value(self.sidebar_open))
                    .when(row.is_choice(), |d| {
                        d.child(icon(Icon::ChevronDown, 11., t.fg_subtle))
                    })
                    .into_any_element()
            };
            // full-width row; selection = faint fill + accent bar (docs/development/design.md)
            div()
                .id(("setting", i))
                .w_full()
                .h(px(40.))
                .pr_4()
                .flex()
                .items_center()
                .cursor_pointer()
                .map(|d| {
                    if is_cur {
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
                        .when(is_cur, |d| d.bg(t.accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .pl(px(14.))
                        .text_color(t.fg)
                        .child(row.label()),
                )
                .child(value)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.settings_click(i, window, cx)
                }))
        });
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.))
                    .px_4()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(t.border)
                    .font_weight(FontWeight::MEDIUM)
                    .child("Settings"),
            )
            .child(div().py_1p5().children(rows))
    }
}
