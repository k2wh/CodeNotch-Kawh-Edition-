/**
 * Sample data for running the UI in a plain browser (`npm run dev`).
 *
 * Outside the Tauri shell there is no backend to talk to, so the HUD would
 * render empty and be impossible to iterate on. This fills it with a realistic
 * mix: healthy numbers, a session waiting for approval, a rate limit, a
 * signed-out provider and one that isn't installed.
 */

import { useCallback, useEffect, useState } from "react";

import type { Bootstrap, Config, HudMetrics, Telemetry, UpdateStatus } from "../types";

const now = Date.now();
const iso = (offsetMs: number) => new Date(now + offsetMs).toISOString();

/** Mirrors HudSize::Medium in Rust. */
export const DEMO_METRICS: HudMetrics = {
  stripThickness: 92,
  slot: 135,
  stripPadding: 48,
  ring: 58,
  popoverSize: 320,
  popoverGap: 50,
};

export const DEMO_CONFIG: Config = {
  edge: "right",
  edgeOffset: 0.5,
  margin: 0,
  size: "medium",
  accent: "#22d3ee",
  monitor: { kind: "primary" },
  alwaysExpanded: false,
  hidden: false,
  clickThroughWhenCollapsed: true,
  peekOnAttention: true,
  peekSecs: 5,
  notifyOnThresholds: true,
  mutedProviders: [],
  notifySound: true,
  notifyPulse: true,
  notifySoundId: "chime",
  notifyWaitingSoundId: "ping",
  notifyLimitSoundId: "drop",
  notifyRecoverSoundId: "arp",
  notifyVolume: 70,
  resetAsCountdown: true,
  launchAtLogin: false,
  poll: {
    claudeSecs: 90,
    cursorSecs: 45,
    copilotSecs: 120,
    codexSecs: 45,
    geminiSecs: 60,
    perplexitySecs: 120,
    grokSecs: 300,
    glmSecs: 180,
    kimiSecs: 180,
    opencodeSecs: 180,
    commandCodeSecs: 300,
    ollamaSecs: 10,
    lmStudioSecs: 10,
    miniMaxSecs: 180,
    activitySecs: 3,
  },
  providers: {
    claudeCode: true,
    cursor: true,
    copilot: true,
    codex: true,
    gemini: true,
    perplexity: true,
    grok: true,
    glm: true,
    kimi: true,
    opencode: true,
    commandCode: true,
    miniMax: true,
    ollama: true,
    lmStudio: true,
  },
  providerOrder: [
    "claudeCode",
    "cursor",
    "copilot",
    "codex",
    "gemini",
    "perplexity",
    "grok",
    "glm",
    "kimi",
    "opencode",
    "commandCode",
    "miniMax",
    "ollama",
    "lmStudio",
  ],
  ollamaUrl: "http://127.0.0.1:11434",
  minimaxRegion: "international",
  showWeekly: true,
  weeklyOnRing: false,
  estimateTokens: true,
  ringColors: {},
  planUsd: { claudeCode: 200 },
  stayBelowFullscreen: false,
  stayBelowApps: [],
  language: "auto",
};

export const DEMO_TELEMETRY: Telemetry = {
  generatedAt: iso(0),
  peakPct: 86,
  activity: "awaitingInput",
  health: "rateLimited",
  providers: [
    {
      id: "claudeCode",
      key: "claudeCode",
      name: "Claude Code",
      health: "ok",
      activity: "awaitingInput",
      detail: null,
      source: "oauth",
      account: "Max",
      updatedAt: iso(-4_000),
      retryAt: null,
      value: {
        usd: 612.4,
        planUsd: 200,
        coveredDays: 30,
        inputTokens: 412_000_000,
        outputTokens: 6_800_000,
      },
      windows: [
        {
          key: "five_hour",
          label: "5h session",
          usedPct: 86,
          used: null,
          limit: null,
          unit: "percent",
          resetsAt: iso(97 * 60_000),
          estimated: false,
          informational: false,
          inputTokens: 1_447_000,
          outputTokens: 236_000,
          cacheTokens: 105_564_000,
          usd: 73.17,
        },
        {
          key: "seven_day",
          label: "7d all models",
          usedPct: 34,
          used: null,
          limit: null,
          unit: "percent",
          resetsAt: iso(3.4 * 24 * 3600_000),
          estimated: false,
          informational: false,
        },
      ],
      sessions: [
        {
          id: "a1",
          title: "codenotch",
          cwd: "C:\\dev\\codenotch",
          model: "claude-opus-5",
          activity: "awaitingInput",
          lastActivity: iso(-38_000),
          tokens: 184_320,
          detail: null,
        },
        {
          id: "a2",
          title: "billing-api",
          cwd: "C:\\dev\\billing-api",
          model: "claude-opus-5",
          activity: "generating",
          lastActivity: iso(-4_000),
          tokens: 42_100,
          detail: null,
        },
      ],
    },
    {
      id: "cursor",
      key: "cursor",
      name: "Cursor",
      health: "ok",
      activity: "generating",
      detail: null,
      source: "state.vscdb",
      account: "Pro · dev@example.com",
      updatedAt: iso(-12_000),
      retryAt: null,
      windows: [
        {
          key: "gpt-4",
          label: "Requests",
          usedPct: 41,
          used: 205,
          limit: 500,
          unit: "requests",
          resetsAt: iso(11 * 24 * 3600_000),
          estimated: false,
          informational: false,
        },
      ],
      sessions: [
        {
          id: "c1",
          title: "Refactor auth middleware",
          cwd: "web-app",
          model: null,
          activity: "generating",
          lastActivity: iso(-9_000),
          tokens: null,
          detail: "web-app",
        },
      ],
    },
    {
      id: "copilot",
      key: "copilot",
      name: "GitHub Copilot",
      health: "ok",
      activity: "idle",
      detail: null,
      source: "github-copilot cache",
      account: "Individual · octocat",
      updatedAt: iso(-60_000),
      retryAt: null,
      windows: [
        {
          key: "premium_interactions",
          label: "Premium",
          usedPct: 72,
          used: 216,
          limit: 300,
          unit: "requests",
          resetsAt: iso(9 * 24 * 3600_000),
          estimated: false,
          informational: false,
        },
      ],
      sessions: [],
    },
    {
      id: "codex",
      key: "codex",
      name: "Codex",
      health: "rateLimited",
      activity: "idle",
      detail: "Rate limited; retrying shortly (showing figures from 4m ago)",
      source: "sessions",
      account: null,
      updatedAt: iso(-240_000),
      retryAt: iso(90_000),
      windows: [
        {
          key: "primary",
          label: "5h",
          usedPct: 18,
          used: null,
          limit: null,
          unit: "percent",
          resetsAt: iso(140 * 60_000),
          estimated: false,
          informational: false,
        },
      ],
      sessions: [],
    },
    {
      id: "gemini",
      key: "gemini",
      name: "Gemini",
      health: "ok",
      activity: "idle",
      detail: "Gemini reports quota server-side; no local counters",
      source: ".gemini",
      account: "dev@example.com",
      updatedAt: iso(-30_000),
      retryAt: null,
      windows: [],
      sessions: [
        {
          id: "g1",
          title: "docs-site",
          cwd: "docs-site",
          model: null,
          activity: "done",
          lastActivity: iso(-120_000),
          tokens: null,
          detail: "14 messages",
        },
      ],
    },
    {
      id: "perplexity",
      key: "perplexity",
      name: "Perplexity",
      health: "ok",
      activity: "idle",
      detail: null,
      source: "app state",
      account: "Pro · dev@example.com",
      updatedAt: iso(-45_000),
      retryAt: null,
      windows: [
        {
          key: "quota",
          label: "Pro searches",
          usedPct: 52,
          used: 156,
          limit: 300,
          unit: "requests",
          resetsAt: iso(9 * 3600_000),
          estimated: false,
          informational: false,
        },
      ],
      sessions: [],
    },
    {
      id: "ollama",
      key: "ollama",
      name: "Ollama",
      health: "ok",
      activity: "idle",
      detail: null,
      source: "api/ps",
      account: "v0.5.7",
      updatedAt: iso(-3_000),
      retryAt: null,
      windows: [
        {
          key: "resident",
          label: "Resident",
          usedPct: null,
          used: 21_474_836_480,
          limit: null,
          unit: "bytes",
          resetsAt: null,
          estimated: false,
          informational: true,
        },
        {
          key: "vram",
          label: "On GPU",
          usedPct: 93,
          used: null,
          limit: null,
          unit: "percent",
          resetsAt: null,
          estimated: false,
          informational: true,
        },
      ],
      sessions: [
        {
          id: "qwen",
          title: "qwen2.5-coder:32b",
          cwd: null,
          model: "qwen2.5-coder:32b",
          activity: "idle",
          lastActivity: null,
          tokens: 32_768,
          detail: "18.6 GB · 93% GPU · Q4_K_M",
        },
      ],
    },
  ],
};

export const DEMO_BOOTSTRAP: Bootstrap = {
  config: DEMO_CONFIG,
  telemetry: DEMO_TELEMETRY,
  hud: {
    open: false,
    pinned: false,
    hidden: false,
    peeking: false,
    width: DEMO_METRICS.stripThickness,
    // Room for the demo's rings, the update ring and the gear.
    height:
      DEMO_METRICS.stripPadding * 2 +
      (DEMO_TELEMETRY.providers.filter((p) => p.health !== "unavailable").length + 1) *
        DEMO_METRICS.slot +
      30,
  },
  metrics: DEMO_METRICS,
  edge: "right",
  // A version the changelog has notes for, so the browser shows the card a
  // first launch after an update would.
  version: "0.2.0",
  nativeWindow: false,
  whatsNew: "0.2.0",
};

/** The pretend release the demo downloads. */
const DEMO_RELEASE = {
  version: "0.2.1",
  total: 6_400_000,
  notes:
    "The notch updates itself: a new ring downloads the next version in the background, and a click installs it.\n\n## Install\n\n- Windows…",
};

const DEMO_IDLE: UpdateStatus = {
  phase: "idle",
  current: "0.2.0",
  version: null,
  notes: null,
  downloaded: 0,
  total: null,
  error: null,
  checkedAt: null,
  dismissed: false,
};

/**
 * A release playing through every step in a bare browser: found a moment
 * after the page opens, downloaded over a few seconds, ready, and installed
 * on a click, the way the backend would report them.
 */
export function useDemoUpdate(enabled: boolean): {
  status: UpdateStatus | null;
  act: () => void;
  later: () => void;
} {
  const [status, setStatus] = useState<UpdateStatus | null>(enabled ? DEMO_IDLE : null);

  useEffect(() => {
    if (!enabled) return;
    const found = window.setTimeout(() => {
      setStatus({
        ...DEMO_IDLE,
        phase: "downloading",
        version: DEMO_RELEASE.version,
        notes: DEMO_RELEASE.notes,
        total: DEMO_RELEASE.total,
      });
    }, 1500);
    return () => window.clearTimeout(found);
  }, [enabled]);

  const downloading = status?.phase === "downloading";
  useEffect(() => {
    if (!downloading) return;
    const timer = window.setInterval(() => {
      setStatus((current) => {
        if (!current || current.phase !== "downloading") return current;
        const downloaded = Math.min(DEMO_RELEASE.total, current.downloaded + 160_000);
        return downloaded >= DEMO_RELEASE.total
          ? { ...current, phase: "ready", downloaded }
          : { ...current, downloaded };
      });
    }, 150);
    return () => window.clearInterval(timer);
  }, [downloading]);

  const act = useCallback(() => {
    setStatus((current) =>
      current?.phase === "ready" ? { ...current, phase: "installing" } : current,
    );
  }, []);

  // The app would close here and come back as the new version.
  const installing = status?.phase === "installing";
  useEffect(() => {
    if (!installing) return;
    const done = window.setTimeout(() => setStatus(DEMO_IDLE), 2500);
    return () => window.clearTimeout(done);
  }, [installing]);

  const later = useCallback(() => {
    setStatus((current) => (current ? { ...current, dismissed: true } : current));
  }, []);

  return { status, act, later };
}
