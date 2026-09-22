#!/usr/bin/env bash
# Type-check the macOS build without a Mac, in the Linux image from
# packaging/linux: `cargo check` and clippy for aarch64-apple-darwin.
#
# Nothing is linked, so no macOS SDK is needed, except that one dependency
# (objc2-exception-helper) compiles a small C file in its build script. A
# stand-in compiler and archiver that write empty files get it through;
# `check` never looks inside them. This proves the Rust compiles for the
# Mac, not that it runs there: that takes a Mac (see build.sh).
set -uo pipefail

rsync -a --delete \
  --exclude .git --exclude node_modules --exclude dist --exclude src-tauri/target \
  /host/ /build/
# The crate embeds the frontend at compile time; an empty one does for a check.
mkdir -p /build/dist && [ -f /build/dist/index.html ] || echo '<!doctype html>' >/build/dist/index.html

rustup target add aarch64-apple-darwin >/dev/null 2>&1

mkdir -p /opt/stand-in
cat >/opt/stand-in/cc <<'EOF'
#!/bin/sh
# Write whatever -o names, empty; answer anything else with success.
while [ $# -gt 0 ]; do
  if [ "$1" = "-o" ]; then : >"$2"; shift; fi
  shift
done
exit 0
EOF
cat >/opt/stand-in/ar <<'EOF'
#!/bin/sh
# Make the first *.a named an empty archive.
for arg in "$@"; do
  case "$arg" in *.a) printf '!<arch>\n' >"$arg"; exit 0 ;; esac
done
exit 0
EOF
chmod +x /opt/stand-in/cc /opt/stand-in/ar
export CC_aarch64_apple_darwin=/opt/stand-in/cc AR_aarch64_apple_darwin=/opt/stand-in/ar

cd /build/src-tauri
echo "--- clippy, aarch64-apple-darwin"
cargo clippy --target aarch64-apple-darwin --workspace --all-targets 2>&1 \
  | grep -E '^(warning|error)' -A 12 | grep -vE '^\s+(Compiling|Checking)'
echo "--- done (exit ${PIPESTATUS[0]})"
