#!/usr/bin/env bash
# Try a built .deb out on a virtual screen: install it, start it as a desktop
# session would, and report what the window manager and the X server see of
# the notch. Screenshots and the app's log land in /out beside the package.
#
# Run in the image built from the Dockerfile beside this script.
set -uo pipefail

# SHOTS prefixes the screenshots, so runs side by side keep theirs apart.
shot="/out/${SHOTS:-}"

apt-get update -qq >/dev/null
apt-get install -y -qq /out/CodeNotch_*_amd64.deb >/dev/null 2>&1 \
  && echo "installed $(dpkg-query -W code-notch)"
# WITH_GL=1 gives WebKit Mesa's software OpenGL, closer to a machine with a
# graphics card than the plain CPU path it takes without.
if [ -n "${WITH_GL:-}" ]; then
  apt-get install -y -qq libgl1-mesa-dri libegl-mesa0 >/dev/null 2>&1 && echo "with Mesa GL"
fi

export DISPLAY=:99
Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp &
sleep 1
openbox --sm-disable >/dev/null 2>&1 &
# NO_COMPOSITOR=1 leaves transparency out, for telling the app's drawing
# apart from the compositor's.
[ -n "${NO_COMPOSITOR:-}" ] || picom --backend xrender --no-vsync >/dev/null 2>&1 &
sleep 1

# Something to click through to: a window filling the screen, under the notch.
xlogo -geometry 1920x1080+0+0 >/dev/null 2>&1 &
sleep 1

# A pretend Claude account, so there is a ring to draw. Signed out, so it
# asks nobody for anything.
mkdir -p ~/.claude/projects

# APP_ENV passes extra environment to the app, for comparing renderers.
env ${APP_ENV:-} dbus-run-session -- codenotch >"${shot}codenotch.log" 2>&1 &
sleep 12

notch=$(xdotool search --onlyvisible --name '^CodeNotch$' | head -n1)
logo=$(xdotool search --onlyvisible --class xlogo | head -n1)
echo "--- what the window manager was given"
xprop -id "$notch" _NET_WM_STATE _NET_WM_DESKTOP WM_HINTS \
  | grep -vE 'bitmap|group leader|Initial state' | sed 's/^/  /'
xwininfo -id "$notch" | grep -E 'Absolute|Width|Height|Map State' | sed 's/^/  /'
import -window root "${shot}rest.png"

# Onto the strip, which opens it, then onto the ring, which opens its card.
xdotool mousemove 1910 560; sleep 0.4
for x in 1900 1890 1880 1874; do xdotool mousemove "$x" 512; sleep 0.1; done
sleep 1
import -window root "${shot}card.png"
# The notch's own pixels, before any compositor has had them.
import -window "$notch" "${shot}card-window.png"
echo "  screenshots: rest.png, card.png, card-window.png"

# Who gets the pointer. With the window manager gone the X server alone
# decides, so this is the notch's own input region being tested.
pkill openbox; sleep 1
who() {
  eval "$(xdotool getmouselocation --shell 2>/dev/null)"
  case "$WINDOW" in
    "$notch") echo "the notch" ;;
    "$logo") echo "the window underneath" ;;
    *) echo "something else ($WINDOW)" ;;
  esac
}
echo "--- clicks"
xdotool mousemove 900 540; sleep 1.5
# The card should be gone without a trace.
import -window root "${shot}after.png"
xdotool mousemove 1700 300; sleep 0.3
echo "  resting, pointer beside the strip:   $(who)"
xdotool mousemove 1905 540; sleep 0.6
echo "  pointer on the strip (opens it):     $(who)"
xdotool mousemove 900 540; sleep 1.5
xdotool mousemove 1700 300; sleep 0.3
echo "  after leaving again:                 $(who)"

echo "--- app log"
grep -vE 'AT-SPI|dbind' "${shot}codenotch.log" | tail -n 20
