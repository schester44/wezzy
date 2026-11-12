//! The sidebar: agents, servers and actions sections and their rows.

use super::widgets::{kbd, row_selection, section_header, status_indicator, term_family, with_alpha};
use super::{icon, spinner, Icon, Theme};
use crate::agents::{Agent, Status};
use crate::app::{strip_escapes, TermApp, UI_STATE};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, MouseDownEvent, SharedString};

impl TermApp {
    pub(super) fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
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
}

pub(super) fn agent_row(
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
pub(super) fn subagent_line(s: crate::hooks::Subagent, t: &Theme) -> impl IntoElement {
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
pub(super) fn name_input(
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

pub(super) fn server_row(
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

pub(super) fn run_row(r: crate::actions::Run, cursor: bool, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
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

/// A sidebar section label that folds its rows: click to toggle. The chevron
/// shows on hover, and stays while folded so the state is visible.
pub(super) fn sidebar_section_header(
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
