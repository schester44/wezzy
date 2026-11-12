# Attention queue

Every agent that's waiting on you, in one list. Press ⌘J from anywhere, work
through it with the keyboard, and you're back where you were: no hunting
through tabs and workspaces for the agent that asked.

Agents blocked on you come first (a permission prompt, a question, a plan to
review), then the ones that finished while you were elsewhere. Within each,
whoever has waited longest is on top.

## Using it

| | |
|---|---|
| ⌘J | Open the queue (again to close it) |
| `j` / `k`, ↓ / ↑ | Move through the list |
| `g` / `G` | First / last |
| `1`–`9` | Select that item |
| ↵ or `o` | Go to the agent's pane |
| `a` | Approve a permission prompt |
| `r` | Reject a permission prompt |
| `d` | Dismiss a finished agent without going to it |
| esc | Close |

**Attention Queue** in ⌘K opens it too. Clicking a row selects it; clicking it
again goes to the agent.

Each row shows what the agent wants, then its project and name. The selected
row shows the request in full (the whole command, the file it wants to edit
and the new text, the URL) with its buttons below.

Answering the last item closes the queue.

## Approving from the queue

Claude Code and Codex permission prompts can be answered without leaving the
queue. **Approve** picks the prompt's first option (yes, once) and **Reject**
declines it, the same as pressing ↵ or esc in the agent's pane.

Some requests ask twice: deleting files, force pushes and hard resets, database
migrations and drops, `sudo`, deploys and publishing, infrastructure tools
(`terraform`, `kubectl`, cloud CLIs), piping a download into a shell, and any
MCP tool. Their **Approve** button is red, and `a` shows the request in a
confirmation bar first; press ↵ to approve or any other key to back out.

Everything else (questions, plans, other agents' prompts, a "yes, and don't
ask again") is answered in the agent's pane: press ↵ to go there.

## Setting it up

The queue uses agent hooks. Install them from Settings (⌘,) → **Agent hooks**,
or with **Install Agent Hooks** in ⌘K; see [Sidebar](sidebar.md#agent-hooks).
Approving from the queue needs Claude Code's or Codex's hooks.

To move it to another key, bind `wezzy.action.AttentionQueue` in
`~/.config/wezzy/wezzy.lua`:

```lua
local ok, wezzy = pcall(require, "wezzy")
if ok then
  table.insert(config.keys, { key = "a", mods = "LEADER", action = wezzy.action.AttentionQueue })
end
```

## Details

- A finished agent stays in the queue, and in the Dock badge count, until you
  go to its pane or dismiss it. A blocked agent stays until it gets its answer.
- An agent without hooks shows up when it rings the bell, and can only be
  opened.
- After you approve or reject, the item leaves the queue right away. If the
  agent asks again, it comes back.
- When an agent's prompt comes from one of its subagents, approving answers
  that prompt; it's shown in the same pane.
- If the agent's prompt is no longer on screen (you answered it in the pane a
  moment ago), the key goes to the agent as ordinary input.
