//! Draws a `TextField` (`src/textfield.rs`): the wrapped text, the caret and
//! the selection, and hit-tests the mouse into text offsets. It inherits the
//! font from the parent's text style; the box around it is the caller's.
//!
//! Each paint leaves the visual rows in the field's `Shared`, so ↑/↓ can move
//! by wrapped rows and the next mouse event hit-tests what's on screen. With
//! `max_rows` the field caps its height and scrolls to keep the caret visible.

use crate::textfield::{FieldMouse, Shared, TextField, VRow};
use gpui::{
    fill, point, px, relative, size, App, Bounds, ContentMask, CursorStyle, DispatchPhase, Element,
    ElementId, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, ScrollWheelEvent, SharedString, Style, TextAlign, TextRun, Window, WrappedLine,
};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

pub struct FieldStyle {
    pub color: Hsla,
    /// the first line's color and weight (a commit subject)
    pub first_line: Option<(Hsla, FontWeight)>,
    pub placeholder: SharedString,
    pub placeholder_color: Hsla,
    /// `None` hides the caret (the box doesn't have focus)
    pub caret: Option<Hsla>,
    pub selection: Hsla,
    pub line_height: Pixels,
    pub max_rows: Option<usize>,
}

type MouseHandler = Rc<dyn Fn(FieldMouse, &mut Window, &mut App)>;

/// The element for `field`; `on_mouse` receives hit-tested mouse events and
/// should hand them to `TextField::mouse`.
pub fn text_field(
    field: &TextField,
    style: FieldStyle,
    on_mouse: impl Fn(FieldMouse, &mut Window, &mut App) + 'static,
) -> FieldElement {
    FieldElement {
        text: SharedString::from(field.text().to_string()),
        cursor: field.cursor(),
        selection: field.selection(),
        rev: field.rev(),
        shared: field.shared.clone(),
        style,
        on_mouse: Rc::new(on_mouse),
    }
}

pub struct FieldElement {
    text: SharedString,
    cursor: usize,
    selection: Range<usize>,
    rev: u64,
    shared: Rc<RefCell<Shared>>,
    style: FieldStyle,
    on_mouse: MouseHandler,
}

pub struct Prepainted {
    /// each logical line with its first visual row
    lines: Vec<(usize, WrappedLine)>,
    placeholder: Option<WrappedLine>,
    quads: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    hitbox: Hitbox,
}

impl IntoElement for FieldElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl FieldElement {
    fn runs(&self, window: &Window) -> Vec<TextRun> {
        let style = window.text_style();
        let run = |len: usize, color: Hsla, weight: Option<FontWeight>| {
            let mut font = style.font();
            if let Some(w) = weight {
                font.weight = w;
            }
            TextRun { len, font, color, background_color: None, underline: None, strikethrough: None }
        };
        let len = self.text.len();
        match self.style.first_line {
            Some((color, weight)) => {
                // the first run takes the newline too (shape_text skips it from the run)
                let first = self.text.find('\n').map(|i| i + 1).unwrap_or(len);
                vec![run(first, color, Some(weight)), run(len - first, self.style.color, None)]
                    .into_iter()
                    .filter(|r| r.len > 0)
                    .collect()
            }
            None => vec![run(len, self.style.color, None)],
        }
    }

    fn font_size(window: &Window) -> Pixels {
        window.text_style().font_size.to_pixels(window.rem_size())
    }

    fn shape(text: &SharedString, runs: &[TextRun], font_size: Pixels, width: Option<Pixels>, window: &Window) -> Vec<WrappedLine> {
        window
            .text_system()
            .shape_text(text.clone(), font_size, runs, width, None)
            .map(|l| l.into_vec())
            .unwrap_or_default()
    }
}

/// The visual rows of `lines`, with a caret stop on every char boundary.
fn visual_rows(lines: &[WrappedLine]) -> Vec<VRow> {
    let mut rows = vec![];
    let mut base = 0;
    for line in lines {
        let layout = &line.unwrapped_layout;
        let text = line.text.as_ref();
        let mut starts = vec![0];
        for b in line.wrap_boundaries() {
            starts.push(layout.runs[b.run_ix].glyphs[b.glyph_ix].index);
        }
        for (k, &s) in starts.iter().enumerate() {
            let e = starts.get(k + 1).copied().unwrap_or(text.len());
            let x0 = f32::from(layout.x_for_index(s));
            let stops = text[s..e]
                .char_indices()
                .map(|(j, _)| s + j)
                .chain([e])
                .map(|i| (base + i, f32::from(layout.x_for_index(i)) - x0))
                .collect();
            rows.push(VRow { start: base + s, end: base + e, last: k + 1 == starts.len(), stops });
        }
        base += text.len() + 1;
    }
    rows
}

fn hit(shared: &Shared, local: gpui::Point<Pixels>, line_height: Pixels) -> usize {
    let n = shared.rows.len();
    if n == 0 {
        return 0;
    }
    let r = ((local.y / line_height).floor() as isize + shared.scroll as isize).clamp(0, n as isize - 1) as usize;
    shared.rows[r].index_at(f32::from(local.x))
}

impl Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let (text, runs) = (self.text.clone(), self.runs(window));
        let (lh, max_rows) = (self.style.line_height, self.style.max_rows);
        // the measure runs later, outside this element's text style
        let font_size = Self::font_size(window);
        let id = window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                gpui::AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let lines = Self::shape(&text, &runs, font_size, width, window);
            let rows: usize = lines.iter().map(|l| l.wrap_boundaries().len() + 1).sum::<usize>().max(1);
            let rows = max_rows.map_or(rows, |m| rows.min(m));
            let w = width.unwrap_or_else(|| lines.iter().map(|l| l.width()).fold(px(0.), |a, b| a.max(b)));
            size(w, lh * rows as f32)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _cx: &mut App,
    ) -> Prepainted {
        let lh = self.style.line_height;
        let font_size = Self::font_size(window);
        let wrapped = Self::shape(&self.text, &self.runs(window), font_size, Some(bounds.size.width), window);
        let rows = visual_rows(&wrapped);
        let mut sh = self.shared.borrow_mut();
        sh.rev = self.rev;
        sh.rows = rows;
        let cur = sh.row_of(self.cursor);
        // keep the caret's row in view when the caret moved or the text changed;
        // otherwise the scroll wheel's offset stands
        let visible = self.style.max_rows.unwrap_or(usize::MAX);
        if sh.fitted != Some((self.cursor, self.rev)) {
            sh.fitted = Some((self.cursor, self.rev));
            if cur < sh.scroll {
                sh.scroll = cur;
            } else if cur >= sh.scroll.saturating_add(visible) {
                sh.scroll = cur + 1 - visible;
            }
        }
        sh.scroll = sh.scroll.min(sh.rows.len().saturating_sub(visible));
        let scroll = sh.scroll;
        let top = |r: usize| bounds.top() + lh * (r as f32 - scroll as f32);

        let mut lines = vec![];
        let mut first_row = 0;
        for line in wrapped {
            let n = line.wrap_boundaries().len() + 1;
            lines.push((first_row, line));
            first_row += n;
        }

        let sel = &self.selection;
        let mut quads = vec![];
        if !sel.is_empty() {
            for (r, row) in sh.rows.iter().enumerate() {
                let lo = sel.start.max(row.start);
                let hi = sel.end.min(row.end);
                // a selected newline shows as a sliver past the line's end
                let newline = row.last && sel.start <= row.end && sel.end > row.end;
                if lo < hi || newline {
                    let x0 = row.x(lo.min(hi));
                    let x1 = row.x(hi) + if newline { 4. } else { 0. };
                    quads.push(fill(
                        Bounds::new(point(bounds.left() + px(x0), top(r)), size(px(x1 - x0), lh)),
                        self.style.selection,
                    ));
                }
            }
        }
        let caret = self.style.caret.filter(|_| sel.is_empty()).map(|color| {
            let h = (font_size * 1.15).min(lh);
            let x = sh.rows.get(cur).map(|r| r.x(self.cursor)).unwrap_or(0.);
            fill(
                Bounds::new(point(bounds.left() + px(x), top(cur) + (lh - h) / 2.), size(px(1.5), h)),
                color,
            )
        });
        drop(sh);

        let placeholder = (self.text.is_empty() && !self.style.placeholder.is_empty())
            .then(|| {
                let style = window.text_style();
                let run = TextRun {
                    len: self.style.placeholder.len(),
                    font: style.font(),
                    color: self.style.placeholder_color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                Self::shape(&self.style.placeholder, &[run], font_size, None, window).into_iter().next()
            })
            .flatten();

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Prepainted { lines, placeholder, quads, caret, hitbox }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        p: &mut Prepainted,
        window: &mut Window,
        cx: &mut App,
    ) {
        let lh = self.style.line_height;
        window.set_cursor_style(CursorStyle::IBeam, &p.hitbox);
        let scroll = self.shared.borrow().scroll;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for q in p.quads.drain(..) {
                window.paint_quad(q);
            }
            for (row, line) in &p.lines {
                let origin = point(bounds.left(), bounds.top() + lh * (*row as f32 - scroll as f32));
                line.paint(origin, lh, TextAlign::Left, None, window, cx).ok();
            }
            if let Some(ph) = &p.placeholder {
                ph.paint(point(bounds.left() + px(4.), bounds.top()), lh, TextAlign::Left, None, window, cx).ok();
            }
            if let Some(c) = p.caret.take() {
                window.paint_quad(c);
            }
        });

        let origin = bounds.origin;
        {
            let (shared, on_mouse, hitbox) = (self.shared.clone(), self.on_mouse.clone(), p.hitbox.clone());
            window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left || !hitbox.is_hovered(window) {
                    return;
                }
                let index = hit(&shared.borrow(), ev.position - origin, lh);
                cx.stop_propagation();
                on_mouse(FieldMouse::Down { index, clicks: ev.click_count, shift: ev.modifiers.shift }, window, cx);
            });
        }
        {
            let (shared, on_mouse) = (self.shared.clone(), self.on_mouse.clone());
            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || ev.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let index = {
                    let sh = shared.borrow();
                    if !sh.dragging {
                        return;
                    }
                    hit(&sh, ev.position - origin, lh)
                };
                on_mouse(FieldMouse::Drag { index }, window, cx);
            });
        }
        {
            let (shared, on_mouse) = (self.shared.clone(), self.on_mouse.clone());
            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && shared.borrow().dragging {
                    on_mouse(FieldMouse::Up, window, cx);
                }
            });
        }
        if let Some(max) = self.style.max_rows {
            let (shared, hitbox) = (self.shared.clone(), p.hitbox.clone());
            window.on_mouse_event(move |ev: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let mut sh = shared.borrow_mut();
                let most = sh.rows.len().saturating_sub(max);
                if most == 0 {
                    return;
                }
                sh.wheel -= f32::from(ev.delta.pixel_delta(lh).y);
                let steps = (sh.wheel / f32::from(lh)).trunc();
                sh.wheel -= steps * f32::from(lh);
                sh.scroll = (sh.scroll as f32 + steps).clamp(0., most as f32) as usize;
                cx.stop_propagation();
                window.refresh();
            });
        }
    }
}
