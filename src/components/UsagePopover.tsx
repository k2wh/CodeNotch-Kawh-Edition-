/**
 * The detail card that opens beside a hovered ring.
 *
 * One block per usage window: what it counts and when it resets on the top
 * line, a bar, then the percentage. A tail on the edge nearest the strip points
 * back at the ring it belongs to, so with several rings stacked up it is always
 * obvious which one you are reading.
 */

import type { Config, Edge, ProviderSnapshot, Session } from "../types";
import {
  activityLabel,
  formatAgo,
  formatCount,
  formatDays,
  formatMultiple,
  formatReset,
  formatUsd,
  healthLabel,
  windowValue,
} from "../lib/format";
import { useI18n } from "../lib/i18n";
import { BrandIcon } from "./BrandIcon";

interface Props {
  provider: ProviderSnapshot;
  config: Config;
  edge: Edge;
  /** Centre of the ring this belongs to, in window coordinates. */
  anchor: number;
  /** Strip thickness: the tail is drawn in proportion to it. */
  thickness: number;
  onFocusProvider: (provider: ProviderSnapshot, session?: Session | null) => void;
}

/**
 * The tail, as multiples of the strip's thickness.
 *
 * Measured off the reference along with the silhouette: a long spike with
 * concave flanks, not a triangle. Each flank is one quadratic whose control
 * point sits on the card's own edge, 46% of the way from the tip back to the
 * base corner — which is what gives it the drawn-out, liquid look.
 */
const TAIL_LENGTH = 0.4;
const TAIL_HALF = 0.33;
const TAIL_CONTROL = 0.46;

/** The tail as an SVG, pointing away from the card towards the strip. */
function Tail({ edge, thickness }: { edge: Edge; thickness: number }) {
  const length = Math.round(thickness * TAIL_LENGTH);
  const half = Math.round(thickness * TAIL_HALF);
  const c = half * (1 - TAIL_CONTROL);

  // Drawn pointing right, then rotated onto the edge that needs it.
  const vertical = edge === "left" || edge === "right";
  const w = vertical ? length : half * 2;
  const h = vertical ? half * 2 : length;
  const flip = edge === "left" || edge === "top";
  const at = (along: number, across: number) => {
    const a = flip ? length - along : along;
    return vertical ? `${a} ${across}` : `${across} ${a}`;
  };

  return (
    <svg
      className="popover-tail"
      width={w}
      height={h}
      viewBox={`0 0 ${w} ${h}`}
      style={{ "--tail-half": `${half}px` } as React.CSSProperties}
      aria-hidden
    >
      <path
        d={[
          `M ${at(0, 0)}`,
          `Q ${at(0, c)} ${at(length, half)}`,
          `Q ${at(0, half * 2 - c)} ${at(0, half * 2)}`,
          "Z",
        ].join(" ")}
        fill="var(--popover-bg)"
      />
    </svg>
  );
}

export function UsagePopover({
  provider,
  config,
  edge,
  anchor,
  thickness,
  onFocusProvider,
}: Props) {
  const { t, locale, server } = useI18n();
  const health = healthLabel(provider.health, t);
  const activity = activityLabel(provider.activity, t);
  const detail = provider.detail ? server(provider.detail) : null;

  return (
    <div
      className="popover"
      data-edge={edge}
      // The tail tracks the ring; everything else stays put.
      style={{ "--tail-offset": `${anchor}px` } as React.CSSProperties}
      role="dialog"
      aria-label={t("popover.aria", { name: provider.name })}
    >
      <Tail edge={edge} thickness={thickness} />

      <header className="popover-head">
        <BrandIcon provider={provider.id} className="popover-mark" />
        <span className="popover-title">{t("popover.title", { name: provider.name })}</span>
        {activity && (
          <span
            className="popover-activity"
            data-state={provider.activity}
          >
            {activity}
          </span>
        )}
      </header>

      {provider.account && <p className="popover-account">{server(provider.account)}</p>}

      {provider.windows.length > 0 ? (
        <div className="popover-windows">
          {provider.windows.map((window) => {
            const reset = formatReset(window.resetsAt, config.resetAsCountdown, t, locale);
            const pct = window.usedPct;
            const tone =
              window.informational || pct === null
                ? "info"
                : pct >= 70
                  ? "high"
                  : pct >= 40
                    ? "mid"
                    : "low";
            return (
              <div key={window.key} className="usage-block">
                <div className="usage-line">
                  <span className="usage-label">{server(window.label)}</span>
                  <span className="usage-reset tnum">
                    {/* "Resetting" is a state, not a time: wrapping it in
                        "Resets in ..." read as "Resets in resetting". */}
                    {reset === t("time.resetting")
                      ? reset
                      : reset
                        ? t(config.resetAsCountdown ? "popover.resetsIn" : "popover.resetsAt", {
                            time: reset,
                          })
                        : ""}
                  </span>
                </div>
                <div className="usage-track">
                  <div
                    className="usage-fill"
                    data-tone={tone}
                    style={{ width: `${pct === null ? 0 : Math.min(pct, 100)}%` }}
                  />
                </div>
                <div className="usage-value tnum">
                  {pct !== null
                    ? t("popover.used", { value: windowValue(window) })
                    : windowValue(window)}
                  {/* What the percentage is worth in tokens, once enough of
                      the window has gone to say. Marked "~": it is worked
                      out from what this machine spent. */}
                  {window.usedTokens != null && window.capacityTokens != null && (
                    <span className="usage-tokens">
                      ~{formatCount(window.usedTokens)} / {formatCount(window.capacityTokens)}
                    </span>
                  )}
                </div>
                {/* Tokens sent against tokens generated, and what buying them
                    would have cost. Counted from the transcripts rather than
                    fitted against a percentage, so this stands on its own. */}
                {window.inputTokens != null && window.outputTokens != null && (
                  <div className="usage-split tnum">
                    <span title={t("popover.inputTitle")}>
                      ↓ {formatCount(window.inputTokens)}
                    </span>
                    <span title={t("popover.outputTitle")}>
                      ↑ {formatCount(window.outputTokens)}
                    </span>
                    {window.cacheTokens != null && window.cacheTokens > 0 && (
                      <span title={t("popover.cacheTitle")}>
                        ♻ {formatCount(window.cacheTokens)}
                      </span>
                    )}
                    {window.usd != null && <span>{formatUsd(window.usd)}</span>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <p className="popover-empty">{detail ?? t("popover.empty")}</p>
      )}

      {/* An explanation only when the numbers can't be trusted. */}
      {health && provider.windows.length > 0 && detail && (
        <p className="popover-note">{detail}</p>
      )}

      {/* What the plan has returned: the same tokens, priced as if they had
          been bought from the API, against what the plan costs over the
          stretch actually counted. */}
      {provider.value && provider.value.planUsd > 0 && (
        <div className="popover-value">
          <div className="value-line">
            <span className="value-amount tnum">{formatUsd(provider.value.usd)}</span>
            <span
              className="value-multiple tnum"
              data-good={provider.value.usd >= provider.value.planUsd}
            >
              {formatMultiple(provider.value.usd / provider.value.planUsd)}
            </span>
          </div>
          <div className="value-note">
            {t("popover.valueNote", {
              plan: formatUsd(provider.value.planUsd),
              days: formatDays(provider.value.coveredDays, t),
            })}
          </div>
        </div>
      )}

      {provider.sessions.length > 0 && (
        <div className="popover-sessions">
          {provider.sessions.slice(0, 3).map((session) => (
            <button
              key={session.id}
              type="button"
              className="session-row"
              onClick={() => onFocusProvider(provider, session)}
            >
              <span className="session-dot" data-state={session.activity} />
              <span className="session-title">{session.title}</span>
              <span className="session-meta tnum">
                {(session.detail && server(session.detail)) ??
                  formatAgo(session.lastActivity, t) ??
                  ""}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
