//! Shortcut sheets: the git panel's (`?`) and the global one (Leader ?).
//! Both are drawn over the whole window; any key or click closes them.

use super::view::kbd;
use super::Theme;
use crate::app::{Overlay, TermApp};
use crate::shortcuts::{Group, Row};
use gpui::{div, prelude::*, px, relative, AnyElement, Context, FontWeight, MouseButton, MouseDownEvent};

impl TermApp {
    pub(super) fn shortcuts_modal(&self, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !matches!(self.overlay, Some(Overlay::Shortcuts)) {
            return None;
        }
        let columns = crate::shortcuts::sheet(&self.keys);
        let width = 300. * columns.len().max(2) as f32;
        Some(sheet(
            "Keyboard shortcuts",
            columns,
            width,
            t,
            cx,
            |this| this.overlay = None,
        ))
    }
}

/// Static rows for hand-written sheets: keys are space-separated alternatives.
pub(super) fn group(title: &str, rows: &[(&str, &str)]) -> Group {
    Group {
        title: title.into(),
        rows: rows
            .iter()
            .map(|(k, l)| Row {
                keys: k.split(' ').map(str::to_string).collect(),
                label: l.to_string(),
            })
            .collect(),
    }
}

pub(super) fn sheet(
    title: &'static str,
    columns: Vec<Vec<Group>>,
    width: f32,
    t: &Theme,
    cx: &mut Context<TermApp>,
    close: fn(&mut TermApp),
) -> AnyElement {
    let column = |groups: Vec<Group>| {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_5()
            .children(groups.into_iter().map(|g| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.fg_subtle)
                            .child(g.title),
                    )
                    .children(g.rows.into_iter().map(|r| {
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(12.))
                            .child(div().flex_1().min_w_0().truncate().text_color(t.fg_muted).child(r.label))
                            .child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .gap_1()
                                    .children(r.keys.iter().map(|k| kbd(k, t))),
                            )
                    }))
            }))
    };
    div()
        .absolute()
        .inset_0()
        .bg(gpui::hsla(0., 0., 0., 0.35))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                close(this);
                cx.notify();
            }),
        )
        .child(
            div()
                .w(px(width))
                .max_w_full()
                .max_h(relative(0.9))
                .mx_6()
                .flex()
                .flex_col()
                .rounded_xl()
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .shadow_2xl()
                .overflow_hidden()
                .child(
                    div()
                        .flex_none()
                        .h(px(48.))
                        .px_5()
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(t.border)
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(
                    div()
                        .id("shortcut-sheet")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_5()
                        .flex()
                        .gap_8()
                        .children(columns.into_iter().map(column)),
                )
                .child(
                    div()
                        .flex_none()
                        .px_5()
                        .pb_4()
                        .text_size(px(11.))
                        .text_color(t.fg_subtle)
                        .child("Press any key to close"),
                ),
        )
        .into_any_element()
}
