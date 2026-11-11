# Agent overview

A full view for managing the whole fleet. The sidebar tells you *that* an
agent needs you; the overview is *where you handle it*. One key replaces the
window body with a grid of live cards, one per agent across every workspace,
each showing the real bottom of that agent's terminal. You approve, deny,
reply or interrupt from the card without switching workspaces or moving
focus, then esc back to exactly where you were.

## Decisions

- Cards are grouped by workspace in a stable order (workspace order, then
  pane id), the same order as the sidebar and Leader 1–9. Cards never reorder
  by status. Triage comes from icons, header counts and Tab.
- `y` (allow) acts immediately, only when the hook reports a real permission
  prompt, with a terse toast ("Allowed · parser-fix"). No confirm step.
- The sidebar's agent list stays; the overview complements it.

## Layout

```
│ Agents 6                                                 ⚠ 2   ◌ 3   ● 1 │
│ wezzy 4                                                                  │
│▌⚠ 1 parser-fix  Claude  fix/parser  +42 −7  3m ┆ ◌ 2 sidebar  Claude  main │
│▌ Bash wants to run: cargo test --all           ┆   Editing view.rs         │
│▌╭ Bash command ────────────────────────╮       ┆  ⏺ Update(src/ui/view.rs) │
│▌│ ❯ 1. Yes                             │       ┆  ✻ Thinking…              │
│▌│   2. Yes, and don't ask again        │       ┆                           │
│▌╰──────────────────────────────────────╯       ┆                           │
│▌               Allow y   Always Y   Deny n     ┆                           │
│ api 2                                                                    │
│ ⚠ 5 migrate  Claude  main  40s  Which database should I target?          │
```

Card: line 1 is status icon, Leader number (mono, `fg_subtle`), name (`fg`,
medium), harness (`fg_subtle`), branch, `+N −M` (success/danger, mono), time
in the current state. Line 2 is the hook detail (tool or permission message),
only when there is one. Then 10–14 rows of the real pane, cropped, not
scaled. Allow / Always / Deny buttons only on a card waiting on a permission
prompt. 1px hairlines between cards, no tinted backgrounds. Selected card:
faint fill plus a 2px accent bar on the left.

**Stage (Space):** the selected agent fills the stage, live, scaled to fit
(never above 1.0); the others become a strip on the left styled like the
sidebar's `agent_row`. j/k moves the strip and the stage follows. With one
agent the overview opens straight into the stage.

**Many agents:** fixed card height, the grid scrolls; above 12 agents tails
shrink to 6 rows.

**Empty:** bot icon in a 32px hairline circle, "No agents" in `fg_muted`, one
`New agent ⌘A` button.

## Keys

| Key | Action |
|---|---|
| ⌘O, Leader 0, palette "Agent overview" (`agent-overview`) | Open / close |
| esc, `q` | Close, back where you were |
| Leader 1–9 | Jump to that agent and close the overview |
| `h/j/k/l`, arrows | Move between cards |
| `g` / `G` | First / last |
| `1`–`9` | Select card N |
| Tab / Shift+Tab | Next / previous agent that wants you: needs input, then unseen done |
| ↵ | Jump in: close and focus that pane |
| Space | Toggle the stage |
| `y` / `Y` / `n` | Allow / always allow / deny (only with a per-harness key map; otherwise a short toast) |
| `r` | Inline reply under the card: enter pastes and submits, shift+enter newline, esc cancels |
| ⌃C | Interrupt (Claude gets Esc, others ⌃C) |
| `d` | Jump in and open the git panel |
| `F` | Send the git panel's comment queue to this agent (`send_feedback_to`) |
| `x` | Close the agent's pane, inline confirm |
| `?` | Shortcut sheet |

Mouse: click selects, double-click or click the name jumps in, buttons work
like `y` / `n`, header counts cycle like Tab, workspace labels fold, wheel
scrolls the grid (in the stage, the pane's scrollback via `viewports`).

Seen: dwelling ~1s on an agent in the stage marks it seen; the grid doesn't.

## How it's built

- **State:** `TermApp.overview: Option<OverviewState>`, a window-level mode
  that replaces the sidebar/terminal row in `ui/view.rs::render` the way
  `zoomed` swaps layout. Not an `Overlay` (those are modal) and not a pane
  view (those belong to one workspace). New `src/overview.rs` (state, keys,
  actions) and `src/ui/overview.rs` (view); one branch in `on_key` after
  overlays, before regions.
- **Agents:** `agents::collect()` as is; add `since: Instant` from a static
  pane → (Status, Instant) map updated on status change; sort by (workspace
  order, pane id).
- **Liveness:** `PaneOutput` already triggers coalesced redraws for any pane;
  `ClientPane::get_lines` fetches rows on demand.
- **Diff +/−:** `git diff --shortstat HEAD` (`GIT_OPTIONAL_LOCKS=0`) once per
  distinct agent repo root every 5s, only while the overview is open.
- **Never resize panes.** Cards paint the pane's own lines at real columns
  with `render::metrics(.., font_scale * 0.75)`, anchored on the bottom
  non-blank rows, clipped at the card edge. The stage scales to fit. While
  the overview is open `terminal_canvas` isn't painted, so nothing resizes
  behind your back.
- **render.rs:** snapshot a bare `Arc<dyn Pane>` (origin 0, size from
  `get_dimensions()`, plus a row window); give `paint_pane` a clip bounds and
  row offset; `focused = false` for hollow cursors.
- **Acting:** `pane.key_down` from a per-harness table (Claude: allow = Enter,
  always = "2", deny = Esc, interrupt = Esc); reply = `send_paste` + Enter as
  in `feedback.rs`. `y/Y/n` only when NeedsInput and the hook says it's a
  permission prompt; questions get `r`.
- **Cost:** 6–20 cards × ~12 rows is about one full-screen terminal's worth of
  shaped lines. If redraw frequency hurts, cap overview redraws at ~20fps.

## Phases

1. Grid of cropped live cards grouped by workspace, header counts;
   hjkl / g / G / 1–9 / Tab / ↵; `y/Y/n` for Claude, `r`, ⌃C; ⌘O, Leader 0,
   palette command; empty state; `docs/features/overview.md`.
2. Stage + strip (scaled pane, scrollback wheel, dwell-to-seen); diff +/−
   poller and branch; `d`, `F`, `x`; time in state.
3. Per-harness crop anchors and approve maps; typing into the stage; `/`
   filter; workspace folding; compact density at 20+ agents.
