//! The terminal canvas: pane cards, cells, splits, and what's painted over them
//! (hints, PaneSelect labels, copy mode), plus the native views placed in panes.

use super::widgets::{term_family, with_alpha};
use super::Theme;
use crate::app::{GridGeom, TermApp};
use crate::render::{self, PaintOpts};
use config::DimensionContext;
use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, Context, FontWeight, Hsla, Pixels, SharedString, Window,
};
use wezterm_term::TerminalSize;

impl TermApp {
    /// Native views, each placed over its placeholder pane's cells. Uses the
    /// grid geometry the canvas recorded last frame.
    pub(super) fn pane_views(&mut self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(tab) = self.active_tab() else {
            return Vec::new();
        };
        let g = self.grid.get();
        let area = point(g.origin.x - g.pad.x, g.origin.y - g.pad.y);
        let metrics = render::metrics(window, &config::configuration(), self.font_scale);
        let cards = crate::settings::get().pane_cards;
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
                        .when(cards, |d| d.rounded(g.card_radius() - px(1.)))
                        .overflow_hidden()
                        .child(view.render(&vcx, t, cx))
                        .into_any_element(),
                )
            })
            .collect()
    }

    pub(super) fn terminal_canvas(
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
        let cards = crate::settings::get().pane_cards;
        let split_idle = gpui::Hsla { a: 0.10, ..fg };
        let t = crate::ui::Theme::new(palette, config.window_background_opacity);
        let gap_color = t.chrome;
        let card_style = render::CardStyle {
            fill: t.term_bg,
            // inactive cards recede into the sidebar / top bar color
            inactive_fill: t.chrome,
            // no edge on inactive cards; a faint one marks the active card
            border: gpui::transparent_black(),
            active_border: with_alpha(t.fg, 0.10),
            radius: px(0.), // from the grid, at paint time
        };
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
                let geom = GridGeom {
                    origin,
                    pad: point(pl, pt),
                    size: bounds.size,
                    cell_w: metrics.cell_w,
                    line_h: metrics.line_h,
                    cols,
                    rows,
                };
                grid.set(geom);

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
                Some((tab, origin, splits, geom))
            },
            move |bounds, state, window, cx| {
                let Some((tab, origin, splits, geom)) = state else { return };
                let pane_cards: Vec<_> = if cards {
                    tab.iter_panes()
                        .iter()
                        .map(|p| render::Card {
                            bounds: geom.card(p.left, p.top, p.width, p.height),
                            active: p.is_active,
                        })
                        .collect()
                } else {
                    Vec::new()
                };
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
                let card_style = render::CardStyle { radius: geom.card_radius(), ..card_style };
                if cards {
                    render::paint_gaps(bounds, &pane_cards, card_style.radius, gap_color, window);
                }
                render::paint_tab(origin, &pane_cards, &card_style, &panes, &painted, &paint_metrics, &opts, window, cx);
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

    /// Copy mode's search field, while typing after / or ?.
    pub(super) fn copy_search(&self, t: &Theme) -> Option<impl IntoElement> {
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
}

/// Copy mode / mouse selection: the selection, and copy mode's cursor, over one pane.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_copy_mode(
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
pub(super) fn paint_pane_label(
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

/// QuickSelect overlay: tint each match, label at its start.
pub(super) fn paint_hints(
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
