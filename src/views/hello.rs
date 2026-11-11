//! The smallest view: proves out layout, focus and key routing. Shows its own
//! size so resizing and splitting are easy to check.

use super::{PaneView, ViewCx};
use crate::ui::Theme;
use gpui::{div, prelude::*, px, AnyElement, App, FontWeight, Keystroke};

pub struct Hello {
    name: String,
    last_key: Option<String>,
    keys: usize,
}

impl Hello {
    pub fn new(args: &str) -> Self {
        Self {
            name: if args.is_empty() { "world".into() } else { args.into() },
            last_key: None,
            keys: 0,
        }
    }
}

impl PaneView for Hello {
    fn render(&mut self, vcx: &ViewCx, t: &Theme, _cx: &mut App) -> AnyElement {
        let mono = crate::ui::term_family();
        let line = |s: String| {
            div()
                .font_family(mono.clone())
                .text_size(px(11.))
                .text_color(t.fg_subtle)
                .child(s)
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if vcx.focused { t.fg } else { t.fg_muted })
                    .child(format!("Hello, {}", self.name)),
            )
            .child(line(format!(
                "{}×{} cells · {:.0}×{:.0} px",
                vcx.cols,
                vcx.rows,
                f32::from(vcx.size.width),
                f32::from(vcx.size.height),
            )))
            .child(line(format!("pane {}", vcx.pane_id)))
            .children(
                self.last_key
                    .as_ref()
                    .map(|k| line(format!("{k} · {}", self.keys))),
            )
            .into_any_element()
    }

    fn key(&mut self, ks: &Keystroke, _cx: &mut App) -> bool {
        self.last_key = Some(ks.unparse());
        self.keys += 1;
        true
    }
}
