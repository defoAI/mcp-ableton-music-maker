//! Reading the audio Live recorded for a capture, and measuring it.
//!
//! Live records captures as PCM WAV or AIFF into the project's own folder.
//! This module reads those files (nothing else) and turns them into the
//! numbers the feedback loop needs: peak, RMS per bar, silence, clipping and
//! stereo correlation. It never writes, copies or sends audio.

use std::path::Path;

/// Decoded audio: one `Vec<f32>` per channel, samples in −1.0…1.0.
#[derive(Debug, Clone)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    pub fn frames(&self) -> usize {
        self.channels.first().map(Vec::len).unwrap_or(0)
    }

    pub fn duration_s(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }
}

/// Read a WAV or AIFF file by extension, then by magic bytes.
pub fn read_file(path: &Path) -> Result<Audio, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 12 {
        return Err(format!("{}: too short to be an audio file", path.display()));
    }
    let audio = match (&bytes[0..4], &bytes[8..12]) {
        (b"RIFF", b"WAVE") => read_wav(&bytes)?,
        (b"FORM", b"AIFF") | (b"FORM", b"AIFC") => read_aiff(&bytes)?,
        _ => {
            return Err(format!(
                "{}: not a WAV or AIFF file (Live records those two; check Preferences › Record › File Type)",
                path.display()
            ))
        }
    };
    if audio.duration_s() < 0.1 {
        return Err(format!(
            "{}: empty capture ({} frames)",
            path.display(),
            audio.frames()
        ));
    }
    Ok(audio)
}

fn read_wav(bytes: &[u8]) -> Result<Audio, String> {
    let mut reader =
        hound::WavReader::new(std::io::Cursor::new(bytes)).map_err(|e| format!("WAV: {e}"))?;
    let spec = reader.spec();
    let n = spec.channels as usize;
    if n == 0 {
        return Err("WAV: no channels".into());
    }
    let mut channels: Vec<Vec<f32>> = vec![Vec::new(); n];
    let mut i = 0usize;
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for s in reader.samples::<f32>() {
                let s = s.map_err(|e| format!("WAV: {e}"))?;
                channels[i % n].push(s);
                i += 1;
            }
        }
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            for s in reader.samples::<i32>() {
                let s = s.map_err(|e| format!("WAV: {e}"))?;
                channels[i % n].push(s as f32 / scale);
                i += 1;
            }
        }
    }
    Ok(Audio {
        sample_rate: spec.sample_rate,
        channels,
    })
}

/// AIFF and AIFF-C (PCM only): COMM for the format, SSND for the samples.
fn read_aiff(bytes: &[u8]) -> Result<Audio, String> {
    let be16 = |b: &[u8]| i16::from_be_bytes([b[0], b[1]]);
    let be32u = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let mut pos = 12usize;
    let mut channels_n = 0usize;
    let mut bits = 0u16;
    let mut rate = 0u32;
    let mut pcm = true;
    let mut ssnd: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = be32u(&bytes[pos + 4..pos + 8]) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        let body = &bytes[body_start..body_end];
        match id {
            b"COMM" => {
                if body.len() < 18 {
                    return Err("AIFF: COMM chunk too short".into());
                }
                channels_n = be16(&body[0..2]) as usize;
                bits = be16(&body[6..8]) as u16;
                rate = extended_to_u32(&body[8..18]);
                if body.len() >= 22 {
                    let compression = &body[18..22];
                    pcm = matches!(compression, b"NONE" | b"sowt" | b"twos");
                    if compression == b"sowt" {
                        // little-endian PCM in an AIFF-C container
                        let ssnd_le = true;
                        return read_aiff_samples(bytes, channels_n, bits, rate, ssnd_le);
                    }
                }
            }
            b"SSND" => {
                if body.len() < 8 {
                    return Err("AIFF: SSND chunk too short".into());
                }
                let offset = be32u(&body[0..4]) as usize;
                ssnd = Some(&body[8 + offset..]);
            }
            _ => {}
        }
        pos = body_end + (size & 1);
    }
    if !pcm {
        return Err("AIFF: compressed AIFF-C is not supported".into());
    }
    let data = ssnd.ok_or("AIFF: no SSND chunk")?;
    decode_pcm(data, channels_n, bits, rate, false)
}

fn read_aiff_samples(
    bytes: &[u8],
    n: usize,
    bits: u16,
    rate: u32,
    le: bool,
) -> Result<Audio, String> {
    let be32u = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = be32u(&bytes[pos + 4..pos + 8]) as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        if id == b"SSND" && body.len() >= 8 {
            let offset = be32u(&body[0..4]) as usize;
            return decode_pcm(&body[8 + offset..], n, bits, rate, le);
        }
        pos += 8 + size + (size & 1);
    }
    Err("AIFF: no SSND chunk".into())
}

fn decode_pcm(data: &[u8], n: usize, bits: u16, rate: u32, le: bool) -> Result<Audio, String> {
    if n == 0 || rate == 0 {
        return Err("AIFF: missing channel count or sample rate".into());
    }
    let width = match bits {
        8 | 16 | 24 | 32 => (bits / 8) as usize,
        other => return Err(format!("AIFF: {other}-bit samples are not supported")),
    };
    let scale = (1u64 << (bits - 1)) as f32;
    let mut channels: Vec<Vec<f32>> = vec![Vec::new(); n];
    let frames = data.len() / (width * n);
    for f in 0..frames {
        for (c, channel) in channels.iter_mut().enumerate() {
            let at = (f * n + c) * width;
            let b = &data[at..at + width];
            let v: i32 = match (width, le) {
                (1, _) => b[0] as i8 as i32,
                (2, false) => i16::from_be_bytes([b[0], b[1]]) as i32,
                (2, true) => i16::from_le_bytes([b[0], b[1]]) as i32,
                (3, false) => ((b[0] as i32) << 24 | (b[1] as i32) << 16 | (b[2] as i32) << 8) >> 8,
                (3, true) => ((b[2] as i32) << 24 | (b[1] as i32) << 16 | (b[0] as i32) << 8) >> 8,
                (4, false) => i32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                (4, true) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                _ => unreachable!(),
            };
            channel.push(v as f32 / scale);
        }
    }
    Ok(Audio {
        sample_rate: rate,
        channels,
    })
}

/// The 80-bit IEEE 754 extended float AIFF uses for the sample rate.
/// How long a sample is, from its header alone: at most 4 KB is read and no
/// audio is decoded, so a whole sample folder can be measured in the time one
/// file would take to load. None for a format whose header this does not parse
/// (MP3, M4A, Ogg, and FLAC) — a missing length is better than a wrong one.
pub fn header_seconds(path: &Path) -> Option<f64> {
    let mut head = [0u8; 4096];
    let read = {
        use std::io::Read;
        let mut file = std::fs::File::open(path).ok()?;
        file.read(&mut head).ok()?
    };
    let head = &head[..read];
    if head.len() < 16 {
        return None;
    }
    match (&head[0..4], &head[8..12]) {
        (b"RIFF", b"WAVE") => wav_header_seconds(head),
        (b"FORM", b"AIFF") | (b"FORM", b"AIFC") => aiff_header_seconds(head),
        _ => None,
    }
}

/// `data` bytes over the byte rate the `fmt ` chunk declares.
fn wav_header_seconds(head: &[u8]) -> Option<f64> {
    let le16 = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]) as u64;
    let le32 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64;
    let (mut pos, mut rate, mut channels, mut bits) = (12usize, 0u64, 0u64, 0u64);
    while pos + 8 <= head.len() {
        let size = le32(&head[pos + 4..pos + 8]) as usize;
        let body = &head[pos + 8..(pos + 8 + size).min(head.len())];
        match &head[pos..pos + 4] {
            b"fmt " if body.len() >= 16 => {
                channels = le16(&body[2..4]);
                rate = le32(&body[4..8]);
                bits = le16(&body[14..16]);
            }
            b"data" => {
                let per_second = rate * channels * (bits / 8);
                return (per_second > 0).then(|| size as f64 / per_second as f64);
            }
            _ => {}
        }
        pos += 8 + size + (size & 1);
    }
    None
}

/// Frames over the sample rate the COMM chunk declares.
fn aiff_header_seconds(head: &[u8]) -> Option<f64> {
    let be32 = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let mut pos = 12usize;
    while pos + 8 <= head.len() {
        let size = be32(&head[pos + 4..pos + 8]) as usize;
        let body = &head[pos + 8..(pos + 8 + size).min(head.len())];
        if &head[pos..pos + 4] == b"COMM" && body.len() >= 18 {
            let frames = be32(&body[2..6]) as f64;
            let rate = extended_to_u32(&body[8..18]);
            return (rate > 0).then(|| frames / rate as f64);
        }
        pos += 8 + size + (size & 1);
    }
    None
}

fn extended_to_u32(b: &[u8]) -> u32 {
    let exponent = ((((b[0] & 0x7f) as u16) << 8) | b[1] as u16) as i32;
    let mantissa = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if exponent == 0 && mantissa == 0 {
        return 0;
    }
    let shift = 63 - (exponent - 16383);
    if !(0..64).contains(&shift) {
        return 0;
    }
    (mantissa >> shift) as u32
}

/// What a capture sounds like, in numbers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Measurements {
    pub duration_s: f64,
    pub sample_rate: u32,
    pub channels: usize,
    pub peak_dbfs: f64,
    pub rms_dbfs: f64,
    /// RMS per bar in dBFS; the last entry may be a partial bar.
    pub rms_per_bar: Vec<f64>,
    /// 1-based bars whose RMS is below −60 dBFS.
    pub silent_bars: Vec<usize>,
    pub clipped_samples: usize,
    /// −1…1 for stereo; `None` for mono.
    pub stereo_correlation: Option<f64>,
    /// Peak minus RMS: how much dynamic range is left in the take.
    pub crest_db: f64,
    /// Energy share per octave band, 31 Hz … 16 kHz, each 0–1, summing to 1.
    pub octaves: Vec<f64>,
    /// Energy below 200 Hz against energy above 4 kHz, in dB.
    pub lf_hf_db: f64,
    /// Seconds of silence at the head — a capture that starts before the
    /// playhead arrives is not a mix observation.
    pub leading_silence_s: f64,
}

/// The centre frequency of each octave band the reading reports.
pub const OCTAVE_CENTERS: [f64; 10] = [
    31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// A short label per band: "31", "63", "125", … "16k".
pub fn octave_labels() -> Vec<String> {
    OCTAVE_CENTERS
        .iter()
        .map(|hz| {
            if *hz >= 1000.0 {
                format!("{}k", (hz / 1000.0).round() as i64)
            } else {
                format!("{}", hz.round() as i64)
            }
        })
        .collect()
}

fn dbfs(x: f64) -> f64 {
    if x <= 0.0 {
        -120.0
    } else {
        (20.0 * x.log10()).max(-120.0)
    }
}

/// Measure decoded audio; `seconds_per_bar` comes from the set's tempo and
/// time signature (`60 / tempo × beats per bar`).
pub fn measure(audio: &Audio, seconds_per_bar: f64) -> Measurements {
    let frames = audio.frames();
    let n = audio.channels.len();
    let mut peak = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut clipped = 0usize;
    for ch in &audio.channels {
        for &s in ch {
            let a = (s as f64).abs();
            if a > peak {
                peak = a;
            }
            if a >= 0.999 {
                clipped += 1;
            }
            sum_sq += (s as f64) * (s as f64);
        }
    }
    let total = (frames * n).max(1) as f64;
    let rms = (sum_sq / total).sqrt();

    let bar_frames = ((seconds_per_bar.max(0.01)) * audio.sample_rate as f64).round() as usize;
    let bar_frames = bar_frames.max(1);
    let mut rms_per_bar = Vec::new();
    let mut silent = Vec::new();
    let mut start = 0usize;
    while start < frames {
        let end = (start + bar_frames).min(frames);
        let mut sq = 0.0f64;
        let mut count = 0usize;
        for ch in &audio.channels {
            for &s in &ch[start..end] {
                sq += (s as f64) * (s as f64);
                count += 1;
            }
        }
        let bar_rms = dbfs((sq / count.max(1) as f64).sqrt());
        if bar_rms < -60.0 {
            silent.push(rms_per_bar.len() + 1);
        }
        rms_per_bar.push((bar_rms * 10.0).round() / 10.0);
        start = end;
    }

    // Silence at the head: the first sample above −60 dBFS, in seconds.
    let mut leading = frames;
    'lead: for i in 0..frames {
        for ch in &audio.channels {
            if (ch[i] as f64).abs() > 0.001 {
                leading = i;
                break 'lead;
            }
        }
    }
    let leading_silence_s = leading as f64 / audio.sample_rate.max(1) as f64;

    let stereo_correlation = if n >= 2 && frames > 1 {
        let (l, r) = (&audio.channels[0], &audio.channels[1]);
        let ml = l.iter().map(|&x| x as f64).sum::<f64>() / frames as f64;
        let mr = r.iter().map(|&x| x as f64).sum::<f64>() / frames as f64;
        let (mut num, mut dl, mut dr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..frames {
            let a = l[i] as f64 - ml;
            let b = r[i] as f64 - mr;
            num += a * b;
            dl += a * a;
            dr += b * b;
        }
        if dl > 0.0 && dr > 0.0 {
            Some(((num / (dl.sqrt() * dr.sqrt())) * 100.0).round() / 100.0)
        } else {
            Some(0.0)
        }
    } else {
        None
    };

    let peak_dbfs = (dbfs(peak) * 10.0).round() / 10.0;
    let rms_dbfs = (dbfs(rms) * 10.0).round() / 10.0;
    let energy = octave_energy(audio);
    let sum: f64 = energy.iter().sum();
    let octaves: Vec<f64> = if sum > 0.0 {
        energy.iter().map(|e| e / sum).collect()
    } else {
        vec![0.0; OCTAVE_CENTERS.len()]
    };
    // Below 200 Hz against above 4 kHz: the ratio a producer hears as "the
    // low end is masking everything", in dB.
    let low: f64 = energy[0] + energy[1] + energy[2];
    let high: f64 = energy[7] + energy[8] + energy[9];
    let lf_hf_db = if low <= 0.0 && high <= 0.0 {
        0.0
    } else {
        let ratio = (low.max(1e-12)) / (high.max(1e-12));
        ((10.0 * ratio.log10()).clamp(-60.0, 60.0) * 10.0).round() / 10.0
    };
    Measurements {
        duration_s: (audio.duration_s() * 100.0).round() / 100.0,
        sample_rate: audio.sample_rate,
        channels: n,
        peak_dbfs,
        rms_dbfs,
        rms_per_bar,
        silent_bars: silent,
        clipped_samples: clipped,
        stereo_correlation,
        crest_db: ((peak_dbfs - rms_dbfs) * 10.0).round() / 10.0,
        octaves,
        lf_hf_db,
        leading_silence_s: (leading_silence_s * 1000.0).round() / 1000.0,
    }
}

/// One short sentence a producer can act on, from the numbers.
pub fn reading(m: &Measurements) -> String {
    // Nothing to read: say that, rather than describing the spectrum and the
    // crest factor of silence.
    if m.peak_dbfs <= -60.0 {
        return "silent from end to end — nothing was playing".to_string();
    }
    let mut notes = Vec::new();
    // Spectral balance first: peak and RMS never caught a mix whose energy is
    // all under 120 Hz, and that is the question a producer is asking.
    let labels = octave_labels();
    if let Some((i, share)) = m
        .octaves
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, s)| (i, *s))
    {
        let below_250: f64 = m.octaves.iter().take(3).sum();
        if below_250 >= 0.7 {
            notes.push(format!(
                "{:.0} % of the energy is below 250 Hz — the low end is masking everything above it",
                below_250 * 100.0
            ));
        } else if share >= 0.45 {
            notes.push(format!(
                "{:.0} % of the energy sits in one octave around {} Hz",
                share * 100.0,
                labels[i]
            ));
        } else if m.lf_hf_db >= 24.0 {
            notes.push(format!(
                "low end is {:.0} dB above the top end — bass-heavy",
                m.lf_hf_db
            ));
        } else if m.lf_hf_db <= -6.0 {
            notes.push(format!(
                "top end is {:.0} dB above the low end — thin",
                -m.lf_hf_db
            ));
        } else {
            notes.push("spectral balance is even".into());
        }
    }
    if m.crest_db < 6.0 {
        notes.push(format!(
            "crest {:.1} dB — squashed, little dynamic range left",
            m.crest_db
        ));
    } else if m.crest_db > 20.0 {
        notes.push(format!("crest {:.1} dB — very peaky", m.crest_db));
    }
    if m.leading_silence_s > 0.05 {
        notes.push(format!(
            "the take begins with {:.2} s of silence",
            m.leading_silence_s
        ));
    }
    let bars = &m.rms_per_bar;
    if m.leading_silence_s > 0.05 {
        // A take that starts early says nothing about the music; do not
        // compare its halves.
    } else if bars.len() >= 2 {
        let half = bars.len() / 2;
        let mean = |s: &[f64]| s.iter().sum::<f64>() / s.len() as f64;
        let diff = mean(&bars[half..]) - mean(&bars[..half]);
        if diff.abs() >= 1.0 {
            notes.push(format!(
                "bars {}–{} are {:.1} dB {} than bars 1–{}",
                half + 1,
                bars.len(),
                diff.abs(),
                if diff > 0.0 { "louder" } else { "quieter" },
                half
            ));
        } else {
            notes.push("level is even across the bars".into());
        }
    }
    if m.clipped_samples > 0 {
        notes.push(format!(
            "{} clipped samples — lower the master or the loudest track",
            m.clipped_samples
        ));
    } else if m.peak_dbfs > -1.0 {
        notes.push("peak within 1 dB of full scale".into());
    } else {
        notes.push("no clipping".into());
    }
    if !m.silent_bars.is_empty() {
        let list: Vec<String> = m.silent_bars.iter().map(|b| b.to_string()).collect();
        notes.push(format!("silent bar(s): {}", list.join(", ")));
    } else {
        notes.push("no silent bars".into());
    }
    if let Some(c) = m.stereo_correlation {
        if c > 0.98 {
            notes.push("effectively mono".into());
        } else if c < 0.0 {
            notes.push("sides are out of phase — check stereo wideners".into());
        }
    }
    notes.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sine(freq: f64, amp: f64, seconds: f64, rate: u32) -> Vec<f32> {
        (0..(seconds * rate as f64) as usize)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()) as f32
            })
            .collect()
    }

    fn write_wav(path: &Path, channels: &[Vec<f32>], rate: u32) {
        let spec = hound::WavSpec {
            channels: channels.len() as u16,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..channels[0].len() {
            for ch in channels {
                w.write_sample((ch[i] * 32767.0) as i16).unwrap();
            }
        }
        w.finalize().unwrap();
    }

    /// A minimal big-endian 16-bit AIFF writer for the reader test.
    fn write_aiff(path: &Path, channels: &[Vec<f32>], rate: u32) {
        let n = channels.len() as u16;
        let frames = channels[0].len() as u32;
        let mut ssnd = Vec::new();
        ssnd.extend_from_slice(&0u32.to_be_bytes());
        ssnd.extend_from_slice(&0u32.to_be_bytes());
        for i in 0..frames as usize {
            for ch in channels {
                ssnd.extend_from_slice(&((ch[i] * 32767.0) as i16).to_be_bytes());
            }
        }
        // 80-bit extended: 44100 = 0x400E AC44 0000 0000 0000
        let mut ext = [0u8; 10];
        let exp = 16383 + 15; // 44100 < 2^16
        ext[0] = (exp >> 8) as u8;
        ext[1] = exp as u8;
        let mant = (rate as u64) << (63 - 15);
        ext[2..10].copy_from_slice(&mant.to_be_bytes());
        let mut comm = Vec::new();
        comm.extend_from_slice(&n.to_be_bytes());
        comm.extend_from_slice(&frames.to_be_bytes());
        comm.extend_from_slice(&16u16.to_be_bytes());
        comm.extend_from_slice(&ext);
        let mut body = Vec::new();
        body.extend_from_slice(b"AIFF");
        body.extend_from_slice(b"COMM");
        body.extend_from_slice(&(comm.len() as u32).to_be_bytes());
        body.extend_from_slice(&comm);
        body.extend_from_slice(b"SSND");
        body.extend_from_slice(&(ssnd.len() as u32).to_be_bytes());
        body.extend_from_slice(&ssnd);
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"FORM").unwrap();
        f.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
        f.write_all(&body).unwrap();
    }

    #[test]
    fn wav_and_aiff_decode_to_the_same_audio() {
        let dir = tempfile::tempdir().unwrap();
        let l = sine(440.0, 0.5, 1.0, 44100);
        let r = sine(440.0, 0.25, 1.0, 44100);
        let wav = dir.path().join("a.wav");
        let aiff = dir.path().join("a.aif");
        write_wav(&wav, &[l.clone(), r.clone()], 44100);
        write_aiff(&aiff, &[l, r], 44100);
        let a = read_file(&wav).unwrap();
        let b = read_file(&aiff).unwrap();
        assert_eq!(a.sample_rate, 44100);
        assert_eq!(b.sample_rate, 44100);
        assert_eq!(a.frames(), 44100);
        assert_eq!(b.frames(), 44100);
        assert!((a.channels[0][100] - b.channels[0][100]).abs() < 1e-4);
        assert!((a.channels[1][100] - b.channels[1][100]).abs() < 1e-4);
    }

    #[test]
    fn measurements_match_known_signals() {
        // Two bars of a −20 dBFS sine, then two bars of a full-scale square; 1 s bars.
        let rate = 8000;
        let mut l = sine(100.0, 0.1, 2.0, rate);
        l.extend((0..2 * rate as usize).map(|i| if i % 80 < 40 { 1.0f32 } else { -1.0 }));
        let r = l.clone();
        let audio = Audio {
            sample_rate: rate,
            channels: vec![l, r],
        };
        let m = measure(&audio, 1.0);
        assert_eq!(m.duration_s, 4.0);
        assert_eq!(m.rms_per_bar.len(), 4);
        assert!(
            (m.rms_per_bar[0] - (-23.0)).abs() < 0.3,
            "{:?}",
            m.rms_per_bar
        ); // 0.1 amp sine ≈ −23 dBFS RMS
        assert!((m.rms_per_bar[3] - 0.0).abs() < 0.2);
        assert!(m.silent_bars.is_empty());
        assert_eq!(m.peak_dbfs, 0.0);
        assert!(m.clipped_samples > 0);
        assert_eq!(m.stereo_correlation, Some(1.0));
        let text = reading(&m);
        assert!(
            text.contains("bars 3–4 are 23.0 dB louder than bars 1–2")
                && text.contains("clipped")
                && text.contains("mono"),
            "{text}"
        );

        // A bar of sine then a bar of nothing: the silent bar is named.
        let mut l = sine(100.0, 0.5, 1.0, rate);
        l.extend(vec![0.0f32; rate as usize]);
        let quiet = Audio {
            sample_rate: rate,
            channels: vec![l],
        };
        let m = measure(&quiet, 1.0);
        assert_eq!(m.silent_bars, vec![2]);
        assert_eq!(m.stereo_correlation, None);
        assert!(reading(&m).contains("silent bar(s): 2"));
    }

    #[test]
    fn left_only_signal_reads_as_uncorrelated_and_short_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let l = sine(200.0, 0.5, 0.5, 8000);
        let r = vec![0.0f32; l.len()];
        let audio = Audio {
            sample_rate: 8000,
            channels: vec![l, r],
        };
        let m = measure(&audio, 0.25);
        assert_eq!(m.stereo_correlation, Some(0.0));
        let short = dir.path().join("short.wav");
        write_wav(&short, &[vec![0.1f32; 100]], 44100);
        assert!(read_file(&short).unwrap_err().contains("empty capture"));
        let junk = dir.path().join("junk.wav");
        std::fs::write(&junk, b"not audio at all, really").unwrap();
        assert!(read_file(&junk).unwrap_err().contains("not a WAV or AIFF"));
    }

    #[test]
    fn the_spectrum_and_the_crest_are_measured_not_guessed() {
        let rate = 44100;
        // A 60 Hz tone: the energy belongs in the 63 Hz octave, and peak and
        // RMS alone would call this mix "even".
        let low = Audio {
            sample_rate: rate,
            channels: vec![sine(60.0, 0.5, 1.0, rate)],
        };
        let m = measure(&low, 1.0);
        let loudest = m
            .octaves
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(octave_labels()[loudest], "63", "{:?}", m.octaves);
        assert!(m.lf_hf_db > 30.0, "all the energy is at the bottom: {m:?}");
        assert!(reading(&m).contains("below 250 Hz"), "{}", reading(&m));
        // A sine's crest factor is 3 dB; a square wave's is 0.
        assert!((m.crest_db - 3.0).abs() < 0.5, "{}", m.crest_db);
        let square: Vec<f32> = (0..rate as usize)
            .map(|i| if (i / 100) % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let flat = Audio {
            sample_rate: rate,
            channels: vec![square],
        };
        let m = measure(&flat, 1.0);
        assert!(m.crest_db < 1.0, "{}", m.crest_db);
        assert!(reading(&m).contains("squashed"), "{}", reading(&m));

        // A 10 kHz tone lands at the top and reads as thin.
        let high = Audio {
            sample_rate: rate,
            channels: vec![sine(10000.0, 0.5, 1.0, rate)],
        };
        let m = measure(&high, 1.0);
        assert!(m.lf_hf_db < -30.0, "{:?}", m.octaves);
    }

    #[test]
    fn silence_at_the_head_is_measured_and_never_narrated_as_music() {
        let rate = 8000;
        let mut ch = vec![0.0f32; rate as usize]; // one second of nothing
        ch.extend(sine(200.0, 0.5, 1.0, rate));
        let audio = Audio {
            sample_rate: rate,
            channels: vec![ch],
        };
        let m = measure(&audio, 1.0);
        assert!(
            (m.leading_silence_s - 1.0).abs() < 0.01,
            "{}",
            m.leading_silence_s
        );
        let text = reading(&m);
        assert!(text.contains("begins with 1.00 s of silence"), "{text}");
        assert!(
            !text.contains("louder than bars"),
            "a take that starts early says nothing about the music: {text}"
        );
    }
}

/// Energy share of three bands over the whole recording: low (< 200 Hz),
/// mid, high (> 4 kHz), each 0–1 and summing to 1. A plain radix-2 FFT on
/// 4096-sample windows of the mono mix; no dependency.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Bands {
    pub low: f64,
    pub mid: f64,
    pub high: f64,
}

/// Energy per octave band, 31 Hz … 16 kHz, from one plain radix-2 FFT pass
/// over 4096-sample windows of the mono mix. Not normalized — `measure`
/// turns it into shares, and `band_balance` sums it into three.
pub fn octave_energy(audio: &Audio) -> [f64; OCTAVE_CENTERS.len()] {
    const N: usize = 4096;
    let mut bands = [0.0f64; OCTAVE_CENTERS.len()];
    let frames = audio.frames();
    let chans = audio.channels.len().max(1) as f32;
    let bin_hz = audio.sample_rate as f64 / N as f64;
    let mut re = vec![0.0f64; N];
    let mut im = vec![0.0f64; N];
    let root2 = std::f64::consts::SQRT_2;
    let mut start = 0;
    while start + N <= frames.max(N) && start < frames {
        for i in 0..N {
            let mut sample = 0.0f32;
            for ch in &audio.channels {
                sample += ch.get(start + i).copied().unwrap_or(0.0);
            }
            // Hann window
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / N as f64).cos();
            re[i] = (sample / chans) as f64 * w;
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for k in 1..N / 2 {
            let e = re[k] * re[k] + im[k] * im[k];
            let hz = k as f64 * bin_hz;
            for (b, centre) in OCTAVE_CENTERS.iter().enumerate() {
                let lo = centre / root2;
                let hi = centre * root2;
                // the lowest band keeps everything under it, the highest
                // everything over it, so no energy is dropped
                let below = b == 0 && hz < lo;
                let above = b == OCTAVE_CENTERS.len() - 1 && hz >= hi;
                if (hz >= lo && hz < hi) || below || above {
                    bands[b] += e;
                    break;
                }
            }
        }
        start += N;
    }
    bands
}

/// Energy share of three bands: low (< 200 Hz), mid, high (> 4 kHz).
pub fn band_balance(audio: &Audio) -> Bands {
    let e = octave_energy(audio);
    let low: f64 = e[0] + e[1] + e[2];
    let high: f64 = e[7] + e[8] + e[9];
    let mid: f64 = e[3] + e[4] + e[5] + e[6];
    let total = low + mid + high;
    if total <= 0.0 {
        return Bands {
            low: 0.0,
            mid: 0.0,
            high: 0.0,
        };
    }
    Bands {
        low: low / total,
        mid: mid / total,
        high: high / total,
    }
}

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
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
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (vr, vi) = (
                    re[i + k + len / 2] * cr - im[i + k + len / 2] * ci,
                    re[i + k + len / 2] * ci + im[i + k + len / 2] * cr,
                );
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod band_tests {
    use super::*;

    fn tone(hz: f64) -> Audio {
        let rate = 44100u32;
        let samples: Vec<f32> = (0..rate as usize)
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin()) as f32
            })
            .collect();
        Audio {
            sample_rate: rate,
            channels: vec![samples],
        }
    }

    #[test]
    fn bands_follow_the_tone() {
        let b = band_balance(&tone(60.0));
        assert!(b.low > 0.9, "{b:?}");
        let b = band_balance(&tone(1000.0));
        assert!(b.mid > 0.9, "{b:?}");
        let b = band_balance(&tone(8000.0));
        assert!(b.high > 0.9, "{b:?}");
        let silent = Audio {
            sample_rate: 44100,
            channels: vec![vec![0.0; 44100]],
        };
        assert_eq!(
            band_balance(&silent),
            Bands {
                low: 0.0,
                mid: 0.0,
                high: 0.0
            }
        );
    }
}
