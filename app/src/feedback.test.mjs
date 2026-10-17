// The Feedback screen without a window: where the strip puts each section
// and bar, what a click on it picks, what the away switch says, and what the
// buttons send to the Rust side.
//
//   cd app/src && node --test feedback.test.mjs

import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

function makeSandbox(reply = {}) {
  const listeners = {};
  const invoked = [];
  const elements = {};
  const handlers = {};
  const el = (id) => (elements[id] ||= {
    id, textContent: '', innerHTML: '', hidden: false, value: '',
    dataset: {}, style: {},
    getAttribute: () => null,
    focus: () => { elements.__focused = id; },
    getBoundingClientRect: () => ({ left: 0, width: 400, height: 120 }),
    getContext: () => new Proxy({}, { get: () => () => {}, set: () => true }),
  });
  const sandbox = {
    console,
    Date,
    setInterval: () => 0,
    setTimeout: () => 0,
    devicePixelRatio: 2,
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    prompt: () => 'but shorter',
    document: {
      documentElement: {},
      querySelector: (s) => el(s.replace('#', '')),
      addEventListener: (type, fn) => { handlers[type] = fn; },
    },
    __TAURI__: {
      core: { invoke: async (cmd, args) => { invoked.push([cmd, args]); return reply[cmd] ?? {}; } },
      event: { listen: (name, fn) => { listeners[name] = fn; } },
    },
    __invoked: invoked,
    __el: el,
    __handlers: handlers,
    __listeners: listeners,
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(new URL('./feedback.js', import.meta.url), 'utf8'), sandbox);
  return sandbox;
}

const strip = {
  song_key: 'song-1',
  revision: 43,
  takes: 2,
  sections: [
    { name: 'Intro', status: 'current', played: '14:02', bars: [1, 2, 3, 4].map((bar) => ({ bar, lufs: -18, peak: -6, revision: 43 })) },
    { name: 'Drop', status: 'stale', played: '13:40', bars: [9, 10, 11, 12].map((bar) => ({ bar, lufs: -9, peak: -0.4, revision: 41 })) },
    { name: 'Outro', status: 'never', played: null, bars: [] },
  ],
};

const state = (over = {}) => ({
  song_key: 'song-1',
  set_name: 'Night Drive',
  strip,
  position: { bar: 10, section: 'Drop', playing: true },
  screen: {
    threads: [
      { comment: { id: 'c-1', bar: 11, section: 'Drop', text: 'hats harsh' }, replies: [{ text: 'hats down 2 dB' }] },
      { comment: { id: 'c-2', bar: 3, section: 'Intro', text: 'pad too long' }, replies: [] },
    ],
    questions: [
      { id: 'o-1', question: 'Which breakdown?', options: [
        { n: 1, name: 'A', what: 'sparse', hear: 'bars 65–72' },
        { n: 2, name: 'B', what: 'full', hear: 'locator Break B' },
      ], picked: null },
    ],
    away: { on: false, since_ms: 0, until_ms: 0, off_reason: 'played' },
    away_on: false,
  },
  ...over,
});

const tick = () => new Promise((r) => setImmediate(r));
// Objects made inside the sandbox have the sandbox's prototypes: compare them as data.
const plain = (v) => JSON.parse(JSON.stringify(v));

test('the strip lays sections out in order, and an unheard one still takes room', () => {
  const s = makeSandbox();
  const F = s.AMMFeedback;
  const lay = F.layout(strip, 120);
  assert.deepEqual(plain(lay.map((x) => x.name)), ['Intro', 'Drop', 'Outro']);
  // 4 + 4 bars heard, and the unheard Outro is drawn four bars wide.
  assert.equal(lay[0].x0, 0);
  assert.equal(lay[2].x1, 120);
  assert.equal(lay[2].x1 - lay[2].x0, 40);
  assert.equal(lay[2].bars.length, 0);
  assert.equal(lay[1].bars[0].bar, 9);
  assert.equal(F.layout({ sections: [] }, 100).length, 0);
});

test('a click on the strip picks the bar under it; an unheard section has none', () => {
  const s = makeSandbox();
  const F = s.AMMFeedback;
  const lay = F.layout(strip, 120);
  assert.deepEqual(plain(F.barAt(lay, 1)), { section: 'Intro', bar: 1, status: 'current' });
  assert.deepEqual(plain(F.barAt(lay, 55)), { section: 'Drop', bar: 10, status: 'stale' });
  assert.equal(F.barAt(lay, 100).bar, null);
  assert.equal(F.barAt(lay, 500), null);
});

test('loudness is drawn with 0 LUFS at the top and −40 at the bottom', () => {
  const F = makeSandbox().AMMFeedback;
  assert.equal(F.lufsY(0, 100), 0);
  assert.equal(F.lufsY(-40, 100), 100);
  assert.equal(F.lufsY(-60, 100), 100);
  assert.equal(F.lufsY(-20, 100), 50);
});

test('the away switch says when it ends and why it went off', () => {
  const F = makeSandbox().AMMFeedback;
  const now = new Date(2026, 9, 17, 14, 40).getTime();
  const until = new Date(2026, 9, 17, 15, 2).getTime();
  assert.equal(F.awayText({ on: true, until_ms: until }, true, now), 'On until 15:02 · 22 min left · turns off when you press play');
  assert.equal(F.awayText({ on: false, off_reason: 'played' }, false, now), 'Off — you pressed play in Live');
  assert.equal(F.awayText({ on: false, off_reason: 'time' }, false, now), 'Off — its time ran out');
  assert.match(F.awayText(null, false, now), /^Off — Claude never plays/);
});

test('a question is a row per option with ▶ and This one, and a pick shows its note', () => {
  const F = makeSandbox().AMMFeedback;
  const q = state().screen.questions[0];
  const html = F.questionHtml(q);
  assert.equal((html.match(/data-act="fb-play"/g) || []).length, 2);
  assert.equal((html.match(/data-act="fb-pick"/g) || []).length, 2);
  assert.match(html, /waiting for your pick/);
  assert.match(html, /locator Break B/);
  const picked = F.questionHtml({ ...q, picked: { option: 2, note: 'but shorter' } });
  assert.equal((picked.match(/data-act="fb-pick"/g) || []).length, 1, 'the picked row loses its button');
  assert.match(picked, /Picked · but shorter/);
  assert.doesNotMatch(picked, /waiting/);
  // Text from a question is escaped, never markup.
  const evil = F.questionHtml({ ...q, question: '<img src=x>' });
  assert.doesNotMatch(evil, /<img/);
});

test('a thread shows its bar, its section and Claude’s replies', () => {
  const F = makeSandbox().AMMFeedback;
  const [answered, open] = state().screen.threads;
  const a = F.threadHtml(answered);
  assert.match(a, /bar 11 · Drop/);
  assert.match(a, /hats down 2 dB/);
  assert.match(F.threadHtml(open), /has not answered yet/);
});

test('the screen asks for its state, and the buttons send what the producer chose', async () => {
  const s = makeSandbox({ feedback_state: state() });
  s.__feedback.onScreen('feedback');
  await tick();
  assert.deepEqual(plain(s.__invoked[0]), ['feedback_state', {}]);
  assert.match(s.__el('fb-sub').textContent, /Night Drive · revision 43 · 2 takes heard/);
  assert.match(s.__el('fb-bar').innerHTML, /playing at bar 10 · Drop/);
  assert.match(s.__el('fb-bar').innerHTML, /you pressed play/);
  assert.equal(s.__el('fbbadge').textContent, 1, 'one question waits');
  const click = (dataset) => s.__handlers.click({ target: { closest: (sel) => (sel === '[data-act]' ? { dataset } : null) } });
  click({ act: 'fb-play', offer: 'o-1', n: '2' });
  assert.deepEqual(plain(s.__invoked.at(-1)), ['feedback_play', { offer: 'o-1', option: 2 }]);
  click({ act: 'fb-pick', offer: 'o-1', n: '1' });
  assert.deepEqual(plain(s.__invoked.at(-1)), ['feedback_pick', { offer: 'o-1', option: 1, note: 'but shorter' }]);
  // The away switch.
  s.__handlers.click({ target: { closest: (sel) => (sel === '#sw-away' ? { getAttribute: () => 'false' } : null) } });
  assert.deepEqual(plain(s.__invoked.at(-1)), ['feedback_away', { on: true }]);
});

test('M stamps the bar playing now, and Enter sends the comment on it', async () => {
  const s = makeSandbox({ feedback_state: state() });
  s.__feedback.onScreen('feedback');
  await tick();
  s.__handlers.keydown({ key: 'm', target: { tagName: 'BODY' }, preventDefault() {} });
  assert.equal(s.__el('fb-at').textContent, 'bar 10 · Drop');
  s.__el('fb-text').value = 'too long here';
  s.__handlers.keydown({ key: 'Enter', target: { tagName: 'INPUT' }, preventDefault() {} });
  await tick();
  assert.deepEqual(plain(s.__invoked.find((c) => c[0] === 'feedback_comment')), ['feedback_comment', { text: 'too long here', bar: 10 }]);
  // Nothing is sent from another screen.
  s.__feedback.onScreen('listen');
  const before = s.__invoked.length;
  s.__handlers.keydown({ key: 'm', target: { tagName: 'BODY' }, preventDefault() {} });
  assert.equal(s.__invoked.length, before);
});
