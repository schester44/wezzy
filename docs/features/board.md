# Board

One board you share with all your agents. An agent that needs something from
you (an answer, a decision, a thing only you can do) files a card instead of
only saying so; you tick it off or reply, and that agent hears about it. You
give agents work the same way. Press ⌘⇧J from anywhere.

Every agent files to the same board, whatever repository it's in, and each
card carries the repository it came from and the agent it belongs to.

Lanes say whose move a card is. The board starts with four: **Needs you**,
**Agent queue**, **In progress** and **Done**. The icon beside a lane's name is
its role: you, the agent, or done.

While any card needs you, the top bar shows an inbox icon with the count, on
the right. Hover it to see where they came from, repository by repository;
click it to open the board they're on. A new one chimes, with the sound you
set for agents.

## Using it

| | |
|---|---|
| ⌘⇧J | Open the board (again to close it) |
| `h` `l`, `j` `k`, `g` `G` | Move between lanes and cards |
| `c` | New card in this lane. The first line is its title, the rest its body (⇧↩ for a new line) |
| ↩ | Open the card: its body and thread |
| `r` | Reply |
| `x` or space | Done |
| `e`, `b` | Edit the title; with the card open, `b` edits the body |
| `H` `L` | Move the card a lane left or right |
| `J` `K` | Move the card down or up its lane |
| `a` | Give the card to an agent: pick from the running agents with `j` `k` ↩ or its number |
| `d` | Delete the card (asks once) |
| `[` `]` | Another board, when you've made more than one |
| `N`, `R` | New lane, rename this lane |
| `T` | Change this lane's role: you, agent, none, done |
| `<` `>` | Move this lane left or right |
| `X` | Delete this lane, once it's empty |
| `?` | Every key |
| esc | Back, or close |

With the mouse: drag a card onto a lane or between two cards, click the circle
to mark it done, click a card twice to open it.

## Setting it up

The board works as soon as you open it. To have agents use it, run
**Install Handoff** from ⌘K (or turn on Handoff in Settings, ⌘,). That:

- puts the `handoff` command on the PATH of every pane,
- registers its MCP server with Claude Code, so the agent has tools to file,
  read, move and complete cards,
- adds two Claude Code hooks: at the start of a session the agent is shown its
  queue and what you changed, and when it's about to stop, anything you've
  answered or handed it since it last looked keeps it going.

**Uninstall Handoff** takes all three out again. Your boards stay.

To move the key, bind `wezzy.action.Board` in `wezzy.lua`.

## Details

- **One board.** Everything goes to the board called Board. Other boards
  still exist for when you want one (`handoff project add`, `handoff -p`),
  and `[` `]` move between the ones with cards on them.
- **Tags.** A card is tagged with the git repository it was made from (a
  worktree counts as its main repository): the agent's, or the pane you
  opened the board from.
- **Where a new card goes.** With `c`, into the lane you're in. A card an
  agent files without naming a lane lands in **Needs you**.
- **Done.** Moving a card into a lane with the done role finishes it; moving
  it out reopens it. `x` sends it to the first such lane.
- **Whose card it is.** A card belongs to one agent: the one that filed it,
  the one you gave it to, or the one that took it. Its name shows on the card.
  "The agent" is the agent in that pane, also after its session restarts there.
- **Giving work to an agent.** Add a card to an agent's lane. Opened from an
  agent's pane, the board gives it to that agent; opened from anywhere else,
  it asks which agent. `a` changes that at any time. A card that's nobody's
  waits: no agent is told about it.
- **What an agent hears.** Only about its own cards: your replies, cards you
  completed, moved, edited or assigned to it, once each. Its queue is its own
  cards too. With ten agents running, each hears about its own work.
- **When it hears.** When you close the board, what you did is typed into the
  session of the agent it's for, if that agent is idle. A working agent gets
  it when it finishes its turn instead, and what's waiting is shown when a
  session starts.
- **A closed pane.** Its agent's cards go back to belonging to nobody, until
  you give them to another agent.
- **Live.** The board redraws when an agent changes it while you look.
- **Only Claude Code is wired up** by Install Handoff. Any agent can still run
  the `handoff` command (`handoff --help`), and any MCP client can start
  `handoff mcp`.
- **It's local.** The board is one file on this Mac,
  `~/.local/share/handoff/handoff.db`; agents on a remote host don't reach it.
- **Without Wezzy.** Handoff doesn't need the app: the same `handoff` command
  builds on its own from `handoff/` in the repository and reads the same file.
  Assigning a card to an agent is only in the app.
