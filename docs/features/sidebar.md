# Sidebar

The sidebar shows what's running right now: every agent and what it's
doing, the dev servers you started in this workspace, and, if you turn it
on, the GitHub Actions runs for the branch you're on. Each list only appears
when it has something in it, and you choose which lists show and in what
order.

## Using it

| | |
|---|---|
| ⌘B | Show or hide the sidebar |
| Ctrl+h at the terminal's left edge | Move into the sidebar |
| `j` / `k`, `g` / `G` | Move down / up, to the top / bottom |
| ↵ on an agent or server | Go to its pane |
| ↵ on an Actions run | Open it in a split: follow it live while it runs, the failed steps' log when it failed, the summary otherwise |
| `o` on a server or run | Open it in your browser |
| esc, Ctrl+l | Back to the terminal |
| Leader 1–9 | Jump to agent 1–9 (numbers show next to agents while the leader is armed) |

Click a row to do what ↵ does. Hover a server or run and click the arrow to
open it in your browser.

Click a list's name, such as **Servers**, to fold it down to its name and
count; click it again to open it. Folded lists stay folded after you restart,
and `j`/`k` skip their rows.

## What it shows

| List | Shows |
|---|---|
| Agents | Agents in every workspace, with their status and the workspace they're in |
| Servers | Programs listening on a port that you started from a pane in this workspace |
| Actions | The latest run of each GitHub Actions workflow on the active pane's branch, with how long ago it started. Off unless you turn it on |

### Agent status

| | |
|---|---|
| Spinner | Working |
| Warning icon | Waiting on you: a permission prompt or a question |
| Green dot | Finished while you were somewhere else, and you haven't been back to it |
| Gray dot | Finished, and you've seen it; or idle |

Claude's subagents show under the agent that started them, one line each:
what it was asked to do and what it's doing now. A line goes away when that
subagent finishes. Subagents running in the background stay listed after
Claude finishes its turn, until they're done. When a subagent asks for
permission, the agent it belongs to shows the warning icon.

When an agent finishes or asks for you while you're in another pane, you hear
the chime and a toast names it. Click the toast to go to that agent. Its dot
stays green until you go to its pane, so you can see at a glance which ones
finished since you last looked.

## Setting it up

Agents and servers show unless you pick your own lists. To turn on Actions,
set **GitHub Actions** to **Shown** in Settings (⌘,). It goes at the bottom.

To choose every list and its order, name them top to bottom in
`~/.config/wezzy/wezzy.lua`:

```lua
local ok, wezzy = pcall(require, "wezzy")
if ok then
  wezzy.setup({
    sidebar = { "agents", "actions", "servers" },
  })
end
```

or in `~/.config/wezzy/settings.json`:

```json
"sidebar": ["agents", "actions"]
```

If both set it, `wezzy.lua` wins. Leave a list out to hide it. With servers
or Actions left out, Wezzy also stops checking for them.

Actions uses the [GitHub CLI](https://cli.github.com), so install it and log in
once with `gh auth login`.

## Good to know

- Subagents only show for Claude, and only with Wezzy's agent hooks installed
  (**Install Claude Code Hooks** in ⌘K).

- Servers on ports below 1024 aren't listed, and neither are the ports agents
  open for themselves.
- Servers running on another machine over SSH aren't listed.
- Actions shows nothing when the active pane isn't in a GitHub repo, or `gh`
  isn't installed or logged in. It follows the pane you're in: switch to a
  pane in another repo or branch and the list changes with it. With a commit
  checked out instead of a branch, it shows that commit's runs.
- Actions checks every 10 seconds while a run is queued or running, every
  minute otherwise, and only while the sidebar is open.
- GitHub doesn't share a job's log until the job finishes, so a run that's
  still going opens as a live list of jobs and steps.
- Whatever your config puts in the left status (`window:set_left_status`)
  shows at the bottom of the sidebar.
