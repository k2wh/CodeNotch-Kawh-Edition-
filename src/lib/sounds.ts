/**
 * The chimes the notch plays when an agent answers.
 *
 * Synthesised in the browser rather than shipped as audio files, and written
 * rather than borrowed from Windows: the system sounds read as *system*
 * events, and differ from machine to machine. These are a handful of short,
 * soft tones that sit under a working room instead of interrupting it —
 * nothing longer than a second and a half, nothing louder than it has to be.
 */

export const SOUND_IDS = [
  "chime",
  "ping",
  "bell",
  "marimba",
  "arp",
  "drop",
  "glass",
  "pulse",
] as const;

export type SoundId = (typeof SOUND_IDS)[number];

/** Overall ceiling, so a chime never startles. */
const MASTER_GAIN = 0.16;
/**
 * Idle time before the audio context is let go.
 *
 * An open context keeps a whole audio process alive — ~8 MB for something
 * used for a second at a time, hours apart. Closing it hands that back;
 * the next chime opens a new one, which costs a few milliseconds nobody
 * will hear. Comfortably longer than the longest chime.
 */
const IDLE_CLOSE_MS = 8000;

let shared: AudioContext | null = null;
let idleTimer: number | null = null;

function releaseWhenQuiet() {
  if (idleTimer !== null) window.clearTimeout(idleTimer);
  idleTimer = window.setTimeout(() => {
    idleTimer = null;
    const context = shared;
    shared = null;
    void context?.close().catch(() => {});
  }, IDLE_CLOSE_MS);
}

function context(): AudioContext | null {
  try {
    const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctor) return null;
    shared ??= new Ctor();
    // A context can start (or fall back to) suspended; resuming is a no-op
    // when it is already running.
    if (shared.state === "suspended") void shared.resume();
    return shared;
  } catch {
    return null;
  }
}

interface Voice {
  /** Hz, or the note it starts on when `glideTo` is set. */
  freq: number;
  /** Seconds from the start of the sound. */
  at?: number;
  /** Seconds the tone takes to fade out. */
  decay: number;
  type?: OscillatorType;
  gain?: number;
  /** Slide to this frequency over the voice's life. */
  glideTo?: number;
  /** Bell-like timbre: modulator frequency as a multiple of `freq`. */
  fm?: number;
}

/** Each chime, as a few voices. */
const RECIPES: Record<SoundId, Voice[]> = {
  // Two bell notes a fifth apart, the second a hair later.
  chime: [
    { freq: 1046.5, decay: 0.9, gain: 0.9 },
    { freq: 1568, at: 0.055, decay: 0.75, gain: 0.55 },
  ],
  // One clean, quick note.
  ping: [
    { freq: 1180, decay: 0.32, gain: 1 },
    { freq: 2360, decay: 0.16, gain: 0.25 },
  ],
  // Struck metal: an FM bell with a long tail.
  bell: [
    { freq: 660, decay: 1.5, gain: 0.9, fm: 1.41 },
    { freq: 1320, at: 0.02, decay: 0.6, gain: 0.2 },
  ],
  // Wooden and warm, with its octave on top.
  marimba: [
    { freq: 523.25, decay: 0.5, gain: 1, type: "triangle" },
    { freq: 1046.5, at: 0.01, decay: 0.22, gain: 0.3 },
  ],
  // Three notes up: the "done" of the set.
  arp: [
    { freq: 659.25, decay: 0.22, gain: 0.8 },
    { freq: 880, at: 0.075, decay: 0.24, gain: 0.8 },
    { freq: 1108.7, at: 0.15, decay: 0.45, gain: 0.85 },
  ],
  // A soft blip that falls away.
  drop: [{ freq: 900, decay: 0.28, gain: 1, glideTo: 420 }],
  // Airy and high, the quietest of them.
  glass: [
    { freq: 1760, decay: 1.2, gain: 0.55 },
    { freq: 2093, at: 0.04, decay: 1, gain: 0.35 },
    { freq: 2637, at: 0.08, decay: 0.7, gain: 0.18 },
  ],
  // Two dry taps, for when a chime is too much.
  pulse: [
    { freq: 720, decay: 0.12, gain: 0.9, type: "triangle" },
    { freq: 720, at: 0.13, decay: 0.14, gain: 0.7, type: "triangle" },
  ],
};

function voice(ctx: AudioContext, start: number, spec: Voice, volume: number) {
  const at = start + (spec.at ?? 0);
  const osc = ctx.createOscillator();
  osc.type = spec.type ?? "sine";
  osc.frequency.setValueAtTime(spec.freq, at);
  if (spec.glideTo) {
    osc.frequency.exponentialRampToValueAtTime(spec.glideTo, at + spec.decay);
  }

  // A modulator an inharmonic ratio above the note is what makes metal sound
  // like metal; it fades faster than the note itself.
  if (spec.fm) {
    const mod = ctx.createOscillator();
    const depth = ctx.createGain();
    mod.frequency.setValueAtTime(spec.freq * spec.fm, at);
    depth.gain.setValueAtTime(spec.freq * 1.6, at);
    depth.gain.exponentialRampToValueAtTime(1, at + spec.decay * 0.5);
    mod.connect(depth).connect(osc.frequency);
    mod.start(at);
    mod.stop(at + spec.decay + 0.05);
  }

  const gain = ctx.createGain();
  const peak = Math.max(0.0002, MASTER_GAIN * (spec.gain ?? 1) * volume);
  // Short attack, exponential fall: a struck note rather than a beep.
  gain.gain.setValueAtTime(0.0001, at);
  gain.gain.exponentialRampToValueAtTime(peak, at + 0.012);
  gain.gain.exponentialRampToValueAtTime(0.0001, at + spec.decay);

  osc.connect(gain).connect(ctx.destination);
  osc.start(at);
  osc.stop(at + spec.decay + 0.05);
}

/**
 * Play a chime at `volume` (0–1). Silently does nothing where audio isn't
 * available.
 *
 * A context can be suspended — the browser's autoplay rules, or the machine
 * waking up — in which case resuming takes a moment and scheduling notes
 * against a stopped clock would drop them. So a suspended context is resumed
 * first and the chime played once it is running.
 */
export function playSound(id: string, volume = 1) {
  const recipe = RECIPES[id as SoundId] ?? RECIPES.chime;
  const ctx = context();
  if (!ctx || volume <= 0) return;

  const ring = () => {
    const start = ctx.currentTime + 0.01;
    for (const spec of recipe) voice(ctx, start, spec, volume);
    releaseWhenQuiet();
  };

  if (ctx.state === "running") ring();
  else void ctx.resume().then(ring, () => {});
}
