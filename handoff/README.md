# Handoff

A board shared by you and your coding agents. An agent files what it needs
from you; you mark it done, answer, and queue its next work; the agent is told
what changed.

It's one SQLite file (`~/.local/share/handoff/handoff.db`, or `$HANDOFF_DB`),
a library, and the `handoff` command. Wezzy is one frontend for it
(`docs/features/board.md`); nothing here depends on Wezzy.

```sh
cargo install --path .
handoff                     # the board
handoff add "Rotate the staging key"
handoff done 3 -m "used the sk_test one"
handoff reply 4 "wait until Friday"
handoff updates             # what the other side did since you last asked
handoff --help
```

Everything goes to one board unless you name another (`-p`), and a card is
tagged with the git repository it was made from. A lane has a
role: `you` (waiting on you), `agent`, `done` or `none`. A card added without
`--lane` goes to the other side: yours to the agent's queue, an agent's in
front of you. Inside an agent's shell the command acts as the agent; `--as`
says otherwise.

## Agents

Claude Code, by hand (Wezzy's **Install Handoff** does the same):

```sh
claude mcp add --scope user handoff -- handoff mcp
```

and in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [{ "hooks": [{ "type": "command", "command": "handoff hook session-start" }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "handoff hook stop" }] }]
  }
}
```

`handoff mcp` is a stdio MCP server: `get_board`, `get_updates`,
`create_cards`, `update_card`, `complete_card`, `reply_to_card`, lanes and
projects. The hooks show an agent its queue when a session starts, and keep it
going at a stop when you've changed the board since it last looked.

An agent is identified by `$HANDOFF_AGENT`, or by the terminal pane it runs in
(`$WEZTERM_PANE`, `$TMUX_PANE`). A card belongs to the agent that filed it,
took it or was given it, and each agent is told only about its own cards.
