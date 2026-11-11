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

- On the server: install `wezterm-mux-server` from the same WezTerm commit
  Wezzy pins (`e68a8ec`, see `Cargo.toml`). The mux protocol has to match.
- In Wezzy: a list of hosts in settings (`name`, `ssh` target), each becoming
  a WezTerm SSH client domain (`ClientDomainConfig::Ssh`). The list exists
  already (`hosts.rs`): today each host opens a plain `ssh` session in a local
  pane; this phase upgrades the same entries to domains. WezTerm proxies the
  connection with `wezterm cli proxy` on the far side.
- **Choosing where things run:** per workspace by default (a workspace is bound
  to a host), with a per-launch override in the agent launcher (⌘A on a remote
  workspace starts the harness there; the palette lists "New Claude agent on devbox").
- **Visible location:** the workspace switcher and agent cards show the host
  when it isn't local. No other chrome.
- **Persistence:** a remote mux server keeps running when the laptop sleeps or
  goes offline. Reattaching restores everything, the same as local sessions do today.

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
  provider's API, starts `wezterm-mux-server` and `wezzy agentd` inside, and
  connects over the provider's tunnel or SSH.
- **Lifecycle in the UI:** create, pause or resume, and destroy, from the
  workspace switcher. Cost per hour shown where the host normally appears.
- **Images:** a base image with the mux server, agentd and common harnesses
  (claude, codex, pi), plus a per-project setup script (from the repo's
  `.wezzy.toml`).
- **Secrets:** harness auth (API keys, OAuth tokens) is injected per sandbox,
  never baked into images.

## Open questions

- **Version pinning:** the mux protocol isn't stable across WezTerm versions.
  Wezzy should probably ship and install its own server build on remote hosts
  (`wezzy install-server <host>`) instead of relying on whatever is there.
- **Networking:** plain SSH everywhere, or Tailscale as the recommended path
  for home servers (no port forwarding, stable names, works from mobile too)?
- **Latency:** the mux client already does local echo prediction
  (`local_echo_threshold_ms`). Tune it per host once we have real numbers.
- **Multiple hosts at once:** one window showing panes from several hosts works
  at the mux level (multiple domains). Decide whether a single tab may mix hosts
  or a tab is always single-host.

## Not in scope (yet)

- Moving a running agent between machines.
- Scheduling: choosing a host automatically based on load or cost.
