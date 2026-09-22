/**
 * The update ring's card: which version, what it brings, how far the download
 * has got, and the one button that matters at each step.
 */

import { Sparkles } from "lucide-react";

import type { Edge, UpdateStatus } from "../types";
import { formatBytes, formatPct, summariseNotes } from "../lib/format";
import { useI18n } from "../lib/i18n";
import { downloadFraction } from "./UpdateRing";
import { Tail } from "./UsagePopover";

/** Installing a .deb asks for the administrator's password; say so first.
 *  (Android calls itself Linux too, and never runs this app.) */
const ON_LINUX =
  typeof navigator !== "undefined" &&
  /Linux/.test(navigator.userAgent) &&
  !/Android/.test(navigator.userAgent);

interface Props {
  status: UpdateStatus;
  edge: Edge;
  /** Centre of the ring this belongs to, in window coordinates. */
  anchor: number;
  /** Strip thickness: the tail is drawn in proportion to it. */
  thickness: number;
  onAct: () => void;
  onLater: () => void;
}

export function UpdatePopover({ status, edge, anchor, thickness, onAct, onLater }: Props) {
  const { t, server } = useI18n();
  const phase = status.phase;
  const failed = phase === "failed" || (phase === "ready" && status.error !== null);
  const fraction = downloadFraction(status);
  const notes = summariseNotes(status.notes);

  const title = failed
    ? t("update.card.failed")
    : phase === "ready"
      ? t("update.card.ready")
      : phase === "downloading"
        ? t("update.card.downloading")
        : phase === "installing"
          ? t("update.card.installing")
          : t("update.card.available");

  const size = status.total ? ` · ${formatBytes(status.total)}` : "";
  const action = failed
    ? t("update.card.retry")
    : phase === "ready"
      ? t("update.card.restart")
      : phase === "available"
        ? t("update.card.download")
        : null;

  return (
    <div
      className="popover"
      data-edge={edge}
      style={{ "--tail-offset": `${anchor}px` } as React.CSSProperties}
      role="dialog"
      aria-label={title}
    >
      <Tail edge={edge} thickness={thickness} />

      <header className="popover-head">
        <Sparkles className="popover-mark update-card-mark" aria-hidden />
        <span className="popover-title">{title}</span>
      </header>

      <p className="popover-account tnum">
        CodeNotch {status.current} → {status.version}
        {size}
      </p>

      {phase === "downloading" && (
        <div className="usage-block update-progress">
          <div className="usage-track">
            <div
              className="usage-fill update-fill"
              style={{ width: `${Math.round((fraction ?? 0) * 100)}%` }}
            />
          </div>
          <div className="usage-value tnum">
            {fraction === null
              ? formatBytes(status.downloaded)
              : `${formatPct(fraction * 100)} · ${formatBytes(status.downloaded)}`}
          </div>
        </div>
      )}

      {notes && <p className="update-notes">{notes}</p>}

      {failed && status.error && (
        <p className="popover-note update-error">{server(status.error)}</p>
      )}

      {action && (
        <div className="update-actions">
          <button type="button" className="update-primary" onClick={onAct}>
            {action}
          </button>
          <button type="button" className="update-later" onClick={onLater}>
            {t("update.card.later")}
          </button>
        </div>
      )}

      {phase === "ready" && !failed && (
        <p className="popover-note">
          {ON_LINUX ? t("update.card.passwordHint") : t("update.card.restartHint")}
        </p>
      )}
    </div>
  );
}
