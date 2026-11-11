# TODO

Managed by Claude (the manager); each item is delegated to a sub-agent working
on its own branch in a worktree, then reviewed and merged back here.

| # | Item | Status | Branch / agent |
|---|---|---|---|
| 1 | Git panel follows the agent's actual work (its real cwd, e.g. a worktree it moved into) | In progress | git agent |
| 2 | Create a PR from the git panel (push + `gh pr create`) | In progress | git agent |
| 3 | Branch / PR info shown (branch, ahead/behind, PR number + state) | In progress | git agent |
| 4 | Status hooks for more harnesses (Codex, pi, opencode, Gemini, …) | In progress | hooks agent |
| 5 | System notifications when an agent needs you and Wezzy isn't focused | In progress | notify agent |
| 6 | Rename agents | In progress | rename agent |
| 7 | Agent overview: its own full view | Approved (docs/roadmap/agent-overview.md); phase 1 starts after 1–6 merge | |
| 8 | Task inbox: agents that pick up work without being managed | Parked: needs design | |

## Later

- PR review comments into the feedback queue (`F`)
- Cost / context usage per agent
- Session timeline, per-turn checkpoints
- Recipe pipelines, best-of-N
- Wezzy MCP server for agents
