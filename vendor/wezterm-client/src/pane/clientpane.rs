use crate::domain::ClientInner;
use crate::pane::mousestate::MouseState;
use crate::pane::renderable::{hydrate_lines, RenderableInner, RenderableState};
use anyhow::bail;
use async_trait::async_trait;
use codec::*;
use config::configuration;
use config::keyassignment::ScrollbackEraseMode;
use mux::domain::DomainId;
use mux::pane::{
    alloc_pane_id, CachePolicy, CloseReason, ForEachPaneLogicalLine, LogicalLine, Pane, PaneId,
    Pattern, SearchResult, WithPaneLines,
};
use mux::renderable::{RenderableDimensions, StableCursorPosition};
use mux::tab::TabId;
use mux::{Mux, MuxNotification};
use parking_lot::{MappedMutexGuard, Mutex, MutexGuard};
use rangeset::RangeSet;
use ratelim::RateLimiter;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use termwiz::input::KeyEvent;
use termwiz::surface::SequenceNo;
use url::Url;
use wezterm_dynamic::Value;
use wezterm_term::color::ColorPalette;
use wezterm_term::{
    Alert, Clipboard, KeyCode, KeyModifiers, Line, MouseEvent, Progress, StableRowIndex,
    TerminalConfiguration, TerminalSize,
};

pub struct ClientPane {
    client: Arc<ClientInner>,
    local_pane_id: PaneId,
    pub remote_pane_id: PaneId,
    // wezzy: atomic so a resync can follow the pane when it moves to another
    // tab; shared so a queued resize names the tab current when it goes out
    remote_tab_id: Arc<AtomicUsize>,
    pub renderable: Mutex<RenderableState>,
    configured_palette: Mutex<ColorPalette>,
    palette: Mutex<ColorPalette>,
    application_palette: Mutex<bool>,
    writer: Mutex<PaneWriter>,
    mouse: Arc<Mutex<MouseState>>,
    clipboard: Mutex<Option<Arc<dyn Clipboard>>>,
    mouse_grabbed: Mutex<bool>,
    alt_screen_active: Mutex<bool>,
    ignore_next_kill: Mutex<bool>,
    /// wezzy: KillPane went to the server. The app kills a closing pane before
    /// it resizes the survivors, and `Mux::remove_pane` kills it again.
    kill_sent: AtomicBool,
    /// wezzy: the sizes we asked the server for, kept apart from
    /// `RenderableInner::dimensions` (which the server's deltas overwrite).
    resize_queue: Arc<Mutex<ResizeQueue>>,
    user_vars: Mutex<HashMap<String, String>>,
    config: Mutex<Option<Arc<dyn TerminalConfiguration>>>,
    unseen_output: Mutex<bool>,
    progress: Mutex<Progress>,
}

/// wezzy: the part of a size the server cares about (dpi isn't in its layout).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct SizeKey {
    rows: usize,
    cols: usize,
    pixel_width: usize,
    pixel_height: usize,
}

impl From<&TerminalSize> for SizeKey {
    fn from(size: &TerminalSize) -> Self {
        Self {
            rows: size.rows,
            cols: size.cols,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

/// wezzy: one `Resize` in flight per pane, latest wins. A divider drag
/// resizes every step; without this each step's PDUs queue up behind the
/// last.
#[derive(Debug)]
struct ResizeQueue {
    /// The newest size handed to `send_size` (in flight or pending). `None`
    /// after a resize failed, so the next `resize()` sends again.
    requested: Option<SizeKey>,
    in_flight: bool,
    /// Sent when the in-flight one is answered.
    pending: Option<TerminalSize>,
}

impl ResizeQueue {
    fn new(size: &TerminalSize) -> Self {
        Self {
            requested: Some(size.into()),
            in_flight: false,
            pending: None,
        }
    }

    /// Whether `size` differs from what we last asked the server for.
    fn differs(&self, size: &TerminalSize) -> bool {
        self.requested != Some(size.into())
    }

    /// Queue `size`. Returns it when it should go out now (nothing in
    /// flight); otherwise it replaces any pending size.
    fn push(&mut self, size: TerminalSize) -> Option<TerminalSize> {
        self.requested = Some((&size).into());
        if self.in_flight {
            self.pending = Some(size);
            None
        } else {
            self.in_flight = true;
            Some(size)
        }
    }

    /// The in-flight resize was answered (`ok` false: it failed). Returns the
    /// next size to send, or `None` when the queue is idle again.
    fn complete(&mut self, ok: bool) -> Option<TerminalSize> {
        match self.pending.take() {
            Some(next) => Some(next),
            None => {
                self.in_flight = false;
                if !ok {
                    self.requested = None;
                }
                None
            }
        }
    }
}

impl ClientPane {
    pub fn remote_tab_id(&self) -> TabId {
        self.remote_tab_id.load(Ordering::Relaxed)
    }

    /// wezzy: tell the server this pane's size even if we think it already
    /// knows, to repair a server layout that disagrees with ours. If a resize
    /// for this pane is unanswered, this one goes when it returns (replacing
    /// any other waiting); one layout op covers the whole run.
    pub fn send_size(&self, size: TerminalSize) {
        let start = self.resize_queue.lock().push(size);
        if let Some(size) = start {
            let client = Arc::clone(&self.client);
            let remote_pane_id = self.remote_pane_id;
            let remote_tab_id = Arc::clone(&self.remote_tab_id);
            let queue = Arc::clone(&self.resize_queue);
            client.begin_layout_op();
            promise::spawn::spawn(async move {
                let mut size = size;
                loop {
                    let res = client
                        .client
                        .resize(Resize {
                            containing_tab_id: remote_tab_id.load(Ordering::Relaxed),
                            pane_id: remote_pane_id,
                            size,
                        })
                        .await;
                    if let Err(err) = &res {
                        log::debug!("resize of remote pane {remote_pane_id} failed: {err:#}");
                    }
                    let next = queue.lock().complete(res.is_ok());
                    match next {
                        Some(next) => size = next,
                        None => break,
                    }
                }
                client.end_layout_op();
            })
            .detach();
        }
        self.renderable.lock().inner.borrow_mut().update_last_send();
    }

    /// wezzy: whether our domain was detached, so its panes are being torn
    /// down here and nothing about them should reach the server.
    fn domain_detached(&self) -> bool {
        Mux::get()
            .get_domain(self.client.local_domain_id)
            .is_some_and(|d| d.state() == mux::domain::DomainState::Detached)
    }

    /// wezzy: the server moved this pane to another tab (PaneSelect, move to
    /// new tab). Resize and zoom name the containing tab, so keep it current.
    pub fn set_remote_tab_id(&self, tab_id: TabId) {
        self.remote_tab_id.store(tab_id, Ordering::Relaxed);
    }

    /// wezzy: user vars set before we attached never reach us (they're not
    /// part of the initial sync). Lets the app fill in ones it can infer, e.g.
    /// IS_NVIM from the foreground process. Never overwrites a reported value.
    pub fn seed_user_var(&self, name: &str, value: &str) {
        self.user_vars
            .lock()
            .entry(name.to_string())
            .or_insert_with(|| value.to_string());
    }

    pub fn new(
        client: &Arc<ClientInner>,
        remote_tab_id: TabId,
        remote_pane_id: PaneId,
        size: TerminalSize,
        title: &str,
    ) -> Self {
        let local_pane_id = alloc_pane_id();
        let writer = PaneWriter {
            client: Arc::clone(client),
            remote_pane_id,
        };

        let mouse = Arc::new(Mutex::new(MouseState::new(
            remote_pane_id,
            client.client.clone(),
        )));

        let fetch_limiter =
            RateLimiter::new(|config| config.ratelimit_mux_line_prefetches_per_second);

        let render = RenderableState {
            inner: RefCell::new(RenderableInner::new(
                client,
                remote_pane_id,
                local_pane_id,
                RenderableDimensions {
                    cols: size.cols as _,
                    viewport_rows: size.rows as _,
                    scrollback_rows: size.rows as _,
                    physical_top: 0,
                    scrollback_top: 0,
                    dpi: size.dpi,
                    pixel_width: size.pixel_width,
                    pixel_height: size.pixel_height,
                    reverse_video: false,
                },
                title,
                fetch_limiter,
            )),
        };

        let config = configuration();
        let palette: ColorPalette = config.resolved_palette.clone().into();

        // Advise the server of our palette preference
        promise::spawn::spawn({
            let palette = palette.clone();
            let client = Arc::clone(client);
            async move {
                client
                    .client
                    .set_configured_palette_for_pane(SetPalette {
                        pane_id: remote_pane_id,
                        palette,
                    })
                    .await
            }
        })
        .detach();

        Self {
            client: Arc::clone(client),
            mouse,
            remote_pane_id,
            local_pane_id,
            remote_tab_id: Arc::new(AtomicUsize::new(remote_tab_id)),
            application_palette: Mutex::new(false),
            renderable: Mutex::new(render),
            writer: Mutex::new(writer),
            configured_palette: Mutex::new(palette.clone()),
            palette: Mutex::new(palette),
            clipboard: Mutex::new(None),
            mouse_grabbed: Mutex::new(false),
            alt_screen_active: Mutex::new(false),
            ignore_next_kill: Mutex::new(false),
            kill_sent: AtomicBool::new(false),
            resize_queue: Arc::new(Mutex::new(ResizeQueue::new(&size))),
            unseen_output: Mutex::new(false),
            user_vars: Mutex::new(HashMap::new()),
            config: Mutex::new(None),
            progress: Mutex::new(Progress::default()),
        }
    }

    pub async fn process_unilateral(&self, pdu: Pdu) -> anyhow::Result<()> {
        match pdu {
            Pdu::GetPaneRenderChangesResponse(mut delta) => {
                *self.mouse_grabbed.lock() = delta.mouse_grabbed;
                *self.alt_screen_active.lock() = delta.alt_screen_active;

                let bonus_lines = std::mem::take(&mut delta.bonus_lines);
                let client = { Arc::clone(&self.renderable.lock().inner.borrow().client) };
                let bonus_lines = hydrate_lines(client, delta.pane_id, bonus_lines).await;

                self.renderable
                    .lock()
                    .inner
                    .borrow_mut()
                    .apply_changes_to_surface(delta, bonus_lines);
            }
            Pdu::SetClipboard(SetClipboard {
                clipboard,
                selection,
                ..
            }) => match self.clipboard.lock().as_ref() {
                Some(clip) => {
                    log::debug!(
                        "Pdu::SetClipboard pane={} remote={} {:?} {:?}",
                        self.local_pane_id,
                        self.remote_pane_id,
                        selection,
                        clipboard
                    );
                    clip.set_contents(selection, clipboard)?;
                }
                None => {
                    log::error!("ClientPane: Ignoring SetClipboard request {:?}", clipboard);
                }
            },
            Pdu::SetPalette(SetPalette { palette, .. }) => {
                *self.application_palette.lock() = palette != *self.configured_palette.lock();

                *self.palette.lock() = palette;
                let mux = Mux::get();
                self.renderable.lock().inner.borrow_mut().make_all_stale();
                mux.notify(MuxNotification::Alert {
                    pane_id: self.local_pane_id,
                    alert: Alert::PaletteChanged,
                });
            }
            Pdu::NotifyAlert(NotifyAlert { alert, .. }) => {
                let mux = Mux::get();
                match &alert {
                    Alert::SetUserVar { name, value } => {
                        self.user_vars.lock().insert(name.clone(), value.clone());
                        // wezzy: publish like LocalPane does
                        mux.notify(MuxNotification::Alert {
                            pane_id: self.local_pane_id,
                            alert: alert.clone(),
                        });
                    }
                    Alert::OutputSinceFocusLost => {
                        *self.unseen_output.lock() = true;
                        mux.notify(MuxNotification::Alert {
                            pane_id: self.local_pane_id,
                            alert: Alert::OutputSinceFocusLost,
                        });
                    }
                    Alert::Progress(progress) => {
                        *self.progress.lock() = progress.clone();
                        mux.notify(MuxNotification::Alert {
                            pane_id: self.local_pane_id,
                            alert: Alert::Progress(progress.clone()),
                        });
                    }
                    _ => {}
                }
                mux.notify(MuxNotification::Alert {
                    pane_id: self.local_pane_id,
                    alert,
                });
            }
            Pdu::PaneRemoved(PaneRemoved { pane_id }) => {
                log::trace!("remote pane {} has been removed", pane_id);
                self.renderable.lock().inner.borrow_mut().dead = true;
                let mux = Mux::get();
                mux.prune_dead_windows();

                self.client.expire_stale_mappings();
            }
            Pdu::PaneFocused(PaneFocused { pane_id }) => {
                // We get here whenever the pane focus is changed on the
                // server. That might be due to the user here in the GUI
                // doing things, or it may be due to a "remote"
                // `wezterm cli activate-pane-direction` or similar call
                // from some other actor.
                // The latter case is the important one: it is desirable
                // for the focus change to be reflected locally after it
                // has been changed on the server, so we work to apply
                // it here.
                log::trace!("advised of remote pane focus: {pane_id}");

                // wezzy: record it first, so applying it locally (which ends
                // in advise_focus) doesn't send it back. Echoes of two quick
                // focus changes otherwise re-sent each other forever. The
                // guard drops at the end of the statement: advise_focus
                // takes the same lock.
                *self.client.focused_remote_pane_id.lock().unwrap() = Some(self.remote_pane_id);

                let mux = Mux::get();
                if let Err(err) = mux.focus_pane_and_containing_tab(self.local_pane_id) {
                    log::error!("Error reconciling remote PaneFocused notification: {err:#}");
                }
            }
            _ => bail!("unhandled unilateral pdu: {:?}", pdu),
        };
        Ok(())
    }

    pub fn remote_pane_id(&self) -> TabId {
        self.remote_pane_id
    }

    /// Arrange to suppress the next Pane::kill call.
    /// This is a bit of a hack that we use when closing a window;
    /// our Domain::local_window_is_closing impl calls this for each
    /// ClientPane in the window so that closing a window effectively
    /// "detaches" the window so that reconnecting later will resume
    /// from where they left off.
    /// It isn't perfect.
    pub fn ignore_next_kill(&self) {
        *self.ignore_next_kill.lock() = true;
    }
}

#[async_trait(?Send)]
impl Pane for ClientPane {
    fn pane_id(&self) -> PaneId {
        self.local_pane_id
    }

    fn get_metadata(&self) -> Value {
        let renderable = self.renderable.lock();
        let inner = renderable.inner.borrow();

        let mut map: BTreeMap<Value, Value> = BTreeMap::new();
        map.insert(
            Value::String("is_tardy".to_string()),
            Value::Bool(inner.is_tardy()),
        );
        map.insert(
            Value::String("since_last_response_ms".to_string()),
            Value::U64(inner.last_recv_time.elapsed().as_millis() as u64),
        );

        Value::Object(map.into())
    }

    fn get_cursor_position(&self) -> StableCursorPosition {
        self.renderable.lock().get_cursor_position()
    }

    fn get_dimensions(&self) -> RenderableDimensions {
        self.renderable.lock().get_dimensions()
    }

    fn with_lines_mut(&self, lines: Range<StableRowIndex>, with_lines: &mut dyn WithPaneLines) {
        mux::pane::impl_with_lines_via_get_lines(self, lines, with_lines);
    }

    fn for_each_logical_line_in_stable_range_mut(
        &self,
        lines: Range<StableRowIndex>,
        for_line: &mut dyn ForEachPaneLogicalLine,
    ) {
        mux::pane::impl_for_each_logical_line_via_get_logical_lines(self, lines, for_line);
    }

    fn get_lines(&self, lines: Range<StableRowIndex>) -> (StableRowIndex, Vec<Line>) {
        self.renderable.lock().get_lines(lines)
    }

    fn get_logical_lines(&self, lines: Range<StableRowIndex>) -> Vec<LogicalLine> {
        mux::pane::impl_get_logical_lines_via_get_lines(self, lines)
    }

    fn get_current_seqno(&self) -> SequenceNo {
        self.renderable.lock().get_current_seqno()
    }

    fn get_changed_since(
        &self,
        lines: Range<StableRowIndex>,
        seqno: SequenceNo,
    ) -> RangeSet<StableRowIndex> {
        self.renderable.lock().get_changed_since(lines, seqno)
    }

    fn set_clipboard(&self, clipboard: &Arc<dyn Clipboard>) {
        self.clipboard.lock().replace(Arc::clone(clipboard));
    }

    fn get_title(&self) -> String {
        let renderable = self.renderable.lock();
        let inner = renderable.inner.borrow();
        inner.title.clone()
    }

    fn get_progress(&self) -> Progress {
        self.progress.lock().clone()
    }

    fn send_paste(&self, text: &str) -> anyhow::Result<()> {
        let client = Arc::clone(&self.client);
        let remote_pane_id = self.remote_pane_id;
        self.renderable
            .lock()
            .inner
            .borrow_mut()
            .predict_from_paste(text);

        let data = text.to_owned();
        promise::spawn::spawn(async move {
            client
                .client
                .send_paste(SendPaste {
                    pane_id: remote_pane_id,
                    data,
                })
                .await
        })
        .detach();
        self.renderable.lock().inner.borrow_mut().update_last_send();
        Ok(())
    }

    fn reader(&self) -> anyhow::Result<Option<Box<dyn std::io::Read + Send>>> {
        Ok(None)
    }

    fn writer(&self) -> MappedMutexGuard<'_, dyn std::io::Write> {
        MutexGuard::map(self.writer.lock(), |writer| {
            let w: &mut dyn std::io::Write = writer;
            w
        })
    }

    fn set_zoomed(&self, zoomed: bool) {
        let render = self.renderable.lock();
        let mut inner = render.inner.borrow_mut();
        let client = Arc::clone(&self.client);
        let remote_pane_id = self.remote_pane_id;
        let remote_tab_id = self.remote_tab_id();
        // Invalidate any cached rows on a resize
        inner.make_all_stale();
        promise::spawn::spawn(async move {
            client
                .client
                .set_zoomed(SetPaneZoomed {
                    containing_tab_id: remote_tab_id,
                    pane_id: remote_pane_id,
                    zoomed,
                })
                .await
        })
        .detach();
        inner.update_last_send();
    }

    fn resize(&self, size: TerminalSize) -> anyhow::Result<()> {
        let render = self.renderable.lock();
        let mut inner = render.inner.borrow_mut();

        let cols = size.cols as usize;
        let rows = size.rows as usize;

        // Render at the new size right away.
        if inner.dimensions.cols != cols
            || inner.dimensions.viewport_rows != rows
            || inner.dimensions.pixel_width != size.pixel_width
            || inner.dimensions.pixel_height != size.pixel_height
        {
            inner.dimensions.cols = cols;
            inner.dimensions.viewport_rows = rows;
            inner.dimensions.pixel_width = size.pixel_width;
            inner.dimensions.pixel_height = size.pixel_height;

            // Invalidate any cached rows on a resize
            inner.make_all_stale();
        }
        drop(inner);
        drop(render);

        // wezzy: detaching takes the panes out of their tabs one by one, and
        // each removal grows the panes left behind. Those sizes are ours
        // alone: sent, they'd overwrite the layout the server keeps for the
        // next attach (as `kill` below doesn't send either).
        if self.domain_detached() {
            return Ok(());
        }

        // wezzy: whether the server needs it is decided by what we last asked
        // for, not by `dimensions`, which a stale delta may have overwritten.
        if self.resize_queue.lock().differs(&size) {
            self.send_size(size);
        }
        Ok(())
    }

    async fn search(
        &self,
        pattern: Pattern,
        range: Range<StableRowIndex>,
        limit: Option<u32>,
    ) -> anyhow::Result<Vec<SearchResult>> {
        match self
            .client
            .client
            .search_scrollback(SearchScrollbackRequest {
                pane_id: self.remote_pane_id,
                pattern,
                range,
                limit,
            })
            .await
        {
            Ok(SearchScrollbackResponse { results }) => Ok(results),
            Err(e) => Err(e),
        }
    }

    fn key_down(&self, key: KeyCode, mods: KeyModifiers) -> anyhow::Result<()> {
        let input_serial;
        {
            let renderable = self.renderable.lock();
            let mut inner = renderable.inner.borrow_mut();
            inner.input_serial = InputSerial::now();
            input_serial = inner.input_serial;
            inner.predict_from_key_event(key, mods);
        }
        let client = Arc::clone(&self.client);
        let remote_pane_id = self.remote_pane_id;
        promise::spawn::spawn(async move {
            client
                .client
                .key_down(SendKeyDown {
                    pane_id: remote_pane_id,
                    event: KeyEvent {
                        key,
                        modifiers: mods,
                    },
                    input_serial,
                })
                .await
        })
        .detach();
        self.renderable.lock().inner.borrow_mut().update_last_send();
        Ok(())
    }

    fn key_up(&self, _key: KeyCode, _mods: KeyModifiers) -> anyhow::Result<()> {
        // TODO: decide how to handle key_up for mux client
        Ok(())
    }

    fn kill(&self) {
        {
            let mut ignore = self.ignore_next_kill.lock();
            if *ignore {
                *ignore = false;
                return;
            }
        }
        // wezzy: the server already removed it (PaneRemoved, or a poll found
        // it gone): nothing to kill.
        if self.renderable.lock().inner.borrow().dead {
            return;
        }
        let client = Arc::clone(&self.client);
        let remote_pane_id = self.remote_pane_id;

        // We only want to ask the server to kill the pane if the user
        // explicitly requested it to die.
        // Domain detaching can implicitly call Pane::kill on the panes
        // in the domain, so we need to check here whether the domain is
        // in the detached state; if so then we must skip sending the
        // kill to the server.
        let send_kill = !self.domain_detached();

        // wezzy: at most one KillPane per pane; the app kills a closing pane
        // before Mux::remove_pane does.
        if send_kill && !self.kill_sent.swap(true, Ordering::SeqCst) {
            client.begin_layout_op();
            promise::spawn::spawn(async move {
                let res = client
                    .client
                    .kill_pane(KillPane {
                        pane_id: remote_pane_id,
                    })
                    .await;
                client.end_layout_op();
                res
            })
            .detach();
        }
    }

    fn mouse_event(&self, event: MouseEvent) -> anyhow::Result<()> {
        self.mouse.lock().append(event);
        if MouseState::next(Arc::clone(&self.mouse)) {
            self.renderable.lock().inner.borrow_mut().update_last_send();
        }
        Ok(())
    }

    fn is_dead(&self) -> bool {
        self.renderable.lock().inner.borrow().dead
    }

    fn palette(&self) -> ColorPalette {
        self.palette.lock().clone()
    }

    fn domain_id(&self) -> DomainId {
        self.client.local_domain_id
    }

    fn is_mouse_grabbed(&self) -> bool {
        *self.mouse_grabbed.lock()
    }

    fn is_alt_screen_active(&self) -> bool {
        *self.alt_screen_active.lock()
    }

    fn get_current_working_dir(&self, _policy: CachePolicy) -> Option<Url> {
        self.renderable.lock().inner.borrow().working_dir.clone()
    }

    fn focus_changed(&self, focused: bool) {
        if focused {
            self.advise_focus();
            *self.unseen_output.lock() = false;
        }
    }

    fn erase_scrollback(&self, erase_mode: ScrollbackEraseMode) {
        let client = Arc::clone(&self.client);
        let remote_pane_id = self.remote_pane_id;
        promise::spawn::spawn(async move {
            client
                .client
                .erase_scrollback(EraseScrollbackRequest {
                    pane_id: remote_pane_id,
                    erase_mode,
                })
                .await
        })
        .detach();
    }

    fn advise_focus(&self) {
        let mut focused_pane = self.client.focused_remote_pane_id.lock().unwrap();
        if *focused_pane != Some(self.remote_pane_id) {
            focused_pane.replace(self.remote_pane_id);
            let client = Arc::clone(&self.client);
            let remote_pane_id = self.remote_pane_id;
            promise::spawn::spawn(async move {
                client
                    .client
                    .set_focused_pane_id(SetFocusedPane {
                        pane_id: remote_pane_id,
                    })
                    .await
            })
            .detach();
        }
    }

    fn has_unseen_output(&self) -> bool {
        *self.unseen_output.lock()
    }

    fn can_close_without_prompting(&self, reason: CloseReason) -> bool {
        match reason {
            CloseReason::Window => true,
            CloseReason::Tab => false,
            CloseReason::Pane => false,
        }
    }

    fn copy_user_vars(&self) -> HashMap<String, String> {
        self.user_vars.lock().clone()
    }

    fn set_config(&self, config: Arc<dyn TerminalConfiguration>) {
        let palette = config.color_palette();
        // If the application running in the pane hasn't changed the
        // palette through escape sequences, speculatively adopt the
        // new palette so that it updates with the lowest latency.
        if !*self.application_palette.lock() {
            *self.palette.lock() = palette.clone();
        }
        *self.configured_palette.lock() = palette.clone();

        // and now send the color palette to the server
        let client = Arc::clone(&self.client);
        let remote_pane_id = self.remote_pane_id;
        promise::spawn::spawn(async move {
            client
                .client
                .set_configured_palette_for_pane(SetPalette {
                    pane_id: remote_pane_id,
                    palette,
                })
                .await
        })
        .detach();
        self.config.lock().replace(config);
    }

    fn get_config(&self) -> Option<Arc<dyn TerminalConfiguration>> {
        self.config.lock().clone()
    }
}

struct PaneWriter {
    client: Arc<ClientInner>,
    remote_pane_id: TabId,
}

impl std::io::Write for PaneWriter {
    /// wezzy: fire and forget, like send_paste; upstream blocked the UI
    /// thread on the round trip (for good if an ssh host stalled). Order is
    /// kept: spawned tasks start in spawn order on the main queue and each
    /// puts its PDU on the client's unbounded channel on its first poll.
    fn write(&mut self, data: &[u8]) -> Result<usize, std::io::Error> {
        let client = Arc::clone(&self.client);
        let pane_id = self.remote_pane_id;
        let data_vec = data.to_vec();
        promise::spawn::spawn(async move {
            if let Err(err) = client
                .client
                .write_to_pane(WriteToPane {
                    pane_id,
                    data: data_vec,
                })
                .await
            {
                log::error!("write to remote pane {pane_id} failed: {err:#}");
            }
        })
        .detach();
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: usize, rows: usize) -> TerminalSize {
        TerminalSize {
            rows,
            cols,
            pixel_width: cols * 10,
            pixel_height: rows * 20,
            dpi: 72,
        }
    }

    #[test]
    fn initial_size_counts_as_requested() {
        let q = ResizeQueue::new(&size(80, 24));
        assert!(!q.differs(&size(80, 24)));
        assert!(q.differs(&size(81, 24)));
        // dpi alone isn't a layout change
        assert!(!q.differs(&TerminalSize {
            dpi: 144,
            ..size(80, 24)
        }));
    }

    #[test]
    fn one_in_flight_latest_wins() {
        let mut q = ResizeQueue::new(&size(80, 24));
        assert_eq!(q.push(size(70, 24)), Some(size(70, 24)));
        assert_eq!(q.push(size(60, 24)), None);
        assert_eq!(q.push(size(50, 24)), None);
        assert!(!q.differs(&size(50, 24)));
        // the in-flight one returns: only the newest goes next
        assert_eq!(q.complete(true), Some(size(50, 24)));
        assert_eq!(q.complete(true), None);
        assert!(!q.in_flight);
        // idle again: the next one goes straight out
        assert_eq!(q.push(size(40, 24)), Some(size(40, 24)));
    }

    #[test]
    fn forced_resend_of_same_size_still_goes_out() {
        let mut q = ResizeQueue::new(&size(80, 24));
        assert!(!q.differs(&size(80, 24)));
        assert_eq!(q.push(size(80, 24)), Some(size(80, 24)));
        // and while one is in flight, it's queued rather than dropped
        assert_eq!(q.push(size(80, 24)), None);
        assert_eq!(q.complete(true), Some(size(80, 24)));
        assert_eq!(q.complete(true), None);
    }

    #[test]
    fn failed_resize_is_retried_by_next_resize() {
        let mut q = ResizeQueue::new(&size(80, 24));
        assert_eq!(q.push(size(70, 24)), Some(size(70, 24)));
        assert_eq!(q.complete(false), None);
        assert!(q.differs(&size(70, 24)));
    }
}
