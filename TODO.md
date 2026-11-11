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

## Later

- PR review comments into the feedback queue (`F`)
- Cost / context usage per agent
- Session timeline, per-turn checkpoints
- Recipe pipelines, best-of-N
- Wezzy MCP server for agents
