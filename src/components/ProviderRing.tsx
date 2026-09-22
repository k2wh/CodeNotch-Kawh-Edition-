/**
 * One provider on the strip: the provider's mark inside a track ring, a
 * coloured arc around it for how much of the limit is gone, and the percentage
 * underneath.
 *
 * The arc doubles as the activity indicator — it sweeps while an agent is
 * generating and pulses amber when one is blocked on the user — so a single
 * glance at the strip answers both "how much is left" and "does anything need
 * me".
 */

import { useEffect, useRef, useState } from "react";

import type { Health, ProviderSnapshot } from "../types";
import {
  blockedUntil,
  formatCountdownShort,
  formatPct,
  ringSplit,
  shortWindowLabel,
  usageTone,
} from "../lib/format";
import { useI18n } from "../lib/i18n";
import { BrandIcon } from "./BrandIcon";

/** When the first ring starts appearing: after the strip has slid in. */
const ENTRANCE_BASE_MS = 260;
/** Gap between one ring's entrance and the next. */
const ENTRANCE_STAGGER_MS = 110;
/** How long the arc takes to draw and the number to count up. */
const FILL_MS = 900;

const prefersReducedMotion = () =>
  typeof window !== "undefined" &&
  window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/**
 * Count up from 0 to `target` once `run` turns true, then follow `target`
 * directly: only the first appearance is animated, later updates just land.
 */
function useCountUp(target: number | null, run: boolean): { value: number | null; ms: number } {
  const [value, setValue] = useState(0);
  const [ms, setMs] = useState(FILL_MS);
  // Where the number actually is, which is where the next move starts from —
  // mid-animation, that is not the last target.
  const shown = useRef(0);

  useEffect(() => {
    if (target === null || !run) return;

    const from = shown.current;
    const distance = Math.abs(target - from);
    // Paced by how far it has to go, so a poll nudging 6% to 7% doesn't take
    // as long as the count up from nothing. The arc is given the same figure,
    // so the two never drift apart.
    const duration = Math.min(FILL_MS, 240 + distance * 7);
    setMs(duration);

    if (prefersReducedMotion() || distance < 0.5) {
      shown.current = target;
      setValue(target);
      return;
    }

    let frame = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const progress = Math.min(1, (now - start) / duration);
      // Ease out: quick at first, settling onto the real number.
      const eased = from + (target - from) * (1 - Math.pow(1 - progress, 3));
      shown.current = eased;
      setValue(eased);
      if (progress < 1) frame = requestAnimationFrame(tick);
      else shown.current = target;
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [target, run]);

  return { value: target === null ? null : value, ms };
}

/**
 * The wait until a spent window comes back, ticking on its own.
 *
 * Its own component with its own timer, so a clock on one ring doesn't
 * re-render the whole strip every few seconds — the rings beside it have not
 * changed. Minute granularity, so ten seconds is a fine cadence to keep it
 * honest.
 */
function Countdown({ target }: { target: string }) {
  const [text, setText] = useState(() => formatCountdownShort(target));
  useEffect(() => {
    setText(formatCountdownShort(target));
    const timer = window.setInterval(() => setText(formatCountdownShort(target)), 10_000);
    return () => window.clearInterval(timer);
  }, [target]);
  return <>{text}</>;
}

interface Props {
  provider: ProviderSnapshot;
  /** Position on the strip, which staggers the entrance animation. */
  index: number;
  /** Ring diameter in px, from the backend's metrics. */
  size: number;
  active: boolean;
  /** Underline the ring with the other window. */
  showWeekly: boolean;
  /** Put the weekly quota on the ring and the session limit underneath. */
  weeklyOnRing: boolean;
  /** User-picked `#rrggbb` replacing the traffic light, if any. */
  customColour?: string;
  /** This agent answered moments ago: pulse until it has been seen. */
  answered: boolean;
  /** Pulse while this agent waits on the user. */
  pulseWaiting: boolean;
  onHover: (provider: ProviderSnapshot | null) => void;
  onActivate: (provider: ProviderSnapshot) => void;
}

/**
 * Traffic-light tone for a utilisation level, or the user's colour. Health
 * problems still win over a custom colour: a broken provider must look broken.
 */
function toneColour(pct: number | null, health: Health, custom?: string): string {
  if (health === "unavailable") return "var(--tone-off)";
  if (health === "stale" || health === "rateLimited") return "var(--tone-stale)";
  if (health === "needsAuth" || health === "error") return "var(--tone-bad)";
  if (custom) return custom;
  switch (usageTone(pct)) {
    case "high":
      return "var(--tone-high)";
    case "mid":
      return "var(--tone-mid)";
    default:
      return "var(--tone-low)";
  }
}

export function ProviderRing({
  provider,
  index,
  size,
  active,
  showWeekly,
  weeklyOnRing,
  customColour,
  answered,
  pulseWaiting,
  onHover,
  onActivate,
}: Props) {
  const { t, server } = useI18n();
  const { ringPct: pct, under: weekly } = ringSplit(provider, showWeekly, weeklyOnRing);
  const colour = toneColour(pct, provider.health, customColour);
  const weeklyPct = weekly?.usedPct ?? null;
  const weeklyColour = toneColour(weeklyPct, provider.health, customColour);

  // Entrance: the disc pops in (CSS), then the arc draws, the number counts up
  // and the weekly bar fills. `filled` is the moment the gauges start moving.
  const delay = ENTRANCE_BASE_MS + index * ENTRANCE_STAGGER_MS;
  const [filled, setFilled] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setFilled(true), delay + 160);
    return () => window.clearTimeout(timer);
    // Only on mount: later re-renders keep the ring as it is.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const { value: shownPct, ms: moveMs } = useCountUp(pct, filled);

  // Spent: the percentage is 100 and says nothing, while when it comes back
  // says everything. The ring shows the wait instead.
  const blocked = blockedUntil(provider);

  // Bold enough to read as a gauge across the room, not a hairline. An eighth
  // of the diameter is what the reference uses.
  const stroke = Math.max(3.5, size * 0.125);
  const radius = (size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  const fraction =
    pct === null || !filled ? 0 : Math.min(Math.max(pct, 0), 100) / 100;

  const generating = provider.activity === "generating";
  const waiting = provider.activity === "awaitingInput";
  const iconSize = Math.round(size * 0.385);

  return (
    <button
      type="button"
      className="ring-slot"
      onMouseEnter={() => onHover(provider)}
      onFocus={() => onHover(provider)}
      onClick={() => onActivate(provider)}
      aria-label={
        pct === null
          ? t("ring.unknown", { name: provider.name })
          : t("ring.used", { name: provider.name, pct: Math.round(pct) })
      }
      data-active={active || undefined}
      style={{ "--ring-delay": `${delay}ms` } as React.CSSProperties}
    >
      <span
        className="ring-disc"
        style={{ width: size, height: size }}
        // Waiting on you pulses until you deal with it; a fresh answer pulses
        // for a few seconds. Hovering the ring counts as noticing it.
        data-attention={
          !active &&
          (waiting && pulseWaiting ? "waiting" : answered ? "answered" : undefined)
        }
      >
        <svg
          viewBox={`0 0 ${size} ${size}`}
          width={size}
          height={size}
          className="ring-arc"
          aria-hidden
        >
          {/* Rotate so the arc starts at 12 o'clock and fills clockwise. */}
          <g transform={`rotate(-90 ${size / 2} ${size / 2})`}>
            <circle
              cx={size / 2}
              cy={size / 2}
              r={radius}
              fill="none"
              stroke="var(--ring-track)"
              strokeWidth={stroke}
            />
            {pct !== null && (
              <circle
                cx={size / 2}
                cy={size / 2}
                r={radius}
                fill="none"
                stroke={colour}
                strokeWidth={stroke}
                strokeLinecap="round"
                strokeDasharray={`${circumference * fraction} ${circumference}`}
                className={waiting ? "activity-pulse" : undefined}
                style={{
                  transition: `stroke-dasharray ${moveMs}ms cubic-bezier(0.22, 1, 0.36, 1), stroke 300ms ease`,
                }}
              />
            )}
            {generating && (
              // A short comet riding the ring, so "working" is visible even at
              // a glance across the room.
              <circle
                cx={size / 2}
                cy={size / 2}
                r={radius}
                fill="none"
                stroke="var(--tone-busy)"
                strokeWidth={stroke * 0.8}
                strokeLinecap="round"
                strokeDasharray={`${circumference * 0.16} ${circumference}`}
                className="activity-spin"
              />
            )}
          </g>
        </svg>

        <BrandIcon
          provider={provider.id}
          className="ring-mark"
          // Inline so the mark scales with the ring rather than a fixed step.
          {...{ style: { width: iconSize, height: iconSize } }}
        />
      </span>

      <span
        className="ring-pct tnum"
        data-blocked={blocked ? "" : undefined}
        // A countdown is up to four characters where a percentage is three,
        // so it takes a smaller size rather than overflowing the ring.
        style={{ fontSize: Math.round(size * (blocked ? 0.27 : 0.355)) }}
      >
        {blocked ? <Countdown target={blocked} /> : pct === null ? "—" : formatPct(shownPct ?? 0)}
      </span>

      {weekly && weeklyPct !== null && (
        <span
          className="ring-weekly"
          style={{ width: size * 0.9 }}
          title={`${server(weekly.label)}: ${formatPct(weeklyPct)}`}
        >
          <span className="ring-weekly-track">
            <span
              className="ring-weekly-fill"
              style={{
                width: filled ? `${Math.min(Math.max(weeklyPct, 0), 100)}%` : "0%",
                background: weeklyColour,
              }}
            />
          </span>
          <span className="ring-weekly-label tnum" style={{ fontSize: Math.round(size * 0.17) }}>
            {/* Named after the window itself, so the line reads "7d 17%" or
                "5h 6%" depending on which of the two is down here. */}
            {shortWindowLabel(weekly) || t("ring.weekly")} {formatPct(weeklyPct)}
          </span>
        </span>
      )}
    </button>
  );
}
