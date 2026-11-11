#!/bin/sh
# dekita agent hook. Agent harnesses (Claude Code, ...) call this on lifecycle
# events with a JSON payload on stdin; we store the latest one per pane so the
# app can show exact status. Installed by dekita; safe to delete.
#
#   usage: agent-hook <harness>        (payload on stdin)
#
# Only active inside dekita panes (DEKITA is set by its session server), so
# agents running in other terminals are unaffected.
[ -n "$DEKITA" ] && [ -n "$WEZTERM_PANE" ] || { cat >/dev/null; exit 0; }

dir="${XDG_STATE_HOME:-$HOME/.local/state}/dekita/agents"
mkdir -p "$dir" 2>/dev/null || exit 0

payload=$(cat)
[ -n "$payload" ] || payload='{}'
tmp="$dir/.$WEZTERM_PANE.$$"
printf '{"harness":"%s","pane":%s,"ts":%s,"payload":%s}\n' \
  "${1:-unknown}" "$WEZTERM_PANE" "$(date +%s)" "$payload" > "$tmp" \
  && mv -f "$tmp" "$dir/$WEZTERM_PANE.json"
exit 0
