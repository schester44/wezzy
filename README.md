# Dekita

A terminal for running coding agents. Built on WezTerm's engine.

![Dekita](docs/screenshot.png)

- Sessions persist. Quit the app, your shells and agents keep running.
- See which agent is working, which needs you, and which is done. A chime when one needs you.
- Git panel: stage hunks, comment on lines, send the comments to an agent, commit.
- Uses your WezTerm config as is. Keys, leader, plugins, smart-splits.
- Vim keys everywhere.

macOS only for now.

## Keys

| | |
|---|---|
| ⌘K | Palette |
| ⌘N / ⇧⌘N | New agent in a split / tab |
| ⌘D | Git panel |
| ⌘R | Recipes |
| ⌘E | Editor |
| ⌘B | Sidebar |
| ⌘, | Settings |
| ⇧⌘↵ | Send last output to an agent |
| ⌃⇧Space | Quick select |
| ⌃h/j/k/l | Move between panes and panels |

## Setup

Agent status needs the Claude Code hook: ⌘, → Agent hooks → Install.

Config lives in `~/.config/dekita/` (`dekita.lua` is WezTerm config, `settings.json` is the rest). Bind app actions like any WezTerm action:

```lua
local ok, dekita = pcall(require, "dekita")
if ok then
  table.insert(config.keys, { key = "b", mods = "CMD", action = dekita.action.ToggleSidebar })
end
```

## Build

```sh
cargo run
scripts/bundle.sh --dmg   # Dekita.app + dmg in target/release
```

Working on Dekita inside Dekita: ⌘K → Rebuild & Restart.

More in [CLAUDE.md](CLAUDE.md), [docs/architecture.md](docs/architecture.md) and [docs/roadmap](docs/roadmap).

Built on [WezTerm](https://github.com/wezterm/wezterm) and [gpui](https://github.com/zed-industries/zed).
