// The Feedback screen: the song as a strip under its loudness, comments
// pinned to a bar, Claude's questions with ▶ and This one, and the switch
// that lets Claude play the set while you are away. Everything shown comes
// from the Rust side (`feedback_state`); nothing here decides what is true.
(function () {
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args || {});
  const $ = (s) => document.querySelector(s);
  const esc = (v) => String(v == null ? '' : v).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  // ── the pure part: what goes where ────────────────────────────────────────

  // A section never heard still takes room on the strip, so the song reads
  // in order; this is how many bars wide it is drawn.
  const UNHEARD_BARS = 4;
  const LUFS_TOP = 0;
  const LUFS_BOTTOM = -40;

  /** Sections laid out left to right across `width`, each bar a slice. */
  function layout(strip, width) {
    const sections = (strip && strip.sections) || [];
    const widthOf = (s) => Math.max(s.bars.length, s.bars.length ? 1 : UNHEARD_BARS);
    const total = sections.reduce((a, s) => a + widthOf(s), 0) || 1;
    const per = width / total;
    let x = 0;
    return sections.map((s) => {
      const w = widthOf(s) * per;
      const bars = s.bars.map((b, i) => ({ bar: b.bar, lufs: b.lufs, peak: b.peak, x0: x + i * per, x1: x + (i + 1) * per }));
      const out = { name: s.name, status: s.status, played: s.played, x0: x, x1: x + w, bars };
      x += w;
      return out;
    });
  }

  /** The bar (and its section) under `x`; a never-heard section has no bar. */
  function barAt(lay, x) {
    for (const s of lay) {
      if (x < s.x0 || x >= s.x1) continue;
      const b = s.bars.find((b) => x >= b.x0 && x < b.x1);
      return { section: s.name, bar: b ? b.bar : null, status: s.status };
    }
    return null;
  }

  function lufsY(lufs, h) {
    const v = Math.max(LUFS_BOTTOM, Math.min(LUFS_TOP, lufs));
    return ((LUFS_TOP - v) / (LUFS_TOP - LUFS_BOTTOM)) * h;
  }

  const hhmm = (ms) => {
    const d = new Date(ms);
    return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
  };

  /** What the away switch says, in words. */
  function awayText(away, onNow, now) {
    if (onNow && away && away.until_ms) {
      const left = Math.max(0, Math.round((away.until_ms - now) / 60000));
      return `On until ${hhmm(away.until_ms)} · ${left} min left · turns off when you press play`;
    }
    const why = away && away.off_reason;
    if (why === 'played') return 'Off — you pressed play in Live';
    if (why === 'time') return 'Off — its time ran out';
    return 'Off — Claude never plays the set to measure it';
  }

  const STATUS = {
    current: 'heard since the last change',
    stale: 'heard before the last change',
    never: 'not heard yet',
  };

  function questionHtml(q) {
    const picked = q.picked;
    const rows = q.options.map((o) => {
      const chosen = picked && picked.option === o.n;
      return `<div class="opt${chosen ? ' chosen' : ''}">
        <button class="btn small" data-act="fb-play" data-offer="${esc(q.id)}" data-n="${o.n}" title="Play ${esc(o.hear)}">▶</button>
        <span class="opt-n num">${o.n}</span>
        <span class="opt-name">${esc(o.name)}</span>
        <span class="opt-what">${esc(o.what)} <span class="opt-hear">${esc(o.hear)}</span></span>
        ${chosen
          ? `<span class="pill off">Picked${picked.note ? ' · ' + esc(picked.note) : ''}</span>`
          : `<button class="btn small primary" data-act="fb-pick" data-offer="${esc(q.id)}" data-n="${o.n}">This one</button>`}
      </div>`;
    }).join('');
    return `<div class="question" data-id="${esc(q.id)}">
      <div class="q-head"><b>${esc(q.question)}</b>${picked ? '' : ' <span class="note">waiting for your pick</span>'}</div>
      ${rows}
    </div>`;
  }

  function threadHtml(t) {
    const c = t.comment;
    const where = c.bar != null ? `bar ${c.bar}${c.section ? ' · ' + esc(c.section) : ''}` : 'no bar';
    const replies = (t.replies || []).map((r) => `<div class="reply"><span class="who">Claude</span> ${esc(r.text)}</div>`).join('');
    return `<div class="thread">
      <div class="comment"><span class="where num">${where}</span> ${esc(c.text)}</div>
      ${replies || '<div class="reply waiting note">Claude has not answered yet — it reads comments on its next get_feedback.</div>'}
    </div>`;
  }

  window.AMMFeedback = { layout, barAt, lufsY, awayText, questionHtml, threadHtml, UNHEARD_BARS, STATUS };

  // ── the screen ────────────────────────────────────────────────────────────

  const state = { onScreen: false, data: null, lay: [], pending: null, error: null };

  function cssVar(name, fallback) {
    try { return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback; } catch (e) { return fallback; }
  }

  function drawStrip() {
    const c = $('#fb-strip');
    if (!c || !c.getContext) return;
    const rect = c.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    c.width = Math.max(1, Math.round(rect.width * dpr));
    c.height = Math.max(1, Math.round(rect.height * dpr));
    const ctx = c.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const w = rect.width;
    const h = rect.height;
    const curveH = h - 18;
    state.lay = layout(state.data && state.data.strip, w);
    ctx.fillStyle = cssVar('--panel', '#F7F6F3');
    ctx.fillRect(0, 0, w, h);
    const colors = {
      current: cssVar('--accent', '#0A7AFF'),
      stale: cssVar('--warn', '#F08A00'),
      never: cssVar('--led-off', '#C9C6BF'),
    };
    for (const s of state.lay) {
      ctx.globalAlpha = s.status === 'never' ? 0.35 : 1;
      ctx.fillStyle = colors[s.status] || colors.never;
      for (const b of s.bars) {
        const y = lufsY(b.lufs, curveH);
        ctx.fillRect(b.x0 + 0.5, y, Math.max(1, b.x1 - b.x0 - 1), curveH - y);
      }
      if (!s.bars.length) {
        ctx.fillRect(s.x0 + 1, curveH - 3, s.x1 - s.x0 - 2, 3);
      }
      ctx.globalAlpha = 1;
      ctx.fillStyle = cssVar('--ink-2', '#5C5A55');
      ctx.font = '11px -apple-system, sans-serif';
      ctx.fillText(s.name + (s.status === 'stale' ? ' •' : ''), s.x0 + 4, h - 4);
      ctx.fillStyle = cssVar('--line-strong', '#CFCCC4');
      ctx.fillRect(s.x0, 0, 1, h);
    }
    if (state.pending && state.pending.bar != null) {
      for (const s of state.lay) {
        const b = s.bars.find((b) => b.bar === state.pending.bar);
        if (b) {
          ctx.fillStyle = cssVar('--ink', '#1D1C1A');
          ctx.fillRect(b.x0, 0, Math.max(2, b.x1 - b.x0), 2);
        }
      }
    }
  }

  function render() {
    const d = state.data || {};
    const scr = d.screen || {};
    const pos = d.position;
    const away = scr.away || d.away;
    const awayOn = scr.away_on ?? d.away_on;
    $('#fb-sub').textContent = d.error
      ? d.error
      : `${d.set_name || 'This set'} · revision ${d.strip ? d.strip.revision : 0} · ${d.strip ? d.strip.takes : 0} take${d.strip && d.strip.takes === 1 ? '' : 's'} heard`;
    $('#fb-bar').innerHTML = `
      <span class="l">Claude may play the set</span>
      <button class="switch" id="sw-away" role="switch" aria-checked="${awayOn ? 'true' : 'false'}"></button>
      <span class="facts">${esc(awayText(away, awayOn, Date.now()))}</span>
      <span class="spacer"></span>
      <span class="facts">${pos ? `${pos.playing ? 'playing' : 'stopped'} at bar ${pos.bar}${pos.section ? ' · ' + esc(pos.section) : ''}` : ''}</span>
      <button class="btn small" data-act="fb-float">Note float</button>`;
    const lay = layout(d.strip, 100);
    $('#fb-legend').innerHTML = lay.length
      ? ['current', 'stale', 'never'].map((k) => `<span class="lg lg-${k}">${STATUS[k]}</span>`).join('')
      : '<span class="note">Nothing heard yet. Turn on Settings › Remember what Live plays, then play the set.</span>';
    const p = state.pending;
    $('#fb-at').textContent = p && p.bar != null ? `bar ${p.bar}${p.section ? ' · ' + p.section : ''}` : (pos ? `bar ${pos.bar} (now)` : 'no bar');
    const qs = scr.questions || [];
    $('#fb-questions').innerHTML = qs.length ? qs.map(questionHtml).join('') : '<p class="empty">No questions. When Claude has options for you, they appear here, each with ▶ to hear it.</p>';
    const ts = scr.threads || [];
    $('#fb-threads').innerHTML = ts.length ? ts.map(threadHtml).join('') : '<p class="empty">No comments yet. Click a bar on the strip, or press M while Live plays.</p>';
    const waiting = qs.filter((q) => !q.picked).length;
    const badge = $('#fbbadge');
    if (badge) { badge.hidden = !waiting; badge.textContent = waiting; }
    drawStrip();
  }

  async function refresh() {
    try { state.data = await invoke('feedback_state'); } catch (e) { state.data = { error: String(e) }; }
    render();
  }

  async function send() {
    const input = $('#fb-text');
    const text = (input.value || '').trim();
    if (!text) return;
    const bar = state.pending && state.pending.bar != null ? state.pending.bar : null;
    try {
      await invoke('feedback_comment', { text, bar });
      input.value = '';
      state.pending = null;
    } catch (e) { state.error = String(e); }
    await refresh();
  }

  /** M while Live plays: stamp the bar playing now and type. */
  function stampNow() {
    const pos = state.data && state.data.position;
    state.pending = pos ? { bar: pos.bar, section: pos.section } : null;
    render();
    const input = $('#fb-text');
    if (input && input.focus) input.focus();
  }

  document.addEventListener('click', (e) => {
    if (!state.onScreen) return;
    if (e.target.closest && e.target.closest('#fb-strip')) {
      const c = $('#fb-strip');
      const x = e.clientX - c.getBoundingClientRect().left;
      const hit = barAt(state.lay, x);
      if (hit && hit.bar != null) { state.pending = hit; render(); const i = $('#fb-text'); if (i && i.focus) i.focus(); }
      return;
    }
    const sw = e.target.closest && e.target.closest('#sw-away');
    if (sw) {
      const on = sw.getAttribute('aria-checked') !== 'true';
      invoke('feedback_away', { on }).then(refresh, (err) => { state.error = String(err); render(); });
      return;
    }
    const act = e.target.closest && e.target.closest('[data-act]');
    if (!act) return;
    switch (act.dataset.act) {
      case 'fb-send': send(); break;
      case 'fb-float': invoke('feedback_float', { open: true }); break;
      case 'fb-play': invoke('feedback_play', { offer: act.dataset.offer, option: +act.dataset.n }).catch((err) => { state.error = String(err); render(); }); break;
      case 'fb-pick': {
        const note = window.prompt ? window.prompt('A note with your pick (optional)', '') : '';
        if (note === null) return;
        invoke('feedback_pick', { offer: act.dataset.offer, option: +act.dataset.n, note }).then(refresh, (err) => { state.error = String(err); render(); });
        break;
      }
      default: break;
    }
  });

  document.addEventListener('keydown', (e) => {
    if (!state.onScreen) return;
    const typing = e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA');
    if (typing && e.key === 'Enter') { e.preventDefault && e.preventDefault(); send(); return; }
    if (!typing && (e.key === 'm' || e.key === 'M') && !e.metaKey && !e.ctrlKey) { e.preventDefault && e.preventDefault(); stampNow(); }
  });

  window.__TAURI__.event.listen('feedback:changed', () => { if (state.onScreen) refresh(); });

  window.__feedback = {
    onScreen(screen) {
      state.onScreen = screen === 'feedback';
      if (state.onScreen) refresh();
    },
    refresh,
    state,
    stampNow,
    send,
  };

  setInterval(() => { if (state.onScreen) refresh(); }, 3000);
})();
