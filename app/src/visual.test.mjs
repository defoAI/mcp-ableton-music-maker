// The visual's randomness and its transitions, without a GPU.
//
//   cd app/src && node --test visual.test.mjs
//
// Two properties pull against each other and both have to hold.
//
// It never repeats: each visit to a preset mints a fresh variant, and the
// order it wanders in is not a cycle.
//
// You never see the change: every number the renderer is handed moves
// continuously, frame after frame, including across the moment one variant
// finishes becoming the next. These tests drive the blender the way the render
// loop does and check that nothing ever jumps.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

function load() {
  const sandbox = { console, Math, Object, Array, JSON };
  sandbox.window = sandbox;
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(new URL('./visual-presets.js', import.meta.url), 'utf8'), sandbox);
  return sandbox.AMMPresets;
}
const P = load();

// Audio held still, so anything that moves is the transition and not the music.
const AUDIO = { vol: 0.6, bass: 0.5, mid: 0.4, treb: 0.3, bassAtt: 0.5, midAtt: 0.4, trebAtt: 0.3, beat: 0.2 };
const NUMS = ['zoom', 'rot', 'dx', 'dy', 'sx', 'sy', 'bright', 'decay', 'warp', 'warpSpeed', 'hueRate', 'gamma', 'swirl', 'ripple', 'chroma'];
const seeded = (n) => P.rng(n);
const blender = (seed, seconds = 6) => P.makeBlender({ seconds, rand: seeded(seed) });

/// What the shaders actually apply. `blend` itself is an internal coordinate
/// and is deliberately not compared: the shader never uses it alone, only to
/// weigh mode A against mode B, and it is that weighting which must not jump.
/// A settled variant (A and B the same, blend 1) and the first frame of a new
/// transition (blend 0, A unchanged) weigh the modes identically, which is
/// exactly why the handover cannot be seen.
function effective(s) {
  const out = {};
  for (const k of NUMS) out[k] = s[k];
  for (let m = 0; m < P.MODES; m++) {
    out['mode' + m] = (s.modeA === m ? 1 - s.blend : 0) + (s.modeB === m ? s.blend : 0);
  }
  for (const n of P.FOLDS) {
    out['fold' + n] = (s.kaleidoA === n ? 1 - s.blend : 0) + (s.kaleidoB === n ? s.blend : 0);
  }
  out.ink = s.waves.reduce((n, w) => n + w.weight, 0);
  out.scale = s.waves.reduce((n, w) => n + w.weight * w.scale, 0);
  out.thick = s.waves.reduce((n, w) => n + w.weight * w.thick, 0);
  out.red = s.waves.reduce((n, w) => n + w.weight * w.colA[0], 0);
  out.green = s.waves.reduce((n, w) => n + w.weight * w.colA[1], 0);
  out.blue = s.waves.reduce((n, w) => n + w.weight * w.colA[2], 0);
  return out;
}

/// The biggest single-frame move any field made, measured against that
/// field's own range over the run — because "warp speed moved 0.03" means
/// nothing until you know it travels across 3.4, while "brightness moved
/// 0.03" is most of its whole range. A real cut lands near 1.0: the picture
/// crosses the entire distance between two variants in one frame. A smooth
/// transition spreads that over a hundred frames or more.
function worstStep(states) {
  const fields = Object.keys(effective(states[0]));
  const series = new Map(fields.map((f) => [f, []]));
  for (const s of states) {
    const e = effective(s);
    for (const f of fields) {
      assert.ok(Number.isFinite(e[f]), `${f} stopped being a number`);
      series.get(f).push(e[f]);
    }
  }
  let worst = { field: null, ratio: 0, delta: 0, span: 0, at: -1 };
  for (const f of fields) {
    const v = series.get(f);
    let lo = Infinity, hi = -Infinity;
    for (const x of v) { if (x < lo) lo = x; if (x > hi) hi = x; }
    const span = hi - lo;
    for (let i = 1; i < v.length; i++) {
      const d = Math.abs(v[i] - v[i - 1]);
      if (span < 1e-12) { assert.ok(d < 1e-12, `${f} moved while its range was flat`); continue; }
      const ratio = d / span;
      if (ratio > worst.ratio) worst = { field: f, ratio, delta: d, span, at: i };
    }
  }
  return worst;
}
const jumped = (w) => `${w.field} moved ${(w.ratio * 100).toFixed(1)}% of its whole range in one frame `
  + `(${w.delta.toPrecision(3)} of ${w.span.toPrecision(3)}) at frame ${w.at}`;

// ── it never repeats ──────────────────────────────────────────────────────

test('a variant of the same preset is different every time', () => {
  const r = seeded(7);
  const seen = new Set();
  for (let i = 0; i < 60; i++) {
    const v = P.makeVariant(2, r);
    seen.add([v.mode, v.kaleido, v.wave, v.warp.toFixed(3), v.swirl.toFixed(3), v.colA.join(',')].join('|'));
  }
  assert.ok(seen.size > 55, `60 visits produced only ${seen.size} different looks`);
});

test('however it is rolled, a variant stays inside what the renderer can draw', () => {
  const r = seeded(11);
  for (let i = 0; i < 4000; i++) {
    const v = P.makeVariant(i % P.PRESETS.length, r);
    assert.ok(P.WAVES.includes(v.wave), `unknown shape ${v.wave}`);
    assert.ok(Number.isInteger(v.mode) && v.mode >= 0 && v.mode < P.MODES, `warp mode ${v.mode}`);
    assert.ok(v.kaleido === 0 || v.kaleido >= 3, `a fold of ${v.kaleido} is not a fold`);
    // A decay of 1 never fades and the screen fills; too low and there are no
    // trails at all, which is the whole look.
    assert.ok(v.decay > 0.93 && v.decay < 0.995, `decay ${v.decay}`);
    assert.ok(v.scale > 0.1 && v.scale < 0.75, `scale ${v.scale}`);
    assert.ok(v.thick > 0.002 && v.thick < 0.025, `thickness ${v.thick}`);
    assert.ok(v.gamma > 0.8 && v.gamma < 1.6, `gamma ${v.gamma}`);
    assert.ok(v.warp >= 0 && v.warp <= 2.2, `warp ${v.warp}`);
    assert.ok(v.warpSpeed > 0 && v.warpSpeed <= 3.5, `warp speed ${v.warpSpeed}`);
    assert.ok(Math.abs(v.hueRate) <= 0.5, `hue rate ${v.hueRate}`);
    assert.ok(Math.abs(v.swirl) <= 0.6, `swirl ${v.swirl}`);
    assert.ok(v.ripple >= 0 && v.ripple <= 0.7, `ripple ${v.ripple}`);
    assert.ok(v.chroma > 0 && v.chroma <= 3, `chroma ${v.chroma}`);
    for (const c of [v.colA, v.colB]) {
      assert.equal(c.length, 3);
      for (const ch of c) assert.ok(ch >= 0 && ch <= 1, `a colour left 0..1: ${c}`);
    }
    const f = v.frame(3, AUDIO);
    for (const k of ['zoom', 'rot', 'dx', 'dy', 'sx', 'sy', 'bright']) {
      assert.ok(Number.isFinite(f[k]), `${k} is not a number`);
    }
    assert.ok(f.zoom > 0.5 && f.zoom < 1.6, `zoom ${f.zoom} would collapse or explode the picture`);
  }
});

test('where it wanders next is never where it already is', () => {
  const b = blender(23);
  b.set(0);
  const visits = [];
  for (let i = 0; i < 300; i++) {
    const here = b.target;
    b.goRandom();
    assert.notEqual(b.target, here, 'it wandered to where it already was');
    visits.push(b.target);
    while (b.blending) b.advance(1 / 30);
  }
  // Every preset gets a turn, and it is not marching in order.
  assert.equal(new Set(visits).size, P.PRESETS.length, 'some presets never came up');
  const inOrder = visits.filter((v, i) => i > 0 && v === (visits[i - 1] + 1) % P.PRESETS.length).length;
  assert.ok(inOrder < visits.length * 0.5, 'it is really just cycling in order');
});

test('the pace of the changes is not a pattern either', () => {
  const b = blender(29);
  b.set(0);
  const lengths = new Set();
  for (let i = 0; i < 40; i++) { b.goRandom(); lengths.add(b.duration.toFixed(4)); while (b.blending) b.advance(1 / 30); }
  assert.ok(lengths.size > 30, `40 transitions took only ${lengths.size} different lengths`);
});

test('a seeded run replays exactly, so a fault can be chased', () => {
  const run = (seed) => {
    const b = blender(seed);
    b.set(0);
    const out = [];
    for (let i = 0; i < 300; i++) { if (i % 50 === 0) b.goRandom(); b.advance(1 / 60); out.push(JSON.stringify(b.state(5, AUDIO))); }
    return out.join('');
  };
  assert.equal(run(99), run(99));
  assert.notEqual(run(99), run(100));
});

// ── you never see the change ──────────────────────────────────────────────

test('a transition ends exactly on each variant, with nothing left over', () => {
  const r = seeded(3);
  for (let i = 0; i < 40; i++) {
    const va = P.makeVariant(i % P.PRESETS.length, r);
    const vb = P.makeVariant((i + 3) % P.PRESETS.length, r);
    const start = P.blendState(va, vb, 0, 4, AUDIO);
    const end = P.blendState(va, vb, 1, 4, AUDIO);
    const onlyA = P.blendState(va, va, 0, 4, AUDIO);
    const onlyB = P.blendState(vb, vb, 1, 4, AUDIO);
    for (const k of NUMS) {
      assert.ok(Math.abs(start[k] - onlyA[k]) < 1e-9, `${k} at the start is not the first variant's`);
      assert.ok(Math.abs(end[k] - onlyB[k]) < 1e-9, `${k} at the end is not the second variant's`);
    }
    assert.equal(start.waves.length, 1, 'one shape at the start');
    assert.equal(end.waves.length, 1, 'one shape at the end');
    assert.equal(start.waves[0].shape, va.wave);
    assert.equal(end.waves[0].shape, vb.wave);
  }
});

test('the wave never dims or doubles during a change', () => {
  const r = seeded(5);
  for (let n = 0; n < 40; n++) {
    const va = P.makeVariant(n % P.PRESETS.length, r), vb = P.makeVariant((n + 1) % P.PRESETS.length, r);
    for (let i = 0; i <= 40; i++) {
      const s = P.blendState(va, vb, i / 40, 7, AUDIO);
      const ink = s.waves.reduce((a, w) => a + w.weight, 0);
      assert.ok(Math.abs(ink - 1) < 1e-9, `weights sum to ${ink}`);
      for (const w of s.waves) assert.ok(w.weight >= 0 && w.weight <= 1, 'a weight left 0..1');
    }
  }
});

test('the ease starts and ends at a standstill, so neither edge shows', () => {
  assert.equal(P.ease(0), 0);
  assert.equal(P.ease(1), 1);
  const slope = (x, h = 1e-4) => (P.ease(x + h) - P.ease(x - h)) / (2 * h);
  assert.ok(Math.abs(slope(0.001)) < 1e-3, `slope at the start was ${slope(0.001)}`);
  assert.ok(Math.abs(slope(0.999)) < 1e-3, `slope at the end was ${slope(0.999)}`);
  assert.ok(slope(0.5) > 1, 'and gets on with it in the middle');
  let prev = -1;
  for (let i = 0; i <= 100; i++) { const v = P.ease(i / 100); assert.ok(v >= prev, 'ease went backwards'); prev = v; }
});

test('every frame of a full transition is a small step from the last', () => {
  const b = blender(41);
  b.set(0);
  b.goRandom();
  const states = [];
  for (let i = 0; i < 900; i++) { states.push(b.state(9, AUDIO)); b.advance(1 / 60); }
  const worst = worstStep(states);
  assert.ok(worst.ratio < 0.06, jumped(worst));
  assert.ok(!b.blending, 'and it finished');
});

test('a change asked for mid-transition is queued, not snapped', () => {
  const b = blender(43);
  b.set(0);
  b.go(2);
  for (let i = 0; i < 60; i++) b.advance(1 / 60);
  assert.ok(b.blending);
  const before = b.state(3, AUDIO);
  assert.equal(b.go(7), true);
  const after = b.state(3, AUDIO);
  for (const k of NUMS) assert.ok(Math.abs(after[k] - before[k]) < 1e-9, `${k} snapped when the key was pressed`);
  assert.equal(b.index, 2, 'still finishing the transition in flight');
  assert.equal(b.target, 7, 'but heading for the new one');
});

test('the handover between two transitions is invisible', () => {
  const b = blender(47);
  b.set(1);
  b.go(4);
  const states = [];
  for (let i = 0; i < 1500; i++) {
    states.push(b.state(11, AUDIO));
    b.advance(1 / 60);
    if (i === 40) b.go(6);
  }
  const worst = worstStep(states);
  assert.ok(worst.ratio < 0.06, jumped(worst));
  assert.equal(b.index, 6);
  assert.ok(!b.blending);
});

test('hours of wandering never once cuts', () => {
  const b = blender(53);
  b.set(0);
  // 30 fps for twenty minutes, changing whenever it feels like it. Time
  // really moves here, so the audio-driven terms move with it.
  const states = [];
  for (let i = 0; i < 36000; i++) {
    states.push(b.state(i / 30, AUDIO));
    b.advance(1 / 30);
    if (!b.blending && i % 240 === 0) b.goRandom();
  }
  const worst = worstStep(states);
  assert.ok(worst.ratio < 0.06, jumped(worst));
});

test('hammering the key stays continuous and lands somewhere sensible', () => {
  const b = blender(59);
  b.set(0);
  const states = [];
  for (let i = 0; i < 3000; i++) {
    states.push(b.state(13, AUDIO));
    b.advance(1 / 60);
    if (i % 37 === 0) b.goRandom();
  }
  const worst = worstStep(states);
  assert.ok(worst.ratio < 0.06, jumped(worst));
  assert.ok(b.index >= 0 && b.index < P.PRESETS.length);
});

test('asking again for the preset showing rolls it anew, smoothly', () => {
  const b = blender(61);
  b.set(3);
  const before = b.state(2, AUDIO);
  // Pressing its own number is "give me another one of these", which is a
  // transition like any other, not a no-op and not a cut.
  assert.equal(b.go(3), true);
  assert.ok(b.blending);
  assert.equal(b.index, 3, 'still the same preset');
  const after = b.state(2, AUDIO);
  for (const k of NUMS) assert.ok(Math.abs(after[k] - before[k]) < 1e-9, `${k} snapped on the way out`);
  const states = [];
  for (let i = 0; i < 900; i++) { states.push(b.state(2 + i / 60, AUDIO)); b.advance(1 / 60); }
  assert.ok(worstStep(states).ratio < 0.06, jumped(worstStep(states)));
});

test('asking for the one it is already heading towards does not restart it', () => {
  const b = blender(61);
  b.set(3);
  b.go(5);
  for (let i = 0; i < 30; i++) b.advance(1 / 60);
  const k = b.k;
  assert.equal(b.go(5), false);
  assert.equal(b.k, k, 'the transition restarted');
});

test('an interrupted transition still finishes, and faster', () => {
  const plain = blender(67);
  plain.set(0); plain.go(1);
  let plainFrames = 0;
  while (plain.blending && plainFrames < 10000) { plain.advance(1 / 60); plainFrames++; }

  const rushed = blender(67);
  rushed.set(0); rushed.go(1);
  for (let i = 0; i < 30; i++) rushed.advance(1 / 60);
  rushed.go(2);
  let rushedFrames = 30;
  while (rushed.index !== 2 && rushedFrames < 10000) { rushed.advance(1 / 60); rushedFrames++; }
  assert.ok(rushedFrames < plainFrames * 1.3, 'the queued change waited too long');
  assert.ok(rushedFrames > 30, 'and did not cut');
});

test('every preset is reachable by its number and is a distinct thing', () => {
  assert.equal(P.PRESETS.length, 8, 'keys 1-8 cover them all');
  const names = new Set();
  for (const [i, p] of P.PRESETS.entries()) {
    assert.ok(P.WAVES.includes(p.wave), `preset ${i} wants an unknown shape: ${p.wave}`);
    assert.ok(p.mode >= 0 && p.mode < P.MODES, `preset ${i} wants warp mode ${p.mode}`);
    assert.ok(p.name && !names.has(p.name), `preset ${i} has no distinct name`);
    names.add(p.name);
  }
});

test('two variants that happen to share a shape keep one wave', () => {
  const r = seeded(71);
  let found = false;
  for (let i = 0; i < 200 && !found; i++) {
    const va = P.makeVariant(0, r), vb = P.makeVariant(4, r);
    if (va.wave !== vb.wave) continue;
    found = true;
    const mid = P.blendState(va, vb, 0.5, 2, AUDIO);
    assert.equal(mid.waves.length, 1);
    assert.equal(mid.waves[0].shape, va.wave);
    const lo = Math.min(va.scale, vb.scale), hi = Math.max(va.scale, vb.scale);
    assert.ok(mid.waves[0].scale >= lo && mid.waves[0].scale <= hi, 'the scale was interpolated');
  }
  assert.ok(found, 'no pair shared a shape in 200 rolls');
});
