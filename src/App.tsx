/**
 * The HUD shell.
 *
 * The window is a fixed, transparent column along the screen edge; the strip
 * and the detail popover are drawn inside it. Four responsibilities beyond
 * rendering:
 *
 * 1. **Anchoring.** The strip hugs the screen edge at the configured offset
 *    along it, and the popover sits on the inward side next to its ring.
 * 2. **Hover intent.** Moving between rings switches the popover instantly;
 *    leaving closes it after a short grace period so crossing the notch on the
 *    way somewhere else doesn't make it flap.
 * 3. **Regions.** Where the strip and popover are drawn goes back to Rust,
 *    which uses it for hover and to clip the window so its empty part never
 *    swallows clicks. The window itself never resizes on hover: resizing a
 *    WebView2 window stretches the old frame for a moment, which looked like
 *    the notch smearing and snapping back.
 * 4. **Motion.** Cards fade in, glide between rings, and fade out before the
 *    region shrinks around them.
 */

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { NotchStrip } from "./components/NotchStrip";
import { PROVIDER_ROWS, SettingsPanel, type ProviderRow } from "./components/SettingsPanel";
import { UpdatePopover } from "./components/UpdatePopover";
import { UPDATE_KEY, updateRingVisible } from "./components/UpdateRing";
import { UsagePopover } from "./components/UsagePopover";
import { WhatsNewPopover } from "./components/WhatsNewPopover";
import { changesFor, type Release } from "./lib/changelog";
import { DEMO_BOOTSTRAP, useDemoUpdate } from "./lib/demo";
import { blockedUntil } from "./lib/format";
import { I18nProvider, resolveLang } from "./lib/i18n";
import { events, IN_TAURI, ipc } from "./lib/ipc";
import { playSound } from "./lib/sounds";
import type {
  Config,
  Edge,
  HudMetrics,
  HudState,
  MonitorInfo,
  ProviderSnapshot,
  Rect,
  Session,
  Telemetry,
  UpdateStatus,
} from "./types";

/** Grace period before closing, so a passing pointer doesn't toggle it. */
const CLOSE_DELAY_MS = 200;
/**
 * Settings are saved this long after the last change, so dragging a slider or
 * a colour picker writes once rather than once per pixel.
 */
const SAVE_DEBOUNCE_MS = 150;
/** Length of the card's exit animation; keep in step with `.leaving` in CSS. */
const LEAVE_MS = 140;
/** How long a ring keeps pulsing after its agent answers. */
const ANSWERED_PULSE_MS = 9000;

/** What the popover slot is showing. */
type Card =
  | { kind: "provider"; provider: ProviderSnapshot }
  | { kind: "update"; status: UpdateStatus }
  | { kind: "whatsNew"; release: Release }
  | { kind: "settings" };

/**
 * Keep the last card on screen for its exit animation after it is dismissed,
 * instead of unmounting it mid-frame.
 */
function usePresence(card: Card | null): { shown: Card | null; leaving: boolean } {
  const [lingering, setLingering] = useState<Card | null>(null);
  const last = useRef<Card | null>(null);

  useEffect(() => {
    if (card) {
      last.current = card;
      setLingering(null);
      return;
    }
    if (!last.current) return;
    setLingering(last.current);
    last.current = null;
    const timer = window.setTimeout(() => setLingering(null), LEAVE_MS);
    return () => window.clearTimeout(timer);
  }, [card]);

  return card ? { shown: card, leaving: false } : { shown: lingering, leaving: lingering !== null };
}

export default function App() {
  const [config, setConfig] = useState<Config>(DEMO_BOOTSTRAP.config);
  const [telemetry, setTelemetry] = useState<Telemetry>(
    IN_TAURI
      ? { ...DEMO_BOOTSTRAP.telemetry, providers: [] }
      : DEMO_BOOTSTRAP.telemetry,
  );
  const [hud, setHud] = useState<HudState>(DEMO_BOOTSTRAP.hud);
  const [metrics, setMetrics] = useState<HudMetrics>(DEMO_BOOTSTRAP.metrics);
  const [edge, setEdge] = useState<Edge>(DEMO_BOOTSTRAP.edge);
  const [version, setVersion] = useState(DEMO_BOOTSTRAP.version);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [showSettings, setShowSettings] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [liveUpdate, setUpdate] = useState<UpdateStatus | null>(null);
  // In a bare browser there is no backend: a pretend release plays through
  // every step, so the ring can be worked on without publishing one.
  const demoUpdate = useDemoUpdate(!IN_TAURI);
  const update = IN_TAURI ? liveUpdate : demoUpdate.status;
  const showUpdate = updateRingVisible(update);
  // What changed, shown once on the first launch of a new version and from
  // the version in settings afterwards.
  const [whatsNew, setWhatsNew] = useState<string | null>(
    IN_TAURI ? null : (DEMO_BOOTSTRAP.whatsNew ?? null),
  );
  const news = useMemo(() => changesFor(whatsNew), [whatsNew]);

  const closeTimer = useRef<number | null>(null);
  /** Whether the notch was open on the last render; see the effect below. */
  const wasOpen = useRef(false);
  const saveTimer = useRef<number | null>(null);
  const stripRef = useRef<HTMLDivElement | null>(null);
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const [stripLength, setStripLength] = useState(0);
  const [tailAnchor, setTailAnchor] = useState<number | null>(null);
  const [popoverLength, setPopoverLength] = useState(0);

  // Providers that have nothing to say don't earn a ring.
  const rings = useMemo(
    () => telemetry.providers.filter((p) => p.health !== "unavailable"),
    [telemetry],
  );

  // Settings rows: one per tool, plus one for any extra account the backend
  // found. With a single account per tool this is exactly the plain list.
  // Accounts come from their own list rather than from the rings: switching
  // one off takes its ring away, and its row has to stay to switch it back.
  const settingsRings = useMemo<ProviderRow[]>(() => {
    const rows = [...PROVIDER_ROWS];
    for (const account of telemetry.accounts ?? []) {
      if (!rows.some((row) => row.key === account.key)) {
        rows.push({ key: account.key, id: account.id, label: account.name });
      }
    }
    return rows;
  }, [telemetry]);

  const vertical = edge === "left" || edge === "right";
  const active = rings.find((p) => p.key === activeId) ?? null;

  const card: Card | null = useMemo(() => {
    if (showSettings) return { kind: "settings" };
    if (activeId === UPDATE_KEY && update && showUpdate) return { kind: "update", status: update };
    if (active) return { kind: "provider", provider: active };
    if (news) return { kind: "whatsNew", release: news };
    return null;
  }, [showSettings, activeId, update, showUpdate, active, news]);
  const { shown, leaving } = usePresence(card);

  // --- Bootstrap and subscriptions ---------------------------------------

  useEffect(() => {
    let cancelled = false;

    ipc.ready().then((bootstrap) => {
      if (cancelled || !bootstrap) return;
      setConfig(bootstrap.config);
      setTelemetry(bootstrap.telemetry);
      setHud(bootstrap.hud);
      setMetrics(bootstrap.metrics);
      setEdge(bootstrap.edge);
      setVersion(bootstrap.version);
      // First launch of a new version: say what changed, and open the notch
      // for long enough to read it.
      if (bootstrap.whatsNew && changesFor(bootstrap.whatsNew)) {
        setWhatsNew(bootstrap.whatsNew);
        void ipc.peek(20);
      }
    });
    // A reloaded webview picks up an update already under way.
    ipc.updateStatus().then((status) => !cancelled && status && setUpdate(status));

    const unsubscribers = [
      events.telemetry(setTelemetry),
      events.config((next) => {
        // A save of ours is still queued: the local copy is newer than this
        // echo of an earlier one, and taking it would jump the slider back.
        if (saveTimer.current === null) setConfig(next);
        setEdge(next.edge);
        // The backend has applied the change by the time it announces it, so
        // this is the moment its strip dimensions match the window again
        // (S/M/L). The bootstrap copy only describes the starting size.
        ipc.getMetrics().then((m) => m && setMetrics(m));
      }),
      events.hudState(setHud),
      events.update(setUpdate),
    ];

    return () => {
      cancelled = true;
      unsubscribers.forEach((p) => p.then((off) => off()));
    };
  }, []);

  useEffect(() => {
    document.documentElement.style.setProperty("--accent", config.accent);
  }, [config.accent]);

  // The backend closed the notch (peek expired, cursor left): drop the popover
  // so the two sides don't disagree about what is on screen.
  useEffect(() => {
    // On the way closed, not on the way in: a card the notch opens itself for
    // (what's new) is set before the backend reports the notch open, and
    // clearing on mount would take it away in the same breath.
    if (wasOpen.current && !hud.open) {
      setActiveId(null);
      setShowSettings(false);
      setWhatsNew(null);
    }
    wasOpen.current = hud.open;
  }, [hud.open]);

  // --- Attention ----------------------------------------------------------

  // A ring pulses while its agent waits on you, and for a few seconds after
  // one answers — the visual half of the chime the backend plays. "Finished"
  // sticks around for minutes, so it is caught on the transition and timed
  // out here rather than pulsing for as long as the state lasts.
  const wasDoing = useRef<Record<string, string>>({});
  const [answered, setAnswered] = useState<string[]>([]);
  useEffect(() => {
    // A muted provider still draws and still pulses; it just doesn't speak.
    const muted = new Set(config.mutedProviders ?? []);
    // News from a ring whose window you're already in isn't news: no chime,
    // no pulse, and the backend holds its peek for the same reason.
    const inFront = new Set(telemetry.inFront ?? []);
    const changed = telemetry.providers.filter((p) => {
      const before = wasDoing.current[p.key];
      wasDoing.current[p.key] = p.activity;
      return (
        before !== undefined &&
        before !== p.activity &&
        !muted.has(p.key) &&
        !inFront.has(p.key)
      );
    });
    // An agent answering and one stopping to ask are different news: the
    // first is over, the second is blocked on you. They get their own
    // sounds, and either can be silenced without silencing the other.
    const wants = changed.filter((p) => p.activity === "done" || p.activity === "awaitingInput");
    if (wants.length === 0) return;

    if (config.notifySound ?? true) {
      const waiting = wants.some((p) => p.activity === "awaitingInput");
      const sound = waiting
        ? (config.notifyWaitingSoundId ?? "ping")
        : (config.notifySoundId ?? "chime");
      // "none" is how a picker says this half should stay quiet.
      if (sound !== "none") playSound(sound, (config.notifyVolume ?? 70) / 100);
    }

    const fresh = wants.filter((p) => p.activity === "done").map((p) => p.key);
    if (fresh.length === 0 || !(config.notifyPulse ?? true)) return;
    setAnswered((current) => [...new Set([...current, ...fresh])]);
    const timer = window.setTimeout(
      () => setAnswered((current) => current.filter((id) => !fresh.includes(id))),
      ANSWERED_PULSE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [telemetry]);

  // Running out is its own kind of news: not "look at this" but "nothing you
  // do helps until the clock runs down", so it gets its own chime. Only on
  // the crossing — a provider that was already spent when the notch started
  // announces nothing.
  // Coming back is worth its own chime too: the window reset while you were
  // doing something else, and knowing without looking is the point.
  const wasBlocked = useRef<Record<string, boolean>>({});
  useEffect(() => {
    const muted = new Set(config.mutedProviders ?? []);
    let hit = false;
    let freed = false;
    for (const provider of telemetry.providers) {
      const spent = blockedUntil(provider) !== null;
      const before = wasBlocked.current[provider.key];
      wasBlocked.current[provider.key] = spent;
      // Still tracked while muted, so unmuting doesn't fire a stale crossing.
      if (muted.has(provider.key)) continue;
      if (before === false && spent) hit = true;
      if (before === true && !spent) freed = true;
    }
    if (!(config.notifySound ?? true)) return;

    // Running out wins if both happened in the same poll: it is the one that
    // changes what you can do next.
    const sound = hit
      ? (config.notifyLimitSoundId ?? "drop")
      : freed
        ? (config.notifyRecoverSoundId ?? "arp")
        : null;
    if (sound && sound !== "none") playSound(sound, (config.notifyVolume ?? 70) / 100);
  }, [telemetry]);

  // Going to the notch yourself is seeing it, and the pulse has done its job.
  // A peek doesn't count: the notch opens itself the moment an agent answers,
  // which would cancel the pulse in the same breath as starting it.
  useEffect(() => {
    if (hud.open && !hud.peeking) setAnswered([]);
  }, [hud.open, hud.peeking]);

  // --- Hover intent -------------------------------------------------------

  const clearCloseTimer = () => {
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  };

  const handleHover = useCallback((provider: ProviderSnapshot | null) => {
    clearCloseTimer();
    if (provider) {
      setActiveId(provider.key);
      setShowSettings(false);
      setWhatsNew(null);
      // Reading a ring's card is seeing it: its pulse can stop.
      setAnswered((current) => current.filter((key) => key !== provider.key));
      void ipc.hover(true);
      return;
    }
    closeTimer.current = window.setTimeout(() => {
      setActiveId(null);
      void ipc.hover(false);
    }, CLOSE_DELAY_MS);
  }, []);

  const handleHoverUpdate = useCallback(() => {
    clearCloseTimer();
    setActiveId(UPDATE_KEY);
    setShowSettings(false);
    setWhatsNew(null);
    void ipc.hover(true);
  }, []);

  // A click on the update ring, or its card's button: download, try again, or
  // install and restart, whichever the step calls for.
  const { act: demoAct, later: demoLater } = demoUpdate;
  const handleUpdateAct = useCallback(() => {
    if (IN_TAURI) void ipc.updateAct();
    else demoAct();
  }, [demoAct]);

  const handleUpdateLater = useCallback(() => {
    setActiveId(null);
    if (IN_TAURI) void ipc.updateDismiss();
    else demoLater();
  }, [demoLater]);

  useEffect(() => clearCloseTimer, []);

  // --- Placement ----------------------------------------------------------

  const windowLength = vertical
    ? hud.height || window.innerHeight
    : hud.width || window.innerWidth;

  // The window spans the whole edge; the strip sits at the configured offset
  // along it, exactly where a window of its own size would have been docked.
  const stripStart =
    stripLength > 0
      ? Math.round(Math.max(0, windowLength - stripLength) * config.edgeOffset)
      : 0;

  /**
   * Centre the popover on the ring it points at, then keep it on screen.
   *
   * Clamping is what makes the tail worth having: for a ring near either end of
   * the strip the card has to slide back inside the window, so its centre no
   * longer lines up with the ring and only the tail says which one you are
   * reading.
   */
  const { popoverStart, tailOffset } = useMemo(() => {
    // The settings panel has no ring of its own: line it up with the strip.
    const anchor = tailAnchor ?? stripStart + stripLength / 2;

    if (popoverLength <= 0) {
      return { popoverStart: Math.max(0, anchor - 150), tailOffset: anchor };
    }

    const limit = Math.max(0, windowLength - popoverLength);
    const start = Math.min(Math.max(anchor - popoverLength / 2, 0), limit);

    // Keep the whole tail off the card's rounded corners: its base is as tall
    // as TAIL_HALF either side of the anchor.
    const inset = metrics.stripThickness * 0.4;
    const offset = Math.min(
      Math.max(anchor - start, inset),
      Math.max(popoverLength - inset, inset),
    );
    return { popoverStart: start, tailOffset: offset };
  }, [tailAnchor, stripStart, stripLength, popoverLength, windowLength, metrics]);

  // --- Measurement --------------------------------------------------------

  // Tell Rust where things are drawn, whenever any of it moves or resizes.
  useLayoutEffect(() => {
    const strip = stripRef.current;
    if (!strip) return;

    const report = () => {
      // The strip's resting box, from layout: on launch it slides in from the
      // edge, and a box measured mid-slide would put the hover area off to
      // the side of where the strip ends up.
      const stripWidth = strip.offsetWidth;
      const stripHeight = strip.offsetHeight;
      const stripBox: Rect = vertical
        ? {
            x: edge === "right" ? window.innerWidth - stripWidth : 0,
            y: stripStart,
            width: stripWidth,
            height: stripHeight,
          }
        : {
            x: stripStart,
            y: edge === "bottom" ? window.innerHeight - stripHeight : 0,
            width: stripWidth,
            height: stripHeight,
          };
      const pop = popoverRef.current;

      // The card's *resting* box, from layout rather than from the screen: it
      // spends its first 220 ms scaled and shifted by the entrance animation,
      // and gliding between rings. Measuring it mid-flight handed Rust a box
      // the card wasn't in yet, so the pointer "left" it and the notch shut.
      let popover: Rect | null = null;
      if (pop) {
        const width = pop.offsetWidth;
        const height = pop.offsetHeight;
        const inset = metrics.stripThickness + metrics.popoverGap;
        popover = vertical
          ? {
              x: edge === "right" ? window.innerWidth - inset - width : inset,
              y: popoverStart,
              width,
              height,
            }
          : {
              x: popoverStart,
              y: edge === "bottom" ? window.innerHeight - inset - height : inset,
              width,
              height,
            };
      }

      setStripLength(vertical ? stripBox.height : stripBox.width);
      setPopoverLength(pop ? (vertical ? pop.offsetHeight : pop.offsetWidth) : 0);

      if (stripBox.width > 0 && stripBox.height > 0) {
        void ipc.setRegions({ strip: stripBox, popover });
      }
    };

    report();
    const observer = new ResizeObserver(report);
    observer.observe(strip);
    if (popoverRef.current) observer.observe(popoverRef.current);
    return () => observer.disconnect();
  }, [vertical, edge, metrics, rings.length, showUpdate, shown, telemetry, stripStart, popoverStart]);

  useEffect(() => {
    if (!showSettings) return;
    ipc.listMonitors().then((list) => list && setMonitors(list));
  }, [showSettings]);

  // Point the tail at the hovered ring's disc. Measuring beats arithmetic here:
  // a slot also holds the percentage label, so its centre sits below the disc's.
  useLayoutEffect(() => {
    const strip = stripRef.current;
    if (!strip || !activeId) {
      // Keep the last anchor while a card is leaving, so it fades out in place.
      if (!leaving) setTailAnchor(null);
      return;
    }
    const disc = strip.querySelector<HTMLElement>("[data-active] .ring-disc");
    if (!disc) return;

    const discBox = disc.getBoundingClientRect();
    setTailAnchor(
      vertical
        ? discBox.top + discBox.height / 2
        : discBox.left + discBox.width / 2,
    );
  }, [activeId, vertical, metrics, rings.length, showUpdate, telemetry, stripStart, leaving]);

  // --- Actions ------------------------------------------------------------

  const handleConfigChange = useCallback((next: Config) => {
    setConfig(next);
    if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = null;
      void ipc.setConfig(next);
    }, SAVE_DEBOUNCE_MS);
  }, []);

  /**
   * Slide the notch along its edge by one step of an Alt-drag.
   *
   * The offset is a fraction of the travel the strip has, not of the screen,
   * so dragging to the far end puts the strip's end at the screen's end
   * rather than its middle off the edge.
   */
  const nudgeAlong = useCallback(
    (delta: number) => {
      const travel = Math.max(1, windowLength - stripLength);
      const edgeOffset = Math.min(1, Math.max(0, config.edgeOffset + delta / travel));
      if (edgeOffset !== config.edgeOffset) {
        handleConfigChange({ ...config, edgeOffset });
      }
    },
    [config, windowLength, stripLength, handleConfigChange],
  );

  useEffect(
    () => () => {
      if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    },
    [],
  );

  /**
   * Raise the app a session lives in. The project folder is the title hint:
   * editors put it in the window title (`… - emitia - Visual Studio Code`),
   * so it picks the right window when several are open.
   */
  const handleFocusProvider = useCallback(
    (provider: ProviderSnapshot, session?: Session | null) => {
      const folder = session?.cwd?.split(/[\\/]/).filter(Boolean).pop() ?? null;
      void ipc.focusProvider(provider.id, folder ?? session?.title ?? null, session?.host ?? null);
    },
    [],
  );

  // Clicking a ring raises the app of its most relevant session. Sessions come
  // ranked (waiting on you, then working, then most recent), so that's the
  // first one.
  const handleActivate = useCallback(
    (provider: ProviderSnapshot) => handleFocusProvider(provider, provider.sessions[0] ?? null),
    [handleFocusProvider],
  );

  // The strip hugs the edge; the popover sits beside it on the inward side.
  const stripStyle: React.CSSProperties = vertical
    ? { [edge === "right" ? "right" : "left"]: 0, top: stripStart }
    : { [edge === "bottom" ? "bottom" : "top"]: 0, left: stripStart };

  const popoverStyle: React.CSSProperties = vertical
    ? {
        [edge === "right" ? "right" : "left"]:
          metrics.stripThickness + metrics.popoverGap,
        width: metrics.popoverSize,
        top: popoverStart,
      }
    : {
        [edge === "bottom" ? "bottom" : "top"]:
          metrics.stripThickness + metrics.popoverGap,
        width: metrics.popoverSize,
        left: popoverStart,
      };

  return (
    <I18nProvider lang={resolveLang(config.language)}>
    <div
      className="relative h-full w-full"
      data-edge={edge}
      onMouseLeave={() => handleHover(null)}
    >
      {shown && (
        <div
          ref={popoverRef}
          // Keyed by kind so switching between a provider card and settings
          // replays the entrance, while hopping between rings glides instead.
          key={shown.kind}
          className={`popover-slot absolute ${leaving ? "leaving" : "reveal"}`}
          style={popoverStyle}
        >
          {shown.kind === "provider" ? (
            <UsagePopover
              provider={shown.provider}
              config={config}
              edge={edge}
              anchor={tailOffset}
              thickness={metrics.stripThickness}
              onFocusProvider={handleFocusProvider}
            />
          ) : shown.kind === "whatsNew" ? (
            <WhatsNewPopover release={shown.release} onClose={() => setWhatsNew(null)} />
          ) : shown.kind === "update" ? (
            <UpdatePopover
              // The live status, so the progress moves while the card is open.
              status={update ?? shown.status}
              edge={edge}
              anchor={tailOffset}
              thickness={metrics.stripThickness}
              onAct={handleUpdateAct}
              onLater={handleUpdateLater}
            />
          ) : (
            <div className="popover popover-flush">
              <SettingsPanel
                config={config}
                rings={settingsRings}
                monitors={monitors}
                version={version}
                update={update}
                onCheckUpdates={() => void ipc.updateCheck()}
                onUpdateAct={handleUpdateAct}
                hasWhatsNew={changesFor(version) !== null}
                onWhatsNew={() => {
                  setShowSettings(false);
                  setWhatsNew(version);
                }}
                onChange={handleConfigChange}
                onClose={() => setShowSettings(false)}
                onOpenConfigDir={() => void ipc.openConfigDir()}
                onQuit={() => void ipc.quit()}
              />
            </div>
          )}
        </div>
      )}

      <div className="absolute" style={stripStyle}>
        <NotchStrip
          innerRef={stripRef}
          providers={rings}
          metrics={metrics}
          edge={edge}
          activeId={activeId}
          showWeekly={config.showWeekly ?? true}
          weeklyOnRing={config.weeklyOnRing ?? false}
          onDragAlong={nudgeAlong}
          tucked={hud.tucked ?? false}
          ringColors={config.ringColors ?? {}}
          answered={config.notifyPulse ?? true ? answered : []}
          pulseWaiting={config.notifyPulse ?? true}
          onHover={handleHover}
          onActivate={handleActivate}
          update={update}
          onHoverUpdate={handleHoverUpdate}
          onActivateUpdate={handleUpdateAct}
          onOpenSettings={() => {
            clearCloseTimer();
            setActiveId(null);
            setWhatsNew(null);
            setShowSettings((open) => !open);
            void ipc.hover(true);
          }}
          onDismissCard={() => {
            // Moving onto the gear closes the provider card, but the pointer is
            // still on the notch -- so don't tell the backend it left.
            clearCloseTimer();
            setActiveId(null);
          }}
          settingsOpen={showSettings}
        />
      </div>
    </div>
    </I18nProvider>
  );
}
