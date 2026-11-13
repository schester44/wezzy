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
  `cx.notify()`. Pane output is coalesced into one redraw; output from panes
  not on screen (other tabs / workspaces) redraws at most every 500 ms.
- **No per-frame animations.** A gpui `Animation` repaints the whole window,
  every pane included, at display rate. `ui::spinner` steps on a 125 ms timer
  that only runs while a spinner was drawn.
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

## Rendering (`render.rs`, `ui/terminal.rs`)

The terminal is a gpui `canvas`. Each frame it sizes the active tab to the
canvas (cols/rows from font metrics and config padding), snapshots each
positioned pane's visible lines, and paints:

- **Cards** (`pane_cards`, on by default): one rounded quad per pane, filled
  with `term_bg` and outlined with a hairline, on the darker `chrome` that the
  sidebar and top bar use. The side panels drop their right border, since the
  change of tone already separates them. Panes never leave the cell grid, so nothing that
  maps the mouse to cells changes. Instead each card reaches into the one-cell
  split strips around its pane and leaves the same gap on every side
  (`GridGeom::card`). That caps the gap at one column, and across a
  side-by-side split the text sits about 2px from the card's edge. View panes
  are placed inside their card (`view_bounds`). The corner radius is the
  window's (`WINDOW_RADIUS`, a guess at macOS's 16pt) minus the gap, so cards
  in the window's corners curve parallel to it.
- **Background runs and text:** same-styled cells are shaped as one run. For
  one-scalar-per-column spans, glyph positions are mapped from their UTF-8
  source indices to grid columns in a private copy of the shaped layout.
  GPUI's `force_width` counts glyphs instead, which shifts text after ligatures.
  Wide glyphs are shaped separately with their natural advance.
- **Cursor:** block cursors repaint the glyph underneath in `cursor_fg`,
  retaining its bold and italic font styling.
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
  (OSC 2) and the `WEZZY_VIEW` user var (`"<kind> <args>"`), then runs `cat`.
  Splits, zoom, swaps, moving tabs and workspaces all treat it as an ordinary
  pane, and it survives app restarts with the session.
- **Restarts lose user vars.** A reattached `ClientPane` only learns user vars
  that are set after the attach, so the placeholder never re-announces itself.
  To cover that, `cat` runs as a child rather than `exec`ed, which keeps the
  shell alive with the spec in its argv (`wezzy-view <title> <base64 spec>`).
  `session::refresh_ttys` reads it back with `views::recover_spec` and seeds the
  user var. Placeholders that `exec`ed `cat` are recovered from the title,
  if it names a configured database.
- **The app draws over it.** `Views::sync` (each frame) creates a `PaneView`
  for every pane announcing one and drops those whose pane is gone. The canvas
  skips painting their cells, and `pane_views` in `ui/terminal.rs` places each
  view absolutely over its pane's rectangle, using the grid geometry from the
  previous paint.
- **Keys:** the keymap still runs first (⌘ chords, leader, Ctrl+h/j/k/l);
  only `Lookup::Unbound` keys go to `PaneView::key` instead of the pane.
- Closing a view pane never asks for confirmation.

## Agents

- **Detection** (`agents.rs`): foreground process name/argv matches a known
  harness, or a hook record exists.
- **Status** (`hooks.rs`): each supported harness writes its latest
  lifecycle event to `~/.local/state/wezzy/agents/<WEZTERM_PANE>.json` as
  `{"harness", "pane", "ts", "payload"}`. Two wirings:
  - *Hook file* (Claude Code `~/.claude/settings.json`, Codex
    `~/.codex/hooks.json`): `assets/hooks/agent-hook.sh <harness>` is merged
    in as a command hook for each event; the payload is the harness's own
    hook input.
  - *Plugin* (pi `~/.pi/agent/extensions/wezzy.ts`, opencode
    `~/.config/opencode/plugins/wezzy.js`): our file from `assets/hooks/`
    writes the record itself, with `{"event", "cwd", "tool", "input", ...}`
    using the harness's event names. Plugin files start with
    `wezzy agent status`, which is how install, uninstall and the startup
    refresh recognize them.

  The app polls the directory, maps server pane ids to local ones and turns
  the payload into Working / NeedsInput / Done / Idle with a per-harness
  `interpret_*`, plus the tool detail and the agent's `cwd`. Every writer
  is a no-op outside Wezzy panes (`WEZZY` + `WEZTERM_PANE`). Install merges
  into existing config (other hooks untouched, a one-time `.wezzy-backup`
  before the first change) and is idempotent. Without hooks, status is
  guessed from title spinners, output and bells.
- **Subagents:** events fired inside a Claude subagent carry `agent_id`; the
  script writes them to `<WEZTERM_PANE>.<agent_id>.json` so they don't
  overwrite the main agent's status, and `HookState::subagents` collects them
  per pane. Labels and the finished flag come from Claude's undocumented
  `<session>/subagents/agent-<id>.meta.json`, read best-effort. A record goes
  on `SubagentStop`, when the meta says finished, when the main session ends
  or changes, or after 30 minutes without events. A subagent waiting on you
  makes its agent NeedsInput. `hooks::start` re-registers hooks installed by a
  build that subscribed to fewer events.
- **Permission requests:** the Notification that follows Claude's
  `PermissionRequest` overwrites the record, so the script also keeps the
  `PermissionRequest` payload in `<record>.req` and deletes it on that agent's
  next event other than a Notification: the file exists while the dialog is
  up. `HookState::request` (`hooks::Request`) carries it while NeedsInput,
  with `full()` text and an `is_risky()` heuristic.
- **Agents overlay** (⌘J, `queue.rs`, `ui/queue.rs`; the command keeps its
  old id, `AttentionQueue`): a flat `Vec<Entry>` of section headers and
  selectable rows, keyed by `queue::Key` so the selection survives rows coming
  and going. **Waiting** is NeedsInput agents, then
  unseen Done ones, each oldest first (`Agent::since`). Below it, each live
  agent's `activity::timeline`, newest first. Approve / reject send
  ↩ / esc to the pane, only for Claude and Codex approval dialogs
  (`Request::is_approval`). An answered item is hidden until the agent's
  record time (`Agent::hook_ts`) moves, since a rejection fires no hook.
- **Names** (`agents::names`): Rename agent stores a name per server pane id
  in a state file, since the mux protocol can't store data on a server pane.
  The file records the server's pid and is ignored under a new server (pane ids
  restart from 0); `session.rs` prunes ids missing from `ListPanes`.
  `Agent::display_name()` is the one way to name an agent in the UI.
- **Transitions:** state changes raise toasts, tab dots and the chime
  (`sound.rs`) when you're not looking at that pane.
- **Notifications** (`notify.rs`): with the window inactive, a transition also
  posts a `UNUserNotificationCenter` notification with identifier
  `pane-<server pane id>` (one per pane, replaced on the next transition), and
  the Dock badge counts `agents::UNSEEN`. Clicks come back through the
  delegate as `AppEvent::NotificationClicked` and go to `focus_pane`;
  `mark_seen` withdraws the pane's notification. UserNotifications throws
  without a bundle id, so `cargo run` builds skip notifications (badge only).
  Permission is asked on the first post. `notify::tests` checks every message
  signature against the runtime, since dev builds assert them at the call.

## Agent runs (`activity.rs`, `runs.rs`, `ui/run_card.rs`)

A run is an agent's turns since you last reviewed its work. `activity.rs` owns
the data; everything in it only locks a mutex and clones, so it's safe to call
from render. Git work happens on short-lived threads.

- **The log:** besides the status record, the hook script appends
  `UserPromptSubmit`, `Stop` and the `PreToolUse` of file tools (Read, Edit,
  MultiEdit, Write, NotebookEdit, apply_patch) to
  `~/.local/state/wezzy/agents/<WEZTERM_PANE>.log`, one JSON line each. It's
  append-only because the single status record loses a `PreToolUse` that's
  followed right away by its `PostToolUse`. Only the hook-file harnesses
  (Claude Code, Codex) write it; the pi and opencode plugins don't.
  `activity::poll` runs on the hooks poller's thread: it reads each log from
  its last offset (starting at the end the first time, so nothing from before
  the app started is replayed), keeps a partial last line for next time,
  empties a log once it's read past 4 MiB, and deletes logs of panes that have
  been gone for a day.
- **Turns and snapshots:** a `UserPromptSubmit` (not a subagent's) starts a
  turn and snapshots the working tree as a tree object: `git add -A` and
  `write-tree` into a temporary `GIT_INDEX_FILE` seeded from a copy of the real
  index, so the real index is never touched and unchanged files aren't
  rehashed. Untracked files are in it, ignored ones aren't. A file tool other
  than Read adds its absolute path to the turn's `files` if it's inside the
  run's repo. `Stop` marks the turn done and takes the summary: the first
  sentence of `last_assistant_message`, or of the last assistant text in
  Claude's transcript. A run lives in one repo; a prompt from another starts a
  new one. Runs are in memory only; `forget_closed_runs` (on `MuxChanged`) drops
  those of closed panes.
- **Timeline** (`activity::timeline`, for ⌘J): separate from runs, because
  runs only end when you dismiss them and most people don't. Each pane has an
  open `Line` (first prompt line, summary, stats, times) with its own base
  snapshot, files and starting HEAD. A prompt ends it (into `History::lines`,
  up to 100) when HEAD moved, the agent has been done for `BREAK` (15 min), or
  the repo changed; dismissing the run ends it too. Its stats come from the
  same `snapshot_files` + `numstat` as a run's, after each `Stop` and undo.
  The pane's history carries the agent's `session_id`; a prompt with another
  one (a restarted agent, `/clear`) clears it. Saved to
  `<WEZTERM_PANE>.runs.json` beside the log on every change, loaded when
  `poll` first sees the pane's log, deleted with the log.
- **Stats:** the run's first snapshot against that snapshot with only the
  agent's files taken from the working tree (`read-tree` + `add -A -- <files>`
  in a temp index), through `git diff --numstat`. Your own edits elsewhere
  don't count, and files changed by shell commands aren't in `files`. Stats are
  recomputed at most every 3s while the agent edits, and 300ms after `Stop`
  (the last edit's `PreToolUse` fires before the write).
- **Undo:** `restore` puts each file back from a snapshot tree
  (`git restore --source=<tree> --worktree`), or deletes it when the tree
  doesn't have it (the agent created it). `undo_turn` uses the last turn's
  snapshot and files and drops the turn; `undo_run` uses the first snapshot and
  every file and ends the run. `runs::undo` calls them on a thread and reports
  through `AppEvent::Flash`. The confirm modal performs
  `EmitEvent("wezzy-run.undo-turn" | "wezzy-run.undo-run")`, which `perform`
  routes to `runs::undo` (not app commands, since those ask first).
- **The card** (`ui/run_card.rs`: a slim bar with +/−, file count, Review
  changes and ×, docked to the bottom edge of each pane of the active tab,
  drawn after `pane_views`) shows while the run is `done && has_changes()`. Typing into
  the pane records the run's `finished_at` in `TermApp::run_card_hidden`, so
  the card hides until the next `Stop` changes it. `activity::dismiss` ends the
  run.
- **Git panel:** `GitPanel::run_filter` limits `rows()` (and section ops, `S`,
  `U`, the counts) to files whose `snap.root.join(path)` is in the run's
  `files()`. `A` (`toggle_run_filter`) cycles through `run_panes_here()`: the
  runs in the panel's repo, the active tab's panes first in layout order, then
  back to unfiltered. `GitPanel::land` puts the cursor on the first file once the
  snapshot of the run's repo arrives (`land_git_cursor`, from render). A commit
  submitted with the filter on, where everything staged is the run's and none
  of its files are left unstaged, sets `committing_run`; `on_committed` ends
  that run. `F` targets the run whose `touched()` covers every commented file
  (`run_for_comments`).

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

## Git panel (`diff.rs`, `gitpanel.rs`, `ui/git.rs`, `feedback.rs`, `pr.rs`)

- **Target:** `gitpanel::git_dir` picks the directory: the `cwd` from the
  pane's latest hook payload (an agent that moved into a worktree), unless a
  shell is back in the foreground, else the pane's cwd.
- **Polling:** a background poller snapshots `git status` plus parsed hunks,
  branch and upstream ahead/behind for that repo every 2s, with
  `GIT_OPTIONAL_LOCKS=0` so it never takes `index.lock`.
- **Pull requests** (`pr.rs`): each snapshot calls `pr::poll`, which runs
  `gh pr view` on its own thread when the (repo, branch) entry is missing,
  older than 60s, or the upstream sha moved; failures (no `gh`, no auth, no
  PR) cache as "no PR". `N` / the palette command run `pr::begin` off the
  main thread (refresh, checks, default branch from `<remote>/HEAD`) and
  answer with `AppEvent::PullRequest`; creating pushes, then `gh pr create
  --base <default>` with `GH_PROMPT_DISABLED=1`.
- **Mutations:** staging and discarding go through `git apply [--cached] [-R]`
  on single-hunk patches.
- **Pull / push** (`diff::sync`): `git pull -q` / `git push -q` on their own
  thread (a first push sets the upstream on `pr::remote_for`'s remote), then
  `AppEvent::Synced`. `GIT_TERMINAL_PROMPT=0`, so missing credentials fail
  with git's message instead of hanging.
- **Review comments:** comments on hunks or lines are sent to an agent as one
  review message with `file:line` context.
- **Text boxes** (`textfield.rs`, `ui/textfield.rs`): the commit / PR composer
  and the comment box share one `TextField` (text, cursor and anchor as byte
  offsets, undo snapshots) and one custom gpui `Element` that shapes the text
  with `shape_text` (wrapped), paints the selection and caret, and hit-tests
  the mouse into offsets. Keys still arrive through `region_key` →
  `composer_key` / `comment_key` → `TextField::key`, with typing from
  `key_char`; there's no `EntityInputHandler`, so no IME composition. Each
  paint leaves the visual rows in the field's `Shared` cell, which ↑/↓ use
  to move by wrapped row (logical lines until the first paint). The ⌘ keys
  the field edits with are kept from the keymap in `app.rs` while a box is
  open. The other single-line inputs (palette, prompts, sidebar rename)
  still edit their own strings.
- **Live layer** (`gitlive.rs`): `diff::publish` compares each snapshot with
  the previous one (`diff::freshness`), keying hunks by path + body hash so
  staging or shifted line numbers don't count as new; the first snapshot, a
  different repo, or a gap over 6s marks nothing. File markers come from
  `activity::touches`. Fades are computed from timestamps at render time, and
  `git_tick` runs a 50ms repaint loop only while something is still fading.
  Follow mode (`w`) runs on every `AppEvent::Redraw`: it remembers the file of
  the latest Edit touch until the snapshot has it, preferring a hunk that
  turned fresh after the edit.

## Persistence

| What | Where |
|---|---|
| Shells, agents, layout, scrollback | the session server (lives until killed) |
| Active workspace | `~/.local/share/wezzy/last-workspace` |
| App settings | `~/.config/wezzy/settings.json` |
| Agent status | `~/.local/state/wezzy/agents/` |
| Agent names | `~/.local/state/wezzy/agent-names.json` (keyed by server pane id + server pid) |
