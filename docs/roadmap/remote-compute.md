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

Built: [Remote hosts](../features/remote-hosts.md) and
[the wezzy command](../features/cli.md). Connect over the system `ssh` to
`wezzy proxy` on the host, `wezzy server` under a systemd user service,
workspaces prefixed with the host, pane ids per server
(`docs/development/architecture.md`, Sessions). The shape follows Rex's
Linux servers (superlogical.com/rex/docs/linux/installation).

Left:

- **Tried on a real host.** Everything above is tested against a server on
  this Mac through a stand-in `ssh`; nothing has run on Linux yet (the
  install script's `cargo install`, systemd, linger).
- **Server builds in CI.** `scripts/build-server.sh` builds the Linux
  binaries in Docker on the Mac that bundles the app; move that to Actions
  (`ci-releases.md`) and add musl builds for hosts older than glibc 2.31.
- **Dropped connections.** On wake or a network change, reconnect hosts that
  were connected; until then grey out their workspaces. Today you connect
  again from ⌘K.
- **Visible location.** Agent cards and the workspace switcher could show the
  host beyond the `devbox/` prefix; the palette could offer "New Claude agent
  on devbox".
- **Version skew.** When the WezTerm pin moves, say so on connect
  (`devbox: 3 sessions on an older server`) instead of a codec error, and
  offer Set up.
- **Socket activation.** The systemd unit starts the server at boot; socket
  activation would need `wezterm-mux-server-impl` to accept an inherited
  listener.
- **The CLI's reach.** Rex's CLI goes further (superlogical.com/rex/docs/reference/cli):
  `wait` for a pane's command to exit with its status, `events` as a JSON
  stream, `send-key`, `move`/`swap`, `hosts check`, and `skill setup` to
  install the agent skill from the binary.

### 2. Make everything that's local today work remotely

Several features read the local machine directly. Each needs a remote path:

| Feature | Today | Remote |
|---|---|---|
| Agent status (`hooks.rs`) | hook writes `~/.local/state/wezzy/agents/<pane>.json`, app polls it | hook writes an OSC 1337 `SetUserVar` (`WEZZY_AGENT=<json>`) to its tty; it rides the mux like smart-splits' vars (vendored `wezterm-client` already republishes them). Same transport works locally, so the file path can go away. |
| Agent detection (`procs.rs`) | foreground process via the local process table and pane tty | hooks become the primary signal. A small helper on the server can answer "what's in the foreground of pane N" if needed. |
| Diff panel (`diff.rs`) | runs `git` in the pane's cwd | runs `git` over the same SSH connection (`ssh host git -C <cwd> …`), or through the helper. Same polling, same snapshot format. |
| Dev servers (`servers.rs`) | `lsof` + pane tty matching | detected on the server; Wezzy forwards the port (`ssh -L`) so `localhost:PORT` still opens in the local browser. |
| ⌘-click `file:line`, ⌘E | opens the editor in a new split, after checking the file exists locally | does nothing in a host's pane today (`session::on_host`). Skip the local check there and split in the pane's domain, so the editor runs on the host next to the files. |
| Send output to agent | reads pane text via the mux | already works remotely. |

The helper is the host's `wezzy` (`cli/`), a new subcommand (`wezzy agentd`)
reached over the same ssh connection. It owns process inspection,
git, and port detection for that host, so the app doesn't have to know about
remote shells.

### 3. Sandbox providers

The same shape with a different way to get a machine:

- **A sandbox domain** that, on attach, creates or resumes a box with the
  provider's API, starts `wezzy server` (and `wezzy agentd`) inside, and
  connects over the provider's tunnel or SSH.
- **Lifecycle in the UI:** create, pause or resume, and destroy, from the
  workspace switcher. Cost per hour shown where the host normally appears.
- **Images:** a base image with `wezzy` and common harnesses
  (claude, codex, pi), plus a per-project setup script (from the repo's
  `.wezzy.toml`).
- **Secrets:** harness auth (API keys, OAuth tokens) is injected per sandbox,
  never baked into images.

## Open questions

- **Networking:** ssh over Tailscale already works with phase 1. Is a direct
  non-ssh path (Tailscale Serve, or tsnet built into `wezzy server`) worth it?
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
