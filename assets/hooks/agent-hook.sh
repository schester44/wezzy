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
# Events from a subagent carry its agent_id; keep those in their own file so
# they don't overwrite the main agent's status. (A key inside a JSON string
# value would be escaped, \"agent_id\", so this only matches the real field.)
name=$WEZTERM_PANE
case "$payload" in
  *'"agent_id":"'*)
    id=$(printf '%s' "$payload" | sed -n 's/.*"agent_id":"\([A-Za-z0-9_-]*\)".*/\1/p' | head -n 1)
    [ -n "$id" ] && name="$WEZTERM_PANE.$id" ;;
esac
tmp="$dir/.$name.$$"
printf '{"harness":"%s","pane":%s,"ts":%s,"payload":%s}\n' \
  "${1:-unknown}" "$WEZTERM_PANE" "$(date +%s)" "$payload" > "$tmp" \
  && mv -f "$tmp" "$dir/$name.json"
exit 0
