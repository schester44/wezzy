# TODO

Managed by Claude (the manager); each item is delegated to a sub-agent working
on its own branch in a worktree, then reviewed and merged back here.

| # | Item | Status | Branch / agent |
|---|---|---|---|
| 1 | Git panel follows the agent's actual work (its real cwd, e.g. a worktree it moved into) | Merged to main; untested in the app | git agent |
| 2 | Create a PR from the git panel (push + `gh pr create`) | Merged to main; untested against GitHub | git agent |
| 3 | Branch / PR info shown (branch, ahead/behind, PR number + state) | Merged to main | git agent |
| 4 | Status hooks for more harnesses (Codex, pi, opencode, Gemini, …) | Merged to main: Codex, pi, OpenCode (Gemini, Amp, Cursor, Aider, Goose skipped) | hooks agent |
| 5 | System notifications when an agent needs you and Wezzy isn't focused | Merged to main; test in the bundled app | notify agent |
| 6 | Rename agents | Merged to main | rename agent |
| 7 | Task inbox: agents that pick up work without being managed | Parked: needs design | |
| 8 | SQL: schema browser (tables, columns, keys, indexes; ↵ opens a table) | Merged to main; catalog queries untested against a database | sql-schema agent |
| 9 | SQL: copy as CSV / JSON / Markdown / INSERT | Merged to main; untested in the app | sql-copy agent |
| 10 | SQL: format the query (⌘⇧F) | Merged to main; untested in the app | sql-format agent |
| 11 | SQL: readable `explain` (plan tree) | Merged to main; untested against a database | sql-explain agent |
| 12 | Agent activity foundation: hook activity log, per-turn snapshots, runs, stats, undo (`activity.rs`) | Done on main | manager |
| 13 | Run diffstat on sidebar agent rows | Done on main | manager |
| 14 | Run card over the agent pane after it stops: summary, diffstat, Review / Undo turn / Ask agent / Commit & PR; git panel run filter | Merged to main; untested in the app | run-card agent |
| 15 | Live agent markers in the git panel: touched-file markers with fade, fresh-hunk glow, follow mode | Merged to main; untested in the app | live-markers agent |

## Later

- SQL panes agents can drive: an agent opens (or reuses) a SQL pane with its query, you see the result there
- PR review comments into the feedback queue (`F`)
- Cost / context usage per agent
- Session timeline
- Recipe pipelines, best-of-N
- Wezzy MCP server for agents
