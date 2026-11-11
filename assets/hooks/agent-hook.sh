#!/bin/sh
# wezzy agent hook. Agent harnesses (Claude Code, Codex) call this on
# lifecycle events with a JSON payload on stdin; we store the latest one per
# pane so the app can show exact status. Installed by wezzy; safe to delete.
#
#   usage: agent-hook <harness>        (payload on stdin)
#
# Only active inside wezzy panes (WEZZY is set by its session server), so
# agents running in other terminals are unaffected. DEKITA is the pre-rename
# marker, still set in panes of a session server started before the rename.

# Codex reads JSON from some hooks' stdout; an empty object changes nothing.
[ "$1" = codex ] && trap 'printf "{}\n"' EXIT

[ -n "$WEZZY$DEKITA" ] && [ -n "$WEZTERM_PANE" ] || { cat >/dev/null; exit 0; }

dir="${XDG_STATE_HOME:-$HOME/.local/state}/wezzy/agents"
mkdir -p "$dir" 2>/dev/null || exit 0

payload=$(cat)
[ -n "$payload" ] || payload='{}'
tmp="$dir/.$WEZTERM_PANE.$$"
printf '{"harness":"%s","pane":%s,"ts":%s,"payload":%s}\n' \
  "${1:-unknown}" "$WEZTERM_PANE" "$(date +%s)" "$payload" > "$tmp" \
  && mv -f "$tmp" "$dir/$WEZTERM_PANE.json"
exit 0
