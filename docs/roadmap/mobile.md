# Mobile

Check on agents and unblock them from a phone: see who's working, who needs
you, read what they did, approve a permission prompt, send a quick reply.
Writing code on a phone is not the goal.

## Shape

Mobile is another client of the same sessions, not a separate product.

```
            ┌───────────────────────────── home server ─────────────────────────────┐
 Mac app ───┤  wezterm-mux-server  ←→  wezzy agentd (status, git, ports, push)    ├─── phone (web app)
            └────────────────────────────────────────────────────────────────────────┘
```

- The always-on machine (a home server, see `remote-compute.md`) owns sessions.
  The Mac app and the phone both attach to it, so they always see the same agents.
- `wezzy agentd` is the piece mobile talks to. It already needs to exist for
  remote compute, so mobile adds an HTTP and WebSocket API to it rather than
  a new service.
- **Networking:** Tailscale is the default path. The phone reaches the server
  by name, nothing is exposed to the internet, and there's no account system to build.

## Phases

### 1. Web app with the agent inbox

A small web app served by `agentd`, installable to the home screen.

- **Inbox:** every agent across workspaces with its exact hook status (working
  + current tool, needs input + Claude's message, done). The same ordering and
  states as the desktop sidebar.
- **Agent view:** the pane's recent output as text (last few hundred lines from
  the mux, no terminal emulation needed for reading), the diff summary for its
  repo, and a reply box that pastes into the agent's prompt.
- **Quick actions:** approve or deny a permission prompt (send the keystroke
  Claude expects), interrupt (Ctrl+C), send a canned reply ("continue", "run the tests").
- **Live updates** over a WebSocket fed by the same status stream the desktop uses.

### 2. Push notifications

- `agentd` sends a push when an agent goes to needs-input or done, with the
  same rules as the desktop chime (only once per transition, rate-limited).
- **Transport:** ntfy first (self-hostable, no Apple developer setup, works
  today with the home-screen web app). Web Push from the installed web app
  after that. A native app would use APNs.
- **Tap a notification:** opens straight to that agent with the reply box focused.
- **Quiet when you're at the desk:** if the Mac app had input in the last few
  minutes, skip the push (the chime already covered it).

### 3. Full terminal (optional)

Interactive terminal on the phone for when a reply box isn't enough: a WebGL
terminal renderer in the web app, fed by the mux. Useful for poking at a stuck
agent; not something to optimize for.

### 4. Native app (later)

Only if the web app hits real limits (background push reliability, share sheet,
widgets). The API stays the same.

## Security

- Tailscale identity is the auth boundary for v1. `agentd` only binds to the
  tailnet interface.
- Every action from mobile is a normal input to a pane, so the same agent
  permission prompts apply. Nothing on mobile bypasses them.
- Later, if exposing beyond a tailnet: device pairing via QR code from the Mac
  app, short-lived tokens, and an audit log of mobile actions.

## Open questions

- **Reply semantics:** paste-and-submit, or paste and let the user confirm?
  Probably submit for canned replies and confirm for free text.
- **Permission prompts:** Claude's prompt is a TUI. Map "approve" and "deny"
  to keystrokes per harness (a small table in `agentd`), or have hooks expose
  the pending decision directly?
- **Multiple machines:** if agents run on several hosts, does each run its own
  `agentd` with the phone aggregating, or does one `agentd` front them all?
