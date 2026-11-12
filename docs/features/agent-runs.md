# Agent runs

When an agent finishes and has changed files, a card shows up over the bottom
of its pane: the branch, what the agent says it did, and how much it changed.
From there you review the changes, send comments back, commit them, or undo
the agent's last turn.

A run is everything the agent changed since you last reviewed its work, across
as many prompts as that took. The sidebar shows its `+/−` on the agent's row
until you're done with it.

## Using it

The card lists what you can do:

| | |
|---|---|
| **Review changes** | Go to the agent's pane and open the git panel on just the files the agent changed, cursor on the first one |
| **Ask for changes** | The same, with the first file open at its first hunk: `f` to comment, `F` to send the comments to this agent |
| **Commit** | Stage the agent's files, open the git panel on them and the commit box with the agent's summary as the message |
| **Undo turn** | Put back the files the agent changed in its last turn, after you confirm |
| × | Done with the run: the card and the sidebar's `+/−` go away |

The card is never in the way of your keys: they keep going to the terminal.
Typing into the agent's pane puts the card away until the agent finishes its
next turn; the run is still there, and so is its `+/−` in the sidebar.

From ⌘K, for the agent in the pane you're in:

| | |
|---|---|
| **Review Agent Changes** | What the card's Review changes does |
| **Undo Agent Turn** | Undo the last turn, after you confirm |
| **Undo Agent Run** | Undo every turn of the run, after you confirm |
| **Dismiss Agent Run** | What the card's × does |

In the git panel:

| | |
|---|---|
| ⌘G | Opens on the run's files when the agent in your pane has finished a run with changes; on everything otherwise |
| A | Only the run's files, or everything again |
| Click the agent's name in the header | Everything again |

## Setting it up

Runs come from Wezzy's agent hooks, for Claude Code and Codex. Install them
from Settings (⌘,) → **Agent hooks**, or **Install Agent Hooks** in ⌘K, then
restart agents that were already running. The agent has to be working in a
git repo.

To put the commands on keys, bind them in `~/.config/wezzy/wezzy.lua`:

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  table.insert(config.keys, { key = "c", mods = "LEADER", action = wezzy.action.ReviewAgentRun })
  table.insert(config.keys, { key = "u", mods = "LEADER", action = wezzy.action.UndoAgentTurn })
end
```

`wezzy.action.UndoAgentRun` and `wezzy.action.DismissAgentRun` bind the same way.

## Details

- **What counts.** Only the files the agent edited with its file tools
  (Claude's Edit, MultiEdit, Write and NotebookEdit, Codex's patches), compared with how
  they were when the run started. Your own edits to other files don't show up
  in the run, and neither do files the agent changed by running commands in the
  shell (a formatter, `sed`, a code generator).
- **When the card shows.** Each time a turn ends while the run has changes,
  even a turn that only answered a question. If the agent's changes add up to
  nothing (it put a file back the way it was), there's no card.
- **Every pane.** Cards show over every pane in the tab, not just the one
  you're in, so you see which agent is done when you look over. A pane too
  small to hold one doesn't get a card.
- **When a run starts and ends.** Your first prompt after you finish with a run
  starts the next one. Committing while the git panel shows the run ends it
  when the commit has only the run's files in it and none of them are left
  unstaged. ×, **Dismiss Agent Run** and **Undo Agent Run** end it too. Prompting the agent from a different repo starts a new run.
- **Committing.** Commit stages the run's files and nothing else; anything you
  had staged already stays staged and goes into the commit too. The message
  starts as the agent's summary, which you can edit. Then `N` opens a pull
  request as usual.
- **Sending comments.** `F` in the git panel sends your comments to the agent
  whose run changed the commented files, without asking which agent. If the
  files aren't all from one run, you pick, as usual.
- **Undo.** Each turn remembers how the files looked when it started. Undo
  puts the agent's files back to that, and deletes the files it created. Any
  change to those files since, yours included, is lost. Other files aren't
  touched. A toast says how many files were restored. Undoing the run's only
  turn ends the run.
- **Restarting Wezzy.** Runs aren't kept when Wezzy quits, and what agents did
  while it wasn't running isn't counted.
