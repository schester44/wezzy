# Dekita: notes for agents

A gpui (Zed's UI framework) app wrapped around WezTerm's engine crates. WezTerm
provides config, the mux (windows/tabs/panes/workspaces), PTYs and terminal
emulation. Dekita provides rendering, input, and its own UI: sidebar, top bar,
palette, git panel, settings, agent tracking.

Read `docs/architecture.md` before structural changes and `docs/design.md`
before any UI work.

## Build and verify

```sh
cargo build      # must stay warning-free
cargo test
```

- Don't launch the GUI or kill processes to test. Build, then let the user run
  it. If you're running inside Dekita, the user restarts with ⌘K → Rebuild & Restart.
- Never kill the session server (`~/.local/share/dekita/pid`): it holds every
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
| `ui/git.rs`, `gitpanel.rs`, `diff.rs`, `feedback.rs` | git panel view, its state/keys, git polling + staging, hunk comments |
| `ui/mod.rs` | `Theme` (derived from the terminal palette), icons |
| `render.rs` | paints panes/cells/cursor into a gpui canvas |
| `keys.rs` | gpui keystroke → WezTerm key, leader + key tables, default bindings |
| `luawin.rs` | Lua `window` object for `action_callback` / `wezterm.on` handlers |
| `commands.rs` | app commands (`AppCommand`), exposed to Lua as `require("dekita").action.*` |
| `palette.rs`, `settings_ui.rs`, `regions.rs` | command palette, settings overlay, keyboard focus in side panels |
| `session.rs` | attach to the session server (`wezterm-mux-server`), pane id mapping |
| `hooks.rs`, `agents.rs`, `procs.rs` | agent hook install/polling, agent detection + status, foreground process lookup |
| `launch.rs`, `recipes.rs` | harness launcher (⌘N), recipes |
| `servers.rs`, `sendout.rs`, `quickselect.rs`, `clipboard.rs` | dev servers, send output to agent, hints + ⌘-click, paste |
| `settings.rs`, `paths.rs`, `sound.rs`, `restart.rs` | settings.json + Lua `setup()`, file locations, chime, restart |
| `vendor/wezterm-client` | patched copy; see its `Cargo.toml` for the diff |
| `config/dekita.lua` | default config written on first run (embedded) |
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
  the app has the `dekita` Lua module, so config code must `pcall(require, "dekita")`.
  Session-server settings (socket, pid, log) are passed on its command line in
  `session.rs`, not in the config.
- **Keys:** user bindings win over `default_bindings()` (`or_insert`). App
  commands are `EmitEvent("dekita.<id>")` under the hood, intercepted in `perform()`.
- **Settings:** `settings.json` is the base and the Lua `setup()` overrides it.
  Change settings from the UI with `settings::update`, which persists.
- **Builds from inside Dekita:** if the app was started with `cargo run`, its
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
- Follow `docs/design.md` for selection, rows, borders and spacing.

## Adding things

- **An app command:** add a variant + `id()` + `lua_name()` in `commands.rs`,
  handle it in `TermApp::run_app_command` (`palette.rs`), list it in
  `commands()` in `palette.rs`, and add a default key in `keys.rs` if it deserves one.
- **An icon:** a 24×24 stroke SVG in `assets/icons/`, then add it to the
  `icons!` list and `Icon` enum in `ui/mod.rs`.
- **A setting:** field + default in `Settings`, `Stored` + load/save in
  `settings.rs`, a `Row` in `settings_ui.rs`.
- **A palette source:** a `Kind` and items in `palette_items()`; ranking,
  sections and keys come for free.

Commit messages are short and imperative. Don't commit unless asked.
