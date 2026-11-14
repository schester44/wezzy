# The wezzy command

Drive Wezzy from a shell: open splits and tabs, type into a pane, read what's on
its screen. It's how a script, or an agent, starts a dev server beside itself
and watches it come up. Add `-S <host>` and the same commands work on a
[remote host](remote-hosts.md).

## Using it

| | |
|---|---|
| `wezzy ls` | List panes (`--json` for everything) |
| `wezzy split -- npm run dev` | Split your pane, run a command; prints the new pane's id |
| `wezzy new` | Open a tab; `--window` for a window, `--workspace <name>` to put it there |
| `wezzy send -p 12 -e "cargo test"` | Type into a pane; `-e` presses enter after |
| `wezzy capture -p 12` | Print a pane's screen; `-n 500` adds 500 lines of scrollback |
| `wezzy focus 12` | Focus a pane |
| `wezzy zoom -p 12` | Zoom or unzoom a pane (`--on`, `--off`) |
| `wezzy title "dev server"` | Name the tab |
| `wezzy kill 12 13` | Close panes |
| `wezzy version` | Version and protocol |

Commands act on your own pane unless you pass `-p <id>`.

`split` goes to the right by default; `--left`, `--up` and `--down` go
elsewhere. Size it with `--percent 30` or `--cells 15`, and pass `--top-level`
to split the whole tab. `split` and `new` take `-c <dir>` for the working
directory, and a command after `--`. Without one you get your shell. A pane
started with a command closes when the command exits.

`send` types the text as if you'd typed it. Pass `--paste` to send it as a
paste, or pipe text in: `echo ls | wezzy send -p 12`.

`capture` prints plain text. `--escapes` keeps colors and styles.

### On a host

```sh
wezzy -S devbox ls
wezzy -S devbox new --window --workspace api -- htop
wezzy -S ssh://dev@10.0.0.5:2222 capture -p 3
```

`-S` takes what you'd type after `ssh`: an alias from `~/.ssh/config`,
`user@host`, or `ssh://user@host:port`. You can also set `WEZZY_SERVER`.
Pane ids are the host's own (`devbox:3` in Wezzy is `-S devbox -p 3`). Without
`-p`, commands act on the pane last focused there.

### The host's server

On a host where you've installed Wezzy:

| | |
|---|---|
| `wezzy server status` | Whether it's running (`--json`) |
| `wezzy server logs` | Its log; `-f` to follow |
| `wezzy server start` | Start it in the background |
| `wezzy server stop` | Stop it. Every shell and agent on it ends |
| `wezzy server install` | Start it at boot and keep it running after logout (systemd) |
| `wezzy server uninstall` | Undo `install`; a running server keeps running |

## Setting it up

There's nothing to install: `wezzy` is the app itself. Wezzy puts it on the
PATH of its own panes, and links it into `~/.local/bin` when that folder is on
your PATH, so other terminals have it too. If `~/.local/bin` isn't on your
PATH, link `~/.local/share/wezzy/bin/wezzy` into a folder that is.

On a host, [Set up](remote-hosts.md#setting-it-up) installs it in
`~/.local/bin`.

## Details

- `kill` won't close the pane you're running in.
- `new` without `--workspace` stays in your pane's workspace, so the tab or
  window opens where you can see it.
- Wezzy only replaces a `~/.local/bin/wezzy` that it made itself; if you
  have another program by that name there, it's left alone.
- `wezterm cli` keeps working in Wezzy's panes too.
