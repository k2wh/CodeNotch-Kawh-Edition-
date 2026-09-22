#!/usr/bin/env bash
# Run the X11 layer's live test (`x11_desktop` in platform/linux_impl.rs) on a
# virtual screen with a window manager and two xterms to find, raise and go
# back between. Uses the same copy of the source build.sh makes.
set -uo pipefail

rsync -a --delete \
  --exclude .git --exclude node_modules --exclude dist --exclude src-tauri/target \
  /host/ /build/
# The crate embeds the frontend at compile time; an empty one does for a test.
mkdir -p /build/dist && [ -f /build/dist/index.html ] || echo '<!doctype html>' >/build/dist/index.html

export DISPLAY=:99
Xvfb :99 -screen 0 1920x1080x24 -nolisten tcp &
sleep 1
openbox --sm-disable >/dev/null 2>&1 &
sleep 1
# Running `sleep` rather than a shell, whose prompt would retitle them.
xterm -T one -geometry 80x24+100+100 -e sleep 600 &
xterm -T two -geometry 80x24+700+300 -e sleep 600 &
sleep 2

cd /build/src-tauri
cargo test --lib -- --ignored x11_desktop --nocapture 2>&1 | grep -vE '^\s+(Compiling|Downloaded|Downloading)'
