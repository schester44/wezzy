//! Command palette: one fuzzy list over everything you can jump to or run.
//!
//! Each `Kind` is fed by a provider below. New surfaces (browser, markdown,
//! visualizers...) add a `Kind` + a provider and get search, ranking, keyboard
//! nav and rendering for free.

use crate::agents;
use crate::app::{Overlay, TermApp};
use crate::commands::AppCommand;
use config::keyassignment::{
    KeyAssignment, PaneDirection, SpawnCommand, SpawnTabDomain, SplitSize,
};
use gpui::{Context, KeyDownEvent, ScrollHandle, Window};
use mux::pane::PaneId;
use mux::tab::TabId;
use mux::window::WindowId;
use mux::Mux;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    All,
    Agents,
    Tabs,
    Workspaces,
    Commands,
    Recipes,
    Hosts,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::All => "",
            Scope::Agents => "Agents",
            Scope::Tabs => "Tabs",
            Scope::Workspaces => "Workspaces",
            Scope::Commands => "Commands",
            Scope::Recipes => "Recipes",
            Scope::Hosts => "SSH",
        }
    }

    fn admits(self, kind: Kind) -> bool {
        match self {
            Scope::All => true,
            Scope::Agents => matches!(kind, Kind::Agent | Kind::Launch),
            Scope::Tabs => matches!(kind, Kind::Tab | Kind::Window),
            Scope::Workspaces => kind == Kind::Workspace,
            Scope::Commands => matches!(kind, Kind::Command | Kind::Launch | Kind::Recipe),
            Scope::Recipes => kind == Kind::Recipe,
            Scope::Hosts => kind == Kind::Host,
        }
    }
}

/// Declaration order is section order when the query is empty.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Kind {
    Agent,
    Launch,
    Recipe,
    Host,
    Database,
    Tab,
    Window,
    Workspace,
    Command,
}

impl Kind {
    pub fn section(self) -> &'static str {
        match self {
            Kind::Agent => "Agents",
            Kind::Launch => "New Agent",
            Kind::Recipe => "Recipes",
            Kind::Host => "SSH",
            Kind::Database => "Databases",
            Kind::Tab => "Tabs",
            Kind::Window => "Windows",
            Kind::Workspace => "Workspaces",
            Kind::Command => "Commands",
        }
    }
}

#[derive(Clone)]
pub enum PaletteAction {
    FocusPane(PaneId),
    FocusTab { window: WindowId, tab: TabId },
    FocusWindow(WindowId),
    Workspace(String),
    App(AppCommand),
    Key(KeyAssignment),
    /// Launch a harness: ↵ splits, ⌘↵ opens a tab.
    Harness(String),
    /// Run a recipe (by name): ↵ splits, ⌘↵ opens a tab.
    Recipe(String),
    /// Open an ssh session: ↵ splits, ⌘↵ opens a tab.
    Ssh(crate::hosts::Host),
    /// Open the SQL view on a database (and run `query`): ↵ opens a tab, ⌘↵ splits.
    Sql {
        conn: crate::views::sql::conn::Conn,
        query: Option<String>,
    },
}

#[derive(Clone)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    pub subtitle: String,
    pub shortcut: Option<String>,
    /// An agent's status, and whether it's unseen.
    pub status: Option<(agents::Status, bool)>,
    pub action: PaletteAction,
}

pub struct PaletteState {
    /// Picking an agent to send this pane's output to (Send Output to Agent).
    pub send_from: Option<PaneId>,
    /// Picking an agent to send the git panel's review comments to.
    pub send_feedback: bool,
    /// Picking a workspace to move this tab to (Move Tab to Workspace).
    pub move_tab: Option<TabId>,
    pub query: String,
    pub scope: Scope,
    pub selected: usize,
    pub scroll: ScrollHandle,
}

impl PaletteState {
    pub fn new(scope: Scope) -> Self {
        Self {
            send_from: None,
            send_feedback: false,
            move_tab: None,
            query: String::new(),
            scope,
            selected: 0,
            scroll: ScrollHandle::new(),
        }
    }
}

/// Built-in commands. `Key` entries reuse WezTerm actions so their shortcut
/// labels come straight from the user's bindings.
fn commands() -> Vec<(&'static str, PaletteAction)> {
    use KeyAssignment::*;
    use PaletteAction::{App, Key};
    let split = |direction| {
        Key(SplitPane(config::keyassignment::SplitPane {
            direction,
            size: SplitSize::Percent(50),
            command: SpawnCommand {
                domain: SpawnTabDomain::CurrentPaneDomain,
                ..Default::default()
            },
            top_level: false,
        }))
    };
    vec![
        ("New Tab", Key(SpawnTab(SpawnTabDomain::CurrentPaneDomain))),
        ("New Terminal", App(AppCommand::NewTerminal)),
        ("New Workspace…", App(AppCommand::NewWorkspace)),
        ("Rename Tab…", App(AppCommand::RenameTab)),
        ("Rename Workspace…", App(AppCommand::RenameWorkspace)),
        ("Move Tab to Workspace…", App(AppCommand::MoveTabToWorkspace)),
        ("Move Pane to New Window", App(AppCommand::MovePaneToNewWindow)),
        ("Balance Panes", App(AppCommand::BalancePanes)),
        ("Split Right", split(PaneDirection::Right)),
        ("Split Down", split(PaneDirection::Down)),
        (
            "Hello Pane",
            Key(SpawnCommandInNewTab(crate::views::spawn_command("hello", "", "Hello"))),
        ),
        (
            "Split Right: Hello Pane",
            Key(SplitPane(config::keyassignment::SplitPane {
                direction: PaneDirection::Right,
                size: SplitSize::Percent(50),
                command: crate::views::spawn_command("hello", "", "Hello"),
                top_level: false,
            })),
        ),
        ("Settings", App(AppCommand::OpenSettings)),
        ("Keyboard Shortcuts", App(AppCommand::ShowShortcuts)),
        ("Restart Wezzy", App(AppCommand::Restart)),
        ("Rebuild & Restart", App(AppCommand::RebuildRestart)),
        ("Check for Updates", App(AppCommand::CheckForUpdates)),
        ("Open Editor", App(AppCommand::OpenEditor)),
        ("Quick Select", Key(QuickSelect)),
        ("Quick Select Errors", App(AppCommand::SelectError)),
        ("Send Output to Agent", App(AppCommand::SendOutputToAgent)),
        ("Run Recipe…", App(AppCommand::RunRecipe)),
        ("SSH…", App(AppCommand::SshHost)),
        ("Toggle Diff Panel", App(AppCommand::ToggleDiffPanel)),
        ("Create Pull Request", App(AppCommand::CreatePullRequest)),
        ("Toggle Pane Zoom", Key(TogglePaneZoomState)),
        ("Close Pane", Key(CloseCurrentPane { confirm: true })),
        ("Close Tab", Key(CloseCurrentTab { confirm: true })),
        ("Next Tab", Key(ActivateTabRelative(1))),
        ("Previous Tab", Key(ActivateTabRelative(-1))),
        ("Toggle Sidebar", App(AppCommand::ToggleSidebar)),
        ("Agent Overview", App(AppCommand::AgentOverview)),
        ("Find Agent…", App(AppCommand::FindAgent)),
        ("Rename Agent…", App(AppCommand::RenameAgent)),
        ("Find Tab…", App(AppCommand::FindTab)),
        ("Switch Workspace…", App(AppCommand::SwitchWorkspace)),
        ("Install Agent Hooks", App(AppCommand::InstallAgentHooks)),
        ("Uninstall Agent Hooks", App(AppCommand::UninstallAgentHooks)),
        ("Reload Configuration", Key(ReloadConfiguration)),
        ("Increase Font Size", Key(IncreaseFontSize)),
        ("Decrease Font Size", Key(DecreaseFontSize)),
        ("Reset Font Size", Key(ResetFontSize)),
        ("Toggle Full Screen", Key(ToggleFullScreen)),
        ("Quit", Key(QuitApplication)),
    ]
}

impl TermApp {
    pub(crate) fn open_palette(&mut self, scope: Scope, cx: &mut Context<Self>) {
        self.ws_menu_open = false;
        self.overlay = Some(Overlay::Palette(PaletteState::new(scope)));
        cx.notify();
    }

    fn palette_items(&self) -> Vec<Item> {
        let mux = Mux::get();
        let mut items = vec![];

        // agents
        for a in agents::collect(self.active_pane().map(|p| p.pane_id())) {
            items.push(Item {
                kind: Kind::Agent,
                title: format!("{} · {}", a.display_name(), a.project),
                subtitle: {
                    let pane = crate::session::remote_pane_id(a.pane_id)
                        .map(|id| format!(" · pane {id}"))
                        .unwrap_or_default();
                    // a named agent still says what it is
                    let kind = a.name.as_ref().map(|_| format!("{} · ", a.kind)).unwrap_or_default();
                    if a.title.is_empty() {
                        format!("{kind}{}{pane}", a.workspace)
                    } else {
                        format!("{kind}{} — {}{pane}", a.title, a.workspace)
                    }
                },
                shortcut: None,
                status: Some((a.status, a.unseen)),
                action: PaletteAction::FocusPane(a.pane_id),
            });
        }

        // harnesses
        let settings = crate::settings::get();
        let default = settings.default_harness_name();
        for h in settings.harnesses() {
            let is_default = h.name == default;
            items.push(Item {
                kind: Kind::Launch,
                title: h.label(),
                subtitle: String::new(),
                shortcut: is_default
                    .then(|| self.keys.shortcut_for(&AppCommand::NewAgent.action()))
                    .flatten(),
                status: None,
                action: PaletteAction::Harness(h.name.clone()),
            });
        }

        // recipes
        for (i, r) in crate::recipes::all().into_iter().enumerate() {
            let harness = r
                .harness
                .clone()
                .unwrap_or_else(|| settings.default_harness_name());
            let label = settings
                .harness(&harness)
                .map(|h| h.label())
                .unwrap_or(harness);
            let preview: String = r.prompt.lines().next().unwrap_or("").chars().take(60).collect();
            items.push(Item {
                kind: Kind::Recipe,
                title: r.name.clone(),
                subtitle: format!("{label} · {preview}"),
                shortcut: (i == 0)
                    .then(|| self.keys.shortcut_for(&AppCommand::RunRecipe.action()))
                    .flatten(),
                status: None,
                action: PaletteAction::Recipe(r.name),
            });
        }

        // ssh hosts
        for (i, e) in crate::hosts::all().into_iter().enumerate() {
            items.push(Item {
                kind: Kind::Host,
                title: e.host.name.clone(),
                subtitle: format!("ssh {}", e.target),
                shortcut: (i == 0)
                    .then(|| self.keys.shortcut_for(&AppCommand::SshHost.action()))
                    .flatten(),
                status: None,
                action: PaletteAction::Ssh(e.host),
            });
        }

        // databases: configured, then found in the active pane's project
        let cwd = self
            .active_pane()
            .and_then(|p| p.get_current_working_dir(mux::pane::CachePolicy::AllowStale))
            .map(|u| std::path::PathBuf::from(u.path()));
        for conn in crate::views::sql::conn::all(cwd.as_deref()) {
            items.push(Item {
                kind: Kind::Database,
                title: conn.name.clone(),
                subtitle: format!("sql {}", conn.target()),
                shortcut: None,
                status: None,
                action: PaletteAction::Sql { conn, query: None },
            });
        }

        // tabs + windows, across every workspace
        let windows = mux.iter_windows();
        for window_id in &windows {
            let Some(w) = mux.get_window(*window_id) else {
                continue;
            };
            let ws = w.get_workspace().to_string();
            let multi = mux.iter_windows_in_workspace(&ws).len() > 1;
            if multi {
                items.push(Item {
                    kind: Kind::Window,
                    title: format!("Window {window_id}"),
                    subtitle: format!("{ws} · {} tabs", w.count_tabs()),
                    shortcut: None,
                    status: None,
                    action: PaletteAction::FocusWindow(*window_id),
                });
            }
            for (idx, tab) in w.iter_tabs().enumerate() {
                let mut title = tab.get_title();
                if title.is_empty() {
                    title = tab
                        .get_active_pane()
                        .map(|p| p.get_title())
                        .unwrap_or_default();
                }
                let place = if multi {
                    format!("{ws} · window {window_id}")
                } else {
                    ws.clone()
                };
                items.push(Item {
                    kind: Kind::Tab,
                    title: format!("{}  {}", idx + 1, title),
                    subtitle: place,
                    shortcut: None,
                    status: None,
                    action: PaletteAction::FocusTab {
                        window: *window_id,
                        tab: tab.tab_id(),
                    },
                });
            }
        }

        // workspaces (moving a tab: every one but its own)
        let moving = match &self.overlay {
            Some(Overlay::Palette(p)) => p.move_tab,
            _ => None,
        };
        let here = moving
            .and_then(|t| mux.window_containing_tab(t))
            .and_then(|w| mux.get_window(w).map(|w| w.get_workspace().to_string()));
        for ws in crate::workspaces::list() {
            if here.as_deref() == Some(ws.name.as_str()) {
                continue;
            }
            items.push(Item {
                kind: Kind::Workspace,
                subtitle: ws.detail(),
                title: ws.name.clone(),
                shortcut: None,
                status: None,
                action: PaletteAction::Workspace(ws.name),
            });
        }

        // commands
        let hooks = crate::hooks::harness_hooks();
        let hooks_installed = crate::hooks::all_installed();
        let any_hooks = hooks.iter().any(|h| h.installed);
        let dev_build = crate::restart::source_dir().is_some();
        let staged = crate::update::staged();
        for (title, action) in commands() {
            if hooks_installed
                && matches!(action, PaletteAction::App(AppCommand::InstallAgentHooks))
            {
                continue;
            }
            if !any_hooks && matches!(action, PaletteAction::App(AppCommand::UninstallAgentHooks)) {
                continue;
            }
            if !dev_build && matches!(action, PaletteAction::App(AppCommand::RebuildRestart)) {
                continue;
            }
            if dev_build && matches!(action, PaletteAction::App(AppCommand::CheckForUpdates)) {
                continue;
            }
            // a downloaded update runs on the next launch
            let title = match (&action, &staged) {
                (PaletteAction::App(AppCommand::Restart), Some(v)) => format!("Restart to Update to {v}"),
                _ => title.to_string(),
            };
            let shortcut = match &action {
                PaletteAction::Key(k) => self.keys.shortcut_for(k),
                PaletteAction::App(c) => self.keys.shortcut_for(&c.action()),
                _ => None,
            };
            items.push(Item {
                kind: Kind::Command,
                title: title.into(),
                subtitle: String::new(),
                shortcut,
                status: None,
                action,
            });
        }
        items
    }

    /// Items for the current query/scope, best match first.
    pub(crate) fn palette_results(&self) -> Vec<Item> {
        let Some(Overlay::Palette(state)) = &self.overlay else {
            return vec![];
        };
        let items: Vec<Item> = self
            .palette_items()
            .into_iter()
            .filter(|i| state.scope.admits(i.kind))
            .collect();
        let query = state.query.trim();
        if query.is_empty() {
            let mut items = items;
            items.sort_by_key(|i| i.kind); // stable: keeps provider order within a section
            return items;
        }
        // before fuzzy filtering, which would drop the database row itself
        let sql_item = sql_query(state.scope, query, &items);
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = vec![];
        let mut scored: Vec<(u32, Item)> = items
            .into_iter()
            .filter_map(|item| {
                let hay = format!("{} {}", item.title, item.subtitle);
                let score = pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher)?;
                // a hit on the name outranks one on the subtitle (e.g. "default"
                // finds the workspace before every pane that lives in it)
                let title = pattern
                    .score(Utf32Str::new(&item.title, &mut buf), &mut matcher)
                    .map_or(0, |s| s * 2);
                let bonus = if item.kind == Kind::Workspace { score / 2 } else { 0 };
                Some((score.max(title) + bonus, item))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.kind.cmp(&b.1.kind)));
        let mut out: Vec<Item> = scored.into_iter().map(|(_, i)| i).collect();
        if let Some(item) = adhoc_ssh(state.scope, query, &out) {
            out.push(item);
        }
        if let Some(item) = sql_item {
            out.insert(0, item);
        }
        out
    }

    pub(crate) fn palette_key(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let count = self.palette_results().len();
        let Some(Overlay::Palette(state)) = self.overlay.as_mut() else {
            return;
        };
        let mut moved = false;
        match ks.key.as_str() {
            "escape" => {
                self.overlay = None;
                return;
            }
            "enter" => {
                let send_from = state.send_from;
                let send_feedback = state.send_feedback;
                let move_tab = state.move_tab;
                let new_ws = (state.scope == Scope::Workspaces)
                    .then(|| state.query.trim().to_string())
                    .filter(|q| !q.is_empty());
                let pick = self.palette_results().get(state_selected(&self.overlay)).cloned();
                self.overlay = None;
                match pick.map(|i| i.action) {
                    Some(PaletteAction::Workspace(name)) if move_tab.is_some() => {
                        self.move_tab_to_workspace(move_tab.unwrap(), name);
                        self.focus.focus(window);
                    }
                    Some(PaletteAction::FocusPane(to)) if send_feedback => {
                        self.send_feedback_to(to, window, cx);
                    }
                    Some(PaletteAction::FocusPane(to)) if send_from.is_some() => {
                        self.send_output(send_from.unwrap(), to, window, cx);
                    }
                    Some(PaletteAction::Harness(name)) => {
                        let placement = if m.platform {
                            crate::launch::Placement::Tab
                        } else {
                            crate::launch::Placement::Split
                        };
                        self.launch_harness(Some(&name), placement, window, cx);
                        self.focus.focus(window);
                    }
                    Some(PaletteAction::Ssh(host)) => {
                        let placement = if m.platform {
                            crate::launch::Placement::Tab
                        } else {
                            crate::launch::Placement::Split
                        };
                        self.open_ssh(&host, placement, window, cx);
                        self.focus.focus(window);
                    }
                    Some(PaletteAction::Sql { conn, query }) => {
                        self.open_sql(&conn, query, m.platform, window, cx);
                        self.focus.focus(window);
                    }
                    Some(PaletteAction::Recipe(name)) => {
                        let placement = if m.platform {
                            crate::launch::Placement::Tab
                        } else {
                            crate::launch::Placement::Split
                        };
                        self.run_recipe(&name, placement, window, cx);
                        self.focus.focus(window);
                    }
                    Some(action) => self.run_palette_action(action, window, cx),
                    // no match in the workspace picker: create one with the typed name
                    None => {
                        if let Some(name) = new_ws {
                            match move_tab {
                                Some(tab) => self.move_tab_to_workspace(tab, name),
                                None => self.run_palette_action(
                                    PaletteAction::Workspace(name),
                                    window,
                                    cx,
                                ),
                            }
                        }
                    }
                }
                return;
            }
            "up" => {
                state.selected = state.selected.saturating_sub(1);
                moved = true;
            }
            "down" | "tab" => {
                state.selected = (state.selected + 1).min(count.saturating_sub(1));
                moved = true;
            }
            "p" | "k" if m.control => {
                state.selected = state.selected.saturating_sub(1);
                moved = true;
            }
            "n" | "j" if m.control => {
                state.selected = (state.selected + 1).min(count.saturating_sub(1));
                moved = true;
            }
            "backspace" => {
                if state.query.is_empty() {
                    state.scope = Scope::All;
                } else if m.alt || m.platform {
                    state.query.clear();
                } else {
                    state.query.pop();
                }
                state.selected = 0;
            }
            "v" if m.platform => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    state.query.push_str(t.lines().next().unwrap_or(""));
                    state.selected = 0;
                }
            }
            _ if !m.control && !m.platform => {
                if let Some(c) = &ks.key_char {
                    if c == ">" && state.query.is_empty() {
                        state.scope = Scope::Commands; // VS Code style
                    } else {
                        state.query.push_str(c);
                    }
                    state.selected = 0;
                }
            }
            _ => {}
        }
        if moved {
            let row = self.palette_row_of(state_selected(&self.overlay));
            if let Some(Overlay::Palette(state)) = &self.overlay {
                state.scroll.scroll_to_item(row);
            }
        }
    }

    /// Index of the selected result among the rendered children (section headers included).
    pub(crate) fn palette_row_of(&self, selected: usize) -> usize {
        let Some(Overlay::Palette(state)) = &self.overlay else {
            return selected;
        };
        if !state.query.trim().is_empty() {
            return selected;
        }
        let results = self.palette_results();
        let mut row = 0;
        let mut last = None;
        for (i, item) in results.iter().enumerate() {
            if last != Some(item.kind) {
                row += 1; // header
                last = Some(item.kind);
            }
            if i == selected {
                return row;
            }
            row += 1;
        }
        row
    }

    pub(crate) fn run_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PaletteAction::FocusPane(id) => self.focus_pane(id, window, cx),
            PaletteAction::FocusTab { window: wid, tab } => {
                self.focus_window(wid);
                if let Some(mut w) = Mux::get().get_window_mut(wid) {
                    if let Some(idx) = w.get_tab_idx_for_id(tab) {
                        w.remember_and_set_active_tab_idx(idx);
                    }
                }
            }
            PaletteAction::FocusWindow(wid) => self.focus_window(wid),
            PaletteAction::Workspace(name) => self.switch_workspace(Some(name), None),
            PaletteAction::App(c) => self.run_app_command(c, window, cx),
            PaletteAction::Key(k) => self.perform(k, None, window, cx),
            PaletteAction::Harness(name) => {
                self.launch_harness(Some(&name), crate::launch::Placement::Split, window, cx)
            }
            PaletteAction::Recipe(name) => {
                self.run_recipe(&name, crate::launch::Placement::Split, window, cx)
            }
            PaletteAction::Ssh(host) => {
                self.open_ssh(&host, crate::launch::Placement::Split, window, cx)
            }
            PaletteAction::Sql { conn, query } => self.open_sql(&conn, query, false, window, cx),
        }
        self.focus.focus(window);
        cx.notify();
    }

    pub(crate) fn run_app_command(
        &mut self,
        c: AppCommand,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match c {
            AppCommand::ToggleSidebar => {
                // the git panel occupies the sidebar's slot: replace it with agents
                if self.diff_open {
                    self.close_diff_panel();
                    self.sidebar_open = true;
                } else {
                    self.sidebar_open = !self.sidebar_open;
                }
                if !self.sidebar_open && self.region == crate::regions::Region::Sidebar {
                    self.region = crate::regions::Region::Terminal;
                }
                let open = self.sidebar_open;
                crate::settings::update(|s| s.show_sidebar = open);
            }
            AppCommand::OpenSettings => self.open_settings(cx),
            AppCommand::AgentOverview => self.toggle_overview(),
            AppCommand::ShowShortcuts => {
                self.overlay = Some(crate::app::Overlay::Shortcuts);
            }
            AppCommand::Restart => self.restart(cx),
            AppCommand::RebuildRestart => self.rebuild_and_restart(cx),
            AppCommand::CheckForUpdates => {
                self.flash("Checking for updates…");
                crate::update::check_now();
            }
            AppCommand::RunRecipe => self.open_palette(Scope::Recipes, cx),
            AppCommand::SshHost => self.open_palette(Scope::Hosts, cx),
            AppCommand::CommandPalette => self.open_palette(Scope::All, cx),
            AppCommand::FindAgent => self.open_palette(Scope::Agents, cx),
            AppCommand::RenameAgent => self.start_agent_rename(None, cx),
            AppCommand::FindTab => self.open_palette(Scope::Tabs, cx),
            AppCommand::SwitchWorkspace => self.open_palette(Scope::Workspaces, cx),
            AppCommand::NewWorkspace => self.prompt_new_workspace(cx),
            AppCommand::RenameTab => self.prompt_rename(false, cx),
            AppCommand::RenameWorkspace => self.prompt_rename(true, cx),
            AppCommand::MoveTabToWorkspace => {
                if let Some(tab) = self.active_tab().map(|t| t.tab_id()) {
                    self.open_palette(Scope::Workspaces, cx);
                    if let Some(Overlay::Palette(p)) = self.overlay.as_mut() {
                        p.move_tab = Some(tab);
                    }
                }
            }
            AppCommand::BalancePanes => {
                if let Some(tab) = self.active_tab() {
                    crate::app::balance_panes(&tab);
                }
                cx.notify();
            }
            AppCommand::MovePaneToNewWindow => {
                if let Some(pane) = self.active_pane().map(|p| p.pane_id()) {
                    // a new window still belongs to this workspace
                    let workspace = Some(Mux::get().active_workspace());
                    promise::spawn::spawn(async move {
                        match Mux::get().move_pane_to_new_tab(pane, None, workspace).await {
                            Ok((tab, window)) => crate::app::send_event(
                                crate::app::AppEvent::TabMoved {
                                    window,
                                    tab: tab.tab_id(),
                                    follow: true,
                                },
                            ),
                            Err(e) => crate::app::send_event(crate::app::AppEvent::Flash(
                                format!("move failed: {e:#}"),
                            )),
                        }
                    })
                    .detach();
                }
            }
            AppCommand::NewTerminal => self.new_terminal(cx),
            AppCommand::NewAgent => {
                self.launch_harness(None, crate::launch::Placement::Split, _window, cx)
            }
            AppCommand::NewAgentTab => {
                self.launch_harness(None, crate::launch::Placement::Tab, _window, cx)
            }
            AppCommand::ToggleDiffPanel => {
                // one left slot: opening the git panel replaces the sidebar, and
                // closing it leaves the slot empty (no falling back to agents)
                if self.diff_open {
                    self.close_diff_panel();
                } else {
                    self.diff_open = true;
                    self.sidebar_open = false;
                    self.region = crate::regions::Region::Diff;
                }
            }
            AppCommand::SendOutputToAgent => self.send_output_to_agent(_window, cx),
            AppCommand::SelectError => self.start_error_select(cx),
            AppCommand::OpenEditor => self.open_editor_here(_window, cx),
            AppCommand::CreatePullRequest => self.pull_request_command(),
            AppCommand::InstallAgentHooks => match crate::hooks::install_all() {
                Ok(done) => self.flash(format!("Hooks installed · {}", hook_labels(&done))),
                Err(e) => self.flash(format!("Couldn't install hooks: {e:#}")),
            },
            AppCommand::UninstallAgentHooks => match crate::hooks::uninstall_all() {
                Ok(done) if done.is_empty() => self.flash("No hooks installed"),
                Ok(done) => self.flash(format!("Hooks removed · {}", hook_labels(&done))),
                Err(e) => self.flash(format!("Couldn't remove hooks: {e:#}")),
            },
        }
        cx.notify();
    }

    /// The SQL view on `conn`, in a new tab (or a split to the right).
    pub(crate) fn open_sql(
        &mut self,
        conn: &crate::views::sql::conn::Conn,
        query: Option<String>,
        split: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(q) = query {
            crate::views::sql::run_on_open(&conn.name, q);
        }
        let command = crate::views::spawn_command("sql", &conn.spec_args(), &conn.name);
        let action = if split {
            KeyAssignment::SplitPane(config::keyassignment::SplitPane {
                direction: PaneDirection::Right,
                size: SplitSize::Percent(50),
                command,
                top_level: false,
            })
        } else {
            KeyAssignment::SpawnCommandInNewTab(command)
        };
        self.perform(action, None, window, cx);
    }

    fn focus_window(&mut self, window_id: WindowId) {
        let mux = Mux::get();
        if let Some(ws) = mux
            .get_window(window_id)
            .map(|w| w.get_workspace().to_string())
        {
            if ws != mux.active_workspace() {
                mux.set_active_workspace(&ws);
            }
            self.mux_window_id = Some(window_id);
        }
    }
}

/// "ssh pi@10.0.0.9" (or anything typed in the SSH picker) connects even
/// when no host matches. Listed last so a configured host always wins.
fn adhoc_ssh(scope: Scope, query: &str, results: &[Item]) -> Option<Item> {
    let target = match scope {
        Scope::Hosts => query.strip_prefix("ssh ").unwrap_or(query),
        Scope::All => query.strip_prefix("ssh ")?,
        _ => return None,
    }
    .trim();
    if target.is_empty()
        || results
            .iter()
            .any(|i| matches!(&i.action, PaletteAction::Ssh(h) if h.name == target))
    {
        return None;
    }
    Some(Item {
        kind: Kind::Host,
        title: format!("ssh {target}"),
        subtitle: String::new(),
        shortcut: None,
        status: None,
        action: PaletteAction::Ssh(crate::hosts::Host {
            name: target.to_string(),
            ssh: target.to_string(),
        }),
    })
}

/// "sql neverwait select * from users": open that database and run the query.
fn sql_query(scope: Scope, query: &str, results: &[Item]) -> Option<Item> {
    if scope != Scope::All {
        return None;
    }
    let rest = query.strip_prefix("sql ")?.trim_start();
    let (name, sql) = rest.split_once(char::is_whitespace)?;
    let sql = sql.trim();
    if sql.is_empty() {
        return None;
    }
    let conn = results.iter().find_map(|i| match &i.action {
        PaletteAction::Sql { conn, query: None } if conn.name == name => Some(conn.clone()),
        _ => None,
    })?;
    Some(Item {
        kind: Kind::Database,
        title: sql.to_string(),
        subtitle: conn.name.clone(),
        shortcut: None,
        status: None,
        action: PaletteAction::Sql {
            conn,
            query: Some(sql.to_string()),
        },
    })
}

fn state_selected(overlay: &Option<Overlay>) -> usize {
    match overlay {
        Some(Overlay::Palette(s)) => s.selected,
        _ => 0,
    }
}

fn hook_labels(hs: &[crate::hooks::Harness]) -> String {
    hs.iter().map(|h| h.label()).collect::<Vec<_>>().join(", ")
}
