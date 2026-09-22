/**
 * Visual picker for the apps the notch stays behind.
 *
 * Shows a live thumbnail of every open window, so the choice is "that one"
 * rather than guessing executable names. Selection is per app: the setting
 * matches by executable, so picking one Chrome window covers every Chrome
 * window, and the cards say so.
 */

import { useEffect, useMemo, useState } from "react";
import { AppWindow, ArrowLeft, Check, RefreshCw, X } from "lucide-react";

import { useI18n } from "../lib/i18n";
import { ipc } from "../lib/ipc";
import type { OpenWindow } from "../types";

/** `chrome.exe` → `chrome`, for display; the config keeps the exe name. */
export const appLabel = (exe: string) => exe.replace(/\.exe$/i, "");

const key = (exe: string) => exe.toLowerCase();

interface Props {
  /** Apps (exe names) currently in the list. */
  selected: string[];
  onSave: (apps: string[]) => void;
  onClose: () => void;
}

export function WindowPicker({ selected, onSave, onClose }: Props) {
  const { t } = useI18n();
  const [windows, setWindows] = useState<OpenWindow[] | null>(null);
  const [picked, setPicked] = useState<Map<string, string>>(
    () => new Map(selected.map((exe) => [key(exe), exe])),
  );

  const load = () => {
    setWindows(null);
    ipc.listOpenWindows().then((list) => setWindows(list ?? []));
  };
  useEffect(load, []);

  // How many windows each app has open, for the "all N windows" note.
  const perApp = useMemo(() => {
    const counts = new Map<string, number>();
    for (const w of windows ?? []) counts.set(key(w.exe), (counts.get(key(w.exe)) ?? 0) + 1);
    return counts;
  }, [windows]);

  // Listed apps with no window open right now still need a way off the list.
  const absent = useMemo(
    () => [...picked.entries()].filter(([k]) => !perApp.has(k)).map(([, exe]) => exe),
    [picked, perApp],
  );

  const toggle = (exe: string) =>
    setPicked((current) => {
      const next = new Map(current);
      if (next.has(key(exe))) next.delete(key(exe));
      else next.set(key(exe), exe);
      return next;
    });

  const unchanged =
    picked.size === selected.length && selected.every((exe) => picked.has(key(exe)));

  return (
    <div className="flex max-h-[520px] flex-col">
      <div className="flex items-center gap-1.5 border-b border-white/8 px-2.5 py-2">
        <button
          type="button"
          onClick={onClose}
          aria-label={t("common.back")}
          className="cursor-pointer rounded p-1 text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text"
        >
          <ArrowLeft className="size-3" />
        </button>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="text-[12px] font-medium leading-tight">{t("picker.title")}</span>
          <span className="text-[9.5px] leading-tight text-notch-faint">
            {t("picker.subtitle")}
          </span>
        </div>
        <button
          type="button"
          onClick={load}
          aria-label={t("picker.refresh")}
          title={t("picker.refresh")}
          className="cursor-pointer rounded p-1 text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text"
        >
          <RefreshCw className={`size-3 ${windows === null ? "animate-spin" : ""}`} />
        </button>
      </div>

      <div className="thin-scroll min-h-0 flex-1 overflow-y-auto p-2.5">
        <div className="grid grid-cols-2 gap-2">
          {windows === null
            ? Array.from({ length: 4 }, (_, i) => <div key={i} className="picker-skeleton" />)
            : windows.map((w, i) => {
                const on = picked.has(key(w.exe));
                const siblings = perApp.get(key(w.exe)) ?? 1;
                return (
                  <button
                    key={w.id}
                    type="button"
                    onClick={() => toggle(w.exe)}
                    className="picker-card"
                    data-selected={on || undefined}
                    style={{ animationDelay: `${Math.min(i, 12) * 28}ms` }}
                    aria-pressed={on}
                    title={w.title}
                  >
                    <span className="picker-thumb">
                      {w.thumbnail ? (
                        <img src={w.thumbnail} alt="" draggable={false} />
                      ) : (
                        <span className="picker-thumb-empty">
                          <AppWindow className="size-5" />
                          <span>{w.minimized ? t("picker.minimized") : t("picker.noPreview")}</span>
                        </span>
                      )}
                      <span className="picker-check" aria-hidden>
                        <Check className="size-3" strokeWidth={3} />
                      </span>
                    </span>
                    <span className="picker-meta">
                      <span className="picker-app">
                        {appLabel(w.exe)}
                        {on && siblings > 1 && (
                          <span className="picker-note">
                            {t("picker.allWindows", { n: siblings })}
                          </span>
                        )}
                      </span>
                      <span className="picker-title">{w.title}</span>
                    </span>
                  </button>
                );
              })}
        </div>

        {windows !== null && windows.length === 0 && (
          <p className="py-6 text-center text-[11px] text-notch-faint">{t("picker.none")}</p>
        )}

        {absent.length > 0 && (
          <div className="mt-3">
            <p className="pb-1 text-[10px] text-notch-faint">{t("picker.absent")}</p>
            <div className="flex flex-wrap gap-1">
              {absent.map((exe) => (
                <span
                  key={exe}
                  className="flex items-center gap-1 rounded-md bg-white/8 py-[3px] pr-1 pl-2 text-[10px]"
                  title={exe}
                >
                  {appLabel(exe)}
                  <button
                    type="button"
                    onClick={() => toggle(exe)}
                    aria-label={t("picker.remove", { app: appLabel(exe) })}
                    className="cursor-pointer rounded p-[1px] text-notch-faint transition-colors hover:bg-white/10 hover:text-notch-text"
                  >
                    <X className="size-2.5" />
                  </button>
                </span>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="flex items-center gap-2 border-t border-white/8 px-2.5 py-2">
        <span className="flex-1 text-[10px] text-notch-muted">
          {picked.size === 0
            ? t("picker.noneSelected")
            : picked.size === 1
              ? t("picker.oneSelected")
              : t("picker.manySelected", { n: picked.size })}
        </span>
        <button
          type="button"
          onClick={onClose}
          className="cursor-pointer rounded-md px-2 py-1 text-[10.5px] text-notch-muted transition-colors hover:bg-white/8 hover:text-notch-text"
        >
          {t("common.cancel")}
        </button>
        <button
          type="button"
          disabled={unchanged}
          onClick={() => {
            onSave([...picked.values()]);
            onClose();
          }}
          className="cursor-pointer rounded-md bg-[var(--accent)] px-2.5 py-1 text-[10.5px] font-semibold text-black transition-[opacity,transform] hover:brightness-110 active:scale-95 disabled:cursor-default disabled:opacity-35"
        >
          {t("common.save")}
        </button>
      </div>
    </div>
  );
}
