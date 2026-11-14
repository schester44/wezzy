# Wezzy

A terminal for people running more coding agents than they can keep track of.

![Wezzy](docs/assets/screenshot.png)

One agent is easy. Nine is a job: which one is still working, which one has
been waiting on you for ten minutes, which one finished and what did it
actually change? Wezzy is a terminal built around that job. It tells you who
needs you, puts what they wrote in front of you to review, keeps a list of
everything they're waiting on you for, and keeps them all running when you
close the lid. Everything has a key, so you rarely leave the keyboard.

It's built on WezTerm's engine and reads your WezTerm config as is, so your
keys, leader, plugins and smart-splits come along for the ride. It's named after
WezTerm, out of love and a mild sense of debt.

macOS only for now. Download `Wezzy.dmg` from the
[latest release](https://github.com/schester44/wezzy/releases/latest); it
[updates itself](docs/features/updates.md).

## What it's like

**You always know who needs you.** Every agent in every workspace is in the
sidebar with what it's doing: working, waiting on you, done. A chime when one
asks for you, `⌥⌘1–9` to jump to it, and ⌘J to answer a permission prompt
without leaving what you were doing. No more cycling through tabs to find the
one that stalled twenty minutes ago. [Agents](docs/features/agents.md)

**Nothing an agent asks you gets lost.** "Done, but I have three questions"
used to scroll away. Now agents put their questions, the things only you can
do, and the follow-ups they noticed on a board you share with them, and carry
on with the rest. You answer when you get to it, and your answer goes back to
the agent that asked: typed into its session if it's idle, handed over when
its turn ends if it's busy. Queue work for an agent the same way. Ten agents
on one board, and each hears only about its own cards.
[Board](docs/features/board.md)

**You read the code before it lands.** Agents write more than you can skim in
a terminal. The git panel shows what one changed while it's changing it: stage
hunk by hunk, comment on a line, send the comments back as a review, commit,
open the pull request. [Git panel](docs/features/git-panel.md)

**The work doesn't stop when you do.** Shells and agents live in a background
session, not in the window. Quit Wezzy, update it, come back tomorrow:
everything is where you left it, still running. Put agents on another machine
and they keep going with your laptop shut; connect again and that machine's
workspaces are back beside your own, as if they were local.
[Remote hosts](docs/features/remote-hosts.md)

**Agents can use the terminal too.** The `wezzy` command lets an agent open a
split, start the dev server beside itself, and read what it printed, instead
of asking you to. [The wezzy command](docs/features/cli.md)

**It's still your terminal.** Your WezTerm config works unchanged. ⌘K finds
anything: agents, tabs, workspaces, recipes, hosts, commands. Paste or drop a
screenshot straight into an agent. The mouse works when you want it.

## Getting around

Enough to start. Everything else is one of these away.

| | |
|---|---|
| ⌘K | Find anything: agents, tabs, workspaces, hosts, commands |
| ⌘I | Start an agent |
| ⌘J | Agents: what's waiting on you, what each one has done |
| ⌘⇧J | Board: what agents need from you, what you've queued for them |
| ⌘G | Git panel: review what an agent changed |
| ⌘⇧K | Switch workspace |
| Leader ? | Every key (the leader is ⌃A); `?` inside a panel lists that panel's |

Each feature's keys are in its page under [docs/](docs/README.md), and the
terminal's own (panes, tabs, copy mode, changing a key) are in
[Keys](docs/features/keys.md).

## Setup

Exact agent status (Claude Code, Codex, pi, OpenCode) needs hooks: ⌘, → Agent hooks.

To have agents use the board (Claude Code for now): ⌘K → Install Handoff. It
gives them the tools to file and read cards and brings your answers back to
them.

To run agents on another machine: ⌘⇧S, pick the host, **Set up**. It needs
key-based ssh and nothing else; see [Remote hosts](docs/features/remote-hosts.md).
Agents on a host run and persist there, but aren't in the sidebar or ⌘J yet.

Config lives in `~/.config/wezzy/`: `wezzy.lua` for appearance, keys and the rest, `settings.json` for what ⌘, edits. See [Configuration](docs/features/config.md).

```lua
local wezzy = require("wezzy")

local config = wezzy.setup({
  window = { padding = 16, opacity = 0.9 },
  keys = { { "cmd+b", wezzy.action.ToggleSidebar } },
  workspaces = { { name = "wezzy", cwd = "~/work/wezzy" } },
})

return config
```

## Build

```sh
cargo run
scripts/bundle.sh           # Wezzy.app in target/release, for this Mac
scripts/rebuild.sh          # bundle, install in /Applications, restart the app
scripts/release.sh          # signed + notarized draft release (docs/development/releasing.md)
```

Working on Wezzy inside Wezzy: ⌘K → Rebuild & Restart.

More in [CLAUDE.md](CLAUDE.md) and [docs/](docs/README.md).

Built on [WezTerm](https://github.com/wezterm/wezterm) and [gpui](https://github.com/zed-industries/zed).

MIT licensed; see [LICENSE](LICENSE).
