//! The agents overlay's card (⌘J, `queue.rs`): what's waiting, then each
//! agent's runs.

use super::widgets::{button, section_header, term_family, with_alpha, ButtonKind};
use super::{icon, spinner, Icon, Theme};
use crate::app::TermApp;
use crate::queue::{Ask, Entry, Item, Key, QueueState, RunRow};
use gpui::{div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, SharedString};

/// Lines of a request shown on the selected row before it's cut off.
const DETAIL_LINES: usize = 12;

impl TermApp {
    pub(super) fn queue_view(&self, state: &QueueState, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = crate::queue::entries();
        let selected = crate::queue::selected_index(state, &entries);
        let confirm = state.confirm.and_then(|p| {
            entries.iter().find_map(|e| match e {
                Entry::Wait(it) if it.pane_id == p => Some(it.clone()),
                _ => None,
            })
        });
        let empty_list = entries.is_empty();
        let mut wait_n = 0;
        let rows: Vec<AnyElement> = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| {
                let sel = selected == Some(i);
                match e {
                    Entry::WaitHeader(n) => section_header("Waiting", Some(n.to_string()), t).into_any_element(),
                    Entry::Wait(it) => {
                        wait_n += 1;
                        self.queue_row(wait_n - 1, it, sel, t, cx)
                    }
                    Entry::Agent { name, project } => agent_header(&name, &project, t).into_any_element(),
                    Entry::Run(r) => self.run_row(r, sel, t, cx),
                }
            })
            .collect();

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
                    .child(div().font_weight(FontWeight::MEDIUM).child("Agents")),
            )
            .when(empty_list, |d| d.child(empty(t)))
            .when(!empty_list, |d| {
                d.child(
                    div()
                        .id("queue-list")
                        .max_h(px(560.))
                        .overflow_y_scroll()
                        .track_scroll(&state.scroll)
                        .pb_1p5()
                        .children(rows),
                )
            })
            .when_some(confirm, |d, it| d.child(confirm_footer(&it, t, cx)))
    }

    /// One of an agent's runs: its summary, what it changed, how long ago.
    fn run_row(&self, r: RunRow, sel: bool, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let key = Key::Run(r.pane_id, r.started);
        let pane_id = r.pane_id;
        let working = r.working;
        div()
            .id(("queue-run", (r.pane_id as u64) << 40 ^ r.started))
            .relative()
            .w_full()
            .px_4()
            .py_2()
            .flex()
            .gap_3()
            .cursor_pointer()
            .map(|d| {
                if sel {
                    d.bg(with_alpha(t.fg, 0.07))
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
                } else {
                    d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
                }
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                let Some(crate::app::Overlay::Queue(s)) = this.overlay.as_mut() else {
                    return;
                };
                if s.selected == Some(key) || ev.click_count() > 1 {
                    this.open_queue_item(pane_id, window, cx);
                } else {
                    s.selected = Some(key);
                    s.confirm = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size(px(16.))
                    .mt(px(2.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(working, |d| d.child(spinner(12., t.fg_muted))),
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
                            .text_color(t.fg)
                            .when(!sel, |d| d.whitespace_nowrap().overflow_hidden().text_ellipsis())
                            .child(r.title.clone()),
                    )
                    .when_some(r.prompt.clone().filter(|_| sel), |d, p| {
                        d.child(div().text_size(px(12.)).text_color(t.fg_muted).child(p))
                    })
                    .when_some(r.stats.filter(|s| s.2 > 0), |d, (a, rm, n)| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_size(px(12.))
                                .text_color(t.fg_subtle)
                                .child(super::git::counts(a, rm, t))
                                .child(crate::runs::files(n)),
                        )
                    })
                    .when(sel, |d| d.child(self.run_actions(&r, t, cx))),
            )
            .children(r.ended.map(|e| {
                div()
                    .flex_none()
                    .mt(px(2.))
                    .font_family(term_family())
                    .text_size(px(11.))
                    .text_color(t.fg_subtle)
                    .child(crate::queue::ago(e))
            }))
            .into_any_element()
    }

    fn run_actions(&self, r: &RunRow, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_id = r.pane_id;
        div()
            .pt_1p5()
            .flex()
            .items_center()
            .gap_2()
            .when(r.reviewable, |d| {
                d.child(button("queue-review", "Review changes", "v", t, ButtonKind::Soft).on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.overlay = None;
                        this.review_run(pane_id, window, cx);
                    },
                )))
            })
            .child(button("queue-run-open", "Open", "↩", t, ButtonKind::Ghost).on_click(cx.listener(
                move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_queue_item(pane_id, window, cx);
                },
            )))
    }

    fn queue_row(&self, i: usize, it: Item, sel: bool, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (ic, color) = match it.ask {
            Ask::Approve(_) => (Icon::Shield, t.warning),
            Ask::Question(_) => (Icon::Message, t.warning),
            Ask::Done => (Icon::Check, t.success),
        };
        let pane_id = it.pane_id;
        let detail = it.detail.clone().filter(|_| sel);
        div()
            .id(("queue-row", pane_id as u64))
            .relative()
            .w_full()
            .pl_4()
            .pr_4()
            .py_2()
            .flex()
            .gap_3()
            .cursor_pointer()
            .map(|d| {
                if sel {
                    d.bg(with_alpha(t.fg, 0.07))
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
                } else {
                    d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
                }
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                // first click selects, a click on the selected row opens it
                let Some(crate::app::Overlay::Queue(s)) = this.overlay.as_mut() else {
                    return;
                };
                if s.selected == Some(Key::Wait(pane_id)) || ev.click_count() > 1 {
                    this.open_queue_item(pane_id, window, cx);
                } else {
                    s.selected = Some(Key::Wait(pane_id));
                    s.confirm = None;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size(px(16.))
                    .mt(px(2.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(ic, 14., color)),
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
                            .text_color(t.fg)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(it.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.fg_subtle)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(it.subtitle.clone()),
                    )
                    .when_some(detail, |d, text| d.child(detail_block(&text, t)))
                    .when(sel, |d| d.child(self.row_actions(&it, t, cx))),
            )
            .when(i < 9, |d| {
                d.child(
                    div()
                        .flex_none()
                        .mt(px(2.))
                        .font_family(term_family())
                        .text_size(px(11.))
                        .text_color(t.fg_subtle)
                        .child((i + 1).to_string()),
                )
            })
            .into_any_element()
    }

    /// The selected row's buttons, each with its key.
    fn row_actions(&self, it: &Item, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_id = it.pane_id;
        let open = button("queue-open", "Open", "↩", t, ButtonKind::Ghost).on_click(cx.listener(
            move |this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.open_queue_item(pane_id, window, cx);
            },
        ));
        let row = div().pt_1p5().flex().items_center().gap_2();
        match &it.ask {
            Ask::Approve(_) => {
                let (yes, no) = (it.clone(), it.clone());
                let risky = it.risky;
                row.child(
                    button("queue-approve", "Approve", "a", t, if risky { ButtonKind::Danger } else { ButtonKind::Soft })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            if risky {
                                if let Some(crate::app::Overlay::Queue(s)) = this.overlay.as_mut() {
                                    s.confirm = Some(yes.pane_id);
                                }
                                cx.notify();
                            } else {
                                this.answer(&yes, true, cx);
                            }
                        })),
                )
                .child(button("queue-reject", "Reject", "r", t, ButtonKind::Ghost).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.answer(&no, false, cx);
                    },
                )))
                .child(open)
            }
            Ask::Question(_) => row.child(open),
            Ask::Done => row.child(open).child(button("queue-dismiss", "Dismiss", "d", t, ButtonKind::Ghost).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    crate::agents::mark_seen(pane_id);
                    this.after_answer(0, cx);
                }),
            )),
        }
    }
}

/// What the agent wants to run, in full (up to `DETAIL_LINES`), in a
/// recessed band.
fn detail_block(text: &str, t: &Theme) -> impl IntoElement {
    let lines: Vec<&str> = text.lines().collect();
    let mut shown = lines.iter().take(DETAIL_LINES).copied().collect::<Vec<_>>().join("\n");
    if lines.len() > DETAIL_LINES {
        shown.push_str(&format!("\n… {} more lines", lines.len() - DETAIL_LINES));
    }
    div()
        .mt_1p5()
        .px_2p5()
        .py_2()
        .rounded_lg()
        .bg(gpui::hsla(0., 0., 0., 0.18))
        .font_family(term_family())
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(t.fg)
        .whitespace_normal()
        .child(SharedString::from(shown))
}

/// "Approve in <project>?" for a risky request: Cancel / Approve.
fn confirm_footer(it: &Item, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    let yes = it.clone();
    div()
        .px_4()
        .py_3()
        .flex()
        .items_center()
        .gap_2()
        .border_t_1()
        .border_color(t.border)
        .bg(gpui::hsla(0., 0., 0., 0.14))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .text_color(t.fg_muted)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(format!("Approve {}?", it.title)),
        )
        .child(button("queue-confirm-no", "Cancel", "esc", t, ButtonKind::Ghost).on_click(cx.listener(
            |this, _: &ClickEvent, _, cx| {
                if let Some(crate::app::Overlay::Queue(s)) = this.overlay.as_mut() {
                    s.confirm = None;
                }
                cx.notify();
            },
        )))
        .child(button("queue-confirm-yes", "Approve", "↩", t, ButtonKind::Danger).on_click(cx.listener(
            move |this, _: &ClickEvent, _, cx| {
                if let Some(crate::app::Overlay::Queue(s)) = this.overlay.as_mut() {
                    s.confirm = None;
                }
                this.answer(&yes, true, cx);
            },
        )))
}

/// The agent's name and project, above its runs.
fn agent_header(name: &str, project: &str, t: &Theme) -> impl IntoElement {
    section_header(name, None, t).when(!project.is_empty(), |d| {
        d.child(
            div()
                .font_weight(FontWeight::NORMAL)
                .text_color(t.fg_subtle)
                .child(SharedString::from(project.to_string())),
        )
    })
}

fn empty(t: &Theme) -> impl IntoElement {
    div()
        .py_8()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .child(
            div()
                .size(px(32.))
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(Icon::Check, 14., t.fg_subtle)),
        )
        .child(div().text_color(t.fg_muted).child("Nothing waiting"))
}
