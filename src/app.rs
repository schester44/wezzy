//! The gpui view: sidebar + terminal area, input routing, and the
//! KeyAssignment interpreter that drives WezTerm's mux.

use crate::keys::{self, KeyState, Lookup};
use crate::luawin;
use config::keyassignment::{
    KeyAssignment, PaneDirection, RotationDirection, SpawnCommand, SpawnTabDomain,
    SplitSize as CfgSplitSize,
};
use config::TermConfig;
use gpui::{
    point, prelude::*, px, size, App, Bounds, ClipboardItem, Context, FocusHandle, KeyDownEvent,
    MouseDownEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions,
};
use mux::domain::SplitSource;
use mux::pane::{Pane, PaneId};
use mux::tab::{PaneNode, SplitDirection, SplitRequest, SplitSize, Tab, TabId};
use mux::window::WindowId;
use mux::{Mux, MuxNotification};
use parking_lot::Mutex;
use portable_pty::CommandBuilder;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use wezterm_term::{StableRowIndex, TerminalSize};

// ---------------------------------------------------------------------------
// Cross-thread plumbing: mux notifications and Lua callbacks post here, the
// view drains it on the gpui foreground executor.

pub enum AppEvent {
    MuxChanged,
    WindowReady(WindowId),
    ConfigReloaded,
    Perform {
        pane_id: Option<PaneId>,
        action: KeyAssignment,
    },
    Clipboard(String),
    Flash(String),
    /// Something off the mux changed (git status, ...); just repaint.
    Redraw,
    RebuildFinished {
        ok: bool,
        dir: std::path::PathBuf,
    },
    UserVar {
        pane_id: PaneId,
        name: String,
        value: String,
    },
    AgentAlert(PaneId),
    /// PaneSelect swapped two panes; `focus` is the pane to make active.
    PanesSwapped {
        tab: TabId,
        focus: PaneId,
    },
    /// A tab landed somewhere else (Move Tab to Workspace, PaneSelect's moves).
    TabMoved {
        window: WindowId,
        tab: TabId,
        follow: bool,
    },
    /// A diff-panel commit finished: Ok(summary) or Err((error, message)).
    Committed {
        result: Result<String, (String, String)>,
        amend: bool,
    },
    AgentStatusChanged {
        pane_id: PaneId,
        status: crate::hooks::HookStatus,
        detail: Option<String>,
    },
    /// A macOS notification was clicked (notify.rs); the session server's pane id.
    NotificationClicked(PaneId),
    /// The git panel's pull request flow (`pr.rs`).
    PullRequest(crate::pr::PrEvent),
}

static EVENTS: OnceLock<smol::channel::Sender<AppEvent>> = OnceLock::new();
static MUX_DIRTY: AtomicBool = AtomicBool::new(false);

pub fn send_event(ev: AppEvent) {
    // Pane output can arrive thousands of times a second; coalesce redraw requests.
    if matches!(ev, AppEvent::MuxChanged) && MUX_DIRTY.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(tx) = EVENTS.get() {
        tx.try_send(ev).ok();
    }
}

/// Bits of UI state the Lua `window` object can read or write.
pub struct UiState {
    pub active_key_table: Option<String>,
    pub leader_active: bool,
    pub left_status: String,
    pub right_status: String,
}

pub static UI_STATE: Mutex<UiState> = Mutex::new(UiState {
    active_key_table: None,
    leader_active: false,
    left_status: String::new(),
    right_status: String::new(),
});

// ---------------------------------------------------------------------------

gpui::actions!(wezzy, [Quit, Hide, OpenSettings, ShowPalette]);

/// macOS menu bar. No key equivalents: keys go through the user's bindings.
fn install_menus(cx: &mut App) {
    use gpui::{Menu, MenuItem, SystemMenuType};
    // through the view so it asks first, like ⌘Q
    cx.on_action(|_: &Quit, _| {
        send_event(AppEvent::Perform {
            pane_id: None,
            action: KeyAssignment::QuitApplication,
        })
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.set_menus(vec![Menu {
        name: "Wezzy".into(),
        items: vec![
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::action("Command Palette", ShowPalette),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Wezzy", Hide),
            MenuItem::action("Quit Wezzy", Quit),
        ],
    }]);
}

/// Dev builds aren't app bundles, so macOS shows a generic exec icon in the
/// Dock. Give them the real one (Wezzy.app gets it from its .icns).
fn set_dock_icon() {
    let in_bundle = std::env::current_exe()
        .ok()
        .is_some_and(|e| e.ancestors().any(|p| p.extension().is_some_and(|x| x == "app")));
    if in_bundle {
        return;
    }
    const PNG: &[u8] = include_bytes!("../assets/icon/wezzy-1024.png");
    #[allow(unexpected_cfgs)]
    unsafe {
        use objc::runtime::Object;
        use objc::{class, msg_send, sel, sel_impl};
        let data: *mut Object = msg_send![class!(NSData), dataWithBytes: PNG.as_ptr() length: PNG.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return;
        }
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![app, setApplicationIconImage: image];
    }
}

pub fn open_main_window(cx: &mut App) {
    set_dock_icon();
    install_menus(cx);
    let config = config::configuration();
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1280.), px(820.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Wezzy".into()),
            appears_transparent: true,
            // vertically centered in the 40px top bar (CHROME_H)
            traffic_light_position: Some(point(px(16.), px(14.))),
        }),
        window_background: if config.window_background_opacity < 1.0 {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Opaque
        },
        ..Default::default()
    };
    cx.open_window(opts, |window, cx| cx.new(|cx| TermApp::new(window, cx)))
        .expect("open window");
    cx.on_window_closed(|cx| cx.quit()).detach();
    cx.activate(true);
}

/// What a prompt does with the submitted line.
#[derive(Clone)]
pub(crate) enum PromptTarget {
    /// A config `PromptInputLine`: an `action_callback` (EmitEvent) or a plain action.
    Action(KeyAssignment),
    RenameTab(mux::tab::TabId),
    /// the workspace's current name
    RenameWorkspace(String),
}

pub(crate) enum Overlay {
    Prompt {
        description: String,
        prompt: String,
        input: String,
        target: PromptTarget,
        pane_id: PaneId,
    },
    Confirm {
        message: String,
        detail: String,
        /// label of the confirming button
        verb: &'static str,
        action: KeyAssignment,
        cancel: Option<KeyAssignment>,
        pane_id: PaneId,
    },
    Palette(crate::palette::PaletteState),
    Settings(crate::settings_ui::SettingsState),
    /// The global shortcut sheet (Leader ?); any key or click closes it.
    Shortcuts,
}

/// Where the terminal grid was last painted, for mouse hit-testing.
#[derive(Clone, Copy, Default)]
pub(crate) struct GridGeom {
    pub origin: Point<Pixels>,
    /// `origin` relative to the terminal area (the window padding)
    pub pad: Point<Pixels>,
    pub cell_w: Pixels,
    pub line_h: Pixels,
}

pub struct TermApp {
    pub(crate) focus: FocusHandle,
    pub(crate) mux_window_id: Option<WindowId>,
    pub(crate) keys: KeyState,
    pub(crate) viewports: HashMap<PaneId, StableRowIndex>,
    pub(crate) font_scale: f32,
    /// top-bar tab under the mouse; shows its close button
    pub(crate) hovered_tab: Option<usize>,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) flash: Option<(String, Instant)>,
    /// The agent pane a toast is about; clicking the toast goes there.
    pub(crate) flash_pane: Option<PaneId>,
    pub(crate) grid: Rc<Cell<GridGeom>>,
    pub(crate) sidebar_open: bool,
    pub(crate) ws_menu_open: bool,
    /// Inline "new workspace" name field inside the workspace menu.
    pub(crate) ws_new: Option<String>,
    /// Inline name field on a sidebar agent row.
    pub(crate) agent_rename: Option<crate::sidebar::AgentRename>,
    pub(crate) quick_select: Option<crate::quickselect::QuickSelect>,
    pub(crate) pane_select: Option<crate::paneselect::PaneSelect>,
    pub(crate) copy_mode: Option<crate::copymode::CopyMode>,
    /// mouse selection (selection.rs)
    pub(crate) selection: Option<crate::selection::MouseSelection>,
    pub(crate) diff_open: bool,
    /// Which part of the window has keyboard focus.
    pub(crate) region: crate::regions::Region,
    pub(crate) sidebar_cursor: usize,
    pub(crate) git: crate::gitpanel::GitPanel,
    pub(crate) diff_width: f32,
    /// agents sidebar width (⌃←/⌃→ while it's focused)
    pub(crate) sidebar_width: f32,
    /// The side panel whose edge is being dragged.
    pub(crate) panel_drag: Option<crate::ui::SidePanel>,
    /// The split divider being dragged (its `PositionedSplit::index`).
    pub(crate) split_drag: Option<usize>,
    pub(crate) ws_cursor: usize,
    /// native views hosted in placeholder panes (`views.rs`)
    pub(crate) views: crate::views::Views,
    scroll_accum: f32,
    _config_sub: config::ConfigSubscription,
}

/// Last time each pane produced output; drives the agent "working" indicator.
pub(crate) static PANE_ACTIVITY: Mutex<Option<HashMap<PaneId, Instant>>> = Mutex::new(None);

pub(crate) fn last_output(pane_id: PaneId) -> Option<Instant> {
    PANE_ACTIVITY.lock().as_ref()?.get(&pane_id).copied()
}

/// Panes that rang the bell / sent a notification and haven't been answered.
/// For agents this means "waiting on you". Cleared when you type into the pane.
pub(crate) static PANE_ALERTS: Mutex<Option<HashMap<PaneId, Instant>>> = Mutex::new(None);

pub(crate) fn has_alert(pane_id: PaneId) -> bool {
    PANE_ALERTS
        .lock()
        .as_ref()
        .is_some_and(|m| m.contains_key(&pane_id))
}

pub(crate) fn clear_alert(pane_id: PaneId) {
    if let Some(m) = PANE_ALERTS.lock().as_mut() {
        m.remove(&pane_id);
    }
}

impl TermApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);

        let (tx, rx) = smol::channel::unbounded();
        EVENTS.set(tx).ok();

        Mux::get().subscribe(|n| {
            match n {
                MuxNotification::PaneOutput(id) => {
                    PANE_ACTIVITY
                        .lock()
                        .get_or_insert_with(HashMap::new)
                        .insert(id, Instant::now());
                    send_event(AppEvent::MuxChanged);
                }
                MuxNotification::Alert {
                    pane_id,
                    alert: wezterm_term::Alert::SetUserVar { name, value },
                } => send_event(AppEvent::UserVar {
                    pane_id,
                    name,
                    value,
                }),
                MuxNotification::Alert {
                    pane_id,
                    alert:
                        wezterm_term::Alert::Bell | wezterm_term::Alert::ToastNotification { .. },
                } => {
                    PANE_ALERTS
                        .lock()
                        .get_or_insert_with(HashMap::new)
                        .insert(pane_id, Instant::now());
                    send_event(AppEvent::AgentAlert(pane_id));
                }
                MuxNotification::AssignClipboard {
                    clipboard: Some(text),
                    ..
                } => send_event(AppEvent::Clipboard(text)),
                _ => send_event(AppEvent::MuxChanged),
            }
            true
        });
        let config_sub = config::subscribe_to_config_reload(|| {
            send_event(AppEvent::ConfigReloaded);
            true
        });

        cx.spawn_in(window, async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this
                    .update_in(cx, |this, window, cx| this.on_event(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // Drive the config's `update-status` handler like WezTerm does.
        cx.spawn(async move |this, cx| loop {
            let interval = config::configuration().status_update_interval.max(100);
            cx.background_executor()
                .timer(Duration::from_millis(interval))
                .await;
            let Ok(target) = this.update(cx, |this, cx| {
                cx.notify(); // also expires leader/flash indicators
                this.mux_window_id.zip(this.active_pane().map(|p| p.pane_id()))
            }) else {
                break;
            };
            if let Some((win, pane)) = target {
                luawin::emit_window_event("update-status".into(), win, pane, None);
            }
        })
        .detach();

        let this = Self {
            focus,
            mux_window_id: None,
            keys: KeyState::new(&config::configuration()),
            viewports: HashMap::new(),
            font_scale: 1.0,
            hovered_tab: None,
            overlay: None,
            flash: None,
            flash_pane: None,
            grid: Rc::default(),
            sidebar_open: crate::settings::get().show_sidebar,
            ws_menu_open: false,
            ws_new: None,
            agent_rename: None,
            quick_select: None,
            pane_select: None,
            copy_mode: None,
            selection: None,
            diff_open: false,
            region: crate::regions::Region::Terminal,
            sidebar_cursor: 0,
            git: Default::default(),
            diff_width: 460.,
            sidebar_width: 248.,
            panel_drag: None,
            split_drag: None,
            ws_cursor: 0,
            views: Default::default(),
            scroll_accum: 0.0,
            _config_sub: config_sub,
        };
        crate::hooks::start();
        crate::notify::start();
        crate::update::start();
        crate::servers::start();
        crate::actions::start();
        crate::diff::start();
        if crate::session::persistent() {
            crate::session::start();
        } else {
            this.spawn_tab(SpawnTabDomain::DefaultDomain, None, Some("default".into()));
        }
        this
    }

    pub(crate) fn active_tab(&self) -> Option<Arc<Tab>> {
        Mux::get().get_active_tab_for_window(self.mux_window_id?)
    }

    pub(crate) fn active_pane(&self) -> Option<Arc<dyn Pane>> {
        self.active_tab()?.get_active_pane()
    }

    pub(crate) fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), Instant::now()));
        self.flash_pane = None;
    }

    /// A toast about an agent: clicking it goes to the agent's pane.
    fn flash_agent(&mut self, msg: impl Into<String>, pane_id: PaneId) {
        self.flash(msg);
        self.flash_pane = Some(pane_id);
    }

    /// A macOS notification about an agent, for when Wezzy is in the background.
    /// Title: who; body: what it needs (or its task when it's done).
    fn notify_agent(&self, a: &crate::agents::Agent, detail: Option<String>) {
        let title = match a.project.as_str() {
            "" => a.display_name().to_string(),
            p => format!("{} · {p}", a.display_name()),
        };
        let body = detail.unwrap_or_else(|| {
            // agents put their task in the pane title; skip a bare "Claude Code"
            let t = a.title.trim();
            let bare = t.to_lowercase().starts_with(&a.kind.to_lowercase());
            if bare { String::new() } else { t.to_string() }
        });
        crate::notify::post(a.pane_id, title, body);
    }

    fn on_event(&mut self, ev: AppEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            AppEvent::MuxChanged => {
                MUX_DIRTY.store(false, Ordering::Release);
                let mux = Mux::get();
                mux.prune_dead_windows();
                crate::notify::sync_badge(); // drops closed panes
                if let Some(id) = self.mux_window_id {
                    if mux.get_window(id).is_none() {
                        // Our window died (last pane exited): fall back to any other window.
                        self.mux_window_id = mux
                            .iter_windows_in_workspace(&mux.active_workspace())
                            .first()
                            .copied()
                            .or_else(|| mux.iter_windows().first().copied());
                        match self.mux_window_id.and_then(|w| mux.get_window(w)) {
                            // The workspace closed with it: follow the window we landed in.
                            Some(w) if w.get_workspace() != mux.active_workspace() => {
                                let ws = w.get_workspace().to_string();
                                drop(w);
                                mux.set_active_workspace(&ws);
                                crate::session::remember_workspace(&ws);
                            }
                            Some(_) => {}
                            None => cx.quit(),
                        }
                    }
                }
            }
            AppEvent::WindowReady(id) => {
                let mux = Mux::get();
                let in_active_ws = mux
                    .get_window(id)
                    .is_some_and(|w| w.get_workspace() == mux.active_workspace());
                if self.mux_window_id.is_none() || in_active_ws {
                    self.mux_window_id = Some(id);
                }
            }
            AppEvent::ConfigReloaded => {
                self.keys = KeyState::new(&config::configuration());
                // saving settings.json reloads too (it sits next to the config)
                if !crate::settings::just_saved() {
                    self.flash("config reloaded");
                }
            }
            AppEvent::Perform { pane_id, action } => self.perform(action, pane_id, window, cx),
            AppEvent::Clipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            AppEvent::Flash(msg) => self.flash(msg),
            AppEvent::Redraw => {}
            AppEvent::RebuildFinished { ok, dir } => self.rebuild_finished(ok, dir, window, cx),
            AppEvent::UserVar {
                pane_id,
                name,
                value,
            } => {
                // nvim (smart-splits) hit its outer edge with a panel beyond it
                if name == "SMART_SPLITS_REQ" {
                    self.smart_splits_edge(pane_id, &value);
                }
                if let Some((_, window_id, _)) = Mux::get().resolve_pane_id(pane_id) {
                    luawin::emit_user_var_changed(window_id, pane_id, name, value);
                }
            }
            AppEvent::Committed { result, amend } => self.on_committed(result, amend),
            AppEvent::PullRequest(ev) => self.on_pull_request(ev, cx),
            AppEvent::AgentStatusChanged {
                pane_id,
                status,
                detail,
            } => {
                use crate::hooks::HookStatus;
                let looking = self.active_pane().map(|p| p.pane_id()) == Some(pane_id)
                    && window.is_window_active();
                let attention = matches!(status, HookStatus::NeedsInput | HookStatus::Done);
                if status == HookStatus::Working {
                    crate::agents::mark_seen(pane_id);
                }
                if attention && !looking {
                    crate::agents::mark_unseen(pane_id);
                    crate::sound::chime();
                    if let Some(a) = crate::agents::collect(None)
                        .into_iter()
                        .find(|a| a.pane_id == pane_id)
                    {
                        let detail = detail.filter(|_| status == HookStatus::NeedsInput);
                        let msg = match &detail {
                            Some(d) => format!("{} · {d}", a.name.as_ref().unwrap_or(&a.project)),
                            None => format!("{} · {}", a.display_name(), a.project),
                        };
                        self.flash_agent(msg, pane_id);
                        if !window.is_window_active() {
                            self.notify_agent(&a, detail);
                        }
                    }
                }
            }
            AppEvent::AgentAlert(pane_id) => {
                let focused = self.active_pane().map(|p| p.pane_id()) == Some(pane_id)
                    && self.focus.is_focused(window);
                if focused {
                    clear_alert(pane_id);
                } else if let Some(a) = crate::agents::collect(None)
                    .into_iter()
                    .find(|a| a.pane_id == pane_id)
                {
                    let fresh = crate::agents::mark_unseen(pane_id);
                    self.flash_agent(format!("{} · {}", a.display_name(), a.project), pane_id);
                    // bells repeat; notify once until you've been back
                    if fresh && !window.is_window_active() {
                        self.notify_agent(&a, None);
                    }
                }
            }
            AppEvent::NotificationClicked(remote) => {
                if let Some(pane_id) = crate::session::local_pane_id(remote) {
                    cx.activate(true);
                    window.activate_window();
                    self.flash = None;
                    self.flash_pane = None;
                    self.region = crate::regions::Region::Terminal;
                    self.focus_pane(pane_id, window, cx);
                }
            }
            AppEvent::TabMoved { window: wid, tab, follow } => self.tab_moved(wid, tab, follow),
            AppEvent::PanesSwapped { tab, focus } => {
                let mux = Mux::get();
                if let (Some(tab), Some(pane)) = (mux.get_tab(tab), mux.get_pane(focus)) {
                    tab.set_active_pane(&pane);
                }
            }
        }
        cx.notify();
    }

    // -- input --------------------------------------------------------------

    pub(crate) fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self.copy_mode.is_some() && self.overlay.is_none() {
            self.copy_mode_key(ev, cx);
            return;
        }
        if self.pane_select.is_some() {
            self.pane_select_key(ev, cx);
            cx.notify();
            return;
        }
        if self.quick_select.is_some() {
            self.quick_select_key(ev, window, cx);
            cx.notify();
            return;
        }
        if self.agent_rename.is_some() {
            self.agent_rename_key(ev, cx);
            return;
        }
        if self.ws_new.is_some() {
            self.ws_new_key(ev, window, cx);
            cx.notify();
            return;
        }
        if self.ws_menu_open {
            self.ws_menu_key(ev, window, cx);
            cx.notify();
            return;
        }
        // the key that opens an overlay also closes it
        if let Some(c) = self.overlay_toggle_command(ev) {
            use crate::commands::AppCommand;
            let closes = matches!(
                (&self.overlay, c),
                (Some(Overlay::Palette(_)), AppCommand::CommandPalette)
                    | (Some(Overlay::Palette(_)), AppCommand::FindAgent)
                    | (Some(Overlay::Palette(_)), AppCommand::FindTab)
                    | (Some(Overlay::Palette(_)), AppCommand::SwitchWorkspace)
                    | (Some(Overlay::Settings(_)), AppCommand::OpenSettings)
            );
            if closes {
                self.overlay = None;
                cx.notify();
                return;
            }
        }
        if matches!(self.overlay, Some(Overlay::Palette(_))) {
            self.palette_key(ev, window, cx);
            cx.notify();
            return;
        }
        if matches!(self.overlay, Some(Overlay::Settings(_))) {
            self.settings_key(ev, window, cx);
            cx.notify();
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(ev, window, cx);
            cx.notify();
            return;
        }
        if self.region != crate::regions::Region::Terminal {
            // ⌘ chords (palette, toggles, tabs…) still go through the keymap while a
            // side panel has focus; the commit composer keeps the ones it edits with
            let ks = &ev.keystroke;
            let composer_owns = (self.git.composer.is_some() || self.git.comment_draft.is_some())
                && matches!(ks.key.as_str(), "enter" | "v" | "backspace");
            if ks.modifiers.platform && !composer_owns {
                if let Some(input) = keys::translate(ks) {
                    if let Lookup::Action(action) = self.keys.lookup(&input.key, input.mods) {
                        self.perform(action, None, window, cx);
                        cx.notify();
                        return;
                    }
                }
            }
            self.region_key(ev, window, cx);
            return;
        }
        if !self.keys.leader_active() {
            let pane = self.active_pane();
            if let Some(view) = pane.and_then(|p| self.views.get_mut(p.pane_id())) {
                if view.claims(&ev.keystroke) {
                    view.key(&ev.keystroke, cx);
                    cx.notify();
                    return;
                }
            }
        }
        if self.edge_nav(&ev.keystroke, cx) {
            return;
        }
        let Some(input) = keys::translate(&ev.keystroke) else {
            return;
        };
        let lookup = self.keys.lookup(&input.key, input.mods);
        log::debug!(
            "key {:?} held={} -> {:?} {:?} => {}",
            ev.keystroke,
            ev.is_held,
            input.key,
            input.mods,
            match &lookup {
                Lookup::Action(a) => format!("{a:?}"),
                Lookup::LeaderArmed => "leader".into(),
                Lookup::Swallow => "swallow".into(),
                Lookup::Agent(i) => format!("agent {}", i + 1),
                Lookup::Unbound => "to pane".into(),
            }
        );
        match lookup {
            Lookup::Action(action) => self.perform(action, None, window, cx),
            Lookup::LeaderArmed | Lookup::Swallow => {}
            Lookup::Agent(i) => match crate::agents::collect(None).get(i) {
                Some(a) => self.focus_pane(a.pane_id, window, cx),
                None => self.flash(&format!("No agent {}", i + 1)),
            },
            Lookup::Unbound => {
                let view = self
                    .active_pane()
                    .and_then(|p| self.views.get_mut(p.pane_id()));
                if let Some(view) = view {
                    view.key(&ev.keystroke, cx);
                } else if let (Some(pane), Some(tk)) = (self.active_pane(), input.term_key) {
                    self.viewports.remove(&pane.pane_id());
                    clear_alert(pane.pane_id());
                    if self.selection.as_ref().is_some_and(|s| s.pane_id == pane.pane_id()) {
                        self.selection = None;
                    }
                    if let Err(e) = pane.key_down(tk, input.term_mods) {
                        log::error!("key_down: {e:#}");
                    }
                }
            }
        }
        let mut ui = UI_STATE.lock();
        ui.leader_active = self.keys.leader_active();
        ui.active_key_table = self.keys.active_table().map(str::to_string);
        drop(ui);
        cx.notify();
    }

    fn overlay_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let Some(overlay) = self.overlay.as_mut() else {
            return;
        };
        let win_id = self.mux_window_id;
        match overlay {
            Overlay::Prompt {
                input,
                target,
                pane_id,
                ..
            } => match ks.key.as_str() {
                "escape" | "enter" => {
                    let line = (ks.key == "enter").then(|| input.clone());
                    let (target, pane_id) = (target.clone(), *pane_id);
                    self.overlay = None;
                    match target {
                        // action_callback: wezterm calls it with (window, pane, line|nil)
                        PromptTarget::Action(KeyAssignment::EmitEvent(name)) => {
                            if let Some(w) = win_id {
                                luawin::emit_window_event(name, w, pane_id, Some(line));
                            }
                        }
                        PromptTarget::Action(other) if line.is_some() => {
                            self.perform(other, Some(pane_id), window, cx)
                        }
                        PromptTarget::RenameTab(tab_id) => {
                            if let (Some(name), Some(tab)) = (line, Mux::get().get_tab(tab_id)) {
                                tab.set_title(name.trim());
                            }
                        }
                        PromptTarget::RenameWorkspace(old) => {
                            let name = line.map(|l| l.trim().to_string()).unwrap_or_default();
                            if !name.is_empty() && name != old {
                                Mux::get().rename_workspace(&old, &name);
                                crate::session::remember_workspace(&name);
                            }
                        }
                        _ => {}
                    }
                }
                "backspace" => {
                    input.pop();
                }
                "v" if ks.modifiers.platform => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        input.push_str(t.lines().next().unwrap_or(""));
                    }
                }
                _ if !ks.modifiers.control && !ks.modifiers.platform => {
                    if let Some(c) = &ks.key_char {
                        input.push_str(c);
                    }
                }
                _ => {}
            },
            Overlay::Confirm { action, .. } => {
                let quit_again = ks.key == "q"
                    && ks.modifiers.platform
                    && matches!(action, KeyAssignment::QuitApplication);
                match ks.key.as_str() {
                    "y" | "enter" => self.answer_confirm(true, window, cx),
                    "n" | "escape" => self.answer_confirm(false, window, cx),
                    _ if quit_again => self.answer_confirm(true, window, cx),
                    _ => {}
                }
            }
            Overlay::Shortcuts => self.overlay = None,
            Overlay::Palette(_) | Overlay::Settings(_) => {}
        }
    }

    pub(crate) fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window);
        self.region = crate::regions::Region::Terminal;
        let g = self.grid.get();
        if g.cell_w <= px(0.) {
            return;
        }
        let col = ((ev.position.x - g.origin.x) / g.cell_w).floor();
        let row = ((ev.position.y - g.origin.y) / g.line_h).floor();
        if col < 0. || row < 0. {
            return;
        }
        let (col, row) = (col as usize, row as usize);
        if let Some(tab) = self.active_tab() {
            // grabbing a divider starts a resize drag
            if let Some(s) = tab.iter_splits().into_iter().find(|s| match s.direction {
                SplitDirection::Horizontal => col == s.left && row >= s.top && row < s.top + s.size,
                SplitDirection::Vertical => row == s.top && col >= s.left && col < s.left + s.size,
            }) {
                self.split_drag = Some(s.index);
                cx.notify();
                return;
            }
            if let Some(p) = tab.iter_panes().into_iter().find(|p| {
                col >= p.left && col < p.left + p.width && row >= p.top && row < p.top + p.height
            }) {
                tab.set_active_idx(p.index);
                // ⌘-click opens the URL / file:line under the mouse
                if ev.modifiers.platform {
                    let pane = p.pane.clone();
                    self.open_link_at(&pane, row - p.top, col - p.left, window, cx);
                    cx.notify();
                    return;
                }
            }
        }
        if self.copy_mode.is_none() {
            self.start_selection(ev.position, ev.click_count, ev.modifiers.shift);
        }
        cx.notify();
    }

    /// Files dropped from Finder (or anywhere else) onto the terminal: like WezTerm,
    /// paste their paths, quoted per `quote_dropped_files`, into the pane under the
    /// cursor. Claude Code / pi pick up pasted image paths as attachments.
    pub(crate) fn on_file_drop(&mut self, paths: &gpui::ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        if paths.paths().is_empty() {
            return;
        }
        self.focus.focus(window);
        self.region = crate::regions::Region::Terminal;
        let Some(tab) = self.active_tab() else {
            return;
        };
        // target the pane the drop landed on, falling back to the active one
        let g = self.grid.get();
        let pos = window.mouse_position();
        if g.cell_w > px(0.) && g.line_h > px(0.) {
            let col = ((pos.x - g.origin.x) / g.cell_w).floor();
            let row = ((pos.y - g.origin.y) / g.line_h).floor();
            if col >= 0. && row >= 0. {
                let (col, row) = (col as usize, row as usize);
                if let Some(p) = tab.iter_panes().into_iter().find(|p| {
                    col >= p.left && col < p.left + p.width && row >= p.top && row < p.top + p.height
                }) {
                    tab.set_active_idx(p.index);
                }
            }
        }
        let Some(pane) = tab.get_active_pane() else {
            return;
        };
        let quoting = config::configuration().quote_dropped_files;
        let text = paths
            .paths()
            .iter()
            .map(|p| quoting.escape(&p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ")
            + " ";
        self.viewports.remove(&pane.pane_id());
        pane.send_paste(&text).ok();
        cx.notify();
    }

    /// Mouse moved while dragging a divider: move it to the cell under the mouse,
    /// like WezTerm's drag_split. Returns false when no drag is in progress.
    pub(crate) fn drag_split(&mut self, ev: &gpui::MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.split_drag else {
            return false;
        };
        if ev.pressed_button != Some(gpui::MouseButton::Left) {
            self.split_drag = None;
            cx.notify();
            return true;
        }
        let g = self.grid.get();
        let Some(tab) = self.active_tab() else {
            return true;
        };
        if g.cell_w <= px(0.) || g.line_h <= px(0.) {
            return true;
        }
        let Some(split) = tab.iter_splits().into_iter().find(|s| s.index == index) else {
            self.split_drag = None;
            return true;
        };
        let col = ((ev.position.x - g.origin.x) / g.cell_w).floor() as isize;
        let row = ((ev.position.y - g.origin.y) / g.line_h).floor() as isize;
        let delta = match split.direction {
            SplitDirection::Horizontal => col - split.left as isize,
            SplitDirection::Vertical => row - split.top as isize,
        };
        if delta != 0 {
            tab.resize_split_by(split.index, delta);
        }
        cx.notify();
        true
    }

    pub(crate) fn on_scroll(&mut self, ev: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let line_h = f32::from(self.grid.get().line_h).max(1.0);
        let dy = match ev.delta {
            ScrollDelta::Lines(p) => p.y * 3.0,
            ScrollDelta::Pixels(p) => f32::from(p.y) / line_h,
        };
        self.scroll_accum += dy;
        let lines = self.scroll_accum.trunc() as isize;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;
        // like WezTerm, the wheel acts on the pane under the mouse
        let g = self.grid.get();
        let hit = (g.cell_w > px(0.) && g.line_h > px(0.))
            .then(|| {
                let col = ((ev.position.x - g.origin.x) / g.cell_w).floor();
                let row = ((ev.position.y - g.origin.y) / g.line_h).floor();
                (col >= 0. && row >= 0.).then_some((col as usize, row as usize))
            })
            .flatten()
            .and_then(|(col, row)| {
                self.active_tab()?.iter_panes().into_iter().find_map(|p| {
                    (col >= p.left && col < p.left + p.width && row >= p.top && row < p.top + p.height)
                        .then(|| (p.pane.clone(), col - p.left, row - p.top))
                })
            });
        let (pane, col, row) = match hit {
            Some(h) => h,
            None => match self.active_pane() {
                Some(p) => (p, 0, 0),
                None => return,
            },
        };
        if pane.is_mouse_grabbed() {
            // the app asked for mouse reporting (Claude Code's fullscreen UI, vim
            // with mouse=a, …): send real wheel events, one per line, as WezTerm does
            use wezterm_term::{KeyModifiers, MouseButton as TMB, MouseEvent, MouseEventKind};
            let button = if lines > 0 { TMB::WheelUp(1) } else { TMB::WheelDown(1) };
            for _ in 0..lines.unsigned_abs() {
                pane.mouse_event(MouseEvent {
                    kind: MouseEventKind::Press,
                    x: col,
                    y: row as i64,
                    x_pixel_offset: 0,
                    y_pixel_offset: 0,
                    button,
                    modifiers: KeyModifiers::NONE,
                })
                .ok();
            }
        } else if pane.is_alt_screen_active() {
            // Like WezTerm's alternate_scroll: turn the wheel into arrow keys for less/vim/etc.
            let key = if lines > 0 {
                termwiz::input::KeyCode::UpArrow
            } else {
                termwiz::input::KeyCode::DownArrow
            };
            for _ in 0..lines.unsigned_abs() {
                pane.key_down(key, termwiz::input::Modifiers::NONE).ok();
            }
        } else {
            self.scroll_by(&pane, -lines);
        }
        cx.notify();
    }

    pub(crate) fn scroll_by(&mut self, pane: &Arc<dyn Pane>, delta: isize) {
        let dims = pane.get_dimensions();
        let id = pane.pane_id();
        let cur = self.viewports.get(&id).copied().unwrap_or(dims.physical_top);
        let top = (cur + delta as StableRowIndex).clamp(dims.scrollback_top, dims.physical_top);
        if top == dims.physical_top {
            self.viewports.remove(&id);
        } else {
            self.viewports.insert(id, top);
        }
    }

    // -- actions -----------------------------------------------------------

    pub(crate) fn perform(
        &mut self,
        action: KeyAssignment,
        pane_id: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use KeyAssignment::*;
        let mux = Mux::get();
        let tab = self.active_tab();
        let pane = pane_id
            .and_then(|id| mux.get_pane(id))
            .or_else(|| self.active_pane());
        let config = config::configuration();
        log::debug!("perform {action:?} pane={pane_id:?}");

        match action {
            Nop | DisableDefaultAssignment => {}
            Multiple(actions) => {
                for a in actions {
                    self.perform(a, pane_id, window, cx);
                }
            }

            // spawning
            SpawnTab(domain) => self.spawn_tab(domain, None, None),
            SpawnWindow => {
                let ws = Mux::get().active_workspace();
                self.spawn_tab(SpawnTabDomain::CurrentPaneDomain, None, Some(ws))
            }
            SpawnCommandInNewTab(cmd) => self.spawn_tab(cmd.domain.clone(), Some(cmd), None),
            SplitHorizontal(cmd) => self.split(SplitDirection::Horizontal, true, SplitSize::Percent(50), false, cmd),
            SplitVertical(cmd) => self.split(SplitDirection::Vertical, true, SplitSize::Percent(50), false, cmd),
            SplitPane(sp) => {
                let (dir, second) = match sp.direction {
                    PaneDirection::Up => (SplitDirection::Vertical, false),
                    PaneDirection::Down => (SplitDirection::Vertical, true),
                    PaneDirection::Left => (SplitDirection::Horizontal, false),
                    PaneDirection::Right => (SplitDirection::Horizontal, true),
                    _ => return,
                };
                let size = match sp.size {
                    CfgSplitSize::Percent(n) => SplitSize::Percent(n),
                    CfgSplitSize::Cells(n) => SplitSize::Cells(n),
                };
                self.split(dir, second, size, sp.top_level, sp.command);
            }

            // panes
            ActivatePaneDirection(dir) => tab.iter().for_each(|t| t.activate_pane_direction(dir)),
            AdjustPaneSize(dir, n) => tab.iter().for_each(|t| t.adjust_pane_size(dir, n)),
            ActivatePaneByIndex(i) => tab.iter().for_each(|t| t.set_active_idx(i)),
            TogglePaneZoomState => tab.iter().for_each(|t| t.toggle_zoom()),
            SetPaneZoomState(z) => tab.iter().for_each(|t| {
                t.set_zoomed(z);
            }),
            RotatePanes(dir) => tab.iter().for_each(|t| match dir {
                RotationDirection::Clockwise => t.rotate_clockwise(),
                RotationDirection::CounterClockwise => t.rotate_counter_clockwise(),
            }),
            CloseCurrentPane { confirm } => {
                if let (Some(tab), Some(pane)) = (tab, pane) {
                    let ask = confirm && needs_confirmation(|| crate::procs::can_close_quietly(&pane));
                    if ask {
                        self.overlay = Some(Overlay::Confirm {
                            message: "Close this pane?".into(),
                            detail: running_detail(&[pane.clone()]),
                            verb: "Close",
                            action: CloseCurrentPane { confirm: false },
                            cancel: None,
                            pane_id: pane.pane_id(),
                        });
                    } else {
                        tab.kill_pane(pane.pane_id());
                        mux.prune_dead_windows();
                    }
                }
            }
            CloseCurrentTab { confirm } => {
                if let (Some(tab), Some(pane)) = (tab, pane) {
                    let panes: Vec<_> =
                        tab.iter_panes_ignoring_zoom().into_iter().map(|p| p.pane).collect();
                    // a split tab always asks: closing it loses the layout too
                    let ask = confirm
                        && (panes.len() > 1
                            || needs_confirmation(|| {
                                panes.iter().all(crate::procs::can_close_quietly)
                            }));
                    if ask {
                        let mut detail = running_detail(&panes);
                        if detail.is_empty() && panes.len() > 1 {
                            detail = format!("{} panes", panes.len());
                        }
                        self.overlay = Some(Overlay::Confirm {
                            message: "Close this tab?".into(),
                            detail,
                            verb: "Close",
                            action: CloseCurrentTab { confirm: false },
                            cancel: None,
                            pane_id: pane.pane_id(),
                        });
                    } else {
                        mux.remove_tab(tab.tab_id());
                        mux.prune_dead_windows();
                    }
                }
            }

            // tabs
            ActivateTab(i) => self.with_window(|w| {
                let n = w.count_tabs() as isize;
                let idx = if i < 0 { n + i } else { i };
                if (0..n).contains(&idx) {
                    w.remember_and_set_active_tab_idx(idx as usize);
                }
            }),
            ActivateTabRelative(d) | ActivateTabRelativeNoWrap(d) => {
                let wrap = matches!(action, ActivateTabRelative(_));
                self.with_window(|w| {
                    let n = w.count_tabs() as isize;
                    if n == 0 {
                        return;
                    }
                    let mut idx = w.get_active_tab_idx() as isize + d;
                    idx = if wrap { idx.rem_euclid(n) } else { idx.clamp(0, n - 1) };
                    w.remember_and_set_active_tab_idx(idx as usize);
                })
            }
            ActivateLastTab => self.with_window(|w| {
                if let Some(idx) = w.get_last_active_tab_idx() {
                    w.remember_and_set_active_tab_idx(idx);
                }
            }),
            MoveTab(to) => self.move_tab(|_, _| to as isize),
            MoveTabRelative(d) => self.move_tab(|cur, _| cur + d),

            // scrolling
            ScrollByPage(f) => {
                if let Some(p) = &pane {
                    let rows = p.get_dimensions().viewport_rows as f64;
                    self.scroll_by(p, (rows * f.into_inner()) as isize);
                }
            }
            ScrollByLine(n) => pane.iter().for_each(|p| self.scroll_by(p, n)),
            ScrollToTop => pane.iter().for_each(|p| self.scroll_by(p, isize::MIN / 2)),
            ScrollToBottom => pane.iter().for_each(|p| {
                self.viewports.remove(&p.pane_id());
            }),
            ClearScrollback(mode) => pane.iter().for_each(|p| p.erase_scrollback(mode)),

            // input
            SendString(s) => {
                if let Some(p) = &pane {
                    p.writer().write_all(s.as_bytes()).ok();
                }
            }
            SendKey(k) => {
                if let Some(p) = &pane {
                    let key = k.key.resolve(config.key_map_preference).clone();
                    if let Some(tk) = keys::wkey_to_term(&key) {
                        p.key_down(tk, k.mods).ok();
                    }
                }
            }
            PasteFrom(_) if pane.as_ref().is_some_and(crate::views::is_view) => {
                let id = pane.as_ref().map(|p| p.pane_id()).unwrap();
                if let (Some(view), Some(text)) =
                    (self.views.get_mut(id), cx.read_from_clipboard().and_then(|c| c.text()))
                {
                    view.paste(&text);
                }
            }
            CopyTo(_) if pane.as_ref().is_some_and(crate::views::is_view) => {
                let id = pane.as_ref().map(|p| p.pane_id()).unwrap();
                if let Some(text) = self.views.get_mut(id).and_then(|v| v.copy()) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            PasteFrom(_) => {
                if let (Some(p), Some(text)) = (&pane, crate::clipboard::paste_text(cx)) {
                    self.viewports.remove(&p.pane_id());
                    p.send_paste(&text).ok();
                }
            }
            CopyTextTo { text, .. } => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            CopyTo(_) => {
                if self.copy_selection(cx) {
                    self.flash("Copied");
                }
            }

            // key tables
            ActivateKeyTable {
                name,
                timeout_milliseconds,
                replace_current,
                one_shot,
                until_unknown,
                prevent_fallback,
            } => self.keys.push_table(
                name,
                timeout_milliseconds,
                replace_current,
                one_shot,
                until_unknown,
                prevent_fallback,
            ),
            PopKeyTable => self.keys.pop_table(),
            ClearKeyTableStack => self.keys.clear_tables(),

            // lua + overlays
            EmitEvent(name) if crate::commands::AppCommand::from_event(&name).is_some() => {
                let c = crate::commands::AppCommand::from_event(&name).unwrap();
                self.run_app_command(c, window, cx);
            }
            ActivateCommandPalette | ShowLauncher => {
                self.open_palette(crate::palette::Scope::All, cx)
            }
            ShowTabNavigator => self.open_palette(crate::palette::Scope::Tabs, cx),
            QuickSelect => self.start_quick_select(&[], cx),
            ActivateCopyMode => self.start_copy_mode(cx),
            QuickSelectArgs(args) => self.start_quick_select(&args.patterns, cx),
            PaneSelect(args) => self.start_pane_select(&args, cx),
            ShowLauncherArgs(args) => {
                use config::keyassignment::LauncherFlags;
                use crate::palette::Scope;
                let scope = if args.flags.contains(LauncherFlags::WORKSPACES)
                    && !args.flags.contains(LauncherFlags::TABS)
                {
                    Scope::Workspaces
                } else if args.flags.contains(LauncherFlags::TABS)
                    && !args.flags.contains(LauncherFlags::WORKSPACES)
                {
                    Scope::Tabs
                } else {
                    Scope::All
                };
                self.open_palette(scope, cx);
            }
            EmitEvent(name) => {
                if let (Some(w), Some(p)) = (self.mux_window_id, &pane) {
                    luawin::emit_window_event(name, w, p.pane_id(), None);
                }
            }
            PromptInputLine(p) => {
                if let Some(pane) = &pane {
                    self.overlay = Some(Overlay::Prompt {
                        description: strip_escapes(&p.description),
                        prompt: p.prompt.clone(),
                        input: p.initial_value.clone().unwrap_or_default(),
                        target: PromptTarget::Action(*p.action),
                        pane_id: pane.pane_id(),
                    });
                }
            }
            Confirmation(c) => {
                if let Some(pane) = &pane {
                    self.overlay = Some(Overlay::Confirm {
                        message: strip_escapes(&c.message),
                        detail: String::new(),
                        verb: "OK",
                        action: *c.action,
                        cancel: c.cancel.map(|b| *b),
                        pane_id: pane.pane_id(),
                    });
                }
            }

            // workspaces
            SwitchToWorkspace { name, spawn } => self.switch_workspace(name, spawn),
            SwitchWorkspaceRelative(d) => {
                let all = mux.iter_workspaces();
                if let Some(cur) = all.iter().position(|w| *w == mux.active_workspace()) {
                    let n = all.len() as isize;
                    let next = all[(cur as isize + d).rem_euclid(n) as usize].clone();
                    self.switch_workspace(Some(next), None);
                }
            }

            // app
            ReloadConfiguration => config::reload(),
            IncreaseFontSize => self.font_scale *= 1.1,
            DecreaseFontSize => self.font_scale = (self.font_scale / 1.1).max(0.3),
            ResetFontSize | ResetFontAndWindowSize => self.font_scale = 1.0,
            ToggleFullScreen => window.toggle_fullscreen(),
            Hide | HideApplication => cx.hide(),
            // quitting is rare and easy to hit by accident: always ask
            // (answer_confirm quits for real; ⌘Q again also confirms)
            QuitApplication => match &pane {
                Some(pane) => {
                    self.overlay = Some(Overlay::Confirm {
                        message: "Quit Wezzy?".into(),
                        detail: String::new(),
                        verb: "Quit",
                        action: QuitApplication,
                        cancel: None,
                        pane_id: pane.pane_id(),
                    });
                }
                None => cx.quit(),
            },
            ResetTerminal => pane.iter().for_each(|p| {
                use termwiz::escape::{Action, Esc, EscCode};
                p.perform_actions(vec![Action::Esc(Esc::Code(EscCode::FullReset))]);
            }),
            OpenUri(uri) => cx.open_url(&uri),

            other => {
                let name = format!("{other:?}");
                let name = name.split(['(', ' ', '{']).next().unwrap_or("").to_string();
                log::warn!("not implemented yet: {other:?}");
                self.flash(format!("{name}: not implemented yet"));
            }
        }
        cx.notify();
    }

    fn with_window(&self, f: impl FnOnce(&mut mux::window::Window)) {
        if let Some(id) = self.mux_window_id {
            if let Some(mut w) = Mux::get().get_window_mut(id) {
                f(&mut w);
            }
        }
    }

    fn move_tab(&self, target: impl FnOnce(isize, isize) -> isize) {
        self.with_window(|w| {
            let n = w.count_tabs() as isize;
            let cur = w.get_active_tab_idx();
            let to = target(cur as isize, n).clamp(0, n - 1) as usize;
            if to != cur {
                let tab = w.remove_tab_idx(cur);
                w.insert_tab_at_idx(to, &tab);
                w.set_active_tab_idx_without_saving(to);
            }
        });
        Mux::get().notify(MuxNotification::WindowInvalidated(
            self.mux_window_id.unwrap_or_default(),
        ));
    }

    fn current_size(&self) -> TerminalSize {
        self.active_tab()
            .map(|t| t.get_size())
            .unwrap_or_else(|| config::configuration().initial_size(0, None))
    }

    /// `workspace`: Some(name) => new mux window in that workspace; None => new tab in ours.
    pub(crate) fn spawn_tab(
        &self,
        domain: SpawnTabDomain,
        cmd: Option<SpawnCommand>,
        workspace: Option<String>,
    ) {
        let mux = Mux::get();
        let window_id = if workspace.is_some() {
            None
        } else {
            self.mux_window_id
        };
        let size = self.current_size();
        let current_pane = self.active_pane().map(|p| p.pane_id());
        let workspace = workspace.unwrap_or_else(|| mux.active_workspace());
        promise::spawn::spawn(async move {
            let (builder, cwd) = build_command(cmd.as_ref());
            match mux
                .spawn_tab_or_window(
                    window_id,
                    domain,
                    builder,
                    cwd,
                    size,
                    current_pane,
                    workspace,
                    None,
                )
                .await
            {
                Ok((_tab, pane, wid)) => {
                    pane.set_config(Arc::new(TermConfig::new()));
                    send_event(AppEvent::WindowReady(wid));
                }
                Err(e) => send_event(AppEvent::Flash(format!("spawn failed: {e:#}"))),
            }
        })
        .detach();
    }

    fn split(
        &self,
        direction: SplitDirection,
        target_is_second: bool,
        size: SplitSize,
        top_level: bool,
        cmd: SpawnCommand,
    ) {
        let Some(pane) = self.active_pane() else {
            return;
        };
        let pane_id = pane.pane_id();
        let request = SplitRequest {
            direction,
            target_is_second,
            top_level,
            size,
        };
        promise::spawn::spawn(async move {
            let (command, command_dir) = build_command(Some(&cmd));
            match Mux::get()
                .split_pane(
                    pane_id,
                    request,
                    SplitSource::Spawn {
                        command,
                        command_dir,
                    },
                    cmd.domain.clone(),
                )
                .await
            {
                Ok((pane, _)) => pane.set_config(Arc::new(TermConfig::new())),
                Err(e) => send_event(AppEvent::Flash(format!("split failed: {e:#}"))),
            }
        })
        .detach();
    }

    pub(crate) fn switch_workspace(&mut self, name: Option<String>, spawn: Option<SpawnCommand>) {
        let mux = Mux::get();
        let name = name.unwrap_or_else(|| mux.generate_workspace_name());
        mux.set_active_workspace(&name);
        crate::session::remember_workspace(&name);
        if let Some(w) = mux.iter_windows_in_workspace(&name).first() {
            self.mux_window_id = Some(*w);
        } else {
            // a saved workspace opens in its directory
            let spawn = spawn.or_else(|| crate::workspaces::saved(&name).map(|w| w.spawn()));
            let domain = spawn
                .as_ref()
                .map(|s| s.domain.clone())
                .unwrap_or(SpawnTabDomain::DefaultDomain);
            self.spawn_tab(domain, spawn, Some(name.clone()));
            // WindowReady points us at the new window once its shell is up.
            self.mux_window_id = None;
        }
    }

    /// Move a tab into another workspace (running, saved or new). A tab that is
    /// its window's only tab, going to a workspace with no window yet, takes
    /// its window along. Otherwise the session protocol has no "move tab": the
    /// first pane moves into a new tab there, and the rest follow one by one
    /// into splits rebuilt from the original layout.
    pub(crate) fn move_tab_to_workspace(&mut self, tab_id: TabId, target: String) {
        let mux = Mux::get();
        let (Some(tab), Some(src)) = (mux.get_tab(tab_id), mux.window_containing_tab(tab_id))
        else {
            return;
        };
        let Some((source_ws, tabs_in_src)) = mux
            .get_window(src)
            .map(|w| (w.get_workspace().to_string(), w.count_tabs()))
        else {
            return;
        };
        if source_ws == target {
            return;
        }
        // the source window dies with its last tab: follow the tab unless the
        // workspace has another window to show
        let follow = tabs_in_src == 1 && mux.iter_windows_in_workspace(&source_ws).len() == 1;
        let dest = mux.iter_windows_in_workspace(&target).first().copied();

        if tabs_in_src == 1 && dest.is_none() {
            if let Some(mut w) = mux.get_window_mut(src) {
                w.set_workspace(&target);
            }
            self.tab_moved(src, tab_id, follow);
            return;
        }
        let layout = tab.codec_pane_tree();
        let active = tab.get_active_pane().map(|p| p.pane_id());
        promise::spawn::spawn(async move {
            match rebuild_tab(layout, active, dest, target).await {
                Ok((tab, window)) => send_event(AppEvent::TabMoved { window, tab, follow }),
                Err(e) => send_event(AppEvent::Flash(format!("move failed: {e:#}"))),
            }
        })
        .detach();
    }

    fn tab_moved(&mut self, window_id: WindowId, tab_id: TabId, follow: bool) {
        let mux = Mux::get();
        let Some(ws) = mux.get_window(window_id).map(|w| w.get_workspace().to_string()) else {
            return;
        };
        if let Some(mut w) = mux.get_window_mut(window_id) {
            if let Some(idx) = w.get_tab_idx_for_id(tab_id) {
                w.set_active_tab_idx_without_saving(idx);
            }
        }
        if follow {
            mux.set_active_workspace(&ws);
            crate::session::remember_workspace(&ws);
            self.mux_window_id = Some(window_id);
        } else {
            self.flash(format!("Moved to {ws}"));
        }
    }

    /// Bring a pane to the front: switch workspace, tab and pane as needed.
    pub(crate) fn focus_pane(&mut self, pane_id: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let mux = Mux::get();
        let Some((_, window_id, tab_id)) = mux.resolve_pane_id(pane_id) else {
            return;
        };
        if let Some(ws) = mux.get_window(window_id).map(|w| w.get_workspace().to_string()) {
            if ws != mux.active_workspace() {
                mux.set_active_workspace(&ws);
            }
        }
        self.mux_window_id = Some(window_id);
        if let Some(mut w) = mux.get_window_mut(window_id) {
            if let Some(idx) = w.get_tab_idx_for_id(tab_id) {
                w.remember_and_set_active_tab_idx(idx);
            }
        }
        if let (Some(tab), Some(pane)) = (mux.get_tab(tab_id), mux.get_pane(pane_id)) {
            tab.set_active_pane(&pane);
        }
        self.focus.focus(window);
        cx.notify();
    }

    /// Tab ✕ button: same confirmation policy as the CloseCurrentTab binding.
    pub(crate) fn close_tab_at(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.with_window(|w| {
            if idx < w.count_tabs() {
                w.remember_and_set_active_tab_idx(idx);
            }
        });
        self.perform(KeyAssignment::CloseCurrentTab { confirm: true }, None, window, cx);
    }

    /// Leader , / Leader $: the name prompt, filled with the current name.
    pub(crate) fn prompt_rename(&mut self, workspace: bool, cx: &mut Context<Self>) {
        let (Some(tab), Some(pane)) = (self.active_tab(), self.active_pane()) else {
            return;
        };
        let (description, input, target) = if workspace {
            let ws = Mux::get().active_workspace();
            ("Rename workspace", ws.clone(), PromptTarget::RenameWorkspace(ws))
        } else {
            ("Rename tab", tab.get_title(), PromptTarget::RenameTab(tab.tab_id()))
        };
        self.overlay = Some(Overlay::Prompt {
            description: description.into(),
            prompt: String::new(),
            input,
            target,
            pane_id: pane.pane_id(),
        });
        cx.notify();
    }

    /// Open the workspace menu with its inline name field focused.
    pub(crate) fn prompt_new_workspace(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        self.sidebar_open = true;
        self.ws_menu_open = true;
        self.ws_new = Some(String::new());
        cx.notify();
    }

    fn smart_splits_edge(&mut self, pane_id: PaneId, value: &str) {
        let Ok(req) = serde_json::from_str::<serde_json::Value>(value) else {
            return;
        };
        if req["action"].as_str() != Some("move") {
            return;
        }
        let (dir, region, open) = match req["dir"].as_str() {
            Some("right") => (PaneDirection::Right, crate::regions::Region::Diff, self.diff_open),
            Some("left") => (PaneDirection::Left, crate::regions::Region::Sidebar, self.sidebar_open),
            _ => return,
        };
        let is_active = self.active_pane().map(|p| p.pane_id()) == Some(pane_id);
        if open && is_active && self.active_tab().is_some_and(|t| t.get_pane_direction(dir, false).is_none()) {
            self.region = region;
        }
    }

    /// The app command this keystroke is bound to, if any (no leader / key tables).
    fn overlay_toggle_command(&self, ev: &KeyDownEvent) -> Option<crate::commands::AppCommand> {
        self.overlay.as_ref()?;
        let input = keys::translate(&ev.keystroke)?;
        match self.keys.peek(&input.key, input.mods)? {
            KeyAssignment::EmitEvent(name) => crate::commands::AppCommand::from_event(&name),
            KeyAssignment::ActivateCommandPalette => {
                Some(crate::commands::AppCommand::CommandPalette)
            }
            _ => None,
        }
    }

    /// j/k/↵ in the workspace menu.
    fn ws_menu_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let names: Vec<String> = crate::workspaces::list().into_iter().map(|e| e.name).collect();
        let n = names.len() + 1; // + "New workspace"
        self.ws_cursor = self.ws_cursor.min(n - 1);
        match ks.key.as_str() {
            "escape" | "q" => self.ws_menu_open = false,
            "[" if ks.modifiers.control => self.ws_menu_open = false,
            "j" | "down" => self.ws_cursor = (self.ws_cursor + 1).min(n - 1),
            "k" | "up" => self.ws_cursor = self.ws_cursor.saturating_sub(1),
            "enter" => {
                if let Some(name) = names.get(self.ws_cursor).cloned() {
                    self.ws_menu_open = false;
                    self.switch_workspace(Some(name), None);
                    self.focus.focus(window);
                } else {
                    self.ws_new = Some(String::new());
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn ws_new_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let Some(input) = self.ws_new.as_mut() else {
            return;
        };
        match ks.key.as_str() {
            "escape" => {
                self.ws_new = None;
                self.ws_menu_open = false;
            }
            "enter" => {
                let typed = input.trim().to_string();
                self.ws_new = None;
                self.ws_menu_open = false;
                let name = if typed.is_empty() {
                    Mux::get().generate_workspace_name()
                } else {
                    typed
                };
                self.switch_workspace(Some(name), None);
                self.focus.focus(window);
            }
            "backspace" if ks.modifiers.alt || ks.modifiers.platform => input.clear(),
            "backspace" => {
                input.pop();
            }
            "v" if ks.modifiers.platform => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    input.push_str(t.lines().next().unwrap_or("").trim());
                }
            }
            _ if !ks.modifiers.control && !ks.modifiers.platform => {
                if let Some(c) = &ks.key_char {
                    input.push_str(c);
                }
            }
            _ => {}
        }
    }
}

/// Recreate a tab's layout in `dest` (or a new window in `workspace`) by
/// moving its panes there on the session server: the first pane becomes a new
/// tab, then each split is redone by moving its second half's first pane next
/// to the first half's.
async fn rebuild_tab(
    layout: PaneNode,
    active: Option<PaneId>,
    dest: Option<WindowId>,
    workspace: String,
) -> anyhow::Result<(TabId, WindowId)> {
    fn first_leaf(node: &PaneNode) -> Option<PaneId> {
        match node {
            PaneNode::Empty => None,
            PaneNode::Leaf(e) => Some(e.pane_id),
            PaneNode::Split { left, .. } => first_leaf(left),
        }
    }
    let mux = Mux::get();
    let first = first_leaf(&layout).ok_or_else(|| anyhow::anyhow!("tab has no panes"))?;
    let (tab, window) = mux.move_pane_to_new_tab(first, dest, Some(workspace)).await?;
    let mut todo = vec![(layout, first)];
    while let Some((node, anchor)) = todo.pop() {
        let PaneNode::Split { left, right, node } = node else {
            continue;
        };
        let moving = first_leaf(&right).ok_or_else(|| anyhow::anyhow!("empty split"))?;
        let (a, b) = match node.direction {
            SplitDirection::Horizontal => (node.first.cols, node.second.cols),
            SplitDirection::Vertical => (node.first.rows, node.second.rows),
        };
        let percent = (b * 100 / (a + b).max(1)).clamp(1, 99) as u8;
        let request = SplitRequest {
            direction: node.direction,
            target_is_second: true,
            top_level: false,
            size: SplitSize::Percent(percent),
        };
        mux.split_pane(
            anchor,
            request,
            SplitSource::MovePane(moving),
            SpawnTabDomain::CurrentPaneDomain,
        )
        .await?;
        todo.push((*left, anchor));
        todo.push((*right, moving));
    }
    if let (Some(active), Some(pane)) = (active, active.and_then(|p| mux.get_pane(p))) {
        if let Some(tab) = mux.resolve_pane_id(active).and_then(|(_, _, t)| mux.get_tab(t)) {
            tab.set_active_pane(&pane);
        }
    }
    Ok((tab.tab_id(), window))
}

fn build_command(spawn: Option<&SpawnCommand>) -> (Option<CommandBuilder>, Option<String>) {
    let Some(spawn) = spawn else {
        return (None, None);
    };
    let cwd = spawn
        .cwd
        .as_ref()
        .and_then(|c| c.to_str().map(str::to_string));
    if spawn.args.is_none() && spawn.cwd.is_none() && spawn.set_environment_variables.is_empty()
    {
        return (None, cwd);
    }
    let mut b = spawn
        .args
        .as_ref()
        .map(|a| CommandBuilder::from_argv(a.iter().map(Into::into).collect()))
        .unwrap_or_else(CommandBuilder::new_default_prog);
    for (k, v) in &spawn.set_environment_variables {
        b.env(k, v);
    }
    if let Some(c) = &spawn.cwd {
        b.cwd(c);
    }
    (Some(b), cwd)
}

/// `wezterm.format(...)` output is SGR-escaped; we only want the text.
pub(crate) fn strip_escapes(s: &str) -> String {
    use termwiz::escape::{parser::Parser, Action};
    let mut out = String::new();
    Parser::new().parse(s.as_bytes(), |a| match a {
        Action::Print(c) => out.push(c),
        Action::PrintString(s) => out.push_str(&s),
        _ => {}
    });
    out.trim().to_string()
}


fn needs_confirmation(can_close_quietly: impl FnOnce() -> bool) -> bool {
    use crate::settings::CloseConfirmation::*;
    match crate::settings::get().close_confirmation {
        Never => false,
        Always => true,
        Smart => !can_close_quietly(),
    }
}

/// "claude is still running" / "claude and node are still running"
fn running_detail(panes: &[Arc<dyn Pane>]) -> String {
    let mut names: Vec<String> = panes
        .iter()
        .filter(|p| !crate::views::is_view(p))
        .filter_map(crate::procs::foreground_name)
        .map(|n| {
            std::path::Path::new(&n)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or(n)
        })
        .filter(|n| {
            !config::configuration()
                .skip_close_confirmation_for_processes_named
                .iter()
                .any(|s| s == n)
        })
        .collect();
    names.dedup();
    match names.len() {
        0 => String::new(),
        1 => format!("{} is still running.", names[0]),
        _ => format!("{} are still running.", names.join(", ")),
    }
}


/// Evens out every split in `tab`: panes laid side by side get equal widths,
/// panes stacked get equal heights, however the splits are nested.
///
/// Splits are visited parent first (the preorder `resize_split_by` indexes
/// by), re-reading the tree each time since resizing a split rescales the
/// splits inside it. The resizes reach the server the same way `AdjustPaneSize`
/// does, as pane resizes.
pub(crate) fn balance_panes(tab: &Tab) {
    // how many panes a subtree lays out along `dir`
    fn span(node: &PaneNode, dir: SplitDirection) -> usize {
        match node {
            PaneNode::Split { left, right, node } if node.direction == dir => {
                span(left, dir) + span(right, dir)
            }
            PaneNode::Split { left, right, .. } => span(left, dir).max(span(right, dir)),
            _ => 1,
        }
    }
    // (total cells, first side's cells, first side's span, second side's span), preorder
    fn splits(node: &PaneNode, out: &mut Vec<(usize, usize, usize, usize)>) {
        if let PaneNode::Split { left, right, node } = node {
            let (total, first) = match node.direction {
                SplitDirection::Horizontal => (node.width(), node.first.cols),
                SplitDirection::Vertical => (node.height(), node.first.rows),
            };
            out.push((total, first, span(left, node.direction), span(right, node.direction)));
            splits(left, out);
            splits(right, out);
        }
    }
    let list = |tab: &Tab| {
        let mut v = vec![];
        splits(&tab.codec_pane_tree(), &mut v);
        v
    };
    for i in 0..list(tab).len() {
        let Some(&(total, first, a, b)) = list(tab).get(i) else {
            break;
        };
        let delta = even_first(total, a, b) as isize - first as isize;
        if delta != 0 {
            tab.resize_split_by(i, delta);
        }
    }
}

/// Size of a split's first side so that `a + b` panes (with a one-cell divider
/// between each) share `total` cells evenly, `a` of them on the first side.
fn even_first(total: usize, a: usize, b: usize) -> usize {
    let n = (a + b).max(1);
    ((total + 1) * a + n / 2) / n - 1
}

#[cfg(test)]
mod balance_tests {
    use super::even_first;

    #[test]
    fn halves() {
        // 50 | 50
        assert_eq!(even_first(101, 1, 1), 50);
    }

    #[test]
    fn one_and_two() {
        // 33 | 33 | 33: the first side holds one of three
        assert_eq!(even_first(101, 1, 2), 33);
        // and the second split, inside the 67-cell right side
        assert_eq!(even_first(67, 1, 1), 33);
    }

    #[test]
    fn two_and_one() {
        // 33 | 33 | 33: first side holds two panes and a divider
        assert_eq!(even_first(101, 2, 1), 67);
    }
}
