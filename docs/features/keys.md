# Keys

Wezzy comes with a full keymap: ⌘ shortcuts for the app, and a leader key for
splitting, moving and renaming, tmux-style. Press **Leader ?** at any time to
see every key, including your own. Start typing to narrow the sheet to what
you're looking for: `split`, `tab` or a key like `⌘k`. esc clears what you
typed, then closes. The git panel's sheet (**?**) and the SQL view's sheet
work the same way.

In Settings (⌘,), press **/** to search the settings by name or value. ↑ and ↓
move through what's left, and ↵ changes the highlighted setting. esc clears the
search.

The leader is **⌃A**. Press it, let go, then press the next key within three
seconds.

## Panes and tabs

| | |
|---|---|
| ⌘T | New tab |
| ⌘N or ⌘\\ | New terminal: a split in the current tab, placed like a new agent |
| ⌘I / ⌘⇧I | New agent in a split / a new tab |
| Leader w | New window |
| ⌘W | Close pane |
| Leader x | Close another pane without leaving yours |
| Leader s / Leader v | Split down / right |
| Leader ⇧S / Leader ⇧V | Split up / left |
| Leader ↵ or ⌘↵ | Zoom the pane |
| Leader r | Resize mode: `hjkl` resize, `=` makes the panes in the tab equal size and leaves resize mode, esc to finish |
| Leader m | Swap the pane with another |
| Leader [ / Leader ] | Previous / next tab |
| ⌘1–9 | Go to tab |
| ⌃⌘1–8 | Move the tab to that position |
| Leader 1–9 | Go to window |
| ⌥⌘1–9 | Go to agent; numbers show in the sidebar while you hold ⌥⌘ |
| Leader , | Rename the tab |
| Leader q | Close tab |
| ⌃h/j/k/l | Move between panes and into the side panels |

Drag the line between two panes to resize them. Click the line (without
dragging) to open a terminal, your default agent or the editor right there,
between the two panes. Esc or a click elsewhere puts the menu away.

Making panes equal size counts the panes along each side: with one pane on the
left and two stacked on the right, the left and right halves are the same
width and the two on the right are the same height. **Balance Panes** in ⌘K
does the same, and `wezzy.action.BalancePanes` binds it to any key.

Each pane sits on a rounded card, a shade lighter than the sidebar and top bar,
with a small gap between panes. The gap is the divider: drag it to resize the
panes. The active pane's outline is brighter. For flat panes split by a thin
line, turn **Pane cards** off in Settings (⌘,), or in `~/.config/wezzy/wezzy.lua`:

```lua
wezzy.setup({
  pane_cards = false,
})
```

or in `~/.config/wezzy/settings.json`:

```json
"pane_cards": false
```

The gap between cards is never wider than one column of text, so a pane's text
sits close to the side of its card when the pane is next to another one.

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

## Selecting with the mouse

Drag over a pane to select text, then press **⌘C** to copy it.

| | |
|---|---|
| Drag | Select characters |
| Double-click | Select a word; drag to add words |
| Triple-click | Select a line; drag to add lines |
| ⇧-click | Extend the selection to the mouse |
| ⌘C | Copy the selection |

Dragging past the top or bottom of the pane scrolls it. A selection stays inside
the pane it started in. Typing into the pane or clicking without dragging clears
it. Words end at spaces, brackets and quotes, so a path or URL selects whole.
⌘-click still opens the link under the mouse.

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

**Leader l** puts a label on every URL, path, git hash, IP and long number
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

## Closing another pane

**Leader x** puts a red × label on every pane in the tab. Type one and that
pane closes, and you stay in your pane. esc cancels. If the pane is running
something, you're asked first, the same as with ⌘W. It's also in the command
palette as **Close a Pane…**. To bind it to another key in your config, use
`require("wezzy").action.ClosePane`.

## Typing ⌃A

⌃A also moves to the start of the line in most shells. Press it twice
(⌃A ⌃A) to send it to the shell.

## Changing keys

Your bindings in `~/.config/wezzy/wezzy.lua` win over Wezzy's, one key at a
time. Rebinding Leader s leaves every other key alone:

```lua
wezzy.setup({
  keys = {
    { "leader+s", wezzy.action.SplitPane({ direction = "Right" }) },
    { "cmd+j", wezzy.action.FindAgent },
  },
})
```

or one at a time, anywhere in the file:

```lua
wezzy.bind("cmd+j", wezzy.action.FindAgent)
```

A key is its modifiers and the key joined with `+`: `cmd`, `ctrl`, `alt` (or
`opt`), `shift` and `leader`, then a letter, `enter`, `tab`, `space`, `esc`,
`left`/`right`/`up`/`down`, `f1`–`f12` and so on. `wezzy.action` has Wezzy's
app actions ([the list](config.md#actions)) and every WezTerm action. Give a function instead of an action to
run your own Lua; see [Configuration](config.md#keys).

Pick your own leader:

```lua
wezzy.setup({
  leader = { "ctrl+b", timeout = 2000 },
})
```

`wezterm = { disable_default_key_bindings = true }` in `wezzy.setup` turns off
every built-in key, leader included, so only your own bindings are left.
