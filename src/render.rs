//! Paints a WezTerm tab (its split tree of panes) into a gpui canvas.
//! Deliberately simple: one shaped text run per same-styled span of cells,
//! positioned on the cell grid so ligature/nerd-font advances can't drift.

use config::ConfigHandle;
use gpui::{
    fill, point, px, size, App, Bounds, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight,
    Hsla, Pixels, Point, Rgba, StrikethroughStyle, TextRun, UnderlineStyle, Window,
};
use mux::pane::Pane;
use mux::renderable::StableCursorPosition;
use mux::tab::{PositionedSplit, SplitDirection};
use std::sync::Arc;
use termwiz::surface::{CursorShape, CursorVisibility};
use wezterm_term::color::{ColorAttribute, ColorPalette, SrgbaTuple};
use wezterm_term::{Intensity, Line, StableRowIndex, Underline};

#[derive(Clone)]
pub struct Metrics {
    pub font: Font,
    pub font_size: Pixels,
    pub cell_w: Pixels,
    pub line_h: Pixels,
}

pub fn metrics(window: &Window, config: &ConfigHandle, font_scale: f32) -> Metrics {
    let families: Vec<_> = config.font.font.iter().map(|f| f.family.clone()).collect();
    let primary = config.font.font.first();
    let font = Font {
        family: families
            .first()
            .cloned()
            .unwrap_or_else(|| "Menlo".into())
            .into(),
        features: FontFeatures::default(),
        fallbacks: (families.len() > 1).then(|| {
            let mut rest = families[1..].to_vec();
            rest.push("Menlo".into());
            FontFallbacks::from_fonts(rest)
        }),
        weight: FontWeight(
            primary
                .map(|f| f.weight.to_opentype_weight() as f32)
                .unwrap_or(400.0),
        ),
        style: FontStyle::Normal,
    };
    let font_size = px(config.font_size as f32 * font_scale);
    let ts = window.text_system();
    let id = ts.resolve_font(&font);
    let cell_w = ts
        .advance(id, font_size, 'M')
        .map(|s| s.width)
        .unwrap_or(font_size * 0.6)
        * config.cell_width as f32;
    let line_h = (ts.ascent(id, font_size) + ts.descent(id, font_size).abs())
        * config.line_height as f32;
    Metrics {
        font,
        font_size,
        cell_w,
        line_h: px(f32::from(line_h).ceil()),
    }
}

pub fn color(c: SrgbaTuple) -> Hsla {
    Rgba {
        r: c.0,
        g: c.1,
        b: c.2,
        a: c.3,
    }
    .into()
}

/// Everything needed to paint one pane, captured during render.
pub struct PaneSnap {
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub height: usize,
    pub is_active: bool,
    pub lines: Vec<Line>,
    pub cursor: Option<StableCursorPosition>,
    pub first_row: StableRowIndex,
    pub palette: ColorPalette,
}

pub fn snapshot(
    pos: &mux::tab::PositionedPane,
    viewport_top: Option<StableRowIndex>,
) -> PaneSnap {
    let pane: &Arc<dyn Pane> = &pos.pane;
    let dims = pane.get_dimensions();
    let top = viewport_top.unwrap_or(dims.physical_top);
    let (first_row, lines) = pane.get_lines(top..top + pos.height as StableRowIndex);
    let cursor = pane.get_cursor_position();
    PaneSnap {
        left: pos.left,
        top: pos.top,
        width: pos.width,
        height: pos.height,
        is_active: pos.is_active,
        lines,
        cursor: (cursor.visibility == CursorVisibility::Visible).then_some(cursor),
        first_row,
        palette: pane.palette(),
    }
}

pub struct PaintOpts {
    pub inactive_brightness: f32,
    pub default_cursor_bar: bool,
    pub focused: bool,
}

/// A pane's card: its bounds, whether it's the active pane, and how it's drawn.
pub struct Card {
    pub bounds: Bounds<Pixels>,
    pub active: bool,
}

#[derive(Clone, Copy)]
pub struct CardStyle {
    pub fill: Hsla,
    pub inactive_fill: Hsla,
    pub border: Hsla,
    pub active_border: Hsla,
    pub radius: Pixels,
}

/// The chrome between and around the cards: `area` with a hole for each card.
/// Painting it under the cards instead would stack two translucent layers and
/// leave the panes nearly opaque; this way every pixel is painted once. The
/// holes are a hair smaller than the cards so no seam shows under the border.
pub fn paint_gaps(area: Bounds<Pixels>, cards: &[Card], radius: Pixels, color: Hsla, window: &mut Window) {
    let mut b = gpui::PathBuilder::fill();
    let (o, s) = (area.origin, area.size);
    b.move_to(o);
    b.line_to(point(o.x + s.width, o.y));
    b.line_to(point(o.x + s.width, o.y + s.height));
    b.line_to(point(o.x, o.y + s.height));
    b.close();
    // lyon fills even-odd by default and cards never overlap: each card is a hole
    for c in cards {
        let k = px(0.5);
        let (x0, y0) = (c.bounds.origin.x + k, c.bounds.origin.y + k);
        let (x1, y1) = (c.bounds.origin.x + c.bounds.size.width - k, c.bounds.origin.y + c.bounds.size.height - k);
        let r = (radius - k).max(px(0.)).min((x1 - x0) / 2.).min((y1 - y0) / 2.);
        let rr = point(r, r);
        b.move_to(point(x0 + r, y0));
        b.line_to(point(x1 - r, y0));
        b.arc_to(rr, px(0.), false, true, point(x1, y0 + r));
        b.line_to(point(x1, y1 - r));
        b.arc_to(rr, px(0.), false, true, point(x1 - r, y1));
        b.line_to(point(x0 + r, y1));
        b.arc_to(rr, px(0.), false, true, point(x0, y1 - r));
        b.line_to(point(x0, y0 + r));
        b.arc_to(rr, px(0.), false, true, point(x0 + r, y0));
        b.close();
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn paint_tab(
    origin: Point<Pixels>,
    cards: &[Card],
    card_style: &CardStyle,
    panes: &[PaneSnap],
    splits: &[(PositionedSplit, Hsla, bool)],
    m: &Metrics,
    opts: &PaintOpts,
    window: &mut Window,
    cx: &mut App,
) {
    for c in cards {
        window.paint_quad(gpui::quad(
            c.bounds,
            card_style.radius,
            if c.active { card_style.fill } else { card_style.inactive_fill },
            px(1.),
            if c.active { card_style.active_border } else { card_style.border },
            gpui::BorderStyle::Solid,
        ));
    }
    for p in panes {
        paint_pane(origin, p, m, opts, window, cx);
    }
    for (s, color, hot) in splits {
        let strip = split_strip(origin, s, m);
        if !cards.is_empty() {
            // The gap between cards is the divider; the resize cursor is its
            // only hover state.
            continue;
        }
        // Splits occupy one cell between panes; draw a line through its middle,
        // thicker while hovered / dragged.
        let w = if *hot { px(2.0) } else { px(1.0) };
        let b = match s.direction {
            SplitDirection::Horizontal => Bounds::new(
                point(strip.center().x - w / 2.0, strip.origin.y),
                size(w, strip.size.height),
            ),
            SplitDirection::Vertical => Bounds::new(
                point(strip.origin.x, strip.center().y - w / 2.0),
                size(strip.size.width, w),
            ),
        };
        window.paint_quad(fill(b, *color));
    }
}

/// The one-cell strip a split occupies, in window coordinates.
pub fn split_strip(origin: Point<Pixels>, s: &PositionedSplit, m: &Metrics) -> Bounds<Pixels> {
    let o = point(
        origin.x + m.cell_w * s.left as f32,
        origin.y + m.line_h * s.top as f32,
    );
    match s.direction {
        SplitDirection::Horizontal => Bounds::new(o, size(m.cell_w, m.line_h * s.size as f32)),
        SplitDirection::Vertical => Bounds::new(o, size(m.cell_w * s.size as f32, m.line_h)),
    }
}

#[derive(PartialEq, Clone, Copy)]
struct Style {
    fg: SrgbaTuple,
    bg: Option<SrgbaTuple>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

fn paint_pane(
    origin: Point<Pixels>,
    p: &PaneSnap,
    m: &Metrics,
    opts: &PaintOpts,
    window: &mut Window,
    cx: &mut App,
) {
    let dim = |c: SrgbaTuple| -> SrgbaTuple {
        if p.is_active {
            c
        } else {
            let b = opts.inactive_brightness;
            SrgbaTuple(c.0 * b, c.1 * b, c.2 * b, c.3)
        }
    };
    let pane_origin = point(
        origin.x + m.cell_w * p.left as f32,
        origin.y + m.line_h * p.top as f32,
    );

    for (row, line) in p.lines.iter().enumerate() {
        let y = pane_origin.y + m.line_h * row as f32;
        // (start_col, cols, text, style)
        let mut spans: Vec<(usize, usize, String, Style)> = vec![];
        for cell in line.visible_cells() {
            let col = cell.cell_index();
            if col >= p.width {
                break;
            }
            let a = cell.attrs();
            let (mut fg_attr, mut bg_attr) = (a.foreground(), a.background());
            if a.reverse() {
                std::mem::swap(&mut fg_attr, &mut bg_attr);
            }
            let fg = if a.reverse() && fg_attr == ColorAttribute::Default {
                p.palette.background
            } else {
                p.palette.resolve_fg(fg_attr)
            };
            let bg = if bg_attr == ColorAttribute::Default && !a.reverse() {
                None
            } else if a.reverse() && bg_attr == ColorAttribute::Default {
                Some(p.palette.foreground)
            } else {
                Some(p.palette.resolve_bg(bg_attr))
            };
            let style = Style {
                fg: dim(fg),
                bg: bg.map(dim),
                bold: a.intensity() == Intensity::Bold,
                italic: a.italic(),
                underline: a.underline() != Underline::None,
                strike: a.strikethrough(),
            };
            let text = if a.invisible() { " " } else { cell.str() };
            let w = cell.width().max(1);
            // Wide glyphs get their own span so the forced grid advance doesn't squash them.
            match spans.last_mut() {
                Some(last) if last.3 == style && last.0 + last.1 == col && w == 1 && last.1 > 0 && !last.2.is_empty() && last_is_narrow(last) => {
                    last.1 += 1;
                    last.2.push_str(text);
                }
                _ => spans.push((col, w, text.to_string(), style)),
            }
        }

        for (col, cols, text, style) in spans {
            let x = pane_origin.x + m.cell_w * col as f32;
            if let Some(bg) = style.bg {
                window.paint_quad(fill(
                    Bounds::new(point(x, y), size(m.cell_w * cols as f32, m.line_h)),
                    color(bg),
                ));
            }
            if text.trim().is_empty() && !style.underline && !style.strike {
                continue;
            }
            let mut font = m.font.clone();
            if style.bold {
                font.weight = FontWeight::BOLD;
            }
            if style.italic {
                font.style = FontStyle::Italic;
            }
            let fg = color(style.fg);
            let run = TextRun {
                len: text.len(),
                font,
                color: fg,
                background_color: None,
                underline: style.underline.then_some(UnderlineStyle {
                    color: Some(fg),
                    thickness: px(1.0),
                    wavy: false,
                }),
                strikethrough: style.strike.then_some(StrikethroughStyle {
                    color: Some(fg),
                    thickness: px(1.0),
                }),
            };
            let narrow = text.chars().count() == cols;
            let shaped = window.text_system().shape_line(
                text.into(),
                m.font_size,
                &[run],
                narrow.then_some(m.cell_w),
            );
            shaped.paint(point(x, y), m.line_h, window, cx).ok();
        }
    }

    // Cursor
    if let Some(c) = &p.cursor {
        let row = c.y - p.first_row;
        if row >= 0 && (row as usize) < p.height && c.x < p.width {
            let x = pane_origin.x + m.cell_w * c.x as f32;
            let y = pane_origin.y + m.line_h * row as f32;
            let cur = color(p.palette.cursor_bg);
            let bar = match c.shape {
                CursorShape::Default => opts.default_cursor_bar,
                CursorShape::BlinkingBar | CursorShape::SteadyBar => true,
                _ => false,
            };
            let underline = matches!(
                c.shape,
                CursorShape::BlinkingUnderline | CursorShape::SteadyUnderline
            );
            let b = if !p.is_active || !opts.focused {
                None
            } else if bar {
                Some(Bounds::new(point(x, y), size(px(2.0), m.line_h)))
            } else if underline {
                Some(Bounds::new(
                    point(x, y + m.line_h - px(2.0)),
                    size(m.cell_w, px(2.0)),
                ))
            } else {
                Some(Bounds::new(point(x, y), size(m.cell_w, m.line_h)))
            };
            let is_block = !bar && !underline;
            match b {
                Some(b) => {
                    window.paint_quad(fill(b, cur));
                    // Re-draw the glyph under a block cursor in cursor_fg so it stays readable.
                    if is_block {
                        if let Some(cell) = p.lines.get(row as usize).and_then(|l| {
                            l.visible_cells().find(|cell| cell.cell_index() == c.x)
                        })
                        {
                            let text = cell.str().to_string();
                            if !text.trim().is_empty() {
                                let run = TextRun {
                                    len: text.len(),
                                    font: m.font.clone(),
                                    color: color(p.palette.cursor_fg),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                };
                                window
                                    .text_system()
                                    .shape_line(text.into(), m.font_size, &[run], None)
                                    .paint(point(x, y), m.line_h, window, cx)
                                    .ok();
                            }
                        }
                    }
                }
                None if p.is_active => window.paint_quad(gpui::outline(
                    Bounds::new(point(x, y), size(m.cell_w, m.line_h)),
                    cur,
                    gpui::BorderStyle::Solid,
                )),
                None => {}
            }
        }
    }
}

fn last_is_narrow(span: &(usize, usize, String, Style)) -> bool {
    span.2.chars().count() == span.1
}
