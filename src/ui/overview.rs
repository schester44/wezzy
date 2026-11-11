//! The agent overview (⌘O): a header with counts, then one live card per
//! agent, grouped by workspace. State and keys are in `overview.rs`.
//!
//!   │ Agents 6                                         ⚠ 2  ◌ 3  ● 1 │
//!   │ wezzy 4                                                       │
//!   │▌⚠ 1 parser-fix  Claude  fix/parser  3m ┆ ◌ 2 Claude  main  40s │
//!   │▌ Claude needs your permission to use… ┆   Editing view.rs     │
//!   │▌ <the pane's last rows, cropped>      ┆   <…>                 │
//!   │▌           Allow y  Always Y  Deny n  ┆                       │
//!
//! Each tail paints the pane's own lines at its real columns (font scaled to
//! 0.75), clipped at the card's edge. Panes are never resized for a card.

use super::view::{section_header, status_indicator, subagent_line, term_family, with_alpha};
use super::{icon, spinner, Icon, Theme};
use crate::agents::{Agent, Status};
use crate::app::TermApp;
use crate::overview::{self, Answer, Row};
use crate::render::{self, PaintOpts};
use gpui::{
    canvas, div, prelude::*, px, AnyElement, ClickEvent, ContentMask, Context, FontWeight, Hsla,
    MouseDownEvent, SharedString, Window,
};
use mux::Mux;

/// Narrowest a card gets before the grid drops a column.
const CARD_MIN_W: f32 = 440.;
const MAX_COLS: usize = 4;
/// Pane rows in each card's tail, and with more than `MANY` agents.
const TAIL_ROWS: usize = 12;
const TAIL_ROWS_MANY: usize = 6;
const MANY: usize = 12;
/// Card chrome above the tail: padding, line 1, line 2, gap.
const CARD_HEAD_H: f32 = 12. + 20. + 18. + 8.;
const CARD_FOOT_H: f32 = 12.;

impl TermApp {
    pub(crate) fn overview_view(&mut self, t: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let agents = crate::agents::collect(self.active_pane().map(|p| p.pane_id()));
        let config = config::configuration();
        let m = render::metrics(window, &config, self.font_scale * 0.75);
        let opts = PaintOpts {
            inactive_brightness: 1.0,
            default_cursor_bar: matches!(
                config.default_cursor_style,
                config::DefaultCursorStyle::BlinkingBar | config::DefaultCursorStyle::SteadyBar
            ),
            // hollow cursors: these panes don't have your keys
            focused: false,
        };
        let width = f32::from(window.viewport_size().width);
        let cols = ((width / CARD_MIN_W).floor() as usize).clamp(1, MAX_COLS).min(agents.len().max(1));
        let Some(o) = self.overview.as_mut() else {
            return div().into_any_element();
        };
        o.cols = cols;
        o.prune();
        let selected = o.index(&agents);
        let rows = overview::rows(&agents, cols);
        if o.reveal {
            o.reveal = false;
            if let Some(r) = overview::row_of(&rows, selected) {
                // the first card brings the top label along
                o.scroll.scroll_to_item(if r == 1 { 0 } else { r });
            }
        }
        let reply = o.reply.clone();
        let answerable: Vec<bool> = agents.iter().map(|a| o.can_answer(a)).collect();
        let scroll = o.scroll.clone();

        let body = if agents.is_empty() {
            self.overview_empty(t, cx).into_any_element()
        } else {
            let single = agents.len() == 1;
            let tail_rows = if agents.len() > MANY { TAIL_ROWS_MANY } else { TAIL_ROWS };
            let card_h = px(CARD_HEAD_H + CARD_FOOT_H) + m.line_h * tail_rows as f32;
            let rows: Vec<AnyElement> = rows
                .iter()
                .map(|row| match row {
                    Row::Workspace(name, n) => section_header(name, Some(n.to_string()), t).into_any_element(),
                    Row::Cards(idx) => div()
                        .flex()
                        .flex_row()
                        .border_t_1()
                        .border_color(t.border)
                        .map(|d| if single { d.flex_1().min_h_0() } else { d.flex_none().h(card_h) })
                        .children((0..cols).map(|c| match idx.get(c) {
                            Some(&i) => card(Card {
                                agent: &agents[i],
                                index: i,
                                selected: i == selected,
                                answerable: answerable[i],
                                reply: reply.as_ref().filter(|(id, _)| *id == agents[i].pane_id).map(|(_, r)| r.clone()),
                                last: c + 1 == cols,
                                metrics: m.clone(),
                                opts: PaintOpts { ..opts },
                            }, t, cx)
                            .into_any_element(),
                            None => div().flex_1().min_w_0().into_any_element(),
                        }))
                        .into_any_element(),
                })
                .collect();
            div()
                .id("overview-grid")
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .when(!single, |d| d.overflow_y_scroll().track_scroll(&scroll).pb_4())
                .children(rows)
                .into_any_element()
        };

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.term_bg)
            .child(self.overview_header(&agents, t, cx))
            .child(body)
            .into_any_element()
    }

    /// "Agents 6", and on the right what wants you: ⚠ needs input, the
    /// spinner working, ● done and unseen. Each count cycles through its agents.
    fn overview_header(&self, agents: &[Agent], t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let needs = agents.iter().filter(|a| a.status == Status::NeedsInput).count();
        let working = agents.iter().filter(|a| a.status == Status::Working).count();
        let unseen = agents.iter().filter(|a| a.status == Status::Done && a.unseen).count();
        let count = |id: &'static str, n: usize, ind: AnyElement| {
            (n > 0).then(|| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .font_family(term_family())
                    .text_size(px(11.))
                    .text_color(t.fg_muted)
                    .child(ind)
                    .child(n.to_string())
            })
        };
        div()
            .flex_none()
            .px_4()
            .pt(px(14.))
            .pb(px(12.))
            .flex()
            .items_center()
            .gap_4()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_baseline()
                    .gap_1p5()
                    .child(div().font_weight(FontWeight::MEDIUM).child("Agents"))
                    .child(
                        div()
                            .font_family(term_family())
                            .text_size(px(11.))
                            .text_color(t.fg_subtle)
                            .child(agents.len().to_string()),
                    ),
            )
            .children(count("ov-needs", needs, icon(Icon::Alert, 13., t.warning).into_any_element()).map(|d| {
                d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.overview_cycle(|a| a.status == Status::NeedsInput);
                    cx.notify();
                }))
            }))
            .children(count("ov-working", working, spinner(12., t.fg_muted).into_any_element()).map(|d| {
                d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.overview_cycle(|a| a.status == Status::Working);
                    cx.notify();
                }))
            }))
            .children(
                count(
                    "ov-unseen",
                    unseen,
                    div().size(px(6.)).rounded_full().bg(t.success).into_any_element(),
                )
                .map(|d| {
                    d.on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.overview_cycle(|a| a.status == Status::Done && a.unseen);
                        cx.notify();
                    }))
                }),
            )
    }

    fn overview_empty(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let action = crate::commands::AppCommand::NewAgent.action();
        let key = self.keys.shortcut_for(&action);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_1()
                    .border_color(t.border)
                    .child(icon(Icon::Bot, 14., t.fg_subtle)),
            )
            .child(div().text_color(t.fg_muted).child("No agents"))
            .child(
                soft_button("ov-new-agent", "New agent", key, t).on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| {
                        this.perform(action.clone(), None, window, cx);
                        cx.notify();
                    },
                )),
            )
    }
}

struct Card<'a> {
    agent: &'a Agent,
    index: usize,
    selected: bool,
    answerable: bool,
    reply: Option<String>,
    /// last column: no hairline on the right
    last: bool,
    metrics: render::Metrics,
    opts: PaintOpts,
}

fn card(c: Card, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    let a = c.agent;
    let pane_id = a.pane_id;
    let name = a.display_name().to_string();
    // the harness, when the name doesn't already say it
    let harness = a.name.is_some().then_some(a.kind);
    let branch = a.cwd.as_deref().and_then(overview::branch);
    let (detail_color, detail) = match a.status {
        Status::NeedsInput => (t.warning, a.detail.clone()),
        Status::Working => (t.fg_muted, a.detail.clone()),
        Status::Done | Status::Idle => (t.fg_muted, None),
    };
    let subagents = a.subagents.clone();
    let more = subagents.len().saturating_sub(2);

    let line1 = div()
        .flex_none()
        .h(px(20.))
        .flex()
        .items_center()
        .gap_2()
        .whitespace_nowrap()
        .child(
            div()
                .w(px(14.))
                .flex_none()
                .flex()
                .justify_center()
                .child(status_indicator(a.status, a.unseen, t)),
        )
        .when(c.index < 9, |d| {
            d.child(
                div()
                    .flex_none()
                    .font_family(term_family())
                    .text_size(px(11.))
                    .text_color(t.fg_subtle)
                    .child((c.index + 1).to_string()),
            )
        })
        .child(
            div()
                .id(("ov-name", pane_id))
                .flex_shrink()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(name)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.overview_jump(pane_id, window, cx);
                })),
        )
        .children(harness.map(|h| div().flex_none().text_size(px(12.)).text_color(t.fg_subtle).child(h)))
        .children(branch.map(|b| {
            div()
                .flex_shrink()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .text_size(px(12.))
                .text_color(t.fg_subtle)
                .child(icon(Icon::GitBranch, 11., t.fg_subtle))
                .child(div().overflow_hidden().text_ellipsis().child(b))
        }))
        .child(div().flex_1())
        .child(
            div()
                .flex_none()
                .font_family(term_family())
                .text_size(px(11.))
                .text_color(t.fg_subtle)
                .child(overview::ago(a.since)),
        );

    let line2 = div()
        .flex_none()
        .min_h(px(18.))
        .pl(px(22.))
        .flex()
        .flex_col()
        .gap_1()
        .text_size(px(12.))
        .children(detail.map(|d| {
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(detail_color)
                .child(d)
        }))
        .children(subagents.into_iter().take(2).map(|s| subagent_line(s, t)))
        .when(more > 0, |d| {
            d.child(
                div()
                    .font_family(term_family())
                    .text_size(px(11.))
                    .text_color(t.fg_subtle)
                    .child(format!("+{more}")),
            )
        });

    let (m, opts) = (c.metrics, c.opts);
    let tail = canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let Some(pane) = Mux::get().get_pane(pane_id) else { return };
            let rows = (f32::from(bounds.size.height) / f32::from(m.line_h)).floor() as usize;
            if rows == 0 {
                return;
            }
            let cols = (f32::from(bounds.size.width) / f32::from(m.cell_w)).ceil() as usize;
            let snap = render::bottom_snapshot(&pane, rows, cols);
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                render::paint_pane(bounds.origin, &snap, &m, &opts, window, cx);
            });
        },
    )
    .flex_1()
    .min_h_0()
    .w_full();

    div()
        .id(("ov-card", pane_id))
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_hidden()
        .pt(px(12.))
        .pb(px(CARD_FOOT_H))
        .px_4()
        .flex()
        .flex_col()
        .gap_0p5()
        .when(!c.last, |d| d.border_r_1().border_color(t.border))
        .map(|d| {
            if c.selected {
                d.bg(with_alpha(t.fg, 0.07))
                    .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
            } else {
                d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
            }
        })
        .child(line1)
        .child(line2)
        .child(div().flex_none().h(px(6.)))
        .child(tail)
        .when(c.answerable && c.reply.is_none(), |d| d.child(answer_buttons(a, t, cx)))
        .children(c.reply.map(|r| reply_input(r, t, cx)))
        .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
            if ev.click_count() >= 2 {
                this.overview_jump(pane_id, window, cx);
            } else {
                this.overview_select(pane_id);
                cx.notify();
            }
        }))
}

fn answer_buttons(a: &Agent, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    let pane_id = a.pane_id;
    let button = |id: &'static str, label: &'static str, key: &'static str, answer: Answer| {
        soft_button(id, label, Some(key.into()), t).on_click(cx.listener(
            move |this, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                if let Some(a) = crate::agents::collect(None).into_iter().find(|a| a.pane_id == pane_id) {
                    this.overview_answer(&a, answer);
                }
                cx.notify();
            },
        ))
    };
    div()
        .flex_none()
        .pt_2()
        .flex()
        .justify_end()
        .gap_2()
        .child(button("ov-allow", "Allow", "y", Answer::Allow))
        .child(button("ov-always", "Always", "Y", Answer::Always))
        .child(button("ov-deny", "Deny", "n", Answer::Deny))
}

/// The inline reply field: recessed, accent ring, caret at the end.
fn reply_input(text: String, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    let caret = || div().w(px(1.5)).h(px(14.)).flex_none().bg(t.accent);
    let lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    let n = lines.len();
    div()
        .id("ov-reply")
        .flex_none()
        .mt_2()
        .px_2()
        .py_1p5()
        .flex()
        .flex_col()
        .rounded_lg()
        .bg(gpui::hsla(0., 0., 0., 0.22))
        .border_1()
        .border_color(with_alpha(t.accent, 0.6))
        .text_size(px(13.))
        .children(lines.into_iter().enumerate().map(move |(i, line)| {
            let last = i + 1 == n;
            div()
                .min_h(px(18.))
                .flex()
                .items_center()
                .map(|d| {
                    if last && text.is_empty() {
                        d.child(caret()).child(div().ml_0p5().text_color(t.fg_subtle).child("Reply"))
                    } else if last {
                        d.child(div().whitespace_nowrap().child(SharedString::from(line))).child(caret().ml_px())
                    } else {
                        d.child(div().whitespace_nowrap().child(SharedString::from(line)))
                    }
                })
        }))
        .on_click(|_, _, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
            if let Some(o) = this.overview.as_mut() {
                o.reply = None;
            }
            cx.notify();
        }))
}

/// Soft fill, hairline, label plus its key dimmed (design.md "Button").
fn soft_button(id: &'static str, label: &'static str, key: Option<String>, t: &Theme) -> gpui::Stateful<gpui::Div> {
    let fill: Hsla = with_alpha(t.fg, 0.07);
    let hover: Hsla = with_alpha(t.fg, 0.12);
    div()
        .id(id)
        .flex_none()
        .h(px(22.))
        .px_2()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_md()
        .cursor_pointer()
        .bg(fill)
        .border_1()
        .border_color(t.border)
        .text_color(t.fg)
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| s.bg(hover))
        .child(label)
        .children(key.map(|k| div().text_color(t.fg_subtle).child(k)))
}
