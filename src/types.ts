/**
 * Mirrors of the Rust types in `src-tauri/core/src/model.rs` and `config.rs`.
 *
 * Both sides serialise camelCase, so these line up field for field. Keep them
 * in sync: the Rust structs are the source of truth.
 */

export type ProviderId =
  | "claudeCode"
  | "cursor"
  | "copilot"
  | "codex"
  | "gemini"
  | "perplexity"
  | "grok"
  | "glm"
  | "kimi"
  | "opencode"
  | "commandCode"
  | "miniMax"
  | "ollama"
  | "lmStudio";

/** Whether a provider's numbers can be trusted right now. */
export type Health =
  | "unavailable"
  | "ok"
  | "stale"
  | "rateLimited"
  | "needsAuth"
  | "error";

/** What a provider's agent is doing. */
export type Activity = "idle" | "done" | "generating" | "awaitingInput";

export type UsageUnit = "percent" | "tokens" | "requests" | "credits" | "bytes";

export interface UsageWindow {
  key: string;
  label: string;
  /** 0..100, or null when the provider gives counts with no denominator. */
  usedPct: number | null;
  used: number | null;
  limit: number | null;
  unit: UsageUnit;
  /** RFC 3339. */
  resetsAt: string | null;
  /** Derived locally rather than reported; rendered with a `~`. */
  estimated: boolean;
  /** Context rather than a quota (e.g. Ollama's GPU share): never coloured as an alarm. */
  informational: boolean;
  /** The provider's weekly (7-day) quota. */
  weekly?: boolean;
  /** How long the window runs, in minutes. */
  windowMinutes?: number | null;
  /** Input-equivalent tokens spent in this window (estimated locally). */
  usedTokens?: number | null;
  /** Input-equivalent tokens the window holds at 100% (estimated locally). */
  capacityTokens?: number | null;
  /** Tokens new to the model this window, and tokens it generated. */
  inputTokens?: number | null;
  outputTokens?: number | null;
  /** The conversation re-read from cache — around 98% of the raw count. */
  cacheTokens?: number | null;
  /** What those tokens would have cost at the API's list price, in USD. */
  usd?: number | null;
}

/** What a plan has returned: list price of its tokens against its fee. */
export interface PlanValue {
  usd: number;
  /** The plan's fee over the same stretch the tokens were counted over. */
  planUsd: number;
  /** How many days of history this rests on. */
  coveredDays: number;
  inputTokens: number;
  outputTokens: number;
}

export interface Session {
  id: string;
  title: string;
  cwd: string | null;
  model: string | null;
  activity: Activity;
  lastActivity: string | null;
  tokens: number | null;
  detail: string | null;
  /**
   * Where the session runs, as its transcript recorded it: `claude-desktop`,
   * `claude-vscode`, `cli`, `Codex Desktop`, `codex_vscode`, `codex_exec`, …
   */
  host?: string | null;
}

export interface ProviderSnapshot {
  /** What this ring is: the tool. Drives the icon and the click target. */
  id: ProviderId;
  /** Which ring this is: `claudeCode`, or `claudeCode:work` for a second
   *  account. Everything kept per ring is keyed by this. */
  key: string;
  /** The account's own name, when the tool has more than one. */
  accountKey?: string | null;
  name: string;
  health: Health;
  activity: Activity;
  detail: string | null;
  source: string | null;
  account: string | null;
  windows: UsageWindow[];
  sessions: Session[];
  updatedAt: string;
  retryAt: string | null;
  /** Absent unless the token estimate is on and the plan has a price set. */
  value?: PlanValue | null;
}

export interface Telemetry {
  providers: ProviderSnapshot[];
  generatedAt: string;
  peakPct: number | null;
  activity: Activity;
  health: Health;
  /** Rings that just asked for attention with their agent's window already in
   *  front: the news is on screen, so nothing announces it. */
  inFront?: string[];
  /** Every extra account found, switched on or off. Only those switched on
   *  have a ring; settings lists them all so any can be switched back on. */
  accounts?: ExtraAccount[];
}

/** An account of a tool beyond its default one (`~/.claude-work`, say). */
export interface ExtraAccount {
  /** The ring's key: `claudeCode:work`. */
  key: string;
  id: ProviderId;
  /** As its ring's card is titled: `Claude Code · work`. */
  name: string;
}

export type Edge = "top" | "bottom" | "left" | "right";
export type HudSize = "small" | "medium" | "large";

export type MonitorChoice =
  | { kind: "primary" }
  | { kind: "index"; index: number };

export interface PollConfig {
  claudeSecs: number;
  cursorSecs: number;
  copilotSecs: number;
  codexSecs: number;
  geminiSecs: number;
  perplexitySecs: number;
  grokSecs: number;
  glmSecs: number;
  kimiSecs: number;
  opencodeSecs: number;
  commandCodeSecs: number;
  ollamaSecs: number;
  lmStudioSecs: number;
  miniMaxSecs: number;
  activitySecs: number;
}

export interface Config {
  edge: Edge;
  edgeOffset: number;
  margin: number;
  size: HudSize;
  accent: string;
  monitor: MonitorChoice;

  alwaysExpanded: boolean;
  hidden: boolean;
  clickThroughWhenCollapsed: boolean;
  peekOnAttention: boolean;
  peekSecs: number;
  notifyOnThresholds: boolean;
  /** Providers that announce nothing, while still being polled and drawn. */
  mutedProviders?: string[];
  /** Chime when an agent answers or starts waiting on you. */
  notifySound?: boolean;
  /** Pulse a halo around the ring for the same moments. */
  notifyPulse?: boolean;
  /** Which chime: `chime`, `ping`, `bell`, `marimba`, … */
  notifySoundId?: string;
  /** The chime when an agent stops to ask you something; "none" silences it. */
  notifyWaitingSoundId?: string;
  /** The chime when a window runs out; "none" silences it. */
  notifyLimitSoundId?: string;
  /** The chime when it resets and the provider is usable again. */
  notifyRecoverSoundId?: string;
  /** Chime loudness, 0–100. */
  notifyVolume?: number;
  resetAsCountdown: boolean;
  launchAtLogin: boolean;

  poll: PollConfig;
  providers: Record<string, boolean>;
  providerOrder: string[];
  ollamaUrl: string;
  /** Which MiniMax service the account is on: `international` or `china`. */
  minimaxRegion?: string;

  showWeekly: boolean;
  /** Weekly quota on the ring, session limit on the line under it. */
  weeklyOnRing?: boolean;
  /** Work out what each window is worth in tokens. */
  estimateTokens?: boolean;
  /** `#rrggbb` per provider id; absent means traffic-light colours. */
  ringColors: Record<string, string>;
  /** What each plan costs per month in USD, per provider id. Set one and the
   *  card shows what that plan has returned in tokens. */
  planUsd?: Record<string, number>;

  /** Stay behind a full-screen app in front instead of floating over it. */
  stayBelowFullscreen: boolean;
  /** Executables (e.g. `chrome.exe`) the notch stays behind while in front. */
  stayBelowApps: string[];

  /** "auto" (follow Windows), "en", "pt" or "es". */
  language?: string;
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Where the webview drew the strip and the card. Mirrors `Regions` in Rust. */
export interface HudRegions {
  strip: Rect;
  popover: Rect | null;
}

export interface HudState {
  /** The notch is expanded: hovered, pinned, peeking or being edited. */
  open: boolean;
  pinned: boolean;
  hidden: boolean;
  peeking: boolean;
  /** Tucked into the edge: a game, a video or a listed app is in front. */
  tucked?: boolean;
  /** Logical window size, so the webview can place the strip within it. */
  width: number;
  height: number;
}

/**
 * Strip and popover dimensions, sent by the backend so both sides draw to the
 * same numbers the window is sized with. Mirrors `HudMetrics` in Rust.
 */
export interface HudMetrics {
  stripThickness: number;
  slot: number;
  stripPadding: number;
  ring: number;
  popoverSize: number;
  popoverGap: number;
}

/** An open app window, for the "stay behind" picker. Mirrors `OpenWindow`. */
export interface OpenWindow {
  id: string;
  title: string;
  /** Executable, e.g. `chrome.exe`; selection is per app. */
  exe: string;
  /** PNG data URL, or null when minimised or uncapturable. */
  thumbnail: string | null;
  minimized: boolean;
}

export interface MonitorInfo {
  index: number;
  label: string;
  width: number;
  height: number;
  primary: boolean;
}

export interface Bootstrap {
  config: Config;
  telemetry: Telemetry;
  hud: HudState;
  metrics: HudMetrics;
  edge: Edge;
  version: string;
  /** False on non-Windows dev builds, where the Win32 layer is a no-op. */
  nativeWindow: boolean;
}

/** Providers whose card can raise a real window when clicked. */
export const FOCUSABLE: ReadonlySet<ProviderId> = new Set<ProviderId>([
  "claudeCode",
  "cursor",
  "copilot",
  "codex",
  "gemini",
  "perplexity",
]);
