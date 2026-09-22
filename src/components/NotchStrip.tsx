/**
 * The notch itself: a black strip hugging one screen edge with a ring per
 * provider.
 *
 * The strip is flush against the edge and tapers away at both ends along the
 * curve in NotchShape — the join that makes it read as carved out of the
 * display rather than a panel floating on top of it.
 */

import { useLayoutEffect, useRef, useState } from "react";
import { Settings2 } from "lucide-react";

import { useI18n } from "../lib/i18n";
import type { Edge, HudMetrics, ProviderSnapshot } from "../types";
import { NotchShape } from "./NotchShape";
import { ProviderRing } from "./ProviderRing";

interface Props {
  providers: ProviderSnapshot[];
  metrics: HudMetrics;
  edge: Edge;
  activeId: string | null;
  showWeekly: boolean;
  weeklyOnRing: boolean;
  ringColors: Record<string, string>;
  /** Providers whose agent just answered: their ring pulses for a moment. */
  answered: string[];
  /** Pulse while an agent is waiting on the user, too. */
  pulseWaiting: boolean;
  onHover: (provider: ProviderSnapshot | null) => void;
  onActivate: (provider: ProviderSnapshot) => void;
  onOpenSettings: () => void;
  /** Close whatever card is open without letting the notch collapse. */
  onDismissCard: () => void;
  /** Alt-drag along the edge, reported one step at a time in pixels. */
  onDragAlong?: (delta: number) => void;
  settingsOpen: boolean;
  /** Something the notch stays behind has the foreground. */
  tucked?: boolean;
  innerRef?: React.Ref<HTMLDivElement>;
}

export function NotchStrip({
  providers,
  metrics,
  edge,
  activeId,
  showWeekly,
  weeklyOnRing,
  ringColors,
  answered,
  pulseWaiting,
  onHover,
  onActivate,
  onDragAlong,
  onOpenSettings,
  onDismissCard,
  settingsOpen,
  tucked,
  innerRef,
}: Props) {
  const { t } = useI18n();
  const vertical = edge === "left" || edge === "right";
  const shellRef = useRef<HTMLDivElement | null>(null);
  const [shell, setShell] = useState({ width: 0, height: 0 });

  // The silhouette is drawn at exact pixel size, so it has to follow the strip
  // as rings come and go.
  useLayoutEffect(() => {
    const node = shellRef.current;
    if (!node) return;
    const measure = () => {
      const box = node.getBoundingClientRect();
      setShell({ width: Math.ceil(box.width), height: Math.ceil(box.height) });
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    return () => observer.disconnect();
  }, [vertical, metrics, providers.length]);

  // Alt and drag slides the notch along its edge. Alt because a plain drag
  // across the strip is how you get from one ring to the next.
  const drag = useRef<{ id: number; from: number } | null>(null);
  const moved = useRef(false);

  return (
    <div
      ref={(node) => {
        shellRef.current = node;
        if (typeof innerRef === "function") innerRef(node);
        else if (innerRef) {
          (innerRef as React.RefObject<HTMLDivElement | null>).current = node;
        }
      }}
      className="notch-strip"
      data-edge={edge}
      data-tucked={tucked || undefined}
      onPointerDown={(e) => {
        if (!e.altKey || !onDragAlong) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { id: e.pointerId, from: vertical ? e.clientY : e.clientX };
        moved.current = false;
      }}
      onPointerMove={(e) => {
        if (!drag.current || e.pointerId !== drag.current.id) return;
        const at = vertical ? e.clientY : e.clientX;
        // Reported as a step rather than a total, so clamping at either end
        // doesn't leave the notch owing the pointer a debt of pixels.
        if (at !== drag.current.from) {
          moved.current = true;
          onDragAlong?.(at - drag.current.from);
          drag.current.from = at;
        }
      }}
      onPointerUp={(e) => {
        if (drag.current?.id === e.pointerId) drag.current = null;
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
      onClickCapture={(e) => {
        // A drag that ends over a ring shouldn't also open that ring's app.
        if (moved.current) {
          e.stopPropagation();
          e.preventDefault();
          moved.current = false;
        }
      }}
      style={{
        [vertical ? "width" : "height"]: metrics.stripThickness,
        padding: vertical
          ? `${metrics.stripPadding}px 0`
          : `0 ${metrics.stripPadding}px`,
      }}
    >
      <NotchShape
        className="notch-silhouette"
        width={shell.width}
        height={shell.height}
        edge={edge}
      />

      <div
        className="notch-rings"
        style={{ flexDirection: vertical ? "column" : "row" }}
      >
        {providers.map((provider, index) => (
          <span
            key={provider.key}
            className="ring-cell"
            style={{ [vertical ? "height" : "width"]: metrics.slot }}
          >
            <ProviderRing
              provider={provider}
              index={index}
              size={metrics.ring}
              active={activeId === provider.key}
              showWeekly={showWeekly}
              weeklyOnRing={weeklyOnRing}
              customColour={ringColors[provider.key]}
              answered={answered.includes(provider.key)}
              pulseWaiting={pulseWaiting}
              onHover={onHover}
              onActivate={onActivate}
            />
          </span>
        ))}

        {/* The strip is the whole UI, so settings live on it too. */}
        <button
          type="button"
          className="strip-settings"
          onClick={onOpenSettings}
          onMouseEnter={onDismissCard}
          aria-label={t("strip.settings")}
          data-active={settingsOpen || undefined}
        >
          <Settings2 aria-hidden />
        </button>
      </div>
    </div>
  );
}
