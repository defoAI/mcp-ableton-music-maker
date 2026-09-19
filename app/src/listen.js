// The Listen screen: start the tap, draw what arrives, and say plainly what
// is happening when nothing does. Every state below is derived from what the
// Rust side reports, never from what was clicked.
(function () {
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args || {});
  const $ = (s) => document.querySelector(s);
  const esc = (v) => String(v == null ? '' : v).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const S = window.AMMSpectrum;

  const state = {
    onScreen: false,
    status: { listening: false, available: true },
    frame: null,
    drawn: null,
    float: false,
    visual: false,
    release: localStorage.getItem('listen.release') || 'fast',
    starting: false,
    error: null,
  };

  // ── what the producer is looking at ───────────────────────────────────────
  function phase() {
    const st = state.status;
    if (!st.available) return 'unsupported';
    if (!st.listening) return st.live_pid ? 'idle' : 'no_live';
    const f = state.frame;
    if (!f) return 'starting';
    if (!f.silent) return 'listening';
    if (f.silent_for_ms != null && f.silent_for_ms > 3000) {
      if (f.live_playing === 1) return 'no_audio';
      if (f.live_playing === 0) return 'quiet';
    }
    return 'listening';
  }

  const BAR = {
    unsupported: () => ['attn', '<span class="led warn"></span>Needs macOS 14.4 or later', 'Everything else in the app works here.', ''],
    no_live: () => ['attn', '<span class="led warn"></span>Live is not running', 'There is nothing to listen to until Live is open.', '<button class="btn small" disabled>Start listening</button>'],
    idle: () => ['', '<span class="led"></span>Not listening', `${esc(state.status.live_name || 'Ableton Live')} is running. Start to see its output here.`, '<button class="btn small primary" data-act="listen-start">Start listening</button>'],
    starting: () => ['', '<span class="led live"></span>Starting…', 'Asking macOS for Live’s audio.', '<button class="btn small" data-act="listen-stop">Stop</button>'],
    listening: () => ['', '<span class="led live"></span>Listening to Live', facts(), controls()],
    quiet: () => ['', '<span class="led live"></span>Listening to Live', 'Live is open and quiet: press play, or fire a section.', controls()],
    no_audio: () => ['down', '<span class="led bad"></span>Live is playing, but nothing arrives', 'macOS may not be letting this app hear other apps.', '<button class="btn small primary" data-act="listen-settings">Open System Settings</button><button class="btn small" data-act="listen-restart">Try again</button><button class="btn small" data-act="listen-stop">Stop</button>'],
  };

  function facts() {
    const st = state.status;
    const rate = st.sample_rate ? (st.sample_rate / 1000).toFixed(1) + ' kHz' : '—';
    const buf = st.buffer_frames ? `${st.buffer_frames}-sample buffer` : 'buffer unknown';
    return `pid ${st.live_pid} · ${rate} · ${buf} · meters ${Math.round(st.meter_lag_ms || 0)} ms behind, spectrum over ${Math.round(st.window_ms || 0)} ms`;
  }

  function controls() {
    return `<span class="seg">
        <button data-release="fast" aria-pressed="${state.release === 'fast'}">Fast</button>
        <button data-release="slow" aria-pressed="${state.release === 'slow'}">Slow</button>
      </span>
      <button class="btn small" data-act="listen-float">${state.float ? 'Close float' : 'Float'}</button>
      <button class="btn small" data-act="listen-visual">${state.visual ? 'Close visual' : 'Visual'}</button>
      <button class="btn small" data-act="listen-visual-full" title="The visual, full screen">Full screen</button>
      <button class="btn small" data-act="listen-stop">Stop</button>`;
  }

  const GATE = {
    unsupported: `<div class="gate"><h3>This screen needs macOS 14.4</h3>
      <p>Hearing another app’s audio without installing a virtual audio driver is something macOS added in 14.4. Installing the Remote Script, connecting Claude and the activity log all work as before.</p></div>`,
    no_live: `<div class="gate"><h3>Open Live first</h3>
      <p>The app listens to Live’s own process, so it can only start once Live is running. Everything on the Overview and Setup screens still works.</p></div>`,
    idle: `<div class="gate"><h3>See what Live is putting out</h3>
      <p>The app listens to the audio Live sends to your speakers, on this Mac, and draws it: a spectrum from 20 Hz to 20 kHz and the master level in dBFS. Live keeps playing through the output it has; nothing is routed and nothing is recorded. The sound is analysed in memory and thrown away, about thirty times a second.</p>
      <p class="note">The first time, macOS asks once whether this app may record audio from other apps. It does not give access to the microphone or the screen, and you can take it back in System Settings › Privacy &amp; Security › Screen &amp; System Audio Recording.</p>
      <div class="gate-actions"><button class="btn primary" data-act="listen-start">Start listening</button></div></div>`,
    no_audio: `<div class="gate"><h3>Live is playing, but nothing arrives</h3>
      <p>macOS does not tell an app whether it may hear other apps; it simply delivers silence. Live says its transport is running, so the likely cause is the permission. Open <b>System Settings › Privacy &amp; Security › Screen &amp; System Audio Recording</b>, switch on <b>Ableton Music Maker</b>, then try again.</p>
      <div class="gate-actions"><button class="btn primary" data-act="listen-settings">Open System Settings</button><button class="btn" data-act="listen-restart">Try again</button></div></div>`,
  };

  let lastPhase = null;
  // The state line is derived, so it has to be redrawn the moment the
  // derivation changes — when the first frame arrives, when silence passes
  // three seconds, when Live answers about its transport.
  function renderIfPhaseChanged() {
    if (phase() !== lastPhase) render();
  }

  function render() {
    if (!$('#listenbar')) return;
    const p = phase();
    lastPhase = p;
    const [cls, title, sub, controls] = (BAR[p] || BAR.idle)();
    $('#listenbar').className = 'listenbar ' + cls;
    $('#listenbar').innerHTML =
      `<span class="state">${title}</span><span class="facts">${sub}</span><span class="spacer"></span>${controls}` +
      (state.error ? `<span class="facts bad-text">${esc(state.error)}</span>` : '');
    const gate = GATE[p] || null;
    $('#listen-gate').hidden = !gate;
    $('#listen-gate').innerHTML = gate || '';
    $('#listen-scope').hidden = !!gate;
    const navLed = $('#listen-led');
    if (navLed) navLed.hidden = !state.status.listening;
  }

  // ── drawing ───────────────────────────────────────────────────────────────
  let raf = 0;
  function loop() {
    raf = 0;
    if (!state.onScreen || !state.status.listening) return;
    const f = state.frame;
    if (f) {
      state.drawn = S.smooth(state.drawn, f, state.release);
      S.drawSpectrum($('#spec'), state.drawn, { labels: true });
      S.drawMeter($('#meter'), f);
      readouts(f);
    }
    schedule();
  }
  function schedule() {
    if (!raf && state.onScreen && state.status.listening) raf = requestAnimationFrame(loop);
  }

  let lastReadout = 0;
  function readouts(f) {
    const now = performance.now();
    if (now - lastReadout < 120) return;
    lastReadout = now;
    const loud = S.loudestBand(state.drawn);
    $('#peakband').textContent = loud ? `${S.hz(loud.freq)} · ${S.db(loud.db)} dBFS` : '—';
    $('#pk').textContent = `${S.db(f.peak[0])} / ${S.db(f.peak[1])}`;
    $('#rms').textContent = S.db(Math.max(f.rms[0], f.rms[1]));
    $('#corr').textContent = (f.correlation >= 0 ? '+' : '−') + Math.abs(f.correlation).toFixed(2);
    $('#tph').textContent = S.db(f.peak_hold);
    $('#clipled').classList.toggle('on', !!f.clip);
    const ranges = (state.status.ranges || []);
    $('#words').innerHTML = f.ranges
      .map((v, i) => {
        const r = ranges[i] || {};
        return `<div class="word"><span class="k">${esc(r.name || '')}</span><span class="v num">${S.db(v)} dB</span><span class="d">${esc(hzRange(r))}</span></div>`;
      })
      .join('');
  }
  const hzRange = (r) => (r.lo == null ? '' : `${S.hz(r.lo)} – ${S.hz(r.hi)}`);

  // ── commands ──────────────────────────────────────────────────────────────
  async function refresh() {
    try { state.status = await invoke('listen_status'); } catch (e) { state.error = String(e); }
    render();
    schedule();
  }

  async function start() {
    if (state.starting) return;
    state.starting = true;
    state.error = null;
    state.frame = null;
    state.drawn = null;
    render();
    try {
      state.status = await invoke('listen_start');
    } catch (e) {
      state.error = String(e).replace(/^Error:\s*/, '');
      await refresh();
      state.starting = false;
      return;
    }
    state.starting = false;
    render();
    schedule();
  }

  async function stop() {
    try { await invoke('listen_stop'); } catch (e) { /* stopping never fails loudly */ }
    state.frame = null;
    state.drawn = null;
    await refresh();
  }

  async function toggleVisual(fullscreen) {
    const open = fullscreen ? true : !state.visual;
    try { await invoke('listen_visual', { open, fullscreen: fullscreen ? true : null }); state.visual = open; } catch (e) { state.error = String(e); }
    render();
  }

  async function toggleFloat() {
    const open = !state.float;
    try { await invoke('listen_float', { open }); state.float = open; } catch (e) { state.error = String(e); }
    render();
  }

  // ── wiring ────────────────────────────────────────────────────────────────
  document.addEventListener('click', (e) => {
    const rel = e.target.closest('[data-release]');
    if (rel) {
      state.release = rel.dataset.release;
      localStorage.setItem('listen.release', state.release);
      render();
      return;
    }
    const act = e.target.closest('[data-act]');
    if (!act) return;
    switch (act.dataset.act) {
      case 'listen-start': start(); break;
      case 'listen-stop': stop(); break;
      case 'listen-restart': start(); break;
      case 'listen-float': toggleFloat(); break;
      case 'listen-visual': toggleVisual(false); break;
      case 'listen-visual-full': toggleVisual(true); break;
      case 'listen-settings':
        invoke('open_external', { target: 'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture' });
        break;
      default: break;
    }
  });

  window.__TAURI__.event.listen('listen:frame', (e) => {
    state.frame = e.payload;
    if (!state.status.listening) state.status.listening = true;
    renderIfPhaseChanged();
    schedule();
  });
  window.__TAURI__.event.listen('listen:stopped', (e) => {
    state.error = (e.payload && e.payload.message) || null;
  });
  window.__TAURI__.event.listen('listen:ended', () => {
    state.status.listening = false;
    state.frame = null;
    refresh();
  });

  // A window closed from its own title bar tells the buttons here.
  window.__TAURI__.event.listen('listen:window-closed', (e) => {
    const label = e.payload && e.payload.label;
    if (label === 'listen-float') state.float = false;
    if (label === 'listen-visual') state.visual = false;
    render();
  });

  window.__listen = {
    onScreen(screen) {
      state.onScreen = screen === 'listen';
      if (state.onScreen) { refresh(); }
      else { render(); }
    },
    refresh,
  };

  refresh();
  setInterval(() => { if (state.onScreen) refresh(); }, 4000);
})();
