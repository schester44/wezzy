# Configuration

Wezzy is configured from one Lua file, `~/.config/wezzy/wezzy.lua`: the
window, font and colors, your keys and plugins, and the app's own options like
saved workspaces and hosts. You describe it in one table passed to
`wezzy.setup`. Wezzy reloads the file when you save it.

Settings (⌘,) edits `~/.config/wezzy/settings.json`. When both set something,
`wezzy.lua` wins.

## Using it

```lua
local wezzy = require("wezzy")

local config = wezzy.setup({
  color_scheme = "Tokyo Night",
  font = { family = { "JetBrains Mono", "Menlo" }, size = 14 },
  window = { padding = 16, opacity = 0.9 },
  plugins = { "https://github.com/smart-splits-nvim/backend-wezterm" },
  keys = {
    { "cmd+shift+a", wezzy.action.FindAgent },
  },
  workspaces = {
    { name = "wezzy", cwd = "~/work/wezzy" },
  },
})

wezzy.bind("cmd+d", function(window, pane)
  window:perform_action(wezzy.action.SpawnCommandInNewTab({ args = { "lazygit" } }), pane)
end)

return config
```

The file ends with `return config`. `wezzy.setup` can be called more than
once, and `wezzy.bind` anywhere; they all add to the same config.

| Option | What it sets |
|---|---|
| `window` | `padding`: one number for every side, or `{ left, right, top, bottom }`. `opacity`: 0 to 1 |
| `font` | `family`: a name, or a list to fall back through. `size`, `line_height`, `cell_width`. A plain string is the family |
| `color_scheme` | Any [WezTerm color scheme](https://wezterm.org/colorschemes/index.html) by name |
| `cursor` | `"BlinkingBar"`, `"SteadyBlock"`, `"BlinkingUnderline"` and the rest of WezTerm's cursor styles |
| `leader` | The leader key, `"ctrl+a"` by default. `{ "ctrl+b", timeout = 2000 }` sets how long it waits, in milliseconds |
| `keys` | Your key bindings, below |
| `plugins` | WezTerm plugins, below |
| `wezterm` | Any other WezTerm option, below |

The app's options go in the same table:

| Option | |
|---|---|
| `workspaces` | Saved workspaces: [Workspaces](workspaces.md) |
| `hosts` | SSH hosts: [SSH](ssh.md) |
| `databases` | Databases: [SQL](sql.md) |
| `recipes`, `harnesses`, `default_harness`, `agent_layout` | What ⌘I launches and where: [Agents](agents.md) |
| `sidebar`, `notifications` | [Sidebar](sidebar.md) |
| `editor` | What opens files: [Git panel](git-panel.md) |
| `pane_cards`, `close_confirmation` | [Keys](keys.md) |
| `auto_update` | [Updates](updates.md) |

A misspelled option is an error that names it, instead of being ignored.

### Keys

A key is its modifiers and the key joined with `+`: `cmd`, `ctrl`, `alt` (or
`opt`), `shift` and `leader`, then the key: a letter or symbol, `enter`, `tab`,
`space`, `esc`, `backspace`, `delete`, `left`/`right`/`up`/`down`, `home`,
`end`, `pageup`, `pagedown`, `f1`–`f12`. `cmd++` is ⌘+.

```lua
wezzy.setup({
  keys = {
    { "cmd+b", wezzy.action.ToggleSidebar },
    { "leader+s", wezzy.action.SplitPane({ direction = "Right" }) },
    { "cmd+shift+g", function(window, pane) --[[ your Lua ]] end },
  },
})
```

`wezzy.action` has Wezzy's app actions, below, and every
[WezTerm action](https://wezterm.org/config/lua/keyassignment/index.html).
Each binding replaces Wezzy's built-in one for that key and leaves the rest
alone; Leader ? lists them all.

A function runs when you press the key, with the window and the pane you're
in. `window:perform_action(action, pane)` runs any action from there; leave
out `pane` for the active one. `wezzy.on(event, function(window, pane) … end)`
handles WezTerm events the same way.

### Actions

Wezzy's own actions, as `wezzy.action.<Name>`:

| Action | |
|---|---|
| `CommandPalette` | The command palette |
| `FindAgent`, `FindTab` | Jump to an agent or a tab |
| `AttentionQueue` | The agents overlay |
| `SwitchWorkspace`, `NewWorkspace`, `RenameWorkspace` | Workspaces |
| `MoveTabToWorkspace` | Move this tab to another workspace |
| `NewTerminal` | A shell in a new pane, placed like a new agent |
| `NewAgent`, `NewAgentTab` | Start the default harness in a new pane or tab |
| `RenameAgent` | Rename the agent in this pane |
| `RunRecipe` | Pick a recipe and run it |
| `SendOutputToAgent` | Send this pane's output to an agent |
| `SelectError` | Pick an error on screen |
| `OpenEditor` | Your editor in this pane's directory, in a new pane |
| `ToggleSidebar`, `ToggleGitPanel` | The sidebar, the git panel |
| `CreatePullRequest` | Open a pull request for this branch |
| `ReviewAgentRun`, `UndoAgentTurn`, `UndoAgentRun`, `DismissAgentRun` | The agent run in this pane: [Agent runs](agent-runs.md) |
| `RenameTab` | Rename this tab |
| `MovePaneToNewTab` | Move this pane to a tab of its own |
| `ClosePane` | Pick a pane to close |
| `BalancePanes` | Make this tab's panes the same size |
| `SshHost` | Pick an SSH host |
| `InstallAgentHooks`, `UninstallAgentHooks` | Agent hooks: [Agents](agents.md) |
| `OpenSettings`, `ShowShortcuts` | Settings, the shortcut sheet |
| `Restart`, `RebuildRestart`, `CheckForUpdates` | Restart Wezzy, rebuild it from source and restart, check for an update |

`SpawnCommandInNewPane` runs a command in a new pane of the current tab,
placed like a new terminal. It takes what WezTerm's `SpawnCommandInNewTab`
takes: `args`, `cwd`, `set_environment_variables`, `domain`. Without `args`
it opens your shell.

```lua
wezzy.bind("cmd+shift+t", wezzy.action.SpawnCommandInNewPane({ args = { "htop" } }))
wezzy.bind("cmd+shift+l", wezzy.action.SpawnCommandInNewPane({
  args = { "lazygit" },
  cwd = wezzy.wezterm.home_dir .. "/work/wezzy",
}))
```

### Plugins

Each entry is a WezTerm plugin's URL, `{ url, options }` to pass it options,
or a function that receives the config:

```lua
wezzy.setup({
  plugins = {
    "https://github.com/smart-splits-nvim/backend-wezterm",
    function(config) require("theme").apply(config) end,
  },
})
```

Your `keys` win over keys a plugin adds.

### Other WezTerm options

`wezterm` takes any [WezTerm option](https://wezterm.org/config/lua/config/index.html),
as a table or as a function that receives the config. It's applied after the
options above, so it has the last word:

```lua
wezzy.setup({
  wezterm = {
    enable_kitty_keyboard = true,
    scrollback_lines = 20000,
  },
})
```

Lua modules in `~/.config/wezterm/` can be required from `wezzy.lua`, so an
existing WezTerm setup can be reused: `wezterm = function(config) require("theme").apply(config) end`.

### Completion in your editor

Wezzy keeps type definitions for `require("wezzy")` in
`~/.local/share/wezzy/types/`, written each time it starts. Point
lua-language-server at them and your editor completes `setup` options,
`wezzy.action` names and their arguments, and flags misspelled ones. Put a
`.luarc.json` next to `wezzy.lua`:

```json
{
  "runtime.version": "Lua 5.4",
  "workspace.library": ["~/.local/share/wezzy/types"]
}
```

WezTerm's own actions and the `wezterm` module aren't in them.

## Details

- On first run Wezzy writes a starter `wezzy.lua`. `$WEZZY_CONFIG` points it
  at another file.
- Wezzy draws its own window, so WezTerm options for the native window, tab
  bar and GPU don't change anything.
