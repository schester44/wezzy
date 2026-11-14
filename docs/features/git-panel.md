# Git panel

Review what your agents changed without leaving the terminal. The git panel
shows the changes in the repo you're working in, lets you stage, discard and
commit them hunk by hunk, and sends your line comments back to the agent as
one review.

When the branch is ready, push it and open a pull request from the same place.

## Opening it

| | |
|---|---|
| ⌘G | Open or close the git panel; it opens on an agent's run when there's one to review (see below) |
| ⌃h | Move into the panel from the leftmost pane |
| esc or ⌃l | Back to the terminal |
| q | Close the panel |
| ⌘K → **Create Pull Request** | Open the panel and start a pull request |

The panel takes the sidebar's place on the left while it's open. With pane
cards on, the panel lights up like the active pane while it has focus, and the
panes all look inactive, so you can tell its keys (`?` lists them) are live.

## What it shows

The panel follows the pane you're in. When that pane runs an agent with
Wezzy's hooks installed (⌘, → Agent hooks), it shows the repo the agent is
working in, even if the agent moved into another folder or a git worktree.
Otherwise it shows the repo of the pane's current folder.

The header has the repo, the branch and the last commit, plus:

- **Ahead / behind:** `↑2 ↓1` when the branch is ahead of or behind its
  upstream. Nothing when they're in step.
- **No upstream:** a crossed-out cloud when the branch hasn't been pushed yet.
- **Pull request:** the branch's pull request number with an icon for its
  state: open, draft, merged or closed. Click it to open the PR in your browser.
- **+/− counts** for everything that's changed.

Below that, changes are grouped into **Untracked**, **Unstaged** and **Staged**.
Expand a file to see its hunks.

## Watching the agent work

While an agent works, the panel shows where its attention is:

- **Markers:** a dot left of a file means an agent just edited it. It pulses
  once when the edit lands. A faint ring means the agent only read the file.
  Markers fade over about 10 seconds, so the files the agent moved through
  read as a short trail. When more than one agent is working in the repo, each
  marker shows the letter of the agent's pane instead of a dot.
- **New changes glow:** a hunk that just appeared or changed lights up and
  fades over a few seconds, and so does a file that just showed up. A
  collapsed file glows when a hunk inside it is new.
- **Follow (`w`):** the cursor jumps to the file the agent last edited,
  expands it and lands on the hunk the edit made, then keeps doing that for
  each new edit. An eye in the header shows it's on. Moving the cursor
  yourself, clicking a row, clicking the eye or pressing `w` again stops it.

Markers and follow need Wezzy's agent hooks (⌘, → Agent hooks). The glow
works without them, for any change to the repo.

## Keys

Press `?` in the panel to see these.

| | |
|---|---|
| j / k, ↓ / ↑ | Move (through diff lines too) |
| ] / [ | Next / previous hunk or file |
| g / G | Top / bottom |
| ⌃d / ⌃u | Half a page down / up |
| tab, space | Expand / collapse (`l` expands, `h` collapses) |
| ↵ | Expand, or open the hunk in your editor |
| o | Open the file at the cursor line in your editor |
| w | Follow the agent's edits, or stop |
| s | Stage or unstage the file, hunk or section |
| S | Stage everything; unstage everything if it's all staged |
| u | Unstage the file, hunk or section |
| U | Unstage everything |
| x or d | Discard (press `d`, `y` or ↵ again to confirm) |
| c | Commit what's staged |
| a | Amend the last commit |
| p | Pull |
| P | Push |
| N | Open the branch's pull request, or create one |
| f | Comment on the line, or the whole hunk from its header |
| V | Select lines from the cursor, then `f` to comment on them |
| F | Send your comments to an agent |
| A | Only the files an agent's run changed: each agent's in turn, then everything |
| ↵ / d on a comment | Edit / delete it |
| r | Run a recipe on this repo |
| R | Refresh |
| < / > | Narrower / wider |

## An agent's run

When the agent in the pane you're in has finished a run with changes you
haven't reviewed, ⌘G opens the panel on just the files that run changed. The
header shows the agent's name; click it to see everything. `A` steps through
the run of each agent that changed this repo (with more than one, the header
shows its pane letter too), then back to everything. The +/− counts, the section counts and
`S` / `U` only cover the files shown. See [Agent runs](agent-runs.md) for the
bar that opens the panel this way.

## Committing

`c` opens a box at the top of the panel. The first line is the subject. ↵
commits, ⇧↵ adds a line, esc closes the box and keeps what you wrote for next
time. `a` opens the box with the last commit's message to amend it. Past 50
characters the subject shows its length, in the warning color past 72. A long
message scrolls inside the box, keeping the line you're on in view.

### Editing text

The commit box, the comment box and the pull request box edit like any macOS
text field. Click to place the cursor, drag to select, double-click to select
a word, triple-click for a line, and shift-click to extend the selection.

| | |
|---|---|
| ← / → | Move by character (collapses a selection) |
| ⌥← / ⌥→ | Move by word |
| ⌘← / ⌘→, ⌃A / ⌃E | Start / end of the line |
| ↑ / ↓ | Up / down a row, wrapped rows included |
| ⌘↑ / ⌘↓ | Start / end of the text |
| ⇧ with any of these | Select as you move |
| ⌘A | Select all |
| ⌘C / ⌘X / ⌘V | Copy / cut / paste |
| ⌘Z / ⇧⌘Z | Undo / redo |
| ⌫ / ⌥⌫ / ⌘⌫ | Delete a character / a word / to the start of the line |
| fn⌫ | Delete the character after the cursor |
| ⌃K | Delete to the end of the line |
| tab | Insert four spaces |
| ⇧↵ | New line |
| ↵ | Commit, save the comment, or create the pull request |
| esc | Close the box |

## Comments for the agent

Leave comments with `f` on a line, a selection (`V`) or a hunk header, then
press `F`. All of them go to the agent as one message, each with its file, line
numbers and the changed lines, pasted into the agent's prompt so you can add to
it before pressing ↵. Comments on files one agent's run changed go to that
agent. Otherwise, if more than one agent is open in the workspace, you pick
which one.

The comment box takes the same [editing keys](#editing-text) as the commit
box: ↵ saves the comment and esc cancels it.

## Pulling and pushing

`p` pulls the branch from its upstream and `P` pushes it. Both run in the
background; a toast says how many commits came in or went out, or shows git's
error. Pushing a branch that has no upstream yet pushes it to its remote
(`origin` if it has no other) and sets the upstream.

Pulling follows your git config (`pull.rebase`, `pull.ff`). If the branch and
its upstream have diverged and git hasn't been told how to reconcile them, the
pull fails with git's message.

## Pull requests

Press `N` on a branch with commits to compose a pull request. The title starts
as the commit's subject when the branch has one commit, or as the branch name
in words (`steve/fix-login` becomes "Fix login"). Add a body on the lines below
it with ⇧↵.

| | |
|---|---|
| ↵ | Create the pull request |
| ⌃D | Toggle draft |
| esc | Cancel |

Wezzy pushes the branch first (setting its upstream if it doesn't have one),
then creates the pull request against the repo's default branch. A toast says
when it's done, and the header shows the new PR. Nothing opens in your
browser.

If the branch already has an open pull request, `N` opens it in your browser
instead.

## Setting it up

Pull request info and creation use the [GitHub CLI](https://cli.github.com).
Install it and log in:

```sh
brew install gh
gh auth login
```

Opening files uses your editor, `$EDITOR` by default. To pick another, set
`editor` in `~/.config/wezzy/wezzy.lua`:

```lua
wezzy.setup({
  editor = { "nvim", "+{line}", "{file}" },
})
```

## Details

- The panel refreshes every couple of seconds, and right away after you
  stage, discard, commit, pull or push. It never locks the repo, so it won't get in the way
  of an agent that's committing.
- Pull and push never ask for a password. If git needs credentials it
  can't find on its own, the toast shows the error.
- The pull request in the header is checked about once a minute, when you
  switch branches, after a push, and when you press `R`.
- Without `gh`, or when you're not logged in, the header shows no pull
  request and `N` shows a toast.
- `N` won't create a pull request from the default branch, from a detached
  HEAD, in a repo with no remote, or when the branch has no commits the default
  branch doesn't have. A toast says which.
- A merged or closed pull request doesn't stop `N`: it composes a new one.
- If creating fails, the box opens again with what you wrote, and a toast
  shows the error.
- Very large diffs and binary files show up without hunks. You can still
  stage, discard and open them as whole files.
- A zoomed pane (⌘↵) hides the panel until you unzoom.
- Nothing glows when the panel opens or switches to another repo: only
  changes that arrive while you're looking count. Staging or unstaging a hunk
  doesn't make it glow.
- Follow waits while you write a commit message or comment, select lines or
  confirm a discard, and picks up the latest edit after. Closing the panel
  turns it off. An edit to a file that never shows up in the diff (it ended
  up unchanged) is skipped.
