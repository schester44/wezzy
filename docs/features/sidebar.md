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
| `r` on an agent | Rename it |
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

When an agent finishes or asks for you while you're in another pane, you hear
the chime and a toast names it. Click the toast to go to that agent. Its dot
stays green until you go to its pane, so you can see at a glance which ones
finished since you last looked.

### Naming an agent

Press `r` on an agent, or run **Rename Agent…** from ⌘K, which renames the
agent selected in the sidebar or else the one in the pane you're in. Type a
name like "auth refactor" and press ↵. esc or a click elsewhere leaves the
name as it was. Clear the field and press ↵ to go back to the default.

The name takes the place of the agent's kind ("Claude") in the sidebar, in ⌘K,
in toasts and in notifications. The kind and the task it's on show on the line
below.

A name belongs to the pane, not the program: it stays while the pane is open,
including when you quit and reopen Wezzy, and goes with the pane when it
closes. Start another agent in the same pane and it keeps the name. If the
session server restarts (after a reboot, say), the names are gone.

Renaming opens the sidebar if it's hidden (closing the git panel if that's
open) and hides it again when you're done. To give **Rename Agent** a key,
bind `wezzy.action.RenameAgent`:

```lua
local ok, wezzy = pcall(require, "wezzy")
if ok then
  table.insert(config.keys, { key = "r", mods = "CMD|ALT", action = wezzy.action.RenameAgent })
end
```

### When Wezzy is in the background

If you're in another app, you also get a macOS notification. Its title is the
agent and project, such as **Claude · wezzy**, or the agent's name if you gave
it one (**auth refactor · wezzy**). When the agent asks for you, it says what
it wants, such as Claude's permission prompt; when it finishes, it shows the
agent's task. Click it to bring Wezzy to the front on that agent's
pane.

The Dock icon shows how many agents are waiting on you: ones that asked for
you or finished since you last went to them. Going to an agent's pane takes
it off the count and clears its notification from Notification Center.

macOS asks whether Wezzy may send notifications the first time one comes up.
To change your answer later, or how they appear, open System Settings →
Notifications → Wezzy.

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

To turn off notifications and the Dock badge, set **Notifications** to
**Off** in Settings (⌘,), or pass `notifications = false` to `wezzy.setup`, or
add `"notifications": false` to `settings.json`. The chime and toasts are
separate: **Sound** controls the chime.

Actions uses the [GitHub CLI](https://cli.github.com), so install it and log in
once with `gh auth login`.

## Good to know

- You get one notification each time an agent asks for you or finishes, and
  a newer one for the same agent replaces the older.
- Notifications only work in the installed Wezzy app. A build run from
  source (`cargo run`) shows the Dock badge, the toast and the chime, but no
  notifications.
- Notifications left from before you restart Wezzy are cleared when it opens.

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
