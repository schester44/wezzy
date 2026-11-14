//! App-level commands: things WezTerm's `KeyAssignment` has no variant for
//! (sidebar, palette, and later browser/markdown/visualizer surfaces).
//!
//! They ride on `EmitEvent("wezzy.<id>")`, so they bind exactly like any
//! WezTerm action, including inside leader and key tables:
//!
//! ```lua
//! local wezzy = require("wezzy")
//! wezzy.bind("cmd+b", wezzy.action.ToggleSidebar)
//! ```

use config::keyassignment::KeyAssignment;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppCommand {
    ToggleSidebar,
    CommandPalette,
    FindAgent,
    FindTab,
    SwitchWorkspace,
    NewWorkspace,
    NewTerminal,
    NewAgent,
    NewAgentTab,
    InstallAgentHooks,
    UninstallAgentHooks,
    RenameAgent,
    ToggleGitPanel,
    SendOutputToAgent,
    SelectError,
    OpenEditor,
    OpenSettings,
    RunRecipe,
    SshHost,
    Restart,
    RebuildRestart,
    CheckForUpdates,
    ShowShortcuts,
    RenameTab,
    RenameWorkspace,
    MoveTabToWorkspace,
    MovePaneToNewTab,
    ClosePane,
    BalancePanes,
    CreatePullRequest,
    AttentionQueue,
    ReviewAgentRun,
    UndoAgentTurn,
    UndoAgentRun,
    DismissAgentRun,
}

impl AppCommand {
    pub const ALL: &[AppCommand] = &[
        Self::ToggleSidebar,
        Self::CommandPalette,
        Self::FindAgent,
        Self::FindTab,
        Self::SwitchWorkspace,
        Self::NewWorkspace,
        Self::NewTerminal,
        Self::NewAgent,
        Self::NewAgentTab,
        Self::InstallAgentHooks,
        Self::UninstallAgentHooks,
        Self::RenameAgent,
        Self::ToggleGitPanel,
        Self::SendOutputToAgent,
        Self::SelectError,
        Self::OpenEditor,
        Self::OpenSettings,
        Self::RunRecipe,
        Self::SshHost,
        Self::Restart,
        Self::RebuildRestart,
        Self::CheckForUpdates,
        Self::ShowShortcuts,
        Self::RenameTab,
        Self::RenameWorkspace,
        Self::MoveTabToWorkspace,
        Self::MovePaneToNewTab,
        Self::ClosePane,
        Self::BalancePanes,
        Self::CreatePullRequest,
        Self::AttentionQueue,
        Self::ReviewAgentRun,
        Self::UndoAgentTurn,
        Self::UndoAgentRun,
        Self::DismissAgentRun,
    ];

    /// Stable id, used in the event name.
    pub fn id(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "toggle-sidebar",
            Self::CommandPalette => "command-palette",
            Self::FindAgent => "find-agent",
            Self::FindTab => "find-tab",
            Self::SwitchWorkspace => "switch-workspace",
            Self::NewWorkspace => "new-workspace",
            Self::NewTerminal => "new-terminal",
            Self::NewAgent => "new-agent",
            Self::NewAgentTab => "new-agent-tab",
            Self::InstallAgentHooks => "install-agent-hooks",
            Self::UninstallAgentHooks => "uninstall-agent-hooks",
            Self::RenameAgent => "rename-agent",
            Self::ToggleGitPanel => "toggle-git-panel",
            Self::SendOutputToAgent => "send-output-to-agent",
            Self::SelectError => "select-error",
            Self::OpenEditor => "open-editor",
            Self::OpenSettings => "open-settings",
            Self::RunRecipe => "run-recipe",
            Self::SshHost => "ssh-host",
            Self::Restart => "restart",
            Self::RebuildRestart => "rebuild-restart",
            Self::CheckForUpdates => "check-for-updates",
            Self::ShowShortcuts => "show-shortcuts",
            Self::RenameTab => "rename-tab",
            Self::RenameWorkspace => "rename-workspace",
            Self::MoveTabToWorkspace => "move-tab-to-workspace",
            Self::MovePaneToNewTab => "move-pane-to-new-tab",
            Self::ClosePane => "close-pane",
            Self::BalancePanes => "balance-panes",
            Self::CreatePullRequest => "create-pull-request",
            Self::AttentionQueue => "attention-queue",
            Self::ReviewAgentRun => "review-agent-run",
            Self::UndoAgentTurn => "undo-agent-turn",
            Self::UndoAgentRun => "undo-agent-run",
            Self::DismissAgentRun => "dismiss-agent-run",
        }
    }

    /// Name under `require("wezzy").action`.
    pub fn lua_name(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "ToggleSidebar",
            Self::CommandPalette => "CommandPalette",
            Self::FindAgent => "FindAgent",
            Self::FindTab => "FindTab",
            Self::SwitchWorkspace => "SwitchWorkspace",
            Self::NewWorkspace => "NewWorkspace",
            Self::NewTerminal => "NewTerminal",
            Self::NewAgent => "NewAgent",
            Self::NewAgentTab => "NewAgentTab",
            Self::InstallAgentHooks => "InstallAgentHooks",
            Self::UninstallAgentHooks => "UninstallAgentHooks",
            Self::RenameAgent => "RenameAgent",
            Self::ToggleGitPanel => "ToggleGitPanel",
            Self::SendOutputToAgent => "SendOutputToAgent",
            Self::SelectError => "SelectError",
            Self::OpenEditor => "OpenEditor",
            Self::OpenSettings => "OpenSettings",
            Self::RunRecipe => "RunRecipe",
            Self::SshHost => "SshHost",
            Self::Restart => "Restart",
            Self::RebuildRestart => "RebuildRestart",
            Self::CheckForUpdates => "CheckForUpdates",
            Self::ShowShortcuts => "ShowShortcuts",
            Self::RenameTab => "RenameTab",
            Self::RenameWorkspace => "RenameWorkspace",
            Self::MoveTabToWorkspace => "MoveTabToWorkspace",
            Self::MovePaneToNewTab => "MovePaneToNewTab",
            Self::ClosePane => "ClosePane",
            Self::BalancePanes => "BalancePanes",
            Self::CreatePullRequest => "CreatePullRequest",
            Self::AttentionQueue => "AttentionQueue",
            Self::ReviewAgentRun => "ReviewAgentRun",
            Self::UndoAgentTurn => "UndoAgentTurn",
            Self::UndoAgentRun => "UndoAgentRun",
            Self::DismissAgentRun => "DismissAgentRun",
        }
    }

    /// How the shortcut sheet names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "Toggle sidebar",
            Self::CommandPalette => "Command palette",
            Self::FindAgent => "Find agent",
            Self::FindTab => "Find tab",
            Self::SwitchWorkspace => "Switch workspace",
            Self::NewWorkspace => "New workspace",
            Self::NewTerminal => "New terminal",
            Self::NewAgent => "New agent",
            Self::NewAgentTab => "New agent in a tab",
            Self::InstallAgentHooks => "Install agent hooks",
            Self::UninstallAgentHooks => "Uninstall agent hooks",
            Self::RenameAgent => "Rename agent",
            Self::ToggleGitPanel => "Git panel",
            Self::SendOutputToAgent => "Send output to agent",
            Self::OpenEditor => "Open editor",
            Self::OpenSettings => "Settings",
            Self::RunRecipe => "Run recipe",
            Self::SshHost => "SSH",
            Self::Restart => "Restart",
            Self::RebuildRestart => "Rebuild & restart",
            Self::CheckForUpdates => "Check for updates",
            Self::ShowShortcuts => "Keyboard shortcuts",
            Self::SelectError => "Select an error",
            Self::RenameTab => "Rename tab",
            Self::RenameWorkspace => "Rename workspace",
            Self::MoveTabToWorkspace => "Move tab to workspace",
            Self::MovePaneToNewTab => "Move pane to new tab",
            Self::ClosePane => "Close a pane",
            Self::BalancePanes => "Balance panes",
            Self::CreatePullRequest => "Create pull request",
            Self::AttentionQueue => "Agents",
            Self::ReviewAgentRun => "Review agent changes",
            Self::UndoAgentTurn => "Undo agent turn",
            Self::UndoAgentRun => "Undo agent run",
            Self::DismissAgentRun => "Dismiss agent run",
        }
    }

    pub fn event_name(self) -> String {
        format!("wezzy.{}", self.id())
    }

    pub fn from_event(name: &str) -> Option<Self> {
        let id = name.strip_prefix("wezzy.")?;
        // pre-rename configs
        let id = if id == "toggle-diff-panel" { Self::ToggleGitPanel.id() } else { id };
        Self::ALL.iter().copied().find(|c| c.id() == id)
    }

    pub fn action(self) -> KeyAssignment {
        KeyAssignment::EmitEvent(self.event_name())
    }
}

/// The `wezzy` Lua module (`src/lua/wezzy.lua`): `setup`, `bind`, `action`.
pub const LUA_MODULE: &str = include_str!("lua/wezzy.lua");

/// Lua name → event name for every app command, as the module's `actions`.
fn lua_actions() -> Vec<(&'static str, String)> {
    let mut v: Vec<_> = AppCommand::ALL.iter().map(|c| (c.lua_name(), c.event_name())).collect();
    // pre-rename name for the git panel
    v.push(("ToggleDiffPanel", AppCommand::ToggleGitPanel.event_name()));
    v
}

/// The module's `actions` as a Lua table literal, for the session server's
/// entry file (`session::server_config`).
pub fn lua_actions_literal() -> String {
    let items: Vec<String> =
        lua_actions().into_iter().map(|(name, event)| format!("{name} = {event:?}")).collect();
    format!("{{ {} }}", items.join(", "))
}

/// LuaLS types for the module (`src/lua/types.lua`), with a field for every
/// app command.
const LUA_TYPES: &str = include_str!("lua/types.lua");

pub fn lua_types() -> String {
    let fields: String = AppCommand::ALL
        .iter()
        .map(|c| format!("---{}\n---@field {} wezzy.Action\n", c.label(), c.lua_name()))
        .collect();
    LUA_TYPES.replace("--@ACTIONS@\n", &fields)
}

/// Where `lua_types` goes, for `workspace.library`.
pub fn lua_types_dir() -> std::path::PathBuf {
    crate::paths::data_dir().join("types")
}

/// Writes the types when they changed (a new build's commands).
pub fn write_lua_types() {
    let file = lua_types_dir().join("wezzy.lua");
    let text = lua_types();
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_str()) {
        let written = std::fs::create_dir_all(lua_types_dir()).and_then(|_| std::fs::write(&file, text));
        if let Err(err) = written {
            log::warn!("writing {}: {err}", file.display());
        }
    }
}

/// Exposes `require("wezzy")` to the config, with the app's settings hook.
pub fn register_lua() {
    fn setup(lua: &mlua::Lua) -> anyhow::Result<()> {
        let ctx = lua.create_table()?;
        let actions = lua.create_table()?;
        for (name, event) in lua_actions() {
            actions.set(name, event)?;
        }
        ctx.set("actions", actions)?;
        ctx.set("settings", lua.create_function(crate::settings::lua_setup)?)?;
        let module: mlua::Table = lua.load(LUA_MODULE).set_name("=wezzy").call(ctx)?;
        let package: mlua::Table = lua.globals().get("package")?;
        let loaded: mlua::Table = package.get("loaded")?;
        loaded.set("wezzy", module.clone())?;
        // pre-rename configs say require("dekita")
        loaded.set("dekita", module)?;
        Ok(())
    }
    config::lua::add_context_setup_func(setup);
}

#[cfg(test)]
mod lua_types_tests {
    use super::*;

    /// Every option `setup` accepts has a field in `wezzy.Setup`.
    #[test]
    fn types_cover_setup_options() {
        let types = lua_types();
        let setup = types.split("---@class wezzy.Setup").nth(1).unwrap();
        let setup = setup.split("---@class").next().unwrap();
        let mut names = vec![];
        for list in ["local APP = set({", "local OPTIONS = set({"] {
            let rest = LUA_MODULE.split(list).nth(1).unwrap();
            let body = rest.split("})").next().unwrap();
            names.extend(body.split(',').map(|n| n.trim().trim_matches('"').to_string()).filter(|n| !n.is_empty()));
        }
        assert!(names.len() > 15);
        for name in names {
            assert!(setup.contains(&format!("---@field {name}? ")), "wezzy.Setup has no field {name}");
        }
    }

    #[test]
    fn types_list_every_action() {
        let types = lua_types();
        assert!(!types.contains("@ACTIONS@"));
        for c in AppCommand::ALL {
            assert!(types.contains(&format!("---@field {} wezzy.Action\n", c.lua_name())));
        }
    }
}
