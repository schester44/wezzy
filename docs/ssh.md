# SSH hosts

Open a shell on another machine from the palette. Hosts come from your config
and from `~/.ssh/config`, so aliases you already have show up without setup.
Any target you type also works, configured or not.

## Using it

| | |
|---|---|
| ⌘⇧S | SSH picker (the palette, scoped to hosts) |
| ⌘K, then `ssh <name>` | Same hosts in the full palette |
| ↵ | Open in a split |
| ⌘↵ | Open in a new tab |

The picker matches on the name and the target, so `0.5` finds a host
whose target is `dev@10.0.0.5`. Each row shows the host's name with
`ssh <target>` beside it, so in ⌘K `ssh devbox` finds it too.

To connect somewhere nobody configured, type the target in the picker
(`pi@10.0.0.9`, `-p 2222 admin@nas.local`) or `ssh <target>` in ⌘K. The last
row is `ssh <target>`. It's listed after every match so a configured host always
comes first, and it's hidden when the text exactly matches a host's name.

## Configuring it

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  wezzy.setup({
    hosts = {
      { name = "devbox", ssh = "dev@10.0.0.5" },
      { name = "nas", ssh = "-p 2222 admin@nas.local" },  -- anything ssh takes
    },
  })
end
```

or in `~/.config/wezzy/settings.json`:

```json
"hosts": [{ "name": "devbox", "ssh": "dev@10.0.0.5" }]
```

- `name` is what you search for and what the row shows.
- `ssh` is the arguments to `ssh`, written as on a command line: a target, an
  ssh_config alias, or options plus a target. It's split like a shell would
  split it, so quote arguments that contain spaces.
- You can use both. Entries in the Lua config replace `settings.json` entries
  with the same name.

### From `~/.ssh/config`

Every `Host` alias without wildcards (`*`, `?`, `!`) is listed with its alias
as both name and argument, so ssh applies the rest of that block. The row shows
where the alias goes: `User@HostName`, plus `:Port` when it isn't 22. A
`Host a b` line gives two rows.

- `Include` is followed, up to four levels deep. Paths can be absolute,
  `~/…`, or relative to `~/.ssh`, and `*` in the file name matches files in
  sorted order.
- `Match` blocks are skipped; they aren't something you'd pick by name.
- An alias with the same name as a configured host is hidden; the configured
  host wins.

Order in the picker: configured hosts in config order, then ssh_config aliases
in file order.

## Behavior

- A host opens like an agent from ⌘N. ↵ places the split by
  `agent_layout` (`bsp` or `master-stack`); with no tab open, it opens a tab
  instead. ⌘↵ opens a new tab in the current workspace.
- `ssh` runs through your login shell (`$SHELL -lic`), so your `PATH`, ssh agent
  setup and shell aliases apply.
- When the ssh session ends, the pane stays open with a local login shell.
- The pane gets `WEZZY_HARNESS=ssh` in its environment.
- These are plain `ssh` processes in a local pane. Wezzy doesn't connect to the
  remote machine itself, so remote panes have no agent tracking, git panel or
  dev-server detection for the remote side.
- Hosts can't be edited in Settings (⌘,). Use the config or `settings.json`.

## Code

| | |
|---|---|
| `src/hosts.rs` | `Host`, `all()` (configured + ssh_config), ssh_config parsing, `open_ssh` |
| `adhoc_ssh` (`palette.rs`) | the `ssh <target>` row for unconfigured targets |
| `spawn_harness`, `run_in_split` (`launch.rs`) | split/tab placement and the login-shell wrapper |
| `settings.rs` | `hosts` in `settings.json` and `setup()` |

`docs/roadmap/remote-compute.md` (phase 1) turns the same `name`/`ssh` list
into SSH mux domains.
