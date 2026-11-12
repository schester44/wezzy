//! Seams: the split dividers double as places to open something. Click a
//! divider (press and release without moving) and a small menu opens on it,
//! where you clicked: a terminal, the default agent, or the editor, opened
//! right there. The pane after the divider (below or right of it) is split
//! and the new pane takes the side next to the divider. Dragging the divider
//! still resizes and opens nothing.

use crate::app::TermApp;
use crate::ui::{icon, with_alpha, Icon, Theme};
use gpui::prelude::*;
use gpui::{div, point, px, Context, MouseButton, MouseDownEvent, Pixels, Point, Window};
use mux::tab::{PositionedSplit, SplitDirection, SplitRequest, SplitSize};

/// Movement (px) that turns a press on a divider into a resize drag.
const DRAG_SLOP: f32 = 3.;
const BUTTON: f32 = 26.;
const PAD: f32 = 3.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Terminal,
    Agent,
    Editor,
}

const ITEMS: [Item; 3] = [Item::Terminal, Item::Agent, Item::Editor];

pub struct Seam {
    /// `PositionedSplit::index` of the divider
    index: usize,
    direction: SplitDirection,
    /// Where the menu sits, in window coordinates: on the divider's center
    /// line, level with the click.
    at: Point<Pixels>,
    open: bool,
    hovered: Option<Item>,
    /// Pressed on the divider and not moved yet: releasing opens the menu,
    /// moving resizes.
    press: Option<Point<Pixels>>,
}

impl Seam {
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// The menu's length along the divider.
fn menu_long() -> f32 {
    BUTTON * ITEMS.len() as f32 + PAD * 2.
}

impl TermApp {
    /// A press on divider `s` at `pos`: a click opens the menu there, a drag
    /// resizes (`split_drag` is set by the caller).
    pub(crate) fn press_seam(&mut self, s: &PositionedSplit, pos: Point<Pixels>) {
        let g = self.grid.get();
        let (cw, lh) = (f32::from(g.cell_w), f32::from(g.line_h));
        let half = menu_long() / 2.;
        let at = match s.direction {
            // a vertical line at column `left`, `size` rows tall
            SplitDirection::Horizontal => {
                let lo = s.top as f32 * lh + half;
                let hi = ((s.top + s.size) as f32 * lh - half).max(lo);
                point(
                    g.origin.x + px((s.left as f32 + 0.5) * cw),
                    g.origin.y + px(f32::from(pos.y - g.origin.y).clamp(lo, hi)),
                )
            }
            SplitDirection::Vertical => {
                let lo = s.left as f32 * cw + half;
                let hi = ((s.left + s.size) as f32 * cw - half).max(lo);
                point(
                    g.origin.x + px(f32::from(pos.x - g.origin.x).clamp(lo, hi)),
                    g.origin.y + px((s.top as f32 + 0.5) * lh),
                )
            }
        };
        self.seam = Some(Seam {
            index: s.index,
            direction: s.direction,
            at,
            open: false,
            hovered: None,
            press: Some(pos),
        });
    }

    /// While a press on a divider hasn't moved far enough to be a drag: true
    /// means hold the divider still. Past the slop it's a resize and no menu.
    pub(crate) fn seam_holds(&mut self, pos: Point<Pixels>) -> bool {
        let Some(press) = self.seam.as_ref().and_then(|s| s.press) else {
            return false;
        };
        let (dx, dy) = (f32::from(pos.x - press.x), f32::from(pos.y - press.y));
        if dx.hypot(dy) < DRAG_SLOP {
            return true;
        }
        self.seam = None;
        false
    }

    /// Mouse up after a press on a divider that never became a drag: open
    /// the menu.
    pub(crate) fn release_seam(&mut self) -> bool {
        let Some(s) = self.seam.as_mut().filter(|s| s.press.is_some()) else {
            return false;
        };
        s.press = None;
        s.open = true;
        self.split_drag = None;
        true
    }

    pub(crate) fn close_seam(&mut self) {
        self.seam = None;
    }

    /// Open `item` at the seam: split the pane just after the divider (below
    /// or right of it), level with the +, and put the new pane on the
    /// divider's side.
    fn pick_seam(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let Some(seam) = self.seam.take() else {
            return;
        };
        let g = self.grid.get();
        let Some(tab) = self.active_tab() else {
            return;
        };
        let Some(split) = tab.iter_splits().into_iter().find(|s| s.index == seam.index) else {
            return;
        };
        let col = f32::from((seam.at.x - g.origin.x) / g.cell_w).max(0.) as usize;
        let row = f32::from((seam.at.y - g.origin.y) / g.line_h).max(0.) as usize;
        let target = tab.iter_panes().into_iter().find(|p| match split.direction {
            SplitDirection::Horizontal => {
                p.left == split.left + 1 && row >= p.top && row < p.top + p.height
            }
            SplitDirection::Vertical => {
                p.top == split.top + 1 && col >= p.left && col < p.left + p.width
            }
        });
        let Some(target) = target else {
            return;
        };
        self.next_split = Some((
            target.pane.pane_id(),
            SplitRequest {
                direction: seam.direction,
                // the new pane takes the side against the divider
                target_is_second: false,
                top_level: false,
                size: SplitSize::Percent(50),
            },
        ));
        self.region = crate::regions::Region::Terminal;
        match item {
            Item::Terminal => self.new_terminal(cx),
            Item::Agent => self.launch_harness(None, crate::launch::Placement::Split, window, cx),
            Item::Editor => self.open_editor_here(window, cx),
        }
        // a failed launch never consumed it
        self.next_split = None;
        cx.notify();
    }

    /// The menu on a clicked divider. Positioned in the
    /// terminal area, whose top-left is the grid origin minus the padding.
    pub(crate) fn seam_view(&self, t: &Theme, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let seam = self.seam.as_ref().filter(|s| s.open)?;
        let g = self.grid.get();
        let area = point(g.origin.x - g.pad.x, g.origin.y - g.pad.y);
        let at = point(seam.at.x - area.x, seam.at.y - area.y);
        let vertical = seam.direction == SplitDirection::Horizontal;

        let agent = {
            let s = crate::settings::get();
            s.harness(&s.default_harness_name())
                .map(|h| h.label())
                .unwrap_or_else(|| "Agent".into())
        };
        let label = |item: Item| match item {
            Item::Terminal => "Terminal".to_string(),
            Item::Agent => agent.clone(),
            Item::Editor => "Editor".to_string(),
        };
        let pad = PAD;
        let long = menu_long();
        let short = BUTTON + pad * 2.;
        let (w, h) = if vertical { (short, long) } else { (long, short) };
        let hovered = seam.hovered;

        let buttons = ITEMS.iter().map(|&item| {
            let glyph = match item {
                Item::Terminal => Icon::Terminal,
                Item::Agent => Icon::Sparkles,
                Item::Editor => Icon::Code,
            };
            let hot = hovered == Some(item);
            div()
                .id(("seam-item", item as usize))
                .size(px(BUTTON))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(hot, |d| d.bg(with_alpha(t.fg, 0.08)))
                .child(icon(glyph, 14., if hot { t.fg } else { t.fg_muted }))
                .on_hover(cx.listener(move |this, on: &bool, _, cx| {
                    if let Some(s) = this.seam.as_mut() {
                        if *on {
                            s.hovered = Some(item);
                        } else if s.hovered == Some(item) {
                            s.hovered = None;
                        }
                    }
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.pick_seam(item, window, cx);
                    }),
                )
        });

        let menu = div()
            .absolute()
            .left(at.x - px(w / 2.))
            .top(at.y - px(h / 2.))
            .w(px(w))
            .h(px(h))
            .p(px(pad))
            .flex()
            .map(|d| if vertical { d.flex_col() } else { d.flex_row() })
            .bg(t.elevated)
            .rounded_lg()
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .occlude()
            .children(buttons)
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.close_seam();
                cx.notify();
            }));

        // what the hovered button opens, beside the menu
        let tag = hovered.map(|item| {
            let gap = 8.;
            div()
                .absolute()
                .map(|d| {
                    if vertical {
                        let i = ITEMS.iter().position(|&x| x == item).unwrap_or(0) as f32;
                        d.left(at.x + px(w / 2. + gap))
                            .top(at.y - px(h / 2.) + px(pad + BUTTON * i + (BUTTON - 22.) / 2.))
                    } else {
                        d.left(at.x - px(w / 2.)).top(at.y + px(h / 2. + gap))
                    }
                })
                .h(px(22.))
                .px_2()
                .flex()
                .items_center()
                .whitespace_nowrap()
                .text_xs()
                .text_color(t.fg_muted)
                .bg(t.elevated)
                .rounded_md()
                .border_1()
                .border_color(t.border)
                .child(label(item))
        });

        Some(
            div()
                .absolute()
                .inset_0()
                .child(menu)
                .children(tag)
                .into_any_element(),
        )
    }
}
