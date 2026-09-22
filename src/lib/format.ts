/** Small formatting helpers shared by the pill and the expanded card. */

import type { Translate } from "./i18n";
import type {
  Activity,
  Health,
  ProviderSnapshot,
  UsageUnit,
  UsageWindow,
} from "../types";

/** "42%" — or "—" when there is no percentage to show. */
export function formatPct(pct: number | null | undefined): string {
  if (pct === null || pct === undefined || Number.isNaN(pct)) return "—";
  // Below 1% still reads as 1% rather than 0%, so "barely used" and "unused"
  // don't look identical.
  if (pct > 0 && pct < 1) return "1%";
  return `${Math.round(pct)}%`;
}

/** Compact counts: 1.2k, 3.4M. */
export function formatCount(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (abs >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return `${Math.round(value)}`;
}

/**
 * Money, in the currency these APIs are priced in.
 *
 * Kept in dollars rather than converted: the figure is a list price from a
 * published rate card, and turning it into local currency would need a rate
 * the notch doesn't have and would imply a precision it can't stand behind.
 */
export function formatUsd(usd: number): string {
  if (usd >= 1000) return `$${Math.round(usd).toLocaleString("en-US")}`;
  if (usd >= 10) return `$${usd.toFixed(0)}`;
  if (usd >= 1) return `$${usd.toFixed(2)}`;
  return `$${usd.toFixed(2)}`;
}

/** "3.4x" — how many times over a plan has paid for itself. */
export function formatMultiple(times: number): string {
  return times >= 10 ? `${Math.round(times)}x` : `${times.toFixed(1)}x`;
}

/**
 * How much history a figure rests on: "6h", "3 days".
 *
 * Worth saying plainly. A ratio built on an afternoon is a different claim
 * from one built on a month, and the reader can only tell if told.
 */
export function formatDays(days: number, t: Translate): string {
  if (days < 1) {
    const hours = Math.max(1, Math.round(days * 24));
    return t("time.hoursShort", { count: String(hours) });
  }
  return t("time.daysShort", { count: String(Math.round(days)) });
}

export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[unit]}`;
}

/**
 * The first paragraph of a release's notes, as plain text.
 *
 * The notes are the GitHub release's Markdown; the card only has room for what
 * they lead with, so headings are skipped and links, emphasis and code marks
 * are reduced to their words.
 */
export function summariseNotes(markdown: string | null, max = 240): string | null {
  if (!markdown) return null;
  const paragraph: string[] = [];
  for (const raw of markdown.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) {
      if (paragraph.length) break;
      continue;
    }
    if (line.startsWith("#") || line.startsWith("```")) {
      if (paragraph.length) break;
      continue;
    }
    paragraph.push(line.replace(/^[-*+]\s+/, ""));
  }
  const text = paragraph
    .join(" ")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_`]+/g, "")
    .replace(/\s+/g, " ")
    .trim();
  if (!text) return null;
  if (text.length <= max) return text;
  const cut = text.slice(0, max);
  const lastSpace = cut.lastIndexOf(" ");
  return `${(lastSpace > max * 0.6 ? cut.slice(0, lastSpace) : cut).trimEnd()}…`;
}

/** The headline value for a window: a percentage if there is one, else counts. */
export function windowValue(window: UsageWindow): string {
  const prefix = window.estimated ? "~" : "";
  if (window.usedPct !== null) return prefix + formatPct(window.usedPct);
  if (window.used === null) return "—";
  return prefix + formatUnit(window.used, window.unit);
}

export function formatUnit(value: number, unit: UsageUnit): string {
  switch (unit) {
    case "bytes":
      return formatBytes(value);
    case "tokens":
      return `${formatCount(value)} tok`;
    case "requests":
      return `${formatCount(value)} req`;
    case "credits":
      return `${formatCount(value)} cr`;
    default:
      return formatPct(value);
  }
}

/**
 * "resets in 2h 14m", or a clock time when the user prefers that.
 *
 * Returns null when there is no reset to show, so callers can omit the line
 * entirely rather than render an empty one.
 */
export function formatReset(
  resetsAt: string | null,
  asCountdown: boolean,
  t: Translate,
  locale: string,
  now: number = Date.now(),
): string | null {
  if (!resetsAt) return null;
  const target = new Date(resetsAt).getTime();
  if (Number.isNaN(target)) return null;

  if (!asCountdown) {
    return new Date(target).toLocaleTimeString(locale, {
      hour: "2-digit",
      minute: "2-digit",
    });
  }

  const remaining = target - now;
  if (remaining <= 0) return t("time.resetting");

  const minutes = Math.floor(remaining / 60_000);
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const mins = minutes % 60;

  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${mins}m`;
  if (minutes > 0) return `${minutes}m`;
  return "<1m";
}

/** "3m ago" for a session's last activity. */
export function formatAgo(
  iso: string | null,
  t: Translate,
  now: number = Date.now(),
): string | null {
  if (!iso) return null;
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return null;

  const secs = Math.max(0, Math.round((now - then) / 1000));
  if (secs < 10) return t("time.justNow");
  if (secs < 60) return t("time.secondsAgo", { n: secs });
  const mins = Math.floor(secs / 60);
  if (mins < 60) return t("time.minutesAgo", { n: mins });
  const hours = Math.floor(mins / 60);
  if (hours < 24) return t("time.hoursAgo", { n: hours });
  return t("time.daysAgo", { n: Math.floor(hours / 24) });
}

/** Short human label for a health state. */
export function healthLabel(health: Health, t: Translate): string | null {
  switch (health) {
    case "ok":
      return null;
    case "stale":
      return t("health.stale");
    case "rateLimited":
      return t("health.rateLimited");
    case "needsAuth":
      return t("health.needsAuth");
    case "error":
      return t("health.error");
    case "unavailable":
      return t("health.unavailable");
  }
}

export function activityLabel(activity: Activity, t: Translate): string | null {
  switch (activity) {
    case "generating":
      return t("activity.generating");
    case "awaitingInput":
      return t("activity.awaitingInput");
    case "done":
      return t("activity.done");
    case "idle":
      return null;
  }
}

/**
 * Traffic-light bucket for a utilisation level.
 *
 * Deliberately not a smooth gradient: the point is that a glance tells you
 * which of three buckets you're in, without reading the number.
 */
export function usageTone(pct: number | null): "low" | "mid" | "high" {
  if (pct === null) return "low";
  if (pct >= 70) return "high";
  if (pct >= 40) return "mid";
  return "low";
}

/**
 * The highest utilisation across a provider's quota windows.
 *
 * Mirrors `ProviderSnapshot::peak_pct` in Rust: informational windows are
 * context, not consumption, so they never drive the ring.
 */
export function peakOf(provider: ProviderSnapshot): number | null {
  const quota = provider.windows
    .filter((w) => !w.informational)
    .map((w) => w.usedPct)
    .filter((p): p is number => p !== null);
  return quota.length > 0 ? Math.max(...quota) : null;
}

/**
 * The provider's main weekly quota window, if it reports one. The backend
 * flags weekly windows and orders the all-models one first.
 */
export function weeklyOf(provider: ProviderSnapshot): UsageWindow | null {
  return (
    provider.windows.find((w) => !w.informational && w.usedPct !== null && w.weekly) ??
    null
  );
}

/** The short session window — the fullest one that isn't the weekly quota. */
export function sessionOf(provider: ProviderSnapshot): UsageWindow | null {
  const session = provider.windows.filter(
    (w) => !w.informational && !w.weekly && w.usedPct !== null,
  );
  if (session.length === 0) return null;
  return session.reduce((worst, w) => ((w.usedPct ?? 0) > (worst.usedPct ?? 0) ? w : worst));
}

/**
 * Which window the ring draws, and which one underlines it.
 *
 * The two never show the same thing: whichever is on the ring, the other goes
 * underneath. Which way round is the user's call — a weekly quota is the one
 * that runs out on a Thursday and ruins a week, while the session limit is
 * the one that bites in the next ten minutes, and which of those you want to
 * see at a glance depends on how you work.
 *
 * With nothing to underline with, the ring falls back to the peak of
 * everything, which is what it showed before any of this was optional.
 */
export function ringSplit(
  provider: ProviderSnapshot,
  showSecondary: boolean,
  weeklyOnRing: boolean,
): { ringPct: number | null; under: UsageWindow | null } {
  if (!showSecondary) return { ringPct: peakOf(provider), under: null };

  const weekly = weeklyOf(provider);
  const session = sessionOf(provider);
  const [onRing, below] = weeklyOnRing ? [weekly, session] : [session, weekly];
  if (!below) return { ringPct: peakOf(provider), under: null };

  return { ringPct: onRing?.usedPct ?? peakOf(provider), under: below };
}

/**
 * A reading of 99.7% is drawn as "100%", so it is spent as far as anyone
 * reading the notch is concerned.
 */
const SPENT_PCT = 99.5;

/**
 * When this provider can be used again, if a window is spent.
 *
 * The latest reset among the spent windows, not the earliest: every one of
 * them has to come back before the provider does, and a countdown that
 * expires while you are still locked out is worse than none.
 */
export function blockedUntil(provider: ProviderSnapshot): string | null {
  const spent = provider.windows.filter(
    (w) => !w.informational && (w.usedPct ?? 0) >= SPENT_PCT && w.resetsAt,
  );
  if (spent.length === 0) return null;
  return spent.reduce((latest, w) =>
    new Date(w.resetsAt as string) > new Date(latest.resetsAt as string) ? w : latest,
  ).resetsAt;
}

/**
 * A countdown small enough to sit inside a ring: "2h14", "14m", "<1m".
 *
 * Coarser than the card's, which has room to spell it out. What this has to
 * carry is whether the wait is hours or minutes.
 */
export function formatCountdownShort(target: string, now: number = Date.now()): string {
  const remaining = new Date(target).getTime() - now;
  if (!Number.isFinite(remaining) || remaining <= 0) return "0m";

  const minutes = Math.floor(remaining / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rest = minutes % 60;
    return rest === 0 ? `${hours}h` : `${hours}h${String(rest).padStart(2, "0")}`;
  }
  const days = Math.floor(hours / 24);
  const rest = hours % 24;
  return rest === 0 ? `${days}d` : `${days}d${rest}`;
}

/**
 * A window in two or three characters, for the line under the ring: "7d",
 * "5h". Derived from how long the window runs rather than from its name,
 * which is a sentence in whichever language the provider answered in.
 */
export function shortWindowLabel(window: UsageWindow): string {
  const minutes = window.windowMinutes;
  if (!minutes) return window.weekly ? "7d" : "";
  if (minutes % 1440 === 0) return `${minutes / 1440}d`;
  if (minutes % 60 === 0) return `${minutes / 60}h`;
  return `${minutes}m`;
}
