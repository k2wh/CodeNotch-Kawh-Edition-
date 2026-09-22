# CodeNotch (Kawhã Edition)

A lightweight always-on-top HUD for **Windows, Linux and macOS** that shows how
much of each AI coding assistant's usage limit you have burned, when the window
resets, and whether a background agent is generating, finished, or **waiting
for your approval**.

This edition builds on [Rohan's CodeNotch for
Windows](https://github.com/Rohanx04/CodeNotch), an open-source alternative to
the macOS [codenotch](https://github.com/vinzdg/codenotch), rebuilt on Tauri v2
with a Rust backend and a React frontend. See [what this edition
adds](#what-this-edition-adds).

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

## What this edition adds

Compared with the Windows port it started from:

**Providers and accounts**

- Seven more providers, fourteen in all: Grok, GLM (Z.ai and BigModel), Kimi,
  OpenCode, Command Code, MiniMax and, running on your machine, LM Studio. Each
  borrows the login its own tool already made; none signs in by itself.
- Claude's limits as Anthropic names them, the weekly limit per model included,
  beside the 5-hour and all-models weekly ones.
- A ring per account. A second Claude Code or Codex account (`~/.claude-work`,
  `~/.codex-<profile>`) gets its own ring, colour, switch, mute and token count,
  so one account's spending is never counted against another's limit. An
  account switched off stays in settings to be switched back on, and an extra
  Claude account only ever uses its own login.

**Tokens, and what a plan returns**

- The tokens spent in each window, the 5-hour and the weekly alike: new input,
  output and conversation re-read from cache, apart, with what they would have
  cost at list price. Each message is counted once, though Claude Code writes
  it on several lines.
- How big each window is in tokens (`~120.5M / 631.0M`), worked out from what
  this machine spent against the percentage the provider reports. It stays
  blank until that can be known rather than guessing, and a 5-hour window never
  claims more than the week around it.
- What the subscription returns: set what a plan costs a month and the card
  prices the tokens it covered at the API's rates, against the fee for the days
  actually counted: `$1,284`, `6.4x` the plan.

**The rings and their colours**

- The other window as a line under each ring (the week under the 5-hour limit),
  and a switch to swap which one is on the ring.
- A spent ring counts down to getting the limit back (`2h14`) instead of
  sitting at 100%.
- A colour of your choosing per provider, or the traffic-light green, amber and
  red; Codex wears OpenAI's mark, in its ring's colour.
- Providers reorder by dragging in settings, Alt-drag slides the notch along
  its edge, and it enters with some motion: the strip slides in, the rings pop
  in turn and the numbers count up.

**Notifications**

- A chime for each kind of news: an agent finished, an agent is waiting on you,
  a limit ran out, a limit came back. Eight synthesised sounds with a volume and
  a preview; all but "finished" can be silenced one by one.
- The ring pulses amber while an agent waits on you and green when one answers.
- Mute per provider: still watched and drawn, never heard from.
- Quiet when you're already there: no chime, pulse or peek for an agent whose
  window is in front.
- What an agent is doing comes from its own records, the end of a turn or a
  permission prompt, rather than from a file going quiet, which missed a short
  answer between two reads.

**Agent hooks**

- Claude Code and Codex can run a command on every event: a tool starting, a
  permission prompt, a turn ending. With hooks on, they tell the notch the
  moment it happens instead of it finding out from the transcripts a second or
  two later, or not at all for a quick turn.
- The hook is CodeNotch itself (`codenotch --hook <agent>`), so there is nothing
  to install or keep in step. It appends the event to a queue in
  `~/.codenotch/` and exits at once, notch running or not, and the notch reads
  the queue twice a second. The queue sits where an agent inside a packaged
  app, Claude's desktop app among them, can still reach it.
- The agents' own settings (`~/.claude/settings.json`, `~/.codex/hooks.json`)
  are merged, never replaced, and the hooks are pointed back at the app after
  an update or a move. On Windows the installer sets them up and the
  uninstaller takes them out; anywhere, it's the "Instant agent updates" switch
  in settings.

**Getting around**

- A click on a ring raises the app the agent runs in: Claude's app, the VS Code
  window of that project, a terminal, the Codex app. A second click goes back
  to where you were, and on Windows Claude's or the Codex app opens if it was
  closed.
- The notch never takes the focus: clicking it doesn't take the keyboard from
  your app, and it stays out of Alt+Tab.
- It stays behind full-screen apps (a game, a video) and behind apps you pick
  from a list of open windows with live thumbnails, sliding into the edge and
  back out.

**Keeping itself up to date**

- A ring of its own, in the accent colour, when a release is out: it fills as
  the new version downloads in the background, and a click installs it and
  brings the notch back. Nothing installs on its own.
- Every download is checked against this project's signing key before it runs,
  so a file that wasn't signed for this app is refused.
- Settings say how updates arrive — downloaded, only announced, or never — and
  can look for one now.
- What changed, in the notch's own language, the first time a new version
  opens, and from the version in settings afterwards.

**Everywhere else**

- Linux (X11) and macOS versions, as well as Windows.
- English, Portuguese and Spanish, following the system's language.
- Settings grouped by what you came to change: appearance, the notch, alerts,
  limits, system.
- An installer with CodeNotch's icon and artwork.
- Lighter at rest, about 60–120 MB instead of ~350 MB on Windows, and gentler
  on the providers. Claude is asked every five minutes, as the macOS original
  does, and a rate-limit back-off survives a restart.

## What it watches

`~` is your home folder (`%USERPROFILE%` on Windows), and a tool's app data is
under `%APPDATA%` on Windows, `~/.config` on Linux and
`~/Library/Application Support` on a Mac.

| Provider | Source | What you get |
| --- | --- | --- |
| **Claude Code** | `GET /api/oauth/usage` with the token from `~/.claude/.credentials.json`, Windows Credential Manager or the macOS keychain; falls back to session transcripts under `~/.claude/projects/`, and, where Claude Code never signed in, to the Claude desktop app's own `plan-usage-history.json`. Extra accounts in `~/.claude-<name>` | Real 5-hour and weekly utilisation, the weekly limit per model, reset times, plan, per-project sessions, and whether a session is parked on a permission prompt |
| **Cursor** | Cursor's `User/globalStorage/state.vscdb` and per-workspace databases in its app data, read without locking them | Plan, account, any cached request counters, live composer sessions per project |
| **Codex** | Rollout transcripts under `~/.codex/sessions/` (plus `~/.codex-<profile>`) | The rate-limit snapshot Codex records from the API, token totals, pending tool approvals |
| **GitHub Copilot** | Cached quota payloads in `github-copilot` under the local app data (`%LOCALAPPDATA%` on Windows), `~/.config/github-copilot/` and `~/.copilot/` | Plan, signed-in user, chat/completions/premium quota and reset date |
| **Gemini** | `~/.gemini/`: settings, OAuth creds, the signed-in account, and per-project logs under `tmp/` | Account, sign-in state, which projects are active and when |
| **Perplexity** | `Perplexity` and `Comet` in the app data | Plan and account where the app caches them |
| **Grok** | The Grok CLI's session in `~/.grok/auth.json`, x.ai's entries only, sent to the CLI's own billing proxy | The share of the week used, overall or per product |
| **GLM** | A Z.ai or BigModel key held by Claude Code (only when its base URL is z.ai's or bigmodel.cn's), ZCode or OpenCode | 5-hour and weekly limits, on token and credit plans |
| **Kimi** | The Kimi CLI's short-lived token in `~/.kimi-code/credentials/`, read afresh on every poll | The coding plan's usage and reset times |
| **OpenCode** | Only the `opencode-go` entry of OpenCode's `auth.json` | The Go plan's monthly window |
| **Command Code** | `~/.commandcode/auth.json`, or `COMMAND_CODE_API_KEY` | What the month has cost against what the plan has left |
| **MiniMax** | A key in `MINIMAX_API_KEY`, or a cookie you pasted into `MINIMAX_COOKIE`, asked of the region set in `minimaxRegion` | The coding plan's usage and reset |
| **Ollama** | `http://127.0.0.1:11434/api/ps` | Resident models, VRAM vs system-RAM split, quantisation, context length |
| **LM Studio** | Its local server, at the address in LM Studio's own settings | Loaded models, their size and context, like Ollama |

Gemini and Perplexity keep usage server-side and cache no quota locally, so
their cards say that rather than showing a ring. Both adapters will pick a
quota up automatically if a future build starts caching one.

Two rules hold across every adapter:

- **Read-only.** Nothing is written to, locked, or modified in another tool's
  state. The Cursor adapter parses the SQLite file format directly (including
  committed WAL frames) rather than opening a connection, so a running Cursor
  can never be disturbed and can never block us. The one exception is the
  [agent hooks](#what-this-edition-adds), merged into Claude Code's and Codex's
  settings only while they are switched on.
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

The window itself draws nothing: no backdrop, no rounded corners, no border
(`DwmSetWindowAttribute`), so the only thing on screen is the notch the page
paints. It is a fixed size, clipped with `SetWindowRgn` to what is drawn,
because resizing a WebView2 window on hover stretches its last frame. Tao, the
window layer under Tauri, rewrites a window's extended styles whenever one of
its own flags changes, showing the window included, so the notch's window is
subclassed and every style change goes through one function that keeps the bits
above.

The Linux and macOS versions reach the same goals their own way: EWMH hints and
a SHAPE input region on X11, a non-activating `NSPanel` at the menu bar's level
on a Mac. See [packaging/linux](packaging/linux/README.md) and
[packaging/macos](packaging/macos/README.md); how a release is built and signed
is in [packaging/releasing.md](packaging/releasing.md).

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
cargo test          # adapters, SQLite reader, layout, config, collector, ledger
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

Settings live in `config.json` in CodeNotch's folder of the app data
(`%APPDATA%\CodeNotch` on Windows, `~/.config/CodeNotch` on Linux,
`~/Library/Application Support/CodeNotch` on a Mac) and are editable from the
gear at the end of the strip or by hand (unknown keys are ignored, missing ones
get defaults, and a corrupt file falls back to defaults rather than refusing to
start).

| Key | Default | Notes |
| --- | --- | --- |
| `edge` / `edgeOffset` / `margin` | `right` / `0.5` / `0` | Which screen edge to pin to and where along it |
| `size` | `medium` | `small`, `medium`, `large` |
| `accent` | `#22d3ee` | Accent colour, `#rrggbb` |
| `ringColors` | none | A ring colour per provider, `#rrggbb`; without one, green, amber and red |
| `monitor` | `{"kind":"primary"}` | Or `{"kind":"index","index":1}` |
| `language` | `auto` | Follow the system, or `en`, `pt`, `es` |
| `alwaysExpanded` | `false` | Keep a detail card open rather than waiting for hover |
| `clickThroughWhenCollapsed` | `true` | Let clicks fall through while resting |
| `peekOnAttention` / `peekSecs` | `true` / `5` | Briefly expand when an agent finishes or needs you |
| `notifyOnThresholds` | `true` | Alert once at 80% and once at 100% per window |
| `notifySound` / `notifyVolume` | `true` / `70` | Chimes, and how loud, 0–100 |
| `notifySoundId` / `notifyWaitingSoundId` / `notifyLimitSoundId` / `notifyRecoverSoundId` | `chime` / `ping` / `drop` / `arp` | The chime for an agent finishing, one waiting on you, a limit running out and one coming back: `chime`, `ping`, `bell`, `marimba`, `arp`, `drop`, `glass`, `pulse`, and for all but the first, `none` |
| `notifyPulse` | `true` | Pulse the ring for the same moments |
| `mutedProviders` | none | Providers still drawn but never announced |
| `showWeekly` / `weeklyOnRing` | `true` / `false` | The other window as a line under the ring, and whether the week has the ring |
| `resetAsCountdown` | `true` | `1h 36m` rather than a clock time |
| `estimateTokens` | `true` | Token counts and window sizes, and with `planUsd`, what a plan returns |
| `planUsd` | none | What a plan costs a month in US$, by provider: `{"claudeCode": 200}` |
| `stayBelowFullscreen` / `stayBelowApps` | `false` / none | Stay behind full-screen apps, and behind these apps while they're in front |
| `launchAtLogin` | `false` | Start at sign-in: a `Run` registry entry on Windows, an autostart entry on Linux, a launch agent on a Mac |
| `updates` | `auto` | How a new release arrives: `auto` downloads it in the background, `notify` only says it's out, `off` never looks. Installing always waits for a click |
| `poll.*` | 10–300s | Per-provider intervals, clamped to 5–3600s; Claude's is never under 300s |
| `providers` | all `true` | Turn individual providers, or accounts, off |
| `providerOrder` | all | The order of the rings |
| `ollamaUrl` | `http://127.0.0.1:11434` | Point at a remote or WSL daemon |
| `minimaxRegion` | `international` | Or `china`, MiniMax's other service |

## Layout

```
src/                      React frontend (pill, expanded card, settings)
src-tauri/
  src/
    platform/             One layer per system: Win32, X11, AppKit
    hud.rs                Collapsed/expanded state and window placement
    focus.rs              Which window is in front, and the way back to it
    hooks.rs              Agent hooks and their queue
    commands.rs           The IPC surface
    poll.rs               Background polling loop and attention peeks
    tray.rs               Tray icon and menu
  core/                   codenotch-core -- no Tauri, fully unit-tested
    src/
      adapters/           One module per provider
      ledger.rs           Tokens spent, per window and per account
      estimate.rs         How big a window is in tokens
      prices.rs           List prices, for what a plan returns
      sqlite.rs           Read-only SQLite + WAL reader
      layout.rs           Edge anchoring and DPI maths
      collector.rs        Per-provider schedules, staleness, alerts
      model.rs            Domain types
packaging/                Linux (.deb, in Docker) and macOS builds
scripts/                  Icon and test-fixture generators
```

## Privacy

Everything runs on your machine. CodeNotch asks GitHub for this project's
latest release every six hours, which is the only request that isn't about a
provider — `updates: "off"` in the settings stops it. Its other network calls
are to each provider's own usage endpoint, with the login its tool already
holds:
Anthropic's for Claude Code, the Grok CLI's billing proxy, Z.ai or BigModel,
Kimi, OpenCode, Command Code and MiniMax, only for the providers switched on
and signed in. Ollama and LM Studio are asked on your own machine, or wherever
you point them. No telemetry, no analytics, no third-party services.
Credentials are read but never copied, logged, or transmitted anywhere other
than the provider they belong to. The Copilot adapter, for instance, reads
`apps.json` only to learn *that* you are signed in and deliberately ignores the
tokens beside it; OpenCode's file holds your keys for other vendors, and only
its own entry is read; a key in Claude Code's settings goes to Z.ai or
BigModel only when Claude Code is itself pointed there.

## Licence

MIT.

Six of the provider marks on the strip come from [Simple
Icons](https://simpleicons.org), whose icon data is released under CC0 1.0
(`LICENSES/CC0-1.0.txt`); Codex's is drawn by hand, since OpenAI had theirs
withdrawn from that set. The marks for GLM, Kimi, OpenCode, Command Code,
MiniMax and LM Studio come from the asset set of the [macOS
original](https://github.com/vinzdg/codenotch), under the MIT licence
(`LICENSES/MIT-codenotch-macos.txt`), and Grok shows its initials. Each logo
remains the trademark of its owner and is used here only to identify which tool
a ring belongs to.
