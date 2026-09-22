# macOS build

The Mac build follows the original CodeNotch for macOS
([vinzdg/codenotch](https://github.com/vinzdg/codenotch)) where it can: the
notch is a borderless, non-activating `NSPanel` at the menu bar's level, on
every Space and over full-screen apps, so looking at it or clicking it never
takes focus off what you were doing; and a click on a ring walks up from the
agent's process to the app it runs in. See `src-tauri/src/platform/macos_impl.rs`.

## Installing

Each release has the `.dmg`, built and launched on one of GitHub's Macs by the
Release workflow (`.github/workflows/release.yml`). Open it and drag CodeNotch
to Applications.

The app is signed ad hoc, not by Apple, so the first time macOS says it can't
check it for malware. Open **System Settings → Privacy & Security** and choose
**Open Anyway** next to CodeNotch; on macOS 14 and older, right-click the app →
Open does the same. It only asks once.

On first launch macOS asks to let CodeNotch read Claude Code's login from the
keychain (the service `Claude Code-credentials`). Choose **Always Allow**; an
ad hoc signature changes with every build, so a new version asks again.

## Building, on a Mac

```sh
bash packaging/macos/build.sh
```

It checks for the Xcode command line tools, Rust and Node.js, then builds a
universal app (Apple Silicon and Intel) as an `.app` and a `.dmg` under
`src-tauri/target/universal-apple-darwin/release/bundle/`. The app opens right
away on the Mac that built it; on another Mac it's like a downloaded one, above.

## Checking, without a Mac

`check.sh` runs clippy for `aarch64-apple-darwin` in the Linux image from
`packaging/linux`, so a change can be known to compile for the Mac before one
is at hand. It proves the code compiles, not that it runs:

```sh
docker run --rm -v "$PWD:/host:ro" \
  -v cn-target:/build/src-tauri/target -v cn-node:/build/node_modules \
  -v cn-cargo:/root/.cargo/registry \
  codenotch-linux bash /host/packaging/macos/check.sh
```

## What differs from Windows and Linux

- Other apps are found and raised as apps, by process id. Raising one window
  of an app would need the Accessibility and Screen Recording permissions.
- The "stay behind" list shows app names without thumbnails, for the same
  reason.
- "Stay behind full-screen apps" keeps the notch off full-screen Spaces
  rather than sliding it away.
