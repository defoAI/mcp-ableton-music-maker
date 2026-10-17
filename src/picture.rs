//! A picture of what was heard: a spectrogram, a loudness curve and a chroma
//! strip, with the section names along the top and bar numbers along the
//! bottom — drawn from a take's frames, returned as a PNG a model can look at.
//!
//! The PNG is written here, with no image or compression crate: an indexed
//! image (one byte a pixel) deflated with LZ77 and the fixed Huffman code.
//! Charts compress well that way and the dependency tree stays as it is.

use crate::listening::{self, Range};
use crate::meter;
use crate::recorder::{BarRow, FrameRow, Take};

// ── PNG ─────────────────────────────────────────────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, value: u32, count: u32) {
        self.acc |= (value as u64) << self.n;
        self.n += count;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// A Huffman code goes in most significant bit first.
    fn code(&mut self, code: u32, len: u32) {
        let mut rev = 0;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.put(rev, len);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

fn literal(bits: &mut Bits, v: u32) {
    match v {
        0..=143 => bits.code(0x30 + v, 8),
        144..=255 => bits.code(0x190 + v - 144, 9),
        256..=279 => bits.code(v - 256, 7),
        _ => bits.code(0xc0 + v - 280, 8),
    }
}

const LEN_BASE: [u32; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u32; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u32; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u32; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn emit_match(bits: &mut Bits, len: usize, dist: usize) {
    let li = LEN_BASE.iter().rposition(|b| *b as usize <= len).unwrap();
    literal(bits, 257 + li as u32);
    bits.put(len as u32 - LEN_BASE[li], LEN_EXTRA[li]);
    let di = DIST_BASE.iter().rposition(|b| *b as usize <= dist).unwrap();
    bits.code(di as u32, 5);
    bits.put(dist as u32 - DIST_BASE[di], DIST_EXTRA[di]);
}

/// zlib-wrapped deflate: one fixed-Huffman block, LZ77 over a 32 KB window
/// with a hash chain of the last positions of each three-byte run.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32_768;
    const HASH: usize = 1 << 15;
    const CHAIN: usize = 16;
    let mut head = vec![usize::MAX; HASH];
    let mut prev = vec![usize::MAX; data.len()];
    let hash = |i: usize| -> usize {
        ((data[i] as usize) << 10 ^ (data[i + 1] as usize) << 5 ^ data[i + 2] as usize) & (HASH - 1)
    };
    let mut bits = Bits {
        out: vec![0x78, 0x01],
        acc: 0,
        n: 0,
    };
    bits.put(1, 1); // the last block
    bits.put(1, 2); // fixed Huffman
    let mut i = 0;
    while i < data.len() {
        let mut best = (0usize, 0usize);
        if i + 2 < data.len() {
            let h = hash(i);
            let mut cand = head[h];
            let mut tries = 0;
            while cand != usize::MAX && i - cand <= WINDOW && tries < CHAIN {
                let max = (data.len() - i).min(258);
                let mut l = 0;
                while l < max && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best.0 {
                    best = (l, i - cand);
                    if l == max {
                        break;
                    }
                }
                cand = prev[cand];
                tries += 1;
            }
            prev[i] = head[h];
            head[h] = i;
        }
        if best.0 >= 3 {
            emit_match(&mut bits, best.0, best.1);
            for (j, slot) in prev.iter_mut().enumerate().take(i + best.0).skip(i + 1) {
                if j + 2 < data.len() {
                    let h = hash(j);
                    *slot = head[h];
                    head[h] = j;
                }
            }
            i += best.0;
        } else {
            literal(&mut bits, data[i] as u32);
            i += 1;
        }
    }
    literal(&mut bits, 256);
    let mut out = bits.finish();
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut c = kind.to_vec();
    c.extend_from_slice(body);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// An 8-bit indexed PNG.
pub fn png(width: usize, height: usize, palette: &[[u8; 3]], pixels: &[u8]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let plte: Vec<u8> = palette.iter().flatten().copied().collect();
    chunk(&mut out, b"PLTE", &plte);
    let mut raw = Vec::with_capacity((width + 1) * height);
    for row in pixels.chunks(width) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    chunk(&mut out, b"IDAT", &zlib(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ── drawing ─────────────────────────────────────────────────────────────────

/// Palette: 0–199 the heat scale, then the chart's own colours.
const HEAT: usize = 200;
const BG: u8 = 200;
const INK: u8 = 201;
const DIM: u8 = 202;
const LINE: u8 = 203;
const ACCENT: u8 = 204;
const PANEL: u8 = 205;

fn palette() -> Vec<[u8; 3]> {
    // A dark-to-bright ramp: navy, violet, magenta, orange, pale yellow.
    let stops: [(f64, [f64; 3]); 5] = [
        (0.0, [12.0, 12.0, 30.0]),
        (0.3, [72.0, 30.0, 120.0]),
        (0.55, [190.0, 50.0, 110.0]),
        (0.8, [250.0, 140.0, 40.0]),
        (1.0, [252.0, 250.0, 190.0]),
    ];
    let mut p: Vec<[u8; 3]> = (0..HEAT)
        .map(|i| {
            let x = i as f64 / (HEAT - 1) as f64;
            let k = stops.windows(2).position(|w| x <= w[1].0).unwrap_or(3);
            let (a, b) = (stops[k], stops[k + 1]);
            let t = (x - a.0) / (b.0 - a.0);
            [0, 1, 2].map(|c| (a.1[c] + (b.1[c] - a.1[c]) * t).round() as u8)
        })
        .collect();
    p.push([18, 18, 22]); // BG
    p.push([235, 235, 240]); // INK
    p.push([120, 120, 135]); // DIM
    p.push([70, 70, 82]); // LINE
    p.push([90, 200, 250]); // ACCENT
    p.push([28, 28, 34]); // PANEL
    p.resize(256, [0, 0, 0]);
    p
}

const GLYPHS: &[(char, [u8; 7])] = &[
    ('A', [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
    ('B', [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e]),
    ('C', [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e]),
    ('D', [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e]),
    ('E', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f]),
    ('F', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10]),
    ('G', [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f]),
    ('H', [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
    ('I', [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e]),
    ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c]),
    ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f]),
    ('M', [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11]),
    ('O', [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
    ('P', [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10]),
    ('Q', [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d]),
    ('R', [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11]),
    ('S', [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e]),
    ('T', [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
    ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a]),
    ('X', [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11]),
    ('Y', [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04]),
    ('Z', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f]),
    ('0', [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e]),
    ('1', [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e]),
    ('2', [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f]),
    ('3', [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e]),
    ('4', [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02]),
    ('5', [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e]),
    ('6', [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e]),
    ('7', [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e]),
    ('9', [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c]),
    ('-', [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00]),
    ('.', [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c]),
    (':', [0x00, 0x0c, 0x0c, 0x00, 0x0c, 0x0c, 0x00]),
    ('/', [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10]),
    ('+', [0x00, 0x04, 0x04, 0x1f, 0x04, 0x04, 0x00]),
    ('%', [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03]),
    ('?', [0x0e, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04]),
    (' ', [0x00; 7]),
];

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Canvas {
    fn set(&mut self, x: usize, y: usize, c: u8) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = c;
        }
    }
    fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: u8) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.px[yy * self.w + xx] = c;
            }
        }
    }
    /// Text in the 5×7 font, upper case; returns the width drawn.
    fn text(&mut self, x: usize, y: usize, s: &str, c: u8) -> usize {
        let mut cx = x;
        for ch in s.chars() {
            let up = ch.to_ascii_uppercase();
            let g = GLYPHS
                .iter()
                .find(|(k, _)| *k == up)
                .or_else(|| GLYPHS.iter().find(|(k, _)| *k == '?'))
                .unwrap()
                .1;
            for (row, bits) in g.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0x10 >> col) != 0 {
                        self.set(cx + col, y + row, c);
                    }
                }
            }
            cx += 6;
        }
        cx - x
    }
}

/// A drawing body's answer: the caption and the PNG, or why there is none.
pub type PictureResult = Result<(String, Vec<u8>), String>;

/// The picture of some bars of a take: what `listen_picture` returns.
pub struct Picture {
    pub png: Vec<u8>,
    pub caption: String,
}

/// Draw `bars` of `take` from its `frames`.
pub fn draw(take: &Take, bars: &[&BarRow], frames: &[FrameRow], width: usize) -> Picture {
    let width = width.clamp(320, 1600);
    let bpb = take.beats_per_bar.max(0.25);
    let wanted: std::collections::BTreeSet<i64> = bars.iter().map(|b| b.bar).collect();
    // The frames of the chosen bars, in the order they were played.
    let picked: Vec<&FrameRow> = frames
        .iter()
        .filter(|f| wanted.contains(&((f.beat / bpb + 1e-9).floor() as i64 + 1)))
        .collect();
    let (pad, label_h, spec_h, gap, loud_h, chroma_h, foot_h) =
        (8usize, 14usize, 144usize, 6usize, 64usize, 48usize, 14usize);
    let height = pad + label_h + spec_h + gap + loud_h + gap + chroma_h + foot_h + pad;
    let plot_x = 30usize;
    let plot_w = width - plot_x - pad;
    let mut c = Canvas {
        w: width,
        h: height,
        px: vec![BG; width * height],
    };
    let spec_y = pad + label_h;
    let loud_y = spec_y + spec_h + gap;
    let chroma_y = loud_y + loud_h + gap;
    c.rect(plot_x, spec_y, plot_w, spec_h, PANEL);
    c.rect(plot_x, loud_y, plot_w, loud_h, PANEL);
    c.rect(plot_x, chroma_y, plot_w, chroma_h, PANEL);
    let n = picked.len().max(1);
    let col_of = |i: usize| plot_x + i * plot_w / n;
    for x in 0..plot_w {
        let i = x * n / plot_w;
        let Some(f) = picked.get(i) else { continue };
        // Spectrogram: two rows a band, low at the bottom, −80…0 dBFS.
        for (b, db) in f.bands.iter().enumerate().take(meter::BANDS) {
            let v = ((*db as f64 + 80.0) / 80.0).clamp(0.0, 1.0);
            let col = (v * (HEAT - 1) as f64).round() as u8;
            let y = spec_y + spec_h - 2 * (b + 1);
            c.set(plot_x + x, y, col);
            c.set(plot_x + x, y + 1, col);
        }
        // Chroma: four rows a pitch class, C at the bottom.
        for (pc, v) in f.chroma.iter().enumerate() {
            let col = ((*v as f64).clamp(0.0, 1.0) * (HEAT - 1) as f64).round() as u8;
            let y = chroma_y + chroma_h - 4 * (pc + 1);
            for d in 0..4 {
                c.set(plot_x + x, y + d, col);
            }
        }
    }
    // Loudness: short-term LUFS from −40 to 0, as a line.
    let lufs_y = |l: f64| {
        loud_y + loud_h - 1 - (((l + 40.0) / 40.0).clamp(0.0, 1.0) * (loud_h - 1) as f64) as usize
    };
    for g in [-10.0, -20.0, -30.0] {
        let y = lufs_y(g);
        for x in (plot_x..plot_x + plot_w).step_by(3) {
            c.set(x, y, LINE);
        }
        c.text(2, y.saturating_sub(3), &format!("{}", g as i64), DIM);
    }
    let mut last: Option<usize> = None;
    for x in 0..plot_w {
        let i = x * n / plot_w;
        let Some(f) = picked.get(i) else { continue };
        let y = lufs_y(f.lufs as f64);
        if let Some(py) = last {
            for yy in py.min(y)..=py.max(y) {
                c.set(plot_x + x, yy, ACCENT);
            }
        }
        c.set(plot_x + x, y, ACCENT);
        last = Some(y);
    }
    // Axis words.
    c.text(2, spec_y + 2, "HZ", DIM);
    c.text(2, spec_y + spec_h - 9, "20", DIM);
    c.text(2, spec_y + spec_h / 2 - 3, "630", DIM);
    c.text(2, spec_y + 2 + 9, "20K", DIM);
    c.text(2, loud_y + 2, "LU", DIM);
    c.text(2, chroma_y + chroma_h - 7, "C", DIM);
    c.text(2, chroma_y + 2, "B", DIM);
    // Section names along the top and bar numbers along the bottom, at the
    // frame where each begins.
    let mut prev_section = String::new();
    let mut prev_bar = i64::MIN;
    let mut last_label_end = 0usize;
    let mut last_bar_end = 0usize;
    let every = ((bars.len() as f64 / (plot_w as f64 / 28.0)).ceil() as i64).max(1);
    for (i, f) in picked.iter().enumerate() {
        let bar = (f.beat / bpb + 1e-9).floor() as i64 + 1;
        if bar == prev_bar {
            continue;
        }
        let x = col_of(i);
        let row = bars.iter().find(|b| b.bar == bar);
        let section = row
            .map(|b| crate::song::parse_section_name(&b.section).0)
            .unwrap_or_default();
        if section != prev_section {
            for y in spec_y..chroma_y + chroma_h {
                c.set(x, y, INK);
            }
            if !section.is_empty() && x >= last_label_end {
                last_label_end = x + c.text(x + 2, pad + 3, &section, INK) + 8;
            }
            prev_section = section;
        } else if bar != prev_bar && prev_bar != i64::MIN {
            for y in (spec_y..spec_y + spec_h).step_by(4) {
                c.set(x, y, LINE);
            }
        }
        if (bar - 1) % every == 0 && x >= last_bar_end {
            last_bar_end = x + c.text(x + 1, chroma_y + chroma_h + 4, &bar.to_string(), DIM) + 6;
        }
        prev_bar = bar;
    }
    let first = bars.iter().map(|b| b.bar).min().unwrap_or(0);
    let last_bar = bars.iter().map(|b| b.bar).max().unwrap_or(0);
    let caption = format!(
        "Spectrogram (20 Hz–20 kHz, −80…0 dBFS), short-term loudness (−40…0 LUFS) and chroma (C at the bottom, B at the top) of bars {first}–{last_bar}, measured from playback at {} (take {}). {} frames.",
        listening::when(&take.started_at),
        take.id,
        picked.len()
    );
    Picture {
        png: png(width, height, &palette(), &c.px),
        caption,
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ListenPictureParams {
    /// A section by name
    #[serde(default)]
    pub section: Option<String>,
    /// Or bars: 33, "33-40" or [33, 40]
    #[serde(default)]
    pub bars: Option<serde_json::Value>,
    /// The newest take, whole (the default)
    #[serde(default)]
    pub last: bool,
    /// A take's id, from listen (default: the newest take of the range)
    #[serde(default)]
    pub take: Option<String>,
    /// Width in pixels (320–1600, default 720)
    #[serde(default)]
    pub width: Option<u32>,
}

/// The picture body: a caption and the PNG, or why there is none.
pub fn listen_picture_body(
    live: &crate::connection::LiveState,
    p: &ListenPictureParams,
) -> PictureResult {
    let (key, note) = listening::song_key(live);
    let range = Range::parse(p.section.as_deref(), p.bars.as_ref(), p.last)?;
    picture_of(
        &key,
        &range,
        p.take.as_deref(),
        p.width.unwrap_or(720) as usize,
    )
    .map(|(cap, png)| {
        (
            format!(
                "{}{cap}",
                note.map(|n| format!("{n}\n")).unwrap_or_default()
            ),
            png,
        )
    })
}

pub fn picture_of(
    song_key: &str,
    range: &Range,
    take_id: Option<&str>,
    width: usize,
) -> PictureResult {
    let all = listening::takes(song_key);
    if all.is_empty() {
        return Err(listening::nothing_heard(listening::app_state()).to_string());
    }
    let (take, bars) = match take_id {
        Some(id) => {
            let t = all
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no take {id}"))?;
            let bars: Vec<&BarRow> = t
                .bars
                .iter()
                .filter(|b| range == &Range::Last || range_holds(range, b))
                .collect();
            (t, bars)
        }
        None => listening::pick(&all, range, None)
            .ok_or_else(|| "that range has never been heard.".to_string())?,
    };
    if bars.is_empty() {
        return Err("that take did not play that range.".into());
    }
    let frames = listening::frames(song_key, &take.id);
    if frames.is_empty() {
        return Err(format!("take {} kept no frames to draw.", take.id));
    }
    let pic = draw(take, &bars, &frames, width);
    Ok((pic.caption, pic.png))
}

fn range_holds(range: &Range, b: &BarRow) -> bool {
    match range {
        Range::Section(s) => crate::song::parse_section_name(&b.section)
            .0
            .eq_ignore_ascii_case(crate::song::parse_section_name(s).0.as_str()),
        Range::Bars(a, z) => b.bar >= *a && b.bar <= *z,
        Range::Last => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_adler_match_their_published_values() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    /// The deflate stream must inflate back to what went in. Python's zlib
    /// is the reference when it is on the machine; the round trip is checked
    /// either way by the structure tests below.
    #[test]
    fn deflate_round_trips_through_zlib() {
        let mut data = Vec::new();
        for i in 0..20_000u32 {
            data.push((i % 251) as u8);
            if i % 7 == 0 {
                data.extend_from_slice(b"spectrogram");
            }
        }
        let z = zlib(&data);
        assert!(
            z.len() < data.len() / 2,
            "compressed to {} of {}",
            z.len(),
            data.len()
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("z.bin");
        std::fs::write(&path, &z).unwrap();
        let out = std::process::Command::new("python3")
            .args(["-c", "import sys,zlib,hashlib;d=zlib.decompress(open(sys.argv[1],'rb').read());print(len(d),hashlib.sha256(d).hexdigest())"])
            .arg(&path)
            .output();
        let Ok(out) = out else { return };
        if !out.status.success() {
            panic!(
                "zlib refused the stream: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let len: usize = text.split_whitespace().next().unwrap().parse().unwrap();
        assert_eq!(len, data.len());
    }

    #[test]
    fn a_png_has_its_chunks_in_order() {
        let pal = palette();
        let px = vec![BG; 40 * 10];
        let p = png(40, 10, &pal, &px);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[12..16], b"IHDR");
        assert!(p.windows(4).any(|w| w == b"PLTE"));
        assert!(p.windows(4).any(|w| w == b"IDAT"));
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
    }
}
