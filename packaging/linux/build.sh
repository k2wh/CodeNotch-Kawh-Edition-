#!/usr/bin/env bash
# Build the .deb inside the image from the Dockerfile beside this script.
#
# The checkout is mounted read-only at /host and the package lands in /out.
# The source is copied in rather than built in place: a Windows checkout's
# node_modules hold Windows binaries, and building on the mount is slow.
set -euo pipefail

rsync -a --delete \
  --exclude .git --exclude node_modules --exclude dist --exclude src-tauri/target \
  /host/ /build/
cd /build

npm ci --no-audit --no-fund
npx tauri build --bundles deb

mkdir -p /out
cp src-tauri/target/release/bundle/deb/*.deb /out/
ls -l /out
