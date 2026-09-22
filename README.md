# CodeNotch (Kawhã Edition)

A lightweight always-on-top HUD for **Windows, Linux and macOS** that shows how
much of each AI coding assistant's usage limit you have burned, when the window
resets, and whether a background agent is generating, finished, or **waiting
for your approval**.

This edition builds on [Rohan's CodeNotch for
Windows](https://github.com/Rohanx04/CodeNotch), an open-source alternative to
the macOS [codenotch](https://github.com/vinzdg/codenotch), rebuilt on Tauri v2
with a Rust backend and a React frontend. It adds seven providers, per-model
weekly limits, a countdown and chimes when a limit runs out and comes back,
token counts and what a plan has returned, several accounts per tool, a ring
that takes you to an agent's window and back, quiet alerts when you're already
there, English, Portuguese and Spanish, and the Linux and macOS versions.

**Download** from [Releases](https://github.com/k2wh/CodeNotch-Kawh-Edition-/releases):
the `.exe` installer for Windows 10/11, the `.deb` for Ubuntu 22.04+ and
Pop!_OS 22.04 (an X11 session), and the `.dmg` for macOS 11 or newer, Apple
Silicon and Intel alike. The Mac app is built on a Mac, see
[packaging/macos](packaging/macos/README.md); the Linux one in Docker, see
[packaging/linux](packaging/linux/README.md).

```
                          ╭──────╮   resting: a black strip carved into the
                          │  ◕   │   screen edge, one ring per provider,
                          │ 86%  │   click-through so it never eats a click
   ┌────────────────────╮ │      │
   │ ✳ Claude Code Usage│ │  ◔   │
   │ Max                │◄┤ 41%  │   hover a ring: a card opens beside it,
   │ 5h session   1h 36m│ │      │   its tail pointing back at the ring
   │ ▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░  │ │  ◕   │
   │ 86% Used           │ │ 72%  │
   └────────────────────╯ ╰──────╯
```

The ring is both gauge and activity light: it fills as the limit burns down
(green, then amber, then red), a cyan comet rides it while an agent is
generating, and it pulses amber when one is blocked on a `[y/N]` prompt.

## What it watches

| Provider | Source | What you get |
| --- | --- | --- |
| **Claude Code** | `GET /api/oauth/usage` with the token from `%USERPROFILE%\.claude\.credentials.json` or Windows Credential Manager; falls back to session transcripts under `%USERPROFILE%\.claude\projects\` | Real 5-hour and 7-day utilisation, reset times, plan, per-project sessions, and whether a session is parked on a permission prompt |
| **Cursor** | `%APPDATA%\Cursor\User\globalStorage\state.vscdb` and per-workspace databases, read without locking them | Plan, account, any cached request counters, live composer sessions per project |
| **Codex** | Rollout transcripts under `%USERPROFILE%\.codex\sessions\` (plus `~/.codex-<profile>`) | The rate-limit snapshot Codex records from the API, token totals, pending tool approvals |
| **GitHub Copilot** | Cached quota payloads under `%LOCALAPPDATA%\github-copilot\`, `~/.config/github-copilot\`, `~/.copilot\` | Plan, signed-in user, chat/completions/premium quota and reset date |
| **Gemini** | `%USERPROFILE%\.gemini\` — settings, OAuth creds, the signed-in account, and per-project logs under `tmp/` | Account, sign-in state, which projects are active and when |
| **Perplexity** | `%APPDATA%\Perplexity\` and `%APPDATA%\Comet\` | Plan and account where the app caches them |
| **Ollama** | `http://127.0.0.1:11434/api/ps` | Resident models, VRAM vs system-RAM split, quantisation, context length |

Gemini and Perplexity keep usage server-side and cache no quota locally, so
their cards say that rather than showing a ring. Both adapters will pick a
quota up automatically if a future build starts caching one.

Two rules hold across every adapter:

- **Read-only.** Nothing is written to, locked, or modified in another tool's
  state. The Cursor adapter parses the SQLite file format directly (including
  committed WAL frames) rather than opening a connection, so a running Cursor
  can never be disturbed and can never block us.
- **No invented numbers.** A failed collection degrades to a visible status —
  `stale`, `sign in`, `rate limited`, `error` — instead of a plausible-looking
  zero. Anything derived locally rather than reported by the provider is marked
  with a `~`.

## Window behaviour

The point of a HUD is that it never gets in the way, which on Windows means
getting five things right:

| Goal | How |
| --- | --- |
| Never steals focus from your IDE or terminal | `WS_EX_NOACTIVATE`, and `SWP_NOACTIVATE` on **every** move and resize |
| Stays out of Alt+Tab and the taskbar | `WS_EX_TOOLWINDOW`, with `WS_EX_APPWINDOW` cleared |
| Stays on top | `HWND_TOPMOST`, re-asserted on every poll (other topmost windows can displace it) |
| Doesn't swallow clicks while resting | `WS_EX_TRANSPARENT` toggled as the popover opens and closes |
| Opens on hover *despite* being click-through | the cursor is polled with `GetCursorPos` |

That last row is the subtle one. A `WS_EX_TRANSPARENT` window receives no mouse
messages at all — not even `mouseenter` — so a click-through notch can never be
told the pointer arrived. Hover is therefore driven from Rust by polling the
cursor against the window rectangle, which is what lets the notch be
click-through and hoverable at the same time.

Appearance uses `DwmSetWindowAttribute` for immersive dark mode, rounded corners
and an accent-tinted border, with the acrylic backdrop left to Tauri's own
window effect so the two mechanisms don't fight. All the Windows 11-era
attributes are best-effort: on Windows 10 they fail harmlessly and the CSS
fallback (`backdrop-filter`) carries the look.

Clicking a provider card raises that tool's window via `SetForegroundWindow`,
using the `AttachThreadInput` dance that Windows requires — without it the call
silently no-ops and the taskbar button just flashes.

## Requirements

For building on Windows (Linux and macOS: see `packaging/`):

- Windows 10 (1809+) or Windows 11, 64-bit
- [Rust](https://rustup.rs/) 1.82+ with the MSVC toolchain
- [Node.js](https://nodejs.org/) 20+
- **Microsoft Visual Studio C++ Build Tools** (the "Desktop development with
  C++" workload) — Rust's MSVC toolchain needs the linker
- **WebView2** — preinstalled on Windows 11 and current Windows 10; the
  installer bundles a bootstrapper otherwise

## Run it

```powershell
git clone https://github.com/k2wh/CodeNotch-Kawh-Edition-.git
cd CodeNotch-Kawh-Edition-

npm install
npm run tauri:dev
```

The notch appears against the right edge of your primary monitor, one ring per
provider. Hover a ring for its detail card; click one to bring that tool's
window to the front. A tray icon appears alongside it: left-click peeks the HUD,
right-click gives you show/hide, refresh, the config folder, and quit.

To produce an installer:

```powershell
npm run tauri:build
```

The NSIS and MSI packages land in
`src-tauri\target\release\bundle\`.

### Iterating on the UI without Windows

The frontend runs standalone in a browser against built-in sample data, which is
the fastest way to work on layout:

```bash
npm run dev      # http://localhost:1420
```

## Tests

The collection logic lives in `codenotch-core`, a crate with no Tauri or GUI
dependency, so its tests run on any host:

```bash
cd src-tauri/core
cargo test          # 171 tests: adapters, SQLite reader, layout, config, collector
cargo clippy --all-targets
```

The SQLite reader is checked against databases produced by real SQLite —
including a WAL that was deliberately left uncheckpointed, and a torn one — so
it is validated against the actual on-disk format rather than our assumptions
about it. Regenerate the fixtures with:

```bash
python3 scripts/gen_test_fixtures.py
```

Type-check the whole Windows application, including the Win32 layer, from any
platform:

```bash
rustup target add x86_64-pc-windows-msvc
cd src-tauri
cargo check --target x86_64-pc-windows-msvc
cargo clippy --target x86_64-pc-windows-msvc --all-targets
```

This works because the crate pulls in no C-compiled dependencies: TLS goes
through schannel via `native-tls`, and SQLite is parsed in pure Rust. The one
external tool it does need is `llvm-rc`, which `tauri-build` shells out to when
compiling the Windows resource file off-Windows (`sudo apt install llvm` on
Debian/Ubuntu); without it the build script panics with
`NotAttempted("llvm-rc")`.

Frontend:

```bash
npm run typecheck
npm run build
```

## Configuration

Settings live at `%APPDATA%\CodeNotch\config.json` and are editable from the
gear at the end of the strip or by hand (unknown keys are ignored, missing ones get defaults, and a
corrupt file falls back to defaults rather than refusing to start).

| Key | Default | Notes |
| --- | --- | --- |
| `edge` / `edgeOffset` / `margin` | `right` / `0.5` / `0` | Which screen edge to pin to and where along it |
| `size` | `medium` | `small`, `medium`, `large` |
| `accent` | `#22d3ee` | Ring and border accent, `#rrggbb` |
| `monitor` | `{"kind":"primary"}` | Or `{"kind":"index","index":1}` |
| `alwaysExpanded` | `false` | Keep a detail card open rather than waiting for hover |
| `clickThroughWhenCollapsed` | `true` | Let clicks fall through while resting |
| `peekOnAttention` / `peekSecs` | `true` / `5` | Briefly expand when an agent finishes or needs you |
| `notifyOnThresholds` | `true` | Alert once at 80% and once at 100% per window |
| `resetAsCountdown` | `true` | `1h 36m` rather than a clock time |
| `launchAtLogin` | `false` | Adds an `HKCU\...\CurrentVersion\Run` entry |
| `poll.*` | 45–120s | Per-provider intervals, clamped to 5–3600s |
| `providers.gemini` etc. | `true` | One switch per provider |
| `providers` | all `true` | Turn individual providers off |
| `ollamaUrl` | `http://127.0.0.1:11434` | Point at a remote or WSL daemon |

## Layout

```
src/                      React frontend (pill, expanded card, settings)
src-tauri/
  src/
    platform/             Win32: styles, docking, DPI, focus, registry
    hud.rs                Collapsed/expanded state and window placement
    commands.rs           The IPC surface
    poll.rs               Background polling loop and attention peeks
    tray.rs               Tray icon and menu
  core/                   codenotch-core -- no Tauri, fully unit-tested
    src/
      adapters/           One module per provider
      sqlite.rs           Read-only SQLite + WAL reader
      layout.rs           Edge anchoring and DPI maths
      collector.rs        Per-provider schedules, staleness, alerts
      model.rs            Domain types
scripts/                  Icon and test-fixture generators
```

## Privacy

Everything is local. The only network calls CodeNotch makes are to Anthropic's
usage endpoint with your existing Claude Code token, and to your own Ollama
daemon. No telemetry, no analytics, no third-party services. Credentials are
read but never copied, logged, or transmitted anywhere other than the provider
they belong to — the Copilot adapter, for instance, reads `apps.json` only to
learn *that* you are signed in and deliberately ignores the tokens beside it.

## Licence

MIT.

Six of the seven provider marks on the strip come from [Simple
Icons](https://simpleicons.org), whose icon data is released under CC0 1.0
(`LICENSES/CC0-1.0.txt`); Codex's is drawn by hand, since OpenAI had theirs
withdrawn from that set. Each logo remains the
trademark of its owner and is used here only to identify which tool a ring
belongs to.
