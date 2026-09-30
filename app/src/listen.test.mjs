// The Listen screen without a window: the state machine that decides what the
// producer is told, and the scale the spectrum is drawn to.
//
//   cd app/src && node --test listen.test.mjs
//
// Both files are written for a browser, so this gives them the few globals
// they touch and then drives them the way the app does.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// ── the smallest browser these two files need ─────────────────────────────
function canvasStub(w = 600, h = 300) {
  const calls = [];
  const ctx = new Proxy(
    { fillRect: (...a) => calls.push(['fillRect', ...a]), fillText: (...a) => calls.push(['fillText', ...a]) },
    {
      get(target, prop) {
        if (prop in target) return target[prop];
        // Every other 2d method is a recorder; every property is settable.
        return (...a) => calls.push([String(prop), ...a]);
      },
      set(target, prop, value) { target[prop] = value; return true; },
    }
  );
  return {
    width: w, height: h, calls,
    getContext: () => ctx,
    getBoundingClientRect: () => ({ width: w, height: h }),
  };
}

function makeSandbox() {
  const listeners = {};
  const invoked = [];
  const elements = {};
  const el = (id) => (elements[id] ||= {
    id, textContent: '', innerHTML: '', hidden: false, className: '',
    dataset: {}, style: {}, classList: { toggle: () => {}, add: () => {}, remove: () => {} },
    addEventListener: () => {},
  });
  const sandbox = {
    console,
    performance,
    localStorage: { getItem: () => null, setItem: () => {} },
    requestAnimationFrame: () => 0,
    cancelAnimationFrame: () => {},
    setInterval: () => 0,
    matchMedia: () => ({ matches: false }),
    devicePixelRatio: 2,
    getComputedStyle: () => ({ getPropertyValue: () => '#E39B21' }),
    document: {
      documentElement: {},
      querySelector: (s) => el(s.replace('#', '')),
      addEventListener: (_t, fn) => { sandbox.__click = fn; },
    },
    __TAURI__: {
      core: { invoke: async (cmd, args) => { invoked.push([cmd, args]); return sandbox.__reply[cmd] ?? {}; } },
      event: { listen: (name, fn) => { listeners[name] = fn; } },
    },
    __reply: {},
    __listeners: listeners,
    __invoked: invoked,
    __el: el,
  };
  sandbox.window = sandbox;
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(new URL('./spectrum.js', import.meta.url), 'utf8'), sandbox);
  return sandbox;
}

function loadListen(sandbox, status) {
  sandbox.__reply.listen_status = status;
  vm.runInContext(fs.readFileSync(new URL('./listen.js', import.meta.url), 'utf8'), sandbox);
  return sandbox;
}

const frame = (over = {}) => ({
  bands: new Array(72).fill(-40),
  hold: new Array(72).fill(-35),
  peak: [-6, -6.4], rms: [-14, -14.2], peak_hold: -3.1,
  correlation: 0.71, clip: false, ranges: [-9, -6, -12, -8, -14, -20],
  silent: false, silent_for_ms: null, live_playing: -1,
  ...over,
});

// ── the scale ─────────────────────────────────────────────────────────────

test('the spectrum is drawn to one scale, 0 dB at the top', () => {
  const s = makeSandbox();
  const c = canvasStub(600, 300);
  // A single loud band and a single quiet one must land far apart, with the
  // loud one nearer the top of the drawing.
  const bands = new Array(72).fill(-80);
  bands[10] = 0;     // full scale
  bands[40] = -36;   // half way down a 72 dB range
  s.AMMSpectrum.drawSpectrum(c, { bands, hold: [] }, { labels: true });
  const bars = c.calls.filter((k) => k[0] === 'fillRect' && k[4] > 1);
  assert.ok(bars.length >= 2, 'both bands were drawn');
  const tall = bars.reduce((a, b) => (b[4] > a[4] ? b : a));
  const short = bars.reduce((a, b) => (b[4] < a[4] ? b : a));
  // The full-scale band reaches (almost) the whole plot; −36 dB is near half.
  assert.ok(tall[4] / short[4] > 1.8, `full scale ${tall[4]} vs −36 dB ${short[4]}`);
  // Labels name values the chart reaches.
  const labels = c.calls.filter((k) => k[0] === 'fillText').map((k) => k[1]);
  for (const want of ['0', '-72', '20', '1k', '20k']) {
    assert.ok(labels.includes(want), `axis labels include ${want}: ${labels.join(',')}`);
  }
});

test('the canvas matches the device pixel ratio so nothing is blurred', () => {
  const s = makeSandbox();
  const c = canvasStub(600, 300);
  s.AMMSpectrum.drawSpectrum(c, { bands: [], hold: [] }, {});
  assert.equal(c.width, 1200);
  assert.equal(c.height, 600);
});

test('levels are written the way every level in this product is written', () => {
  const s = makeSandbox();
  const { db, hz } = s.AMMSpectrum;
  assert.equal(db(-3.2), '−3.2');
  assert.equal(db(0), '−0.0');
  assert.equal(db(1.5), '+1.5');
  assert.equal(db(-80), '−∞');
  assert.equal(db(null), '−∞');
  assert.equal(hz(62), '62 Hz');
  assert.equal(hz(1500), '1.5 kHz');
  assert.equal(hz(16000), '16 kHz');
});

test('the loudest band is found and named by its centre frequency', () => {
  const s = makeSandbox();
  const bands = new Array(72).fill(-60);
  bands[0] = -10;                    // the 20–21 Hz band
  const low = s.AMMSpectrum.loudestBand({ bands });
  assert.ok(low.freq > 20 && low.freq < 22, `got ${low.freq}`);
  assert.equal(low.db, -10);
  assert.equal(s.AMMSpectrum.loudestBand({ bands: new Array(72).fill(-80) }), null);
});

test('smoothing falls slowly but rises at once', () => {
  const s = makeSandbox();
  const prev = { bands: [-10, -10] };
  const next = { bands: [-40, -5] };
  const fast = s.AMMSpectrum.smooth(prev, next, 'fast');
  const slow = s.AMMSpectrum.smooth(prev, next, 'slow');
  assert.equal(fast.bands[1], -5, 'a rise is immediate');
  assert.ok(fast.bands[0] > -40 && fast.bands[0] < -10, 'a fall is eased');
  assert.ok(slow.bands[0] > fast.bands[0], 'slow eases more than fast');
});

// ── what the producer is told ─────────────────────────────────────────────

async function phaseFor(status, frames = []) {
  const s = makeSandbox();
  loadListen(s, status);
  await new Promise((r) => setImmediate(r));
  for (const f of frames) s.__listeners['listen:frame']({ payload: f });
  await new Promise((r) => setImmediate(r));
  return s.__el('listenbar');
}

test('an old macOS says so instead of failing', async () => {
  const bar = await phaseFor({ listening: false, available: false });
  assert.match(bar.innerHTML, /Needs macOS 14\.4 or later/);
  assert.equal(bar.className, 'listenbar attn');
});

test('no Live means nothing to listen to, and Start is off', async () => {
  const bar = await phaseFor({ listening: false, available: true, live_pid: null });
  assert.match(bar.innerHTML, /Live is not running/);
  assert.match(bar.innerHTML, /disabled/);
});

test('Live running but not listening offers to start', async () => {
  const bar = await phaseFor({ listening: false, available: true, live_pid: 42, live_name: 'Ableton Live 12 Suite' });
  assert.match(bar.innerHTML, /Not listening/);
  assert.match(bar.innerHTML, /Ableton Live 12 Suite/);
  assert.match(bar.innerHTML, /data-act="listen-start"/);
});

test('while audio arrives the state line names the format and the lag', async () => {
  const bar = await phaseFor(
    { listening: true, available: true, live_pid: 42, sample_rate: 48000, buffer_frames: 512, meter_lag_ms: 43.7, window_ms: 85.3, ranges: [] },
    [frame()]
  );
  assert.match(bar.innerHTML, /Listening to Live/);
  assert.match(bar.innerHTML, /48\.0 kHz/);
  assert.match(bar.innerHTML, /512-sample buffer/);
  assert.match(bar.innerHTML, /meters 44 ms behind/);
  assert.match(bar.innerHTML, /spectrum over 85 ms/);
});

test('silence with the transport stopped is called what it is', async () => {
  const bar = await phaseFor(
    { listening: true, available: true, live_pid: 42, ranges: [] },
    [frame({ silent: true, silent_for_ms: 4000, live_playing: 0 })]
  );
  assert.match(bar.innerHTML, /Live is open and quiet/);
  assert.equal(bar.className, 'listenbar ');
});

test('silence while Live is playing points at the permission', async () => {
  const bar = await phaseFor(
    { listening: true, available: true, live_pid: 42, ranges: [] },
    [frame({ silent: true, silent_for_ms: 4000, live_playing: 1 })]
  );
  assert.match(bar.innerHTML, /playing, but nothing arrives/);
  assert.equal(bar.className, 'listenbar down');
  assert.match(bar.innerHTML, /data-act="listen-settings"/);
});

test('a short silence is not yet an alarm', async () => {
  const bar = await phaseFor(
    { listening: true, available: true, live_pid: 42, ranges: [] },
    [frame({ silent: true, silent_for_ms: 900, live_playing: 1 })]
  );
  assert.match(bar.innerHTML, /Listening to Live/);
  assert.doesNotMatch(bar.innerHTML, /nothing arrives/);
});

test('the gate explains the permission before anything is asked for', async () => {
  const s = makeSandbox();
  loadListen(s, { listening: false, available: true, live_pid: 42 });
  await new Promise((r) => setImmediate(r));
  const gate = s.__el('listen-gate');
  assert.equal(gate.hidden, false);
  assert.match(gate.innerHTML, /analysed in memory and thrown away/);
  assert.match(gate.innerHTML, /never saved|nothing is recorded/);
  assert.match(gate.innerHTML, /Screen &amp; System Audio Recording/);
  assert.equal(s.__el('listen-scope').hidden, true);
});

test('a frame turns into the readouts the producer reads', async () => {
  const s = makeSandbox();
  loadListen(s, {
    listening: true, available: true, live_pid: 42, sample_rate: 48000,
    ranges: [{ name: 'sub', lo: 20, hi: 60 }, { name: 'bass', lo: 60, hi: 200 },
             { name: 'low mids', lo: 200, hi: 500 }, { name: 'mids', lo: 500, hi: 2000 },
             { name: 'presence', lo: 2000, hi: 6000 }, { name: 'air', lo: 6000, hi: 20000 }],
  });
  await new Promise((r) => setImmediate(r));
  s.__listeners['listen:frame']({ payload: frame() });
  // The drawing loop is driven by requestAnimationFrame, which this sandbox
  // does not run, so call the readouts the way a frame would.
  s.__el('spec').getContext = () => canvasStub().getContext();
  await new Promise((r) => setImmediate(r));
  assert.equal(s.__el('listen-scope').hidden, false);
});

test('the float window is asked for by name, and only once', async () => {
  const s = makeSandbox();
  loadListen(s, { listening: true, available: true, live_pid: 42, ranges: [] });
  await new Promise((r) => setImmediate(r));
  const target = { closest: (sel) => (sel === '[data-act]' ? { dataset: { act: 'listen-float' } } : null) };
  s.__click({ target });
  await new Promise((r) => setImmediate(r));
  const calls = s.__invoked.filter((c) => c[0] === 'listen_float');
  assert.equal(calls.length, 1);
  assert.equal(calls[0][1].open, true);
});

test('stopping tells the Rust side and forgets the last frame', async () => {
  const s = makeSandbox();
  loadListen(s, { listening: true, available: true, live_pid: 42, ranges: [] });
  await new Promise((r) => setImmediate(r));
  s.__listeners['listen:frame']({ payload: frame() });
  const target = { closest: (sel) => (sel === '[data-act]' ? { dataset: { act: 'listen-stop' } } : null) };
  s.__reply.listen_status = { listening: false, available: true, live_pid: 42 };
  s.__click({ target });
  await new Promise((r) => setImmediate(r));
  await new Promise((r) => setImmediate(r));
  assert.ok(s.__invoked.some((c) => c[0] === 'listen_stop'));
  assert.match(s.__el('listenbar').innerHTML, /Not listening/);
});
