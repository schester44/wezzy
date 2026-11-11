# Wezzy: notes for agents

A gpui (Zed's UI framework) app wrapped around WezTerm's engine crates. WezTerm
provides config, the mux (windows/tabs/panes/workspaces), PTYs and terminal
emulation. Wezzy provides rendering, input, and its own UI: sidebar, top bar,
palette, git panel, settings, agent tracking.

Read `docs/development/architecture.md` before structural changes and `docs/development/design.md`
before any UI work.

## Build and verify

```sh
cargo build      # must stay warning-free
cargo test
```

- Don't launch the GUI or kill processes to test. Build, then let the user run
  it. If you're running inside Wezzy, the user restarts with ⌘K → Rebuild & Restart.
- Never kill the session server (`~/.local/share/wezzy/pid`): it holds every
  shell and agent, including the one you're running in.
- WezTerm crates are git deps pinned to one commit (`e68a8ec`, matching the
  bundled `wezterm-mux-server`). Bump them all together or not at all; the mux
  protocol must match the server.

## Where things are

| | |
|---|---|
| `main.rs` | startup: env, settings, config, mux + session domain, gpui app |
| `app.rs` | `TermApp` state, event plumbing (`AppEvent`), key routing, `perform()` (the KeyAssignment interpreter), spawning/splitting |
| `ui/view.rs` | the main view: top bar, tabs, sidebar, terminal canvas, overlays |
| `ui/git.rs`, `gitpanel.rs`, `diff.rs`, `feedback.rs`, `pr.rs` | git panel view, its state/keys, git polling + staging, hunk comments, branch PR lookup + creation via `gh` |
| `ui/mod.rs` | `Theme` (derived from the terminal palette), icons |
| `render.rs` | paints panes/cells/cursor into a gpui canvas |
| `keys.rs` | gpui keystroke → WezTerm key, leader + key tables, default bindings |
| `copymode.rs` | copy mode (Leader y): vim motions over scrollback, selection, search, copy |
| `luawin.rs` | Lua `window` object for `action_callback` / `wezterm.on` handlers |
| `commands.rs` | app commands (`AppCommand`), exposed to Lua as `require("wezzy").action.*` |
| `palette.rs`, `settings_ui.rs`, `regions.rs` | command palette, settings overlay, keyboard focus in side panels |
| `session.rs` | attach to the session server (`wezterm-mux-server`), pane id mapping |
| `hooks.rs`, `agents.rs`, `procs.rs` | agent hook install/polling, agent detection + status, foreground process lookup |
| `launch.rs`, `recipes.rs` | harness launcher (⌘N), recipes |
| `hosts.rs` | SSH hosts (⌘⇧S / "ssh …" in ⌘K): config + `~/.ssh/config` aliases |
| `workspaces.rs` | saved workspaces (name + cwd), the running + saved list behind the picker and sidebar menu |
| `sidebar.rs`, `actions.rs` | sidebar sources (agents, servers, actions): which show, their items, ↵ / `o`; GitHub Actions runs via `gh` |
| `servers.rs`, `sendout.rs`, `quickselect.rs`, `clipboard.rs` | dev servers, send output to agent, hints + ⌘-click, paste |
| `paneselect.rs` | PaneSelect (Leader m): pane labels, swaps done on the server |
| `settings.rs`, `paths.rs`, `sound.rs`, `restart.rs` | settings.json + Lua `setup()`, file locations, chime, restart |
| `update.rs` | automatic updates: polls the release feed, verifies and swaps in the new bundle |
| `scripts/bundle.sh`, `scripts/release.sh` | build Wezzy.app; cut a signed, notarized draft release (`docs/development/releasing.md`) |
| `vendor/wezterm-client` | patched copy; see its `Cargo.toml` for the diff |
| `config/wezzy.lua` | default config written on first run (embedded) |
| `assets/` | icons (SVG, embedded), app icon, agent hook script |

## Rules that bite

- **Main thread.** The mux and Lua run on the main thread. WezTerm schedules its
  futures through `promise::spawn`, which `main.rs` routes to the GCD main queue
  (gpui owns the run loop). From background threads, talk to the UI with
  `app::send_event(AppEvent::…)`; never touch gpui state off the main thread.
- **Mux notifications** (`Mux::subscribe` in `TermApp::new`) only enqueue
  events. Pane output is coalesced (`MUX_DIRTY`) to avoid a redraw storm.
- **Panes are usually remote.** In normal mode every pane is a `ClientPane`
  owned by the session server. Things that work on a `LocalPane` may not: no
  foreground process info (use `procs::foreground`), no semantic zones (read
  cell `semantic_type` instead), user vars only after attach (`seed_user_var`).
  `session::local_pane_id` / `remote_pane_id` convert between our ids and
  `$WEZTERM_PANE`.
- **The config is evaluated twice:** by the app and by the session server. Only
  the app has the `wezzy` Lua module, so config code must `pcall(require, "wezzy")`.
  Session-server settings (socket, pid, log) are passed on its command line in
  `session.rs`, not in the config.
- **Keys:** user bindings win over `default_bindings()` (`or_insert`). App
  commands are `EmitEvent("wezzy.<id>")` under the hood, intercepted in `perform()`.
- **Settings:** `settings.json` is the base and the Lua `setup()` overrides it.
  Change settings from the UI with `settings::update`, which persists.
- **Builds from inside Wezzy:** if the app was started with `cargo run`, its
  environment carries cargo's `CARGO_PKG_*` vars, which makes cargo rebuild
  `ring`/`gpui` when builds alternate with a normal terminal.

## UI conventions

- Minimal copy. No hint rows, explanatory labels, placeholder sentences or
  status words like "Idle" or "Your turn". Icons, dots and highlights carry state;
  text only when it's real information (Claude's permission message, the tool
  it's running). Toasts are terse.
- Colors come from `Theme`, which is derived from the terminal palette. Don't
  hardcode colors. Accent/success/warning/danger mean something; never decorate with them.
- Vim keys in every panel (`j/k/g/G`, ↵, esc). Ctrl+h/j/k/l moves between
  panes and into panels; don't steal it from nvim (`procs::is_vim`).
- Follow `docs/development/design.md` for selection, rows, borders and spacing.

## Adding things

- **An app command:** add a variant + `id()` + `lua_name()` in `commands.rs`,
  handle it in `TermApp::run_app_command` (`palette.rs`), list it in
  `commands()` in `palette.rs`, and add a default key in `keys.rs` if it deserves one.
- **An icon:** a 24×24 stroke SVG in `assets/icons/`, then add it to the
  `icons!` list and `Icon` enum in `ui/mod.rs`.
- **A setting:** field + default in `Settings`, `Stored` + load/save in
  `settings.rs`, a `Row` in `settings_ui.rs`.
- **A sidebar source:** a `Source` + `Item` variant in `sidebar.rs`, its items
  and keys there, a row in `ui/view.rs`; its poller idles unless
  `sidebar_enabled`.
- **A palette source:** a `Kind` and items in `palette_items()`; ranking,
  sections and keys come for free.

## Documentation

Docs are part of the work, not a follow-up. Each kind has its own folder,
reader and voice:

| | For | Voice |
|---|---|---|
| `docs/features/` | people using Wezzy | a landing page for the feature: what you can do with it and how. No internals, file names or code. |
| `docs/development/` | people building Wezzy | how it works and why: architecture, design rules, conventions. File and function names welcome. |
| `docs/roadmap/` | planning | work that isn't built yet. Move what ships into `features/`. |
| `docs/assets/` | the repo | images for the README and docs. |

`docs/README.md` indexes all of them.

**When to write or update them:**

- **New major feature** (a panel, a palette source, a config section, a new
  kind of thing users can do): write `docs/features/<feature>.md` in the same
  change and add it to the index.
- **Changing or refactoring a feature:** update its feature doc in the same
  change, and `docs/development/` if the change makes it wrong. If a feature is
  removed, remove its doc and index entry.
- **Small changes** (a new key for an existing command, a new setting) go in the
  existing doc for that feature, not a new file.
- **Before finishing,** check every claim against the code: key bindings in
  `keys.rs`, config fields in `settings.rs`, behavior in the implementation.
  Don't document what you haven't verified.

**Feature docs** (`docs/features/workspaces.md` is the model):

1. **Opening.** Two or three sentences on what it does for you.
2. **Using it.** Keys, palette commands, clicks, in a table when there are several.
3. **Setting it up.** Lua `setup()` and `settings.json` examples that work as
   written, and where the file lives.
4. **Details.** What happens in each case and the limits, in terms of what the
   user sees ("the pane stays open with a local shell"), not how it's built.

Write to the reader ("you"), in plain words. Name things as the UI names them.
Don't write "now", "new" or change history: the doc describes the app as it is.

**Development docs** explain mechanisms and constraints that span features.
Code maps and internals belong here or in module comments, not in feature docs.

Commit messages are short and imperative. Don't commit unless asked.
