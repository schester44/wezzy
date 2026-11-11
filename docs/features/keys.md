# Keys

Wezzy comes with a full keymap: ⌘ shortcuts for the app, and a leader key for
splitting, moving and renaming, tmux-style. Press **Leader ?** at any time to
see every key, including your own.

The leader is **⌃A**. Press it, let go, then press the next key within three
seconds.

## Panes and tabs

| | |
|---|---|
| ⌘T | New tab |
| ⌘W | Close pane |
| Leader s / Leader v | Split down / right |
| Leader ⇧S / Leader ⇧V | Split up / left |
| Leader ↵ or ⌘↵ | Zoom the pane |
| Leader r | Resize mode: `hjkl` resize, esc to finish |
| Leader m | Swap the pane with another |
| Leader [ / Leader ] | Previous / next tab |
| ⌘1–9 | Go to tab |
| ⌃⌘1–8 | Move the tab to that position |
| Leader , | Rename the tab |
| Leader q | Close tab |
| ⌃h/j/k/l | Move between panes and into the side panels |

Closing a tab with more than one pane always asks first. Otherwise, Wezzy asks
when something other than a shell is still running, as set in Settings (⌘,).

⌘Q asks before quitting; press ↵, `y` or ⌘Q again to quit. **Quit** in ⌘K
and the Wezzy menu ask too. Your shells and agents keep running after you quit.

## Workspaces

| | |
|---|---|
| Leader k or ⌘⇧K | Switch workspace |
| Leader n | New workspace |
| Leader $ | Rename the workspace |

More in [Workspaces](workspaces.md).

## Copy mode

**Leader y** gives you a cursor to move through the screen and the scrollback,
select, and copy. The view holds still while you're in it, even if the program
keeps printing, and your keys don't reach the program.

| | |
|---|---|
| h j k l, arrows | Move |
| w b e | Next word, previous word, end of word |
| 0 ^ $ | Start of line, first character, end of line |
| gg G | Top and bottom of the scrollback |
| H M L | Top, middle and bottom of the screen |
| ⌃U ⌃D / ⌃B ⌃F | Half a page / a full page up and down |
| v / V | Select characters / whole lines |
| / ? | Search down / up; ↵ to jump |
| n N | Next / previous match |
| y, ↵ or ⌘C | Copy and leave |
| esc | Clear the selection, or leave |
| q | Leave |

With nothing selected, y copies the line the cursor is on. Search ignores case
unless you type a capital letter. Wrapped lines copy as one line.

## Quick select

**Leader f** puts a label on every URL, path, git hash, IP and long number
on screen. Type a label to copy that text, or type it in capitals to copy and
paste it into the pane. esc cancels.

## Swapping panes

**Leader m** puts a letter on every other pane in the tab. Type one and that
pane trades places with yours, and you stay in your pane. esc cancels. A zoomed
pane is unzoomed so you can see the others.

WezTerm's `PaneSelect` action does the rest of the job, in any mode:

```lua
{ key = "p", mods = "LEADER", action = wezterm.action.PaneSelect({ mode = "Activate" }) },
```

| `mode` | Picking a pane |
|---|---|
| `Activate` | Focuses it |
| `SwapWithActive` | Swaps it with yours, and you move to it |
| `SwapWithActiveKeepFocus` | Swaps it with yours, and you stay in your pane (Leader m) |
| `MoveToNewTab` | Moves it into a tab of its own |
| `MoveToNewWindow` | Moves it into a window of its own in this workspace |

`alphabet` sets the letters (it defaults to `quick_select_alphabet`) and
`show_pane_ids = true` adds each pane's `$WEZTERM_PANE` to its label.

## Typing ⌃A

⌃A also moves to the start of the line in most shells. Press it twice
(⌃A ⌃A) to send it to the shell.

## Changing keys

Your bindings in `~/.config/wezzy/wezzy.lua` win over Wezzy's, one key at a
time. Rebinding Leader s leaves every other key alone:

```lua
config.keys = {
  { key = "s", mods = "LEADER", action = wezterm.action.SplitPane({ direction = "Right" }) },
}
```

Set your own leader the WezTerm way:

```lua
config.leader = { key = "b", mods = "CTRL", timeout_milliseconds = 2000 }
```

Wezzy's app actions bind the same way, through the `wezzy` module:

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  table.insert(config.keys, { key = "j", mods = "CMD", action = wezzy.action.FindAgent })
end
```

`config.disable_default_key_bindings = true` turns off every built-in key,
leader included, so only your own bindings are left.
