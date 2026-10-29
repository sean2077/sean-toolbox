#!/usr/bin/env bash
# Run native acceptance in an isolated display, never on the user's desktop.
set -euo pipefail
cd "$(dirname "$0")/../../.."
for dependency in xvfb-run openbox xterm xprop; do
  command -v "$dependency" >/dev/null || { echo "Missing test dependency: $dependency" >&2; exit 1; }
done
export TOOLBOX_X11_SMOKE=1
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY
exec xvfb-run -a -s '-screen 0 1280x800x24' bash -c '
  set -euo pipefail
  openbox >/dev/null 2>&1 &
  wm_pid=$!
  trap "kill $wm_pid 2>/dev/null || true" EXIT
  for attempt in {1..40}; do
    if xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q "window id"; then break; fi
    sleep 0.05
  done
  cargo test -p toolbox-windows native_x11_smoke -- --ignored --nocapture
'
