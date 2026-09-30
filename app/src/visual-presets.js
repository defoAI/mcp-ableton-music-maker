// The visual's presets, the random variation of one, and the blend between two.
//
// A preset is a page in the MilkDrop sense: a warp mode, a wave shape, a fold,
// a palette and a per-frame function of time and the audio. Nothing here
// draws; it returns numbers, so the transitions can be tested without a GPU
// (`node --test visual.test.mjs`).
//
// Two things matter and they pull against each other.
//
// Randomness: a preset is never shown twice the same way. Every visit mints a
// *variant* — the palette turned somewhere new, the warp and the fold and the
// swirl rolled again — so the wall never repeats and the next one is never the
// one you expect.
//
// Continuity: a change is never a cut. A variant is a fixed set of numbers for
// as long as it is on screen, so two of them can be interpolated; the warp
// modes are mixed inside the shader rather than swapped; the wave shapes
// dissolve over each other on the same feedback buffer. All the dice are
// thrown at the moment a transition *starts*, never during one.
(function () {
  const TAU = Math.PI * 2;

  /// Smootherstep: zero slope *and* zero curvature at both ends, so neither
  /// the start nor the end of a transition has an edge you could notice.
  function ease(x) {
    const t = Math.max(0, Math.min(1, x));
    return t * t * t * (t * (t * 6 - 15) + 10);
  }

  function hsv(h, s, v) {
    h = ((h % 1) + 1) % 1;
    const i = Math.floor(h * 6), f = h * 6 - i;
    const p = v * (1 - s), q = v * (1 - f * s), u = v * (1 - (1 - f) * s);
    switch (i % 6) {
      case 0: return [v, u, p];
      case 1: return [q, v, p];
      case 2: return [p, v, u];
      case 3: return [p, q, v];
      case 4: return [u, p, v];
      default: return [v, p, q];
    }
  }

  /// Turn a colour around the grey axis. Cheap, and it keeps brightness.
  function turnHue(c, a) {
    const k = 0.57735, cs = Math.cos(a), sn = Math.sin(a), d = k * (c[0] + c[1] + c[2]);
    return [
      Math.min(1, Math.max(0, c[0] * cs + (k * c[2] - k * c[1]) * sn + k * d * (1 - cs))),
      Math.min(1, Math.max(0, c[1] * cs + (k * c[0] - k * c[2]) * sn + k * d * (1 - cs))),
      Math.min(1, Math.max(0, c[2] * cs + (k * c[1] - k * c[0]) * sn + k * d * (1 - cs))),
    ];
  }

  const lerp = (a, b, k) => a + (b - a) * k;
  const lerp3 = (a, b, k) => [lerp(a[0], b[0], k), lerp(a[1], b[1], k), lerp(a[2], b[2], k)];
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

  /// A small deterministic generator, so a seeded run replays exactly and the
  /// tests are not a coin toss.
  function rng(seed) {
    let s = (seed >>> 0) || 0x9e3779b9;
    return function next() {
      s ^= s << 13; s >>>= 0;
      s ^= s >> 17;
      s ^= s << 5; s >>>= 0;
      return s / 4294967296;
    };
  }

  // Each preset's `frame` returns only what moves with the music; the warp
  // mode adds its own character in the shader. `zoom` is a multiplier around
  // 1 and every mode's contribution is added to it, never assigned, so two
  // modes can be mixed without one overriding the other.
  const PRESETS = [
    {
      name: 'Nebular Drift', mode: 0, wave: 'circle', kaleido: 0,
      decay: 0.975, warp: 0.5, warpSpeed: 0.9, hueRate: 0.12, scale: 0.28, thick: 0.007, gamma: 1.15,
      hue: 0.62, hue2: 0.85, sat: 0.85,
      frame: (t, a) => ({ zoom: 1.012 + 0.05 * a.bass, rot: 0.006 * Math.sin(t * 0.2) + 0.01 * a.mid, dx: 0, dy: 0, sx: 1, sy: 1, bright: 1.05 + 0.25 * a.beat }),
    },
    {
      name: 'Tunnel of Bass', mode: 4, wave: 'spectrum', kaleido: 0,
      decay: 0.982, warp: 0.25, warpSpeed: 0.5, hueRate: 0.07, scale: 0.35, thick: 0.006, gamma: 1.1,
      hue: 0.05, hue2: 0.12, sat: 0.95,
      frame: (t, a) => ({ zoom: 1, rot: 0.004 + 0.02 * a.treb, dx: 0, dy: 0, sx: 1, sy: 1, bright: 1.1 + 0.3 * a.beat }),
    },
    {
      name: 'Kaleido Bloom', mode: 3, wave: 'dual', kaleido: 6,
      decay: 0.968, warp: 0.9, warpSpeed: 1.4, hueRate: 0.2, scale: 0.32, thick: 0.008, gamma: 1.2,
      hue: 0.32, hue2: 0.55, sat: 0.85,
      frame: (t, a) => ({ zoom: 1.02 + 0.06 * a.bass, rot: 0.012 * Math.sin(t * 0.5), dx: 0, dy: 0, sx: 1, sy: 1, bright: 1 + 0.35 * a.beat }),
    },
    {
      name: 'Ink Waves', mode: 6, wave: 'line', kaleido: 0,
      decay: 0.955, warp: 1.4, warpSpeed: 0.7, hueRate: 0.05, scale: 0.5, thick: 0.012, gamma: 1.05,
      hue: 0.53, hue2: 0.58, sat: 0.6,
      frame: (t, a) => ({ zoom: 1, rot: 0, dx: 0.002 * Math.sin(t * 0.7), dy: -0.004 - 0.006 * a.bass, sx: 1, sy: 1, bright: 1.15 + 0.2 * a.beat }),
    },
    {
      name: 'Solar Flare', mode: 1, wave: 'circle', kaleido: 0,
      decay: 0.97, warp: 0.4, warpSpeed: 2.0, hueRate: 0.03, scale: 0.4, thick: 0.009, gamma: 1.25,
      hue: 0.08, hue2: 0.16, sat: 0.95,
      frame: (t, a) => ({ zoom: 1.03 + 0.08 * a.bass, rot: 0.02 + 0.03 * a.mid, dx: 0, dy: 0, sx: 1, sy: 1, bright: 1.05 + 0.4 * a.beat }),
    },
    {
      name: 'Neon Grid', mode: 7, wave: 'spiral', kaleido: 8,
      decay: 0.965, warp: 0.2, warpSpeed: 1, hueRate: 0.25, scale: 0.3, thick: 0.005, gamma: 1.15,
      hue: 0.9, hue2: 0.48, sat: 0.92,
      frame: (t, a) => ({ zoom: 1.015 + 0.04 * a.bass, rot: 0.008, dx: 0, dy: 0, sx: 1 + 0.03 * a.mid, sy: 1 - 0.03 * a.mid, bright: 1 + 0.3 * a.beat }),
    },
    {
      name: 'Deep Spiral', mode: 2, wave: 'dual', kaleido: 0,
      decay: 0.978, warp: 0.6, warpSpeed: 0.8, hueRate: 0.1, scale: 0.26, thick: 0.007, gamma: 1.1,
      hue: 0.7, hue2: 0.95, sat: 0.82,
      frame: (t, a) => ({ zoom: 1.008 + 0.05 * a.bass, rot: 0.01 + 0.02 * a.treb, dx: 0, dy: 0, sx: 1, sy: 1, bright: 1.05 + 0.3 * a.beat }),
    },
    {
      name: 'Breathing Sea', mode: 5, wave: 'line', kaleido: 0,
      decay: 0.985, warp: 0.35, warpSpeed: 0.4, hueRate: 0.04, scale: 0.34, thick: 0.01, gamma: 1.05,
      hue: 0.5, hue2: 0.62, sat: 0.8,
      frame: (t, a) => ({ zoom: 1.004 + 0.03 * a.bass, rot: 0.003 * Math.sin(t * 0.3), dx: 0, dy: 0.001 * Math.sin(t * 0.5), sx: 1, sy: 1, bright: 1.1 + 0.25 * a.beat }),
    },
  ];

  const WAVES = ['line', 'dual', 'circle', 'spiral', 'spectrum'];
  const FOLDS = [0, 0, 0, 3, 4, 5, 6, 8, 12];
  /// Warp modes 8 and 9 exist only for variants: a radial pull and a drifting
  /// figure of eight. Nothing in PRESETS asks for them by default.
  const MODES = 10;

  /// One appearance of a preset. Every number is fixed for as long as it is on
  /// screen, which is what lets two of them be interpolated.
  function makeVariant(index, rand) {
    const p = PRESETS[((index % PRESETS.length) + PRESETS.length) % PRESETS.length];
    const r = typeof rand === 'function' ? rand : Math.random;
    const jitter = (v, amount) => v * (1 + (r() * 2 - 1) * amount);
    const pick = (list) => list[Math.floor(r() * list.length) % list.length];

    // Most of the time a variant keeps the preset's own character; now and
    // then it takes someone else's warp or shape, which is where the real
    // surprises come from.
    const mode = r() < 0.62 ? p.mode : Math.floor(r() * MODES) % MODES;
    const wave = r() < 0.7 ? p.wave : pick(WAVES);
    const kaleido = r() < 0.5 ? p.kaleido : pick(FOLDS);
    const hueOffset = r() * TAU;
    const spin = r() < 0.5 ? -1 : 1;
    const sat = clamp(jitter(p.sat, 0.25), 0.25, 1);
    const colA = turnHue(hsv(p.hue, sat, 1), hueOffset);
    const colB = turnHue(hsv(p.hue2, sat, 1), hueOffset + (r() * 0.8 - 0.4));
    const zoomGain = jitter(1, 0.5);
    const rotGain = jitter(1, 0.6);

    return {
      index, name: p.name,
      mode, wave, kaleido,
      decay: clamp(jitter(p.decay, 0.012), 0.94, 0.992),
      warp: clamp(jitter(p.warp, 0.6), 0, 2.2),
      warpSpeed: clamp(jitter(p.warpSpeed, 0.7), 0.1, 3.5),
      hueRate: clamp(jitter(p.hueRate, 0.9) * (r() < 0.25 ? -1 : 1), -0.5, 0.5),
      scale: clamp(jitter(p.scale, 0.35), 0.12, 0.7),
      thick: clamp(jitter(p.thick, 0.5), 0.003, 0.02),
      gamma: clamp(jitter(p.gamma, 0.15), 0.85, 1.5),
      // The trippy extras, applied continuously on top of whatever mode is
      // running, so they blend like everything else.
      swirl: r() < 0.45 ? (r() * 0.5 + 0.05) * spin : 0,
      ripple: r() < 0.5 ? r() * 0.7 : 0,
      chroma: clamp(jitter(1.4, 0.8), 0.2, 3),
      colA, colB,
      frame(t, a) {
        const v = p.frame(t, a);
        return {
          zoom: 1 + (v.zoom - 1) * zoomGain,
          rot: v.rot * rotGain * spin,
          dx: v.dx, dy: v.dy, sx: v.sx, sy: v.sy, bright: v.bright,
        };
      },
    };
  }

  /// Every number the renderer needs for one frame, with variants `va` and
  /// `vb` mixed by `k` (0 = entirely `va`, 1 = entirely `vb`). The warp modes
  /// and the folds are not mixed here — they go to the shaders as a pair plus
  /// `blend`, so the mixing happens per pixel where it belongs.
  function blendState(va, vb, k, t, audio) {
    const e = ease(k);
    const fa = va.frame(t, audio), fb = vb.frame(t, audio);
    const num = (key) => lerp(fa[key], fb[key], e);
    const pre = (key) => lerp(va[key], vb[key], e);

    // The wave: one shape when both agree, otherwise the two dissolving
    // through each other. Their weights always sum to one, so the ink on
    // screen never dips or spikes at a change.
    const waves = [];
    if (va.wave === vb.wave) {
      waves.push({
        shape: va.wave, scale: pre('scale'), thick: pre('thick'), weight: 1,
        colA: lerp3(va.colA, vb.colA, e), colB: lerp3(va.colB, vb.colB, e),
      });
    } else {
      if (e < 1) waves.push({ shape: va.wave, scale: va.scale, thick: va.thick, weight: 1 - e, colA: va.colA, colB: va.colB });
      if (e > 0) waves.push({ shape: vb.wave, scale: vb.scale, thick: vb.thick, weight: e, colA: vb.colA, colB: vb.colB });
    }

    return {
      name: e < 0.5 ? va.name : vb.name,
      blend: e,
      modeA: va.mode, modeB: vb.mode,
      kaleidoA: va.kaleido, kaleidoB: vb.kaleido,
      zoom: num('zoom'), rot: num('rot'), dx: num('dx'), dy: num('dy'),
      sx: num('sx'), sy: num('sy'), bright: num('bright'),
      decay: pre('decay'), warp: pre('warp'), warpSpeed: pre('warpSpeed'),
      hueRate: pre('hueRate'), gamma: pre('gamma'),
      swirl: pre('swirl'), ripple: pre('ripple'), chroma: pre('chroma'),
      waves,
    };
  }

  /// Holds the two variants on screen and how far between them we are.
  ///
  /// The handover is the delicate part. `from` and `to` only ever change at
  /// the instant a transition completes, and there the two descriptions agree
  /// exactly — blending fully into `to` and starting fresh from `to` are the
  /// same picture — so the numbers the renderer sees never jump. A request
  /// that arrives mid-transition is therefore queued rather than applied, and
  /// the transition in flight simply hurries up.
  function makeBlender(opts) {
    const o = typeof opts === 'number' ? { seconds: opts } : (opts || {});
    const rand = typeof o.rand === 'function' ? o.rand : Math.random;
    const base = Math.max(0.001, o.seconds || 6);
    let from = makeVariant(0, rand), to = from, k = 1, queued = null, speed = 1, dur = base;

    /// Somewhere between half and one and a half times the nominal length, so
    /// even the pace of the changes is not a pattern.
    const rollDuration = () => base * (0.6 + rand() * 0.9);

    return {
      get from() { return from; },
      get to() { return to; },
      get k() { return k; },
      get blending() { return k < 1; },
      get index() { return to.index; },
      /// What the picture is heading for, queue included — this is what
      /// "next preset" should step from.
      get target() { return queued == null ? to.index : queued.index; },
      get duration() { return dur; },
      /// Move towards a fresh variant of preset `i`. Returns false if nothing
      /// had to change.
      go(i) {
        const n = PRESETS.length;
        const next = ((i % n) + n) % n;
        const v = makeVariant(next, rand);
        if (k < 1) {
          // Mid-transition: never snap. Queue it and hurry the current one.
          if (next === to.index) { queued = null; return false; }
          queued = v;
          speed = 2.5;
          return true;
        }
        from = to;
        to = v;
        k = 0;
        speed = 1;
        dur = rollDuration();
        return true;
      },
      /// Somewhere else. Never the one already showing, so every change is a
      /// change.
      goRandom() {
        const n = PRESETS.length;
        if (n < 2) return this.go(0);
        const here = this.target;
        let next = Math.floor(rand() * (n - 1)) % (n - 1);
        if (next >= here) next += 1;
        return this.go(next);
      },
      /// Jump with no transition. Only for the very first frame.
      set(i) {
        from = to = makeVariant(i, rand);
        k = 1; queued = null; speed = 1; dur = base;
      },
      advance(dt) {
        if (k >= 1) return k;
        k = Math.min(1, k + (Math.max(0, dt) / dur) * speed);
        if (k >= 1) {
          // blendState(from, to, 1) and blendState(to, x, 0) are the same
          // picture, which is why the handover is invisible.
          from = to;
          speed = 1;
          if (queued != null) { to = queued; queued = null; k = 0; dur = rollDuration(); }
        }
        return k;
      },
      state(t, audio) { return blendState(from, to, k, t, audio); },
    };
  }

  const api = { PRESETS, WAVES, FOLDS, MODES, blendState, makeVariant, makeBlender, ease, hsv, turnHue, rng, TAU };
  // The page reaches it as a global; the tests require it directly.
  (typeof window !== 'undefined' ? window : globalThis).AMMPresets = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})();
