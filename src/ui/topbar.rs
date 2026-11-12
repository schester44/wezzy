//! The top bar: traffic-light room, workspace and window switchers, tabs.

use super::view::{CHROME_H, TRAFFIC_LIGHTS_W};
use super::widgets::{kbd, menu_row, pill, small_icon_button, term_family, with_alpha};
use super::{icon, Icon, Theme};
use crate::agents::{self, Status};
use crate::app::TermApp;
use config::keyassignment::{KeyAssignment, SpawnTabDomain};
use gpui::{
    deferred, div, prelude::*, px, AnyElement, ClickEvent, Context, Hsla, MouseButton, MouseDownEvent,
};
use mux::Mux;
use std::time::Duration;

pub(super) struct TabInfo {
    pub(super) idx: usize,
    pub(super) title: String,
    pub(super) active: bool,
    pub(super) zoomed: bool,
    /// activity (green) or waiting-on-you (amber)
    pub(super) dot: Option<Hsla>,
    /// what the tab holds, when it's a single native view (database for SQL)
    pub(super) icon: Option<Icon>,
}

impl TermApp {
    pub(super) fn workspace_switcher(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn top_bar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mux = Mux::get();
        let all_agents = agents::collect(None);
        let (blocked, finished) = crate::queue::counts(&all_agents);
        let statuses: std::collections::HashMap<_, _> =
            all_agents.into_iter().map(|a| (a.pane_id, a.status)).collect();
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
                        let icon = match panes.as_slice() {
                            [only] => match self.views.kind(only.pane.pane_id()) {
                                Some("sql") => Some(Icon::Database),
                                _ => None,
                            },
                            _ => None,
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
                            icon,
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
            .children(attention_chip(blocked, finished, t, cx))
    }

    /// One numbered chip per window in the workspace; absent with a single
    /// window. A dot marks a window with an agent waiting on you.
    pub(super) fn window_switcher(
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

    pub(super) fn tab(&self, tab: TabInfo, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
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
            .when_some(tab.icon, |d, ic| {
                d.child(
                    div()
                        .flex_none()
                        .child(icon(ic, 12., if tab.active { t.fg_muted } else { t.fg_subtle })),
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
}

/// What's waiting in Agents (⌘J) at a glance: agents blocked on you (warning icon) and
/// agents that finished while you were elsewhere (check), each with its
/// count. Absent when nothing is waiting; click opens Agents.
fn attention_chip(blocked: usize, finished: usize, t: &Theme, cx: &mut Context<TermApp>) -> Option<impl IntoElement> {
    if blocked + finished == 0 {
        return None;
    }
    let part = |ic: Icon, color, n: usize| {
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(icon(ic, 12., color))
            .child(
                div()
                    .font_family(term_family())
                    .text_size(px(11.))
                    .text_color(t.fg_muted)
                    .child(n.to_string()),
            )
    };
    Some(
        div()
            .id("attention-chip")
            .flex_none()
            .h(px(22.))
            .px_2()
            .flex()
            .items_center()
            .gap_2p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .when(blocked > 0, |d| d.child(part(Icon::Alert, t.warning, blocked)))
            .when(finished > 0, |d| d.child(part(Icon::Check, t.success, finished)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if matches!(this.overlay, Some(crate::app::Overlay::Queue(_))) {
                    this.overlay = None;
                    cx.notify();
                } else {
                    this.open_queue(cx);
                }
            })),
    )
}
