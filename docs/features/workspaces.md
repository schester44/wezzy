# Workspaces

Keep each project in its own workspace. A workspace holds its own tabs, splits,
shells and agents. Switching swaps everything on screen, and the workspaces you
leave keep running, even after you quit Wezzy.

Save the projects you return to and they're one keystroke away, opening in the
right folder whether they're running or not.

## Switching

| | |
|---|---|
| ⌘⇧K | Pick a workspace |
| ⌘K | Search everything; workspaces come first when the name matches |
| Workspace name in the sidebar | Open the workspace menu (`j`/`k`, ↵, esc) |

To make a new workspace, type a name nobody's used in the picker and press ↵,
or choose **New workspace** in the sidebar menu. Leave the name blank and Wezzy
picks one for you.

When you reopen Wezzy, you're back in the workspace you were last in.

A workspace goes away when you close its last tab.

## Saved workspaces

Give a workspace a name and a folder, and it's always in the list, even when it
isn't open:

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

Put this in `~/.config/wezzy/wezzy.lua`. You can also list them in
`~/.config/wezzy/settings.json`:

```json
"workspaces": [{ "name": "neverwait", "cwd": "~/work/neverwait" }]
```

If both list the same name, `wezzy.lua` wins.

Saved workspaces appear after your open ones in the picker, in ⌘K and in the
sidebar menu. One that isn't open has a dimmed name and shows its folder
instead of a tab count.

Picking a saved workspace:

- **If it isn't open,** Wezzy opens it with a shell in its folder.
- **If it's already open,** Wezzy switches to it, just like any other workspace.

Only that first shell uses the saved folder. New tabs and splits start wherever
the current pane is.

## In your key bindings

Bind a key straight to a workspace, saved or not:

```lua
{ key = "1", mods = "LEADER", action = wezterm.action.SwitchToWorkspace({ name = "neverwait" }) },
```

If the action includes its own `spawn`, that runs instead and the saved folder
isn't used.

`wezterm.action.SwitchWorkspaceRelative(1)` and `(-1)` step through open
workspaces. They skip saved ones that aren't open.

## What's per workspace

These only see the workspace you're in:

- Sending output or hunk feedback to an agent
- The dev servers list in the sidebar

The agents sidebar and ⌘K show agents and tabs from every workspace, so you can
jump straight to one anywhere.
