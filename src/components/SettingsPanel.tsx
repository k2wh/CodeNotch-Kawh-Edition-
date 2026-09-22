/** The settings view inside the expanded card. */

import { createContext, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  ArrowLeft,
  Bell,
  BellOff,
  Check,
  ChevronDown,
  ChevronRight,
  Crosshair,
  FolderOpen,
  GripVertical,
  Power,
  RefreshCw,
  Volume2,
} from "lucide-react";

import { formatAgo, formatPct } from "../lib/format";
import { LANGUAGES, useI18n, type LanguageSetting } from "../lib/i18n";
import { ipc } from "../lib/ipc";
import { playSound, SOUND_IDS } from "../lib/sounds";
import type {
  Config,
  Edge,
  HudSize,
  MonitorInfo,
  ProviderId,
  UpdatesMode,
  UpdateStatus,
} from "../types";
import { downloadFraction } from "./UpdateRing";
import { appLabel, WindowPicker } from "./WindowPicker";

/**
 * Focus handlers for anything that needs the keyboard or opens a native popup
 * (text fields, colour pickers, dropdowns). The notch normally refuses focus
 * and closes when the pointer leaves it; while one of these is in use it has
 * to accept keystrokes and stay open even with the pointer over the popup.
 */
const interactive = {
  onFocus: () => void ipc.setInteractive(true),
  onBlur: () => void ipc.setInteractive(false),
};

const EDGES: Edge[] = ["top", "bottom", "left", "right"];

const SIZES: { value: HudSize; label: string }[] = [
  { value: "small", label: "S" },
  { value: "medium", label: "M" },
  { value: "large", label: "L" },
];

const ACCENTS = ["#22d3ee", "#a78bfa", "#34d399", "#fbbf24", "#fb7185", "#60a5fa"];

/**
 * One row per ring, not per tool.
 *
 * A tool signed into two accounts draws two rings, and each gets its own
 * colour, switch, mute and place in the order — so settings are keyed by the
 * ring, which is what `key` is. For a tool with one account that key is just
 * the provider's own.
 */
export interface ProviderRow {
  key: string;
  id: ProviderId;
  label: string;
}

/** Product names: the same in every language. */
const PROVIDERS: { id: ProviderId; label: string }[] = [
  { id: "claudeCode", label: "Claude Code" },
  { id: "cursor", label: "Cursor" },
  { id: "copilot", label: "GitHub Copilot" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
  { id: "perplexity", label: "Perplexity" },
  { id: "grok", label: "Grok" },
  { id: "glm", label: "GLM (Z.ai)" },
  { id: "kimi", label: "Kimi" },
  { id: "opencode", label: "OpenCode" },
  { id: "commandCode", label: "Command Code" },
  { id: "miniMax", label: "MiniMax" },
  { id: "ollama", label: "Ollama" },
  { id: "lmStudio", label: "LM Studio" },
];

/**
 * The rows a machine with one account per tool shows — which is most of them.
 * Extra accounts are appended to this by the caller, from what the backend
 * actually found.
 */
export const PROVIDER_ROWS: ProviderRow[] = PROVIDERS.map((provider) => ({
  key: provider.id,
  id: provider.id,
  label: provider.label,
}));

/**
 * The providers a plan price can be set for: the ones whose transcripts say
 * what was spent, which is what makes a plan's return calculable.
 */
const PRICED_PROVIDERS: { id: ProviderId; label: string }[] = [
  { id: "claudeCode", label: "Claude Code" },
  { id: "codex", label: "Codex" },
];

interface Props {
  config: Config;
  /** Every ring on screen, so a second account gets its own row. */
  rings: ProviderRow[];
  monitors: MonitorInfo[];
  version: string;
  /** Where a new release stands, for the updates row. */
  update: UpdateStatus | null;
  onCheckUpdates: () => void;
  onUpdateAct: () => void;
  /** This build carries notes for the version it is: offer them beside it. */
  hasWhatsNew: boolean;
  onWhatsNew: () => void;
  onChange: (config: Config) => void;
  onClose: () => void;
  onOpenConfigDir: () => void;
  onQuit: () => void;
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-2 py-[5px]">
      {/* Longer translations wrap instead of pushing the control off the card. */}
      <span className="min-w-0 text-[11px] leading-tight text-notch-muted">{label}</span>
      {children}
    </div>
  );
}

function Toggle({
  checked,
  onChange,
  label,
}: {
  checked: boolean;
  onChange: (value: boolean) => void;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative h-[16px] w-[30px] shrink-0 cursor-pointer rounded-full transition-colors ${
        checked ? "bg-[var(--accent)]" : "bg-white/15"
      }`}
    >
      <span
        className="absolute top-[2px] size-[12px] rounded-full bg-white transition-[left] duration-150"
        style={{ left: checked ? 16 : 2 }}
      />
    </button>
  );
}

/** Ring colours on offer: one row of warm, one of cool, one of neutrals. */
const SWATCHES = [
  "#ff3b30", "#ff7b00", "#ffb300", "#ffe600", "#a3e635", "#34d399",
  "#00d3a7", "#22d3ee", "#38bdf8", "#60a5fa", "#818cf8", "#a78bfa",
  "#d946ef", "#fb7185", "#f5f5f7", "#9ca3af",
];

/** The hue of a `#rrggbb`, so the slider starts where the colour is. */
function colourHue(hex: string | undefined): number {
  if (!hex || hex.length !== 7) return 0;
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const span = max - Math.min(r, g, b);
  if (span === 0) return 0;
  const hue =
    max === r ? (g - b) / span + (g < b ? 6 : 0) : max === g ? (b - r) / span + 2 : (r - g) / span + 4;
  return Math.round(hue * 60);
}

/** `#rrggbb` for a hue on the slider, at a fixed saturation and lightness. */
function hueColour(hue: number): string {
  const f = (n: number) => {
    const k = (n + hue / 30) % 12;
    const value = 0.62 - 0.32 * Math.max(-1, Math.min(k - 3, 9 - k, 1));
    return Math.round(255 * value)
      .toString(16)
      .padStart(2, "0");
  };
  return `#${f(0)}${f(8)}${f(4)}`;
}

/**
 * A provider's ring colour: the traffic light, one of a set of swatches, or
 * any hue off the slider. Windows' own colour dialog used to do this, which
 * meant a system window over the notch and the notch held open behind it.
 */
function RingColourPicker({
  value,
  onChange,
  name,
}: {
  value: string | undefined;
  onChange: (value: string | undefined) => void;
  name: string;
}) {
  const { t } = useI18n();
  const { anchorRef, open, toggle, close, float } = useCardFloat();
  const auto = "conic-gradient(var(--tone-low), var(--tone-mid), var(--tone-high), var(--tone-low))";
  // The slider owns its position while dragging: driving it straight from the
  // colour meant it snapped back to where the colour's hue rounded to.
  const [hue, setHue] = useState(() => colourHue(value));
  useEffect(() => setHue(colourHue(value)), [value]);

  return (
    <>
      <button
        ref={anchorRef}
        type="button"
        onClick={toggle}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={t("settings.ringColour", { name })}
        title={value ?? t("settings.autoColour")}
        className="swatch"
        data-open={open || undefined}
        style={{ background: value ?? auto }}
      />

      {float(
        <div className="colour-pop" role="dialog" aria-label={t("settings.ringColour", { name })}>
          <button
            type="button"
            className="colour-auto"
            data-selected={!value || undefined}
            onClick={() => {
              onChange(undefined);
              close();
            }}
          >
            <span className="swatch" style={{ background: auto }} />
            {t("settings.autoColour")}
          </button>

          <div className="colour-grid">
            {SWATCHES.map((colour) => (
              <button
                key={colour}
                type="button"
                className="swatch"
                data-selected={value?.toLowerCase() === colour || undefined}
                style={{ background: colour }}
                aria-label={colour}
                title={colour}
                onClick={() => {
                  onChange(colour);
                  close();
                }}
              />
            ))}
          </div>

          <label className="colour-hue">
            <span className="text-[9.5px] text-notch-faint">{t("settings.colourHue")}</span>
            <input
              type="range"
              min={0}
              max={359}
              value={hue}
              aria-label={t("settings.colourHue")}
              onChange={(e) => {
                const next = Number(e.target.value);
                setHue(next);
                onChange(hueColour(next));
              }}
            />
          </label>
        </div>,
      )}
    </>
  );
}

/**
 * The Ollama address, edited locally and saved on Enter or when the field
 * loses focus. Saving per keystroke would re-poll every provider and rewrite
 * the config file for each character typed.
 */
function OllamaField({
  value,
  label,
  onCommit,
}: {
  value: string;
  label: string;
  onCommit: (value: string) => void;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);

  const commit = () => {
    const next = draft.trim();
    if (next && next !== value) onCommit(next);
    else setDraft(value);
  };

  return (
    <input
      type="text"
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onFocus={interactive.onFocus}
      onBlur={() => {
        commit();
        interactive.onBlur();
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape") {
          setDraft(value);
          e.currentTarget.blur();
        }
      }}
      spellCheck={false}
      className="w-[150px] shrink-0 rounded bg-white/8 px-1.5 py-[2px] text-[10px] outline-none focus:bg-white/12"
      aria-label={label}
    />
  );
}


/**
 * What a plan costs its subscriber, in dollars a month.
 *
 * Blank means "don't work this out for me", which is why an empty field
 * clears the setting rather than being rejected as invalid.
 */
function PlanPriceField({
  value,
  label,
  onCommit,
}: {
  value: number | undefined;
  label: string;
  onCommit: (value: number | null) => void;
}) {
  const asText = (n: number | undefined) => (n == null ? "" : String(n));
  const [draft, setDraft] = useState(asText(value));
  useEffect(() => setDraft(asText(value)), [value]);

  const commit = () => {
    const text = draft.trim().replace(",", ".");
    if (!text) {
      if (value != null) onCommit(null);
      return;
    }
    const next = Number(text);
    if (Number.isFinite(next) && next > 0 && next !== value) onCommit(next);
    else setDraft(asText(value));
  };

  return (
    <span className="flex shrink-0 items-center gap-1 text-[10px] text-notch-faint">
      $
      <input
        type="text"
        inputMode="decimal"
        value={draft}
        placeholder="—"
        onChange={(e) => setDraft(e.target.value)}
        onFocus={interactive.onFocus}
        onBlur={() => {
          commit();
          interactive.onBlur();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "Escape") {
            setDraft(asText(value));
            e.currentTarget.blur();
          }
        }}
        spellCheck={false}
        className="tnum w-[54px] rounded bg-white/8 px-1.5 py-[2px] text-right text-[10px] text-notch-text outline-none focus:bg-white/12"
        aria-label={label}
      />
    </span>
  );
}

/** The settings card, so a dropdown can float inside it. */
const PanelContext = createContext<React.RefObject<HTMLDivElement | null> | null>(null);

/**
 * A little panel that floats over the settings card, anchored to a control.
 *
 * Everything the notch draws has to stay inside the card: the window is
 * clipped to it, so a native popup or a page-level menu is simply cut off at
 * its edge. So these render into the card, flip above their control when
 * there's no room below, and cap their height to the space they have.
 */
function useCardFloat() {
  const panelRef = useContext(PanelContext);
  const anchorRef = useRef<HTMLButtonElement | null>(null);
  const floatRef = useRef<HTMLDivElement | null>(null);
  const [style, setStyle] = useState<React.CSSProperties | null>(null);
  const open = style !== null;

  const place = () => {
    const panel = panelRef?.current;
    const anchor = anchorRef.current;
    if (!panel || !anchor) return;

    const card = panel.getBoundingClientRect();
    const box = anchor.getBoundingClientRect();
    const gap = 4;
    const margin = 8;
    // Right-aligned with the control, which sits flush with the card's right
    // edge: the two line up.
    const right = Math.max(margin, card.right - box.right);
    const below = card.bottom - box.bottom - gap - margin;
    const above = box.top - card.top - gap - margin;

    setStyle(
      below >= Math.min(160, above)
        ? { top: box.bottom - card.top + gap, right, maxHeight: Math.max(96, below) }
        : { bottom: card.bottom - box.top + gap, right, maxHeight: Math.max(96, above) },
    );
  };

  // A scroll or a click elsewhere would leave it stranded where it was.
  //
  // "Elsewhere" has to exclude the panel itself: closing on mousedown inside
  // it tore the option out from under the pointer, so the click that followed
  // landed on nothing and the choice was lost.
  useEffect(() => {
    if (!open) return;
    const close = (event: Event) => {
      const target = event.target as Node;
      if (anchorRef.current?.contains(target) || floatRef.current?.contains(target)) {
        return;
      }
      setStyle(null);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("scroll", close, true);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("scroll", close, true);
    };
  }, [open]);

  const float = (content: React.ReactNode) =>
    open && panelRef?.current
      ? createPortal(
          <div ref={floatRef} className="card-float reveal" style={style ?? undefined}>
            {content}
          </div>,
          panelRef.current,
        )
      : null;

  return {
    anchorRef,
    open,
    float,
    close: () => setStyle(null),
    toggle: () => (open ? setStyle(null) : place()),
  };
}

/**
 * A dropdown in the notch's own language rather than the system's.
 *
 * The list floats over the card rather than expanding in place: in place it
 * pushed the rows below it around and grew the whole card. It also can't be a
 * native popup or a fixed-position menu — the window is clipped to what the
 * card occupies, so anything outside the card is simply cut off. So it renders
 * into the card itself, anchored under its button, flipping above when there
 * isn't room below.
 */
function Picker<T extends string>({
  value,
  options,
  label,
  onChange,
  onHighlight,
}: {
  value: T;
  options: { value: T; label: string }[];
  label: string;
  onChange: (value: T) => void;
  /** Called as an option is chosen, for a sound to play itself. */
  onHighlight?: (value: T) => void;
}) {
  const { anchorRef, open, toggle, close, float } = useCardFloat();
  const current = options.find((option) => option.value === value);

  return (
    <>
      <button
        ref={anchorRef}
        type="button"
        className="picker-button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={label}
        onClick={toggle}
      >
        <span className="picker-value">{current?.label ?? value}</span>
        <ChevronDown
          className="size-3 shrink-0 transition-transform"
          data-open={open || undefined}
        />
      </button>

      {float(
        <div className="picker-list" role="listbox" aria-label={label}>
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              role="option"
              aria-selected={option.value === value}
              className="picker-option"
              data-selected={option.value === value || undefined}
              onClick={() => {
                onChange(option.value);
                onHighlight?.(option.value);
                close();
              }}
            >
              <Check
                className="size-2.5 shrink-0 opacity-0"
                data-shown={option.value === value || undefined}
              />
              <span className="picker-option-label">{option.label}</span>
            </button>
          ))}
        </div>,
      )}
    </>
  );
}

/**
 * The provider rows, in the order the notch draws them.
 *
 * A stored order can be short (written before a provider existed) or carry a
 * name this build doesn't know, so it is treated as a preference rather than
 * the list itself: known ids first in the order given, anything left over
 * appended.
 */
function orderedProviders(rows: ProviderRow[], order: string[] | undefined): ProviderRow[] {
  const known = new Map(rows.map((row) => [row.key, row]));
  const sorted = (order ?? []).flatMap((key) => {
    const row = known.get(key);
    if (!row) return [];
    known.delete(key);
    return [row];
  });
  return [...sorted, ...known.values()];
}

/**
 * Providers, reorderable by their handle.
 *
 * The drag is pointer-based rather than HTML5 drag-and-drop: a drag image is
 * a screenshot the compositor takes of the page, and this page is a clipped,
 * layered window, so what it captures is a rectangle of black.
 */
function ProviderList({
  order,
  config,
  patch,
}: {
  order: ProviderRow[];
  config: Config;
  patch: (patch: Partial<Config>) => void;
}) {
  const { t } = useI18n();
  const [dragging, setDragging] = useState<number | null>(null);
  const [over, setOver] = useState<number | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  const rowAt = (clientY: number) => {
    const rows = listRef.current?.children;
    if (!rows || rows.length === 0) return 0;
    for (let i = 0; i < rows.length; i += 1) {
      const box = rows[i].getBoundingClientRect();
      if (clientY < box.top + box.height / 2) return i;
    }
    return rows.length - 1;
  };

  const commit = (from: number, to: number) => {
    const ids = order.map((row) => row.key);
    const [moved] = ids.splice(from, 1);
    ids.splice(to, 0, moved);
    patch({ providerOrder: ids });
  };

  const muted = new Set(config.mutedProviders ?? []);

  return (
    <div ref={listRef}>
      {order.map((provider, index) => (
        <div
          key={provider.key}
          data-dragging={dragging === index || undefined}
          data-over={over === index && dragging !== index ? "" : undefined}
          className="rounded transition-colors data-dragging:opacity-50 data-over:bg-white/8"
        >
          <Row label={provider.label}>
            <span className="flex shrink-0 items-center gap-1.5">
              <button
                type="button"
                aria-label={`${provider.label} — ${muted.has(provider.key) ? t("settings.unmute") : t("settings.mute")}`}
                title={muted.has(provider.key) ? t("settings.unmute") : t("settings.mute")}
                onClick={() => {
                  const next = new Set(muted);
                  if (next.has(provider.key)) next.delete(provider.key);
                  else next.add(provider.key);
                  patch({ mutedProviders: [...next] });
                }}
                className="cursor-pointer rounded p-1 text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text"
              >
                {muted.has(provider.key) ? (
                  <BellOff className="size-3" />
                ) : (
                  <Bell className="size-3" />
                )}
              </button>
              <RingColourPicker
                name={provider.label}
                value={config.ringColors?.[provider.key]}
                onChange={(colour) => {
                  const ringColors = { ...(config.ringColors ?? {}) };
                  if (colour) ringColors[provider.key] = colour;
                  else delete ringColors[provider.key];
                  patch({ ringColors });
                }}
              />
              <Toggle
                label={provider.label}
                checked={config.providers[provider.key] ?? true}
                onChange={(enabled) =>
                  patch({
                    providers: { ...config.providers, [provider.key]: enabled },
                  })
                }
              />
              <span
                role="button"
                tabIndex={-1}
                aria-label={t("settings.reorder")}
                title={t("settings.reorder")}
                onPointerDown={(e) => {
                  e.currentTarget.setPointerCapture(e.pointerId);
                  setDragging(index);
                  setOver(index);
                  interactive.onFocus();
                }}
                onPointerMove={(e) => {
                  if (dragging === null) return;
                  setOver(rowAt(e.clientY));
                }}
                onPointerUp={() => {
                  if (dragging !== null && over !== null && over !== dragging) {
                    commit(dragging, over);
                  }
                  setDragging(null);
                  setOver(null);
                  interactive.onBlur();
                }}
                className="cursor-grab p-1 text-notch-faint transition-colors hover:text-notch-text active:cursor-grabbing"
              >
                <GripVertical className="size-3" />
              </span>
            </span>
          </Row>
        </div>
      ))}
    </div>
  );
}

/** The chimes, plus silence — only offered where a chime may be dropped. */
const SOUND_CHOICES = [...SOUND_IDS, "none"] as const;

/**
 * Pick the chime, and hear it. Choosing one plays it, so the list can be
 * walked through by ear; the speaker replays the current one.
 */
function SoundPicker({
  value,
  volume,
  enabled,
  label,
  allowNone,
  onChange,
}: {
  value: string;
  volume: number;
  enabled: boolean;
  label: string;
  /** Whether this chime may be silenced on its own. */
  allowNone?: boolean;
  onChange: (value: string) => void;
}) {
  const { t } = useI18n();
  const choices = allowNone ? SOUND_CHOICES : SOUND_IDS;
  const options = choices.map((sound) => ({
    value: sound as string,
    label: t(`sound.${sound}`),
  }));
  return (
    // Dimmed rather than hidden while the chime is off, so the choice is
    // still visible next to the switch that silences it.
    <div
      data-disabled={!enabled || undefined}
      className="transition-opacity data-disabled:pointer-events-none data-disabled:opacity-40"
    >
      <Row label={label}>
        <span className="flex shrink-0 items-center gap-1">
          <Picker
            value={value}
            label={label}
            options={options}
            onChange={onChange}
            onHighlight={(sound) => sound !== "none" && playSound(sound, volume / 100)}
          />
          <button
            type="button"
            onClick={() => value !== "none" && playSound(value, volume / 100)}
            aria-label={t("settings.soundPlay")}
            title={t("settings.soundPlay")}
            disabled={value === "none"}
            className="cursor-pointer rounded p-1 text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text disabled:cursor-default disabled:opacity-30"
          >
            <Volume2 className="size-3" />
          </button>
        </span>
      </Row>
    </div>
  );
}

/** One level for both chimes: they are the same speaker in the same room. */
function VolumeRow({
  volume,
  enabled,
  preview,
  onVolume,
}: {
  volume: number;
  enabled: boolean;
  preview: string;
  onVolume: (value: number) => void;
}) {
  const { t } = useI18n();
  return (
    <div
      data-disabled={!enabled || undefined}
      className="transition-opacity data-disabled:pointer-events-none data-disabled:opacity-40"
    >
      <Row label={t("settings.volume")}>
        <span className="flex shrink-0 items-center gap-1.5">
          <input
            type="range"
            min={0}
            max={100}
            step={5}
            value={volume}
            onChange={(e) => onVolume(Number(e.target.value))}
            // Hear the level you land on, not every step on the way.
            onPointerUp={() => playSound(preview, volume / 100)}
            onKeyUp={() => playSound(preview, volume / 100)}
            className="h-1 w-[92px] shrink-0 cursor-pointer accent-[var(--accent)]"
            aria-label={t("settings.volume")}
          />
          <span className="tnum w-[26px] text-right text-[9.5px] text-notch-faint">{volume}%</span>
        </span>
      </Row>
    </div>
  );
}

/**
 * Let the agents report their own events.
 *
 * Its state is whether the hook is actually in the agents' settings, not a
 * stored preference: those files are the user's, other tools write to them
 * too, and one can be removed by hand at any time.
 */
function AgentHooksRow() {
  const { t } = useI18n();
  const [installed, setInstalled] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    ipc.agentHooksInstalled().then((state) => setInstalled(state ?? false));
  }, []);

  return (
    <>
      <Row label={t("settings.hooks")}>
        <Toggle
          label={t("settings.hooks")}
          checked={installed ?? false}
          onChange={(next) => {
            if (busy) return;
            setBusy(true);
            setInstalled(next); // optimistic: the switch shouldn't lag
            ipc
              .setAgentHooks(next)
              .then((state) => setInstalled(state ?? next))
              .finally(() => setBusy(false));
          }}
        />
      </Row>
      <p className="pb-1 text-[9.5px] leading-snug text-notch-faint">{t("settings.hooksHint")}</p>
    </>
  );
}

/**
 * How new releases arrive, where the last look left things, and a way to look
 * now. Installing is never a setting: it always waits for a click on the
 * update ring.
 */
function UpdatesRow({
  mode,
  status,
  onMode,
  onCheck,
  onAct,
}: {
  mode: UpdatesMode;
  status: UpdateStatus | null;
  onMode: (mode: UpdatesMode) => void;
  onCheck: () => void;
  /** Download what was found, or install what is ready: the ring's click,
   *  here too, since "later" takes the ring away. */
  onAct: () => void;
}) {
  const { t } = useI18n();
  const version = status?.version ?? "";
  const fraction = status ? downloadFraction(status) : null;

  let line: string;
  switch (status?.phase) {
    case "checking":
      line = t("settings.updatesChecking");
      break;
    case "available":
      line = t("settings.updatesFound", { version });
      break;
    case "downloading":
      line = t("settings.updatesDownloading", {
        version,
        pct: fraction === null ? "" : formatPct(fraction * 100),
      }).trim();
      break;
    case "ready":
      line = t("settings.updatesReady", { version });
      break;
    case "installing":
      line = t("update.card.installing");
      break;
    case "failed":
      line = t("settings.updatesFailed");
      break;
    default: {
      const ago = formatAgo(status?.checkedAt ?? null, t);
      line = ago ? t("settings.updatesUpToDate", { ago }) : t("settings.updatesNotChecked");
    }
  }
  const busy = status?.phase === "checking";
  // What the button does: with a release in hand, the step it's waiting for;
  // otherwise, look for one.
  const act =
    status?.phase === "ready"
      ? t("update.card.restart")
      : status?.phase === "available"
        ? t("update.card.download")
        : null;

  return (
    <>
      <Row label={t("settings.updates")}>
        <SegmentedControl<UpdatesMode>
          value={mode}
          options={[
            { value: "auto", label: t("settings.updatesAuto") },
            { value: "notify", label: t("settings.updatesNotify") },
            { value: "off", label: t("settings.updatesOff") },
          ]}
          onChange={onMode}
        />
      </Row>
      <div className="flex items-center justify-between gap-2 pb-1">
        <span
          className="tnum min-w-0 text-[9.5px] leading-snug text-notch-faint"
          title={status?.error ?? undefined}
        >
          {line}
        </span>
        {act ? (
          <button
            type="button"
            onClick={onAct}
            className="shrink-0 cursor-pointer rounded-md bg-[var(--accent)] px-2 py-[3px] text-[10px] font-semibold text-black transition-[filter] hover:brightness-110"
          >
            {act}
          </button>
        ) : (
          <button
            type="button"
            onClick={onCheck}
            disabled={busy || status?.phase === "downloading" || status?.phase === "installing"}
            className="flex shrink-0 cursor-pointer items-center gap-1 rounded-md bg-white/6 px-2 py-[3px] text-[10px] text-notch-muted transition-colors hover:bg-white/12 hover:text-notch-text disabled:cursor-default disabled:opacity-50"
          >
            <RefreshCw className={`size-3 ${busy ? "activity-spin" : ""}`} aria-hidden />
            {t("settings.updatesCheck")}
          </button>
        )}
      </div>
      <p className="pb-1 text-[9.5px] leading-snug text-notch-faint">{t("settings.updatesHint")}</p>
    </>
  );
}

/**
 * Which apps the notch stays behind instead of floating over. The apps
 * themselves are chosen in the window picker, from live thumbnails.
 */
function StayBehindSection({
  fullscreen,
  apps,
  onFullscreen,
  onApps,
  onOpenPicker,
}: {
  fullscreen: boolean;
  apps: string[];
  onFullscreen: (value: boolean) => void;
  onApps: (value: string[]) => void;
  onOpenPicker: () => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <p className="py-1 text-[10px] text-notch-faint">{t("settings.stayBehind")}</p>
      <Row label={t("settings.fullscreen")}>
        <Toggle label={t("settings.fullscreen")} checked={fullscreen} onChange={onFullscreen} />
      </Row>
      <Row label={t("settings.specificApps")}>
        <button
          type="button"
          onClick={onOpenPicker}
          className="flex shrink-0 cursor-pointer items-center gap-0.5 rounded-md bg-white/6 py-[3px] pr-1 pl-2 text-[10px] text-notch-muted transition-colors hover:bg-white/12 hover:text-notch-text"
        >
          {apps.length > 0 ? t("settings.chosen", { n: apps.length }) : t("settings.choose")}
          <ChevronRight className="size-3" />
        </button>
      </Row>

      {apps.length > 0 && (
        <div className="flex flex-wrap gap-1 py-1">
          {apps.map((exe) => (
            <span
              key={exe}
              className="flex items-center gap-1 rounded-md bg-white/8 py-[3px] pr-1 pl-2 text-[10px] capitalize"
              title={exe}
            >
              {appLabel(exe)}
              <button
                type="button"
                onClick={() => onApps(apps.filter((a) => a !== exe))}
                aria-label={t("settings.stopStayingBehind", { app: appLabel(exe) })}
                className="cursor-pointer rounded px-[3px] leading-none text-notch-faint transition-colors hover:bg-white/10 hover:text-notch-text"
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
    </>
  );
}

function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <div className="flex shrink-0 gap-[2px] rounded-md bg-white/6 p-[2px]">
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          onClick={() => onChange(option.value)}
          className={`cursor-pointer rounded px-1.5 py-[2px] text-[10px] transition-colors ${
            value === option.value
              ? "bg-white/14 text-notch-text"
              : "text-notch-faint hover:text-notch-muted"
          }`}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function SettingsPanel({
  config,
  rings,
  monitors,
  version,
  update,
  onCheckUpdates,
  onUpdateAct,
  hasWhatsNew,
  onWhatsNew,
  onChange,
  onClose,
  onOpenConfigDir,
  onQuit,
}: Props) {
  const { t } = useI18n();
  const panelRef = useRef<HTMLDivElement | null>(null);
  const patch = (changes: Partial<Config>) => onChange({ ...config, ...changes });

  // Closing the panel mid-edit unmounts the field before it can blur, which
  // would leave the notch stuck open and holding focus.
  useEffect(() => () => void ipc.setInteractive(false), []);

  const [view, setView] = useState<"main" | "picker">("main");
  if (view === "picker") {
    return (
      <div className="reveal">
        <WindowPicker
          selected={config.stayBelowApps ?? []}
          onSave={(stayBelowApps) => patch({ stayBelowApps })}
          onClose={() => setView("main")}
        />
      </div>
    );
  }

  // Grouped by what someone is looking for when they open this, rather than
  // by which switch happens to be a boolean. Four headings are enough to
  // find anything without scrolling twice.
  const sections: {
    title: string;
    rows: { key: keyof Config; label: string; fallback?: boolean }[];
  }[] = [
    {
      title: t("settings.groupNotch"),
      rows: [
        { key: "alwaysExpanded", label: t("settings.alwaysExpanded") },
        { key: "clickThroughWhenCollapsed", label: t("settings.clickThrough") },
        { key: "peekOnAttention", label: t("settings.peek") },
      ],
    },
    {
      title: t("settings.groupAlerts"),
      rows: [
        { key: "notifyOnThresholds", label: t("settings.alerts") },
        // The chimes and their level follow this row; see below.
        { key: "notifySound", label: t("settings.sound"), fallback: true },
        { key: "notifyPulse", label: t("settings.pulse"), fallback: true },
      ],
    },
    {
      title: t("settings.groupLimits"),
      rows: [
        // The ring-swap switch follows this row; see below.
        { key: "showWeekly", label: t("settings.showWeekly"), fallback: true },
        { key: "estimateTokens", label: t("settings.estimate"), fallback: true },
        { key: "resetAsCountdown", label: t("settings.countdown") },
      ],
    },
    {
      title: t("settings.groupSystem"),
      rows: [{ key: "launchAtLogin", label: t("settings.launchAtLogin") }],
    },
  ];

  return (
    <PanelContext.Provider value={panelRef}>
    <div className="relative flex flex-col" ref={panelRef}>
      <div className="flex items-center gap-1.5 border-b border-white/8 px-2.5 py-2">
        <button
          type="button"
          onClick={onClose}
          aria-label={t("common.back")}
          className="cursor-pointer rounded p-1 text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text"
        >
          <ArrowLeft className="size-3" />
        </button>
        <span className="flex-1 text-[12px] font-medium">{t("settings.title")}</span>
        {hasWhatsNew && (
          <button
            type="button"
            onClick={onWhatsNew}
            className="cursor-pointer rounded px-1 py-[1px] text-[9px] text-[var(--accent)] transition-colors hover:bg-white/8"
          >
            {t("whatsNew.link")}
          </button>
        )}
        <span className="tnum text-[9px] text-notch-faint">v{version}</span>
      </div>

      <div className="thin-scroll max-h-[420px] overflow-y-auto px-2.5 py-1">
        <p className="py-1 text-[10px] text-notch-faint">{t("settings.groupAppearance")}</p>
        <Row label={t("settings.language")}>
          <SegmentedControl<LanguageSetting>
            value={(config.language as LanguageSetting | undefined) ?? "auto"}
            options={LANGUAGES}
            onChange={(language) => patch({ language })}
          />
        </Row>

        <Row label={t("settings.edge")}>
          <SegmentedControl
            value={config.edge}
            options={EDGES.map((edge) => ({ value: edge, label: t(`edge.${edge}`) }))}
            onChange={(edge) => patch({ edge })}
          />
        </Row>

        <Row label={t("settings.size")}>
          <SegmentedControl
            value={config.size}
            options={SIZES}
            onChange={(size) => patch({ size })}
          />
        </Row>

        <Row label={t("settings.position")}>
          <span className="flex shrink-0 items-center gap-1.5">
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={config.edgeOffset}
              onChange={(e) => patch({ edgeOffset: Number(e.target.value) })}
              className="h-1 w-[92px] shrink-0 cursor-pointer accent-[var(--accent)]"
              aria-label={t("settings.position")}
            />
            {/* Alt-dragging the notch is quick but imprecise; this is how you
                get back to the middle without nudging pixel by pixel. */}
            <button
              type="button"
              onClick={() => patch({ edgeOffset: 0.5 })}
              aria-label={t("settings.recentre")}
              title={t("settings.recentre")}
              disabled={config.edgeOffset === 0.5}
              className="cursor-pointer rounded p-1 text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text disabled:cursor-default disabled:opacity-30"
            >
              <Crosshair className="size-3" />
            </button>
          </span>
        </Row>

        <Row label={t("settings.accent")}>
          <div className="flex shrink-0 gap-1">
            {ACCENTS.map((colour) => (
              <button
                key={colour}
                type="button"
                aria-label={t("settings.accentColour", { colour })}
                onClick={() => patch({ accent: colour })}
                className={`size-[14px] cursor-pointer rounded-full transition-transform ${
                  config.accent === colour
                    ? "ring-2 ring-white/70 ring-offset-1 ring-offset-transparent"
                    : "hover:scale-110"
                }`}
                style={{ background: colour }}
              />
            ))}
          </div>
        </Row>

        {monitors.length > 1 && (
          <Row label={t("settings.display")}>
            <Picker
              label={t("settings.display")}
              value={config.monitor.kind === "index" ? String(config.monitor.index) : "primary"}
              options={[
                { value: "primary", label: t("settings.displayPrimary") },
                ...monitors.map((monitor) => ({
                  value: String(monitor.index),
                  label: t("settings.displayItem", {
                    n: monitor.index + 1,
                    w: monitor.width,
                    h: monitor.height,
                  }),
                })),
              ]}
              onChange={(choice) =>
                patch({
                  monitor:
                    choice === "primary"
                      ? { kind: "primary" }
                      : { kind: "index", index: Number(choice) },
                })
              }
            />
          </Row>
        )}

        <div className="my-1 h-px bg-white/8" />

        {sections.map((section) => (
          <div key={section.title}>
            <p className="py-1 text-[10px] text-notch-faint">{section.title}</p>
            {section.rows.map(({ key, label, fallback }) => (
              <div key={key}>
            <Row label={label}>
              <Toggle
                label={label}
                checked={Boolean(config[key] ?? fallback)}
                onChange={(value) => patch({ [key]: value } as Partial<Config>)}
              />
            </Row>
            {/* Which of the two windows gets the ring. Dimmed rather than
                hidden when there is no second window to swap with. */}
            {key === "showWeekly" && (
              <div
                data-disabled={!(config.showWeekly ?? true) || undefined}
                className="transition-opacity data-disabled:pointer-events-none data-disabled:opacity-40"
              >
                <Row label={t("settings.weeklyOnRing")}>
                  <Toggle
                    label={t("settings.weeklyOnRing")}
                    checked={config.weeklyOnRing ?? false}
                    onChange={(weeklyOnRing) => patch({ weeklyOnRing })}
                  />
                </Row>
              </div>
            )}
            {/* The two chimes and their level, right under the switch that
                turns them on. Finishing and being asked are different news,
                so they get different sounds. */}
            {key === "notifySound" && (
              <>
                <SoundPicker
                  value={config.notifySoundId ?? "chime"}
                  volume={config.notifyVolume ?? 70}
                  enabled={config.notifySound ?? true}
                  label={t("settings.soundPick")}
                  onChange={(notifySoundId) => patch({ notifySoundId })}
                />
                <SoundPicker
                  value={config.notifyWaitingSoundId ?? "ping"}
                  volume={config.notifyVolume ?? 70}
                  enabled={config.notifySound ?? true}
                  label={t("settings.soundWaiting")}
                  allowNone
                  onChange={(notifyWaitingSoundId) => patch({ notifyWaitingSoundId })}
                />
                <SoundPicker
                  value={config.notifyLimitSoundId ?? "drop"}
                  volume={config.notifyVolume ?? 70}
                  enabled={config.notifySound ?? true}
                  label={t("settings.soundLimit")}
                  allowNone
                  onChange={(notifyLimitSoundId) => patch({ notifyLimitSoundId })}
                />
                <SoundPicker
                  value={config.notifyRecoverSoundId ?? "arp"}
                  volume={config.notifyVolume ?? 70}
                  enabled={config.notifySound ?? true}
                  label={t("settings.soundRecover")}
                  allowNone
                  onChange={(notifyRecoverSoundId) => patch({ notifyRecoverSoundId })}
                />
                <VolumeRow
                  volume={config.notifyVolume ?? 70}
                  enabled={config.notifySound ?? true}
                  preview={config.notifySoundId ?? "chime"}
                  onVolume={(notifyVolume) => patch({ notifyVolume })}
                />
                  </>
                )}
            {key === "launchAtLogin" && (
              <UpdatesRow
                mode={config.updates ?? "auto"}
                status={update}
                onMode={(updates) => patch({ updates })}
                onCheck={onCheckUpdates}
                onAct={onUpdateAct}
              />
            )}
              </div>
            ))}
            <div className="my-1 h-px bg-white/8" />
          </div>
        ))}

        <StayBehindSection
          fullscreen={config.stayBelowFullscreen ?? false}
          apps={config.stayBelowApps ?? []}
          onFullscreen={(stayBelowFullscreen) => patch({ stayBelowFullscreen })}
          onApps={(stayBelowApps) => patch({ stayBelowApps })}
          onOpenPicker={() => setView("picker")}
        />

        <div className="my-1 h-px bg-white/8" />

        <p className="py-1 text-[10px] text-notch-faint">{t("settings.providers")}</p>
        <ProviderList
          order={orderedProviders(rings, config.providerOrder)}
          config={config}
          patch={patch}
        />

        {/* Only the providers whose spending the notch can actually price:
            a plan's worth needs a token count behind it. Rides on the token
            estimate, so it stays out of the way when that is off. */}
        {(config.estimateTokens ?? true) && (
          <>
            <div className="my-1 h-px bg-white/8" />
            <p className="py-1 text-[10px] text-notch-faint">{t("settings.planUsd")}</p>
            {PRICED_PROVIDERS.map((provider) => (
              <Row key={provider.id} label={provider.label}>
                <PlanPriceField
                  value={config.planUsd?.[provider.id]}
                  label={`${provider.label} — ${t("settings.planUsd")}`}
                  onCommit={(usd) => {
                    const planUsd = { ...(config.planUsd ?? {}) };
                    if (usd == null) delete planUsd[provider.id];
                    else planUsd[provider.id] = usd;
                    patch({ planUsd });
                  }}
                />
              </Row>
            ))}
            <p className="pb-1 text-[9px] leading-tight text-notch-faint">
              {t("settings.planUsdHint")}
            </p>
          </>
        )}

        <div className="my-1 h-px bg-white/8" />

        <AgentHooksRow />

        <Row label={t("settings.ollama")}>
          <OllamaField
            value={config.ollamaUrl}
            label={t("settings.ollama")}
            onCommit={(ollamaUrl) => patch({ ollamaUrl })}
          />
        </Row>

        <div className="my-1 h-px bg-white/8" />

        <div className="flex gap-1.5 py-1.5">
          <button
            type="button"
            onClick={onOpenConfigDir}
            className="flex flex-1 cursor-pointer items-center justify-center gap-1 rounded bg-white/8 py-1 text-[10px] transition-colors hover:bg-white/14"
          >
            <FolderOpen className="size-3" /> {t("settings.configFolder")}
          </button>
          <button
            type="button"
            onClick={onQuit}
            className="flex cursor-pointer items-center justify-center gap-1 rounded bg-white/8 px-2 py-1 text-[10px] text-state-bad transition-colors hover:bg-state-bad/20"
          >
            <Power className="size-3" /> {t("settings.quit")}
          </button>
        </div>
      </div>
    </div>
    </PanelContext.Provider>
  );
}
