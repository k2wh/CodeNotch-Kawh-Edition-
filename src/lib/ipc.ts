/**
 * Typed wrappers around the Tauri commands and events.
 *
 * Everything the webview can ask the backend to do goes through here, so the
 * command names live in exactly one place.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  Bootstrap,
  Config,
  HudMetrics,
  HudRegions,
  HudState,
  MonitorInfo,
  OpenWindow,
  ProviderId,
  Telemetry,
  UpdateStatus,
} from "../types";

export const TELEMETRY_EVENT = "codenotch://telemetry";
export const CONFIG_EVENT = "codenotch://config";
export const HUD_STATE_EVENT = "codenotch://hud-state";
export const UPDATE_EVENT = "codenotch://update";

/** True when running inside the Tauri shell rather than a bare browser. */
export const IN_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * Call a command, swallowing failures.
 *
 * A HUD that throws a dialog because a window nudge failed would be worse than
 * one that quietly carries on, so IPC errors are logged and dropped.
 */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T | null> {
  if (!IN_TAURI) return null;
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    console.warn(`[codenotch] ${command} failed:`, error);
    return null;
  }
}

export const ipc = {
  ready: () => call<Bootstrap>("hud_ready"),
  hover: (hovering: boolean) => call<void>("hud_hover", { hovering }),
  /** Where the strip and the card are drawn, in window-relative logical px. */
  setRegions: (regions: HudRegions) => call<void>("hud_set_regions", { regions }),
  togglePin: () => call<boolean>("hud_toggle_pin"),
  /** Hold the notch open and let it take keyboard focus while a field is in use. */
  setInteractive: (on: boolean) => call<void>("hud_set_interactive", { on }),
  getConfig: () => call<Config>("get_config"),
  getMetrics: () => call<HudMetrics>("get_metrics"),
  setConfig: (config: Config) => call<Config>("set_config", { config }),
  refreshNow: () => call<Telemetry>("refresh_now"),
  /** Raise the app a session runs in (`host`), else the provider's usual apps. */
  focusProvider: (provider: ProviderId, titleHint?: string | null, host?: string | null) =>
    call<boolean>("focus_provider", {
      provider,
      titleHint: titleHint ?? null,
      host: host ?? null,
    }),
  setHidden: (hidden: boolean) => call<void>("set_hidden", { hidden }),
  peek: (seconds?: number) => call<void>("peek", { seconds: seconds ?? null }),
  listMonitors: () => call<MonitorInfo[]>("list_monitors"),
  /** Install or remove the agents' hooks; returns whether they're in place. */
  setAgentHooks: (enabled: boolean) => call<boolean>("set_agent_hooks", { enabled }),
  agentHooksInstalled: () => call<boolean>("agent_hooks_installed"),
  listOpenApps: () => call<string[]>("list_open_apps"),
  /** Open app windows with thumbnails; takes a moment to capture. */
  listOpenWindows: () => call<OpenWindow[]>("list_open_windows"),
  openConfigDir: () => call<void>("open_config_dir"),
  quit: () => call<void>("quit_app"),
  updateStatus: () => call<UpdateStatus>("update_status"),
  /** "Check now": resolves once the look is done. */
  updateCheck: () => call<UpdateStatus>("update_check"),
  /** The update ring's click: download, try again, or install and restart. */
  updateAct: () => call<void>("update_act"),
  /** "Later": hide the ring until the next look. */
  updateDismiss: () => call<void>("update_dismiss"),
};

/** Subscribe to a backend event; returns an unsubscribe function. */
export function subscribe<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  if (!IN_TAURI) return Promise.resolve(() => {});
  return listen<T>(event, (e) => handler(e.payload));
}

export const events = {
  telemetry: (handler: (t: Telemetry) => void) =>
    subscribe<Telemetry>(TELEMETRY_EVENT, handler),
  config: (handler: (c: Config) => void) => subscribe<Config>(CONFIG_EVENT, handler),
  hudState: (handler: (s: HudState) => void) =>
    subscribe<HudState>(HUD_STATE_EVENT, handler),
  update: (handler: (s: UpdateStatus) => void) =>
    subscribe<UpdateStatus>(UPDATE_EVENT, handler),
};
