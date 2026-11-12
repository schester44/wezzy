//! The left side-panel shell shared by the agents sidebar and the git panel:
//! fixed width, chrome background, hairline right border, and a drag handle on
//! the right edge. Width changes (drag or ⌃←/⌃→) go through `set_panel_width`.

use super::view::with_alpha;
use super::Theme;
use crate::app::TermApp;
use gpui::{div, prelude::*, px, Context, Div, MouseButton, MouseDownEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidePanel {
    Sidebar,
    Git,
}

impl SidePanel {
    fn min_width(self) -> f32 {
        match self {
            SidePanel::Sidebar => 180.,
            SidePanel::Git => 260.,
        }
    }
}

impl TermApp {
    pub(crate) fn panel_width(&self, panel: SidePanel) -> f32 {
        match panel {
            SidePanel::Sidebar => self.sidebar_width,
            SidePanel::Git => self.diff_width,
        }
    }

    /// Set a panel's width, clamped to its minimum and 80% of the window.
    pub(crate) fn set_panel_width(&mut self, panel: SidePanel, w: f32, window_w: f32) {
        let w = w.clamp(panel.min_width(), (window_w * 0.8).max(panel.min_width()));
        match panel {
            SidePanel::Sidebar => self.sidebar_width = w,
            SidePanel::Git => self.diff_width = w,
        }
    }

    /// The panel frame; callers add their content as children.
    pub(super) fn side_panel(&self, panel: SidePanel, t: &Theme, cx: &mut Context<Self>) -> Div {
        div()
            .relative()
            .w(px(self.panel_width(panel)))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.chrome)
            // flat panes need the edge; cards have their own
            .when(!crate::settings::get().pane_cards, |d| d.border_r_1().border_color(t.border))
            // drag the right edge to resize
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(-3.))
                    .w(px(6.))
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .hover(|s| s.bg(with_alpha(t.accent, 0.35)))
                    .when(self.panel_drag == Some(panel), |d| d.bg(with_alpha(t.accent, 0.5)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.panel_drag = Some(panel);
                            cx.notify();
                        }),
                    ),
            )
    }
}
