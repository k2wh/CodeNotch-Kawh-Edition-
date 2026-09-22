#!/usr/bin/env bash
# Lint and unit-test the Linux side, in the image from the Dockerfile beside
# this script. Uses the same copy of the source build.sh makes.
set -uo pipefail

rsync -a --delete \
  --exclude .git --exclude node_modules --exclude dist --exclude src-tauri/target \
  /host/ /build/
# The crate embeds the frontend at compile time; an empty one does for tests.
mkdir -p /build/dist && [ -f /build/dist/index.html ] || echo '<!doctype html>' >/build/dist/index.html

cd /build/src-tauri
echo "--- clippy"
cargo clippy --workspace --all-targets 2>&1 | grep -E '^(warning|error)' -A 8 | grep -vE '^\s+(Compiling|Checking)'
echo "--- tests"
cargo test --workspace 2>&1 | grep -E '^test result|FAILED|panicked|^error'
