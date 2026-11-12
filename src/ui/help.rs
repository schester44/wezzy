//! Shortcut sheets: the git panel's (`?`) and the global one (Leader ?).
//! Both are drawn over the whole window. Typing filters them; esc, ↵ or a
//! click closes them.

use super::widgets::kbd;
use super::{icon, Icon, Theme};
use crate::app::{Overlay, TermApp};
use crate::shortcuts::{Group, Row};
use gpui::{div, prelude::*, px, relative, AnyElement, Context, FontWeight, MouseButton, MouseDownEvent};

impl TermApp {
    pub(super) fn shortcuts_modal(&self, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let Some(Overlay::Shortcuts(query)) = &self.overlay else {
            return None;
        };
        let columns = crate::shortcuts::sheet(&self.keys);
        let width = 300. * columns.len().max(2) as f32;
        Some(sheet(
            "Keyboard shortcuts",
            query,
            columns,
            width,
            t,
            cx,
            |this| this.overlay = None,
        ))
    }
}

impl TermApp {
    /// A view's sheet (`?` inside it).
    pub(super) fn view_shortcuts_modal(&self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Overlay::ViewShortcuts(pane_id, query)) = &self.overlay else {
            return None;
        };
        let (title, columns) = self.views.shortcuts(*pane_id)?;
        let width = 300. * columns.len().max(2) as f32;
        Some(sheet(title, query, columns, width, t, cx, |this| this.overlay = None))
    }
}

/// Static rows for hand-written sheets: keys are space-separated alternatives.
pub(crate) fn group(title: &str, rows: &[(&str, &str)]) -> Group {
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

/// The search field that heads a sheet or a list: the title stands in until
/// you type.
pub(super) fn search_header(title: &'static str, query: &str, active: bool, t: &Theme) -> gpui::Div {
    let caret = || div().w(px(1.5)).h(px(15.)).bg(t.accent);
    div()
        .flex_none()
        .h(px(48.))
        .flex()
        .items_center()
        .gap_2p5()
        .border_b_1()
        .border_color(t.border)
        .child(icon(Icon::Search, 14., t.fg_subtle))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .map(|d| {
                    if query.is_empty() {
                        d.when(active, |d| d.child(caret()))
                            .child(div().ml_px().font_weight(FontWeight::MEDIUM).child(title))
                    } else {
                        d.child(query.to_string()).when(active, |d| d.child(div().ml_px().child(caret())))
                    }
                }),
        )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn sheet(
    title: &'static str,
    query: &str,
    columns: Vec<Vec<Group>>,
    width: f32,
    t: &Theme,
    cx: &mut Context<TermApp>,
    close: fn(&mut TermApp),
) -> AnyElement {
    let columns = crate::shortcuts::filter(columns, query);
    let empty = columns.is_empty();
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
        .occlude()
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
                .child(search_header(title, query, true, t).px_5())
                .child(
                    div()
                        .id("shortcut-sheet")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .p_5()
                        .flex()
                        .gap_8()
                        .children(columns.into_iter().map(column))
                        .when(empty, |d| {
                            d.justify_center()
                                .py_3()
                                .text_size(px(12.))
                                .text_color(t.fg_subtle)
                                .child("No matches")
                        }),
                ),
        )
        .into_any_element()
}
