//! App-level commands: things WezTerm's `KeyAssignment` has no variant for
//! (sidebar, palette, and later browser/markdown/visualizer surfaces).
//!
//! They ride on `EmitEvent("wezzy.<id>")`, so they bind exactly like any
//! WezTerm action, including inside leader and key tables:
//!
//! ```lua
//! local wezzy = require("wezzy")
//! table.insert(config.keys, { key = "b", mods = "CMD", action = wezzy.action.ToggleSidebar })
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
    ToggleDiffPanel,
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
    BalancePanes,
    CreatePullRequest,
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
        Self::ToggleDiffPanel,
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
        Self::BalancePanes,
        Self::CreatePullRequest,
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
            Self::ToggleDiffPanel => "toggle-diff-panel",
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
            Self::BalancePanes => "balance-panes",
            Self::CreatePullRequest => "create-pull-request",
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
            Self::ToggleDiffPanel => "ToggleDiffPanel",
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
            Self::BalancePanes => "BalancePanes",
            Self::CreatePullRequest => "CreatePullRequest",
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
            Self::ToggleDiffPanel => "Git panel",
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
            Self::BalancePanes => "Balance panes",
            Self::CreatePullRequest => "Create pull request",
        }
    }

    pub fn event_name(self) -> String {
        format!("wezzy.{}", self.id())
    }

    pub fn from_event(name: &str) -> Option<Self> {
        let id = name.strip_prefix("wezzy.")?;
        Self::ALL.iter().copied().find(|c| c.id() == id)
    }

    pub fn action(self) -> KeyAssignment {
        KeyAssignment::EmitEvent(self.event_name())
    }
}

/// Exposes `require("wezzy")` to the config: `wezzy.action.<Name>`.
pub fn register_lua() {
    fn setup(lua: &mlua::Lua) -> anyhow::Result<()> {
        let module = lua.create_table()?;
        let actions = lua.create_table()?;
        for c in AppCommand::ALL {
            let t = lua.create_table()?;
            t.set("EmitEvent", c.event_name())?;
            actions.set(c.lua_name(), t)?;
        }
        module.set("action", actions)?;
        module.set("setup", lua.create_function(crate::settings::lua_setup)?)?;
        let package: mlua::Table = lua.globals().get("package")?;
        let loaded: mlua::Table = package.get("loaded")?;
        loaded.set("wezzy", module.clone())?;
        // pre-rename configs say require("dekita")
        loaded.set("dekita", module)?;
        Ok(())
    }
    config::lua::add_context_setup_func(setup);
}
