use crate::client::Client;
use crate::pane::ClientPane;
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use codec::{ListPanesResponse, SpawnV2, SplitPane};
use config::keyassignment::SpawnTabDomain;
use config::{SshDomain, TlsDomainClient, UnixDomain};
use mux::connui::{ConnectionUI, ConnectionUIParams};
use mux::domain::{alloc_domain_id, Domain, DomainId, DomainState, SplitSource};
use mux::pane::{Pane, PaneId};
use mux::tab::{PaneNode, SplitRequest, Tab, TabId};
use mux::window::WindowId;
use mux::{Mux, MuxNotification};
use portable_pty::CommandBuilder;
use promise::spawn::spawn_into_new_thread;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use wezterm_term::TerminalSize;

pub struct ClientInner {
    pub client: Client,
    pub local_domain_id: DomainId,
    pub local_echo_threshold_ms: Option<u64>,
    pub overlay_lag_indicator: bool,
    remote_to_local_window: Mutex<HashMap<WindowId, WindowId>>,
    remote_to_local_tab: Mutex<HashMap<TabId, TabId>>,
    remote_to_local_pane: Mutex<HashMap<PaneId, PaneId>>,
    pub focused_remote_pane_id: Mutex<Option<PaneId>>,
    /// wezzy: "devbox/" for a host's server. Its workspaces show up locally
    /// with the prefix (two servers can both have a "default") and lose it on
    /// the way back.
    pub workspace_prefix: Option<String>,
    /// wezzy: when each remote tab last had its pane sizes resent after a
    /// refit. Each resend makes the server send TabResized, which resyncs;
    /// if the server's layout stays inconsistent that loops (and floods the
    /// connection) unless it's rate limited.
    refit_resent: Mutex<HashMap<TabId, std::time::Instant>>,
    /// wezzy: remote tabs with a resync scheduled for when their resend
    /// interval ends (a resend was due but rate limited). At most one each.
    refit_retry_pending: Mutex<HashSet<TabId>>,
    /// wezzy: layout changes sent to the server and not answered yet (pane
    /// resizes, kills). While any are in flight the server's tree is older
    /// than ours, so a resync waits for them instead of applying it.
    layout_in_flight: AtomicUsize,
    /// wezzy: a resync was put off by `layout_in_flight`; run it once they land.
    resync_deferred: AtomicBool,
}

/// wezzy: a host's basics, for a command whose environment was cleared
/// (`for_host`), then the command itself ("$@").
const HOST_ENV: &str = r#"u=$(id -un)
[ -n "$PATH" ] || PATH=/usr/local/bin:/usr/bin:/bin
[ -n "$HOME" ] || HOME=$(getent passwd "$u" 2>/dev/null | cut -d: -f6)
[ -n "$HOME" ] || HOME=$(eval "echo ~$u")
[ -n "$SHELL" ] || SHELL=$(getent passwd "$u" 2>/dev/null | cut -d: -f7)
[ -n "$SHELL" ] || SHELL=/bin/sh
[ -n "$LANG" ] || { [ -r /etc/default/locale ] && . /etc/default/locale; [ -n "$LANG" ] || LANG=C.UTF-8; }
USER=$u LOGNAME=$u
export PATH HOME SHELL LANG USER LOGNAME
exec "$@""#;

/// wezzy: a command for a host's server. A CommandBuilder carries this
/// process's whole environment (this Mac's HOME, SHELL, PATH), and the server
/// starts the command with it as is: a shell that isn't on the host, a home
/// that isn't there. Keep only the variables the caller set, and have the
/// host fill in its own basics first. A default program goes as no command,
/// so the server starts its own with its own environment.
fn for_host(
    cmd: Option<CommandBuilder>,
    command_dir: Option<String>,
) -> (Option<CommandBuilder>, Option<String>) {
    let Some(cmd) = cmd else {
        return (None, command_dir);
    };
    let cwd = cmd.get_cwd().cloned();
    if cmd.is_default_prog() {
        let dir = command_dir.or_else(|| cwd.and_then(|c| c.into_string().ok()));
        return (None, dir);
    }
    let mut argv: Vec<std::ffi::OsString> =
        vec!["/bin/sh".into(), "-c".into(), HOST_ENV.into(), "sh".into()];
    argv.extend(cmd.get_argv().iter().cloned());
    let mut out = CommandBuilder::from_argv(argv);
    out.env_clear();
    for (k, v) in cmd.iter_extra_env_as_str() {
        out.env(k, v);
    }
    if let Some(cwd) = cwd {
        out.cwd(cwd);
    }
    (Some(out), command_dir)
}

/// wezzy: how often a tab's pane sizes may be resent after a refit.
const REFIT_RESEND_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

impl ClientInner {
    /// wezzy: `None` when `remote_tab_id`'s pane sizes may be resent now
    /// (and records it), else how long until they may.
    fn resend_sizes_wait(&self, remote_tab_id: TabId) -> Option<std::time::Duration> {
        let now = std::time::Instant::now();
        let mut map = self.refit_resent.lock().unwrap();
        match map.get(&remote_tab_id) {
            Some(last) if now.duration_since(*last) < REFIT_RESEND_INTERVAL => {
                Some(REFIT_RESEND_INTERVAL - now.duration_since(*last))
            }
            _ => {
                map.insert(remote_tab_id, now);
                None
            }
        }
    }

    /// wezzy: a resend for `remote_tab_id` was due but rate limited: resync
    /// once the interval is over, so the server doesn't stay wrong until some
    /// unrelated event. Only one such timer per tab.
    fn schedule_refit_retry(
        self: &Arc<Self>,
        remote_tab_id: TabId,
        wait: std::time::Duration,
    ) {
        if !self.refit_retry_pending.lock().unwrap().insert(remote_tab_id) {
            return;
        }
        let weak = Arc::downgrade(self);
        let domain_id = self.local_domain_id;
        promise::spawn::spawn_into_main_thread(async move {
            smol::Timer::after(wait).await;
            if let Some(inner) = weak.upgrade() {
                inner.refit_retry_pending.lock().unwrap().remove(&remote_tab_id);
            }
            let mux = Mux::try_get().ok_or_else(|| anyhow!("no more mux"))?;
            let domain = mux
                .get_domain(domain_id)
                .ok_or_else(|| anyhow!("no such domain {domain_id}"))?;
            match domain.downcast_ref::<ClientDomain>() {
                Some(d) => d.resync().await,
                None => Ok(()),
            }
        })
        .detach();
    }

    /// wezzy: whether a resync should wait for our in-flight layout changes
    /// (and is now flagged to run when the last one lands).
    fn defer_resync(&self) -> bool {
        if self.layout_in_flight.load(Ordering::SeqCst) == 0 {
            return false;
        }
        self.resync_deferred.store(true, Ordering::SeqCst);
        if self.layout_in_flight.load(Ordering::SeqCst) > 0 {
            return true;
        }
        // they landed meanwhile: whoever finished may have missed the flag
        self.resync_deferred.store(false, Ordering::SeqCst);
        false
    }

    /// wezzy: a layout change is about to go to the server.
    pub(crate) fn begin_layout_op(&self) {
        self.layout_in_flight.fetch_add(1, Ordering::SeqCst);
    }

    /// wezzy: the server answered a layout change. When it was the last one,
    /// run the resync it held back.
    pub(crate) fn end_layout_op(&self) {
        if self.layout_in_flight.fetch_sub(1, Ordering::SeqCst) == 1
            && self.resync_deferred.swap(false, Ordering::SeqCst)
        {
            let domain_id = self.local_domain_id;
            promise::spawn::spawn_into_main_thread(async move {
                let mux = Mux::try_get().ok_or_else(|| anyhow!("no more mux"))?;
                let domain = mux
                    .get_domain(domain_id)
                    .ok_or_else(|| anyhow!("no such domain {domain_id}"))?;
                match domain.downcast_ref::<ClientDomain>() {
                    Some(d) => d.resync().await,
                    None => Ok(()),
                }
            })
            .detach();
        }
    }

    fn remote_to_local_window(&self, remote_window_id: WindowId) -> Option<WindowId> {
        let map = self.remote_to_local_window.lock().unwrap();
        map.get(&remote_window_id).cloned()
    }

    pub(crate) fn expire_stale_mappings(&self) {
        let mux = Mux::get();

        self.remote_to_local_pane
            .lock()
            .unwrap()
            .retain(|_remote_pane_id, local_pane_id| mux.get_pane(*local_pane_id).is_some());

        self.remote_to_local_tab
            .lock()
            .unwrap()
            .retain(
                |remote_tab_id, local_tab_id| match mux.get_tab(*local_tab_id) {
                    Some(tab) => {
                        for pos in tab.iter_panes_ignoring_zoom() {
                            if pos.pane.domain_id() == self.local_domain_id {
                                return true;
                            }
                        }
                        log::trace!(
                            "expire_stale_mappings: domain: {}. will remove \
                            {remote_tab_id} -> {local_tab_id} tab mapping \
                            because tab contains no panes from this domain",
                            self.local_domain_id,
                        );
                        false
                    }
                    None => false,
                },
            );

        self.remote_to_local_window
            .lock()
            .unwrap()
            .retain(
                |_remote_window_id, local_window_id| match mux.get_window(*local_window_id) {
                    Some(w) => {
                        for tab in w.iter_tabs() {
                            for pos in tab.iter_panes_ignoring_zoom() {
                                if pos.pane.domain_id() == self.local_domain_id {
                                    return true;
                                }
                            }
                        }
                        false
                    }
                    None => false,
                },
            );
    }

    fn record_remote_to_local_window_mapping(
        &self,
        remote_window_id: WindowId,
        local_window_id: WindowId,
    ) {
        let mut map = self.remote_to_local_window.lock().unwrap();
        map.insert(remote_window_id, local_window_id);
        log::trace!(
            "record_remote_to_local_window_mapping: {} -> {}",
            remote_window_id,
            local_window_id
        );
    }

    fn local_to_remote_tab(&self, local_tab_id: TabId) -> Option<TabId> {
        let map = self.remote_to_local_tab.lock().unwrap();
        for (remote, local) in map.iter() {
            if *local == local_tab_id {
                return Some(*remote);
            }
        }
        None
    }

    fn local_to_remote_window(&self, local_window_id: WindowId) -> Option<WindowId> {
        let map = self.remote_to_local_window.lock().unwrap();
        for (remote, local) in map.iter() {
            if *local == local_window_id {
                return Some(*remote);
            }
        }
        None
    }

    pub fn remote_to_local_pane_id(&self, remote_pane_id: PaneId) -> Option<TabId> {
        let mut pane_map = self.remote_to_local_pane.lock().unwrap();

        if let Some(id) = pane_map.get(&remote_pane_id) {
            return Some(*id);
        }

        let mux = Mux::get();

        for pane in mux.iter_panes() {
            if pane.domain_id() != self.local_domain_id {
                continue;
            }
            if let Some(pane) = pane.downcast_ref::<ClientPane>() {
                if pane.remote_pane_id() == remote_pane_id {
                    let local_pane_id = pane.pane_id();
                    pane_map.insert(remote_pane_id, local_pane_id);
                    return Some(local_pane_id);
                }
            }
        }
        None
    }
    pub fn remove_old_pane_mapping(&self, remote_pane_id: PaneId) {
        let mut pane_map = self.remote_to_local_pane.lock().unwrap();
        pane_map.remove(&remote_pane_id);
    }

    pub fn remove_old_tab_mapping(&self, remote_tab_id: TabId) {
        let mut tab_map = self.remote_to_local_tab.lock().unwrap();
        let old = tab_map.remove(&remote_tab_id);
        log::trace!("remove_old_tab_mapping: {remote_tab_id} -> {old:?}");
    }

    fn record_remote_to_local_tab_mapping(&self, remote_tab_id: TabId, local_tab_id: TabId) {
        let mut map = self.remote_to_local_tab.lock().unwrap();
        let prior = map.insert(remote_tab_id, local_tab_id);
        log::trace!(
            "record_remote_to_local_tab_mapping: {} -> {} \
             (prior={prior:?}, domain={})",
            remote_tab_id,
            local_tab_id,
            self.local_domain_id,
        );
    }

    pub fn remote_to_local_tab_id(&self, remote_tab_id: TabId) -> Option<TabId> {
        let map = self.remote_to_local_tab.lock().unwrap();
        map.get(&remote_tab_id).copied()
    }

    pub fn is_local(&self) -> bool {
        self.client.is_local
    }
}

#[derive(Clone, Debug)]
pub enum ClientDomainConfig {
    Unix(UnixDomain),
    Tls(TlsDomainClient),
    Ssh(SshDomain),
}

impl ClientDomainConfig {
    pub fn name(&self) -> &str {
        match self {
            ClientDomainConfig::Unix(unix) => &unix.name,
            ClientDomainConfig::Tls(tls) => &tls.name,
            ClientDomainConfig::Ssh(ssh) => &ssh.name,
        }
    }

    pub fn local_echo_threshold_ms(&self) -> Option<u64> {
        match self {
            ClientDomainConfig::Unix(unix) => unix.local_echo_threshold_ms,
            ClientDomainConfig::Tls(tls) => tls.local_echo_threshold_ms,
            ClientDomainConfig::Ssh(ssh) => ssh.local_echo_threshold_ms,
        }
    }

    pub fn overlay_lag_indicator(&self) -> bool {
        match self {
            ClientDomainConfig::Unix(unix) => unix.overlay_lag_indicator,
            ClientDomainConfig::Tls(tls) => tls.overlay_lag_indicator,
            ClientDomainConfig::Ssh(ssh) => ssh.overlay_lag_indicator,
        }
    }

    pub fn label(&self) -> String {
        match self {
            ClientDomainConfig::Unix(unix) => format!("unix mux {}", unix.socket_path().display()),
            ClientDomainConfig::Tls(tls) => format!("TLS mux {}", tls.remote_address),
            ClientDomainConfig::Ssh(ssh) => {
                if let Some(user) = &ssh.username {
                    format!("SSH mux {}@{}", user, ssh.remote_address)
                } else {
                    format!("SSH mux {}", ssh.remote_address)
                }
            }
        }
    }

    pub fn connect_automatically(&self) -> bool {
        match self {
            ClientDomainConfig::Unix(unix) => unix.connect_automatically,
            ClientDomainConfig::Tls(tls) => tls.connect_automatically,
            ClientDomainConfig::Ssh(ssh) => ssh.connect_automatically,
        }
    }
}

impl ClientInner {
    pub fn new(
        local_domain_id: DomainId,
        client: Client,
        local_echo_threshold_ms: Option<u64>,
        overlay_lag_indicator: bool,
    ) -> Self {
        Self {
            client,
            local_domain_id,
            local_echo_threshold_ms,
            overlay_lag_indicator,
            remote_to_local_window: Mutex::new(HashMap::new()),
            remote_to_local_tab: Mutex::new(HashMap::new()),
            remote_to_local_pane: Mutex::new(HashMap::new()),
            focused_remote_pane_id: Mutex::new(None),
            workspace_prefix: None,
            refit_resent: Mutex::new(HashMap::new()),
            refit_retry_pending: Mutex::new(HashSet::new()),
            layout_in_flight: AtomicUsize::new(0),
            resync_deferred: AtomicBool::new(false),
        }
    }

    /// wezzy: the server's workspace name as it shows here.
    pub fn local_workspace(&self, remote: &str) -> String {
        match &self.workspace_prefix {
            Some(p) => format!("{p}{remote}"),
            None => remote.to_string(),
        }
    }

    /// wezzy: a workspace name here as the server knows it.
    pub fn remote_workspace(&self, local: &str) -> String {
        match &self.workspace_prefix {
            Some(p) => local.strip_prefix(p.as_str()).unwrap_or(local).to_string(),
            None => local.to_string(),
        }
    }
}

pub struct ClientDomain {
    config: ClientDomainConfig,
    label: String,
    inner: Mutex<Option<Arc<ClientInner>>>,
    local_domain_id: DomainId,
    workspace_prefix: Option<String>,
}

async fn update_remote_workspace(
    local_domain_id: DomainId,
    pdu: codec::SetWindowWorkspace,
) -> anyhow::Result<()> {
    let inner = ClientDomain::get_client_inner_for_domain(local_domain_id)?;
    inner.client.set_window_workspace(pdu).await?;
    Ok(())
}

fn mux_notify_client_domain(local_domain_id: DomainId, notif: MuxNotification) -> bool {
    let mux = Mux::get();
    let domain = match mux.get_domain(local_domain_id) {
        Some(domain) => domain,
        None => return false,
    };
    let client_domain = match domain.downcast_ref::<ClientDomain>() {
        Some(c) => c,
        None => return false,
    };

    match notif {
        MuxNotification::ActiveWorkspaceChanged(_client_id) => {
            // TODO: advice remote host of interesting workspaces
        }
        MuxNotification::WorkspaceRenamed {
            old_workspace,
            new_workspace,
        } => {
            if let Some(inner) = client_domain.inner() {
                // wezzy: only a workspace with one of this server's windows
                // (now under the new name); with several servers, renaming one
                // host's "default" mustn't rename another's
                let mux = Mux::get();
                let ours = mux.iter_windows().into_iter().any(|w| {
                    client_domain.local_to_remote_window_id(w).is_some()
                        && mux.get_window(w).is_some_and(|w| w.get_workspace() == new_workspace)
                });
                if ours {
                    let old_workspace = inner.remote_workspace(&old_workspace);
                    let new_workspace = inner.remote_workspace(&new_workspace);
                    promise::spawn::spawn(async move {
                        inner
                            .client
                            .rename_workspace(codec::RenameWorkspace {
                                old_workspace,
                                new_workspace,
                            })
                            .await
                    })
                    .detach();
                }
            }
        }
        MuxNotification::WindowWorkspaceChanged(window_id) => {
            // Mux::get_window() may trigger a borrow error if called
            // immediately; defer the bulk of this work.
            // <https://github.com/wezterm/wezterm/issues/2638>
            promise::spawn::spawn_into_main_thread(async move {
                let mux = Mux::get();
                let domain = match mux.get_domain(local_domain_id) {
                    Some(domain) => domain,
                    None => return,
                };
                let domain = match domain.downcast_ref::<ClientDomain>() {
                    Some(domain) => domain,
                    None => return,
                };
                if let Some(remote_window_id) = domain.local_to_remote_window_id(window_id) {
                    if let Some(workspace) = mux
                        .get_window(window_id)
                        .map(|w| w.get_workspace().to_string())
                    {
                        let workspace = match domain.inner() {
                            Some(inner) => inner.remote_workspace(&workspace),
                            None => workspace,
                        };
                        promise::spawn::spawn_into_main_thread(async move {
                            let request = codec::SetWindowWorkspace {
                                window_id: remote_window_id,
                                workspace,
                            };
                            let _ = update_remote_workspace(local_domain_id, request).await;
                        })
                        .detach();
                    }
                } else {
                    log::debug!(
                        "local window id {window_id} has no known remote window \
                        id while reconciling a local WindowWorkspaceChanged event"
                    );
                }
            })
            .detach();
        }
        MuxNotification::TabTitleChanged { tab_id, title } => {
            if let Some(remote_tab_id) = client_domain.local_to_remote_tab_id(tab_id) {
                if let Some(inner) = client_domain.inner() {
                    promise::spawn::spawn(async move {
                        inner
                            .client
                            .set_tab_title(codec::TabTitleChanged {
                                tab_id: remote_tab_id,
                                title,
                            })
                            .await
                    })
                    .detach();
                }
            }
        }
        MuxNotification::WindowTitleChanged {
            window_id,
            title: _,
        } => {
            if let Some(remote_window_id) = client_domain.local_to_remote_window_id(window_id) {
                if let Some(inner) = client_domain.inner() {
                    promise::spawn::spawn_into_main_thread(async move {
                        // De-bounce the title propagation.
                        // There is a bit of a race condition with these async
                        // updates that can trigger a cycle of WindowTitleChanged
                        // PDUs being exchanged between client and server if the
                        // title is changed twice in quick succession.
                        // To avoid that, here on the client, we wait a second
                        // and then report the now-current name of the window, rather
                        // than propagating the title encoded in the MuxNotification.
                        smol::Timer::after(std::time::Duration::from_secs(1)).await;
                        if let Some(mux) = Mux::try_get() {
                            let title = mux
                                .get_window(window_id)
                                .map(|win| win.get_title().to_string());
                            if let Some(title) = title {
                                inner
                                    .client
                                    .set_window_title(codec::WindowTitleChanged {
                                        window_id: remote_window_id,
                                        title,
                                    })
                                    .await?;
                            }
                        }
                        anyhow::Result::<()>::Ok(())
                    })
                    .detach();
                }
            }
        }
        _ => {}
    }
    true
}

impl ClientDomain {
    pub fn new(config: ClientDomainConfig) -> Self {
        let local_domain_id = alloc_domain_id();
        let label = config.label();
        Mux::get().subscribe(move |notif| mux_notify_client_domain(local_domain_id, notif));
        Self {
            config,
            label,
            inner: Mutex::new(None),
            local_domain_id,
            workspace_prefix: None,
        }
    }

    /// wezzy: show this server's workspaces as `<prefix><name>`.
    pub fn with_workspace_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.workspace_prefix = Some(prefix.into());
        self
    }

    pub fn workspace_prefix(&self) -> Option<&str> {
        self.workspace_prefix.as_deref()
    }

    fn inner(&self) -> Option<Arc<ClientInner>> {
        self.inner.lock().unwrap().as_ref().map(Arc::clone)
    }

    pub fn connect_automatically(&self) -> bool {
        self.config.connect_automatically()
    }

    pub fn perform_detach(&self) {
        log::info!("detached domain {}", self.local_domain_id);
        self.inner.lock().unwrap().take();
        let mux = Mux::get();
        mux.domain_was_detached(self.local_domain_id);
    }

    pub fn remote_to_local_pane_id(&self, remote_pane_id: TabId) -> Option<TabId> {
        let inner = self.inner()?;
        inner.remote_to_local_pane_id(remote_pane_id)
    }

    pub fn remote_to_local_window_id(&self, remote_window_id: WindowId) -> Option<WindowId> {
        let inner = self.inner()?;
        inner.remote_to_local_window(remote_window_id)
    }

    pub fn local_to_remote_window_id(&self, local_window_id: WindowId) -> Option<WindowId> {
        let inner = self.inner()?;
        inner.local_to_remote_window(local_window_id)
    }

    pub fn local_to_remote_tab_id(&self, local_tab_id: TabId) -> Option<TabId> {
        let inner = self.inner()?;
        inner.local_to_remote_tab(local_tab_id)
    }

    pub fn get_client_inner_for_domain(domain_id: DomainId) -> anyhow::Result<Arc<ClientInner>> {
        let mux = Mux::get();
        let domain = mux
            .get_domain(domain_id)
            .ok_or_else(|| anyhow!("invalid domain id {}", domain_id))?;
        let domain = domain
            .downcast_ref::<Self>()
            .ok_or_else(|| anyhow!("domain {} is not a ClientDomain", domain_id))?;

        if let Some(inner) = domain.inner() {
            Ok(inner)
        } else {
            bail!("domain has no assigned client");
        }
    }

    /// The reader in the mux may have decided to give up on one or
    /// more tabs at the time that a disconnect was detected, and
    /// it's also possible that another client connected and adjusted
    /// the set of tabs since we were connected, so we need to re-sync.
    pub async fn reattach(domain_id: DomainId, ui: ConnectionUI) -> anyhow::Result<()> {
        let inner = Self::get_client_inner_for_domain(domain_id)?;

        let panes = inner.client.list_panes().await?;
        Self::process_pane_list(inner, panes, None)?;

        ui.close();
        Ok(())
    }

    pub async fn resync(&self) -> anyhow::Result<()> {
        if let Some(inner) = self.inner() {
            // wezzy: our own resizes / kills haven't all landed, so the
            // server's tree is older than the local one; applying it makes
            // panes jump back and forth. Try again once they have. Checked
            // before asking (no point fetching a tree we'd throw away) and
            // again after, since ops can start while the request is out.
            if inner.defer_resync() {
                return Ok(());
            }
            let panes = inner.client.list_panes().await?;
            if inner.defer_resync() {
                return Ok(());
            }
            Self::process_pane_list(inner, panes, None)?;
        }
        Ok(())
    }

    pub fn process_remote_window_title_change(&self, remote_window_id: WindowId, title: String) {
        if let Some(inner) = self.inner() {
            if let Some(local_window_id) = inner.remote_to_local_window(remote_window_id) {
                if let Some(mut window) = Mux::get().get_window_mut(local_window_id) {
                    window.set_title(&title);
                }
            }
        }
    }

    pub fn process_remote_tab_title_change(&self, remote_tab_id: TabId, title: String) {
        if let Some(inner) = self.inner() {
            if let Some(local_tab_id) = inner.remote_to_local_tab_id(remote_tab_id) {
                if let Some(tab) = Mux::get().get_tab(local_tab_id) {
                    tab.set_title(&title);
                }
            }
        }
    }

    fn process_pane_list(
        inner: Arc<ClientInner>,
        panes: ListPanesResponse,
        mut primary_window_id: Option<WindowId>,
    ) -> anyhow::Result<()> {
        let mux = Mux::get();
        log::debug!(
            "domain {}: ListPanes result {:#?}",
            inner.local_domain_id,
            panes
        );

        // "Mark" the current set of known remote ids, so that we can "Sweep"
        // any unreferenced ids at the bottom, garbage collection style
        let mut remote_windows_to_forget: HashSet<WindowId> = inner
            .remote_to_local_window
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();
        let mut remote_tabs_to_forget: HashSet<WindowId> = inner
            .remote_to_local_tab
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();
        let mut remote_panes_to_forget: HashSet<WindowId> = inner
            .remote_to_local_pane
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();

        for (mut tabroot, tab_title) in panes.tabs.into_iter().zip(panes.tab_titles.iter()) {
            let root_size = match tabroot.root_size() {
                Some(size) => size,
                None => continue,
            };
            // wezzy: the server's split sizes can disagree with each other
            // (e.g. after panes were moved between tabs), leaving it unable
            // to split. Fit the tree to the tab and push the fixed sizes back.
            let mut root_size = root_size;
            let mut refit = fit_pane_tree(&mut tabroot, root_size);

            if let Some((remote_window_id, remote_tab_id)) = tabroot.window_and_tab_ids() {
                let tab;

                remote_windows_to_forget.remove(&remote_window_id);
                remote_tabs_to_forget.remove(&remote_tab_id);

                if let Some(tab_id) = inner.remote_to_local_tab_id(remote_tab_id) {
                    match mux.get_tab(tab_id) {
                        Some(t) => tab = t,
                        None => {
                            // We likely decided that we hit EOF on the tab and
                            // removed it from the mux.  Let's add it back, but
                            // with a new id.
                            log::trace!(
                                "we had remote_to_local_tab_id mapping of \
                                 {remote_tab_id} -> {tab_id}, but the local \
                                 tab is not in the mux, make a new tab"
                            );
                            inner.remove_old_tab_mapping(remote_tab_id);
                            tab = Arc::new(Tab::new(&root_size));
                            inner.record_remote_to_local_tab_mapping(remote_tab_id, tab.tab_id());
                            mux.add_tab_no_panes(&tab);
                        }
                    };
                } else {
                    tab = Arc::new(Tab::new(&root_size));
                    mux.add_tab_no_panes(&tab);
                    inner.record_remote_to_local_tab_mapping(remote_tab_id, tab.tab_id());
                }

                tab.set_title(tab_title);

                // wezzy: same panes in the same arrangement as here: the sizes
                // are ours to decide (we laid them out and sent them). The
                // server's can be off, e.g. after a close, where the survivors'
                // new sizes reach it while the closed pane is still in its
                // tree. Keep ours and push them back if they differ.
                let local = tab.codec_pane_tree();
                if !has_zoom(&local)
                    && !has_zoom(&tabroot)
                    && same_shape(&inner, &tabroot, &local)
                {
                    root_size = tab.get_size();
                    refit = copy_sizes(&mut tabroot, &local);
                }
                if !refit {
                    inner.refit_resent.lock().unwrap().remove(&remote_tab_id);
                }

                log::debug!("domain: {} tree: {:#?}", inner.local_domain_id, tabroot);
                let mut workspace = None;
                tab.sync_with_pane_tree(root_size, tabroot, |entry| {
                    workspace.replace(inner.local_workspace(&entry.workspace));
                    remote_panes_to_forget.remove(&entry.pane_id);
                    if let Some(pane_id) = inner.remote_to_local_pane_id(entry.pane_id) {
                        match mux.get_pane(pane_id) {
                            Some(pane) => {
                                // wezzy: the pane may have moved to this tab
                                if let Some(cp) = pane.downcast_ref::<ClientPane>() {
                                    cp.set_remote_tab_id(entry.tab_id);
                                }
                                pane
                            }
                            None => {
                                // We likely decided that we hit EOF on the tab and
                                // removed it from the mux.  Let's add it back, but
                                // with a new id.
                                inner.remove_old_pane_mapping(entry.pane_id);
                                let pane: Arc<dyn Pane> = Arc::new(ClientPane::new(
                                    &inner,
                                    entry.tab_id,
                                    entry.pane_id,
                                    entry.size,
                                    &entry.title,
                                ));
                                mux.add_pane(&pane).expect("failed to add pane to mux");
                                pane
                            }
                        }
                    } else {
                        let pane: Arc<dyn Pane> = Arc::new(ClientPane::new(
                            &inner,
                            entry.tab_id,
                            entry.pane_id,
                            entry.size,
                            &entry.title,
                        ));
                        log::debug!(
                            "domain: {} attaching to remote pane {:?} -> local pane_id {}",
                            inner.local_domain_id,
                            entry,
                            pane.pane_id()
                        );
                        mux.add_pane(&pane).expect("failed to add pane to mux");
                        pane
                    }
                });

                if refit && tab.get_zoomed_pane().is_none() {
                    match inner.resend_sizes_wait(remote_tab_id) {
                        None => {
                            log::debug!("tab {remote_tab_id}: server layout differs from ours; resending pane sizes");
                            for p in tab.iter_panes_ignoring_zoom() {
                                if let Some(cp) = p.pane.downcast_ref::<ClientPane>() {
                                    cp.send_size(TerminalSize {
                                        rows: p.height,
                                        cols: p.width,
                                        pixel_width: p.pixel_width,
                                        pixel_height: p.pixel_height,
                                        dpi: root_size.dpi,
                                    });
                                }
                            }
                        }
                        Some(wait) => inner.schedule_refit_retry(remote_tab_id, wait),
                    }
                }

                if let Some(local_window_id) = inner.remote_to_local_window(remote_window_id) {
                    if let Some(mut window) = mux.get_window_mut(local_window_id) {
                        log::debug!(
                            "domain: {} adding tab to existing local window {}",
                            inner.local_domain_id,
                            local_window_id
                        );
                        if window.get_tab_idx_for_id(tab.tab_id()).is_none() {
                            window.push_tab(&tab);
                        }
                        // wezzy: the server's workspace wins
                        if let Some(ws) = workspace.as_deref() {
                            window.set_workspace(ws);
                        }
                        continue;
                    }
                    // wezzy: the local window went away (pruned here) while
                    // the server still has it: forget it and find or make one
                    log::debug!(
                        "domain: {} remote window {remote_window_id} mapped to \
                         missing local window {local_window_id}; remapping",
                        inner.local_domain_id,
                    );
                    inner
                        .remote_to_local_window
                        .lock()
                        .unwrap()
                        .remove(&remote_window_id);
                }

                if let Some(local_window_id) = primary_window_id {
                    // Verify that the workspace is consistent between the local and remote
                    // windows (wezzy: a primary window that's gone isn't usable)
                    if mux
                        .get_window(local_window_id)
                        .is_some_and(|w| Some(w.get_workspace()) == workspace.as_deref())
                    {
                        // Yes! We can use this window
                        log::debug!(
                            "adding remote window {} as tab to local window {}",
                            remote_window_id,
                            local_window_id
                        );
                        inner.record_remote_to_local_window_mapping(
                            remote_window_id,
                            local_window_id,
                        );
                        mux.add_tab_to_window(&tab, local_window_id)?;
                        primary_window_id.take();
                        continue;
                    }
                }
                log::debug!(
                    "making new local window for remote {} in workspace {:?}",
                    remote_window_id,
                    workspace
                );
                let position = None;
                let local_window_id = mux.new_empty_window(workspace.take(), position);
                inner.record_remote_to_local_window_mapping(remote_window_id, *local_window_id);
                mux.add_tab_to_window(&tab, *local_window_id)?;
            }
        }

        for (remote_window_id, window_title) in panes.window_titles {
            if let Some(local_window_id) = inner.remote_to_local_window(remote_window_id) {
                if let Some(mut window) = mux.get_window_mut(local_window_id) {
                    window.set_title(&window_title);
                }
            }
        }

        // "Sweep" away our mapping for ids that are no longer present in the
        // latest sync
        log::debug!(
            "after sync, remote_windows_to_forget={remote_windows_to_forget:?}, \
                    remote_tabs_to_forget={remote_tabs_to_forget:?}, \
                    remote_panes_to_forget={remote_panes_to_forget:?}"
        );
        if !remote_windows_to_forget.is_empty() {
            let mut windows = inner.remote_to_local_window.lock().unwrap();
            for w in remote_windows_to_forget {
                windows.remove(&w);
            }
        }
        // wezzy: a tab the server dropped because its panes moved elsewhere
        // still holds them locally; empty it so the panes live only in their
        // new tab, and close it.
        let mut stale_tabs = vec![];
        if !remote_tabs_to_forget.is_empty() {
            let mut tabs = inner.remote_to_local_tab.lock().unwrap();
            for t in remote_tabs_to_forget {
                if let Some(local) = tabs.remove(&t) {
                    stale_tabs.push(local);
                }
            }
        }
        for tab_id in stale_tabs {
            let Some(tab) = mux.get_tab(tab_id) else {
                continue;
            };
            let panes: Vec<PaneId> = tab
                .iter_panes_ignoring_zoom()
                .iter()
                .map(|p| p.pane.pane_id())
                .collect();
            let live: HashSet<PaneId> = inner
                .remote_to_local_pane
                .lock()
                .unwrap()
                .iter()
                .filter(|(remote, _)| !remote_panes_to_forget.contains(remote))
                .map(|(_, local)| *local)
                .collect();
            if !panes.iter().any(|p| live.contains(p)) {
                continue; // the usual case (its panes exited): let it die normally
            }
            tab.sync_with_pane_tree(tab.get_size(), PaneNode::Empty, |_| unreachable!());
            mux.remove_tab(tab_id);
            for p in panes.into_iter().filter(|p| !live.contains(p)) {
                mux.remove_pane(p);
            }
        }
        if !remote_panes_to_forget.is_empty() {
            let mut panes = inner.remote_to_local_pane.lock().unwrap();
            for p in remote_panes_to_forget {
                panes.remove(&p);
            }
        }

        Ok(())
    }

    fn finish_attach(
        domain_id: DomainId,
        client: Client,
        panes: ListPanesResponse,
        primary_window_id: Option<WindowId>,
    ) -> anyhow::Result<()> {
        let mux = Mux::get();
        let domain = mux
            .get_domain(domain_id)
            .ok_or_else(|| anyhow!("invalid domain id {}", domain_id))?;
        let domain = domain
            .downcast_ref::<Self>()
            .ok_or_else(|| anyhow!("domain {} is not a ClientDomain", domain_id))?;
        let threshold = domain.config.local_echo_threshold_ms();
        let overlay_lag_indicator = domain.config.overlay_lag_indicator();

        let mut inner = ClientInner::new(domain_id, client, threshold, overlay_lag_indicator);
        inner.workspace_prefix = domain.workspace_prefix.clone();
        let inner = Arc::new(inner);
        *domain.inner.lock().unwrap() = Some(Arc::clone(&inner));

        Self::process_pane_list(inner, panes, primary_window_id)?;

        Ok(())
    }
}

#[async_trait(?Send)]
impl Domain for ClientDomain {
    fn domain_id(&self) -> DomainId {
        self.local_domain_id
    }

    fn domain_name(&self) -> &str {
        self.config.name()
    }

    async fn domain_label(&self) -> String {
        self.label.to_string()
    }

    async fn spawn_pane(
        &self,
        _size: TerminalSize,
        _command: Option<CommandBuilder>,
        _command_dir: Option<String>,
    ) -> anyhow::Result<Arc<dyn Pane>> {
        anyhow::bail!("spawn_pane not implemented for ClientDomain")
    }

    /// Forward the request to the remote; we need to translate the local ids
    /// to those that match the remote for the request, resync the changed
    /// structure, and then translate the results back to local
    async fn move_pane_to_new_tab(
        &self,
        pane_id: PaneId,
        window_id: Option<WindowId>,
        workspace_for_new_window: Option<String>,
    ) -> anyhow::Result<Option<(Arc<Tab>, WindowId)>> {
        let inner = self
            .inner()
            .ok_or_else(|| anyhow!("domain is not attached"))?;

        let local_pane = Mux::get()
            .get_pane(pane_id)
            .ok_or_else(|| anyhow!("pane_id {} is invalid", pane_id))?;
        let pane = local_pane
            .downcast_ref::<ClientPane>()
            .ok_or_else(|| anyhow!("pane_id {} is not a ClientPane", pane_id))?;

        let remote_window_id =
            window_id.and_then(|local_window| self.local_to_remote_window_id(local_window));

        let result = inner
            .client
            .move_pane_to_new_tab(codec::MovePaneToNewTab {
                pane_id: pane.remote_pane_id,
                window_id: remote_window_id,
                workspace_for_new_window: workspace_for_new_window
                    .map(|w| inner.remote_workspace(&w)),
            })
            .await?;

        self.resync().await?;

        let local_tab_id = inner
            .remote_to_local_tab_id(result.tab_id)
            .ok_or_else(|| anyhow!("remote tab {} didn't resolve after resync", result.tab_id))?;

        let local_win_id = self
            .remote_to_local_window_id(result.window_id)
            .ok_or_else(|| {
                anyhow!(
                    "remote window {} didn't resolve after resync",
                    result.window_id
                )
            })?;

        let tab = Mux::get()
            .get_tab(local_tab_id)
            .ok_or_else(|| anyhow!("local tab {local_tab_id} is invalid"))?;

        Ok(Some((tab, local_win_id)))
    }

    async fn spawn(
        &self,
        size: TerminalSize,
        command: Option<CommandBuilder>,
        command_dir: Option<String>,
        window: WindowId,
    ) -> anyhow::Result<Arc<Tab>> {
        let inner = self
            .inner()
            .ok_or_else(|| anyhow!("domain is not attached"))?;

        // wezzy: a new window belongs to its own workspace, not the active one
        let workspace = Mux::get()
            .get_window(window)
            .map(|w| w.get_workspace().to_string())
            .unwrap_or_else(|| Mux::get().active_workspace());
        let workspace = inner.remote_workspace(&workspace);
        let (command, command_dir) = match inner.workspace_prefix {
            Some(_) => for_host(command, command_dir),
            None => (command, command_dir),
        };

        let result = inner
            .client
            .spawn_v2(SpawnV2 {
                domain: SpawnTabDomain::DefaultDomain,
                window_id: inner.local_to_remote_window(window),
                size,
                command,
                command_dir,
                workspace,
            })
            .await?;

        inner.record_remote_to_local_window_mapping(result.window_id, window);

        let mux = Mux::get();

        // wezzy: a resync (e.g. for the server's TabAddedToWindow) can land
        // before this response and build the tab from ListPanes already;
        // making another would show the pane twice.
        let existing = inner
            .remote_to_local_tab_id(result.tab_id)
            .and_then(|id| mux.get_tab(id))
            .filter(|_| {
                inner
                    .remote_to_local_pane_id(result.pane_id)
                    .is_some_and(|id| mux.get_pane(id).is_some())
            });
        if let Some(tab) = existing {
            let tab_id = tab.tab_id();
            // that resync didn't know the new remote window was `window`,
            // so it may have made a window of its own for the tab
            if let Some(other) = mux.window_containing_tab(tab_id).filter(|w| *w != window) {
                let now_empty = match mux.get_window_mut(other) {
                    Some(mut w) => {
                        w.remove_tab_id(tab_id);
                        w.is_empty()
                    }
                    None => false,
                };
                if now_empty {
                    mux.kill_window(other);
                }
            }
            let in_window = mux
                .get_window(window)
                .is_some_and(|w| w.get_tab_idx_for_id(tab_id).is_some());
            if !in_window {
                mux.add_tab_to_window(&tab, window)?;
            }
            return Ok(tab);
        }

        let pane: Arc<dyn Pane> = Arc::new(ClientPane::new(
            &inner,
            result.tab_id,
            result.pane_id,
            size,
            "wezterm",
        ));
        let tab = Arc::new(Tab::new(&size));
        tab.assign_pane(&pane);
        inner.remove_old_tab_mapping(result.tab_id);
        if inner
            .remote_to_local_pane_id(result.pane_id)
            .is_some_and(|id| mux.get_pane(id).is_none())
        {
            inner.remove_old_pane_mapping(result.pane_id);
        }
        inner.record_remote_to_local_tab_mapping(result.tab_id, tab.tab_id());

        mux.add_tab_and_active_pane(&tab)?;
        mux.add_tab_to_window(&tab, window)?;

        Ok(tab)
    }

    async fn split_pane(
        &self,
        source: SplitSource,
        tab_id: TabId,
        pane_id: PaneId,
        split_request: SplitRequest,
    ) -> anyhow::Result<Arc<dyn Pane>> {
        let inner = self
            .inner()
            .ok_or_else(|| anyhow!("domain is not attached"))?;

        let mux = Mux::get();

        let tab = mux
            .get_tab(tab_id)
            .ok_or_else(|| anyhow!("tab_id {} is invalid", tab_id))?;
        let local_pane = mux
            .get_pane(pane_id)
            .ok_or_else(|| anyhow!("pane_id {} is invalid", pane_id))?;
        let pane = local_pane
            .downcast_ref::<ClientPane>()
            .ok_or_else(|| anyhow!("pane_id {} is not a ClientPane", pane_id))?;

        let (command, command_dir, move_pane_id) = match source {
            SplitSource::Spawn {
                command,
                command_dir,
            } => match inner.workspace_prefix {
                Some(_) => {
                    let (command, command_dir) = for_host(command, command_dir);
                    (command, command_dir, None)
                }
                None => (command, command_dir, None),
            },
            // wezzy: the server wants its own id for the pane being moved
            SplitSource::MovePane(move_pane_id) => {
                let moving = mux
                    .get_pane(move_pane_id)
                    .ok_or_else(|| anyhow!("pane_id {} is invalid", move_pane_id))?;
                let moving = moving
                    .downcast_ref::<ClientPane>()
                    .ok_or_else(|| anyhow!("pane_id {} is not a ClientPane", move_pane_id))?;
                (None, None, Some(moving.remote_pane_id))
            }
        };

        let result = inner
            .client
            .split_pane(SplitPane {
                domain: SpawnTabDomain::CurrentPaneDomain,
                pane_id: pane.remote_pane_id,
                split_request,
                command,
                command_dir,
                move_pane_id,
            })
            .await?;

        // wezzy: a moved pane already has a local pane; rebuild both tabs
        // from the server instead of adding a second copy of it
        if move_pane_id.is_some() {
            self.resync().await?;
            let local = inner
                .remote_to_local_pane_id(result.pane_id)
                .ok_or_else(|| anyhow!("remote pane {} didn't resolve", result.pane_id))?;
            return mux
                .get_pane(local)
                .ok_or_else(|| anyhow!("local pane {local} is invalid"));
        }

        // wezzy: a resync that landed before this response may have built
        // the new pane from ListPanes already; don't add a second one
        if let Some(local) = inner.remote_to_local_pane_id(result.pane_id) {
            match mux.get_pane(local) {
                Some(existing) => return Ok(existing),
                None => inner.remove_old_pane_mapping(result.pane_id),
            }
        }

        let pane: Arc<dyn Pane> = Arc::new(ClientPane::new(
            &inner,
            result.tab_id,
            result.pane_id,
            result.size,
            "wezterm",
        ));

        let pane_index = match tab
            .iter_panes()
            .iter()
            .find(|p| p.pane.pane_id() == pane_id)
        {
            Some(p) => p.index,
            None => anyhow::bail!("invalid pane id {}", pane_id),
        };

        tab.split_and_insert(pane_index, split_request, Arc::clone(&pane))
            .ok();

        mux.add_pane(&pane)?;

        Ok(pane)
    }

    async fn attach(&self, window_id: Option<WindowId>) -> anyhow::Result<()> {
        if self.state() == DomainState::Attached {
            // Already attached
            return Ok(());
        }

        let domain_id = self.local_domain_id;
        let config = self.config.clone();

        let activity = mux::activity::Activity::new();
        let ui = ConnectionUI::with_params(ConnectionUIParams {
            window_id,
            ..Default::default()
        });
        ui.title("wezterm: Connecting...");

        ui.async_run_and_log_error({
            let ui = ui.clone();
            async move {
                let mut cloned_ui = ui.clone();
                let client = spawn_into_new_thread(move || match &config {
                    ClientDomainConfig::Unix(unix) => {
                        let initial = true;
                        let no_auto_start = false;
                        Client::new_unix_domain(
                            Some(domain_id),
                            unix,
                            initial,
                            &mut cloned_ui,
                            no_auto_start,
                        )
                    }
                    ClientDomainConfig::Tls(tls) => Client::new_tls(domain_id, tls, &mut cloned_ui),
                    ClientDomainConfig::Ssh(ssh) => Client::new_ssh(domain_id, ssh, &mut cloned_ui),
                })
                .await?;

                ui.output_str("Checking server version\n");
                client.verify_version_compat(&ui).await?;

                ui.output_str("Version check OK!  Requesting pane list...\n");
                let panes = client.list_panes().await?;
                ui.output_str(&format!(
                    "Server has {} tabs.  Attaching to local UI...\n",
                    panes.tabs.len()
                ));
                ClientDomain::finish_attach(domain_id, client, panes, window_id)
            }
        })
        .await
        .map_err(|e| {
            ui.output_str(&format!("Error during attach: {:#}\n", e));
            e
        })?;

        ui.output_str("Attached!\n");
        drop(activity);
        ui.close();
        Ok(())
    }

    fn detachable(&self) -> bool {
        true
    }

    fn detach(&self) -> anyhow::Result<()> {
        self.perform_detach();
        Ok(())
    }

    fn state(&self) -> DomainState {
        if self.inner.lock().unwrap().is_some() {
            DomainState::Attached
        } else {
            DomainState::Detached
        }
    }
}

/// wezzy: whether a tab tree has a zoomed pane.
fn has_zoom(node: &PaneNode) -> bool {
    match node {
        PaneNode::Empty => false,
        PaneNode::Leaf(e) => e.is_zoomed_pane,
        PaneNode::Split { left, right, .. } => has_zoom(left) || has_zoom(right),
    }
}

/// wezzy: whether the server's tree `remote` holds the same panes, split the
/// same ways, as the local tab's tree `local`.
fn same_shape(inner: &ClientInner, remote: &PaneNode, local: &PaneNode) -> bool {
    let map = inner.remote_to_local_pane.lock().unwrap();
    same_shape_by(remote, local, &|remote_id| map.get(&remote_id).copied())
}

/// wezzy: `same_shape` with the remote -> local pane id lookup passed in.
fn same_shape_by(
    remote: &PaneNode,
    local: &PaneNode,
    to_local: &dyn Fn(PaneId) -> Option<PaneId>,
) -> bool {
    match (remote, local) {
        (PaneNode::Leaf(r), PaneNode::Leaf(l)) => to_local(r.pane_id) == Some(l.pane_id),
        (
            PaneNode::Split { left: rl, right: rr, node: rn },
            PaneNode::Split { left: ll, right: lr, node: ln },
        ) => {
            rn.direction == ln.direction
                && same_shape_by(rl, ll, to_local)
                && same_shape_by(rr, lr, to_local)
        }
        _ => false,
    }
}

/// wezzy: copies the sizes of `local` onto `remote` (the same shape); true if
/// any differed.
fn copy_sizes(remote: &mut PaneNode, local: &PaneNode) -> bool {
    let dims = |s: &TerminalSize| (s.rows, s.cols);
    match (remote, local) {
        (PaneNode::Leaf(r), PaneNode::Leaf(l)) => {
            let changed = dims(&r.size) != dims(&l.size);
            r.size = l.size;
            changed
        }
        (
            PaneNode::Split { left: rl, right: rr, node: rn },
            PaneNode::Split { left: ll, right: lr, node: ln },
        ) => {
            let mut changed =
                dims(&rn.first) != dims(&ln.first) || dims(&rn.second) != dims(&ln.second);
            rn.first = ln.first;
            rn.second = ln.second;
            changed |= copy_sizes(rl, ll);
            changed |= copy_sizes(rr, lr);
            changed
        }
        _ => false,
    }
}

/// wezzy: make every split in `node` fit inside `size` (the first child keeps
/// its extent where it can, the second takes the rest), and give each pane the
/// size of its slot. Returns whether anything changed.
fn fit_pane_tree(node: &mut PaneNode, size: TerminalSize) -> bool {
    match node {
        PaneNode::Empty => false,
        PaneNode::Leaf(entry) => {
            let changed = entry.size.rows != size.rows || entry.size.cols != size.cols;
            entry.size = size;
            changed
        }
        PaneNode::Split { left, right, node } => {
            let cell_w = size.pixel_width / size.cols.max(1);
            let cell_h = size.pixel_height / size.rows.max(1);
            let slot = |rows: usize, cols: usize| TerminalSize {
                rows,
                cols,
                pixel_width: cols * cell_w,
                pixel_height: rows * cell_h,
                dpi: size.dpi,
            };
            let (first, second) = match node.direction {
                mux::tab::SplitDirection::Horizontal => {
                    let avail = size.cols.saturating_sub(1);
                    if avail < 2 {
                        return false;
                    }
                    let a = node.first.cols.clamp(1, avail - 1);
                    (slot(size.rows, a), slot(size.rows, avail - a))
                }
                mux::tab::SplitDirection::Vertical => {
                    let avail = size.rows.saturating_sub(1);
                    if avail < 2 {
                        return false;
                    }
                    let a = node.first.rows.clamp(1, avail - 1);
                    (slot(a, size.cols), slot(avail - a, size.cols))
                }
            };
            let dims = |s: &TerminalSize| (s.rows, s.cols);
            let mut changed =
                dims(&node.first) != dims(&first) || dims(&node.second) != dims(&second);
            node.first = first;
            node.second = second;
            changed |= fit_pane_tree(left, first);
            changed |= fit_pane_tree(right, second);
            changed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mux::renderable::StableCursorPosition;
    use mux::tab::{PaneEntry, SplitDirection, SplitDirectionAndSize};

    fn sz(rows: usize, cols: usize) -> TerminalSize {
        TerminalSize {
            rows,
            cols,
            pixel_width: cols * 8,
            pixel_height: rows * 16,
            dpi: 72,
        }
    }

    fn leaf(pane_id: PaneId, size: TerminalSize) -> PaneNode {
        PaneNode::Leaf(PaneEntry {
            window_id: 0,
            tab_id: 0,
            pane_id,
            title: String::new(),
            size,
            working_dir: None,
            is_active_pane: false,
            is_zoomed_pane: false,
            workspace: "default".into(),
            cursor_pos: StableCursorPosition::default(),
            physical_top: 0,
            top_row: 0,
            left_col: 0,
            tty_name: None,
        })
    }

    fn split(
        direction: SplitDirection,
        first: TerminalSize,
        second: TerminalSize,
        left: PaneNode,
        right: PaneNode,
    ) -> PaneNode {
        PaneNode::Split {
            left: Box::new(left),
            right: Box::new(right),
            node: SplitDirectionAndSize {
                direction,
                first,
                second,
            },
        }
    }

    fn dims(s: &TerminalSize) -> (usize, usize) {
        (s.rows, s.cols)
    }

    fn split_dims(node: &PaneNode) -> ((usize, usize), (usize, usize)) {
        match node {
            PaneNode::Split { node, .. } => (dims(&node.first), dims(&node.second)),
            _ => panic!("not a split: {:?}", node),
        }
    }

    fn leaf_dims(node: &PaneNode) -> (usize, usize) {
        match node {
            PaneNode::Leaf(e) => dims(&e.size),
            _ => panic!("not a leaf: {:?}", node),
        }
    }

    fn children(node: &PaneNode) -> (&PaneNode, &PaneNode) {
        match node {
            PaneNode::Split { left, right, .. } => (left, right),
            _ => panic!("not a split: {:?}", node),
        }
    }

    #[test]
    fn fit_leaves_consistent_tree_alone() {
        // 180 cols: 90 | divider | 89
        let make = || {
            split(
                SplitDirection::Horizontal,
                sz(50, 90),
                sz(50, 89),
                leaf(1, sz(50, 90)),
                leaf(2, sz(50, 89)),
            )
        };
        let mut tree = make();
        assert!(!fit_pane_tree(&mut tree, sz(50, 180)));
        assert_eq!(tree, make());
    }

    #[test]
    fn fit_shrinks_children_wider_than_the_tab() {
        // from the server log: a 180-col tab whose split claims 90 + 180
        let mut tree = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 180),
            leaf(1, sz(50, 90)),
            leaf(2, sz(50, 180)),
        );
        assert!(fit_pane_tree(&mut tree, sz(50, 180)));
        assert_eq!(split_dims(&tree), ((50, 90), (50, 89)));
        let (l, r) = children(&tree);
        assert_eq!(leaf_dims(l), (50, 90));
        assert_eq!(leaf_dims(r), (50, 89));
        // the pixel sizes follow the cells
        match r {
            PaneNode::Leaf(e) => assert_eq!((e.size.pixel_width, e.size.pixel_height), (89 * 8, 50 * 16)),
            _ => unreachable!(),
        }
        // and a second pass finds nothing left to fix
        assert!(!fit_pane_tree(&mut tree, sz(50, 180)));
    }

    #[test]
    fn fit_clamps_a_first_child_wider_than_the_tab() {
        let mut tree = split(
            SplitDirection::Horizontal,
            sz(50, 200),
            sz(50, 10),
            leaf(1, sz(50, 200)),
            leaf(2, sz(50, 10)),
        );
        assert!(fit_pane_tree(&mut tree, sz(50, 180)));
        // avail = 179; the first child is clamped so the second keeps a column
        assert_eq!(split_dims(&tree), ((50, 178), (50, 1)));
    }

    #[test]
    fn fit_nested_splits() {
        // 180x50: left 90 | right 89, the right split top/bottom 25 + 24
        // but the server thinks the right side is 180 wide
        let mut tree = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 180),
            leaf(1, sz(50, 90)),
            split(
                SplitDirection::Vertical,
                sz(25, 180),
                sz(24, 180),
                leaf(2, sz(25, 180)),
                leaf(3, sz(24, 180)),
            ),
        );
        assert!(fit_pane_tree(&mut tree, sz(50, 180)));
        let (_, right) = children(&tree);
        assert_eq!(split_dims(right), ((25, 89), (24, 89)));
        let (top, bottom) = children(right);
        assert_eq!(leaf_dims(top), (25, 89));
        assert_eq!(leaf_dims(bottom), (24, 89));

        // once fitted, it's consistent and left alone
        assert!(!fit_pane_tree(&mut tree, sz(50, 180)));
        assert_eq!(split_dims(&tree), ((50, 90), (50, 89)));
        assert_eq!(split_dims(children(&tree).1), ((25, 89), (24, 89)));
    }

    #[test]
    fn fit_gives_up_without_room_to_split() {
        // 2 cols: one for the divider leaves 1, under the 2 a split needs
        for (dir, size) in [
            (SplitDirection::Horizontal, sz(50, 2)),
            (SplitDirection::Vertical, sz(2, 80)),
        ] {
            let make =
                || split(dir, sz(50, 90), sz(50, 180), leaf(1, sz(1, 1)), leaf(2, sz(1, 1)));
            let mut tree = make();
            assert!(!fit_pane_tree(&mut tree, size));
            assert_eq!(tree, make());
        }
    }

    #[test]
    fn fit_resizes_a_lone_leaf() {
        let mut tree = leaf(1, sz(24, 80));
        assert!(fit_pane_tree(&mut tree, sz(50, 180)));
        assert_eq!(leaf_dims(&tree), (50, 180));
        assert!(!fit_pane_tree(&mut tree, sz(50, 180)));
        assert!(!fit_pane_tree(&mut PaneNode::Empty, sz(50, 180)));
    }

    #[test]
    fn copy_sizes_unchanged_when_equal() {
        let local = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 89),
            leaf(10, sz(50, 90)),
            leaf(11, sz(50, 89)),
        );
        let mut remote = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 89),
            leaf(1, sz(50, 90)),
            leaf(2, sz(50, 89)),
        );
        assert!(!copy_sizes(&mut remote, &local));
    }

    #[test]
    fn copy_sizes_takes_local_sizes() {
        let local = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 89),
            leaf(10, sz(50, 90)),
            split(
                SplitDirection::Vertical,
                sz(25, 89),
                sz(24, 89),
                leaf(11, sz(25, 89)),
                leaf(12, sz(24, 89)),
            ),
        );
        let mut remote = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 180),
            leaf(1, sz(50, 90)),
            split(
                SplitDirection::Vertical,
                sz(25, 180),
                sz(24, 180),
                leaf(2, sz(25, 180)),
                leaf(3, sz(24, 180)),
            ),
        );
        assert!(copy_sizes(&mut remote, &local));
        assert_eq!(split_dims(&remote), ((50, 90), (50, 89)));
        let (_, right) = children(&remote);
        assert_eq!(split_dims(right), ((25, 89), (24, 89)));
        let (top, bottom) = children(right);
        assert_eq!(leaf_dims(top), (25, 89));
        assert_eq!(leaf_dims(bottom), (24, 89));
        // pane ids stay the server's
        match top {
            PaneNode::Leaf(e) => assert_eq!(e.pane_id, 2),
            _ => unreachable!(),
        }
        assert!(!copy_sizes(&mut remote, &local));
    }

    #[test]
    fn copy_sizes_detects_a_leaf_only_difference() {
        let local = leaf(10, sz(50, 180));
        let mut remote = leaf(1, sz(24, 80));
        assert!(copy_sizes(&mut remote, &local));
        assert_eq!(leaf_dims(&remote), (50, 180));
    }

    #[test]
    fn has_zoom_finds_a_zoomed_leaf() {
        let mut zoomed = leaf(2, sz(50, 89));
        if let PaneNode::Leaf(e) = &mut zoomed {
            e.is_zoomed_pane = true;
        }
        let plain = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 89),
            leaf(1, sz(50, 90)),
            leaf(2, sz(50, 89)),
        );
        assert!(!has_zoom(&plain));
        assert!(!has_zoom(&PaneNode::Empty));
        let nested = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 89),
            leaf(1, sz(50, 90)),
            split(
                SplitDirection::Vertical,
                sz(25, 89),
                sz(24, 89),
                leaf(3, sz(25, 89)),
                zoomed,
            ),
        );
        assert!(has_zoom(&nested));
    }

    #[test]
    fn same_shape_compares_panes_and_directions() {
        let map: HashMap<PaneId, PaneId> = HashMap::from([(1, 10), (2, 11)]);
        let to_local = |id: PaneId| map.get(&id).copied();
        let tree = |dir, a, b| {
            split(dir, sz(50, 90), sz(50, 89), leaf(a, sz(50, 90)), leaf(b, sz(50, 89)))
        };
        let local = tree(SplitDirection::Horizontal, 10, 11);
        assert!(same_shape_by(&tree(SplitDirection::Horizontal, 1, 2), &local, &to_local));
        // sizes don't matter
        let resized = split(
            SplitDirection::Horizontal,
            sz(50, 90),
            sz(50, 180),
            leaf(1, sz(50, 90)),
            leaf(2, sz(50, 180)),
        );
        assert!(same_shape_by(&resized, &local, &to_local));
        // swapped panes, other direction, unknown pane, different structure
        assert!(!same_shape_by(&tree(SplitDirection::Horizontal, 2, 1), &local, &to_local));
        assert!(!same_shape_by(&tree(SplitDirection::Vertical, 1, 2), &local, &to_local));
        assert!(!same_shape_by(&tree(SplitDirection::Horizontal, 1, 3), &local, &to_local));
        assert!(!same_shape_by(&leaf(1, sz(50, 180)), &local, &to_local));
    }
}
