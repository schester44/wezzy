//! App settings. Edited in the Settings panel (⌘,) and stored in
//! ~/.config/wezzy/settings.json. The Lua config can also set them, and wins:
//!
//! ```lua
//! require("wezzy").setup({
//!   close_confirmation = "smart",     -- "smart" | "never" | "always"
//!   default_harness = "claude",       -- what ⌘I launches
//!   agent_layout = "bsp",             -- "bsp" | "master-stack" (where ⌘I splits)
//!   editor = { "nvim", "+{line}", "{file}" }, -- ⌘-click file:line opens this in a split
//!   auto_update = true,               -- install new releases in the background (see update.rs)
//!   notifications = true,             -- macOS notifications + Dock badge for agents (see notify.rs)
//!   pane_cards = true,                -- panes as rounded cards with gaps between them (see render.rs)
//!   sidebar = { "agents", "servers", "actions" }, -- ⌘B sections, top to bottom (see sidebar.rs)
//!   harnesses = {                     -- name = argv; merged over the built-ins
//!     claude = { "claude" },
//!     codex = { "codex", "--full-auto" },
//!   },
//!   recipes = {                       -- palette prompts that open an agent (see recipes.rs)
//!     { name = "Review changes", harness = "claude", prompt = "/code-review" },
//!   },
//!   hosts = {                         -- SSH hosts for the palette (see hosts.rs)
//!     { name = "devbox", ssh = "dev@10.0.0.5" },
//!   },
//!   workspaces = {                    -- pickable before they're open (see workspaces.rs)
//!     { name = "neverwait", cwd = "~/work/neverwait" },
//!   },
//! })
//! ```
//!
//! Re-applied on every config reload; anything not passed keeps its default.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseConfirmation {
    /// Prompt only if something other than a shell is running (WezTerm's
    /// `skip_close_confirmation_for_processes_named` decides what counts as a shell).
    Smart,
    Never,
    Always,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentLayout {
    /// Split the active pane across the split that made it (side by side,
    /// then stacked, then side by side…); a lone pane splits along its
    /// longer side.
    Bsp,
    /// First pane is the master on the left; new agents stack on the right.
    MasterStack,
}

#[derive(Clone, Debug)]
pub struct Harness {
    pub name: String,
    pub argv: Vec<String>,
}

impl Harness {
    pub fn label(&self) -> String {
        crate::agents::label_for(&self.name)
            .map(str::to_string)
            .unwrap_or_else(|| {
                let mut c = self.name.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            })
    }
}

/// A database for the SQL view: a name and a `postgres://` URL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Database {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub close_confirmation: CloseConfirmation,
    pub agent_layout: AgentLayout,
    pub default_harness: String,
    /// User-configured harnesses (name, argv). Built-ins fill in the rest.
    pub harnesses: Vec<(String, Vec<String>)>,
    /// argv template for opening files; {file} {line} {col}
    pub editor: Vec<String>,
    pub show_sidebar: bool,
    /// Sidebar sources, top to bottom; None = `Source::default_list()`.
    sidebar: Option<Vec<crate::sidebar::Source>>,
    /// Sidebar sections folded to their header (clicked in the sidebar).
    pub sidebar_collapsed: Vec<crate::sidebar::Source>,
    /// macOS system sound played when an agent needs you or finishes;
    /// "" = default (Funk, like pi-chime), "off" = silent.
    pub sound: String,
    /// Check for and install new releases (installed app only).
    pub auto_update: bool,
    /// macOS notifications and the Dock badge when an agent needs you while
    /// Wezzy is in the background.
    pub notifications: bool,
    /// Panes drawn as rounded cards with a gap between them, instead of flat
    /// with a hairline divider.
    pub pane_cards: bool,
    /// settings.json recipes, then the Lua config's
    pub recipes: Vec<crate::recipes::Recipe>,
    /// the settings.json subset, so saving from the UI doesn't copy Lua ones into the file
    file_recipes: Vec<crate::recipes::Recipe>,
    /// settings.json hosts, then the Lua config's
    pub hosts: Vec<crate::hosts::Host>,
    file_hosts: Vec<crate::hosts::Host>,
    /// settings.json databases, then the Lua config's
    pub databases: Vec<Database>,
    file_databases: Vec<Database>,
    /// settings.json saved workspaces, then the Lua config's
    pub workspaces: Vec<crate::workspaces::Saved>,
    file_workspaces: Vec<crate::workspaces::Saved>,
}

impl Settings {
    const DEFAULT: Self = Self {
        close_confirmation: CloseConfirmation::Smart,
        agent_layout: AgentLayout::Bsp,
        default_harness: String::new(),
        harnesses: Vec::new(),
        editor: Vec::new(),
        show_sidebar: true,
        sidebar: None,
        sidebar_collapsed: Vec::new(),
        sound: String::new(),
        auto_update: true,
        notifications: true,
        pane_cards: true,
        recipes: Vec::new(),
        file_recipes: Vec::new(),
        hosts: Vec::new(),
        file_hosts: Vec::new(),
        databases: Vec::new(),
        file_databases: Vec::new(),
        workspaces: Vec::new(),
        file_workspaces: Vec::new(),
    };

    pub fn sound(&self) -> Option<String> {
        match self.sound.as_str() {
            "" => Some("Funk".into()),
            "off" => None,
            s => Some(s.to_string()),
        }
    }

    /// argv template for opening files. Default: $EDITOR (vi-family gets +{line}).
    pub fn editor(&self) -> Vec<String> {
        if !self.editor.is_empty() {
            return self.editor.clone();
        }
        let bin = std::env::var("EDITOR")
            .ok()
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "nvim".into());
        let mut argv: Vec<String> = shell_words::split(&bin).unwrap_or_else(|_| vec![bin.clone()]);
        let name = std::path::Path::new(&argv[0])
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        if matches!(name.as_str(), "nvim" | "vim" | "vi") {
            argv.push("+{line}".into());
        }
        argv.push("{file}".into());
        argv
    }

    /// Configured harnesses plus built-ins whose binary is on PATH, sorted with
    /// the default first.
    pub fn harnesses(&self) -> Vec<Harness> {
        const BUILTIN: &[&str] = &["claude", "codex", "pi", "opencode", "gemini", "aider", "amp"];
        let mut out: Vec<Harness> = self
            .harnesses
            .iter()
            .map(|(name, argv)| Harness {
                name: name.clone(),
                argv: argv.clone(),
            })
            .collect();
        for b in BUILTIN {
            if !out.iter().any(|h| h.name == *b) && on_path(b) {
                out.push(Harness {
                    name: b.to_string(),
                    argv: vec![b.to_string()],
                });
            }
        }
        let default = self.default_harness_name();
        out.sort_by_key(|h| (h.name != default, h.name.clone()));
        out
    }

    /// Sidebar sources, top to bottom.
    pub fn sidebar(&self) -> Vec<crate::sidebar::Source> {
        self.sidebar
            .clone()
            .unwrap_or_else(crate::sidebar::Source::default_list)
    }

    pub fn sidebar_enabled(&self, source: crate::sidebar::Source) -> bool {
        self.sidebar().contains(&source)
    }

    /// Show or hide one source; a newly shown one goes at the bottom.
    pub fn set_sidebar_source(&mut self, source: crate::sidebar::Source, on: bool) {
        let mut list = self.sidebar();
        list.retain(|s| *s != source);
        if on {
            list.push(source);
        }
        self.sidebar = Some(list);
    }

    pub fn default_harness_name(&self) -> String {
        if self.default_harness.is_empty() {
            "claude".into()
        } else {
            self.default_harness.clone()
        }
    }

    pub fn harness(&self, name: &str) -> Option<Harness> {
        self.harnesses().into_iter().find(|h| h.name == name)
    }
}

pub(crate) fn on_path(bin: &str) -> bool {
    let home = config::HOME_DIR.clone();
    let extra = [home.join(".local/bin"), home.join(".cargo/bin"), home.join("go/bin")];
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(extra)
        .any(|d| d.join(bin).is_file())
}

static SETTINGS: Mutex<Settings> = Mutex::new(Settings::DEFAULT);

/// When we last wrote settings.json. WezTerm watches the config file's whole
/// directory, so our own writes trigger a config reload.
static LAST_WRITE: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Whether a config reload right now was most likely caused by our own write
/// (WezTerm debounces for 200ms, then re-evaluates the config).
pub fn just_saved() -> bool {
    LAST_WRITE
        .lock()
        .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2))
}

pub fn get() -> Settings {
    SETTINGS.lock().clone()
}

// -- settings.json -----------------------------------------------------------

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Stored {
    close_confirmation: Option<String>,
    agent_layout: Option<String>,
    default_harness: Option<String>,
    editor: Option<Vec<String>>,
    show_sidebar: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sidebar: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sidebar_collapsed: Option<Vec<String>>,
    sound: Option<String>,
    auto_update: Option<bool>,
    notifications: Option<bool>,
    pane_cards: Option<bool>,
    recipes: Option<Vec<StoredRecipe>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hosts: Option<Vec<StoredHost>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    databases: Option<Vec<Database>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspaces: Option<Vec<StoredWorkspace>>,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredWorkspace {
    name: String,
    cwd: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredHost {
    name: String,
    ssh: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredRecipe {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    harness: Option<String>,
    prompt: String,
}

pub fn file_path() -> PathBuf {
    crate::paths::config_dir().join("settings.json")
}

/// Known source ids in order, unknown and repeated ones dropped.
fn parse_sources(ids: &[String]) -> Vec<crate::sidebar::Source> {
    let mut out = vec![];
    for id in ids {
        match crate::sidebar::Source::from_id(id) {
            Some(src) if !out.contains(&src) => out.push(src),
            Some(_) => {}
            None => log::warn!("settings.json: unknown sidebar source {id:?}"),
        }
    }
    out
}

fn from_file() -> Settings {
    let mut s = Settings::DEFAULT;
    let Ok(text) = std::fs::read_to_string(file_path()) else {
        return s;
    };
    let Ok(st) = serde_json::from_str::<Stored>(&text) else {
        log::warn!("ignoring malformed {}", file_path().display());
        return s;
    };
    if let Some(v) = st.close_confirmation.as_deref().and_then(parse_close) {
        s.close_confirmation = v;
    }
    if let Some(v) = st.agent_layout.as_deref().and_then(parse_layout) {
        s.agent_layout = v;
    }
    if let Some(v) = st.default_harness {
        s.default_harness = v;
    }
    if let Some(v) = st.editor.filter(|v| !v.is_empty()) {
        s.editor = v;
    }
    if let Some(v) = st.show_sidebar {
        s.show_sidebar = v;
    }
    if let Some(v) = st.sidebar {
        s.sidebar = Some(parse_sources(&v));
    }
    if let Some(v) = st.sidebar_collapsed {
        s.sidebar_collapsed = parse_sources(&v);
    }
    if let Some(v) = st.sound {
        s.sound = v;
    }
    if let Some(v) = st.auto_update {
        s.auto_update = v;
    }
    if let Some(v) = st.notifications {
        s.notifications = v;
    }
    if let Some(v) = st.pane_cards {
        s.pane_cards = v;
    }
    if let Some(v) = st.recipes {
        s.file_recipes = v
            .into_iter()
            .map(|r| crate::recipes::Recipe {
                name: r.name,
                harness: r.harness,
                prompt: r.prompt,
            })
            .collect();
        s.recipes = s.file_recipes.clone();
    }
    if let Some(v) = st.hosts {
        s.file_hosts = v
            .into_iter()
            .map(|h| crate::hosts::Host { name: h.name, ssh: h.ssh })
            .collect();
        s.hosts = s.file_hosts.clone();
    }
    if let Some(v) = st.databases {
        s.file_databases = v;
        s.databases = s.file_databases.clone();
    }
    if let Some(v) = st.workspaces {
        s.file_workspaces = v
            .into_iter()
            .map(|w| crate::workspaces::Saved { name: w.name, cwd: w.cwd })
            .collect();
        s.workspaces = s.file_workspaces.clone();
    }
    s
}

/// Load settings.json. Call before the Lua config is evaluated.
pub fn load() {
    *SETTINGS.lock() = from_file();
}

/// Change a setting from the UI and persist it.
pub fn update(f: impl FnOnce(&mut Settings)) {
    let mut s = SETTINGS.lock();
    f(&mut s);
    let st = Stored {
        close_confirmation: Some(close_name(s.close_confirmation).into()),
        agent_layout: Some(layout_name(s.agent_layout).into()),
        default_harness: (!s.default_harness.is_empty()).then(|| s.default_harness.clone()),
        editor: (!s.editor.is_empty()).then(|| s.editor.clone()),
        show_sidebar: Some(s.show_sidebar),
        sidebar: s
            .sidebar
            .as_ref()
            .map(|v| v.iter().map(|x| x.id().to_string()).collect()),
        sidebar_collapsed: (!s.sidebar_collapsed.is_empty())
            .then(|| s.sidebar_collapsed.iter().map(|x| x.id().to_string()).collect()),
        sound: (!s.sound.is_empty()).then(|| s.sound.clone()),
        auto_update: Some(s.auto_update),
        notifications: Some(s.notifications),
        pane_cards: Some(s.pane_cards),
        recipes: (!s.file_recipes.is_empty()).then(|| {
            s.file_recipes
                .iter()
                .map(|r| StoredRecipe {
                    name: r.name.clone(),
                    harness: r.harness.clone(),
                    prompt: r.prompt.clone(),
                })
                .collect()
        }),
        hosts: (!s.file_hosts.is_empty()).then(|| {
            s.file_hosts
                .iter()
                .map(|h| StoredHost {
                    name: h.name.clone(),
                    ssh: h.ssh.clone(),
                })
                .collect()
        }),
        databases: (!s.file_databases.is_empty()).then(|| s.file_databases.clone()),
        workspaces: (!s.file_workspaces.is_empty()).then(|| {
            s.file_workspaces
                .iter()
                .map(|w| StoredWorkspace {
                    name: w.name.clone(),
                    cwd: w.cwd.clone(),
                })
                .collect()
        }),
    };
    drop(s);
    let path = file_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if let Ok(text) = serde_json::to_string_pretty(&st) {
        if std::fs::write(&path, text + "\n").is_ok() {
            *LAST_WRITE.lock() = Some(std::time::Instant::now());
        }
    }
}

fn parse_close(v: &str) -> Option<CloseConfirmation> {
    Some(match v {
        "smart" => CloseConfirmation::Smart,
        "never" => CloseConfirmation::Never,
        "always" => CloseConfirmation::Always,
        _ => return None,
    })
}

fn parse_layout(v: &str) -> Option<AgentLayout> {
    Some(match v {
        "bsp" => AgentLayout::Bsp,
        "master-stack" => AgentLayout::MasterStack,
        _ => return None,
    })
}

pub fn close_name(c: CloseConfirmation) -> &'static str {
    match c {
        CloseConfirmation::Smart => "smart",
        CloseConfirmation::Never => "never",
        CloseConfirmation::Always => "always",
    }
}

pub fn layout_name(l: AgentLayout) -> &'static str {
    match l {
        AgentLayout::Bsp => "bsp",
        AgentLayout::MasterStack => "master-stack",
    }
}

/// Lua: `wezzy.setup{ ... }`
pub fn lua_setup(_: &mlua::Lua, opts: mlua::Table) -> mlua::Result<()> {
    let bad = |field: &str, want: &str, got: &str| {
        mlua::Error::external(format!("wezzy.setup: {field} must be {want}, got {got:?}"))
    };
    // start from settings.json; whatever the config passes overrides it
    let mut s = from_file();
    if let Some(v) = opts.get::<_, Option<String>>("close_confirmation")? {
        s.close_confirmation = match v.as_str() {
            "smart" => CloseConfirmation::Smart,
            "never" => CloseConfirmation::Never,
            "always" => CloseConfirmation::Always,
            other => return Err(bad("close_confirmation", "\"smart\", \"never\" or \"always\"", other)),
        };
    }
    if let Some(v) = opts.get::<_, Option<String>>("agent_layout")? {
        s.agent_layout = match v.as_str() {
            "bsp" => AgentLayout::Bsp,
            "master-stack" => AgentLayout::MasterStack,
            other => return Err(bad("agent_layout", "\"bsp\" or \"master-stack\"", other)),
        };
    }
    if let Some(v) = opts.get::<_, Option<String>>("default_harness")? {
        s.default_harness = v;
    }
    if let Some(v) = opts.get::<_, Option<Vec<String>>>("editor")? {
        s.editor = v;
    }
    if let Some(v) = opts.get::<_, Option<bool>>("auto_update")? {
        s.auto_update = v;
    }
    if let Some(v) = opts.get::<_, Option<bool>>("notifications")? {
        s.notifications = v;
    }
    if let Some(v) = opts.get::<_, Option<bool>>("pane_cards")? {
        s.pane_cards = v;
    }
    if let Some(v) = opts.get::<_, Option<Vec<String>>>("sidebar")? {
        if let Some(bad_id) = v.iter().find(|id| crate::sidebar::Source::from_id(id).is_none()) {
            let ids: Vec<&str> = crate::sidebar::Source::ALL.iter().map(|x| x.id()).collect();
            return Err(bad("sidebar", &format!("a list of {}", ids.join(", ")), bad_id));
        }
        s.sidebar = Some(parse_sources(&v));
    }
    if let Some(t) = opts.get::<_, Option<mlua::Table>>("harnesses")? {
        for pair in t.pairs::<String, Vec<String>>() {
            let (name, argv) = pair?;
            if !argv.is_empty() {
                s.harnesses.push((name, argv));
            }
        }
    }
    if let Some(t) = opts.get::<_, Option<mlua::Table>>("recipes")? {
        for (i, r) in t.sequence_values::<mlua::Table>().enumerate() {
            let r = r?;
            let name: Option<String> = r.get("name")?;
            let prompt: Option<String> = r.get("prompt")?;
            let (Some(name), Some(prompt)) = (name, prompt) else {
                return Err(bad(&format!("recipes[{}]", i + 1), "a table with name and prompt", "missing field"));
            };
            let recipe = crate::recipes::Recipe {
                name,
                harness: r.get("harness")?,
                prompt,
            };
            // the config wins over a settings.json recipe of the same name
            s.recipes.retain(|x| x.name != recipe.name);
            s.recipes.push(recipe);
        }
    }
    if let Some(t) = opts.get::<_, Option<mlua::Table>>("hosts")? {
        for (i, h) in t.sequence_values::<mlua::Table>().enumerate() {
            let h = h?;
            let name: Option<String> = h.get("name")?;
            let ssh: Option<String> = h.get("ssh")?;
            let (Some(name), Some(ssh)) = (name, ssh) else {
                return Err(bad(&format!("hosts[{}]", i + 1), "a table with name and ssh", "missing field"));
            };
            s.hosts.retain(|x| x.name != name);
            s.hosts.push(crate::hosts::Host { name, ssh });
        }
    }
    if let Some(t) = opts.get::<_, Option<mlua::Table>>("databases")? {
        for (i, d) in t.sequence_values::<mlua::Table>().enumerate() {
            let d = d?;
            let name: Option<String> = d.get("name")?;
            let url: Option<String> = d.get("url")?;
            let (Some(name), Some(url)) = (name, url) else {
                return Err(bad(&format!("databases[{}]", i + 1), "a table with name and url", "missing field"));
            };
            s.databases.retain(|x| x.name != name);
            s.databases.push(Database { name, url });
        }
    }
    if let Some(t) = opts.get::<_, Option<mlua::Table>>("workspaces")? {
        for (i, w) in t.sequence_values::<mlua::Table>().enumerate() {
            let w = w?;
            let name: Option<String> = w.get("name")?;
            let cwd: Option<String> = w.get("cwd")?;
            let (Some(name), Some(cwd)) = (name, cwd) else {
                return Err(bad(&format!("workspaces[{}]", i + 1), "a table with name and cwd", "missing field"));
            };
            s.workspaces.retain(|x| x.name != name);
            s.workspaces.push(crate::workspaces::Saved { name, cwd });
        }
    }
    *SETTINGS.lock() = s;
    Ok(())
}
