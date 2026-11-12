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

use super::{icon, spinner, Icon, Theme};
use crate::agents::{self, Agent, Status};
use crate::app::{strip_escapes, GridGeom, Overlay, TermApp, UI_STATE};
use crate::render::{self, PaintOpts};
use config::keyassignment::{KeyAssignment, SpawnTabDomain};
use config::DimensionContext;
use gpui::{
    canvas, deferred, div, point, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight,
    Hsla, MouseButton, MouseDownEvent, Pixels, SharedString, Window,
};
use mux::Mux;
use std::time::Duration;
use wezterm_term::TerminalSize;

const CHROME_H: f32 = 40.;
/// Room for the macOS traffic lights when the sidebar is hidden.
const TRAFFIC_LIGHTS_W: f32 = 88.;
const UI_FONT: &str = ".SystemUIFont";

struct TabInfo {
    idx: usize,
    title: String,
    active: bool,
    zoomed: bool,
    /// activity (green) or waiting-on-you (amber)
    dot: Option<Hsla>,
}

impl Render for TermApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let config = config::configuration();
        self.views.sync();
        let tab = self.active_tab();
        let pane = tab.as_ref().and_then(|t| t.get_active_pane());
        // a zoomed pane (⌘↵) takes the whole window: side panels hide while it lasts
        let zoomed = self.pane_zoomed();
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
                            .bg(t.term_bg)
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                            .on_scroll_wheel(cx.listener(Self::on_scroll))
                            .on_drop(cx.listener(Self::on_file_drop))
                            .child(self.terminal_canvas(&palette, window))
                            .children(self.pane_views(&t, window, cx))
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

impl TermApp {
    // -- sidebar -------------------------------------------------------------

    fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::sidebar::{Item, Source};
        let sections = self.sidebar_sections();
        let sidebar_cur = (self.region == crate::regions::Region::Sidebar)
            .then_some(self.sidebar_cursor);
        let lua_status = strip_escapes(&UI_STATE.lock().left_status);
        // ⌥⌘ held: number the first nine agents for ⌥⌘1–9
        let numbered = self.agent_keys_held;
        let term_font = term_family();

        // rows are numbered across sections, matching the keyboard cursor
        let mut row = 0;
        let mut panel = self.side_panel(super::panel::SidePanel::Sidebar, t, cx).pt_1();
        for section in sections {
            let source = section.source;
            panel = panel.child(sidebar_section_header(source, section.items.len(), section.collapsed, t, cx));
            if section.collapsed {
                continue;
            }
            let rows: Vec<AnyElement> = section
                .items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    let cursor = sidebar_cur == Some(row);
                    row += 1;
                    match item {
                        Item::Agent(a) => {
                            let num = (numbered && i < 9).then(|| i + 1);
                            let editing = self
                                .agent_rename
                                .as_ref()
                                .filter(|r| r.pane_id == a.pane_id)
                                .map(|r| r.input.clone());
                            agent_row(a, cursor, num, editing, t, cx).into_any_element()
                        }
                        Item::Server(s) => server_row(s, cursor, t, cx).into_any_element(),
                        Item::Run(r) => run_row(r, cursor, t, cx).into_any_element(),
                    }
                })
                .collect();
            panel = panel.child(
                div()
                    .id(source.id())
                    .flex()
                    .flex_col()
                    // agents can run long; the sections below stay put
                    .when(source == Source::Agents, |d| d.overflow_y_scroll())
                    .children(rows),
            );
        }

        panel
            .child(div().flex_1())
            .when(!lua_status.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(t.border)
                        .font_family(term_font)
                        .text_xs()
                        .text_color(t.fg_subtle)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(lua_status),
                )
            })
    }

    fn workspace_switcher(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mux = Mux::get();
        let active = mux.active_workspace();
        let open = self.ws_menu_open;

        let button = div()
            .id("ws-button")
            .group("ws-button")
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .cursor_pointer()
            .when(open, |d| d.bg(t.selected))
            .hover(|s| s.bg(t.hover))
            .child(icon(Icon::Home, 13., t.fg_muted))
            .child(
                div()
                    .max_w(px(180.))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(active.clone()),
            )
            .child(
                div()
                    .opacity(if open { 1. } else { 0. })
                    .group_hover("ws-button", |s| s.opacity(1.))
                    .child(icon(Icon::ChevronDown, 10., t.fg_subtle)),
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.ws_menu_open = !this.ws_menu_open;
                this.ws_new = None;
                cx.notify();
            }));

        let all_ws = crate::workspaces::list();
        let n_ws = all_ws.len();
        let menu = open.then(|| {
            let rows = all_ws.into_iter().enumerate().map(|(i, ws)| {
                let is_active = ws.name == active;
                let running = ws.tabs.is_some();
                let detail = ws.detail();
                let name = ws.name.clone();
                menu_row(("ws", i), t)
                    .when(self.ws_cursor == i, |d| d.bg(with_alpha(t.fg, 0.07)))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .when(is_active, |d| d.child(icon(Icon::Check, 14., t.fg))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .when(!running, |d| d.text_color(t.fg_muted))
                            .child(ws.name),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(120.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_xs()
                            .text_color(t.fg_subtle)
                            .child(detail),
                    )
                    // mouse-down, not click: the container's mouse-down-out handler
                    // would otherwise close the menu before a click completes
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.ws_menu_open = false;
                            this.ws_new = None;
                            this.switch_workspace(Some(name.clone()), None);
                            this.focus.focus(window);
                            cx.notify();
                        }),
                    )
            });
            let new_row: AnyElement = match &self.ws_new {
                Some(input) => div()
                    .mx_0p5()
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .bg(gpui::hsla(0., 0., 0., 0.22))
                    .border_1()
                    .border_color(with_alpha(t.accent, 0.6))
                    .child(icon(Icon::Plus, 14., t.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .when(input.is_empty(), |d| {
                                d.child(div().w(px(1.5)).h(px(15.)).bg(t.accent)).child(
                                    div().ml_0p5().text_color(t.fg_subtle).child("Name"),
                                )
                            })
                            .when(!input.is_empty(), |d| {
                                d.child(div().whitespace_nowrap().child(input.clone()))
                                    .child(div().ml_px().w(px(1.5)).h(px(15.)).bg(t.accent))
                            }),
                    )
                    .into_any_element(),
                None => menu_row("ws-new", t)
                    .when(self.ws_cursor == n_ws, |d| d.bg(with_alpha(t.fg, 0.07)))
                    .text_color(t.fg_muted)
                    .child(icon(Icon::Plus, 14., t.fg_muted))
                    .child(div().flex_1().child("New workspace"))
                    .when_some(
                        self.keys.shortcut_for(&crate::commands::AppCommand::NewWorkspace.action()),
                        |d, sc| d.child(kbd(&sc, t)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            // the container's mouse-down-out may have closed it this same event
                            this.ws_menu_open = true;
                            this.ws_new = Some(String::new());
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            };
            deferred(
                div()
                    .absolute()
                    .top(px(28.))
                    .left_0()
                    .w(px(264.))
                    .p_1()
                    .rounded_lg()
                    .bg(t.elevated)
                    .border_1()
                    .border_color(t.border)
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(rows)
                    .child(div().my_1().h(px(1.)).bg(t.border))
                    .child(new_row)
                    ,
            )
            .with_priority(1)
        });

        div()
            .relative()
            .flex_none()
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                if this.ws_menu_open {
                    this.ws_menu_open = false;
                    this.ws_new = None;
                    cx.notify();
                }
            }))
            .child(button)
            .children(menu)
    }

    // -- top bar -------------------------------------------------------------

    fn top_bar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mux = Mux::get();
        let statuses: std::collections::HashMap<_, _> = agents::collect(None)
            .into_iter()
            .map(|a| (a.pane_id, a.status))
            .collect();
        let tabs: Vec<TabInfo> = self
            .mux_window_id
            .and_then(|id| mux.get_window(id))
            .map(|w| {
                let active = w.get_active_tab_idx();
                w.iter_tabs()
                    .enumerate()
                    .map(|(idx, tab)| {
                        let mut title = tab.get_title();
                        if title.is_empty() {
                            title = tab
                                .get_active_pane()
                                .map(|p| p.get_title())
                                .unwrap_or_default();
                        }
                        let panes = tab.iter_panes_ignoring_zoom();
                        let has = |st: Status| {
                            panes.iter().any(|p| statuses.get(&p.pane.pane_id()) == Some(&st))
                        };
                        let waiting = has(Status::NeedsInput);
                        let done = idx != active && has(Status::Done);
                        let busy = idx != active
                            && panes.iter().any(|p| {
                                crate::app::last_output(p.pane.pane_id())
                                    .is_some_and(|t| t.elapsed() < Duration::from_millis(1500))
                            });
                        TabInfo {
                            idx,
                            title,
                            active: idx == active,
                            zoomed: tab.get_zoomed_pane().is_some(),
                            dot: if waiting {
                                Some(t.warning)
                            } else if done {
                                Some(t.accent)
                            } else if busy {
                                Some(t.success)
                            } else {
                                None
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        let leader = self.keys.leader_active();
        let table = self.keys.active_table().map(crate::keys::table_label);

        div()
            .h(px(CHROME_H))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .pl(px(TRAFFIC_LIGHTS_W))
            .pr_3()
            .text_size(px(12.5))
            .bg(t.chrome)
            .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
                if ev.click_count == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(self.workspace_switcher(t, cx))
            .children(self.window_switcher(&statuses, t, cx))
            .child(div().flex_none().w(px(1.)).h(px(14.)).mx_1().bg(t.border))
            .children(tabs.into_iter().map(|tab| self.tab(tab, t, cx)))
            .child(
                small_icon_button("new-tab", Icon::Plus, t).on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.perform(
                            KeyAssignment::SpawnTab(SpawnTabDomain::CurrentPaneDomain),
                            None,
                            window,
                            cx,
                        );
                        this.focus.focus(window);
                    },
                )),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .gap_1()
                    .mr_1()
                    .when(leader, |d| d.child(pill("Leader", t.accent)))
                    .when_some(table, |d, name| d.child(pill(&name, t.warning))),
            )
    }

    /// One numbered chip per window in the workspace; absent with a single
    /// window. A dot marks a window with an agent waiting on you.
    fn window_switcher(
        &self,
        statuses: &std::collections::HashMap<mux::pane::PaneId, Status>,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mux = Mux::get();
        let wins = mux.iter_windows_in_workspace(&mux.active_workspace());
        if wins.len() < 2 {
            return None;
        }
        let chips = wins.into_iter().enumerate().map(|(i, id)| {
            let active = self.mux_window_id == Some(id);
            let waiting = mux.get_window(id).is_some_and(|w| {
                w.iter_tabs().any(|tab| {
                    tab.iter_panes_ignoring_zoom()
                        .iter()
                        .any(|p| statuses.get(&p.pane.pane_id()) == Some(&Status::NeedsInput))
                })
            });
            div()
                .id(("window", i))
                .h(px(20.))
                .min_w(px(20.))
                .px_1()
                .flex()
                .items_center()
                .justify_center()
                .gap_1()
                .rounded_md()
                .cursor_pointer()
                .font_family(term_family())
                .text_size(px(11.))
                .map(|d| {
                    if active {
                        d.bg(t.tab_active).text_color(t.fg)
                    } else {
                        d.text_color(t.fg_subtle)
                            .hover(|s| s.bg(t.hover).text_color(t.fg))
                    }
                })
                .child((i + 1).to_string())
                .when(waiting && !active, |d| {
                    d.child(div().size(px(5.)).flex_none().rounded_full().bg(t.warning))
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.perform(KeyAssignment::ActivateWindow(i), None, window, cx);
                    this.focus.focus(window);
                }))
        });
        Some(div().flex().items_center().gap_0p5().children(chips).into_any_element())
    }

    fn tab(&self, tab: TabInfo, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let idx = tab.idx;
        div()
            .id(("tab", idx))
            .h(px(24.))
            .max_w(px(220.))
            .flex_shrink()
            .min_w_0()
            .px_2p5()
            .flex()
            .items_center()
            .gap_1p5()
            .rounded_md()
            .cursor_pointer()
            .map(|d| {
                if tab.active {
                    d.bg(t.tab_active).text_color(t.fg)
                } else {
                    d.text_color(t.fg_muted)
                        .hover(|s| s.bg(t.hover).text_color(t.fg))
                }
            })
            // ⌘1…⌘9 index, so you know which shortcut reaches the tab
            .when(idx < 9, |d| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(term_family())
                        .text_size(px(11.))
                        .text_color(if tab.active { t.fg_muted } else { t.fg_subtle })
                        .child((idx + 1).to_string()),
                )
            })
            .when_some(tab.dot, |d, c| {
                d.child(div().size(px(5.)).flex_none().rounded_full().bg(c))
            })
            .child(
                div()
                    .min_w_0()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(if tab.title.is_empty() {
                        "shell".to_string()
                    } else {
                        tab.title
                    }),
            )
            .when(tab.zoomed, |d| d.child(pill("ZOOM", t.warning)))
            // close button: only on the hovered tab. gpui applies hover styles
            // after layout, so the hover has to live in state to take up space.
            .when(self.hovered_tab == Some(idx), |d| {
                d.child(
                    div()
                        .id(("tab-close", idx))
                        .flex_none()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .hover(|s| s.bg(t.selected))
                        .child(icon(Icon::X, 9., t.fg_muted))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.hovered_tab = None;
                            this.close_tab_at(idx, window, cx);
                            this.focus.focus(window);
                        })),
                )
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_tab = Some(idx);
                } else if this.hovered_tab == Some(idx) {
                    this.hovered_tab = None;
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.perform(KeyAssignment::ActivateTab(idx as isize), None, window, cx);
                this.focus.focus(window);
            }))
            .into_any_element()
    }

    // -- terminal ------------------------------------------------------------

    /// Native views, each placed over its placeholder pane's cells. Uses the
    /// grid geometry the canvas recorded last frame.
    fn pane_views(&mut self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        let g = self.grid.get();
        let area = point(g.origin.x - g.pad.x, g.origin.y - g.pad.y);
        let metrics = render::metrics(window, &config::configuration(), self.font_scale);
        let focused = self.focus.is_focused(window)
            && self.overlay.is_none()
            && self.region == crate::regions::Region::Terminal;
        tab.iter_panes()
            .into_iter()
            .filter_map(|p| {
                let pane_id = p.pane.pane_id();
                if !self.views.has(pane_id) {
                    return None;
                }
                let b = self.view_bounds(&tab, &p);
                let view = self.views.get_mut(pane_id)?;
                let size = b.size;
                let vcx = crate::views::ViewCx {
                    pane_id,
                    cols: p.width,
                    rows: p.height,
                    size,
                    focused: focused && p.is_active,
                    metrics: metrics.clone(),
                };
                Some(
                    div()
                        .absolute()
                        .left(b.origin.x - area.x)
                        .top(b.origin.y - area.y)
                        .w(size.width)
                        .h(size.height)
                        .overflow_hidden()
                        .child(view.render(&vcx, t, cx))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn terminal_canvas(
        &self,
        palette: &wezterm_term::color::ColorPalette,
        window: &mut Window,
    ) -> impl IntoElement {
        let config = config::configuration();
        let metrics = render::metrics(window, &config, self.font_scale);
        let viewports = self.viewports.clone();
        let grid = self.grid.clone();
        // dividers: a visible hairline at rest, accent while hovered / dragged
        let fg = render::color(palette.foreground);
        let split_idle = gpui::Hsla { a: 0.16, ..fg };
        let split_hot = render::color(palette.colors.0[4]);
        let split_drag = self.split_drag;
        let opts = PaintOpts {
            inactive_brightness: config.inactive_pane_hsb.brightness,
            default_cursor_bar: matches!(
                config.default_cursor_style,
                config::DefaultCursorStyle::BlinkingBar | config::DefaultCursorStyle::SteadyBar
            ),
            focused: self.focus.is_focused(window)
                && self.overlay.is_none()
                && self.region == crate::regions::Region::Terminal,
        };
        let pad = config.window_padding.clone();
        let scale = window.scale_factor();
        let tab = self.active_tab();
        let hints = self
            .quick_select
            .as_ref()
            .map(|q| (q.pane_id, q.matches.clone(), q.typed.clone()));
        let hint_color = render::color(palette.colors.0[5]);
        let pane_labels = self.pane_select.as_ref().map(|ps| {
            let labels: Vec<(mux::pane::PaneId, String)> = ps
                .labels
                .iter()
                .filter(|(_, l)| l.starts_with(&ps.typed))
                .map(|(id, l)| {
                    let mut l = l.to_uppercase();
                    if ps.show_ids {
                        if let Some(remote) = crate::session::remote_pane_id(*id) {
                            l = format!("{l} {remote}");
                        }
                    }
                    (*id, l)
                })
                .collect();
            labels
        });
        let copy = self
            .copy_mode
            .as_ref()
            .map(|c| (c.pane_id, Some(c.cursor), c.selection()))
            .or_else(|| {
                let s = self.selection.as_ref()?;
                Some((s.pane_id, None, Some(s.range()?)))
            });
        let copy_colors = (render::color(palette.selection_bg), render::color(palette.cursor_bg));

        let paint_metrics = metrics.clone();
        canvas(
            move |bounds, window, _| {
                let tab = tab?;
                let ctx = |max: Pixels, cell: Pixels| DimensionContext {
                    dpi: 72.0,
                    pixel_max: f32::from(max),
                    pixel_cell: f32::from(cell),
                };
                let (w, h) = (bounds.size.width, bounds.size.height);
                let pl = px(pad.left.evaluate_as_pixels(ctx(w, metrics.cell_w)));
                let pr = px(pad.right.evaluate_as_pixels(ctx(w, metrics.cell_w)));
                let pt = px(pad.top.evaluate_as_pixels(ctx(h, metrics.line_h)));
                let pb = px(pad.bottom.evaluate_as_pixels(ctx(h, metrics.line_h)));
                let origin = point(bounds.origin.x + pl, bounds.origin.y + pt);
                let cols = (f32::from(w - pl - pr) / f32::from(metrics.cell_w))
                    .floor()
                    .max(1.) as usize;
                let rows = (f32::from(h - pt - pb) / f32::from(metrics.line_h))
                    .floor()
                    .max(1.) as usize;

                let cur = tab.get_size();
                if cur.cols != cols || cur.rows != rows {
                    tab.resize(TerminalSize {
                        rows,
                        cols,
                        pixel_width: (cols as f32 * f32::from(metrics.cell_w) * scale) as usize,
                        pixel_height: (rows as f32 * f32::from(metrics.line_h) * scale) as usize,
                        dpi: (72.0 * scale) as u32,
                    });
                }
                grid.set(GridGeom {
                    origin,
                    pad: point(pl, pt),
                    size: bounds.size,
                    cell_w: metrics.cell_w,
                    line_h: metrics.line_h,
                });

                // each divider owns its one-cell strip between panes: resize cursor,
                // hover highlight, and the drag target for on_mouse_down
                let splits: Vec<_> = tab
                    .iter_splits()
                    .into_iter()
                    .map(|s| {
                        let b = render::split_strip(origin, &s, &metrics);
                        (s, window.insert_hitbox(b, gpui::HitboxBehavior::Normal))
                    })
                    .collect();
                Some((tab, origin, splits))
            },
            move |_, state, window, cx| {
                let Some((tab, origin, splits)) = state else { return };
                let panes: Vec<_> = tab
                    .iter_panes()
                    .iter()
                    .filter(|p| !crate::views::is_view(&p.pane))
                    .map(|p| render::snapshot(p, viewports.get(&p.pane.pane_id()).copied()))
                    .collect();
                let painted: Vec<_> = splits
                    .iter()
                    .map(|(s, hitbox)| {
                        window.set_cursor_style(
                            match s.direction {
                                mux::tab::SplitDirection::Horizontal => gpui::CursorStyle::ResizeLeftRight,
                                mux::tab::SplitDirection::Vertical => gpui::CursorStyle::ResizeUpDown,
                            },
                            hitbox,
                        );
                        let hot = split_drag == Some(s.index)
                            || (split_drag.is_none() && hitbox.is_hovered(window));
                        (*s, if hot { split_hot } else { split_idle }, hot)
                    })
                    .collect();
                render::paint_tab(origin, &panes, &painted, &paint_metrics, &opts, window, cx);
                if let Some((pane_id, matches, typed)) = &hints {
                    if let Some(p) = tab.iter_panes().iter().find(|p| p.pane.pane_id() == *pane_id) {
                        let o = point(
                            origin.x + paint_metrics.cell_w * p.left as f32,
                            origin.y + paint_metrics.line_h * p.top as f32,
                        );
                        paint_hints(o, matches, typed, hint_color, &paint_metrics, window, cx);
                    }
                }
                if let Some(labels) = &pane_labels {
                    for p in tab.iter_panes() {
                        if let Some((_, label)) = labels.iter().find(|(id, _)| *id == p.pane.pane_id()) {
                            let rect = gpui::Bounds::new(
                                point(
                                    origin.x + paint_metrics.cell_w * p.left as f32,
                                    origin.y + paint_metrics.line_h * p.top as f32,
                                ),
                                gpui::size(
                                    paint_metrics.cell_w * p.width as f32,
                                    paint_metrics.line_h * p.height as f32,
                                ),
                            );
                            paint_pane_label(rect, label, hint_color, &paint_metrics, window, cx);
                        }
                    }
                }
                if let Some((pane_id, cursor, sel)) = copy {
                    if let Some(p) = tab.iter_panes().iter().find(|p| p.pane.pane_id() == pane_id) {
                        let top = viewports
                            .get(&pane_id)
                            .copied()
                            .unwrap_or(p.pane.get_dimensions().physical_top);
                        let o = point(
                            origin.x + paint_metrics.cell_w * p.left as f32,
                            origin.y + paint_metrics.line_h * p.top as f32,
                        );
                        paint_copy_mode(
                            o,
                            (p.width, p.height),
                            top,
                            cursor,
                            sel,
                            copy_colors,
                            &paint_metrics,
                            window,
                        );
                    }
                }
            },
        )
        .size_full()
    }

    // -- floating ------------------------------------------------------------

    /// Copy mode's search field, while typing after / or ?.
    fn copy_search(&self, t: &Theme) -> Option<impl IntoElement> {
        let (query, forward) = self.copy_mode.as_ref()?.search.as_ref()?;
        Some(
            div()
                .absolute()
                .bottom_4()
                .left_4()
                .min_w(px(240.))
                .h(px(32.))
                .px_3()
                .flex()
                .items_center()
                .rounded_lg()
                .bg(t.elevated)
                .border_1()
                .border_color(with_alpha(t.accent, 0.6))
                .shadow_lg()
                .font_family(term_family())
                .child(div().text_color(t.fg_subtle).mr_1().child(if *forward { "/" } else { "?" }))
                .child(query.clone())
                .child(div().ml_px().w(px(1.5)).h(px(15.)).bg(t.accent)),
        )
    }

    fn toast(&self, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (msg, _) = self.flash.as_ref()?;
        let target = self.flash_pane;
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
                .items_center()
                .gap_2()
                .rounded_lg()
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .child(div().size(px(6.)).flex_none().rounded_full().bg(t.accent))
                .child(msg.clone())
                .when_some(target, |d, pane_id| {
                    d.cursor_pointer().on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.flash = None;
                        this.flash_pane = None;
                        this.region = crate::regions::Region::Terminal;
                        this.focus_pane(pane_id, window, cx);
                    }))
                }),
        )
    }

    fn modal(&self, t: &Theme, cx: &mut Context<Self>) -> Option<impl IntoElement> {
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
                ..
            } => (
                96.,
                card(380.)
                    .p_4()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(message.clone()))
                    .when(!detail.is_empty(), |d| {
                        d.child(div().text_color(t.fg_muted).child(detail.clone()))
                    })
                    .child(
                        div()
                            .mt_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1())
                            .child(button("confirm-no", "Cancel", t, false).on_click(
                                cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.answer_confirm(false, window, cx)
                                }),
                            ))
                            .child(button("confirm-yes", verb, t, true).on_click(
                                cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.answer_confirm(true, window, cx)
                                }),
                            )),
                    )
                    .into_any_element(),
            ),
            Overlay::Palette(state) => (
                56.,
                card(620.).child(self.palette_view(state, t, cx)).into_any_element(),
            ),
            Overlay::Settings(state) => (
                72.,
                card(460.).child(self.settings_view(state, t, cx)).into_any_element(),
            ),
            Overlay::Shortcuts | Overlay::ViewShortcuts(_) => return None,
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

    fn settings_view(
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

    fn palette_view(
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

    pub(crate) fn answer_confirm(&mut self, yes: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Overlay::Confirm {
            action,
            cancel,
            pane_id,
            ..
        }) = self.overlay.take()
        {
            if yes && matches!(action, KeyAssignment::QuitApplication) {
                cx.quit();
            } else if yes {
                self.perform(action, Some(pane_id), window, cx);
            } else if let Some(c) = cancel {
                self.perform(c, Some(pane_id), window, cx);
            }
        }
        self.focus.focus(window);
        cx.notify();
    }
}

// ---------------------------------------------------------------------------
// small components

fn agent_row(
    a: Agent,
    cursor: bool,
    num: Option<usize>,
    // the inline name field's text, while renaming this agent
    editing: Option<String>,
    t: &Theme,
    cx: &mut Context<TermApp>,
) -> impl IntoElement {
    let pane_id = a.pane_id;
    // the task (agents put a summary in the pane title)...
    let task = (!a.title.is_empty()
        && !a.title.eq_ignore_ascii_case(a.kind)
        && a.title != "Claude Code")
        .then(|| a.title.clone());
    // ...under your name for it, the kind moves down beside the task
    let task = match (&a.name, task) {
        (Some(_), Some(task)) => Some(format!("{} · {task}", a.kind)),
        (Some(_), None) => Some(a.kind.to_string()),
        (None, task) => task,
    };
    let label = a.display_name().to_string();
    // ...and what it's doing / needs right now
    let (status_text, status_color) = match a.status {
        Status::Working => (a.detail.clone(), t.fg_muted),
        Status::NeedsInput => (a.detail.clone(), t.warning),
        Status::Done | Status::Idle => (None, t.fg_subtle),
    };
    // agents from every workspace are listed, so say which one each lives in
    let meta = if a.workspace.is_empty() {
        a.project.clone()
    } else {
        a.workspace.clone()
    };
    div()
        .id(("agent", pane_id))
        .relative()
        .pl(px(14.))
        .pr_4()
        .py(px(7.))
        .flex()
        .gap_2p5()
        .cursor_pointer()
        .map(|d| row_selection(d, cursor, a.focused, t))
        .child(
            div()
                .pt(px(2.))
                .w(px(14.))
                .flex_none()
                .flex()
                .justify_center()
                .child(status_indicator(a.status, a.unseen, t)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(match editing {
                            Some(input) => name_input(input, a.kind, t, cx).into_any_element(),
                            None => div()
                                .flex_shrink()
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(label)
                                .into_any_element(),
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(t.fg_subtle)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(meta),
                        ),
                )
                .when_some(task, |d, task| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(t.fg_muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(task),
                    )
                })
                .when_some(status_text, |d, text| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(status_color)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(text),
                    )
                })
                .when(!a.subagents.is_empty(), |d| {
                    d.child(
                        div()
                            .pt_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .children(a.subagents.into_iter().map(|s| subagent_line(s, t))),
                    )
                }),
        )
        .when_some(num, |d, n| d.child(div().pt(px(1.)).child(kbd(&n.to_string(), t))))
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.focus_pane(pane_id, window, cx)
        }))
}

/// One of an agent's running subagents, nested in its sidebar row: status,
/// what it was asked to do, and what it's doing now.
fn subagent_line(s: crate::hooks::Subagent, t: &Theme) -> impl IntoElement {
    use crate::hooks::HookStatus;
    let needs_you = s.status == HookStatus::NeedsInput;
    let indicator = if needs_you {
        icon(Icon::Alert, 11., t.warning).into_any_element()
    } else {
        spinner(10., t.fg_subtle).into_any_element()
    };
    let label = s.description.unwrap_or(s.kind);
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(div().w(px(12.)).flex_none().flex().justify_center().child(indicator))
        .child(
            div()
                .flex_none()
                .max_w(px(140.))
                .overflow_hidden()
                .text_ellipsis()
                .text_color(t.fg_muted)
                .child(label),
        )
        .when_some(s.detail, |d, detail| {
            d.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(if needs_you { t.warning } else { t.fg_subtle })
                    .child(detail),
            )
        })
}

/// The inline name field on an agent row (Rename agent): ↵ saves, esc or a
/// click elsewhere cancels.
fn name_input(
    input: String,
    fallback: &'static str,
    t: &Theme,
    cx: &mut Context<TermApp>,
) -> impl IntoElement {
    // the name itself becomes editable: same type, same place, so the row
    // doesn't move. Empty shows what it falls back to.
    let caret = div().w(px(1.5)).h(px(14.)).flex_none().bg(t.accent);
    let empty = input.is_empty();
    div()
        .id("agent-name")
        .flex_shrink()
        .min_w_0()
        .flex()
        .items_center()
        .overflow_hidden()
        .border_b_1()
        .border_color(with_alpha(t.accent, 0.6))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .when(!empty, |d| d.child(input))
        .child(caret)
        .when(empty, |d| d.child(div().text_color(t.fg_subtle).child(fallback)))
        .on_click(|_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
            this.finish_agent_rename(false);
            cx.notify();
        }))
}

/// QuickSelect overlay: tint each match, label at its start.
/// Copy mode / mouse selection: the selection, and copy mode's cursor, over one pane.
#[allow(clippy::too_many_arguments)]
fn paint_copy_mode(
    origin: gpui::Point<Pixels>,
    (width, height): (usize, usize),
    top: wezterm_term::StableRowIndex,
    cursor: Option<crate::copymode::Pos>,
    sel: Option<(crate::copymode::Pos, crate::copymode::Pos)>,
    (sel_color, cursor_color): (Hsla, Hsla),
    m: &render::Metrics,
    window: &mut Window,
) {
    use gpui::{fill, size, Bounds};
    let cell = |row: wezterm_term::StableRowIndex, col: usize, cells: usize| {
        Bounds::new(
            point(origin.x + m.cell_w * col as f32, origin.y + m.line_h * (row - top) as f32),
            size(m.cell_w * cells as f32, m.line_h),
        )
    };
    let bottom = top + height as wezterm_term::StableRowIndex - 1;
    if let Some((s, e)) = sel {
        for row in s.0.max(top)..=e.0.min(bottom) {
            let from = if row == s.0 { s.1 } else { 0 };
            let to = if row == e.0 { e.1.min(width - 1) } else { width - 1 };
            if from <= to {
                window.paint_quad(fill(cell(row, from, to - from + 1), with_alpha(sel_color, 0.45)));
            }
        }
    }
    if let Some(cursor) = cursor.filter(|c| (top..=bottom).contains(&c.0)) {
        window.paint_quad(fill(cell(cursor.0, cursor.1.min(width - 1), 1), with_alpha(cursor_color, 0.6)));
    }
}

/// PaneSelect: a large label in the middle of a pane.
fn paint_pane_label(
    rect: gpui::Bounds<Pixels>,
    label: &str,
    accent: Hsla,
    m: &render::Metrics,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    use gpui::{fill, size, Bounds, TextRun};
    let font_size = m.font_size * 2.;
    let mut font = m.font.clone();
    font.weight = FontWeight::BOLD;
    let text: SharedString = label.to_string().into();
    let run = TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(text, font_size, &[run], None);
    let line_h = m.line_h * 2.;
    let badge = size(line.width + px(20.), line_h + px(8.));
    let at = point(
        rect.origin.x + (rect.size.width - badge.width) / 2.,
        rect.origin.y + (rect.size.height - badge.height) / 2.,
    );
    window.paint_quad(fill(Bounds::new(at, badge), accent).corner_radii(px(6.)));
    line.paint(point(at.x + px(10.), at.y + px(4.)), line_h, window, cx).ok();
}

fn paint_hints(
    origin: gpui::Point<Pixels>,
    matches: &[crate::quickselect::Match],
    typed: &str,
    accent: Hsla,
    m: &render::Metrics,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    use gpui::{fill, size, Bounds, TextRun};
    for hit in matches.iter().filter(|h| h.label.starts_with(typed)) {
        let x = origin.x + m.cell_w * hit.col as f32;
        let y = origin.y + m.line_h * hit.row as f32;
        window.paint_quad(fill(
            Bounds::new(point(x, y), size(m.cell_w * hit.cells as f32, m.line_h)),
            with_alpha(accent, 0.22),
        ));
        let label: SharedString = hit.label.to_uppercase().into();
        let w = m.cell_w * label.len() as f32 + px(6.);
        window.paint_quad(
            fill(Bounds::new(point(x - px(1.), y + px(1.)), size(w, m.line_h - px(2.))), accent)
                .corner_radii(px(3.)),
        );
        let mut font = m.font.clone();
        font.weight = FontWeight::BOLD;
        let run = TextRun {
            len: label.len(),
            font,
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        window
            .text_system()
            .shape_line(label, m.font_size, &[run], None)
            .paint(point(x + px(2.), y), m.line_h, window, cx)
            .ok();
    }
}

fn server_row(
    s: crate::servers::Server,
    cursor: bool,
    t: &Theme,
    cx: &mut Context<TermApp>,
) -> impl IntoElement {
    let url = format!("http://localhost:{}", s.port);
    let pane_id = s.pane_id;
    let group: SharedString = format!("server-{}", s.port).into();
    div()
        .id(("server", s.port as usize))
        .group(group.clone())
        .relative()
        .h(px(30.))
        .pl(px(14.))
        .pr_4()
        .flex()
        .items_center()
        .gap_2p5()
        .cursor_pointer()
        .map(|d| row_selection(d, cursor, false, t))
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .flex()
                .justify_center()
                .child(div().size(px(6.)).rounded_full().bg(t.success)),
        )
        .child(div().child(format!("localhost:{}", s.port)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(t.fg_subtle)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(if s.folder.is_empty() {
                    s.command.clone()
                } else {
                    s.folder.clone()
                }),
        )
        .child(
            div()
                .id(("server-pane", s.port as usize))
                .size(px(20.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .opacity(0.)
                .group_hover(group, |st| st.opacity(1.))
                .hover(|st| st.bg(t.selected))
                .child(icon(Icon::External, 12., t.fg_muted))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&url);
                })),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.focus_pane(pane_id, window, cx)
        }))
}

fn run_row(r: crate::actions::Run, cursor: bool, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    use crate::actions::State;
    let group: SharedString = format!("run-{}", r.id).into();
    let url = r.url.clone();
    let dot = |c| div().size(px(6.)).rounded_full().bg(c).into_any_element();
    let indicator = match r.state {
        State::Running => spinner(12., t.fg_muted).into_any_element(),
        State::Queued | State::Other => dot(t.fg_subtle),
        State::Waiting => icon(Icon::Alert, 13., t.warning).into_any_element(),
        State::Success => dot(t.success),
        State::Failure => icon(Icon::X, 12., t.danger).into_any_element(),
    };
    let age = crate::actions::age(r.created);
    div()
        .id(("run", r.id as usize))
        .group(group.clone())
        .relative()
        .h(px(30.))
        .pl(px(14.))
        .pr_4()
        .flex()
        .items_center()
        .gap_2p5()
        .cursor_pointer()
        .map(|d| row_selection(d, cursor, false, t))
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .flex()
                .justify_center()
                .child(indicator),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(r.workflow.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(t.fg_subtle)
                .child(age),
        )
        .child(
            div()
                .id(("run-web", r.id as usize))
                .size(px(20.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .opacity(0.)
                .group_hover(group, |st| st.opacity(1.))
                .hover(|st| st.bg(t.selected))
                .child(icon(Icon::External, 12., t.fg_muted))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.open_url(&url);
                })),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_run(&r, window, cx)))
}

/// `unseen`: it finished while you were elsewhere and you haven't been back.
/// Done stays bright until you go to it, then fades to the idle dot.
fn status_indicator(status: Status, unseen: bool, t: &Theme) -> AnyElement {
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

fn section_header(title: &str, badge: Option<String>, t: &Theme) -> gpui::Div {
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

/// A sidebar section label that folds its rows: click to toggle. The chevron
/// shows on hover, and stays while folded so the state is visible.
fn sidebar_section_header(
    source: crate::sidebar::Source,
    count: usize,
    collapsed: bool,
    t: &Theme,
    cx: &mut Context<TermApp>,
) -> impl IntoElement {
    let group: SharedString = format!("section-{}", source.id()).into();
    div()
        .id(SharedString::from(format!("section-{}", source.id())))
        .group(group.clone())
        .cursor_pointer()
        .child(
            section_header(source.title(), Some(count.to_string()), t).child(
                div()
                    .flex_none()
                    .when(!collapsed, |d| d.opacity(0.).group_hover(group, |st| st.opacity(1.)))
                    .child(icon(
                        if collapsed { Icon::ChevronRight } else { Icon::ChevronDown },
                        11.,
                        t.fg_subtle,
                    )),
            ),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_sidebar_section(source, cx)))
}

/// Sidebar row selection (docs/development/design.md): faint full-width fill + 2px left bar.
/// `cursor` is the keyboard cursor while the sidebar has focus (accent bar);
/// `current` marks the pane you're in (quieter fill, no bar).
fn row_selection(d: gpui::Stateful<gpui::Div>, cursor: bool, current: bool, t: &Theme) -> gpui::Stateful<gpui::Div> {
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

fn small_icon_button(id: &'static str, ic: Icon, t: &Theme) -> gpui::Stateful<gpui::Div> {
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

fn menu_row(id: impl Into<gpui::ElementId>, t: &Theme) -> gpui::Stateful<gpui::Div> {
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

fn button(id: &'static str, label: &'static str, t: &Theme, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(30.))
        .px_3()
        .flex()
        .items_center()
        .rounded_lg()
        .cursor_pointer()
        .font_weight(FontWeight::MEDIUM)
        .map(|d| {
            if primary {
                d.bg(t.accent)
                    .text_color(gpui::black())
                    .hover(|s| s.opacity(0.9))
            } else {
                d.border_1().border_color(t.border).hover(|s| s.bg(t.hover))
            }
        })
        .child(label)
}

fn pill(label: &str, color: Hsla) -> impl IntoElement {
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
