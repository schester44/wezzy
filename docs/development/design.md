# Design language

The look we're after is quiet, precise, and dense without feeling cramped. Chrome
stays neutral and sits back, color only shows up where it means something, and
the hierarchy comes from type weight and spacing rather than boxes and fills.

The git panel (`src/ui/git.rs`), the command palette and the settings overlay
(`palette_view` / `settings_view` in `src/ui/view.rs`) are the reference
implementations. Match them when building
new UI.

## Principles

1. **Neutral by default.** Text, borders and fills are tints of the terminal
   foreground. Accent, success, warning and danger are reserved for meaning
   (selection, diff lines, status, destructive actions) and never used as decoration.
2. **One way to show selection.** The selected row gets a faint full-width fill
   and a 2px accent bar on its left edge. No rounded "pills" or tinted accent
   backgrounds.
3. **Edge to edge.** List rows span the whole container. Content is inset with
   padding, never with margins on the row, so fills, hovers and diff bands
   always reach both edges.
4. **Hierarchy from type, not boxes.** Section labels are small and muted.
   Primary text is `fg`, secondary is `fg_muted`, metadata is `fg_subtle`. Only
   add a border or card when it separates a real surface (modal, input).
5. **No visible keymaps in chrome.** Don't line footers with key hints. Put
   shortcuts behind `?` (a modal sheet) and on buttons themselves (`Commit ↩`).
   A footer appears only for transient state: a confirmation or progress.
6. **Hairlines.** Borders are 1px `t.border` (foreground at 8% alpha). Never
   use a colored border, except the accent ring on a focused text input.

## Tokens

All colors come from `Theme` (`src/ui/mod.rs`) and derive from the WezTerm
palette, so every color scheme works. Never hard-code a hue.

| Token | Use |
|---|---|
| `fg` | primary text: titles, file names, changed code |
| `fg_muted` | secondary text: labels, context code, unselected section titles |
| `fg_subtle` | metadata: counts, paths, line numbers, placeholders, icons |
| `border` | every hairline |
| `hover` | key cap background, ghost button hover |
| `chrome` | panel / sidebar surface |
| `elevated` | modals, palette, toasts |
| `accent` | selection bar, caret, focused input ring (ANSI blue, never magenta) |
| `success` / `danger` | additions / deletions, destructive buttons |
| `warning` | modified status, over-length warnings |

Derived fills (via `with_alpha`):

| Fill | Value |
|---|---|
| selected row (focused) | `fg` at 0.07 |
| selected row (unfocused) | `fg` at 0.04, and the bar becomes `fg_subtle` at 0.5 |
| row hover | `fg` at 0.03 |
| added / removed line | `success` / `danger` at 0.10 |
| recessed band (hunk header, input) | black at 0.14–0.22 |
| modal backdrop | black at 0.28–0.45 |

## Type

- UI font: system (`.SystemUIFont`), 13px base.
- Mono (`term_family()`): code, hashes, counts, key caps, line numbers.
- Sizes: **13** body · **12** secondary/meta · **11** section labels, counts,
  mono meta · **10** key caps. Avoid anything bigger except a search field (14).
- Weights: regular, plus `MEDIUM` for titles, section labels and buttons.
  Avoid `SEMIBOLD`/`BOLD`.
- Section labels are sentence case ("Unstaged", "Agents") with a mono count
  in `fg_subtle` after them. No ALL CAPS.

## Spacing and sizes

- Horizontal content inset: 16px (`px_4`). Nested levels step in by ~12px.
- Row heights: section label 32, list row 30–36, hunk header 26, code line 20.
- Header blocks: 14px top, 12px bottom, hairline below.
- Radii: `rounded_sm` key caps · `rounded_md` buttons · `rounded_lg` inputs ·
  `rounded_xl` modals. Rows have no radius.

## Components

- **Key cap**: `kbd()` in `view.rs`. Mono 10px, 18px tall, `hover` fill, hairline border.
- **Icons**: stroke SVGs in `assets/icons` (lucide style, 2px stroke), 11–15px,
  `fg_subtle` (or `fg` when the row is selected). Use `Icon::ChevronRight` and
  `Icon::ChevronDown` for disclosure, never text glyphs like ▸.
- **Button**: soft fill (`fg` at 7%, 12% on hover), hairline border, `fg` text,
  `MEDIUM`, 22–24px, with its shortcut inline in `fg_subtle` (`Commit ↩`).
  No solid white or accent fills, because they're too loud against the chrome.
- **Destructive button**: `danger` background, white text.
- **Ghost button**: hairline border, `fg` text, `hover` on hover.
- **Multi-line input**: enter submits, shift+enter adds a line, esc cancels.
- **Text input**: recessed fill, 1px accent ring at ~55–70%, 1.5px accent
  caret, `fg_subtle` placeholder that says what to type.
- **Confirmation**: inline footer bar (`Discard hunk in x?` + Cancel / Discard),
  not a modal. `esc` or any other key cancels. App-level confirms (quit, close
  pane / tab, Lua `Confirmation`) are a small modal: `MEDIUM` title and 12px
  `fg_muted` detail, then a recessed footer with a hairline above it holding
  Cancel `esc` (ghost) and the verb `↩`, which is destructive when it kills
  something and a soft button otherwise (`button()` + `ButtonKind` in `view.rs`).
- **Modal header**: 48px row with a `MEDIUM` 13px title (or the search field)
  and a hairline below; the list follows with 6px vertical padding.
- **Setting row**: label in `fg` on the left, value in 12px `fg_muted` on the
  right (`fg` when selected), plus a chevron icon for choices.
- **Shortcut sheet**: a modal opened with `?`, with groups of label on the left
  and key caps on the right. Any key closes it.
- **Status dot**: 6px. `success` = running server, or an agent done while you
  were elsewhere (unseen, until you focus its pane); `fg_subtle` = idle or seen,
  `fg_muted` spinner = working, `warning` alert icon = needs input. Status never
  tints the row; the icon and its detail text carry it.
- **Sidebar row**: same selection as lists. Keyboard cursor gets the `fg` 0.07 fill
  + accent bar; the pane you're in gets only the 0.04 fill, no bar.
- **Empty state**: a 32px hairline circle with an icon, a `fg_muted` title and
  an optional `fg_subtle` detail line, centered.

## Checklist for new UI

- [ ] Every color comes from `Theme`, and accent appears only for selection or focus.
- [ ] Rows are full width and the selection is fill + left bar.
- [ ] Only 1px `border` hairlines, and no tinted card backgrounds.
- [ ] No key hints in persistent chrome. Shortcuts live in `?` or on buttons.
- [ ] Text sizes are 10–13px (14 for search), and weights are regular or medium only.
- [ ] Disclosure uses the chevron icons, not text glyphs.
