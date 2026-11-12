//! Layout:
//!
//!   ┌────────────────────────────────────────────────────────────┐
//!   │ ● ● ●   ⌂ default ⌄ │ (zsh)  claude  +           LEADER   │  top bar
//!   ├──────────────┬─────────────────────────────────────────────┤
//!   │ AGENTS     2 │                                             │
//!   │ ◌ Claude     │                 terminal                    │
//!   │   task…      │                                 ┌───────┐   │
//!   │              │                                 │ toast │   │
//!   │ status       │                                 └───────┘   │
//!   └──────────────┴─────────────────────────────────────────────┘

use super::Theme;
use crate::app::TermApp;
use gpui::{div, prelude::*, px, Context, MouseButton, Window};
use std::time::Duration;

pub(super) const CHROME_H: f32 = 40.;
/// Room for the macOS traffic lights when the sidebar is hidden.
pub(super) const TRAFFIC_LIGHTS_W: f32 = 88.;
pub(super) const UI_FONT: &str = ".SystemUIFont";

impl Render for TermApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = config::configuration();
        self.views.sync();
        self.land_git_cursor();
        let tab = self.active_tab();
        let pane = tab.as_ref().and_then(|t| t.get_active_pane());
        // a zoomed pane (⌘↵) takes the whole window: side panels hide while it lasts
        let zoomed = self.pane_zoomed();
        let cards = crate::settings::get().pane_cards;
        if zoomed && self.region != crate::regions::Region::Terminal {
            self.region = crate::regions::Region::Terminal;
        }
        let palette = pane
            .as_ref()
            .map(|p| p.palette())
            .unwrap_or_else(|| config.resolved_palette.clone().into());
        let t = Theme::new(&palette, config.window_background_opacity);

        if let Some(p) = &pane {
            window.set_window_title(&p.get_title());
            // you're looking at it: whatever its agent finished is seen
            if window.is_window_active() {
                crate::agents::mark_seen(p.pane_id());
            }
        }
        // point the diff panel at the active pane's repo (or its agent's worktree)
        crate::diff::set_target(
            (self.diff_open && !zoomed)
                .then(|| pane.as_ref().and_then(crate::gitpanel::git_dir))
                .flatten(),
        );
        // and the Actions source at its branch, while it's on screen
        crate::actions::set_target(
            (self.sidebar_open
                && !zoomed
                && crate::settings::get().sidebar_enabled(crate::sidebar::Source::Actions))
            .then(|| {
                pane.as_ref()
                    .and_then(|p| p.get_current_working_dir(mux::pane::CachePolicy::AllowStale))
                    .map(|u| std::path::PathBuf::from(u.path()))
            })
            .flatten(),
        );
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(3))
        {
            self.flash = None;
            self.flash_pane = None;
        }

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .font_family(UI_FONT)
            .text_size(px(13.))
            .text_color(t.fg)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_modifiers_changed(cx.listener(|this, ev: &gpui::ModifiersChangedEvent, _, cx| {
                let m = &ev.modifiers;
                let held = m.platform && m.alt && !m.control && !m.shift;
                if held != this.agent_keys_held {
                    this.agent_keys_held = held;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &crate::app::OpenSettings, _, cx| {
                this.open_settings(cx)
            }))
            .on_action(cx.listener(|this, _: &crate::app::ShowPalette, _, cx| {
                this.open_palette(crate::palette::Scope::All, cx)
            }))
            .on_mouse_move(cx.listener(|this, ev: &gpui::MouseMoveEvent, window, cx| {
                if this.drag_view(ev, cx) || this.drag_split(ev, cx) || this.drag_selection(ev, cx) {
                    return;
                }
                let Some(panel) = this.panel_drag else {
                    return;
                };
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.panel_drag = None;
                } else {
                    let w = f32::from(window.viewport_size().width);
                    this.set_panel_width(panel, f32::from(ev.position.x), w);
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &gpui::MouseUpEvent, _, cx| {
                    if this.end_view_drag(ev.position) {
                        cx.notify();
                    }
                    if this.selection.as_ref().is_some_and(|s| s.dragging) {
                        this.end_selection();
                        cx.notify();
                    }
                    if this.release_seam() {
                        cx.notify();
                        return;
                    }
                    if this.panel_drag.is_some() || this.split_drag.is_some() {
                        this.panel_drag = None;
                        this.split_drag = None;
                        cx.notify();
                    }
                }),
            )
            .child(self.top_bar(&t, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    // one left panel, neovim-style: the git panel replaces the agents
                    // sidebar while it's open; closing it brings the sidebar back
                    .map(|d| {
                        if zoomed {
                            d
                        } else if self.diff_open {
                            d.child(self.diff_panel(&t, cx))
                        } else if self.sidebar_open {
                            d.child(self.sidebar(&t, cx))
                        } else {
                            d
                        }
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .relative()
                            // with cards the canvas paints the chrome itself, only
                            // between the cards (see render::paint_gaps)
                            .map(|d| match (cards, tab.is_some()) {
                                (false, _) => d.bg(t.term_bg),
                                (true, false) => d.bg(t.chrome),
                                (true, true) => d,
                            })
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                            .on_scroll_wheel(cx.listener(Self::on_scroll))
                            .on_drop(cx.listener(Self::on_file_drop))
                            .child(self.terminal_canvas(&palette, window))
                            .children(self.pane_views(&t, window, cx))
                            .children(self.run_cards(&t, cx))
                            .children(self.seam_view(&t, cx)),
                    ),
            )
            // toasts and modals sit over the whole window (sidebars included), so
            // they anchor to the window's corner / center rather than the terminal's
            .children(self.toast(&t, cx))
            .children(self.copy_search(&t))
            .children(self.modal(&t, cx))
            .children(self.git_help_modal(&t, cx))
            .children(self.shortcuts_modal(&t, cx))
            .children(self.view_shortcuts_modal(&t, cx))
    }
}
