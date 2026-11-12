# Agents

What needs you and what's been done, in one place. Press ⌘J from anywhere:
the agents waiting on you are on top, and below them each agent's work this
session, one short line per piece of work, so you can catch up without scrolling back
through its output or asking it for a recap.

## Using it

| | |
|---|---|
| ⌘J | Open Agents (again to close it) |
| `j` / `k`, ↓ / ↑ | Move through the list |
| `g` / `G` | First / last |
| `1`–`9` | Select that waiting item |
| ↵ or `o` | Go to the agent's pane |
| `a` | Approve a permission prompt |
| `r` | Reject a permission prompt |
| `d` | Dismiss a finished agent without going to it |
| `v` | Review a run's changes in the git panel |
| esc | Close |

While anything is waiting, the right end of the tab bar shows how many agents
are blocked on you (a warning icon) and how many finished (a check). Click it
to open Agents.

**Agents** in ⌘K opens it too. Clicking a row selects it; clicking it again
goes to the agent.

### Waiting

Agents blocked on you come first (a permission prompt, a question, a plan to
review), then the ones that finished while you were elsewhere. Within each,
whoever has waited longest is on top.

Each row shows what the agent wants, then its project and name. The selected
row shows the request in full (the whole command, the file it wants to edit
and the new text, the URL) with its buttons below.

### What each agent has done

Under each agent's name and project, a line for each piece of work, newest
first: the first sentence of the agent's last message, what it changed (`+/−`
and files) and how long ago it finished. While the agent is on it, the line
has a spinner and shows your prompt.

Your follow-up prompts stay on the same line. The next prompt starts a new
line when:

- you or the agent committed since the line started,
- the agent had been finished for 15 minutes or more,
- the agent moved to another repo, or
- you dismissed the agent's run (× on its bar, or ⌘.).

The selected line also shows the prompt that started it, and **Review
changes** (`v`) on the newest line when the agent's run has changes to review
(see [Agent runs](agent-runs.md)).

The agent in the pane you're in is listed first.

## Approving from Agents

Claude Code and Codex permission prompts can be answered without leaving
Agents. **Approve** picks the prompt's first option (yes, once) and **Reject**
declines it, the same as pressing ↵ or esc in the agent's pane.

Some requests ask twice: deleting files, force pushes and hard resets, database
migrations and drops, `sudo`, deploys and publishing, infrastructure tools
(`terraform`, `kubectl`, cloud CLIs), piping a download into a shell, and any
MCP tool. Their **Approve** button is red, and `a` shows the request in a
confirmation bar first; press ↵ to approve or any other key to back out.

Everything else (questions, plans, other agents' prompts, a "yes, and don't
ask again") is answered in the agent's pane: press ↵ to go there.

## Setting it up

Agents uses agent hooks. Install them from Settings (⌘,) → **Agent hooks**,
or with **Install Agent Hooks** in ⌘K; see [Sidebar](sidebar.md#agent-hooks).
Approving and the lines of what each agent has done need Claude Code's or
Codex's hooks.

To move it to another key, bind `wezzy.action.AttentionQueue` in
`~/.config/wezzy/wezzy.lua`:

```lua
local ok, wezzy = pcall(require, "wezzy")
if ok then
  table.insert(config.keys, { key = "a", mods = "LEADER", action = wezzy.action.AttentionQueue })
end
```

## Details

- A finished agent stays under Waiting, and in the Dock badge count, until you
  go to its pane or dismiss it. A blocked agent stays until it gets its answer.
- An agent without hooks shows up under Waiting when it rings the bell, and can
  only be opened.
- After you approve or reject, the item leaves Waiting right away. If the
  agent asks again, it comes back.
- When an agent's prompt comes from one of its subagents, approving answers
  that prompt; it's shown in the same pane.
- If the agent's prompt is no longer on screen (you answered it in the pane a
  moment ago), the key goes to the agent as ordinary input.
- **Which agents.** Only agents working in a git repo get lines, and only
  while they're running. The lines start over when the agent in a pane starts
  a new session (you restart it, or `/clear` in Claude Code), and are kept
  across Wezzy restarts.
- **A line's summary** is the agent's last message on it, so a line with
  several prompts shows how the last one ended.
- **What counts as changed** is the same as for runs: files the agent edited
  with its file tools, compared with how they were when the line started.
  Undoing a turn updates the line's `+/−`.
