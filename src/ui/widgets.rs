//! Small shared components. docs/development/design.md describes each.

use super::{icon, spinner, Icon, Theme};
use crate::agents::Status;
use gpui::{div, prelude::*, px, AnyElement, FontWeight, Hsla, SharedString};

/// `unseen`: it finished while you were elsewhere and you haven't been back.
/// Done stays bright until you go to it, then fades to the idle dot.
pub(super) fn status_indicator(status: Status, unseen: bool, t: &Theme) -> AnyElement {
    match status {
        Status::Working => spinner(12., t.fg_muted).into_any_element(),
        Status::NeedsInput => icon(Icon::Alert, 13., t.warning).into_any_element(),
        Status::Done if unseen => div()
            .size(px(6.))
            .rounded_full()
            .bg(t.success)
            .into_any_element(),
        Status::Done | Status::Idle => div()
            .size(px(6.))
            .rounded_full()
            .bg(t.fg_subtle)
            .into_any_element(),
    }
}

/// A key cap: mono, 10px, hairline border. The one way shortcuts are shown.
pub(super) fn kbd(label: &str, t: &Theme) -> impl IntoElement {
    div()
        .flex_none()
        .min_w(px(18.))
        .h(px(18.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .border_1()
        .border_color(t.border)
        .bg(t.hover)
        .font_family(term_family())
        .text_size(px(10.))
        .text_color(t.fg_muted)
        .child(SharedString::from(label.to_string()))
}

pub(super) fn section_header(title: &str, badge: Option<String>, t: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .h(px(32.))
        .mt_1()
        .px_4()
        .flex()
        .items_center()
        .gap_1p5()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.fg_muted)
        .child(SharedString::from(title.to_string()))
        .children(badge.map(|b| {
            div()
                .font_family(term_family())
                .text_size(px(11.))
                .font_weight(FontWeight::NORMAL)
                .text_color(t.fg_subtle)
                .child(b)
        }))
}

/// Sidebar row selection (docs/development/design.md): faint full-width fill + 2px left bar.
/// `cursor` is the keyboard cursor while the sidebar has focus (accent bar);
/// `current` marks the pane you're in (quieter fill, no bar).
pub(super) fn row_selection(d: gpui::Stateful<gpui::Div>, cursor: bool, current: bool, t: &Theme) -> gpui::Stateful<gpui::Div> {
    let bar = div().absolute().left_0().top_0().bottom_0().w(px(2.));
    if cursor {
        d.bg(with_alpha(t.fg, 0.07)).child(bar.bg(t.accent))
    } else if current {
        d.bg(with_alpha(t.fg, 0.04))
            .hover(|s| s.bg(with_alpha(t.fg, 0.06)))
    } else {
        d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
    }
}

pub(super) fn small_icon_button(id: &'static str, ic: Icon, t: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(t.hover))
        .child(icon(ic, 13., t.fg_subtle))
}

pub(super) fn menu_row(id: impl Into<gpui::ElementId>, t: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(30.))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(t.hover))
}

#[derive(Clone, Copy)]
pub(super) enum ButtonKind {
    /// hairline border, fill on hover
    Ghost,
    /// soft `fg` fill: the default action
    Soft,
    /// `danger` fill: an action that kills something
    Danger,
}

pub(super) fn button(
    id: &'static str,
    label: &'static str,
    key: &'static str,
    t: &Theme,
    kind: ButtonKind,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(24.))
        .px_2p5()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_md()
        .cursor_pointer()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .map(|d| match kind {
            ButtonKind::Ghost => d
                .border_1()
                .border_color(t.border)
                .text_color(t.fg)
                .hover(|s| s.bg(t.hover)),
            ButtonKind::Soft => d
                .border_1()
                .border_color(t.border)
                .bg(with_alpha(t.fg, 0.07))
                .text_color(t.fg)
                .hover(|s| s.bg(with_alpha(t.fg, 0.12))),
            ButtonKind::Danger => d
                .bg(t.danger)
                .text_color(gpui::white())
                .hover(|s| s.opacity(0.88)),
        })
        .child(label)
        .child(div().opacity(0.55).font_family(term_family()).text_size(px(11.)).child(key))
}

pub(super) fn pill(label: &str, color: Hsla) -> impl IntoElement {
    div()
        .px_1p5()
        .py(px(1.))
        .rounded_md()
        .bg(with_alpha(color, 0.18))
        .text_color(color)
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .child(SharedString::from(label.to_string()))
}

pub(crate) fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

pub(crate) fn term_family() -> SharedString {
    config::configuration()
        .font
        .font
        .first()
        .map(|f| f.family.clone())
        .unwrap_or_else(|| "Menlo".into())
        .into()
}
