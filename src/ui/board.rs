//! The board overlay's card (⌘⇧J, `board.rs`): lanes side by side, or the open
//! card with its thread.

use super::textfield::{text_field, FieldStyle};
use super::widgets::{button, term_family, with_alpha, ButtonKind};
use super::{icon, Icon, Theme};
use crate::app::{Overlay, TermApp};
use crate::board::{BoardState, DragCard, Editing, Input, Picker};
use gpui::{
    div, prelude::*, px, AnyElement, ClickEvent, Context, FontWeight, MouseButton, Render, SharedString, Window,
};
use std::collections::HashMap;
use handoff::{Actor, Card, CardDetail, LaneCards, Role};

/// What follows the cursor while a card is dragged.
struct DragPreview {
    title: SharedString,
    t: Theme,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = &self.t;
        div()
            .max_w(px(260.))
            .px_3()
            .py_1p5()
            .rounded_md()
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .shadow_md()
            .text_size(px(13.))
            .text_color(t.fg)
            .child(self.title.clone())
    }
}

fn role_icon(role: Role) -> Option<Icon> {
    match role {
        Role::You => Some(Icon::User),
        Role::Agent => Some(Icon::Bot),
        Role::Done => Some(Icon::Check),
        Role::None => None,
    }
}

impl TermApp {
    /// The board's shortcut sheet (`?` in it).
    pub(super) fn board_help_modal(&self, t: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Overlay::Board(state)) = &self.overlay else { return None };
        let query = state.help.as_deref()?;
        use super::help::group;
        let left = vec![
            group(
                "Navigate",
                &[
                    ("h l", "Lane left / right"),
                    ("j k", "Card down / up"),
                    ("g G", "First / last card"),
                    ("enter", "Open the card"),
                    ("[ ]", "Another board"),
                ],
            ),
            group(
                "Cards",
                &[
                    ("c", "New card in this lane"),
                    ("x space", "Done"),
                    ("r", "Reply"),
                    ("a", "Give it to an agent"),
                    ("e", "Edit the title"),
                    ("b", "Edit the body (card open)"),
                    ("d", "Delete"),
                ],
            ),
        ];
        let right = vec![
            group(
                "Move",
                &[("H L", "Card to the lane left / right"), ("J K", "Card down / up its lane")],
            ),
            group(
                "Lanes",
                &[
                    ("N", "New lane"),
                    ("R", "Rename"),
                    ("T", "Role: you, agent, none, done"),
                    ("< >", "Move the lane left / right"),
                    ("X", "Delete it, once empty"),
                ],
            ),
        ];
        Some(super::help::sheet("Board", query, vec![left, right], 600., t, cx, |this| {
            if let Some(Overlay::Board(s)) = this.overlay.as_mut() {
                s.help = None;
            }
        }))
    }

    /// `height` is the room for the lanes.
    pub(super) fn board_view(&self, state: &BoardState, height: f32, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let deleting = state.deleting.and_then(|id| {
            let on_board = state.board.lanes.iter().flat_map(|l| &l.cards).find(|c| c.id == id);
            on_board.map(|c| c.title.clone())
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
                    .child(div().font_weight(FontWeight::MEDIUM).child(state.project.name.clone())),
            )
            .map(|d| match (&state.picking, &state.detail) {
                (Some(picker), _) => d.child(self.agent_picker(picker, height, t, cx)),
                (None, Some(detail)) => d.child(self.card_detail(state, detail, height, t, cx)),
                (None, None) => d.child(self.lanes(state, height, t, cx)),
            })
            .when_some(deleting, |d, title| d.child(delete_footer(title, t, cx)))
    }

    fn lanes(&self, state: &BoardState, height: f32, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let new_lane = matches!(state.input, Some(Input { what: Editing::NewLane, .. }));
        div()
            .id("board-lanes")
            .h(px(height))
            .p_2()
            .gap_2()
            .flex()
            .overflow_x_scroll()
            .children(
                state
                    .board
                    .lanes
                    .iter()
                    .enumerate()
                    .map(|(i, lane)| self.lane(state, i, lane, t, cx)),
            )
            .when(new_lane, |d| {
                d.child(
                    div()
                        .flex_1()
                        .min_w(px(200.))
                        .children(state.input.as_ref().map(|input| self.board_input(input, t, cx))),
                )
            })
    }

    fn lane(
        &self,
        state: &BoardState,
        index: usize,
        lane: &LaneCards,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let lane_id = lane.lane.id;
        let current = state.lane == index;
        let input = state.input.as_ref();
        let renaming = input.filter(|i| i.what == Editing::RenameLane(lane_id));
        let adding = input.filter(|i| i.what == Editing::NewCard(lane_id));
        let done = lane.lane.role == Role::Done;
        let hover = with_alpha(t.fg, 0.03);

        let header = match renaming {
            Some(input) => self.board_input(input, t, cx).into_any_element(),
            None => div()
                .flex_none()
                .h(px(36.))
                .px_4()
                .flex()
                .items_center()
                .gap_1p5()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(if current { t.fg } else { t.fg_muted })
                .children(role_icon(lane.lane.role).map(|ic| icon(ic, 12., t.fg_subtle)))
                .child(lane.lane.name.clone())
                .child(
                    div()
                        .font_family(term_family())
                        .text_size(px(11.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(t.fg_subtle)
                        .child(lane.cards.len().to_string()),
                )
                .into_any_element(),
        };

        let mut list = div()
            .id(("board-lane", lane_id as u64))
            .flex_1()
            .min_h_0()
            .pb_2()
            .overflow_y_scroll()
            // a card dropped on the lane itself goes to its end
            .drag_over::<DragCard>(move |s, _, _, _| s.bg(hover))
            .on_drop(cx.listener(move |this, card: &DragCard, _, cx| this.board_drop(card.id, lane_id, None, cx)));
        if let Some(scroll) = state.scrolls.get(&lane_id) {
            list = list.track_scroll(scroll);
        }
        let owners = crate::board::agent_labels();
        let cards = lane.cards.iter().enumerate().map(|(i, card)| {
            let selected = current && state.card == i && adding.is_none();
            let title = input.filter(|i| i.what == Editing::Title(card.id));
            self.card_row(card, lane_id, i, selected, done, title, &owners, t, cx)
        });

        div()
            .flex_1()
            .min_w(px(200.))
            .flex()
            .flex_col()
            // the lane you're in is drawn like the active pane: the terminal's
            // surface and a faint edge; the others recede into the modal
            .rounded_lg()
            .overflow_hidden()
            .border_1()
            .map(|d| {
                if current {
                    d.bg(t.term_bg).border_color(with_alpha(t.fg, 0.10))
                } else {
                    d.border_color(gpui::transparent_black())
                }
            })
            .child(header)
            .child(list.children(cards).children(adding.map(|input| self.board_input(input, t, cx))))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn card_row(
        &self,
        card: &Card,
        lane_id: i64,
        index: usize,
        selected: bool,
        done: bool,
        editing: Option<&Input>,
        owners: &HashMap<String, String>,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = card.id;
        let accent = t.accent;
        let theme = *t;
        let drag = DragCard { id, title: card.title.clone() };
        // the circle you tick: a check once it's done
        let tick = div()
            .id(("board-tick", id as u64))
            .mt(px(2.))
            .size(px(14.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .map(|d| {
                if done {
                    d.child(icon(Icon::Check, 12., t.fg_subtle))
                } else {
                    d.border_1()
                        .border_color(with_alpha(t.fg, 0.28))
                        .cursor_pointer()
                        .hover(|s| s.border_color(t.fg_muted))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.with_board(cx, |s| s.complete(id));
                        }))
                }
            });
        let title = match editing {
            Some(input) => self.board_field(input, t, cx).into_any_element(),
            None => div()
                .text_color(if done { t.fg_muted } else { t.fg })
                .child(card.title.clone())
                .into_any_element(),
        };
        let meta = div()
            .flex()
            .items_center()
            .gap_2()
            .font_family(term_family())
            .text_size(px(11.))
            .text_color(t.fg_subtle)
            .child(format!("#{id}"))
            .children(card.tag.clone())
            // whose it is, when that agent is running
            .children(card.agent.as_ref().and_then(|a| owners.get(a)).map(|name| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .font_family(super::view::UI_FONT)
                    .child(icon(Icon::Bot, 11., t.fg_subtle))
                    .child(name.clone())
            }))
            .when(card.replies > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(icon(Icon::Message, 11., t.fg_subtle))
                        .child(card.replies.to_string()),
                )
            });

        div()
            .id(("board-card", id as u64))
            .relative()
            .w_full()
            .px_4()
            .py_1p5()
            .flex()
            .gap_2p5()
            .cursor_pointer()
            // where a dragged card would land: above this one
            .border_t_2()
            .border_color(gpui::transparent_black())
            .drag_over::<DragCard>(move |s, _, _, _| s.border_color(accent))
            .map(|d| {
                if selected {
                    d.bg(with_alpha(t.fg, 0.07))
                        .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
                } else {
                    d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
                }
            })
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.board_click(id, ev.click_count() > 1, cx);
            }))
            .on_drag(drag, move |card, _, _, cx| {
                cx.new(|_| DragPreview { title: card.title.clone().into(), t: theme })
            })
            .on_drop(cx.listener(move |this, card: &DragCard, _, cx| {
                this.board_drop(card.id, lane_id, Some(index), cx)
            }))
            .child(tick)
            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(title).child(meta))
            .into_any_element()
    }

    /// Whose card is it: every running agent, then nobody.
    fn agent_picker(&self, picker: &Picker, height: f32, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = picker.agents.iter().map(|(_, label)| Some(label.clone())).chain([None]);
        div().id("board-agents").h(px(height)).py_1p5().overflow_y_scroll().children(rows.enumerate().map(
            |(i, label)| {
                let selected = picker.cursor == i;
                div()
                    .id(("board-agent", i))
                    .relative()
                    .w_full()
                    .h(px(32.))
                    .px_4()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .cursor_pointer()
                    .map(|d| {
                        if selected {
                            d.bg(with_alpha(t.fg, 0.07))
                                .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent))
                        } else {
                            d.hover(|s| s.bg(with_alpha(t.fg, 0.03)))
                        }
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.with_board(cx, |s| s.assign_row(i));
                    }))
                    .child(
                        div()
                            .w(px(14.))
                            .font_family(term_family())
                            .text_size(px(11.))
                            .text_color(t.fg_subtle)
                            .child(if i < 9 { (i + 1).to_string() } else { String::new() }),
                    )
                    .map(|d| match label {
                        Some(label) => d.child(icon(Icon::Bot, 13., t.fg_subtle)).child(div().text_color(t.fg).child(label)),
                        None => d.child(icon(Icon::X, 13., t.fg_subtle)).child(div().text_color(t.fg_muted).child("Nobody")),
                    })
            },
        ))
    }

    /// The open card: title, body, thread, and the reply box.
    fn card_detail(
        &self,
        state: &BoardState,
        detail: &CardDetail,
        height: f32,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let card = &detail.card;
        let id = card.id;
        let input = state.input.as_ref();
        let field = |what: Editing| input.filter(|i| i.what == what);
        let done = detail.role == Role::Done;

        let back = div()
            .id("board-back")
            .flex_none()
            .h(px(36.))
            .px_3()
            .flex()
            .items_center()
            .gap_1p5()
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(t.fg_muted)
            .hover(|s| s.text_color(t.fg))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.with_board(cx, |s| {
                    s.detail = None;
                    s.input = None;
                    Ok(())
                })
            }))
            .child(icon(Icon::ChevronLeft, 13., t.fg_subtle))
            .children(role_icon(detail.role).map(|ic| icon(ic, 12., t.fg_subtle)))
            .child(detail.lane.clone())
            .child(div().font_family(term_family()).text_size(px(11.)).text_color(t.fg_subtle).child(format!("#{id}")));

        let title = match field(Editing::Title(id)) {
            Some(input) => self.board_field(input, t, cx).into_any_element(),
            None => div().font_weight(FontWeight::MEDIUM).text_color(t.fg).child(card.title.clone()).into_any_element(),
        };
        let body = match field(Editing::Body(id)) {
            Some(input) => Some(self.board_field(input, t, cx).into_any_element()),
            None => (!card.body.is_empty()).then(|| div().text_color(t.fg_muted).child(card.body.clone()).into_any_element()),
        };
        let thread = detail.thread.iter().map(|reply| {
            let who = match reply.author {
                Actor::You => Icon::User,
                Actor::Agent => Icon::Bot,
            };
            div()
                .flex()
                .gap_2p5()
                .child(div().mt(px(3.)).child(icon(who, 12., t.fg_subtle)))
                .child(div().flex_1().min_w_0().text_color(t.fg).child(reply.body.clone()))
                .child(
                    div()
                        .flex_none()
                        .mt(px(2.))
                        .font_family(term_family())
                        .text_size(px(11.))
                        .text_color(t.fg_subtle)
                        .child(crate::queue::ago(reply.at.max(0) as u64)),
                )
        });

        let footer = match field(Editing::Reply(id)) {
            Some(input) => self.board_input(input, t, cx).into_any_element(),
            None => div()
                .flex_none()
                .px_4()
                .py_2p5()
                .flex()
                .items_center()
                .gap_2()
                .child(button("board-reply", "Reply", "↩", t, ButtonKind::Soft).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        this.with_board(cx, |s| {
                            s.input = Some(Input { what: Editing::Reply(id), field: Default::default() });
                            Ok(())
                        })
                    },
                )))
                .when(!done, |d| {
                    d.child(button("board-done", "Done", "x", t, ButtonKind::Ghost).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.with_board(cx, |s| {
                                s.detail = None;
                                s.complete(id)
                            })
                        },
                    )))
                })
                .into_any_element(),
        };

        div()
            .h(px(height))
            .flex()
            .flex_col()
            .child(back)
            .child(
                div()
                    .id("board-detail")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&state.detail_scroll)
                    .px_4()
                    .pb_3()
                    .max_w(px(720.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(title)
                    .children(body)
                    .children(thread),
            )
            .child(div().flex_none().border_t_1().border_color(t.border).child(footer))
    }

    /// A text box of the board's, in its recessed frame.
    fn board_input(&self, input: &Input, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex_none().px_3().py_2().child(
            div()
                .px_2p5()
                .py_1p5()
                .rounded_lg()
                .bg(gpui::hsla(0., 0., 0., 0.2))
                .border_1()
                .border_color(with_alpha(t.accent, 0.6))
                .child(self.board_field(input, t, cx)),
        )
    }

    fn board_field(&self, input: &Input, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let new_card = matches!(input.what, Editing::NewCard(_));
        text_field(
            &input.field,
            FieldStyle {
                color: if new_card { t.fg_muted } else { t.fg },
                // a new card's first line is its title
                first_line: new_card.then_some((t.fg, FontWeight::NORMAL)),
                placeholder: "".into(),
                placeholder_color: t.fg_subtle,
                caret: Some(t.accent),
                selection: with_alpha(t.accent, 0.3),
                line_height: px(18.),
                max_rows: Some(10),
            },
            move |ev, _, cx| {
                app.update(cx, |this, cx| {
                    if let Some(Overlay::Board(state)) = this.overlay.as_mut() {
                        if let Some(input) = state.input.as_mut() {
                            input.field.mouse(ev);
                        }
                    }
                    cx.notify();
                })
                .ok();
            },
        )
    }
}

/// `Delete "title"?` with its two answers, like the git panel's discard.
fn delete_footer(title: String, t: &Theme, cx: &mut Context<TermApp>) -> impl IntoElement {
    let answer = |yes: bool| {
        cx.listener(move |this: &mut TermApp, _: &ClickEvent, _, cx| {
            this.with_board(cx, |s| s.confirm_delete(yes));
        })
    };
    div()
        .flex_none()
        .h(px(44.))
        .px_4()
        .flex()
        .items_center()
        .gap_2()
        .border_t_1()
        .border_color(t.border)
        .bg(gpui::hsla(0., 0., 0., 0.14))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .text_color(t.fg_muted)
                .child(format!("Delete “{title}”?")),
        )
        .child(button("board-delete-no", "Cancel", "esc", t, ButtonKind::Ghost).on_click(answer(false)))
        .child(button("board-delete-yes", "Delete", "d", t, ButtonKind::Danger).on_click(answer(true)))
}
