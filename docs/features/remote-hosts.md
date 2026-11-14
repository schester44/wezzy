# Remote hosts

Keep shells and agents running on your other machines and work with them as if
they were local. Connect a host and its workspaces join yours: switch to them,
split, open tabs, close the lid. Everything keeps running on the host, and the
next time you connect it's all where you left it.

## Using it

| | |
|---|---|
| ⌘⇧S, then **devbox** under SSH, then **Set up** | Install Wezzy's server on the host |
| ⌘⇧S, then **devbox** under Hosts | Connect the host and switch to its workspace |
| ⌘⇧S, then **Disconnect devbox** | Close the connection; its shells keep running there |
| ⌘⇧S, then **Update devbox** | Send the host this Wezzy's build of the server |
| ⌘⇧K | A connected host's workspaces are in the list as `devbox/default`, `devbox/api`, … |

⌘⇧S lists hosts with Wezzy's server under **Hosts**, with a server icon and a
dot while connected. **SSH** below it has a plain ssh session for every host,
with a terminal icon. Choosing a host that doesn't have the server yet asks
whether to set it up: **Set up** (↵) installs it and connects, **SSH** (`n`)
opens a plain ssh session and stops asking about that host until Wezzy
restarts, esc does neither. The same rows are in ⌘K: type `devbox`.

A host's workspaces have a server icon in the workspace menu and in ⌘⇧K, and
on the workspace button while you're in one.

Once you're in a host's workspace, everything you open there runs on the host:
splits, new tabs, an agent from ⌘I. An agent starts through your login shell
on the host, as it would over ssh. A new workspace named `devbox/<name>` opens
on that host too.

PaneSelect's pane ids say which machine a pane is on: `devbox:3`.

## Setting it up

Hosts are the ones listed under [SSH](ssh.md): your `~/.ssh/config` aliases
and the hosts in your config.

1. **Key-based ssh.** Wezzy connects without a terminal to type into, so
   `ssh devbox` must log in without asking for a password or a passphrase
   (an ssh agent is fine).
2. **Install the server.** Choose **devbox** in ⌘⇧S and answer **Set up**.
   Wezzy sends the host its server over ssh and installs it in the
   background; a message says when it's done, or what went wrong, and the
   host connects once it's set up. Wezzy carries the
   server for 64-bit Linux (x86-64 or ARM, Ubuntu 20.04 or Debian 11 and
   newer), so the host doesn't need internet access. On other systems the
   installer downloads it, or builds it from source if the host has
   [Rust](https://rustup.rs) and a C toolchain, which takes a few minutes.

   You can also install it on the host yourself, if it has internet access:

   ```sh
   curl -fsSL https://raw.githubusercontent.com/schester44/wezzy/main/scripts/install-server.sh | sh
   ```

3. **Connect.** Once a host is set up, it moves to **Hosts** in ⌘⇧S; choose
   it there to connect.

The installer puts `wezzy` in `~/.local/bin` on the host. On a machine with
systemd it also installs a user service, so the server starts at boot and
keeps running after you log out. Without systemd it starts on the first
connection and runs until the machine restarts.

## Details

- **Connected hosts reconnect.** Hosts that were connected when you quit Wezzy
  connect again when it starts, in the background, after your own sessions are
  up.
- **Workspace names.** Every machine has its own `default`, so a host's
  workspaces carry its name: `devbox/default`. Renaming one renames it on the
  host.
- **When a connection fails** a short message says why: ssh's own error
  (`Permission denied (publickey)`, `Connection timed out`), or
  `wezzy isn't set up there` when the server isn't installed, after which the
  host moves back to **SSH** and asks to be set up again. Wezzy gives up on an unreachable host after 10
  seconds.
- **A host you set up by hand** (with the `curl` line) moves to **Hosts**
  once you've answered **Set up** for it in Wezzy.
- **Update** sends the host this Wezzy's build of the server. Its sessions
  keep running on the old one until the server restarts.
- **Set up and Update's full output** is in `~/.local/share/wezzy/hosts/<host>-setup.log`.
- **On the host, from a shell:** `wezzy server status`, `wezzy server logs`,
  and `wezzy server stop` (which ends every shell on it). See
  [the wezzy command](cli.md).
- **What stays on your Mac.** The git panel, ⌘-click on a file path, the
  GitHub Actions source and database suggestions only look at this machine,
  so they do nothing in a host's pane (⌘-click on a link still opens it).
  Agents on a host don't show up in the sidebar or ⌘J yet.
- **Versions.** The host's server and Wezzy have to speak the same protocol.
  Each protocol version installs into its own folder on the host, so a server
  from another version keeps running its sessions untouched.
