//! The measurements a take is remembered by that `audio` did not already
//! have: loudness in LUFS, true peak, the six ranges, the spectrum in the
//! Listen screen's bands, and a twelve-note profile (chroma) with the chord
//! it reads as.
//!
//! Pure functions over samples, shared by the two paths that hear Live: a
//! capture read off disk, and the Mac app's tap measuring a bar in memory.
//! Nothing here touches the file system, the network or Core Audio.

use crate::audio::Audio;

/// The floor every loudness reading is clamped to: below this is silence.
pub const LUFS_FLOOR: f64 = -70.0;
/// The floor every dB level is clamped to, as on the Listen screen.
pub const FLOOR_DB: f64 = -80.0;

/// The six ranges people name when they talk about a mix — the same edges the
/// Listen screen draws (`app/src-tauri/src/listen/analysis.rs`).
pub const RANGES: [(&str, f64, f64); 6] = [
    ("sub", 20.0, 60.0),
    ("bass", 60.0, 200.0),
    ("low mids", 200.0, 500.0),
    ("mids", 500.0, 2000.0),
    ("presence", 2000.0, 6000.0),
    ("air", 6000.0, 20000.0),
];

/// Bands of the remembered spectrum, log-spaced 20 Hz – 20 kHz: the Listen
/// screen's own, so a spectrogram and the live view agree.
pub const BANDS: usize = 72;
pub const FMIN: f64 = 20.0;
pub const FMAX: f64 = 20_000.0;
/// The analysis window for a frame: 4096 points, as on the Listen screen.
pub const FFT_SIZE: usize = 4096;

pub const PITCH_CLASSES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

fn db_of_power(p: f64) -> f64 {
    if p <= 0.0 {
        FLOOR_DB
    } else {
        (10.0 * p.log10()).max(FLOOR_DB)
    }
}

// ── loudness (ITU-R BS.1770) ────────────────────────────────────────────────

/// One biquad, direct form II transposed.
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z1;
        self.z1 = self.b[1] * x - self.a[1] * y + self.z2;
        self.z2 = self.b[2] * x - self.a[2] * y;
        y
    }
}

/// The K-weighting filter for one channel: BS.1770's high shelf and high
/// pass, with the coefficients derived for any sample rate (the published
/// 48 kHz tables are this formula evaluated at 48 kHz).
#[derive(Debug, Clone, Copy)]
pub struct KWeighting {
    shelf: Biquad,
    high_pass: Biquad,
}

impl KWeighting {
    pub fn new(rate: f64) -> Self {
        let rate = if rate > 0.0 { rate } else { 48_000.0 };
        let pi = std::f64::consts::PI;
        // Stage 1: the head's shelf, +4 dB above about 1.7 kHz.
        let f0 = 1_681.974_450_955_533;
        let g = 3.999_843_853_973_347;
        let q = 0.707_175_236_955_419_6;
        let k = (pi * f0 / rate).tan();
        let vh = 10f64.powf(g / 20.0);
        let vb = vh.powf(0.499_666_774_154_541_6);
        let a0 = 1.0 + k / q + k * k;
        let shelf = Biquad {
            b: [
                (vh + vb * k / q + k * k) / a0,
                2.0 * (k * k - vh) / a0,
                (vh - vb * k / q + k * k) / a0,
            ],
            a: [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
            z1: 0.0,
            z2: 0.0,
        };
        // Stage 2: RLB, a high pass at about 38 Hz.
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let k = (pi * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        let high_pass = Biquad {
            b: [1.0, -2.0, 1.0],
            a: [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
            z1: 0.0,
            z2: 0.0,
        };
        Self { shelf, high_pass }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        self.high_pass.process(self.shelf.process(x))
    }
}

/// LUFS from a K-weighted mean square summed over channels.
pub fn lufs_of_mean_square(sum: f64) -> f64 {
    if sum <= 0.0 {
        return LUFS_FLOOR;
    }
    (-0.691 + 10.0 * sum.log10()).max(LUFS_FLOOR)
}

/// The loudness of a whole stretch of audio, ungated: for a bar that is the
/// short-term loudness the spec asks for, measured over exactly that bar.
/// Silence reads [`LUFS_FLOOR`].
pub fn lufs(audio: &Audio) -> f64 {
    let frames = audio.frames();
    if frames == 0 {
        return LUFS_FLOOR;
    }
    let mut sum = 0.0;
    for ch in audio.channels.iter().take(2) {
        let mut k = KWeighting::new(audio.sample_rate as f64);
        let mut sq = 0.0;
        for &s in ch {
            let y = k.process(s as f64);
            sq += y * y;
        }
        sum += sq / frames as f64;
    }
    // A mono file is one channel heard in both ears, as Live plays it.
    if audio.channels.len() == 1 {
        sum *= 2.0;
    }
    lufs_of_mean_square(sum)
}

/// Short-term (3 s) and momentary (400 ms) loudness, streaming: what a frame
/// carries. Blocks of 100 ms, as BS.1770 meters step.
#[derive(Debug, Clone)]
pub struct Loudness {
    rate: f64,
    k: [KWeighting; 2],
    block_len: usize,
    in_block: usize,
    acc: f64,
    /// Mean square (summed over channels) of the newest blocks, oldest first.
    blocks: std::collections::VecDeque<f64>,
}

impl Loudness {
    const SHORT_BLOCKS: usize = 30;
    const MOMENTARY_BLOCKS: usize = 4;

    pub fn new(rate: f64) -> Self {
        let rate = if rate > 0.0 { rate } else { 48_000.0 };
        Self {
            rate,
            k: [KWeighting::new(rate), KWeighting::new(rate)],
            block_len: (rate / 10.0).round().max(1.0) as usize,
            in_block: 0,
            acc: 0.0,
            blocks: std::collections::VecDeque::with_capacity(Self::SHORT_BLOCKS + 1),
        }
    }

    pub fn push(&mut self, l: &[f32], r: &[f32]) {
        for (a, b) in l.iter().zip(r) {
            let x = self.k[0].process(*a as f64);
            let y = self.k[1].process(*b as f64);
            self.acc += x * x + y * y;
            self.in_block += 1;
            if self.in_block == self.block_len {
                self.blocks.push_back(self.acc / self.block_len as f64);
                if self.blocks.len() > Self::SHORT_BLOCKS {
                    self.blocks.pop_front();
                }
                self.acc = 0.0;
                self.in_block = 0;
            }
        }
    }

    fn over(&self, n: usize) -> f64 {
        let take = self.blocks.len().min(n);
        if take == 0 {
            return LUFS_FLOOR;
        }
        let sum: f64 = self.blocks.iter().rev().take(take).sum();
        lufs_of_mean_square(sum / take as f64)
    }

    /// The newest three seconds (or what there is of them).
    pub fn short_term(&self) -> f64 {
        self.over(Self::SHORT_BLOCKS)
    }

    /// The newest 400 ms.
    pub fn momentary(&self) -> f64 {
        self.over(Self::MOMENTARY_BLOCKS)
    }

    pub fn sample_rate(&self) -> f64 {
        self.rate
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.rate);
    }
}

// ── true peak ───────────────────────────────────────────────────────────────

/// Taps per phase of the 4× interpolator. 12 keeps a full-scale sine's
/// inter-sample peak within a few hundredths of a dB.
const TP_TAPS: usize = 12;
const TP_OVERSAMPLE: usize = 4;

fn interpolator() -> Vec<[f64; TP_TAPS]> {
    // A Hann-windowed sinc, split into its four phases. Phase 0 is the
    // sample itself and needs no filter.
    let half = TP_TAPS as f64 / 2.0;
    (0..TP_OVERSAMPLE)
        .map(|phase| {
            let frac = phase as f64 / TP_OVERSAMPLE as f64;
            let mut taps = [0.0; TP_TAPS];
            let mut sum = 0.0;
            for (j, t) in taps.iter_mut().enumerate() {
                let x = j as f64 - half + 1.0 - frac;
                let sinc = if x.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * x / (half + 1.0)).cos();
                *t = sinc * w;
                sum += *t;
            }
            for t in taps.iter_mut() {
                *t /= sum;
            }
            taps
        })
        .collect()
}

/// The highest level between the samples as well as on them, in dBTP: what a
/// DAC or a codec actually reaches. Floored at −80.
pub fn true_peak(audio: &Audio) -> f64 {
    let phases = interpolator();
    let mut peak = 0.0f64;
    for ch in &audio.channels {
        let n = ch.len();
        for i in 0..n {
            peak = peak.max((ch[i] as f64).abs());
            for taps in phases.iter().skip(1) {
                let mut acc = 0.0;
                for (j, t) in taps.iter().enumerate() {
                    let at = i as isize + j as isize - (TP_TAPS / 2) as isize + 1;
                    if at >= 0 && (at as usize) < n {
                        acc += ch[at as usize] as f64 * t;
                    }
                }
                peak = peak.max(acc.abs());
            }
        }
    }
    if peak <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * peak.log10()).max(FLOOR_DB)
    }
}

// ── the six ranges ──────────────────────────────────────────────────────────

/// Energy in each of the six ranges, from the same windowed FFT pass
/// `audio::octave_energy` makes ([`crate::audio::for_each_power`]), so a
/// capture and a remembered bar split the spectrum the same way.
pub fn range_energy(audio: &Audio) -> [f64; 6] {
    let mut out = [0.0f64; 6];
    crate::audio::for_each_power(audio, |power, bin_hz| {
        for (k, e) in power.iter().enumerate().skip(1) {
            let hz = k as f64 * bin_hz;
            if let Some(r) = RANGES.iter().position(|(_, lo, hi)| hz >= *lo && hz < *hi) {
                out[r] += e;
            }
        }
    });
    out
}

/// Each range's share of the whole as dB (0 dB would be all of it), the
/// numbers the Listen screen shows under "In words".
pub fn range_shares_db(energy: &[f64; 6]) -> [f64; 6] {
    let total: f64 = energy.iter().sum();
    let mut out = [FLOOR_DB; 6];
    if total > 0.0 {
        for (o, e) in out.iter_mut().zip(energy) {
            *o = db_of_power(e / total);
        }
    }
    out
}

// ── the spectrum and chroma of one frame ────────────────────────────────────

/// The lower edge of band `i`, in Hz.
pub fn band_edge(i: usize) -> f64 {
    FMIN * (FMAX / FMIN).powf(i as f64 / BANDS as f64)
}

/// One frame's spectrum: [`BANDS`] levels in dBFS (a full-scale sine reads
/// 0 dB in its band) and the chroma, from the newest [`FFT_SIZE`] samples.
pub struct Spectrum {
    rate: f64,
    hann: Vec<f64>,
    band_of_bin: Vec<usize>,
}

impl Spectrum {
    pub fn new(rate: f64) -> Self {
        let rate = if rate > 0.0 { rate } else { 48_000.0 };
        let bin_hz = rate / FFT_SIZE as f64;
        let band_of_bin = (0..FFT_SIZE / 2)
            .map(|k| {
                let f = k as f64 * bin_hz;
                if k == 0 || !(FMIN..FMAX).contains(&f) {
                    usize::MAX
                } else {
                    (((f / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize)
                        .min(BANDS - 1)
                }
            })
            .collect();
        let hann = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FFT_SIZE as f64).cos())
            .collect();
        Self {
            rate,
            hann,
            band_of_bin,
        }
    }

    /// `mono` is the newest window, oldest sample first; shorter is padded.
    pub fn frame(&self, mono: &[f32]) -> (Vec<f32>, [f32; 12]) {
        let mut re = vec![0.0f64; FFT_SIZE];
        let mut im = vec![0.0f64; FFT_SIZE];
        for (i, s) in mono.iter().rev().take(FFT_SIZE).enumerate() {
            let at = FFT_SIZE - 1 - i;
            re[at] = *s as f64 * self.hann[at];
        }
        crate::audio::fft(&mut re, &mut im);
        let power: Vec<f64> = (0..FFT_SIZE / 2)
            .map(|k| re[k] * re[k] + im[k] * im[k])
            .collect();
        // Σ|X|² over a Hann lobe is 3·A²·N²/32 for a sine of amplitude A.
        let norm = 32.0 / 3.0 / (FFT_SIZE as f64 * FFT_SIZE as f64);
        let mut bands = vec![0.0f64; BANDS];
        for (k, &b) in self.band_of_bin.iter().enumerate() {
            if b != usize::MAX {
                bands[b] += power[k];
            }
        }
        let bands = bands.iter().map(|p| db_of_power(p * norm) as f32).collect();
        (bands, chroma(&power, self.rate / FFT_SIZE as f64))
    }
}

/// The twelve pitch classes' share of the energy between 55 Hz and 5 kHz,
/// scaled so the strongest is 1. All zero for silence.
pub fn chroma(power: &[f64], bin_hz: f64) -> [f32; 12] {
    let mut c = [0.0f64; 12];
    for (k, p) in power.iter().enumerate().skip(1) {
        let f = k as f64 * bin_hz;
        if !(55.0..5000.0).contains(&f) {
            continue;
        }
        let midi = 69.0 + 12.0 * (f / 440.0).log2();
        let pc = (midi.round() as i64).rem_euclid(12) as usize;
        c[pc] += p;
    }
    let max = c.iter().cloned().fold(0.0, f64::max);
    let mut out = [0.0f32; 12];
    if max > 0.0 {
        for (o, v) in out.iter_mut().zip(c) {
            *o = (v / max) as f32;
        }
    }
    out
}

/// The major or minor triad a chroma profile reads as, with how clearly
/// (0–1: the correlation with the triad's template). None for silence or a
/// profile no triad explains.
pub fn chord_of(chroma: &[f32; 12]) -> Option<(String, f64)> {
    let total: f32 = chroma.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let mean = total as f64 / 12.0;
    let mut best: Option<(String, f64)> = None;
    for (root, name) in PITCH_CLASSES.iter().enumerate() {
        for (suffix, third) in [("", 4usize), ("m", 3usize)] {
            let tones = [root, (root + third) % 12, (root + 7) % 12];
            // Pearson correlation against a 0/1 template.
            let tmean = 3.0 / 12.0;
            let (mut num, mut dx, mut dy) = (0.0, 0.0, 0.0);
            for (pc, v) in chroma.iter().enumerate() {
                let t = if tones.contains(&pc) { 1.0 } else { 0.0 };
                let x = *v as f64 - mean;
                let y = t - tmean;
                num += x * y;
                dx += x * x;
                dy += y * y;
            }
            if dx <= 0.0 {
                continue;
            }
            let r = num / (dx.sqrt() * dy.sqrt());
            if best.as_ref().is_none_or(|(_, b)| r > *b) {
                best = Some((format!("{name}{suffix}"), r));
            }
        }
    }
    best.filter(|(_, r)| *r >= 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, amp: f64, rate: u32, seconds: f64, phase: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64 + phase).sin())
                    as f32
            })
            .collect()
    }

    fn stereo(s: Vec<f32>, rate: u32) -> Audio {
        Audio {
            sample_rate: rate,
            channels: vec![s.clone(), s],
        }
    }

    /// BS.1770's own check: a 1 kHz sine at −20 dBFS in both channels reads
    /// −20 LUFS (the K-weighting is close to 0 dB at 1 kHz).
    #[test]
    fn a_minus_20_dbfs_tone_reads_minus_20_lufs() {
        for rate in [44_100u32, 48_000, 96_000] {
            let a = stereo(sine(997.0, 0.1, rate, 3.0, 0.0), rate);
            let l = lufs(&a);
            // A sine's RMS is its amplitude over √2: −23 dB RMS, −20 dBFS
            // peak, which BS.1770 calls −20 LKFS with its −0.691 offset.
            assert!((l - -20.0).abs() < 0.15, "{rate} Hz: {l}");
        }
    }

    #[test]
    fn one_channel_reads_three_db_less_than_two() {
        let rate = 48_000;
        let s = sine(997.0, 1.0, rate, 2.0, 0.0);
        let left_only = Audio {
            sample_rate: rate,
            channels: vec![s.clone(), vec![0.0; s.len()]],
        };
        let l = lufs(&left_only);
        assert!((l - -3.01).abs() < 0.15, "{l}");
    }

    #[test]
    fn silence_is_the_floor() {
        let a = stereo(vec![0.0; 48_000], 48_000);
        assert_eq!(lufs(&a), LUFS_FLOOR);
        assert_eq!(true_peak(&a), FLOOR_DB);
        let s = Spectrum::new(48_000.0);
        let (bands, c) = s.frame(&[0.0; FFT_SIZE]);
        assert!(bands.iter().all(|b| *b == FLOOR_DB as f32));
        assert_eq!(c, [0.0; 12]);
        assert_eq!(chord_of(&c), None);
    }

    #[test]
    fn the_streaming_meter_agrees_with_the_whole_buffer() {
        let rate = 48_000;
        let s = sine(440.0, 0.3, rate, 3.0, 0.0);
        let mut m = Loudness::new(rate as f64);
        for chunk in s.chunks(997) {
            m.push(chunk, chunk);
        }
        let whole = lufs(&stereo(s.clone(), rate));
        assert!(
            (m.short_term() - whole).abs() < 0.2,
            "{} vs {whole}",
            m.short_term()
        );
        assert!((m.momentary() - whole).abs() < 0.2);
    }

    /// A sine at a quarter of the sample rate, 45° off the samples: every
    /// sample sits at 0.707 of the peak, so the sample peak reads −3 dB and
    /// the true peak 0.
    #[test]
    fn true_peak_finds_the_peak_between_samples() {
        let rate = 48_000;
        let s = sine(12_000.0, 1.0, rate, 0.1, std::f64::consts::FRAC_PI_4);
        let a = stereo(s, rate);
        let m = crate::audio::measure(&a, 2.0);
        assert!((m.peak_dbfs - -3.0).abs() < 0.1, "{}", m.peak_dbfs);
        let tp = true_peak(&a);
        assert!(tp > -0.5 && tp < 0.3, "true peak {tp}");
    }

    #[test]
    fn the_six_ranges_follow_the_tone() {
        let rate = 48_000;
        for (hz, want) in [
            (40.0, 0),
            (100.0, 1),
            (300.0, 2),
            (1000.0, 3),
            (3000.0, 4),
            (9000.0, 5),
        ] {
            let a = stereo(sine(hz, 0.5, rate, 1.0, 0.0), rate);
            let shares = range_shares_db(&range_energy(&a));
            let loudest = (0..6)
                .max_by(|x, y| shares[*x].partial_cmp(&shares[*y]).unwrap())
                .unwrap();
            assert_eq!(loudest, want, "{hz} Hz: {shares:?}");
            assert!(
                shares[want] > -0.5,
                "{hz} Hz holds nearly all of it: {shares:?}"
            );
        }
    }

    #[test]
    fn a_tone_reads_its_level_in_its_band() {
        let rate = 48_000.0;
        let s = sine(1000.0, 0.5, 48_000, 0.2, 0.0);
        let (bands, c) = Spectrum::new(rate).frame(&s);
        let band = ((1000.0f64 / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize;
        assert!((bands[band] + 6.02).abs() < 0.6, "{}", bands[band]);
        // 1 kHz is a B (MIDI 83.2).
        assert_eq!(c[11], 1.0, "{c:?}");
    }

    #[test]
    fn a_triad_reads_as_its_chord() {
        let rate = 48_000u32;
        let mix = |freqs: &[f64]| -> Vec<f32> {
            let mut out = vec![0.0f32; FFT_SIZE];
            for f in freqs {
                for (o, v) in out.iter_mut().zip(sine(*f, 0.2, rate, 0.1, 0.0)) {
                    *o += v;
                }
            }
            out
        };
        let s = Spectrum::new(rate as f64);
        // A minor: A3 C4 E4.
        let (_, c) = s.frame(&mix(&[220.0, 261.63, 329.63]));
        assert_eq!(chord_of(&c).map(|x| x.0), Some("Am".to_string()), "{c:?}");
        // C major: C4 E4 G4.
        let (_, c) = s.frame(&mix(&[261.63, 329.63, 392.0]));
        assert_eq!(chord_of(&c).map(|x| x.0), Some("C".to_string()), "{c:?}");
    }
}
