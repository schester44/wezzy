# Architecture

```
 ┌──────────────────────────── Wezzy (gpui app) ─────────────────────────────┐
 │  TermApp (app.rs)                                                          │
 │   ├─ view (ui/)        top bar · sidebar · terminal canvas · panels        │
 │   ├─ input (keys.rs)   gpui keystroke → WezTerm key → binding or pane      │
 │   ├─ perform()         KeyAssignment interpreter (split, tab, scroll, …)   │
 │   └─ Lua (luawin.rs)   action_callback / wezterm.on handlers               │
 │                                                                            │
 │  WezTerm crates: config (Lua) · mux · wezterm-term · termwiz               │
 │      Mux ── ClientDomain (vendor/wezterm-client) ──┐                       │
 └────────────────────────────────────────────────────┼───────────────────────┘
                                                      │ unix socket
                                ┌─────────────────────┴──────────────────────┐
                                │  wezterm-mux-server (session server)       │
                                │   LocalDomain → PTYs → shells, agents      │
                                └────────────────────────────────────────────┘
```

## Startup (`main.rs`)

1. Import `PATH` and `EDITOR` from the login shell when launched from Finder.
2. Set `WEZZY=1`. The session server and every pane inherit it, so agent
   hooks know they're inside Wezzy.
3. Load `settings.json`, register the `wezzy` Lua module, then load the
   WezTerm config (`~/.config/wezzy/wezzy.lua`; written from
   `config/wezzy.lua` on first run).
4. Create the mux and register a client identity: the mux tracks the active
   workspace per client, and `set_active_workspace` is a no-op without one.
5. Add the session server as the default domain (`session.rs`), then start gpui.
6. On window open, attach to the server (starting it if needed) and restore the
   last workspace, or spawn a first shell.

## Event flow

- **The main thread owns everything.** gpui runs the macOS run loop.
  WezTerm's `promise::spawn` futures are scheduled onto the GCD main queue
  (`install_schedulers`), which that run loop drains.
- **Mux → UI.** `Mux::subscribe` turns notifications into `AppEvent`s on a
  channel. `TermApp` drains it on the gpui foreground executor and calls
  `cx.notify()`. Pane output is coalesced into one redraw.
- **Background threads** (git polling, `lsof`, hook polling, builds) publish
  through `send_event(AppEvent::…)` or shared snapshots under a mutex.

## Input

`on_key` routes a keystroke to, in order: QuickSelect, inline inputs, the
workspace menu, overlays (palette, settings, prompts), a focused side panel
(`regions.rs`), Ctrl+h/l edge navigation, and finally the keymap (`keys.rs`):

- Leader and key tables are resolved like WezTerm does (`KeyState::lookup`).
- A binding produces a `KeyAssignment`, handled by `TermApp::perform()`.
  `EmitEvent("wezzy.*")` names are app commands; other `EmitEvent`s run Lua
  callbacks through `luawin.rs`, which gives them a `window` object whose
  `perform_action` comes back into `perform()`.
- Unbound keys are encoded by the pane (kitty keyboard protocol when enabled).

## Rendering (`render.rs`, `ui/view.rs`)

The terminal is a gpui `canvas`. Each frame it sizes the active tab to the
canvas (cols/rows from font metrics and config padding), snapshots each
positioned pane's visible lines, and paints:

- **Background runs and text:** same-styled cells are shaped as one run, with
  a forced grid advance so ligatures and nerd-font glyphs can't drift.
- **Cursor:** block cursors repaint the glyph underneath in `cursor_fg`.
- **Splits, then overlays:** split lines, then QuickSelect hints.

Chrome colors come from `Theme`, derived from the active pane's palette, so
the whole UI follows the WezTerm color scheme.

## Sessions (`session.rs`)

- The app is a client of `wezterm-mux-server`, using the bundled binary,
  `$WEZZY_MUX_SERVER`, or an installed WezTerm, in that order. The socket,
  pid and log are passed as `--config` overrides so the user's config stays
  free of plumbing.
- Every pane is a `ClientPane`. The protocol doesn't carry process info, so
  `session.rs` refreshes each pane's tty from `ListPanes` and `procs.rs` reads
  the foreground process for that tty from the macOS process table.
- **Vendored client patch:** user vars set before attach aren't synced, and
  `ClientPane` didn't republish `SetUserVar`. `vendor/wezterm-client` fixes
  both, so `user-var-changed` handlers (smart-splits) work.

## Agents

- **Detection** (`agents.rs`): foreground process name/argv matches a known
  harness, or a hook record exists.
- **Status** (`hooks.rs`): `assets/hooks/agent-hook.sh`, registered as a
  Claude Code hook, writes each lifecycle event to
  `~/.local/state/wezzy/agents/<WEZTERM_PANE>.json`. The app polls the
  directory, maps server pane ids to local ones and derives
  Working / NeedsInput / Done / Idle. Without hooks, status is guessed from
  title spinners, output and bells.
- **Transitions:** state changes raise toasts, tab dots and the chime
  (`sound.rs`) when you're not looking at that pane.

## Git panel (`diff.rs`, `gitpanel.rs`, `ui/git.rs`, `feedback.rs`)

- **Polling:** a background poller snapshots `git status` plus parsed hunks
  for the active pane's repo every 2s, with `GIT_OPTIONAL_LOCKS=0` so it never
  takes `index.lock`.
- **Mutations:** staging and discarding go through `git apply [--cached] [-R]`
  on single-hunk patches.
- **Review comments:** comments on hunks or lines are sent to an agent as one
  review message with `file:line` context.

## Persistence

| What | Where |
|---|---|
| Shells, agents, layout, scrollback | the session server (lives until killed) |
| Active workspace | `~/.local/share/wezzy/last-workspace` |
| App settings | `~/.config/wezzy/settings.json` |
| Agent status | `~/.local/state/wezzy/agents/` |
