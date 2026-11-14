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
| ⌥⌘1–9 | Jump to agent 1–9 (numbers show next to agents while you hold ⌥⌘) |

With pane cards on, the sidebar lights up like the active pane while it has
focus, and the panes all look inactive.

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

Claude Code, Codex, pi and OpenCode report their status exactly once their
hooks are installed (see below), along with what they're doing right now,
such as "Editing app.rs" or the command they're running, and what they're
asking you. Other agents (Aider, Gemini, Amp, Goose, Cursor) and agents without hooks
get a best guess from their output: they show as working while output is
coming in, and as waiting on you when they ring the bell.

Claude's subagents show under the agent that started them, one line each:
what it was asked to do and what it's doing now. A line goes away when that
subagent finishes. Subagents running in the background stay listed after
Claude finishes its turn, until they're done. When a subagent asks for
permission, the agent it belongs to shows the warning icon.

When an agent finishes or asks for you while you're in another pane, you hear
the chime and a toast names it and its project. Below the name, the toast
shows what the agent needs (the permission or question) or, when it's done,
the first sentence of its last message, plus the lines and files it changed
if it edited any. Click the toast to go to that agent. While Wezzy is in the
background the toast stays up, and it goes away a few seconds after you come
back. The agent's dot
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
wezzy.bind("cmd+alt+r", wezzy.action.RenameAgent)
```

To work through every agent that's waiting on you in one place, and see what
each one has done, press ⌘J for [Agents](agents.md).

### When Wezzy is in the background

If you're in another app, you also get a macOS notification. Its title is the
agent and project, such as **Claude · wezzy**, or the agent's name if you gave
it one (**auth refactor · wezzy**). When the agent asks for you, it says what
it wants, such as Claude's permission prompt; when it finishes, it shows the
first sentence of the agent's last message, or its task if that isn't
available. Click it to bring Wezzy to the front on that agent's
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
wezzy.setup({
  sidebar = { "agents", "actions", "servers" },
})
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

### Agent hooks

In Settings (⌘,), **Agent hooks** lists the agents on your PATH that don't
have hooks yet ("Install Codex, Pi"). Press ↵ to install them all; the row
then lists the agents that have them. Press ↵ again to remove them. In ⌘K,
the same things are **Install Agent Hooks** and **Uninstall Agent Hooks**.

| Agent | What gets installed |
|---|---|
| Claude Code | a hook for each event in `~/.claude/settings.json` |
| Codex | a hook for each event in `~/.codex/hooks.json` (`$CODEX_HOME` if set) |
| pi | the extension `~/.pi/agent/extensions/wezzy.ts` |
| OpenCode | the plugin `~/.config/opencode/plugins/wezzy.js` |

Installing adds to what's there and leaves your own hooks alone. Before it
first changes one of your files, it saves a copy next to it ending in
`.wezzy-backup`. Installing twice changes nothing.

Codex runs a new hook only after you trust it: when Codex says hooks need
review, open `/hooks` in Codex and trust Wezzy's. Restart agents that were
already running so they load their hooks.

## Good to know

- Subagents only show for Claude, and only with Wezzy's agent hooks installed
  (**Install Agent Hooks** in ⌘K).
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
- Hooks only report from Wezzy panes. The same agents in another terminal
  aren't affected.
- pi reports only from its interactive session, so runs it starts in the
  background, such as subagents, don't change its status. An OpenCode agent
  counts as working until it and all its subagents are done.
- pi shows as waiting on you while an extension has a question or dialog
  open for you mid-turn. pi has no permission prompts of its own.
- Whatever your config puts in the left status (`window:set_left_status`)
  shows at the bottom of the sidebar.
