#!/usr/bin/env bash
# Build CodeNotch on a Mac: one app for Apple Silicon and Intel alike, as an
# .app and a .dmg, signed ad hoc (no Apple developer account needed).
#
#   bash packaging/macos/build.sh
#
# Needs the Xcode command line tools, Rust and Node.js 22 or newer; it says
# which is missing and how to get it. Run from anywhere in the checkout.
set -euo pipefail

cd "$(dirname "$0")/../.."

missing=0
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Missing the Xcode command line tools: xcode-select --install"
  missing=1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Missing Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  missing=1
fi
if ! command -v npm >/dev/null 2>&1; then
  echo "Missing Node.js 22 or newer: https://nodejs.org (or: brew install node)"
  missing=1
fi
[ "$missing" -eq 0 ] || exit 1

# One binary for both kinds of Mac.
rustup target add aarch64-apple-darwin x86_64-apple-darwin

npm ci --no-audit --no-fund
npx tauri build --target universal-apple-darwin --bundles app,dmg

bundle=src-tauri/target/universal-apple-darwin/release/bundle
echo
echo "Built:"
ls -d "$bundle"/macos/*.app "$bundle"/dmg/*.dmg 2>/dev/null
echo
echo "Open the .dmg and drag CodeNotch to Applications. The first time, macOS"
echo "asks to allow reading Claude Code's login from the keychain: choose"
echo "Always Allow. On another Mac, where the app arrives downloaded, open it"
echo "with right-click > Open the first time: it isn't signed by Apple."
