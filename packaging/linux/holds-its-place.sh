#!/usr/bin/env bash
# Does the notch stay where it was put?
#
# A window manager can move a window after the fact — GNOME does it to a move
# that arrives while a resize is still in flight, which left the notch
# floating by the left edge after a change of edge. The app reads its real
# position back from the X server and says it again; this shoves the window
# aside the way a window manager would and checks that it returns.
#
# Run in the image built from the Dockerfile beside this script, with a built
# .deb in /out:
#   docker run --rm -v "$PWD:/host:ro" -v cn-out:/out codenotch-linux \
#     bash /host/packaging/linux/holds-its-place.sh
set -uo pipefail

apt-get update -qq >/dev/null
apt-get install -y -qq /out/CodeNotch_*_amd64.deb >/dev/null 2>&1 \
  && echo "installed $(dpkg-query -W code-notch)"

export DISPLAY=:99
Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp &
sleep 1
openbox --sm-disable >/dev/null 2>&1 &
sleep 1

mkdir -p ~/.claude/projects
dbus-run-session -- codenotch >/tmp/codenotch.log 2>&1 &
sleep 12

# The app owns several X windows; the notch is the one that carries the name
# and a size worth having. (A one-pixel helper window carries it too.)
notch=""
for id in $(xdotool search --name '^CodeNotch$'); do
  width=$(xdotool getwindowgeometry --shell "$id" | sed -n 's/^WIDTH=//p')
  if [ "${width:-0}" -gt 100 ]; then
    notch=$id
    break
  fi
done
if [ -z "$notch" ]; then
  echo "FAIL: no notch window"
  tail -20 /tmp/codenotch.log
  exit 1
fi

where() {
  # Root-relative, as the app reads it: the geometry line's own x,y is
  # relative to the frame on some window managers.
  xdotool getwindowgeometry --shell "$notch" | grep -E '^(X|Y|WIDTH|HEIGHT)=' | tr '\n' ' '
}

echo "docked:   $(where)"
docked=$(where)

# Shove it into the corner, which is what the report looked like.
xdotool windowmove "$notch" 0 0
sleep 1
echo "shoved:   $(where)"

# A poll to notice and undo it. The poll loop sleeps up to fifteen seconds
# between passes, and a change of edge doesn't wait for one: it looks again
# itself, right after moving.
back=""
for _ in $(seq 1 12); do
  sleep 2
  back=$(where)
  [ "$back" = "$docked" ] && break
done
echo "after:    $back"

if [ "$back" = "$docked" ]; then
  echo "PASS: the notch went back to where it docked"
else
  echo "FAIL: it stayed where it was shoved"
  grep -iE 'window manager moved|putting it back' /tmp/codenotch.log | tail -3
  exit 1
fi
