# SSH

Open a shell on any of your machines in two keystrokes. The hosts in your
`~/.ssh/config` are already there, you can add your own, and anything you type
connects too.

## Connecting

| | |
|---|---|
| ⌘⇧S | Pick a host |
| ⌘K, then `ssh …` | The same hosts, from the main palette |
| ↵ | Open it in a split |
| ⌘↵ | Open it in a new tab |

Search by name or by address: `devbox` and `0.5` both find
`devbox  ssh dev@10.0.0.5`.

To reach a machine that isn't in your list, type where it is and press ↵ on the
`ssh …` row at the bottom:

- in the picker: `pi@10.0.0.9`
- in ⌘K: `ssh pi@10.0.0.9`

Anything you'd type after `ssh` works, options included: `-p 2222 admin@nas.local`.

## Your hosts

Every `Host` in `~/.ssh/config` shows up under its alias, with where it goes
beside it. Files pulled in with `Include` count too. Entries with wildcards
(`Host *`) and `Match` blocks aren't listed, since they aren't machines you'd
pick by name.

To add hosts of your own, or give a machine a friendlier name:

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  wezzy.setup({
    hosts = {
      { name = "devbox", ssh = "dev@10.0.0.5" },
      { name = "nas", ssh = "-p 2222 admin@nas.local" },
    },
  })
end
```

Put this in `~/.config/wezzy/wezzy.lua`, or list them in
`~/.config/wezzy/settings.json`:

```json
"hosts": [{ "name": "devbox", "ssh": "dev@10.0.0.5" }]
```

- `name` is what you search for.
- `ssh` is everything after `ssh` on the command line: an address, a
  `~/.ssh/config` alias, or options and an address.

Your own hosts come first, in the order you list them, then the ones from
`~/.ssh/config`. If one of yours has the same name as an alias, yours is used.
If `wezzy.lua` and `settings.json` both list a name, `wezzy.lua` wins.

## Good to know

- Splits go where a new agent from ⌘I would, following your agent layout
  setting.
- ssh starts from your login shell, so your `PATH`, keys and agent setup
  work as they do in any terminal.
- When you log out of the remote machine, the pane stays open with a local
  shell.
- Wezzy sees the session as a local pane running `ssh`. Agent status, the git
  panel and dev servers don't reach into the remote machine.
- Hosts aren't in Settings (⌘,); add them in your config.
