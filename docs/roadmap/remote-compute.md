# Remote compute

Run agents on machines other than the laptop: home servers first, sandbox
providers later. The Mac app stays the place you watch and steer them from.

## Why it fits

Wezzy is already a client. Shells and agents live in a `wezterm-mux-server`
that the app attaches to over a unix socket (`src/session.rs`), which is why
sessions survive app restarts. Remote compute means putting that server, or
more of them, somewhere else and attaching to it over the network. Rendering,
input, splits, workspaces, keybindings and Lua config all already go through
the mux protocol and need no changes.

## Phases

### 1. Home server over SSH

The shape is the one Rex uses for its Linux servers
(superlogical.com/rex/docs/linux/installation): one binary on the server's
`PATH` that is both the server and its CLI, reached with the system `ssh`
and kept alive by a systemd user service.

#### Transport: the system `ssh` as a proxy command

Each host becomes a `UnixDomain` whose `proxy_command` runs ssh, not a WezTerm
SSH domain (`ClientDomainConfig::Ssh`):

```rust
UnixDomain {
    name: format!("host:{}", host.name),
    proxy_command: Some(["ssh", "-T", "-o", "BatchMode=yes", <host.ssh args>,
                         "wezzy-remote-<pin>", "proxy"]),
    ..
}
```

The vendored client already supports this (`UnixTarget::Proxy` in
`vendor/wezterm-client/src/client.rs`): it runs the command and speaks the mux
protocol over its stdio. Compared with the SSH domain:

- It's the user's own `ssh`, so `~/.ssh/config`, keys, the agent, ProxyJump and
  Tailscale SSH behave as they do in a terminal. The `hosts.rs` entries
  (`-p 2222 admin@nas`, aliases) work unchanged.
- It's the same domain type as the local session server, so there's one code
  path, not two.
- `BatchMode` means no password or host-key prompts. A failed connection is a
  terse toast (`devbox: permission denied (publickey)`).

The `hosts.rs` list stays the source of hosts. Each one gets "Connect devbox"
beside the plain "ssh devbox" (which stays a local pane running `ssh`). An
`ssh://user@host` typed in the picker connects without a config entry.

#### The server side: `wezzy-remote`

A Linux build (x86-64 and ARM64) of the pinned WezTerm crates:

| Command | Does |
|---|---|
| `serve` | the mux server (what `wezterm-mux-server` is today) |
| `proxy` | connect stdio to the server's socket, starting the server if nothing answers (what `wezterm cli proxy` does) |
| `version` | print the WezTerm pin and codec version |
| `server install` | write a systemd user unit + socket (socket activation), run `loginctl enable-linger` so it survives logout and starts at boot |

- **Install:** a tarball installer, `curl -fsSL <url>/install.sh | sh`, into
  `~/.local/bin`, no sudo. "Set up devbox" in the palette runs the same
  installer over ssh, then `server install`. Before the first connect Wezzy
  runs `ssh host wezzy-remote-<pin> version`, so "not installed" and "wrong
  version" are clear errors, not a hung "Connecting…".
- **Why systemd:** a server started with `--daemonize` from an ssh session can
  be killed when the session ends on systems with `KillUserProcesses`. With
  socket activation, `proxy` only has to connect; systemd starts the server.
  Without systemd (Alpine), `proxy` starts it and it lives until reboot.
- **Versioning by protocol, not by app release.** The mux protocol must match
  exactly (unlike Rex, whose servers keep running an older version). The binary
  and its socket are named after the WezTerm pin (`wezzy-remote-e68a8ec`,
  `sock-e68a8ec`), not the Wezzy version, so most Wezzy updates never touch a
  remote server. When the pin moves, the new server installs beside the old
  one. The old one keeps its sessions until you close them, and Wezzy says so
  (`devbox: 3 sessions on an older server`).

#### The app: more than one session domain

Everything assumes the single `"wezzy"` domain today:

| Place | Change |
|---|---|
| `session::DOMAIN`, `install_domain` | the local domain plus one per host; local stays the default |
| `attach_and_restore` | attach local first, remote hosts in the background, so a slow or missing host never blocks startup |
| `refresh_ttys` | every client domain, not `get_domain_by_name(DOMAIN)` |
| `local_pane_id` / `remote_pane_id` | `$WEZTERM_PANE` is ambiguous once two servers have a pane 3; ids become `(domain, id)`. Callers in `hooks.rs`, `agents.rs`, `notify.rs`, `sendout.rs`, `activity.rs`, `palette.rs`, `app.rs`, `ui/terminal.rs` |

Agent status over a user var (phase 2's first row) removes the id question for
agents, so it's worth doing alongside this.

#### Tabs and workspaces

`ClientDomain::attach` already pulls every server window into the local mux,
tagged with the server's workspace name, so the sidebar, picker and tabs show
them as they are. Names collide (every server has a `default`), so the
vendored `domain.rs` prefixes workspaces coming in (`devbox/default`) and
strips the prefix going out. Everything above it, including switching and
`last-workspace`, needs no change.

- **Choosing where things run:** splits go to the current pane's domain (they
  already do). New tabs and ⌘I go to the active workspace's domain; the
  palette lists "New Claude agent on devbox" as a per-launch override.
- **Visible location:** the workspace switcher and agent cards show the host
  when it isn't local. No other chrome.
- **Which hosts connect:** the ones attached when the app quit reattach at
  launch; others attach when you pick one of their workspaces or "Connect devbox".
- **Persistence:** a remote mux server keeps running when the laptop sleeps or
  goes offline. On wake or network change Wezzy reconnects; until then that
  host's workspaces are greyed out. Reattaching restores everything, the same
  as local sessions do today.

#### Order

1. Session code supports several domains, with only the local one. A pure
   refactor, testable today.
2. One remote host with a hand-written proxy (`wezterm-mux-server --daemonize`
   if not running, then `socat - UNIX-CONNECT:$sock`), attached from the
   palette: tabs show up, typing works.
3. Workspace prefix in the vendored client.
4. `wezzy-remote`, the installer, "Set up devbox" and the version check.
5. Phase 2.

### 2. Make everything that's local today work remotely

Several features read the local machine directly. Each needs a remote path:

| Feature | Today | Remote |
|---|---|---|
| Agent status (`hooks.rs`) | hook writes `~/.local/state/wezzy/agents/<pane>.json`, app polls it | hook writes an OSC 1337 `SetUserVar` (`WEZZY_AGENT=<json>`) to its tty; it rides the mux like smart-splits' vars (vendored `wezterm-client` already republishes them). Same transport works locally, so the file path can go away. |
| Agent detection (`procs.rs`) | foreground process via the local process table and pane tty | hooks become the primary signal. A small helper on the server can answer "what's in the foreground of pane N" if needed. |
| Diff panel (`diff.rs`) | runs `git` in the pane's cwd | runs `git` over the same SSH connection (`ssh host git -C <cwd> …`), or through the helper. Same polling, same snapshot format. |
| Dev servers (`servers.rs`) | `lsof` + pane tty matching | detected on the server; Wezzy forwards the port (`ssh -L`) so `localhost:PORT` still opens in the local browser. |
| ⌘-click `file:line`, ⌘E | opens the editor in a new split | already right: the split is created in the same domain, so the editor runs on the remote machine next to the files. |
| Send output to agent | reads pane text via the mux | already works remotely. |

The helper is the same `wezzy` binary run in a headless mode on the server
(`wezzy agentd`), reached through the SSH domain. It owns process inspection,
git, and port detection for that host, so the app doesn't have to know about
remote shells.

### 3. Sandbox providers

The same shape with a different way to get a machine:

- **A sandbox domain** that, on attach, creates or resumes a box with the
  provider's API, starts `wezzy-remote` and `wezzy agentd` inside, and
  connects over the provider's tunnel or SSH.
- **Lifecycle in the UI:** create, pause or resume, and destroy, from the
  workspace switcher. Cost per hour shown where the host normally appears.
- **Images:** a base image with `wezzy-remote`, agentd and common harnesses
  (claude, codex, pi), plus a per-project setup script (from the repo's
  `.wezzy.toml`).
- **Secrets:** harness auth (API keys, OAuth tokens) is injected per sandbox,
  never baked into images.

## Open questions

- **Networking:** ssh over Tailscale already works with phase 1. Is a direct
  non-ssh path (Tailscale Serve, or tsnet built into `wezzy-remote`) worth it?
  It matters mostly for the mobile client.
- **Distribution:** a tarball installer is enough for home servers. Package
  repos (apt, dnf) and a Nix flake, as Rex ships, only if people ask.
- **Latency:** the mux client already does local echo prediction
  (`local_echo_threshold_ms`). Tune it per host once we have real numbers.
- **Multiple hosts at once:** one window showing panes from several hosts works
  at the mux level (multiple domains). Decide whether a single tab may mix hosts
  or a tab is always single-host.

## Not in scope (yet)

- Moving a running agent between machines.
- Scheduling: choosing a host automatically based on load or cost.
