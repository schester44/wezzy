# Agent overview: what's next

The overview's grid of live cards, header counts, card keys (`y/Y/n` for
Claude, `r`, ⌃C), ⌘O / ⌥⌘0 and the palette command are built: see
[the feature doc](../features/overview.md). This page covers what isn't.

## Decisions (still hold)

- Cards are grouped by workspace in a stable order (workspace order, then
  pane id), the same order as the sidebar and ⌥⌘1–9. Cards never reorder
  by status. Triage comes from icons, header counts and Tab.
- `y` (allow) acts immediately, only when the hook reports a real permission
  prompt, with a terse toast ("Allowed · parser-fix"). No confirm step.
- The sidebar's agent list stays; the overview complements it.

## Phase 2

**Stage (Space):** the selected agent fills the stage, live, scaled to fit
(never above 1.0); the others become a strip on the left styled like the
sidebar's `agent_row`. j/k moves the strip and the stage follows. With one
agent the overview opens straight into the stage (today it shows one card
filling the window). Wheel in the stage scrolls the pane's scrollback via
`viewports`.

Seen: dwelling ~1s on an agent in the stage marks it seen; the grid doesn't.

**Diff +/−** on card line 1, after the branch (`success`/`danger`, mono):
`git diff --shortstat HEAD` (`GIT_OPTIONAL_LOCKS=0`) once per distinct agent
repo root every 5s, only while the overview is open. The branch itself is
already shown, read from the repo's `HEAD` file.

| Key | Action |
|---|---|
| Space | Toggle the stage |
| `d` | Jump in and open the git panel |
| `F` | Send the git panel's comment queue to this agent (`send_feedback_to`) |
| `x` | Close the agent's pane, inline confirm |
| `?` | Overview shortcut sheet |

**Time in state:** shown, but `agents::since` starts counting when the app
first sees an agent, so after a restart every agent reads "since launch".
Seed it from the hook record's `ts` instead.

## Phase 3

- Per-harness crop anchors (Claude's input box vs. a plain bottom anchor) and
  approve maps for Codex, pi and OpenCode (`overview::keymap`), once their
  hooks tell us a prompt is a permission prompt (`HookState::permission`).
- Typing into the stage.
- `/` filter.
- Workspace folding (click a workspace label).
- Compact density at 20+ agents.

## Notes for the build

- The stage scales the pane's own lines to fit; still never resize panes.
  `render::bottom_snapshot` + `paint_pane` with a content mask is how cards
  paint today.
- Cost: 6–20 cards × ~12 rows is about one full-screen terminal's worth of
  shaped lines per frame. Any visible spinner keeps gpui animating, so the
  overview repaints every frame while an agent works. If that hurts, cap
  overview redraws at ~20fps.
