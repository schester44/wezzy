# SQL

Query a database without leaving your terminal. Open an editor on any Postgres
database from ⌘K, write a query with table and column names completed for you,
and see the result as a table below it. The project you're in usually needs no
setup: Wezzy finds its database in the `.env` files.

## Opening it

| | |
|---|---|
| ⌘K, then the database's name | Its SQL editor, in a new tab |
| ⌘↵ on it instead | In a split to the right |
| ⌘K, then `sql neverwait select * from users` | Open `neverwait` and run that query |

Databases are listed under "Databases" in ⌘K, with where they are beside them
(`sql localhost/neverwait`). The tab is named after the database, and it splits,
zooms and moves between workspaces like any other pane.

## Writing queries

| | |
|---|---|
| ⌘↵ | Run the statement under the cursor, or the selection |
| esc | Go to the result (or stop a query that's running) |
| ctrl+space | Show completions |
| tab or ↵ | Take the completion |
| ↑ ↓, ctrl+n ctrl+p | Move through completions |
| ⌘Z, ⌘⇧Z | Undo, redo |
| ⌘A, ⌘X, ⌘C, ⌘V | Select all, cut, copy, paste |

Statements are separated by `;`, so you can keep several queries in the editor
and run whichever one the cursor is in.

Completions appear as you type a name:

- after `from`, `join`, `update` or `into`: tables
- after `u.`, where `u` is a table or its alias in the statement: that table's columns
- after a schema name and `.`: its tables
- anywhere else: columns of the tables the statement uses, then tables and keywords

Tables in your search path are completed bare (`users`), others with their schema
(`audit.events`). Names that need quoting get quotes.

## Reading the result

| | |
|---|---|
| h j k l, arrows | Move between cells |
| g, G | First row, last row |
| 0, $ | First column, last column |
| ctrl+d, ctrl+u | Half a page down, up |
| ctrl+f, ctrl+b | A screen down, up |
| [ ] | Previous page, next page |
| V | Start selecting rows; move to extend, V again to stop |
| space | Select or unselect the row |
| ⌘A | Select every row on the page |
| y or ⌘C | Copy the cell, or the selected rows |
| Y | Copy the selected rows (or this row) with column names |
| esc | Clear the selection, or go back to the editor |
| i, ↵ | Back to the editor |

With the mouse: click a cell to move there, shift-click to select every row up
to it, ⌘-click to add or remove a row. The wheel scrolls the rows; click in the
editor to put the cursor there.

Rows are copied tab-separated, one per line, so they paste into a spreadsheet
as cells. A cell is copied exactly as it is.

Queries that only read (`select`, `with`, `table`, `values`) come back 500 rows
at a time. The line between the editor and the result shows where you are
(`1–500 of 12,345`), how long the page took, and how many rows you've
selected; ‹ › at its right end turn the page. Counting a large result can take
a while, so after a few seconds Wezzy stops and says `of many`. Anything else,
and several statements run together, comes back in one go, up to 10,000 rows.

A statement that returns no rows shows how many it changed; an error shows
Postgres's message.

## Setting it up

Wezzy looks for databases in the projects you have open in any pane, and in
your saved workspaces' folders, the one you're in first. In each it reads the `.env`,
`.env.local` and `.env.development` files at the repository root and up to two
folders down (`apps/api/.env`). It uses `DATABASE_URL` (or `POSTGRES_URL`,
`DB_URL`) when it's a `postgres://` URL, and otherwise builds one from `DB_HOST`,
`DB_PORT`, `DB_NAME`, `DB_USER` and `DB_PASS` (or the `PG*` and `POSTGRES_*`
names). The database takes the project's name; when a project has several,
each one is named after its folder too (`neverwait-api`).

To add databases by name, in `~/.config/wezzy/wezzy.lua`:

```lua
local in_app, wezzy = pcall(require, "wezzy")
if in_app then
  wezzy.setup({
    databases = {
      { name = "prod", url = "postgresql://readonly@db.example.com/app?sslmode=require" },
    },
  })
end
```

or in `~/.config/wezzy/settings.json`:

```json
{
  "databases": [
    { "name": "prod", "url": "postgresql://readonly@db.example.com/app?sslmode=require" }
  ]
}
```

Databases you name come first, and always show up, whichever project you're in.

## Details

- Postgres only.
- The editor's text is kept per database, so it's there the next time you open
  that database, after a restart too. The result isn't kept.
- When the editor holds several statements and you select them all, they run
  together and you see the last one's result.
- Each page runs the query again, so a table that's changing can shift rows
  between pages. Add an `order by` to keep pages stable.
- Values are shown on one line and cut to fit their column. `y` copies the whole value.
- Connections use TLS when the server offers it. Certificates are checked only
  with `sslmode=verify-ca` or `verify-full`.
- If the connection drops, close the tab and open it again.
