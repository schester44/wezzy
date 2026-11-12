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
# A permission request is overwritten by the Notification that follows it, so
# keep its payload (the full tool input) beside the record until this agent
# moves on: <name>.req exists exactly while the dialog is up.
req="$dir/$name.req"
case "$payload" in
  *'"hook_event_name":"PermissionRequest"'*)
    printf '%s\n' "$payload" > "$req.$$" && mv -f "$req.$$" "$req" ;;
  *'"hook_event_name":"Notification"'*) ;;
  *) rm -f "$req" ;;
esac
# Activity log, one line per event, appended so none are lost between polls:
# prompts and stops (turn boundaries) and file tools (what the agent read and
# changed), subagents included. Read and trimmed by the app.
log=
case "$payload" in
  *'"hook_event_name":"UserPromptSubmit"'*|*'"hook_event_name": "UserPromptSubmit"'*) log=1 ;;
  *'"hook_event_name":"Stop"'*|*'"hook_event_name": "Stop"'*) log=1 ;;
  *'"hook_event_name":"PreToolUse"'*|*'"hook_event_name": "PreToolUse"'*)
    case "$payload" in
      *'"tool_name":"Read"'*|*'"tool_name": "Read"'*) log=1 ;;
      *'"tool_name":"Edit"'*|*'"tool_name": "Edit"'*) log=1 ;;
      *'"tool_name":"MultiEdit"'*|*'"tool_name": "MultiEdit"'*) log=1 ;;
      *'"tool_name":"Write"'*|*'"tool_name": "Write"'*) log=1 ;;
      *'"tool_name":"NotebookEdit"'*|*'"tool_name": "NotebookEdit"'*) log=1 ;;
      *'"tool_name":"apply_patch"'*|*'"tool_name": "apply_patch"'*) log=1 ;;
    esac ;;
esac
# (raw newlines can't occur inside JSON strings, so dropping them keeps one
# event per line even if a harness pretty-prints)
[ -n "$log" ] && printf '{"harness":"%s","pane":%s,"ts":%s,"payload":%s}\n' \
  "${1:-unknown}" "$WEZTERM_PANE" "$(date +%s)" "$(printf '%s' "$payload" | tr -d '\n')" \
  >> "$dir/$WEZTERM_PANE.log"

tmp="$dir/.$name.$$"
printf '{"harness":"%s","pane":%s,"ts":%s,"payload":%s}\n' \
  "${1:-unknown}" "$WEZTERM_PANE" "$(date +%s)" "$payload" > "$tmp" \
  && mv -f "$tmp" "$dir/$name.json"
exit 0
