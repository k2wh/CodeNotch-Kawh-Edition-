/**
 * What changed, version by version, in the app's own languages.
 *
 * Carried in the build rather than fetched, so the card works offline, reads
 * in the language the notch is set to, and says the same thing however the
 * version arrived — updated by the ring or installed by hand.
 *
 * Newest first. Keep each line to one change, in the words of someone using
 * the notch rather than writing it, and add an entry in the same commit that
 * bumps the version.
 */

import type { Lang } from "./i18n";

export interface Release {
  version: string;
  /** ISO date, shown beside the version. */
  date: string;
  changes: Record<Lang, string[]>;
}

export const CHANGELOG: Release[] = [
  {
    version: "0.2.2",
    date: "2026-09-22",
    changes: {
      en: [
        "The Claude ring works for people who only have the Claude desktop app: where Claude Code never signed in, it reads the app's own record of the plan — the same 5-hour and weekly windows — instead of showing nothing.",
        "If that app isn't running, its last reading is marked stale rather than passed off as current, and the 5-hour window drops once it is old enough to have rolled over.",
      ],
      pt: [
        "O anel do Claude passa a funcionar para quem só tem o app do Claude no computador: onde o Claude Code nunca entrou, ele lê o registro que o próprio app guarda do plano, com as mesmas janelas de 5 horas e de semana, em vez de não mostrar nada.",
        "Se esse app não estiver aberto, a última leitura dele aparece marcada como desatualizada, e a janela de 5 horas some quando já é velha o bastante para ter virado.",
      ],
      es: [
        "El anillo de Claude ya funciona para quien solo tiene la app de Claude: donde Claude Code nunca inició sesión, lee el registro que la propia app guarda del plan, con las mismas ventanas de 5 horas y de semana, en lugar de no mostrar nada.",
        "Si esa app no está abierta, su última lectura se marca como desactualizada, y la ventana de 5 horas desaparece cuando ya es vieja como para haber cambiado.",
      ],
    },
  },
  {
    version: "0.2.1",
    date: "2026-09-22",
    changes: {
      en: [
        "Claude Opus 5.5 has its own rates, so what a plan has returned counts its tokens at what they would have cost: $4 in and $20 out per million, with cache reads at a twentieth of input rather than the usual tenth.",
        "The first version to arrive through the update ring.",
      ],
      pt: [
        "O Claude Opus 5.5 entrou na tabela de preços, então o quanto o plano rendeu conta os tokens dele pelo que custariam: $4 de entrada e $20 de saída por milhão, com a leitura de cache a um vinte avos da entrada, e não ao décimo de sempre.",
        "A primeira versão a chegar pelo anel de atualização.",
      ],
      es: [
        "Claude Opus 5.5 ya tiene sus propias tarifas, así que lo que el plan ha rendido cuenta sus tokens por lo que habrían costado: $4 de entrada y $20 de salida por millón, con las lecturas de caché a una veinteava parte de la entrada, no a la décima de siempre.",
        "La primera versión que llega por el anillo de actualización.",
      ],
    },
  },
  {
    version: "0.2.0",
    date: "2026-09-22",
    changes: {
      en: [
        "CodeNotch updates itself: a ring on the strip downloads the next version in the background, and a click installs it and brings the notch back.",
        "Settings say how updates arrive — downloaded, only announced, or never — and can look for one now.",
        "This card, on the first launch of a new version.",
        "Linux and macOS: “Start at sign-in” works, which it never did outside Windows.",
      ],
      pt: [
        "O CodeNotch se atualiza: um anel na faixa baixa a versão nova em segundo plano, e um clique instala e traz o notch de volta.",
        "As configurações dizem como a atualização chega — baixada, só avisada ou nunca — e podem procurar uma agora.",
        "Este cartão, na primeira vez que uma versão nova abre.",
        "Linux e macOS: o “Abrir ao iniciar a sessão” passou a funcionar, o que só acontecia no Windows.",
      ],
      es: [
        "CodeNotch se actualiza solo: un anillo en la tira descarga la versión nueva en segundo plano, y un clic la instala y devuelve el notch.",
        "Los ajustes dicen cómo llega la actualización — descargada, solo anunciada o nunca — y pueden buscar una ahora.",
        "Esta tarjeta, la primera vez que se abre una versión nueva.",
        "Linux y macOS: “Abrir al iniciar sesión” ya funciona, cosa que solo pasaba en Windows.",
      ],
    },
  },
];

/** What changed in a version, if this build carries its notes. */
export function changesFor(version: string | null | undefined): Release | null {
  if (!version) return null;
  return CHANGELOG.find((release) => release.version === version) ?? null;
}
