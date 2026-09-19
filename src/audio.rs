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

    Measurements {
        duration_s: (audio.duration_s() * 100.0).round() / 100.0,
        sample_rate: audio.sample_rate,
        channels: n,
        peak_dbfs: (dbfs(peak) * 10.0).round() / 10.0,
        rms_dbfs: (dbfs(rms) * 10.0).round() / 10.0,
        rms_per_bar,
        silent_bars: silent,
        clipped_samples: clipped,
        stereo_correlation,
    }
}

/// One short sentence a producer can act on, from the numbers.
pub fn reading(m: &Measurements) -> String {
    let mut notes = Vec::new();
    let bars = &m.rms_per_bar;
    if bars.len() >= 2 {
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
}
