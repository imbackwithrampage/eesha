// The arithmetic behind the live bars: which bins of a spectrum form each band,
// how a band's energy becomes a bar height, and how heights move over time.
// Ported from reference/vorssaint-utils/Sources/Vorssaint/Services/Notch/
// NotchAudioLevelSupport.swift (band edges, `bandRanges`, `bandLevels`,
// `Smoother`; its `barIndex` is in src/features/audio-extras/index.ts). The
// transform is plain radix-2, written here.

use std::f32::consts::PI;
use std::ops::Range;

pub const BANDS: usize = 7;
/// Band edges in hertz, from the bass up to the air.
const EDGES_HZ: [f64; BANDS + 1] = [50.0, 120.0, 250.0, 500.0, 1000.0, 2000.0, 4500.0, 12_000.0];
/// Levels this many decibels under full scale read as silence.
const FLOOR_DB: f64 = 60.0;
/// Updates per second of sound.
const UPDATES: usize = 30;
/// Samples in one look at the spectrum.
pub const SIZE: usize = 1024;

/// The bins of a spectrum with `size / 2` usable bins that belong to each band.
/// Every band keeps at least one bin, and bin 0, the DC offset, belongs to none.
pub fn band_ranges(rate: f64, size: usize) -> [Range<usize>; BANDS] {
    let usable = (size / 2).max(2);
    let width = if rate > 0.0 && size > 0 { rate / size as f64 } else { 1.0 };
    std::array::from_fn(|band| {
        let lower = ((EDGES_HZ[band] / width) as usize).clamp(1, usable - 1);
        let upper = ((EDGES_HZ[band + 1] / width) as usize).clamp(lower + 1, usable);
        lower..upper
    })
}

/// Each band's mean magnitude on a decibel scale with a floor, from 0 to 1.
/// Magnitudes are relative to full scale.
pub fn band_levels(magnitudes: &[f32], bands: &[Range<usize>; BANDS]) -> [f64; BANDS] {
    std::array::from_fn(|i| {
        let bins = &magnitudes[bands[i].start.min(magnitudes.len())..bands[i].end.min(magnitudes.len())];
        if bins.is_empty() {
            return 0.0;
        }
        let mean = f64::from(bins.iter().sum::<f32>()) / bins.len() as f64;
        if mean <= 0.0 {
            return 0.0;
        }
        ((20.0 * mean.log10() + FLOOR_DB) / FLOOR_DB).clamp(0.0, 1.0)
    })
}

/// Bars rise fast and fall slowly, and the loudest band sets the scale so a
/// quiet album still fills the meter. The scale itself relaxes slowly.
pub struct Smoother {
    levels: [f64; BANDS],
    ceiling: f64,
}

impl Default for Smoother {
    fn default() -> Self {
        Smoother { levels: [0.0; BANDS], ceiling: 0.05 }
    }
}

impl Smoother {
    const ATTACK: f64 = 0.55;
    const DECAY: f64 = 0.12;
    const CEILING_DECAY: f64 = 0.995;
    const PEAK_TARGET: f64 = 0.92;

    pub fn next(&mut self, raw: [f64; BANDS]) -> [f64; BANDS] {
        let loudest = raw.iter().copied().fold(0.0, f64::max);
        self.ceiling = loudest.max(self.ceiling * Self::CEILING_DECAY).max(0.05);
        let gain = Self::PEAK_TARGET / self.ceiling;
        for (level, raw) in self.levels.iter_mut().zip(raw) {
            let target = (raw * gain).clamp(0.0, 1.0);
            *level += (target - *level) * if target > *level { Self::ATTACK } else { Self::DECAY };
        }
        self.levels
    }
}

/// Mono sound in, the seven bar heights out, thirty times a second of sound.
/// It holds the last 1024 samples and nothing else of what it hears.
pub struct Analyzer {
    ring: [f32; SIZE],
    at: usize,
    since: usize,
    every: usize,
    window: [f32; SIZE],
    /// cos and sin of -2πk/SIZE, for the transform.
    turn: [(f32, f32); SIZE / 2],
    ranges: [Range<usize>; BANDS],
    smoother: Smoother,
}

impl Analyzer {
    pub fn new(rate: u32) -> Self {
        Analyzer {
            ring: [0.0; SIZE],
            at: 0,
            since: 0,
            every: (rate as usize / UPDATES).max(1),
            // Hann.
            window: std::array::from_fn(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / SIZE as f32).cos()),
            turn: std::array::from_fn(|k| {
                let a = -2.0 * PI * k as f32 / SIZE as f32;
                (a.cos(), a.sin())
            }),
            ranges: band_ranges(f64::from(rate), SIZE),
            smoother: Smoother::default(),
        }
    }

    pub fn feed(&mut self, mono: &[f32], mut out: impl FnMut([f32; BANDS])) {
        for sample in mono {
            self.ring[self.at] = *sample;
            self.at = (self.at + 1) % SIZE;
            self.since += 1;
            if self.since >= self.every {
                self.since = 0;
                out(self.look().map(|v| v as f32));
            }
        }
    }

    fn look(&mut self) -> [f64; BANDS] {
        let mut re: [f32; SIZE] = std::array::from_fn(|i| self.ring[(self.at + i) % SIZE] * self.window[i]);
        let mut im = [0.0f32; SIZE];
        fft(&mut re, &mut im, &self.turn);
        // A full-scale sine under a Hann window peaks at SIZE / 4.
        let magnitudes: [f32; SIZE / 2] = std::array::from_fn(|k| re[k].hypot(im[k]) / (SIZE as f32 / 4.0));
        self.smoother.next(band_levels(&magnitudes, &self.ranges))
    }
}

/// In place, SIZE points.
fn fft(re: &mut [f32; SIZE], im: &mut [f32; SIZE], turn: &[(f32, f32); SIZE / 2]) {
    let bits = SIZE.trailing_zeros();
    for i in 0..SIZE {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut half = 1;
    while half < SIZE {
        let stride = SIZE / (2 * half);
        for start in (0..SIZE).step_by(2 * half) {
            for k in 0..half {
                let (c, s) = turn[k * stride];
                let (a, b) = (start + k, start + k + half);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        half *= 2;
    }
}
