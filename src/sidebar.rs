//! The sidebar (⌘B) is a stack of sources: live views of what's going on
//! (agents, dev servers, ...). Each source has its own background poller and
//! produces items; a source with no items shows no section. Users pick which
//! sources appear, and in what order, with the `sidebar` setting. Agents and
//! servers show by default; Actions is opt-in.
//!
//! Adding a source: a `Source` variant (+ `ALL`, `id`, `title`), an `Item`
//! variant, its items in `TermApp::source_items`, its keys in
//! `TermApp::sidebar_activate`, and a row in `ui/view.rs`. Pollers that cost
//! something should check `settings::get().sidebar_enabled(..)` and idle
//! while their source is off.

use crate::agents::Agent;
use crate::app::TermApp;
use crate::regions::Region;
use crate::servers::Server;
use gpui::{Context, Window};
use mux::Mux;

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

    /// ↵ / `o` on a sidebar row (keyboard cursor).
    pub(crate) fn sidebar_activate(&mut self, item: Item, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        match item {
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

    /// Follow a live run, or show a finished one's log, in a split.
    pub(crate) fn open_run(&mut self, r: &crate::actions::Run, window: &mut Window, cx: &mut Context<Self>) {
        let harness = crate::settings::Harness {
            name: "gh".into(),
            argv: r.view_command(),
        };
        self.run_command_in_split(&harness, window, cx);
    }
}
