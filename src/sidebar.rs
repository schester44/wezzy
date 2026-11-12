//! The sidebar (⌘B) is a stack of sources: live views of what's going on
//! (agents, dev servers, ...). Each source has its own background poller and
//! produces items; a source with no items shows no section. Users pick which
//! sources appear, and in what order, with the `sidebar` setting. Agents and
//! servers show by default; Actions is opt-in.
//!
//! Adding a source: a `Source` variant (+ `ALL`, `id`, `title`), an `Item`
//! variant, its items in `TermApp::source_items`, its keys in
//! `TermApp::sidebar_activate`, and a row in `ui/sidebar.rs`. Pollers that cost
//! something should check `settings::get().sidebar_enabled(..)` and idle
//! while their source is off.

use crate::agents::Agent;
use crate::app::TermApp;
use crate::regions::Region;
use crate::servers::Server;
use gpui::{Context, KeyDownEvent, Window};
use mux::pane::PaneId;
use mux::Mux;

/// The inline name field on an agent row (Rename agent / `r`).
pub struct AgentRename {
    pub pane_id: PaneId,
    pub input: String,
    /// Where keyboard focus goes back to when done.
    back: Region,
    /// The sidebar was opened for this; close it again when done.
    hide_sidebar: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Agents,
    Servers,
    /// GitHub Actions runs for the active pane's branch (actions.rs).
    Actions,
}

impl Source {
    pub const ALL: &[Source] = &[Source::Agents, Source::Servers, Source::Actions];

    /// Name in settings.json / `setup()`.
    pub fn id(self) -> &'static str {
        match self {
            Source::Agents => "agents",
            Source::Servers => "servers",
            Source::Actions => "actions",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.id() == id)
    }

    /// Section header.
    pub fn title(self) -> &'static str {
        match self {
            Source::Agents => "Agents",
            Source::Servers => "Servers",
            Source::Actions => "Actions",
        }
    }

    /// Shown unless the user configures otherwise.
    pub fn default_list() -> Vec<Source> {
        vec![Source::Agents, Source::Servers]
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    Agent(Agent),
    Server(Server),
    Run(crate::actions::Run),
}

pub struct Section {
    pub source: Source,
    pub items: Vec<Item>,
    /// Folded to its header: rows hidden, and skipped by the keyboard cursor.
    pub collapsed: bool,
}

impl TermApp {
    /// Enabled sources that have something to show, in display order. The
    /// view and the keyboard cursor both walk this.
    pub(crate) fn sidebar_sections(&self) -> Vec<Section> {
        let s = crate::settings::get();
        s.sidebar()
            .into_iter()
            .map(|source| Section {
                source,
                items: self.source_items(source),
                collapsed: s.sidebar_collapsed.contains(&source),
            })
            .filter(|s| !s.items.is_empty())
            .collect()
    }

    /// Every visible sidebar row, flattened in display order.
    pub(crate) fn sidebar_items(&self) -> Vec<Item> {
        self.sidebar_sections()
            .into_iter()
            .filter(|s| !s.collapsed)
            .flat_map(|s| s.items)
            .collect()
    }

    /// Fold or unfold a section (click on its header). Remembered in settings.json.
    pub(crate) fn toggle_sidebar_section(&mut self, source: Source, cx: &mut Context<Self>) {
        crate::settings::update(|s| {
            if s.sidebar_collapsed.contains(&source) {
                s.sidebar_collapsed.retain(|x| *x != source);
            } else {
                s.sidebar_collapsed.push(source);
            }
        });
        cx.notify();
    }

    fn source_items(&self, source: Source) -> Vec<Item> {
        match source {
            // agents from every workspace, so you can see who needs you
            Source::Agents => crate::agents::collect(self.active_pane().map(|p| p.pane_id()))
                .into_iter()
                .map(Item::Agent)
                .collect(),
            Source::Servers => self.workspace_servers().into_iter().map(Item::Server).collect(),
            Source::Actions => crate::actions::runs().into_iter().map(Item::Run).collect(),
        }
    }

    /// Dev servers started from panes in the active workspace.
    pub(crate) fn workspace_servers(&self) -> Vec<Server> {
        let mux = Mux::get();
        let panes: Vec<_> = mux
            .iter_windows_in_workspace(&mux.active_workspace())
            .into_iter()
            .filter_map(|w| mux.get_window(w).map(|w| w.iter_tabs().cloned().collect::<Vec<_>>()))
            .flatten()
            .flat_map(|tab| tab.iter_panes_ignoring_zoom().into_iter().map(|p| p.pane))
            .collect();
        crate::servers::for_panes(&panes)
    }

    /// ↵ / `o` / `r` on a sidebar row (keyboard cursor).
    pub(crate) fn sidebar_activate(&mut self, item: Item, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match item {
            Item::Agent(a) if key == "r" => self.start_agent_rename(Some(a.pane_id), cx),
            _ if key == "r" => {}
            Item::Agent(a) => {
                self.region = Region::Terminal;
                self.focus_pane(a.pane_id, window, cx);
            }
            Item::Server(s) if key == "o" => cx.open_url(&format!("http://localhost:{}", s.port)),
            Item::Server(s) => {
                self.region = Region::Terminal;
                self.focus_pane(s.pane_id, window, cx);
            }
            Item::Run(r) if key == "o" => cx.open_url(&r.url),
            Item::Run(r) => {
                self.region = Region::Terminal;
                self.open_run(&r, window, cx);
            }
        }
    }

    /// Name an agent in place on its sidebar row (Rename agent / `r`): `pane_id`,
    /// else the one under the sidebar cursor, else the active pane's.
    pub(crate) fn start_agent_rename(&mut self, pane_id: Option<PaneId>, cx: &mut Context<Self>) {
        let selected = match self.sidebar_items().get(self.sidebar_cursor) {
            Some(Item::Agent(a)) if self.region == Region::Sidebar => Some(a.pane_id),
            _ => None,
        };
        let target = pane_id
            .or(selected)
            .or(self.active_pane().map(|p| p.pane_id()))
            .and_then(|id| crate::agents::collect(None).into_iter().find(|a| a.pane_id == id));
        let Some(agent) = target else {
            self.flash("No agent in this pane");
            return;
        };
        let settings = crate::settings::get();
        if !settings.sidebar().contains(&Source::Agents) {
            self.flash("Agents are off in the sidebar");
            return;
        }
        // the field lives on the row, so the row has to be on screen
        if settings.sidebar_collapsed.contains(&Source::Agents) {
            crate::settings::update(|s| s.sidebar_collapsed.retain(|x| *x != Source::Agents));
        }
        self.overlay = None;
        self.ws_menu_open = false;
        if let Some(tab) = self.active_tab().filter(|t| t.get_zoomed_pane().is_some()) {
            tab.set_zoomed(false);
        }
        // the git panel shares the sidebar's slot (closing it moves focus to the terminal)
        if self.diff_open {
            self.close_diff_panel();
        }
        let back = self.region;
        let hide_sidebar = !self.sidebar_open;
        self.sidebar_open = true;
        self.region = Region::Sidebar;
        if let Some(i) = self
            .sidebar_items()
            .iter()
            .position(|it| matches!(it, Item::Agent(a) if a.pane_id == agent.pane_id))
        {
            self.sidebar_cursor = i;
        }
        self.agent_rename = Some(AgentRename {
            pane_id: agent.pane_id,
            input: agent.name.clone().unwrap_or_default(),
            back,
            hide_sidebar,
        });
        cx.notify();
    }

    /// Close the name field, saving what was typed when `save` (blank clears).
    pub(crate) fn finish_agent_rename(&mut self, save: bool) {
        let Some(r) = self.agent_rename.take() else {
            return;
        };
        if save {
            crate::agents::names::set(r.pane_id, &r.input);
        }
        self.region = r.back;
        if r.hide_sidebar {
            self.sidebar_open = false;
            if self.region == Region::Sidebar {
                self.region = Region::Terminal;
            }
        }
    }

    /// Every key goes to the name field while it's open.
    pub(crate) fn agent_rename_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        // the agent quit while its row was being edited: the field is gone
        let pane_id = self.agent_rename.as_ref().map(|r| r.pane_id);
        if !crate::agents::collect(None).iter().any(|a| Some(a.pane_id) == pane_id) {
            self.finish_agent_rename(false);
            cx.notify();
            return;
        }
        let Some(r) = self.agent_rename.as_mut() else {
            return;
        };
        match ks.key.as_str() {
            "escape" => self.finish_agent_rename(false),
            "[" if m.control => self.finish_agent_rename(false),
            "enter" => self.finish_agent_rename(true),
            "backspace" if m.alt || m.platform => r.input.clear(),
            "backspace" => {
                r.input.pop();
            }
            "v" if m.platform => {
                if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                    r.input.push_str(t.lines().next().unwrap_or("").trim());
                }
            }
            _ if !m.control && !m.platform => {
                if let Some(c) = &ks.key_char {
                    r.input.push_str(c);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Follow a live run, or show a finished one's log, in a split.
    pub(crate) fn open_run(&mut self, r: &crate::actions::Run, window: &mut Window, cx: &mut Context<Self>) {
        let harness = crate::settings::Harness {
            name: "gh".into(),
            argv: r.view_command(),
        };
        self.run_command_in_split(&harness, window, cx);
    }
}
