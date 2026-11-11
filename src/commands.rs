//! App-level commands: things WezTerm's `KeyAssignment` has no variant for
//! (sidebar, palette, and later browser/markdown/visualizer surfaces).
//!
//! They ride on `EmitEvent("dekita.<id>")`, so they bind exactly like any
//! WezTerm action, including inside leader and key tables:
//!
//! ```lua
//! local dekita = require("dekita")
//! table.insert(config.keys, { key = "b", mods = "CMD", action = dekita.action.ToggleSidebar })
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
    NewAgent,
    NewAgentTab,
    InstallAgentHooks,
    ToggleDiffPanel,
    SendOutputToAgent,
    OpenEditor,
    OpenSettings,
    RunRecipe,
    SshHost,
    Restart,
    RebuildRestart,
}

impl AppCommand {
    pub const ALL: &[AppCommand] = &[
        Self::ToggleSidebar,
        Self::CommandPalette,
        Self::FindAgent,
        Self::FindTab,
        Self::SwitchWorkspace,
        Self::NewWorkspace,
        Self::NewAgent,
        Self::NewAgentTab,
        Self::InstallAgentHooks,
        Self::ToggleDiffPanel,
        Self::SendOutputToAgent,
        Self::OpenEditor,
        Self::OpenSettings,
        Self::RunRecipe,
        Self::SshHost,
        Self::Restart,
        Self::RebuildRestart,
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
            Self::NewAgent => "new-agent",
            Self::NewAgentTab => "new-agent-tab",
            Self::InstallAgentHooks => "install-agent-hooks",
            Self::ToggleDiffPanel => "toggle-diff-panel",
            Self::SendOutputToAgent => "send-output-to-agent",
            Self::OpenEditor => "open-editor",
            Self::OpenSettings => "open-settings",
            Self::RunRecipe => "run-recipe",
            Self::SshHost => "ssh-host",
            Self::Restart => "restart",
            Self::RebuildRestart => "rebuild-restart",
        }
    }

    /// Name under `require("dekita").action`.
    pub fn lua_name(self) -> &'static str {
        match self {
            Self::ToggleSidebar => "ToggleSidebar",
            Self::CommandPalette => "CommandPalette",
            Self::FindAgent => "FindAgent",
            Self::FindTab => "FindTab",
            Self::SwitchWorkspace => "SwitchWorkspace",
            Self::NewWorkspace => "NewWorkspace",
            Self::NewAgent => "NewAgent",
            Self::NewAgentTab => "NewAgentTab",
            Self::InstallAgentHooks => "InstallAgentHooks",
            Self::ToggleDiffPanel => "ToggleDiffPanel",
            Self::SendOutputToAgent => "SendOutputToAgent",
            Self::OpenEditor => "OpenEditor",
            Self::OpenSettings => "OpenSettings",
            Self::RunRecipe => "RunRecipe",
            Self::SshHost => "SshHost",
            Self::Restart => "Restart",
            Self::RebuildRestart => "RebuildRestart",
        }
    }

    pub fn event_name(self) -> String {
        format!("dekita.{}", self.id())
    }

    pub fn from_event(name: &str) -> Option<Self> {
        let id = name.strip_prefix("dekita.")?;
        Self::ALL.iter().copied().find(|c| c.id() == id)
    }

    pub fn action(self) -> KeyAssignment {
        KeyAssignment::EmitEvent(self.event_name())
    }
}

/// Exposes `require("dekita")` to the config: `dekita.action.<Name>`.
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
        loaded.set("dekita", module)?;
        Ok(())
    }
    config::lua::add_context_setup_func(setup);
}
