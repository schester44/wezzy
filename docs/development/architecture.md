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
  both, so `user-var-changed` handlers (smart-splits) work. It also fixes
  `split_pane` with `SplitSource::MovePane`, which sent the local pane id.
- **Layout changes must go through the server.** The client rebuilds each
  tab's split tree from `ListPanes` on every resync (`TabResized`, which any
  pane resize can trigger), so a change made only to the local `Tab` (e.g.
  `Tab::swap_active_with_index`, `rotate_*`) is undone. The protocol can split,
  move a pane into a split (`MovePane`), and move a pane to a new tab or window;
  `paneselect.rs` builds swaps from those.
- **Moving tabs:** no PDU moves a tab between windows. A tab that is its
  window's only tab moves with its window (`SetWindowWorkspace`, via
  `Window::set_workspace`). Any other tab is rebuilt in place: its first pane
  goes to a new tab (`MovePaneToNewTab`), then each split is redone with
  `MovePane`, walking the old `codec_pane_tree` (`rebuild_tab` in `app.rs`).
  The client drops a local tab once the server has removed it and its panes
  live elsewhere, so a moved pane isn't shown twice.

## View panes (`views/`)

Native gpui views (not terminals) that live in the normal tab/split layout.

- **A placeholder holds the slot.** The server owns the layout and the client
  rebuilds it from `ListPanes`, so a view still occupies a real server pane.
  `views::spawn_command` runs `sh` with `stty -echo`, which sets the pane title
  (OSC 2) and the `WEZZY_VIEW` user var (`"<kind> <args>"`), then `exec cat`.
  Splits, zoom, swaps, moving tabs and workspaces all treat it as an ordinary
  pane, and it survives app restarts with the session.
- **The app draws over it.** `Views::sync` (each frame) creates a `PaneView`
  for every pane announcing one and drops those whose pane is gone. The canvas
  skips painting their cells, and `pane_views` in `ui/view.rs` places each
  view absolutely over its pane's rectangle, using the grid geometry from the
  previous paint.
- **Keys:** the keymap still runs first (⌘ chords, leader, Ctrl+h/j/k/l);
  only `Lookup::Unbound` keys go to `PaneView::key` instead of the pane.
- Closing a view pane never asks for confirmation.

## Agents

- **Detection** (`agents.rs`): foreground process name/argv matches a known
  harness, or a hook record exists.
- **Status** (`hooks.rs`): `assets/hooks/agent-hook.sh`, registered as a
  Claude Code hook, writes each lifecycle event to
  `~/.local/state/wezzy/agents/<WEZTERM_PANE>.json`. The app polls the
  directory, maps server pane ids to local ones and derives
  Working / NeedsInput / Done / Idle. Without hooks, status is guessed from
  title spinners, output and bells.
- **Subagents:** events fired inside a Claude subagent carry `agent_id`; the
  script writes them to `<WEZTERM_PANE>.<agent_id>.json` so they don't
  overwrite the main agent's status, and `HookState::subagents` collects them
  per pane. Labels and the finished flag come from Claude's undocumented
  `<session>/subagents/agent-<id>.meta.json`, read best-effort. A record goes
  on `SubagentStop`, when the meta says finished, when the main session ends
  or changes, or after 30 minutes without events. A subagent waiting on you
  makes its agent NeedsInput. `hooks::start` re-registers hooks installed by a
  build that subscribed to fewer events.
- **Transitions:** state changes raise toasts, tab dots and the chime
  (`sound.rs`) when you're not looking at that pane.

## Sidebar (`sidebar.rs`)

- **Sources:** the sidebar is a stack of sources (`Source`: agents, servers,
  actions).
  Each has its own background poller and turns its snapshot into `Item`s;
  `TermApp::sidebar_sections` lists the enabled ones that have items, in the
  order of the `sidebar` setting. The view and the keyboard cursor
  (`regions.rs`) both walk that list, so row indexes always agree.
- **Opt-in:** sources that cost something to poll check
  `settings::get().sidebar_enabled(..)` and idle while they're off
  (`servers.rs` stops running `lsof`). Sources should also stay hidden when
  they don't apply to the workspace.
- **Actions** (`actions.rs`, opt-in): shells out to `gh` so its auth and
  hosts apply. The view sets the target (active pane's cwd) only while the
  sidebar is open with Actions on; the poller resolves repo root + branch
  (or commit when detached), `owner/repo` via `gh repo view` (cached;
  unresolvable repos retried every 5 min), then `gh run list`, keeping each
  workflow's newest run. 10s while a run is active, 60s otherwise; results
  are cached per branch so switching back is instant. ↵ runs `gh run watch` /
  `gh run view --log-failed` / `gh run view` in a split (`run_command_in_split`).

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
