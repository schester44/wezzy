# Agent overview

See every agent at once, in every workspace, each as a live card showing the
bottom of its terminal. Approve a permission prompt, reply, or interrupt an
agent right from its card, then close the overview and you're back exactly
where you were.

## Using it

| | |
|---|---|
| ⌘O or ⌥⌘0 | Open or close the overview |
| ⌘K → **Agent Overview** | Same |
| esc, `q` | Close it |
| `h` / `j` / `k` / `l`, arrows | Move between cards |
| `g` / `G` | First / last card |
| `1`–`9` | Select card 1–9 |
| Tab / Shift+Tab | Next / previous agent that wants you |
| ↵ | Go to the selected agent's pane |
| ⌥⌘1–9 | Go to agent 1–9 |
| `y` / `Y` / `n` | Allow / always allow / deny the selected agent's permission prompt |
| `r` | Reply to the selected agent |
| ⌃C | Interrupt the selected agent |

With the mouse: click a card to select it, double-click it or click its name
to go to its pane, and click **Allow**, **Always** or **Deny** on a card
that's asking for permission. Click a count in the header to step through
the agents it counts.

Tab goes to agents waiting on you first. When none are, it goes to agents
that finished while you were elsewhere.

### Replying

`r` opens a field under the card. Type your reply and press ↵ to send it to
the agent as if you'd typed it there and pressed enter. Shift+↵ adds a line,
esc cancels, and ⌘V pastes.

## Setting it up

Nothing to set up, though answering prompts from a card needs Claude Code's
hooks (⌘, → Agent hooks). To open the overview with another key, bind
`wezzy.action.AgentOverview` in `~/.config/wezzy/wezzy.lua`:

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  table.insert(config.keys, { key = "o", mods = "CMD|SHIFT", action = wezzy.action.AgentOverview })
end
```

## What a card shows

- **First line:** the agent's status, its number (the same number ⌥⌘1–9
  uses), its name, the harness when you've named it, the branch it's on, and
  how long it's been in its current state.
- **Second line:** what it's doing or asking right now, such as "Editing
  app.rs" or Claude's permission message, when there's something to say. Up to
  two of Claude's running subagents show under it.
- **Its terminal:** the last rows of the agent's pane, at its real width,
  cropped at the card's edge. It updates live.

The header counts agents waiting on you (warning icon), working (spinner),
and finished while you were elsewhere (green dot), the same icons as the
sidebar.

## Details

- Cards are grouped by workspace, and within a workspace they're in the order
  the agents started. They never move around when an agent's status changes.
  The sidebar and ⌥⌘1–9 number agents the same way.
- `y`, `Y` and `n` work on Claude Code's permission prompts, and only when
  Claude's hooks are installed (⌘, → Agent hooks), since that's how Wezzy
  knows a prompt is up. Allow picks the highlighted "Yes", Always picks "Yes,
  and don't ask again", and Deny cancels the prompt. Questions and other
  agents' prompts need a reply (`r`) or a visit (↵). After you answer, the
  buttons hide while the agent carries on.
- ⌃C sends Esc to Claude Code, which stops its turn without quitting, and
  ⌃C to every other agent.
- Looking at a card doesn't count as seeing it: an agent that finished while
  you were away keeps its green dot until you go to its pane.
- Going to an agent any other way, such as clicking a toast or a
  notification, also closes the overview.
- ⌘ shortcuts keep working while the overview is open. Keys you type never
  reach a terminal, except a reply you send.
- Panes keep their size while they're shown on cards.
- With a single agent, its card fills the window. With more than 12, each
  card shows fewer terminal rows so more fit on screen, and the grid scrolls.
- With no agents, the overview offers **New agent**.
