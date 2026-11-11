# Updates

Wezzy keeps itself up to date. When a new release is out, it downloads and
installs it in the background, and the next time Wezzy starts you're on the new
version. Your shells and agents keep running through the restart.

## Using it

| | |
|---|---|
| ⌘K → **Restart to Update to …** | Start the new version now. The command reads **Restart Wezzy** when there's nothing waiting |
| ⌘K → **Check for Updates** | Check right away, even with automatic updates off |
| ⌘, → Updates | **Automatic** or **Off** |

Wezzy checks shortly after it starts and every few hours after that. When an
update has been installed, a message names the version. Quitting and reopening
Wezzy also starts the new version.

## Setting it up

Updates are on unless you turn them off. To turn them off, set it in Settings
(⌘,), or in `~/.config/wezzy/wezzy.lua`:

```lua
local ok, wezzy = pcall(require, "wezzy")
if ok then
  wezzy.setup({
    auto_update = false,
  })
end
```

or in `~/.config/wezzy/settings.json`:

```json
"auto_update": false
```

If both set it, `wezzy.lua` wins.

## Installing

Download `Wezzy.dmg` from the
[latest release](https://github.com/schester44/wezzy/releases/latest), open it,
and drag Wezzy to Applications. That copy updates itself from then on.

## Good to know

- Wezzy only installs an update that's signed by the same developer as the copy
  you're running and approved by macOS. A download that doesn't check out is
  thrown away.
- Wezzy replaces itself where it's installed, so it needs to be able to write
  to that folder (Applications, usually). If it can't, the check fails with a
  message when you run **Check for Updates**, and quietly otherwise.
- Once in a while a release changes the session server that holds your shells.
  Wezzy doesn't install those on its own, because switching servers closes
  every shell and agent. It tells you once, and you install it yourself when
  you're ready: quit Wezzy, stop the session server with
  `kill $(cat ~/.local/share/wezzy/pid)`, install the new version from the
  release page, and open it.
- A copy you build yourself doesn't update itself; **Check for Updates** says
  so. Wezzy run from a source checkout doesn't offer the command.
