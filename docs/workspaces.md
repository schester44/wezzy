# Workspaces

A workspace is a named set of tabs and splits, usually one per project. Only
one is on screen at a time; switching swaps the whole set. They're WezTerm's
workspaces: the mux tracks them, and the session server keeps their shells
and agents running while you're in another workspace or after you quit the app.

## Switching

| | |
|---|---|
| ⌘⇧K | Workspace picker (the palette, scoped to workspaces) |
| ⌘K | The full palette; workspaces rank above tabs and panes with the same name |
| Sidebar header | Click the workspace name for the menu: `j/k`, ↵, esc |

Typing a name that doesn't match anything in the picker and pressing ↵
creates that workspace. "New workspace" in the menu (or the `NewWorkspace`
command) asks for a name; leave it blank to get a generated one.

Across restarts, Wezzy reopens the workspace you last used
(`~/.local/share/wezzy/last-workspace`). If that workspace is gone, it shows
whichever one still has tabs.

A workspace ends when its last tab closes.

## Saved workspaces

Saved workspaces are workspaces you can pick before they exist. Each one has
a name and a directory:

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  wezzy.setup({
    workspaces = {
      { name = "neverwait", cwd = "~/work/neverwait" },
      { name = "wezzy", cwd = "~/work/wezzy" },
    },
  })
end
```

or in `~/.config/wezzy/settings.json`:

```json
"workspaces": [{ "name": "neverwait", "cwd": "~/work/neverwait" }]
```

You can use both. Entries in the Lua config replace `settings.json` entries
with the same name. `cwd` accepts `~`.

In the picker, the palette and the sidebar menu, saved workspaces come after
the running ones. A saved workspace that isn't running has a dimmed name and
shows its directory where running workspaces show a tab count.

Opening a saved workspace:

- **Not running:** creates the workspace with one shell in `cwd`.
- **Running:** switches to it. The directory isn't used again.

The directory applies only to the first shell. Later tabs and splits start
where the active pane is, as in any other workspace.

Every way of switching follows this, including Lua:

```lua
wezterm.action.SwitchToWorkspace({ name = "neverwait" })
```

If that action passes its own `spawn` command, the command is used instead of
the saved directory.

Saved workspaces also apply at startup. When the session server has no panes
and the last-used workspace is a saved one, the first shell starts in its
directory.

## Scoped to the workspace

These features act on the current workspace only:

- Send output to an agent and send hunk feedback, which pick from the agents in this workspace
- The dev servers list in the sidebar
- `SwitchWorkspaceRelative`, which cycles through running workspaces only

The agents sidebar and the palette's agent and tab results cover every workspace.

## Code

| | |
|---|---|
| `src/workspaces.rs` | saved workspaces, the merged list shown in the picker and menu |
| `TermApp::switch_workspace` (`app.rs`) | switch, or create with the saved spawn |
| `settings.rs` | `workspaces` in `settings.json` and `setup()` |
| `session.rs` | last-used workspace, first shell on an empty server |
| `workspace_switcher` (`ui/view.rs`) | the sidebar menu |
