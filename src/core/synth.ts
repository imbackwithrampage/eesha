// Every sound the app plays, rendered in code: there are no audio files.
// The score is the table in docs/ARCHITECTURE.md §10. Timbre: a sine with a triangle one
// octave up at −12 dB, 6 ms attack, exponential release; "noise" is band-passed white noise.
// Each sound is rendered once at preload by an OfflineAudioContext (src/core/sound.ts).

import type { SoundName } from "./sound";

const RATE = 44100;
/** Master level: a single full voice peaks at −14 dBFS. */
const MASTER = 10 ** (-14 / 20);

/** Pitch over time: [ms from the start of the voice, Hz], glided exponentially. */
type Bend = [ms: number, hz: number][];

interface Voice {
  /** Start, in ms. */
  at?: number;
  /** Pitch, or the centre of the band for noise. */
  hz: number | Bend;
  /** Length, in ms. The release tail runs on to three times this. */
  ms: number;
  db?: number;
  noise?: boolean;
  /** Detune, in cents. */
  cents?: number;
  /** Square-ish: adds the third harmonic. */
  h3?: boolean;
  /** Vibrato: [rate in Hz, depth in cents]. */
  vib?: [number, number];
  /** Low-pass cutoff in Hz, for a dull voice. */
  lp?: number;
}

const E3 = 164.81, A3 = 220, C4 = 261.63, E4 = 329.63, F4 = 349.23, A4 = 440, B4 = 493.88;
const C5 = 523.25, Cs5 = 554.37, E5 = 659.25, Fs5 = 739.99, G5 = 783.99, Gs5 = 830.61;
const A5 = 880, B5 = 987.77, C6 = 1046.5;

/** Notes one after the other, `each` ms apart and long; the last one may ring longer. */
const seq = (notes: number[], each: number, last = each, db = 0): Voice[] =>
  notes.map((hz, i) => ({ at: i * each, hz, ms: i === notes.length - 1 ? last : each, db }));

const TICK: Voice = { hz: 2000, ms: 15, noise: true };
const MOTIF = seq([A4, E5, Fs5, A5], 90, 240);

const SCORE: Record<SoundName, Voice[]> = {
  peek: [{ hz: G5, ms: 50 }],
  open: seq([E5, A5], 70, 90),
  close: seq([A5, E5], 70, 90),
  hover: [{ hz: C6, ms: 30, db: -6 }],
  blip: [{ hz: A5, ms: 40 }],
  tick: [TICK],
  pop: [{ hz: [[0, 300], [60, 900]], ms: 60 }],
  approval: seq([A4, Cs5, E5], 60),
  // The same, the last note bent up a tone.
  question: [...seq([A4, Cs5], 60), { at: 120, hz: [[0, E5], [8, E5], [40, Fs5]], ms: 60 }],
  approve: seq([E5, A5], 60, 120),
  finish: seq([A4, Cs5, E5, A5], 60, 200),
  error: [{ hz: A3, ms: 160, h3: true, db: -3 }, { hz: A3, cents: -12, ms: 160, h3: true, db: -3 }],
  annoyed: seq([E4, C4], 80),
  dizzy: [{ hz: A4, ms: 300, vib: [6, 40] }],
  work: [{ hz: 1200, ms: 12, noise: true, db: -10 }],
  think: [{ hz: A3, ms: 90, db: -8 }, { at: 180, hz: A3, ms: 90, db: -8 }],
  search: [{ hz: C6, ms: 30 }, { at: 60, hz: C6, ms: 30 }],
  send: [{ hz: [[0, 800], [140, 4000]], ms: 140, noise: true, db: -4 }],
  // Legato: one voice that slides from the first note to the second.
  love: [{ hz: [[0, C5], [100, C5], [135, E5]], ms: 240 }],
  proud: seq([E5, Gs5, B5], 50, 160),
  wink: [{ hz: [[0, B5], [8, B5], [30, C6]], ms: 45 }],
  yawn: [{ hz: [[0, A4], [400, E4]], ms: 400, db: -6 }],
  attach: [TICK, { at: 15, hz: A5, ms: 60 }],
  rate: [{ hz: E3, ms: 200, lp: 600 }],
  sleep: seq([E5, B4], 120, 120, -8),
  gulp: [{ hz: [[0, A4], [60, F4], [120, A4]], ms: 120 }],
  slap: [{ hz: 600, ms: 40, noise: true }],
  greet: MOTIF,
  greeting: MOTIF,
};

/** The soft limiter: unity gain for quiet signals, never past ±0.76. */
const LIMIT = Float32Array.from({ length: 1025 }, (_, i) => Math.tanh(i / 512 - 1));

let white: AudioBuffer | null = null;
/** One second of white noise, shared by every noise voice. */
function whiteNoise(): AudioBuffer {
  if (!white) {
    white = new AudioBuffer({ length: RATE, sampleRate: RATE });
    const data = white.getChannelData(0);
    for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
  }
  return white;
}

/** Sets a parameter to a pitch, or schedules its glide. `k` scales it (a partial). */
function pitch(param: AudioParam, hz: number | Bend, t: number, k = 1) {
  if (typeof hz === "number") {
    param.value = hz * k;
    return;
  }
  hz.forEach(([ms, f], i) => {
    if (i === 0) param.setValueAtTime(f * k, t);
    else param.exponentialRampToValueAtTime(f * k, t + ms / 1000);
  });
}

function voice(c: OfflineAudioContext, out: AudioNode, v: Voice) {
  const t = (v.at ?? 0) / 1000;
  const len = v.ms / 1000;
  const end = t + len * 3;
  const attack = v.noise ? 0.001 : 0.006;
  // The band-pass keeps little of the noise, less the lower the band: make it up, so a
  // noise voice peaks about where a tone does.
  const makeup = v.noise ? 86 / Math.sqrt(typeof v.hz === "number" ? v.hz : v.hz[0][1]) : 1;

  // Envelope: up in 6 ms (1 ms for noise), held for a third of the length, then an exponential release.
  const env = c.createGain();
  env.gain.setValueAtTime(0, t);
  env.gain.linearRampToValueAtTime(10 ** ((v.db ?? 0) / 20) * makeup, t + attack);
  env.gain.setTargetAtTime(0, t + Math.max(attack, len * 0.35), len * 0.3);
  env.connect(out);

  let into: AudioNode = env;
  if (v.noise || v.lp) {
    const filter = c.createBiquadFilter();
    filter.type = v.noise ? "bandpass" : "lowpass";
    filter.Q.value = v.noise ? 1.4 : 0.7;
    pitch(filter.frequency, v.lp ?? v.hz, t);
    filter.connect(env);
    into = filter;
  }

  if (v.noise) {
    const src = c.createBufferSource();
    src.buffer = whiteNoise();
    src.connect(into);
    src.start(t);
    src.stop(end);
    return;
  }

  let lfo: GainNode | null = null;
  if (v.vib) {
    const osc = c.createOscillator();
    osc.frequency.value = v.vib[0];
    lfo = c.createGain();
    lfo.gain.value = v.vib[1];
    osc.connect(lfo);
    osc.start(t);
    osc.stop(end);
  }

  // The partials: [multiple of the pitch, wave, level]. The first two sum to 1.
  const partials: [number, OscillatorType, number][] = [[1, "sine", 0.8], [2, "triangle", 0.2]];
  if (v.h3) partials.push([3, "sine", 0.27]);
  for (const [k, type, level] of partials) {
    const osc = c.createOscillator();
    osc.type = type;
    pitch(osc.frequency, v.hz, t, k);
    osc.detune.value = v.cents ?? 0;
    lfo?.connect(osc.detune);
    const gain = c.createGain();
    gain.gain.value = level;
    osc.connect(gain).connect(into);
    osc.start(t);
    osc.stop(end);
  }
}

/** Renders one sound into a mono 44.1 kHz buffer. */
export function renderSound(name: SoundName): Promise<AudioBuffer> {
  const voices = SCORE[name];
  const seconds = Math.max(...voices.map((v) => (v.at ?? 0) + v.ms * 3)) / 1000 + 0.01;
  const c = new OfflineAudioContext(1, Math.ceil(seconds * RATE), RATE);
  const master = c.createGain();
  master.gain.value = MASTER;
  const limiter = c.createWaveShaper();
  limiter.curve = LIMIT;
  master.connect(limiter).connect(c.destination);
  for (const v of voices) voice(c, master, v);
  return c.startRendering();
}
