#!/usr/bin/env bash
# Build Wezzy.app, install it in /Applications and restart the running app.
#
#   scripts/rebuild.sh
#
# Only the app quits: the session server (wezterm-mux-server) keeps running, so
# every shell and agent survives, and the reopened app attaches to it again.
# Safe to run from a Wezzy pane for the same reason.
set -euo pipefail
cd "$(dirname "$0")/.."

APP=/Applications/Wezzy.app
# the app's own binary; never the bundled wezterm-mux-server
EXE="^$APP/Contents/MacOS/wezzy( |$)"

./scripts/bundle.sh

echo "==> quitting Wezzy"
if pgrep -f "$EXE" >/dev/null; then
  pkill -TERM -f "$EXE" || true
  for _ in $(seq 50); do
    pgrep -f "$EXE" >/dev/null || break
    sleep 0.1
  done
  pkill -KILL -f "$EXE" 2>/dev/null || true
fi

echo "==> installing $APP"
rm -rf "$APP"
mv target/release/Wezzy.app "$APP"

echo "==> opening Wezzy"
open "$APP"
