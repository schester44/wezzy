//! Native panes: gpui views that live in the terminal layout.
//!
//! The session server owns the layout, so a view still occupies a real server
//! pane. That pane runs a placeholder (`sh` → `cat`) which sets its title and
//! the `WEZZY_VIEW` user var, then waits. Wherever the app sees that user var it
//! skips the pane's cells and draws the view in the pane's rectangle instead.
//! Splits, tabs, zoom, swaps, workspaces and restarts all treat it as an
//! ordinary pane; closing the pane ends the placeholder and drops the view.

mod editor;
mod hello;
pub mod sql;

use crate::ui::Theme;
use config::keyassignment::{SpawnCommand, SpawnTabDomain};
use gpui::{AnyElement, App, Keystroke, Modifiers, Pixels, Point, Size};
use mux::pane::{Pane, PaneId};
use mux::Mux;
use std::collections::HashMap;
use std::sync::Arc;

/// User var naming the view: `"<kind>"` or `"<kind> <args>"`.
pub const USER_VAR: &str = "WEZZY_VIEW";

/// What a view gets to know when it draws.
pub struct ViewCx {
    pub pane_id: PaneId,
    /// pane size in cells (what the server sees)
    pub cols: usize,
    pub rows: usize,
    /// the pane's rectangle
    pub size: Size<Pixels>,
    /// this is the active pane and the terminal region has focus
    pub focused: bool,
    /// the terminal font and its cell size
    pub metrics: crate::render::Metrics,
}

pub trait PaneView {
    fn render(&mut self, vcx: &ViewCx, t: &Theme, cx: &mut App) -> AnyElement;
    /// A key the keymap didn't claim. `true` if the view used it.
    fn key(&mut self, ks: &Keystroke, cx: &mut App) -> bool;
    /// Keys the view takes before the keymap (⌘↵ in the SQL editor).
    fn claims(&self, _ks: &Keystroke) -> bool {
        false
    }
    /// A click at `pos`, relative to the view's top-left.
    fn mouse_down(&mut self, _pos: Point<Pixels>, _mods: &Modifiers, _clicks: usize, _cx: &mut App) {}
    /// The wheel over the view: `lines` up (positive) or down.
    fn scroll(&mut self, _pos: Point<Pixels>, _lines: isize) {}
    /// ⌘V
    fn paste(&mut self, _text: &str) {}
    /// ⌘C: the text to copy, if anything is selected.
    fn copy(&mut self) -> Option<String> {
        None
    }
}

/// Build a view from its user var, or `None` for an unknown kind.
fn create(spec: &str) -> Option<Box<dyn PaneView>> {
    let (kind, args) = spec.split_once(' ').unwrap_or((spec, ""));
    match kind {
        "hello" => Some(Box::new(hello::Hello::new(args))),
        "sql" => Some(Box::new(sql::SqlView::new(args))),
        _ => None,
    }
}

/// The pane's view spec, if it hosts one.
pub fn spec(pane: &Arc<dyn Pane>) -> Option<String> {
    pane.copy_user_vars().get(USER_VAR).cloned()
}

pub fn is_view(pane: &Arc<dyn Pane>) -> bool {
    spec(pane).is_some()
}

/// The placeholder that holds a view's slot on the server. Positional args
/// keep `title` and the spec out of the shell's parser.
pub fn spawn_command(kind: &str, args: &str, title: &str) -> SpawnCommand {
    use base64::Engine;
    let spec = if args.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {args}")
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(spec);
    let script = format!(
        "stty -echo 2>/dev/null; \
         printf '\\033[?25l\\033]2;%s\\007\\033]1337;SetUserVar={USER_VAR}=%s\\007' \"$1\" \"$2\"; \
         exec cat >/dev/null"
    );
    SpawnCommand {
        args: Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            script,
            "wezzy-view".into(),
            title.into(),
            encoded,
        ]),
        domain: SpawnTabDomain::DefaultDomain,
        ..Default::default()
    }
}

/// Live views, keyed by local pane id.
#[derive(Default)]
pub struct Views {
    views: HashMap<PaneId, (String, Box<dyn PaneView>)>,
}

impl Views {
    /// Create views for panes that announced one; drop those whose pane closed
    /// (or whose spec changed).
    pub fn sync(&mut self) {
        let Some(mux) = Mux::try_get() else { return };
        let mut live = HashMap::new();
        for pane in mux.iter_panes() {
            if let Some(s) = spec(&pane) {
                live.insert(pane.pane_id(), s);
            }
        }
        self.views
            .retain(|id, (s, _)| live.get(id).is_some_and(|l| l == s));
        for (id, s) in live {
            if self.views.contains_key(&id) {
                continue;
            }
            match create(&s) {
                Some(v) => {
                    self.views.insert(id, (s, v));
                }
                None => log::warn!("pane {id}: unknown view {s:?}"),
            }
        }
    }

    pub fn get_mut(&mut self, pane_id: PaneId) -> Option<&mut Box<dyn PaneView>> {
        self.views.get_mut(&pane_id).map(|(_, v)| v)
    }
}
