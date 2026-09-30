//! What the numbers on the Listen screen mean, as pure functions over
//! samples. No audio is kept beyond the analysis window, and nothing here
//! touches the file system, the network or Core Audio — so it is all
//! testable without Live, and the tests below are the specification.
//!
//! Units are the artist's, the same ones the server's readouts use: dBFS
//! with 0 dB at the top and −80 dB as the floor (`song::meter_db`).

/// Bands drawn across the spectrum, log-spaced from 20 Hz to 20 kHz.
pub const BANDS: usize = 72;
/// Analysis window. 4096 points is 85 ms at 48 kHz and puts a whole Hann
/// main lobe inside one band from about 300 Hz up, so a tone reads its own
/// level rather than a leakage-dependent one.
pub const FFT_SIZE: usize = 4096;
pub const FMIN: f64 = 20.0;
pub const FMAX: f64 = 20_000.0;
/// The floor every dB value is clamped to, as in `song::meter_db`.
pub const FLOOR_DB: f32 = -80.0;
/// Below this the screen calls it silence.
pub const SILENCE_DB: f32 = -70.0;
/// How long a band's peak line waits before it drops to the current value.
const HOLD_MS: f64 = 2000.0;
/// How fast the master peak-hold line falls once nothing beats it.
const PEAK_FALL_DB_PER_S: f32 = 12.0;
/// How long the clip light stays on after a sample reaches full scale.
const CLIP_MS: f64 = 2000.0;

/// The six ranges people name when they talk about a mix.
pub const RANGES: [(&str, f64, f64); 6] = [
    ("sub", 20.0, 60.0),
    ("bass", 60.0, 200.0),
    ("low mids", 200.0, 500.0),
    ("mids", 500.0, 2000.0),
    ("presence", 2000.0, 6000.0),
    ("air", 6000.0, 20000.0),
];

/// One frame of measurements, as the window receives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub bands: Vec<f32>,
    pub hold: Vec<f32>,
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub peak_hold: f32,
    pub correlation: f32,
    pub clip: bool,
    pub ranges: [f32; 6],
    pub silent: bool,
}

/// dB from a linear amplitude, floored like every other level in the product.
pub fn db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return FLOOR_DB;
    }
    (20.0 * amplitude.log10()).max(FLOOR_DB)
}

/// The lower edge of band `i`, in Hz. Bands are geometric, so band `BANDS`
/// is exactly `FMAX`.
pub fn band_edge(i: usize) -> f64 {
    FMIN * (FMAX / FMIN).powf(i as f64 / BANDS as f64)
}

// ── the transform ───────────────────────────────────────────────────────────

/// In-place radix-2 FFT. `re` and `im` must be the same power-of-two length,
/// and `tw` the twiddle table from [`twiddles`] for that length.
fn fft(re: &mut [f32], im: &mut [f32], tw: &[(f32, f32)]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());
    debug_assert_eq!(im.len(), n);
    debug_assert_eq!(tw.len(), n / 2);

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2usize;
    while len <= n {
        let half = len / 2;
        let step = n / len;
        let mut base = 0usize;
        while base < n {
            for k in 0..half {
                let (wr, wi) = tw[k * step];
                let a = base + k;
                let b = a + half;
                let vr = re[b] * wr - im[b] * wi;
                let vi = re[b] * wi + im[b] * wr;
                re[b] = re[a] - vr;
                im[b] = im[a] - vi;
                re[a] += vr;
                im[a] += vi;
            }
            base += len;
        }
        len <<= 1;
    }
}

/// e^(-2πi k / n) for k in 0..n/2, computed in f64 so a 4096-point transform
/// does not accumulate the error a f32 recurrence would.
fn twiddles(n: usize) -> Vec<(f32, f32)> {
    (0..n / 2)
        .map(|k| {
            let a = -2.0 * std::f64::consts::PI * k as f64 / n as f64;
            (a.cos() as f32, a.sin() as f32)
        })
        .collect()
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (0.5 - 0.5 * x.cos()) as f32
        })
        .collect()
}

// ── the analyser ────────────────────────────────────────────────────────────

pub struct Analyzer {
    sample_rate: f64,
    hann: Vec<f32>,
    tw: Vec<(f32, f32)>,
    /// The band each bin belongs to, or `usize::MAX` for bins outside
    /// 20 Hz–20 kHz. Bins per band, resolved once from the sample rate.
    band_of_bin: Vec<usize>,
    /// For bands narrower than one bin: the bin to read, and how much of a
    /// bin the band is worth.
    narrow: Vec<Option<(usize, f32)>>,
    /// Rolling window, newest sample last.
    win_l: Vec<f32>,
    win_r: Vec<f32>,
    pos: usize,
    /// Peak of everything pushed since the last frame.
    peak_l: f32,
    peak_r: f32,
    hold: Vec<f32>,
    hold_at: Vec<f64>,
    peak_hold: f32,
    clip_until: f64,
    last_frame_at: f64,
    silent_since: Option<f64>,
}

impl Analyzer {
    pub fn new(sample_rate: f64) -> Self {
        let rate = if sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        let bin_hz = rate / FFT_SIZE as f64;
        let bins = FFT_SIZE / 2;

        let mut band_of_bin = vec![usize::MAX; bins];
        for (bin, slot) in band_of_bin.iter_mut().enumerate().skip(1) {
            let f = bin as f64 * bin_hz;
            if f < FMIN || f >= FMAX {
                continue;
            }
            // Geometric spacing inverts in closed form.
            let idx = ((f / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize;
            if idx < BANDS {
                *slot = idx;
            }
        }
        // A band can be narrower than one bin at the bottom of the range;
        // then it reads the nearest bin, scaled to the band's width, so the
        // curve stays a power density instead of jumping.
        let has_bin = {
            let mut has = vec![false; BANDS];
            for &b in &band_of_bin {
                if b != usize::MAX {
                    has[b] = true;
                }
            }
            has
        };
        let narrow = (0..BANDS)
            .map(|i| {
                if has_bin[i] {
                    return None;
                }
                let lo = band_edge(i);
                let hi = band_edge(i + 1);
                let centre = (lo * hi).sqrt();
                let bin = (centre / bin_hz).round() as usize;
                let bin = bin.clamp(1, bins - 1);
                Some((bin, ((hi - lo) / bin_hz) as f32))
            })
            .collect();

        Self {
            sample_rate: rate,
            hann: hann(FFT_SIZE),
            tw: twiddles(FFT_SIZE),
            band_of_bin,
            narrow,
            win_l: vec![0.0; FFT_SIZE],
            win_r: vec![0.0; FFT_SIZE],
            pos: 0,
            peak_l: 0.0,
            peak_r: 0.0,
            hold: vec![FLOOR_DB; BANDS],
            hold_at: vec![0.0; BANDS],
            peak_hold: FLOOR_DB,
            clip_until: f64::NEG_INFINITY,
            last_frame_at: 0.0,
            silent_since: None,
        }
    }

    /// How long the analyser has seen nothing but silence, in ms, or None if
    /// it is hearing something.
    pub fn silent_for(&self, now_ms: f64) -> Option<f64> {
        self.silent_since.map(|t| now_ms - t)
    }

    /// Add samples. `l` and `r` must be the same length.
    pub fn push(&mut self, l: &[f32], r: &[f32], now_ms: f64) {
        let n = l.len().min(r.len());
        for i in 0..n {
            let (a, b) = (l[i], r[i]);
            self.win_l[self.pos] = a;
            self.win_r[self.pos] = b;
            self.pos = (self.pos + 1) % FFT_SIZE;
            let (aa, ab) = (a.abs(), b.abs());
            if aa > self.peak_l {
                self.peak_l = aa;
            }
            if ab > self.peak_r {
                self.peak_r = ab;
            }
            if aa >= 0.999 || ab >= 0.999 {
                self.clip_until = now_ms + CLIP_MS;
            }
        }
    }

    /// The window in time order, oldest first.
    fn ordered(&self, src: &[f32], out: &mut [f32]) {
        let (head, tail) = src.split_at(self.pos);
        out[..tail.len()].copy_from_slice(tail);
        out[tail.len()..].copy_from_slice(head);
    }

    /// The newest audio as `points` mono samples in −1…1, oldest first: two
    /// window samples per point, averaged. What a waveform display draws.
    pub fn wave(&self, points: usize) -> Vec<f32> {
        let points = points.clamp(1, FFT_SIZE / 2);
        let span = points * 2;
        let mut out = Vec::with_capacity(points);
        for i in 0..points {
            // The window is a ring ending at `pos`; walk back `span` samples.
            let base = (self.pos + FFT_SIZE - span + i * 2) % FFT_SIZE;
            let next = (base + 1) % FFT_SIZE;
            let a = 0.5 * (self.win_l[base] + self.win_r[base]);
            let b = 0.5 * (self.win_l[next] + self.win_r[next]);
            out.push((0.5 * (a + b)).clamp(-1.0, 1.0));
        }
        out
    }

    /// Measure everything pushed so far and reset the per-frame peaks.
    pub fn frame(&mut self, now_ms: f64) -> Frame {
        let mut l = vec![0.0f32; FFT_SIZE];
        let mut r = vec![0.0f32; FFT_SIZE];
        self.ordered(&self.win_l, &mut l);
        self.ordered(&self.win_r, &mut r);

        // Levels over the window.
        let (mut sl, mut sr, mut slr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..FFT_SIZE {
            sl += (l[i] as f64) * (l[i] as f64);
            sr += (r[i] as f64) * (r[i] as f64);
            slr += (l[i] as f64) * (r[i] as f64);
        }
        let rms_l = (sl / FFT_SIZE as f64).sqrt() as f32;
        let rms_r = (sr / FFT_SIZE as f64).sqrt() as f32;
        let correlation = if sl > 0.0 && sr > 0.0 {
            (slr / (sl.sqrt() * sr.sqrt())).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };

        // The spectrum, on the mono sum so one curve describes the mix.
        let mut re = vec![0.0f32; FFT_SIZE];
        let mut im = vec![0.0f32; FFT_SIZE];
        for i in 0..FFT_SIZE {
            re[i] = 0.5 * (l[i] + r[i]) * self.hann[i];
        }
        fft(&mut re, &mut im, &self.tw);

        let bins = FFT_SIZE / 2;
        let mut power = vec![0.0f64; bins];
        for (k, p) in power.iter_mut().enumerate().skip(1) {
            *p = (re[k] as f64) * (re[k] as f64) + (im[k] as f64) * (im[k] as f64);
        }

        // Σ|X|² over a whole Hann lobe is 3·A²·N²/32 for a sine of amplitude
        // A, so this constant makes a full-scale tone read 0 dBFS wherever it
        // sits between bins.
        let norm = 20.0 * ((32.0f64 / 3.0).sqrt() / FFT_SIZE as f64).log10();

        let mut band_power = vec![0.0f64; BANDS];
        for (k, &b) in self.band_of_bin.iter().enumerate() {
            if b != usize::MAX {
                band_power[b] += power[k];
            }
        }
        for (i, n) in self.narrow.iter().enumerate() {
            if let Some((bin, width)) = *n {
                band_power[i] = power[bin] * width.max(0.0) as f64;
            }
        }
        let bands: Vec<f32> = band_power
            .iter()
            .map(|p| {
                if *p <= 0.0 {
                    FLOOR_DB
                } else {
                    ((10.0 * p.log10() + norm) as f32).max(FLOOR_DB)
                }
            })
            .collect();

        // Each range as its share of the whole, so the numbers say where the
        // mix sits rather than how loud it is.
        let bin_hz = self.sample_rate / FFT_SIZE as f64;
        let total: f64 = power.iter().sum();
        let mut ranges = [FLOOR_DB; 6];
        for (i, (_, lo, hi)) in RANGES.iter().enumerate() {
            let mut sum = 0.0;
            for (k, p) in power.iter().enumerate().skip(1) {
                let f = k as f64 * bin_hz;
                if f >= *lo && f < *hi {
                    sum += p;
                }
            }
            ranges[i] = if total > 0.0 && sum > 0.0 {
                ((10.0 * (sum / total).log10()) as f32).max(FLOOR_DB)
            } else {
                FLOOR_DB
            };
        }

        // Peak hold per band: keep the highest for two seconds, then follow.
        for ((hold, at), &now) in self.hold.iter_mut().zip(self.hold_at.iter_mut()).zip(&bands) {
            if now >= *hold || now_ms - *at > HOLD_MS {
                *hold = now;
                *at = now_ms;
            }
        }

        let peak = [db(self.peak_l), db(self.peak_r)];
        let loudest = peak[0].max(peak[1]);
        if loudest >= self.peak_hold {
            self.peak_hold = loudest;
        } else {
            let dt = ((now_ms - self.last_frame_at).max(0.0) / 1000.0) as f32;
            self.peak_hold = (self.peak_hold - PEAK_FALL_DB_PER_S * dt).max(loudest);
        }
        self.last_frame_at = now_ms;

        let silent = loudest < SILENCE_DB;
        if silent {
            self.silent_since.get_or_insert(now_ms);
        } else {
            self.silent_since = None;
        }

        let frame = Frame {
            bands,
            hold: self.hold.clone(),
            peak,
            rms: [db(rms_l), db(rms_r)],
            peak_hold: self.peak_hold,
            correlation,
            clip: now_ms < self.clip_until,
            ranges,
            silent,
        };
        self.peak_l = 0.0;
        self.peak_r = 0.0;
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, amp: f32, rate: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp as f64 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin()) as f32)
            .collect()
    }

    /// Feed enough samples to fill the window, then measure.
    fn measure(l: &[f32], r: &[f32], rate: f64) -> Frame {
        let mut a = Analyzer::new(rate);
        a.push(l, r, 0.0);
        a.frame(100.0)
    }

    #[test]
    fn fft_matches_a_naive_dft() {
        let n = 64;
        let tw = twiddles(n);
        let input: Vec<f32> = (0..n).map(|i| ((i * 7 % 13) as f32 - 6.0) / 6.0).collect();
        let mut re = input.clone();
        let mut im = vec![0.0f32; n];
        fft(&mut re, &mut im, &tw);
        for k in 0..n {
            let (mut dr, mut di) = (0.0f64, 0.0f64);
            for (t, x) in input.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * t) as f64 / n as f64;
                dr += *x as f64 * a.cos();
                di += *x as f64 * a.sin();
            }
            assert!((re[k] as f64 - dr).abs() < 1e-3, "bin {k}: {} vs {dr}", re[k]);
            assert!((im[k] as f64 - di).abs() < 1e-3, "bin {k}: {} vs {di}", im[k]);
        }
    }

    #[test]
    fn bands_cover_the_range_with_no_gap() {
        assert!((band_edge(0) - FMIN).abs() < 1e-9);
        assert!((band_edge(BANDS) - FMAX).abs() < 1e-6);
        for i in 0..BANDS {
            assert!(band_edge(i) < band_edge(i + 1), "band {i} is empty");
        }
        // Every bin inside the range lands in exactly one band.
        let a = Analyzer::new(48_000.0);
        let bin_hz = 48_000.0 / FFT_SIZE as f64;
        for (k, &b) in a.band_of_bin.iter().enumerate().skip(1) {
            let f = k as f64 * bin_hz;
            if (FMIN..FMAX).contains(&f) {
                assert!(b < BANDS, "bin {k} at {f} Hz has no band");
                assert!((band_edge(b)..band_edge(b + 1)).contains(&f), "bin {k} in the wrong band");
            } else {
                assert_eq!(b, usize::MAX, "bin {k} at {f} Hz should be outside");
            }
        }
    }

    #[test]
    fn a_tone_reads_its_own_level_in_its_own_band() {
        for (freq, amp, want) in [(1000.0, 0.5f32, -6.02f32), (1000.0, 1.0, 0.0), (440.0, 0.25, -12.04)] {
            let s = sine(freq, amp, 48_000.0, FFT_SIZE);
            let f = measure(&s, &s, 48_000.0);
            let band = ((freq / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize;
            assert!(
                (f.bands[band] - want).abs() < 0.5,
                "{freq} Hz at {want} dBFS read {} in band {band}",
                f.bands[band]
            );
            // and it is the loudest band
            let loudest = f
                .bands
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap()
                .0;
            assert_eq!(loudest, band, "{freq} Hz peaked in band {loudest}");
        }
    }

    #[test]
    fn a_tone_between_bins_still_reads_its_level() {
        // Deliberately off-bin: 4096 points at 48 kHz gives 11.72 Hz bins.
        let freq = 1000.0 + 11.71875 / 2.0;
        let s = sine(freq, 0.5, 48_000.0, FFT_SIZE);
        let f = measure(&s, &s, 48_000.0);
        let band = ((freq / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize;
        assert!((f.bands[band] + 6.02).abs() < 0.5, "read {}", f.bands[band]);
    }

    #[test]
    fn peak_and_rms_are_the_artists_numbers() {
        let s = sine(1000.0, 0.5, 48_000.0, FFT_SIZE);
        let f = measure(&s, &s, 48_000.0);
        assert!((f.peak[0] + 6.02).abs() < 0.1, "peak {}", f.peak[0]);
        // RMS of a sine is its amplitude over root two: −9 dBFS for 0.5.
        assert!((f.rms[0] + 9.03).abs() < 0.1, "rms {}", f.rms[0]);
        assert!(!f.clip);
    }

    #[test]
    fn silence_reads_the_floor_everywhere() {
        let q = vec![0.0f32; FFT_SIZE];
        let f = measure(&q, &q, 48_000.0);
        assert!(f.silent);
        assert_eq!(f.peak, [FLOOR_DB, FLOOR_DB]);
        assert_eq!(f.rms, [FLOOR_DB, FLOOR_DB]);
        assert_eq!(f.correlation, 0.0);
        for (i, b) in f.bands.iter().enumerate() {
            assert_eq!(*b, FLOOR_DB, "band {i}");
        }
        for r in f.ranges {
            assert_eq!(r, FLOOR_DB);
        }
    }

    #[test]
    fn correlation_reads_the_width() {
        let s = sine(300.0, 0.5, 48_000.0, FFT_SIZE);
        let inverted: Vec<f32> = s.iter().map(|v| -v).collect();
        assert!((measure(&s, &s, 48_000.0).correlation - 1.0).abs() < 1e-3);
        assert!((measure(&s, &inverted, 48_000.0).correlation + 1.0).abs() < 1e-3);
        // Two different tones are close to uncorrelated.
        let other = sine(1100.0, 0.5, 48_000.0, FFT_SIZE);
        assert!(measure(&s, &other, 48_000.0).correlation.abs() < 0.2);
    }

    #[test]
    fn full_scale_lights_the_clip_light_and_holds_it() {
        let mut a = Analyzer::new(48_000.0);
        let mut s = sine(1000.0, 0.9, 48_000.0, FFT_SIZE);
        s[10] = 1.0;
        a.push(&s, &s, 0.0);
        assert!(a.frame(0.0).clip);
        assert!(a.frame(1999.0).clip, "the light holds for two seconds");
        assert!(!a.frame(2001.0).clip, "then it goes out");
    }

    #[test]
    fn the_six_ranges_are_shares_of_the_whole() {
        let mut l = sine(50.0, 0.4, 48_000.0, FFT_SIZE);
        for (i, v) in sine(3000.0, 0.4, 48_000.0, FFT_SIZE).iter().enumerate() {
            l[i] += v;
        }
        let f = measure(&l, &l, 48_000.0);
        let total: f32 = f.ranges.iter().map(|r| 10f32.powf(r / 10.0)).sum();
        assert!(
            (10.0 * total.log10()).abs() < 0.2,
            "the ranges should account for the whole spectrum, got {} dB",
            10.0 * total.log10()
        );
        // Energy at 50 Hz and 3 kHz, so sub and presence lead.
        assert!(f.ranges[0] > f.ranges[2], "sub {} vs low mids {}", f.ranges[0], f.ranges[2]);
        assert!(f.ranges[4] > f.ranges[5], "presence {} vs air {}", f.ranges[4], f.ranges[5]);
    }

    #[test]
    fn band_hold_waits_two_seconds_then_follows_down() {
        let mut a = Analyzer::new(48_000.0);
        let loud = sine(1000.0, 0.8, 48_000.0, FFT_SIZE);
        a.push(&loud, &loud, 0.0);
        let first = a.frame(0.0);
        let band = ((1000.0f64 / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize;
        let quiet = sine(1000.0, 0.05, 48_000.0, FFT_SIZE);
        a.push(&quiet, &quiet, 100.0);
        let soon = a.frame(100.0);
        assert!(soon.bands[band] < first.bands[band] - 10.0, "the band fell");
        assert!((soon.hold[band] - first.bands[band]).abs() < 0.5, "the hold line stayed up");
        let later = a.frame(2200.0);
        assert!((later.hold[band] - later.bands[band]).abs() < 0.5, "then it followed down");
    }

    #[test]
    fn peak_hold_falls_at_twelve_db_a_second() {
        let mut a = Analyzer::new(48_000.0);
        let loud = sine(1000.0, 1.0, 48_000.0, FFT_SIZE);
        a.push(&loud, &loud, 0.0);
        let f = a.frame(0.0);
        assert!((f.peak_hold - 0.0).abs() < 0.1);
        let quiet = vec![0.0f32; 512];
        a.push(&quiet, &quiet, 500.0);
        let f = a.frame(1000.0);
        assert!((f.peak_hold + 12.0).abs() < 0.5, "peak hold {}", f.peak_hold);
    }

    #[test]
    fn silence_is_timed_so_the_screen_can_explain_it() {
        let mut a = Analyzer::new(48_000.0);
        let q = vec![0.0f32; 1024];
        a.push(&q, &q, 0.0);
        a.frame(0.0);
        assert_eq!(a.silent_for(3000.0), Some(3000.0));
        let loud = sine(1000.0, 0.5, 48_000.0, 1024);
        a.push(&loud, &loud, 3100.0);
        a.frame(3100.0);
        assert_eq!(a.silent_for(3200.0), None);
    }

    #[test]
    fn mono_and_odd_sample_rates_still_work() {
        for rate in [44_100.0, 48_000.0, 88_200.0, 96_000.0] {
            let s = sine(1000.0, 0.5, rate, FFT_SIZE);
            let f = measure(&s, &s, rate);
            let band = ((1000.0f64 / FMIN).ln() / (FMAX / FMIN).ln() * BANDS as f64).floor() as usize;
            assert!(
                (f.bands[band] + 6.02).abs() < 0.6,
                "at {rate} Hz the tone read {}",
                f.bands[band]
            );
        }
    }

    #[test]
    fn the_wave_is_the_newest_audio_in_order() {
        let mut a = Analyzer::new(48_000.0);
        // A ramp: every sample is its own index, so order is visible.
        let ramp: Vec<f32> = (0..512).map(|i| i as f32 / 512.0).collect();
        a.push(&ramp, &ramp, 0.0);
        let w = a.wave(256);
        assert_eq!(w.len(), 256);
        assert!(w.windows(2).all(|p| p[1] > p[0]), "oldest first, rising");
        assert!((w[0] - 0.5 / 512.0).abs() < 1e-3, "starts at the first ramp sample: {}", w[0]);
        assert!((w[255] - 510.5 / 512.0).abs() < 1e-3, "ends at the newest: {}", w[255]);
        // More audio pushes the ramp out of the newest window.
        a.push(&[0.0; 600], &[0.0; 600], 1.0);
        assert!(a.wave(256).iter().all(|v| *v == 0.0));
        // Never outside the range a display expects.
        a.push(&[3.0; 8], &[-3.0; 8], 2.0);
        assert!(a.wave(4).iter().all(|v| (-1.0..=1.0).contains(v)));
    }

    #[test]
    fn a_short_push_does_not_panic_or_read_stale_peaks() {
        let mut a = Analyzer::new(48_000.0);
        a.push(&[0.5, -0.5], &[0.5, -0.5], 0.0);
        let f = a.frame(0.0);
        assert!((f.peak[0] + 6.02).abs() < 0.1);
        // The per-frame peak resets, so the next frame reports the new audio.
        let f2 = a.frame(33.0);
        assert_eq!(f2.peak[0], FLOOR_DB);
    }
}
