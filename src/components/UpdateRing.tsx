/**
 * One more ring on the strip while a new release is on its way: the same
 * track and arc as a provider's ring, in the accent colour.
 *
 * The arc fills as the download comes in, the mark in the middle says which
 * step it's at (an arrow while it downloads, a circular arrow once it's ready,
 * a box while it installs), and the line underneath says what a click does.
 */

import { Download, Package, RefreshCw, TriangleAlert } from "lucide-react";

import type { UpdateStatus } from "../types";
import { formatPct } from "../lib/format";
import { useI18n } from "../lib/i18n";

/** The key the update ring goes by, beside the providers' own. */
export const UPDATE_KEY = "update";

/** Whether there is anything for the ring to say. */
export function updateRingVisible(status: UpdateStatus | null): status is UpdateStatus {
  if (!status || status.dismissed || !status.version) return false;
  return (
    status.phase === "available" ||
    status.phase === "downloading" ||
    status.phase === "ready" ||
    status.phase === "installing" ||
    status.phase === "failed"
  );
}

/** How much of the download is in, 0–1, or null when there is no size to
 *  show (not known yet, or past downloading and installing). */
export function downloadFraction(status: UpdateStatus): number | null {
  if (status.phase === "ready") return 1;
  if (status.phase !== "downloading" || !status.total) return null;
  return Math.min(1, Math.max(0, status.downloaded / status.total));
}

interface Props {
  status: UpdateStatus;
  /** Ring diameter in px, from the backend's metrics. */
  size: number;
  active: boolean;
  onHover: () => void;
  onActivate: () => void;
}

export function UpdateRing({ status, size, active, onHover, onActivate }: Props) {
  const { t } = useI18n();

  // The same proportions as a provider's ring, so it belongs on the strip.
  const stroke = Math.max(3.5, size * 0.125);
  const radius = (size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  const iconSize = Math.round(size * 0.36);

  const phase = status.phase;
  const fraction = downloadFraction(status);
  const failed = phase === "failed" || (phase === "ready" && status.error !== null);
  // Size not known yet, or installing: a short arc riding the ring instead.
  const spinning = (phase === "downloading" && fraction === null) || phase === "installing";

  const Mark =
    failed ? TriangleAlert : phase === "ready" ? RefreshCw : phase === "installing" ? Package : Download;

  // The number: the download's progress while it comes in, otherwise which
  // version this is.
  const big =
    phase === "downloading" && fraction !== null
      ? formatPct(fraction * 100)
      : phase === "installing"
        ? "…"
        : (status.version ?? "");
  const small =
    failed
      ? t("update.ring.retry")
      : phase === "ready"
        ? t("update.ring.restart")
        : phase === "installing"
          ? t("update.ring.installing")
          : phase === "available"
            ? t("update.ring.download")
            : `v${status.version}`;

  return (
    <button
      type="button"
      className="ring-slot"
      onMouseEnter={onHover}
      onFocus={onHover}
      onClick={onActivate}
      aria-label={t("update.ring.aria", { version: status.version ?? "" })}
      data-active={active || undefined}
    >
      <span
        className="ring-disc"
        style={{ width: size, height: size }}
        // Ready: a halo in the accent colour, a few times, to say a click is
        // waiting. Not while the card is open: it's already been seen.
        data-attention={!active && phase === "ready" && !failed ? "update" : undefined}
      >
        <svg
          viewBox={`0 0 ${size} ${size}`}
          width={size}
          height={size}
          className="ring-arc"
          aria-hidden
        >
          <g transform={`rotate(-90 ${size / 2} ${size / 2})`}>
            <circle
              cx={size / 2}
              cy={size / 2}
              r={radius}
              fill="none"
              stroke="var(--ring-track)"
              strokeWidth={stroke}
            />
            {fraction !== null && !failed && (
              <circle
                cx={size / 2}
                cy={size / 2}
                r={radius}
                fill="none"
                stroke="var(--accent)"
                strokeWidth={stroke}
                strokeLinecap="round"
                strokeDasharray={`${circumference * fraction} ${circumference}`}
                style={{ transition: "stroke-dasharray 300ms ease-out" }}
              />
            )}
            {spinning && (
              <circle
                cx={size / 2}
                cy={size / 2}
                r={radius}
                fill="none"
                stroke="var(--accent)"
                strokeWidth={stroke * 0.8}
                strokeLinecap="round"
                strokeDasharray={`${circumference * 0.22} ${circumference}`}
                className="activity-spin"
              />
            )}
          </g>
        </svg>

        <Mark
          className="ring-mark update-mark"
          data-failed={failed || undefined}
          style={{ width: iconSize, height: iconSize }}
          strokeWidth={2.2}
          aria-hidden
        />
      </span>

      <span
        className="ring-pct tnum"
        // A version is five characters where a percentage is three, so it
        // takes the smaller size a countdown does.
        style={{ fontSize: Math.round(size * (big.length > 4 ? 0.27 : 0.355)) }}
      >
        {big}
      </span>

      <span className="ring-update-label" style={{ fontSize: Math.round(size * 0.17) }}>
        {small}
      </span>
    </button>
  );
}
