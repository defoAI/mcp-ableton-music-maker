// The Prompts screen without a window: four cards, the body that follows the
// selection, what the copy button actually puts on the clipboard, and the two
// states that earn a banner.
//
//   cd app/src && node --test prompts.test.mjs
//
// `app.js` is written for a browser and holds no copy of the prompt text — it
// renders what the Tauri command returns — so this gives it the few globals it
// touches, a stubbed `prompts` command, and then drives it the way a producer
// would: open the screen, click a card, press Copy.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// Four prompts shaped exactly as app/src-tauri/src/prompts.rs returns them.
const FIXTURE = [
  {
    id: 'song', file: 'prompts/make-a-song.md',
    title: 'Make a song from nothing', when: 'For an empty set, or a new idea in an old one.',
    body: 'You are producing in my open Ableton Live set…\n\n1. Call get_context.',
    words: 583, card: 'An empty set, and a track at the end of it.',
    meta: '4 questions, then it keeps refining · make-a-song.md',
    steps: [{ name: 'It looks', what: 'Reads the set.', tools: ['get_context', 'adv_get_library_status'] }],
  },
  {
    id: 'perform', file: 'prompts/prepare-a-performance-set.md',
    title: 'Prepare a set I can perform', when: 'For a set that has to keep running, and keep changing.',
    body: 'You are running my open Ableton Live set as a live instrument…',
    words: 682, card: 'Sections and a setlist, then it drives.',
    meta: 'Never goes silent · prepare-a-performance-set.md',
    steps: [{ name: 'It writes while it plays', what: 'The next section.', tools: ['make_section', 'add_to_song'] }],
  },
  {
    id: 'finish', file: 'prompts/finish-what-i-have.md',
    title: 'Finish what I have already got', when: 'For the set that has been open for three weeks.',
    body: 'You are working in my open Ableton Live set…\n\nThere is already material here.',
    words: 387, card: 'Half-finished material, honestly assessed.',
    meta: 'Touches nothing without asking · finish-what-i-have.md',
    steps: [{ name: 'It looks', what: 'Every track.', tools: ['get_context'] }],
  },
  {
    id: 'sample', file: 'prompts/build-around-my-idea.md',
    title: 'Build around my sample or idea', when: 'For the thing you already love and cannot get past.',
    body: 'You are producing in my open Ableton Live set… I already have something.',
    words: 386, card: 'One loop, one recording, one idea.',
    meta: 'Nothing is copied or moved · build-around-my-idea.md',
    steps: [{ name: 'It places the source', what: 'Warped and looped.', tools: ['add_sample'] }],
  },
];

const HEALTHY = {
  clients: { desktop: { configured: true } },
  sessions: [],
  live: { live_reachable: true, up_to_date: true },
};

// ── the smallest browser app.js needs ─────────────────────────────────────
function makeSandbox(status = HEALTHY) {
  const elements = {};
  const copied = [];
  const el = (id) => (elements[id] ||= {
    id, textContent: '', innerHTML: '', hidden: false, value: '',
    dataset: {}, style: {}, disabled: false,
    classList: { add: () => {}, remove: () => {}, toggle: () => {} },
    addEventListener: () => {},
    setAttribute: () => {}, removeAttribute: () => {}, getAttribute: () => null,
    remove: () => {}, select: () => {},
  });
  const sandbox = {
    console,
    setTimeout, clearTimeout,
    setInterval: () => 0,
    confirm: () => false,
    navigator: { clipboard: { writeText: (t) => { copied.push(t); return Promise.resolve(); } } },
    document: {
      body: { appendChild: () => {} },
      createElement: () => el('scratch'),
      documentElement: {},
      querySelector: (s) => el(s.replace('#', '')),
      querySelectorAll: () => [],
      addEventListener: (_t, fn) => { sandbox.__click = fn; },
    },
    __TAURI__: {
      core: {
        invoke: async (cmd, args) => {
          sandbox.__invoked.push([cmd, args]);
          const r = sandbox.__reply[cmd];
          if (r === undefined) throw new Error(`no stub for ${cmd}`);
          return r;
        },
      },
      event: { listen: () => {} },
    },
    __reply: {
      get_status: status,
      activity_sessions: [],
      prompts: { prompts: FIXTURE },
    },
    __invoked: [],
    __copied: copied,
    __el: el,
  };
  sandbox.window = sandbox;
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(fs.readFileSync(new URL('./app.js', import.meta.url), 'utf8'), sandbox);
  return sandbox;
}

const settle = () => new Promise((r) => setTimeout(r, 0));

// A click the real handler will see: every other `closest` misses.
function click(sandbox, selector, node) {
  sandbox.__click({ target: { closest: (s) => (s === selector ? node : null) } });
}

async function openPrompts(status = HEALTHY) {
  const sandbox = makeSandbox(status);
  await settle();
  sandbox.__goto('prompts');
  await settle();
  return sandbox;
}

// ── the card rail ─────────────────────────────────────────────────────────

test('the screen asks Rust for the prompts and draws all four', async () => {
  const s = await openPrompts();
  assert.ok(s.__invoked.some(([cmd]) => cmd === 'prompts'), 'never called the prompts command');
  const rail = s.__el('prail').innerHTML;
  for (const p of FIXTURE) {
    assert.ok(rail.includes(`data-pick="${p.id}"`), `${p.id} has no card`);
    assert.ok(rail.includes(p.title), `${p.title} is not on its card`);
    assert.ok(rail.includes(p.card), `${p.id} has no one-liner`);
  }
  assert.equal((rail.match(/data-pick=/g) || []).length, 4);
  assert.ok(rail.includes('aria-pressed="true"'), 'nothing is selected');
});

test('the first prompt is shown in full, with its file and its length', async () => {
  const s = await openPrompts();
  assert.equal(s.__el('p-title').textContent, FIXTURE[0].title);
  assert.equal(s.__el('p-why').textContent, FIXTURE[0].when);
  assert.equal(s.__el('p-text').textContent, FIXTURE[0].body);
  assert.equal(s.__el('p-path').textContent, 'prompts/make-a-song.md');
  assert.match(s.__el('p-len').textContent, /^583 words · about 800 tokens/);
  assert.ok(s.__el('p-steps').innerHTML.includes('adv_get_library_status'), 'the tool chips are missing');
});

test('the screen holds no copy of the prompt text', () => {
  const js = fs.readFileSync(new URL('./app.js', import.meta.url), 'utf8');
  assert.ok(!js.includes('get_context.'), 'prompt prose has been pasted into the UI');
  assert.ok(js.includes("invoke('prompts')"), 'the UI no longer reads the prompts from Rust');
});

// ── selection ─────────────────────────────────────────────────────────────

test('picking a card swaps the body, the title and the file', async () => {
  const s = await openPrompts();
  click(s, '[data-pick]', { dataset: { pick: 'finish' } });
  const p = FIXTURE[2];
  assert.equal(s.__el('p-title').textContent, p.title);
  assert.equal(s.__el('p-text').textContent, p.body);
  assert.equal(s.__el('p-path').textContent, p.file);
  assert.ok(s.__el('prail').innerHTML.includes(`data-pick="finish" aria-pressed="true"`));
});

// ── copy ──────────────────────────────────────────────────────────────────

test('the copy button carries the selected prompt, and says what to do next', async () => {
  const s = await openPrompts();
  click(s, '[data-act]', { dataset: { act: 'copyprompt' } });
  await settle();
  assert.deepEqual(s.__copied, [FIXTURE[0].body]);

  click(s, '[data-pick]', { dataset: { pick: 'perform' } });
  click(s, '[data-act]', { dataset: { act: 'copyprompt' } });
  await settle();
  assert.equal(s.__copied.length, 2);
  assert.equal(s.__copied[1], FIXTURE[1].body);
  assert.equal(s.__el('toast').textContent, 'Copied. Paste it into Claude and answer its questions.');
});

test('copying reaches nothing but the clipboard', async () => {
  const s = await openPrompts();
  const before = s.__invoked.length;
  click(s, '[data-act]', { dataset: { act: 'copyprompt' } });
  await settle();
  assert.equal(s.__invoked.length, before, 'copying called into Rust: ' + JSON.stringify(s.__invoked.slice(before)));
});

// ── the banner, in the two unhealthy states only ──────────────────────────

test('a healthy chain gets no banner', async () => {
  const s = await openPrompts();
  assert.equal(s.__el('pbanner').hidden, true);
  assert.equal(s.__el('pbanner').innerHTML, '');
});

test('no client configured: the banner says so and links to Setup', async () => {
  const s = await openPrompts({ clients: { desktop: { configured: false } }, sessions: [], live: { live_reachable: true } });
  const b = s.__el('pbanner');
  assert.equal(b.hidden, false);
  assert.match(b.innerHTML, /No client is configured/);
  assert.match(b.innerHTML, /data-go="setup"/);
  assert.match(b.innerHTML, /Finish setup/);
});

test('Live not running: the banner names the call that fails first', async () => {
  const s = await openPrompts({ clients: { desktop: { configured: true } }, sessions: [], live: { live_reachable: false } });
  const b = s.__el('pbanner');
  assert.equal(b.hidden, false);
  assert.match(b.innerHTML, /Live is not open/);
  assert.match(b.innerHTML, /get_context/);
  assert.match(b.innerHTML, /data-go="setup"/);
});

test('a running session counts as a configured client', async () => {
  const s = await openPrompts({ clients: {}, sessions: [{ pid: 1 }], live: { live_reachable: true } });
  assert.equal(s.__el('pbanner').hidden, true);
});

test('the prompt is still shown and still copyable with Live closed', async () => {
  const s = await openPrompts({ clients: { desktop: { configured: false } }, sessions: [], live: { live_reachable: false } });
  assert.equal(s.__el('p-text').textContent, FIXTURE[0].body);
  click(s, '[data-act]', { dataset: { act: 'copyprompt' } });
  await settle();
  assert.deepEqual(s.__copied, [FIXTURE[0].body]);
});
