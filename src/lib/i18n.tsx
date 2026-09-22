/**
 * Interface languages.
 *
 * English is the source: its keys define what every other language has to
 * provide, and the `Dict` type makes a missing translation a compile error.
 * Text that comes from the backend (provider messages, window names) is
 * written in English there and translated here by its exact wording, so the
 * Rust side doesn't need to know about languages at all.
 */

import { createContext, useContext, useMemo, type ReactNode } from "react";

export type Lang = "en" | "pt" | "es";
/** What the config stores: a language, or follow Windows. */
export type LanguageSetting = "auto" | Lang;

export const LANGUAGES: { value: LanguageSetting; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "en", label: "EN" },
  { value: "pt", label: "PT" },
  { value: "es", label: "ES" },
];

const en = {
  "common.back": "Back",
  "common.cancel": "Cancel",
  "common.save": "Save",

  "settings.title": "Settings",
  "settings.language": "Language",
  "settings.edge": "Screen edge",
  "edge.top": "Top",
  "edge.bottom": "Bottom",
  "edge.left": "Left",
  "edge.right": "Right",
  "settings.size": "Size",
  "settings.position": "Position along edge",
  "settings.accent": "Accent",
  "settings.accentColour": "Accent {colour}",
  "settings.display": "Display",
  "settings.displayPrimary": "Primary",
  "settings.displayItem": "Display {n} · {w}×{h}",
  "settings.alwaysExpanded": "Always expanded",
  "settings.clickThrough": "Click through when resting",
  "settings.peek": "Peek when an agent needs you",
  "settings.alerts": "Alert at 80% and 100%",
  "settings.sound": "Chime when an agent answers",
  "settings.soundPick": "Chime",
  "settings.soundWaiting": "Chime when asking",
  "settings.soundLimit": "Chime when a limit runs out",
  "settings.soundRecover": "Chime when it comes back",
  "settings.soundPlay": "Play this chime",
  "settings.volume": "Volume",
  "settings.pulse": "Pulse the ring when one answers",
  "sound.chime": "Chime",
  "sound.ping": "Ping",
  "sound.bell": "Bell",
  "sound.marimba": "Marimba",
  "sound.arp": "Rising",
  "sound.drop": "Drop",
  "sound.glass": "Glass",
  "sound.pulse": "Taps",
  "sound.none": "Silent",
  "settings.showWeekly": "Show weekly limit",
  "settings.weeklyOnRing": "Weekly on the ring",
  "settings.estimate": "Estimate limits in tokens",
  "settings.planUsd": "What the plan costs (US$/month)",
  "settings.planUsdHint": "Set it and the card says what the plan has returned",
  "settings.countdown": "Show resets as countdown",
  "settings.launchAtLogin": "Start at sign-in",
  "settings.hooks": "Instant agent updates",
  "settings.hooksHint":
    "Adds a hook to Claude Code and Codex so they report the moment a turn ends. Their settings files are merged, never replaced.",
  "settings.stayBehind": "Stay behind",
  "settings.fullscreen": "Full-screen games & videos",
  "settings.specificApps": "Specific apps",
  "settings.choose": "Choose",
  "settings.chosen": "{n} chosen",
  "settings.stopStayingBehind": "Stop staying behind {app}",
  "settings.groupNotch": "The notch",
  "settings.groupAlerts": "Alerts",
  "settings.groupLimits": "Limits",
  "settings.groupSystem": "System",
  "settings.groupAppearance": "Appearance",
  "settings.providers": "Providers · ring colour",
  "settings.mute": "Mute alerts",
  "settings.unmute": "Unmute alerts",
  "settings.reorder": "Drag to reorder",
  "settings.recentre": "Recentre",
  "settings.autoColour": "Automatic (green / yellow / red)",
  "settings.autoColourFor": "{name}: automatic colour",
  "settings.ringColour": "{name} ring colour",
  "settings.colourHue": "Any hue",
  "settings.ollama": "Ollama address",
  "settings.configFolder": "Config folder",
  "settings.quit": "Quit",
  "strip.settings": "CodeNotch settings",

  "picker.title": "Stay behind apps",
  "picker.subtitle": "The notch hides behind these while they're in front",
  "picker.refresh": "Refresh",
  "picker.minimized": "Minimized",
  "picker.noPreview": "No preview",
  "picker.allWindows": "all {n}",
  "picker.none": "No other windows open",
  "picker.absent": "In the list, not open now",
  "picker.remove": "Remove {app}",
  "picker.noneSelected": "No apps selected",
  "picker.oneSelected": "1 app selected",
  "picker.manySelected": "{n} apps selected",

  "popover.title": "{name} Usage",
  "popover.aria": "{name} usage",
  "popover.resetsIn": "Resets in {time}",
  "popover.resetsAt": "Resets {time}",
  "popover.used": "{value} used",
  "popover.inputTitle": "Tokens new to the model (typed, read, cached)",
  "popover.cacheTitle": "Conversation re-read from cache, billed at a tenth",
  "popover.outputTitle": "Tokens generated",
  "popover.valueNote": "at list price · plan costs {plan} over {days}",
  "popover.empty": "Nothing to report yet.",

  "ring.unknown": "{name}: usage unknown",
  "ring.used": "{name}: {pct}% used",
  "ring.weekly": "7d",

  "health.stale": "stale",
  "health.rateLimited": "rate limited",
  "health.needsAuth": "sign in",
  "health.error": "error",
  "health.unavailable": "not running",
  "activity.generating": "working",
  "activity.awaitingInput": "needs you",
  "activity.done": "finished",

  "time.resetting": "resetting",
  "time.justNow": "just now",
  "time.secondsAgo": "{n}s ago",
  "time.minutesAgo": "{n}m ago",
  "time.hoursAgo": "{n}h ago",
  "time.daysAgo": "{n}d ago",
  "time.hoursShort": "{count}h",
  "time.daysShort": "{count} days",
};

export type Key = keyof typeof en;
type Dict = Record<Key, string>;

const pt: Dict = {
  "common.back": "Voltar",
  "common.cancel": "Cancelar",
  "common.save": "Salvar",

  "settings.title": "Configurações",
  "settings.language": "Idioma",
  "settings.edge": "Borda da tela",
  "edge.top": "Topo",
  "edge.bottom": "Base",
  "edge.left": "Esq.",
  "edge.right": "Dir.",
  "settings.size": "Tamanho",
  "settings.position": "Posição na borda",
  "settings.accent": "Destaque",
  "settings.accentColour": "Destaque {colour}",
  "settings.display": "Monitor",
  "settings.displayPrimary": "Principal",
  "settings.displayItem": "Monitor {n} · {w}×{h}",
  "settings.alwaysExpanded": "Sempre expandido",
  "settings.clickThrough": "Deixar cliques passarem em repouso",
  "settings.peek": "Abrir quando um agente precisar de você",
  "settings.alerts": "Alertar em 80% e 100%",
  "settings.sound": "Som quando um agente responder",
  "settings.soundPick": "Som",
  "settings.soundWaiting": "Som ao pedir algo",
  "settings.soundLimit": "Som ao bater o limite",
  "settings.soundRecover": "Som ao voltar do limite",
  "settings.soundPlay": "Ouvir este som",
  "settings.volume": "Volume",
  "settings.pulse": "Pulsar o anel quando responder",
  "sound.chime": "Sino",
  "sound.ping": "Toque",
  "sound.bell": "Campainha",
  "sound.marimba": "Marimba",
  "sound.arp": "Subida",
  "sound.drop": "Queda",
  "sound.glass": "Cristal",
  "sound.pulse": "Batidas",
  "sound.none": "Sem som",
  "settings.showWeekly": "Mostrar limite semanal",
  "settings.weeklyOnRing": "Semanal no círculo",
  "settings.estimate": "Estimar os limites em tokens",
  "settings.planUsd": "Quanto o plano custa (US$/mês)",
  "settings.planUsdHint": "Preencha e o card mostra o quanto o plano rendeu",
  "settings.countdown": "Renovação em contagem regressiva",
  "settings.launchAtLogin": "Abrir ao iniciar a sessão",
  "settings.hooks": "Aviso imediato dos agentes",
  "settings.hooksHint":
    "Instala um hook no Claude Code e no Codex para avisarem assim que o turno termina. Os arquivos de configuração deles são complementados, nunca substituídos.",
  "settings.stayBehind": "Ficar atrás",
  "settings.fullscreen": "Jogos e vídeos em tela cheia",
  "settings.specificApps": "Apps específicos",
  "settings.choose": "Escolher",
  "settings.chosen": "{n} escolhidos",
  "settings.stopStayingBehind": "Parar de ficar atrás de {app}",
  "settings.groupNotch": "A barra",
  "settings.groupAlerts": "Alertas",
  "settings.groupLimits": "Limites",
  "settings.groupSystem": "Sistema",
  "settings.groupAppearance": "Aparência",
  "settings.providers": "Provedores · cor do anel",
  "settings.mute": "Silenciar avisos",
  "settings.unmute": "Reativar avisos",
  "settings.reorder": "Arraste para reordenar",
  "settings.recentre": "Recentralizar",
  "settings.autoColour": "Automática (verde / amarelo / vermelho)",
  "settings.autoColourFor": "{name}: cor automática",
  "settings.ringColour": "Cor do anel de {name}",
  "settings.colourHue": "Outra cor",
  "settings.ollama": "Endereço do Ollama",
  "settings.configFolder": "Pasta de config.",
  "settings.quit": "Sair",
  "strip.settings": "Configurações do CodeNotch",

  "picker.title": "Ficar atrás de apps",
  "picker.subtitle": "O notch fica atrás destes quando estão em primeiro plano",
  "picker.refresh": "Atualizar",
  "picker.minimized": "Minimizada",
  "picker.noPreview": "Sem prévia",
  "picker.allWindows": "todas as {n}",
  "picker.none": "Nenhuma outra janela aberta",
  "picker.absent": "Na lista, mas fechados agora",
  "picker.remove": "Remover {app}",
  "picker.noneSelected": "Nenhum app selecionado",
  "picker.oneSelected": "1 app selecionado",
  "picker.manySelected": "{n} apps selecionados",

  "popover.title": "Uso do {name}",
  "popover.aria": "Uso do {name}",
  "popover.resetsIn": "Renova em {time}",
  "popover.resetsAt": "Renova às {time}",
  "popover.used": "{value} usado",
  "popover.inputTitle": "Tokens novos para o modelo (digitados, lidos, em cache)",
  "popover.cacheTitle": "Conversa relida do cache, cobrada a um décimo",
  "popover.outputTitle": "Tokens gerados",
  "popover.valueNote": "a preço de tabela · o plano custa {plan} em {days}",
  "popover.empty": "Nada para mostrar ainda.",

  "ring.unknown": "{name}: uso desconhecido",
  "ring.used": "{name}: {pct}% usado",
  "ring.weekly": "7d",

  "health.stale": "desatualizado",
  "health.rateLimited": "limitado",
  "health.needsAuth": "entrar",
  "health.error": "erro",
  "health.unavailable": "parado",
  "activity.generating": "trabalhando",
  "activity.awaitingInput": "precisa de você",
  "activity.done": "concluído",

  "time.resetting": "renovando",
  "time.justNow": "agora mesmo",
  "time.secondsAgo": "há {n}s",
  "time.minutesAgo": "há {n} min",
  "time.hoursAgo": "há {n} h",
  "time.daysAgo": "há {n} d",
  "time.hoursShort": "{count}h",
  "time.daysShort": "{count} dias",
};

const es: Dict = {
  "common.back": "Volver",
  "common.cancel": "Cancelar",
  "common.save": "Guardar",

  "settings.title": "Ajustes",
  "settings.language": "Idioma",
  "settings.edge": "Borde de pantalla",
  "edge.top": "Arriba",
  "edge.bottom": "Abajo",
  "edge.left": "Izq.",
  "edge.right": "Der.",
  "settings.size": "Tamaño",
  "settings.position": "Posición en el borde",
  "settings.accent": "Acento",
  "settings.accentColour": "Acento {colour}",
  "settings.display": "Pantalla",
  "settings.displayPrimary": "Principal",
  "settings.displayItem": "Pantalla {n} · {w}×{h}",
  "settings.alwaysExpanded": "Siempre expandido",
  "settings.clickThrough": "Dejar pasar clics en reposo",
  "settings.peek": "Abrir cuando un agente te necesite",
  "settings.alerts": "Avisar al 80% y 100%",
  "settings.sound": "Sonido cuando un agente responda",
  "settings.soundPick": "Sonido",
  "settings.soundWaiting": "Sonido al pedir algo",
  "settings.soundLimit": "Sonido al agotar el límite",
  "settings.soundRecover": "Sonido al recuperarse",
  "settings.soundPlay": "Escuchar este sonido",
  "settings.volume": "Volumen",
  "settings.pulse": "Pulsar el anillo al responder",
  "sound.chime": "Campana",
  "sound.ping": "Toque",
  "sound.bell": "Campanilla",
  "sound.marimba": "Marimba",
  "sound.arp": "Ascenso",
  "sound.drop": "Caída",
  "sound.glass": "Cristal",
  "sound.pulse": "Golpes",
  "sound.none": "Sin sonido",
  "settings.showWeekly": "Mostrar límite semanal",
  "settings.weeklyOnRing": "Semanal en el círculo",
  "settings.estimate": "Estimar los límites en tokens",
  "settings.planUsd": "Cuánto cuesta el plan (US$/mes)",
  "settings.planUsdHint": "Complétalo y la tarjeta dice cuánto ha rendido el plan",
  "settings.countdown": "Renovación como cuenta atrás",
  "settings.launchAtLogin": "Abrir al iniciar sesión",
  "settings.hooks": "Aviso inmediato de los agentes",
  "settings.hooksHint":
    "Instala un hook en Claude Code y Codex para que avisen en cuanto termina el turno. Sus archivos de configuración se completan, nunca se reemplazan.",
  "settings.stayBehind": "Quedarse detrás",
  "settings.fullscreen": "Juegos y vídeos a pantalla completa",
  "settings.specificApps": "Apps concretas",
  "settings.choose": "Elegir",
  "settings.chosen": "{n} elegidas",
  "settings.stopStayingBehind": "Dejar de quedarse detrás de {app}",
  "settings.groupNotch": "La barra",
  "settings.groupAlerts": "Alertas",
  "settings.groupLimits": "Límites",
  "settings.groupSystem": "Sistema",
  "settings.groupAppearance": "Apariencia",
  "settings.providers": "Proveedores · color del anillo",
  "settings.mute": "Silenciar avisos",
  "settings.unmute": "Reactivar avisos",
  "settings.reorder": "Arrastra para reordenar",
  "settings.recentre": "Recentrar",
  "settings.autoColour": "Automático (verde / amarillo / rojo)",
  "settings.autoColourFor": "{name}: color automático",
  "settings.ringColour": "Color del anillo de {name}",
  "settings.colourHue": "Otro color",
  "settings.ollama": "Dirección de Ollama",
  "settings.configFolder": "Carpeta de config.",
  "settings.quit": "Salir",
  "strip.settings": "Ajustes de CodeNotch",

  "picker.title": "Quedarse detrás de apps",
  "picker.subtitle": "El notch se queda detrás de estas cuando están al frente",
  "picker.refresh": "Actualizar",
  "picker.minimized": "Minimizada",
  "picker.noPreview": "Sin vista previa",
  "picker.allWindows": "las {n}",
  "picker.none": "No hay otras ventanas abiertas",
  "picker.absent": "En la lista, cerradas ahora",
  "picker.remove": "Quitar {app}",
  "picker.noneSelected": "Ninguna app seleccionada",
  "picker.oneSelected": "1 app seleccionada",
  "picker.manySelected": "{n} apps seleccionadas",

  "popover.title": "Uso de {name}",
  "popover.aria": "Uso de {name}",
  "popover.resetsIn": "Se renueva en {time}",
  "popover.resetsAt": "Se renueva a las {time}",
  "popover.used": "{value} usado",
  "popover.inputTitle": "Tokens nuevos para el modelo (escritos, leídos, en caché)",
  "popover.cacheTitle": "Conversación releída del caché, cobrada a una décima",
  "popover.outputTitle": "Tokens generados",
  "popover.valueNote": "a precio de lista · el plan cuesta {plan} en {days}",
  "popover.empty": "Nada que mostrar todavía.",

  "ring.unknown": "{name}: uso desconocido",
  "ring.used": "{name}: {pct}% usado",
  "ring.weekly": "7d",

  "health.stale": "desactualizado",
  "health.rateLimited": "limitado",
  "health.needsAuth": "iniciar sesión",
  "health.error": "error",
  "health.unavailable": "detenido",
  "activity.generating": "trabajando",
  "activity.awaitingInput": "te necesita",
  "activity.done": "terminado",

  "time.resetting": "renovando",
  "time.justNow": "ahora mismo",
  "time.secondsAgo": "hace {n} s",
  "time.minutesAgo": "hace {n} min",
  "time.hoursAgo": "hace {n} h",
  "time.daysAgo": "hace {n} d",
  "time.hoursShort": "{count}h",
  "time.daysShort": "{count} días",
};

const DICTS: Record<Lang, Dict> = { en, pt, es };

/** BCP 47 tag for dates and numbers. */
export const LOCALES: Record<Lang, string> = { en: "en-US", pt: "pt-BR", es: "es-ES" };

/**
 * Backend text, keyed by its exact English wording. Anything not listed (an
 * HTTP error, say) is shown as the backend wrote it.
 */
const SERVER: Record<Exclude<Lang, "en">, Record<string, string>> = {
  pt: {
    "Token expired; run `claude` to refresh": "Token expirado; rode `claude` para renovar",
    "Token rejected; run `claude` to sign in again": "Token recusado; rode `claude` para entrar de novo",
    "Rate limited; retrying shortly": "Limite de requisições atingido; tentando de novo em breve",
    "Can't reach Claude; retrying shortly": "Sem conexão com o Claude; tentando de novo em breve",
    "Sign in with `claude` to show usage limits": "Entre com `claude` para ver os limites de uso",
    "No recent Codex activity to read limits from": "Nenhuma atividade recente do Codex para ler os limites",
    "Perplexity keeps usage server-side; nothing local to read": "O Perplexity guarda o uso no servidor; nada local para ler",
    "Run `gemini` to sign in": "Rode `gemini` para entrar",
    "Sign-in expired; run `gemini` to refresh": "Login expirado; rode `gemini` para renovar",
    "Gemini reports quota server-side; no local counters": "O Gemini informa a cota no servidor; sem contadores locais",
    "Cursor reports quota server-side; no local counters": "O Cursor informa a cota no servidor; sem contadores locais",
    "Running, no models loaded": "Rodando, nenhum modelo carregado",
    "Sign in to Copilot in your editor": "Entre no Copilot pelo seu editor",
    "Unlimited on this plan": "Ilimitado neste plano",
    "No cached quota; open Copilot Chat to refresh it": "Sem cota em cache; abra o Copilot Chat para atualizar",
    "5h session": "Sessão de 5h",
    "7d all models": "7d todos os modelos",
    "5h tokens": "Tokens em 5h",
    "Requests": "Requisições",
    "Pro searches": "Pesquisas Pro",
    "Resident": "Residente",
    "On GPU": "Na GPU",
    "Completions": "Autocompletar",
    "Free trial": "Teste grátis",
    "Free": "Grátis",
    "showing figures from {age} ago": "mostrando números de {age} atrás",
  },
  es: {
    "Token expired; run `claude` to refresh": "Token caducado; ejecuta `claude` para renovarlo",
    "Token rejected; run `claude` to sign in again": "Token rechazado; ejecuta `claude` para volver a iniciar sesión",
    "Rate limited; retrying shortly": "Límite de solicitudes alcanzado; reintentando en breve",
    "Can't reach Claude; retrying shortly": "No se puede conectar con Claude; reintentando en breve",
    "Sign in with `claude` to show usage limits": "Inicia sesión con `claude` para ver los límites de uso",
    "No recent Codex activity to read limits from": "No hay actividad reciente de Codex para leer los límites",
    "Perplexity keeps usage server-side; nothing local to read": "Perplexity guarda el uso en el servidor; no hay nada local que leer",
    "Run `gemini` to sign in": "Ejecuta `gemini` para iniciar sesión",
    "Sign-in expired; run `gemini` to refresh": "La sesión caducó; ejecuta `gemini` para renovarla",
    "Gemini reports quota server-side; no local counters": "Gemini informa la cuota en el servidor; no hay contadores locales",
    "Cursor reports quota server-side; no local counters": "Cursor informa la cuota en el servidor; no hay contadores locales",
    "Running, no models loaded": "En ejecución, sin modelos cargados",
    "Sign in to Copilot in your editor": "Inicia sesión en Copilot desde tu editor",
    "Unlimited on this plan": "Ilimitado en este plan",
    "No cached quota; open Copilot Chat to refresh it": "No hay cuota en caché; abre Copilot Chat para actualizarla",
    "5h session": "Sesión de 5 h",
    "7d all models": "7d todos los modelos",
    "5h tokens": "Tokens en 5 h",
    "Requests": "Solicitudes",
    "Pro searches": "Búsquedas Pro",
    "Resident": "Residente",
    "On GPU": "En la GPU",
    "Completions": "Autocompletado",
    "Free trial": "Prueba gratuita",
    "Free": "Gratis",
    "showing figures from {age} ago": "mostrando datos de hace {age}",
  },
};

/** The language to show for a setting: "auto" follows the system. */
export function resolveLang(setting: string | undefined): Lang {
  if (setting === "en" || setting === "pt" || setting === "es") return setting;
  const system = (typeof navigator !== "undefined" ? navigator.language : "en").toLowerCase();
  if (system.startsWith("pt")) return "pt";
  if (system.startsWith("es")) return "es";
  return "en";
}

type Vars = Record<string, string | number>;

function fill(template: string, vars?: Vars): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}

export type Translate = (key: Key, vars?: Vars) => string;

function makeTranslate(lang: Lang): Translate {
  const dict = DICTS[lang];
  return (key, vars) => fill(dict[key] ?? en[key], vars);
}

/** Translate text that came from the backend in English. */
function makeServer(lang: Lang): (text: string) => string {
  if (lang === "en") return (text) => text;
  const table = SERVER[lang];
  return (text) => {
    if (table[text]) return table[text];
    // "Rate limited; retrying shortly (showing figures from 3m ago)"
    const cached = /^(.*) \(showing figures from (.+) ago\)$/.exec(text);
    if (cached) {
      const head = table[cached[1]] ?? cached[1];
      const tail = fill(table["showing figures from {age} ago"], { age: cached[2] });
      return `${head} (${tail})`;
    }
    return text;
  };
}

interface I18n {
  lang: Lang;
  locale: string;
  t: Translate;
  /** Backend text (provider messages, window and plan names). */
  server: (text: string) => string;
}

const I18nContext = createContext<I18n>({
  lang: "en",
  locale: LOCALES.en,
  t: makeTranslate("en"),
  server: (text) => text,
});

export function I18nProvider({ lang, children }: { lang: Lang; children: ReactNode }) {
  const value = useMemo<I18n>(
    () => ({ lang, locale: LOCALES[lang], t: makeTranslate(lang), server: makeServer(lang) }),
    [lang],
  );
  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18n {
  return useContext(I18nContext);
}
