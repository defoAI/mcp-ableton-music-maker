// The UI reads what the app's Rust side reports; every number on screen comes
// from a file the server wrote or from one check against Live.
(function () {
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args || {});
  const $ = (s) => document.querySelector(s);
  const esc = (v) => String(v == null ? '' : v).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const fmt = (n) => Number(n || 0).toLocaleString('en-US');
  const tok = (chars) => Math.ceil((chars || 0) / 4);
  const ms = (v) => (v == null ? '—' : v >= 1000 ? (v / 1000).toFixed(1) + ' s' : Math.round(v) + ' ms');
  const hhmm = (iso) => { try { const d = new Date(iso); return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }); } catch { return ''; } };
  const median = (arr) => { if (!arr.length) return null; const s = [...arr].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };

  const state = {
    screen: 'overview',
    status: null,
    settings: null,
    sessions: [],
    sessionId: null,
    lines: [],
    filter: 'all',
    selected: null,
    client: 'desktop',
    clientInfo: null,
    prompts: null,
    prompt: 'song',
    setup: { installing: false, checking: false, check: null, testing: false, test: null, installMsg: null },
  };

  // ── derived facts about a line ────────────────────────────────────────────
  const WRITE_PREFIX = /^(set_|create_|add_|clear_|delete_|fire_|stop_|start_|load_|switch_|duplicate_)/;
  const kindOf = (l) => ((l.commands || []).some((c) => WRITE_PREFIX.test(c)) ? 'write' : 'read');
  const SUMMARY = {
    get_session_info: () => 'Read the session',
    get_session_snapshot: () => 'Snapshot of every track, clip and device',
    set_tempo: (p) => (p.tempo != null ? `Tempo → ${p.tempo}` : 'Set the tempo'),
    start_playback: () => 'Play',
    stop_playback: () => 'Stop',
    get_track_info: (p) => (p.track_index != null ? `Read track ${p.track_index}` : 'Read a track'),
    create_midi_track: () => 'New MIDI track',
    create_audio_track: () => 'New audio track',
    set_track_name: (p) => (p.name ? `Renamed track ${p.track_index} to “${p.name}”` : 'Renamed a track'),
    create_clip: (p) => (p.track_index != null ? `New clip in track ${p.track_index}, slot ${p.clip_index}` : 'New clip'),
    create_audio_clip: () => 'Imported an audio clip',
    get_clip_notes: (p) => (p.track_index != null ? `Read notes of track ${p.track_index}, slot ${p.clip_index}` : 'Read clip notes'),
    add_notes_to_clip: (p) => (p.notes ? `${p.notes.length} notes into track ${p.track_index}, slot ${p.clip_index}` : 'Added notes to a clip'),
    clear_notes_from_clip: () => 'Cleared a clip',
    set_clip_name: (p) => (p.name ? `Clip renamed to “${p.name}”` : 'Renamed a clip'),
    set_arrangement_clip_name: () => 'Renamed an Arrangement clip',
    delete_clip: () => 'Deleted a clip',
    fire_clip: (p) => (p.track_index != null ? `Fired track ${p.track_index}, slot ${p.clip_index}` : 'Fired a clip'),
    stop_clip: () => 'Stopped a clip',
    get_device_parameters: (p) => (p.track_index != null ? `Read device ${p.device_index} on track ${p.track_index}` : 'Read device parameters'),
    set_device_parameter: (p) => (p.value != null ? `Parameter ${p.parameter_index} → ${p.value}` : 'Changed a device parameter'),
    load_instrument_or_effect: (p) => (p.uri ? `Loaded ${p.uri.split(':').pop()}` : 'Loaded an instrument or effect'),
    load_drum_kit: () => 'Loaded a drum kit',
    get_browser_tree: () => 'Listed the browser',
    get_browser_items_at_path: (p) => (p.path ? `Listed ${p.path}` : 'Listed browser items'),
    switch_to_arrangement_view: () => 'Switched to Arrangement view',
    set_arrangement_time: (p) => (p.time != null ? `Moved to beat ${p.time}` : 'Moved the playhead'),
    get_arrangement_clips: () => 'Listed Arrangement clips',
    duplicate_to_arrangement: (p) => (p.destination_time != null ? `Clip → Arrangement at beat ${p.destination_time}` : 'Copied a clip to the Arrangement'),
    create_locator: (p) => (p.name ? `Locator “${p.name}”` : 'Added a locator'),
    get_remote_script_info: () => 'Checked the Remote Script',
  };
  const summarize = (l) => {
    const f = SUMMARY[l.tool];
    const text = f ? f(l.params || {}) : (l.tool || '').replace(/_/g, ' ');
    return l.ok === false ? text + ' — failed' : text;
  };

  // ── rendering ─────────────────────────────────────────────────────────────
  function rowHtml(l, i) {
    const res = l.ok ? '<span class="res"><span class="led good"></span>ok</span>' : '<span class="res"><span class="led bad"></span>error</span>';
    return `<tr data-i="${i}" aria-selected="${state.selected === i}">
      <td class="mono num">${esc(hhmm(l.ts))}</td>
      <td><span class="kind ${kindOf(l)}"></span><span class="tool">${esc(l.tool)}</span></td>
      <td>${esc(summarize(l))}</td>
      <td class="r mono num">${ms(l.live_ms)}</td>
      <td>${res}</td>
      <td class="r mono num tok">${tok(l.in_chars)} → <b>${fmt(tok(l.out_chars))}</b></td>
    </tr>`;
  }

  function renderOverview() {
    const st = state.status;
    if (!st) return;
    const sessions = st.sessions || [];
    const live = st.live || {};
    const cell = (who, led, text, extra, cls) => `
      <div class="cell ${cls || ''}">
        <div class="who"><span class="led ${led}"></span>${who}</div>
        <div class="state">${text}</div>
        ${extra || ''}
      </div>`;
    const fix = (label) => `<div class="fix"><button class="btn small primary" data-go="setup">${label}</button></div>`;

    let clientCell, serverCell, liveCell;
    if (sessions.length === 0) {
      clientCell = st.clients?.desktop?.configured
        ? cell('Your client', 'off', 'Claude Desktop is set up · no session open right now', '', '')
        : cell('Your client', 'off', 'No client has connected yet', fix('Connect a client'), 'attn');
      serverCell = st.sidecar_present
        ? cell('<span class="mono">ableton-music-maker</span>', 'off', `Bundled ${esc(st.server_version)} · starts when a client does`)
        : cell('<span class="mono">ableton-music-maker</span>', 'bad', 'Server binary missing next to the app — run <span class="mono">scripts/build-sidecar.sh</span>', '', 'down');
    } else {
      // Which servers have actually driven Live: their activity file has calls.
      const busy = sessions.filter((s) => state.sessions.some((a) => a.alive && a.calls > 0 && a.file === s.activity_file));
      const b = busy[0] || sessions[0];
      const name = b.client?.name || 'A client';
      const names = esc(sessions.map((s) => s.client?.name || '?').join(', '));
      if (busy.length > 1) clientCell = cell('Clients', 'warn', `${busy.length} clients are driving Live at the same time: ${names}. Only one should.`, '', 'attn');
      else if (sessions.length > 1) clientCell = cell(esc(name), 'good', `Connected · since ${esc(hhmm(b.started))}. ${sessions.length} copies of the server are running (${names}); Claude Desktop starts one per mode, and idle copies are harmless.`);
      else clientCell = cell(esc(name), 'good', `Connected${b.client?.version ? ' · ' + esc(b.client.version) : ''} · since ${esc(hhmm(b.started))}`);
      serverCell = cell('<span class="mono">ableton-music-maker</span>', 'live', `Running ${esc(b.server_version)} · started by ${esc(name)} at ${esc(hhmm(b.started))} · pid ${b.pid}`);
    }
    if (live.live_reachable) {
      if (live.up_to_date) {
        const s = live.session || {};
        const detail = s.tempo != null ? ` · ${s.tempo} BPM · ${s.track_count} tracks` : '';
        liveCell = cell('Ableton Live', 'good', `Live answered · AbletonMusicMaker ${esc(live.script_version)} · ${live.capabilities} commands${esc(detail)}`);
      } else {
        liveCell = cell('Ableton Live', 'warn', `Remote Script ${esc(live.script_version)} loaded — this app ships ${esc(live.expected_version)}. Tools that need the newer script will refuse until it is updated.`, fix('Update Remote Script'), 'attn');
      }
    } else {
      liveCell = cell('Ableton Live', 'bad', `Not reachable at ${esc(live.host)}:${esc(live.port)} — Live is not running, or AbletonMusicMaker is not selected as a Control Surface.`, fix('How to select it'), 'down');
    }
    $('#chain').innerHTML = clientCell + '<div class="link">→</div>' + serverCell + '<div class="link">→</div>' + liveCell;
    $('#ovsub').textContent = sessions.length ? `${sessions[0].client?.name || 'Client'} session` : (live.live_reachable ? 'Live is ready' : 'Waiting for Live');
    $('#setupbadge').hidden = !!(live.up_to_date && (sessions.length || st.clients?.desktop?.configured));

    // Tiles and recent calls come from the newest session's lines.
    const lines = state.lines;
    const errors = lines.filter((l) => !l.ok).length;
    const tin = lines.reduce((a, l) => a + tok(l.in_chars), 0);
    const tout = lines.reduce((a, l) => a + tok(l.out_chars), 0);
    const lives = lines.map((l) => l.live_ms).filter((v) => v != null);
    const slowest = lines.reduce((m, l) => (l.live_ms > (m?.live_ms || -1) ? l : m), null);
    const writes = lines.filter((l) => kindOf(l) === 'write').length;
    $('#tiles-title').textContent = state.sessionId ? `Session ${state.sessionLabel || ''}` : 'This session';
    $('#tiles').innerHTML = `
      <div class="tile"><span class="k">Calls</span><span class="v num">${lines.length}</span><span class="d">${errors} error${errors === 1 ? '' : 's'}</span></div>
      <div class="tile hot"><span class="k">Tokens, est.</span><span class="v num">${fmt(tin + tout)}</span><span class="d num">${fmt(tin)} in · ${fmt(tout)} out</span></div>
      <div class="tile"><span class="k">Live round-trip</span><span class="v num">${lives.length ? ms(median(lives)) : '—'}</span><span class="d">${slowest ? 'median · slowest ' + ms(slowest.live_ms) + ' (' + esc(slowest.tool) + ')' : 'median'}</span></div>
      <div class="tile"><span class="k">Changes to the set</span><span class="v num">${writes}</span><span class="d">tracks, clips, notes, devices</span></div>`;
    $('#recent').innerHTML = lines.length
      ? lines.slice(-5).reverse().map((l) => rowHtml(l, lines.indexOf(l))).join('')
      : '<tr><td colspan="6" class="empty">Calls appear here as soon as a client talks to Live.</td></tr>';
    $('#actbadge').hidden = !errors;
    $('#actbadge').textContent = errors;
  }

  function renderActivity() {
    const sel = $('#session');
    sel.innerHTML = state.sessions.length
      ? state.sessions.map((s) => `<option value="${esc(s.id)}" ${s.id === state.sessionId ? 'selected' : ''}>${esc(sessionLabel(s))}</option>`).join('')
      : '<option value="">No sessions yet</option>';
    const list = state.lines.map((l, i) => [l, i]).filter(([l]) => state.filter === 'all' || (state.filter === 'error' ? !l.ok : kindOf(l) === state.filter));
    $('#rows').innerHTML = list.length
      ? list.reverse().map(([l, i]) => rowHtml(l, i)).join('')
      : `<tr><td colspan="6" class="empty">${state.sessions.length ? 'Nothing matches.' : 'No calls recorded yet. Ask Claude to do something in Live.'}</td></tr>`;
    renderDetail();
  }

  function sessionLabel(s) {
    const who = s.client?.name || (s.alive ? 'Client' : 'Earlier session');
    const when = s.started ? new Date(s.started) : null;
    const day = when ? (when.toDateString() === new Date().toDateString() ? 'today' : when.toLocaleDateString()) : '';
    return `${who} · ${day} ${when ? when.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : ''} · ${s.calls} call${s.calls === 1 ? '' : 's'}${s.alive ? ' · live' : ''}`;
  }

  function renderDetail() {
    const d = $('#detail');
    const l = state.lines[state.selected];
    if (state.selected == null || !l) { d.hidden = true; return; }
    d.hidden = false;
    const pre = (v) => `<pre>${esc(typeof v === 'string' ? v : JSON.stringify(v, null, 2))}</pre>`;
    const missing = '<pre><span class="redacted">Payloads are not kept. Turn on “Keep the payloads too” in Settings to see parameters and results here. Sizes are always recorded.</span></pre>';
    d.innerHTML = `
      <div class="dh"><div><span class="tool" style="font-size:14px">${esc(l.tool)}</span> <span class="note">· ${esc(hhmm(l.ts))} · ${kindOf(l) === 'write' ? 'changes the set' : 'read only'} · sent ${esc((l.commands || []).join(', ') || 'nothing to Live')}</span></div>
        <button class="btn small" id="closedetail">Close</button></div>
      <div class="kv">
        <div><span class="k">Whole call</span><span class="mono num">${ms(l.duration_ms)}</span></div>
        <div><span class="k">Inside Live</span><span class="mono num">${ms(l.live_ms)}</span></div>
        <div><span class="k">Received</span><span class="mono num">${fmt(l.in_chars)} chars ≈ ${tok(l.in_chars)} tokens</span></div>
        <div><span class="k">Returned</span><span class="mono num">${fmt(l.out_chars)} chars ≈ ${fmt(tok(l.out_chars))} tokens</span></div>
      </div>
      ${l.ok ? '' : `<div class="callout bad"><b>Error returned to Claude:</b> ${esc(l.error)}</div>`}
      <div><div class="eyebrow">Parameters</div>${l.params !== undefined ? pre(l.params) : missing}</div>
      ${l.ok ? `<div><div class="eyebrow">Result</div>${l.result !== undefined ? pre(l.result) : `<pre><span class="redacted">${fmt(l.out_chars)} chars, not kept</span></pre>`}</div>` : ''}`;
    $('#closedetail').onclick = () => { state.selected = null; renderActivity(); };
  }

  function renderSetup() {
    const st = state.status || {};
    const script = st.script || { targets: [] };
    const target = script.targets[0];
    const live = st.live || {};
    const su = state.setup;
    const steps = [];

    // 1 Remote Script
    const scriptDone = !!target?.up_to_date;
    let scriptLine;
    if (!target) scriptLine = 'No Ableton User Library was found on this Mac. Choose it, or open Live once so it creates one.';
    else if (su.installing) scriptLine = 'Installing…';
    else if (target.up_to_date) scriptLine = `Installed · <span class="mono">AbletonMusicMaker ${esc(target.installed)}</span>`;
    else if (target.installed) scriptLine = `Found <span class="mono">${esc(target.installed)}</span>, this app ships <span class="mono">${esc(script.expected)}</span>. The old file is kept as <span class="path">__init__.py.bak</span>.`;
    else scriptLine = 'Not found. The app will copy one folder into your library.';
    steps.push(`<div class="step ${scriptDone ? 'done' : 'current'}"><span class="n">${scriptDone ? '✓' : '1'}</span><span class="t">Remote Script in Live’s User Library</span>
      <div class="body">
        <div>${scriptLine}</div>
        ${target ? `<div class="path">${esc(target.script)}</div>` : ''}
        ${su.installMsg ? `<div class="${su.installMsg.ok ? 'okline' : 'callout bad'}">${esc(su.installMsg.text)}</div>` : ''}
        <div class="actions">
          ${su.installing ? '' : `<button class="btn small ${scriptDone ? '' : 'primary'}" data-act="install" ${target ? '' : 'disabled'}>${scriptDone ? 'Reinstall' : target?.installed ? 'Update Remote Script' : 'Install into Live'}</button>`}
          <button class="btn small" data-act="changelib">Change library…</button>
        </div>
      </div></div>`);

    // 2 Select in Live
    const selDone = !!(live.live_reachable && live.up_to_date);
    let checkLine = '';
    if (su.checking) checkLine = '<div>Asking Live…</div>';
    else if (su.check) {
      const c = su.check;
      if (c.live_reachable && c.up_to_date) checkLine = `<div class="okline">Live answered: AbletonMusicMaker ${esc(c.script_version)}, ${c.capabilities} commands.</div>`;
      else if (c.live_reachable) checkLine = `<div class="callout">Live answered with Remote Script ${esc(c.script_version)}; this app ships ${esc(c.expected_version)}. Update it in step 1, then restart Live.</div>`;
      else checkLine = `<div class="callout bad">No answer on ${esc(c.host)}:${esc(c.port)}. Live is not running, or the control surface is not selected yet. Live has to be restarted after the install.</div>`;
    } else if (selDone) checkLine = `<div class="okline">Live answered: AbletonMusicMaker ${esc(live.script_version)}, ${live.capabilities} commands.</div>`;
    steps.push(`<div class="step ${selDone ? 'done' : scriptDone ? 'current' : ''}"><span class="n">${selDone ? '✓' : '2'}</span><span class="t">Select it in Live</span>
      <div class="body">
        <div>Restart Live, then open <b>Settings › Link, Tempo &amp; MIDI</b>. In a <b>Control Surface</b> slot choose <b>AbletonMusicMaker</b>; set Input and Output to <b>None</b>.</div>
        ${checkLine}
        <div class="actions"><button class="btn small ${selDone ? '' : 'primary'}" data-act="check">${selDone ? 'Check again' : 'Check'}</button></div>
      </div></div>`);

    // 3 Client
    const cliDone = !!st.clients?.desktop?.configured;
    const ci = state.clientInfo;
    let cfg = '<div>Loading…</div>';
    if (ci && ci.kind === state.client) {
      if (ci.kind === 'desktop') cfg = `<div>Adds this entry to <span class="path">${esc(ci.path)}</span> and keeps a copy of the file first.</div>
<pre>${esc(JSON.stringify(ci.entry, null, 2))}</pre>
<div class="actions"><button class="btn small ${ci.configured ? '' : 'primary'}" data-act="addclient">${ci.configured ? 'Added — write again' : 'Add to Claude Desktop'}</button>${ci.configured ? '<span class="okline">Set up. Restart Claude Desktop if it is open.</span>' : ''}</div>`;
      else if (ci.kind === 'code') cfg = `<div>Run this once in a terminal:</div><pre>${esc(ci.command)}</pre><div class="actions"><button class="btn small primary" data-act="copy" data-copy="${esc(ci.command)}">Copy command</button></div>`;
      else cfg = `<div>In Cursor, open <b>Settings › MCP</b>, add a server and paste this as the command:</div><pre>${esc(ci.command)}</pre><div class="actions"><button class="btn small primary" data-act="copy" data-copy="${esc(ci.command)}">Copy path</button></div>`;
    }
    steps.push(`<div class="step ${cliDone ? 'done' : selDone ? 'current' : ''}"><span class="n">${cliDone ? '✓' : '3'}</span><span class="t">Connect a client</span>
      <div class="body">
        <div class="seg">
          <button data-client="desktop" aria-pressed="${state.client === 'desktop'}">Claude Desktop</button>
          <button data-client="code" aria-pressed="${state.client === 'code'}">Claude Code</button>
          <button data-client="cursor" aria-pressed="${state.client === 'cursor'}">Cursor</button>
        </div>
        ${cfg}
        ${st.clients?.desktop?.legacy_entry ? '<div class="callout"><b>The original AbletonMCP entry is still in Claude Desktop’s config.</b> It talks to the same Remote Script, so Claude sees two overlapping tool sets. <button class="btn small" data-act="removelegacy">Remove the old entry</button> (a backup of the file is kept)</div>' : ''}
        <div class="note">Only one client should talk to Live at a time. The app warns when two are running.</div>
      </div></div>`);

    // 4 Test
    const t = su.test;
    const testDone = !!(t && t.live_reachable && t.up_to_date);
    let testLine = '';
    if (su.testing) testLine = '<div>Running…</div>';
    else if (t) testLine = testDone
      ? `<div class="okline">Live answered${t.session?.tempo != null ? ` in a session at ${t.session.tempo} BPM with ${t.session.track_count} tracks` : ''}. Ask Claude to build something.</div>`
      : `<div class="callout bad">${t.live_reachable ? 'Live answered but the Remote Script is not up to date.' : 'Live did not answer.'} ${esc(t.error || '')}</div>`;
    steps.push(`<div class="step ${testDone ? 'done' : cliDone ? 'current' : ''}"><span class="n">${testDone ? '✓' : '4'}</span><span class="t">Test the chain</span>
      <div class="body">
        <div>The app asks Live for the loaded Remote Script and the current session, exactly as the server does on startup. Nothing is changed in your set.</div>
        ${testLine}
        <div class="actions"><button class="btn small ${testDone ? '' : 'primary'}" data-act="test">${testDone ? 'Run again' : 'Run a test call'}</button></div>
      </div></div>`);
    $('#steps').innerHTML = steps.join('');
  }

  // ── Prompts ───────────────────────────────────────────────────────────────
  // Four finished prompts, read out of prompts/*.md by the Rust side. The UI
  // holds no copy of the text: `state.prompts` is whatever the command
  // returned, and the copy button copies the body of the selected one.
  const selectedPrompt = () => (state.prompts || []).find((p) => p.id === state.prompt) || (state.prompts || [])[0] || null;

  // What a pasted prompt would hit first, if it would not work. Nothing here
  // probes anything: it reads the chain the Overview already computed.
  function promptBanner() {
    const st = state.status;
    if (!st) return null;
    const configured = !!st.clients?.desktop?.configured || (st.sessions || []).length > 0;
    if (!configured) {
      return { text: '<b>No client is configured yet.</b> Your client starts the server, so a pasted prompt has nothing to run it. Copying still works if you are getting ready.', action: 'Finish setup' };
    }
    if (!st.live?.live_reachable) {
      return { text: '<b>Live is not open.</b> Every prompt here starts with <span class="mono">get_context</span>, which needs a running set. Open Live with AbletonMusicMaker selected as a Control Surface, then paste.', action: 'How to select it' };
    }
    return null;
  }

  function renderPrompts() {
    const list = state.prompts;
    const banner = $('#pbanner');
    const b = promptBanner();
    banner.hidden = !b;
    banner.innerHTML = b ? `<span>${b.text}</span><button class="btn small" data-go="setup">${esc(b.action)}</button>` : '';
    if (!list) {
      $('#prail').innerHTML = '<div class="empty">Loading…</div>';
      return;
    }
    $('#prail').innerHTML = list.map((p) => `
      <button class="pcard" type="button" data-pick="${esc(p.id)}" aria-pressed="${p.id === state.prompt}">
        <span class="t">${esc(p.title)}</span>
        <span class="d">${esc(p.card)}</span>
        <span class="m">${esc(p.meta)}</span>
      </button>`).join('');
    const p = selectedPrompt();
    if (!p) return;
    $('#p-title').textContent = p.title;
    $('#p-why').textContent = p.when;
    $('#p-text').textContent = p.body;
    $('#p-path').textContent = p.file;
    $('#p-len').textContent = `${fmt(p.words)} words · about ${fmt(Math.round(p.words / 0.75 / 100) * 100)} tokens of your conversation`;
    $('#p-steps').innerHTML = (p.steps || []).map((s) => `
      <div class="pstep">
        <span class="n">${esc(s.name)}</span>
        <span class="s">${esc(s.what)}</span>
        ${(s.tools || []).length ? `<span class="tchips">${s.tools.map((t) => `<span class="tchip">${esc(t)}</span>`).join('')}</span>` : ''}
      </div>`).join('');
  }

  async function loadPrompts() {
    try {
      const r = await invoke('prompts');
      state.prompts = r.prompts || [];
      if (!state.prompts.some((p) => p.id === state.prompt)) state.prompt = state.prompts[0]?.id || null;
    } catch (e) {
      state.prompts = [];
      toast('Could not read the prompts: ' + e);
    }
    renderPrompts();
  }

  function renderSettings() {
    const s = state.settings;
    if (!s) return;
    $('#host').value = s.host;
    $('#port').value = s.port;
    $('#retention').value = s.retention_days;
    $('#sw-log').setAttribute('aria-checked', s.activity);
    $('#sw-payload').setAttribute('aria-checked', s.payloads);
    $('#sw-payload').disabled = !s.activity;
    const st = state.status || {};
    const bytes = state.sessions.reduce((a, x) => a + (x.bytes || 0), 0);
    $('#activity-path').textContent = (st.state_dir || '~/.ableton-music-maker') + '/activity/';
    $('#data-activity').textContent = `${state.sessions.length} session${state.sessions.length === 1 ? '' : 's'} · ${bytes < 1024 ? bytes + ' B' : Math.round(bytes / 1024) + ' KB'}`;
    $('#data-activity-path').textContent = (st.state_dir || '') + '/activity/';
    $('#data-sessions').textContent = `${(st.sessions || []).length} heartbeat${(st.sessions || []).length === 1 ? '' : 's'}`;
    $('#data-sessions-path').textContent = (st.state_dir || '') + '/sessions/';
    const target = st.script?.targets?.[0];
    $('#data-script').textContent = target?.installed || 'not installed';
    $('#data-script-path').textContent = target?.script || '';
    $('#about').textContent = `App ${st.app_version || ''} · bundled server ${st.server_version || ''} · Remote Script ${st.expected_script_version || ''}`;
  }

  function renderFoot() {
    const st = state.status || {};
    $('#sidefoot').innerHTML = `<span>App ${esc(st.app_version || '')} · server ${esc(st.server_version || '')}</span><span>Remote Script expected ${esc(st.expected_script_version || '')}</span><span>Third-party, not made by Ableton</span>`;
  }

  // ── data ──────────────────────────────────────────────────────────────────
  async function refreshStatus() {
    try {
      state.status = await invoke('get_status');
      await loadSessions();
      renderFoot();
      renderOverview();
      if (state.screen === 'setup') renderSetup();
      if (state.screen === 'settings') renderSettings();
      if (state.screen === 'prompts') renderPrompts();
    } catch (e) { toast('Status failed: ' + e); }
  }

  async function loadSessions() {
    state.sessions = await invoke('activity_sessions');
    if (!state.sessionId || !state.sessions.some((s) => s.id === state.sessionId)) {
      state.sessionId = state.sessions[0]?.id || null;
      state.selected = null;
    }
    const cur = state.sessions.find((s) => s.id === state.sessionId);
    state.sessionLabel = cur ? sessionLabel(cur) : '';
    await loadLines();
  }

  async function loadLines() {
    if (!state.sessionId) { state.lines = []; renderActivity(); return; }
    try {
      const before = state.lines.length;
      state.lines = await invoke('activity_lines', { session: state.sessionId });
      if (state.screen === 'activity' || before !== state.lines.length) renderActivity();
    } catch (e) { state.lines = []; renderActivity(); }
  }

  async function loadCaptures() {
    const body = $('#captures');
    try {
      const r = await invoke('list_captures');
      const caps = r.captures || [];
      body.innerHTML = caps.length
        ? caps.map((c) => `<tr>
            <td class="mono num">${c.slot}</td>
            <td>${esc(c.name)}${c.is_recording ? ' <span class="pill off">recording</span>' : ''}</td>
            <td class="r mono num">${c.length}</td>
            <td class="path">${esc(c.file_path || '—')}</td>
            <td>${c.file_path ? `<button class="btn small" data-play="${esc(c.file_path)}">Play</button> <button class="btn small" data-reveal="${esc(c.file_path)}">Reveal</button>` : ''}</td>
          </tr>`).join('')
        : '<tr><td colspan="5" class="empty">No captures yet. Ask Claude to capture a section.</td></tr>';
    } catch (e) {
      body.innerHTML = `<tr><td colspan="5" class="empty">${esc(String(e))}</td></tr>`;
    }
  }

  async function loadClientInfo() {
    try { state.clientInfo = await invoke('client_config', { kind: state.client }); } catch (e) { state.clientInfo = { kind: state.client, error: String(e) }; }
    renderSetup();
  }

  async function loadSettings() {
    state.settings = await invoke('get_settings');
    renderSettings();
  }

  async function saveSettings(patch) {
    Object.assign(state.settings, patch);
    try {
      const r = await invoke('set_settings', { settings: state.settings });
      toast(r.desktop_config_updated ? 'Saved. Claude Desktop’s entry updated for its next session.' : 'Saved');
    } catch (e) { toast('Could not save: ' + e); }
    renderSettings();
  }

  // ── actions ───────────────────────────────────────────────────────────────
  async function doAct(a, el) {
    const su = state.setup;
    if (a === 'install') {
      su.installing = true; su.installMsg = null; renderSetup();
      try {
        const r = await invoke('install_script');
        const first = r.results?.[0];
        su.installMsg = { ok: true, text: `${first?.status === 'unchanged' ? 'Already installed' : 'Installed'}. Restart Live to load it.` };
      } catch (e) { su.installMsg = { ok: false, text: String(e) }; }
      su.installing = false;
      await refreshStatus(); renderSetup();
    }
    if (a === 'changelib') {
      try {
        const r = await invoke('pick_library');
        if (!r.cancelled) { toast('Library set to ' + r.library); await refreshStatus(); renderSetup(); }
      } catch (e) { toast(String(e)); }
    }
    if (a === 'check') {
      su.checking = true; renderSetup();
      try { su.check = await invoke('check_live'); } catch (e) { su.check = { live_reachable: false, error: String(e) }; }
      su.checking = false;
      state.status && (state.status.live = su.check);
      renderSetup(); renderOverview();
    }
    if (a === 'addclient') {
      try {
        const r = await invoke('configure_client', { kind: 'desktop' });
        toast(r.backup ? 'Entry added. Backup kept beside the file.' : 'Config created.');
        await refreshStatus(); await loadClientInfo();
      } catch (e) { toast('Could not write the config: ' + e); }
    }
    if (a === 'copy') copy(el.dataset.copy);
    if (a === 'copyprompt') {
      const p = selectedPrompt();
      if (p) copy(p.body, 'Copied. Paste it into Claude and answer its questions.');
    }
    if (a === 'removelegacy') {
      try {
        const r = await invoke('remove_legacy_client');
        toast(r.removed ? 'Old AbletonMCP entry removed. Restart Claude Desktop.' : 'Nothing to remove.');
        await refreshStatus(); renderSetup();
      } catch (e) { toast('Could not edit the config: ' + e); }
    }
    if (a === 'test') {
      su.testing = true; renderSetup();
      try { su.test = await invoke('check_live'); } catch (e) { su.test = { live_reachable: false, error: String(e) }; }
      su.testing = false;
      state.status && (state.status.live = su.test);
      renderSetup(); renderOverview();
    }
  }

  function copy(text, msg) {
    const done = () => toast(msg || 'Copied');
    if (navigator.clipboard?.writeText) navigator.clipboard.writeText(text).then(done, () => fallback());
    else fallback();
    function fallback() {
      const ta = document.createElement('textarea'); ta.value = text; document.body.appendChild(ta); ta.select();
      try { document.execCommand('copy'); done(); } catch { toast('Copy failed — select the text and copy it'); }
      ta.remove();
    }
  }

  function go(screen) {
    state.screen = screen;
    document.querySelectorAll('.screen').forEach((s) => { s.hidden = s.id !== 's-' + screen; });
    document.querySelectorAll('.nav button').forEach((b) => { if (b.dataset.go === screen) b.setAttribute('aria-current', 'page'); else b.removeAttribute('aria-current'); });
    if (screen === 'setup') { renderSetup(); loadClientInfo(); }
    if (screen === 'settings') loadSettings();
    if (screen === 'activity') { renderActivity(); loadCaptures(); }
    if (screen === 'prompts') { if (state.prompts) renderPrompts(); else loadPrompts(); }
    if (window.__listen) window.__listen.onScreen(screen);
  }
  window.__goto = go;

  function toast(msg) { const t = $('#toast'); t.textContent = msg; t.classList.add('show'); clearTimeout(t._h); t._h = setTimeout(() => t.classList.remove('show'), 2200); }

  // ── wiring ────────────────────────────────────────────────────────────────
  document.addEventListener('click', (e) => {
    const open = e.target.closest('[data-open]'); if (open) { e.preventDefault(); invoke('open_external', { target: open.dataset.open }); return; }
    const play = e.target.closest('[data-play]'); if (play) { invoke('play_file', { path: play.dataset.play }).then(() => toast('Playing'), (err) => toast(String(err))); return; }
    const reveal = e.target.closest('[data-reveal]'); if (reveal) { invoke('reveal_file', { path: reveal.dataset.reveal }); return; }
    const pick = e.target.closest('[data-pick]'); if (pick) { state.prompt = pick.dataset.pick; renderPrompts(); return; }
    const goBtn = e.target.closest('[data-go]'); if (goBtn) { go(goBtn.dataset.go); return; }
    const chip = e.target.closest('.chip'); if (chip) { state.filter = chip.dataset.filter; document.querySelectorAll('.chip').forEach((c) => c.setAttribute('aria-pressed', c === chip)); renderActivity(); return; }
    const row = e.target.closest('tbody tr[data-i]'); if (row) { state.selected = +row.dataset.i; if (state.screen === 'overview') go('activity'); renderActivity(); return; }
    const cl = e.target.closest('[data-client]'); if (cl) { state.client = cl.dataset.client; state.clientInfo = null; renderSetup(); loadClientInfo(); return; }
    const act = e.target.closest('[data-act]'); if (act) { doAct(act.dataset.act, act); return; }
    const sw = e.target.closest('.switch'); if (sw && !sw.disabled && state.settings) {
      const on = sw.getAttribute('aria-checked') !== 'true';
      if (sw.id === 'sw-log') saveSettings({ activity: on, payloads: on ? state.settings.payloads : false });
      if (sw.id === 'sw-payload') saveSettings({ payloads: on });
      return;
    }
    if (e.target.id === 'clearlog') {
      if (!state.sessionId) return;
      if (!confirm('Delete this session’s activity log?')) return;
      invoke('clear_session', { session: state.sessionId }).then(() => { state.sessionId = null; return loadSessions(); }).then(() => { toast('Log cleared'); renderOverview(); }, (err) => toast(String(err)));
    }
    if (e.target.id === 'deletedata') {
      if (!confirm('Delete the activity log and heartbeats? The Remote Script and your client config stay.')) return;
      invoke('delete_local_data').then((r) => { toast(`Deleted ${r.removed} file${r.removed === 1 ? '' : 's'}`); return refreshStatus(); }).then(renderSettings, (err) => toast(String(err)));
    }
  });
  $('#session').addEventListener('change', (e) => { state.sessionId = e.target.value || null; state.selected = null; loadSessions().then(renderOverview); });
  for (const id of ['host', 'port', 'retention']) {
    $('#' + id).addEventListener('change', (e) => {
      const v = e.target.value.trim();
      if (id === 'host') saveSettings({ host: v || 'localhost' });
      if (id === 'port') { const p = parseInt(v, 10); if (p > 0 && p < 65536) saveSettings({ port: p }); else { toast('Port must be 1–65535'); renderSettings(); } }
      if (id === 'retention') { const d = parseInt(v, 10); if (d >= 1) saveSettings({ retention_days: d }); else renderSettings(); }
    });
  }

  refreshStatus();
  setInterval(refreshStatus, 10000);
  setInterval(() => { if (state.sessionId && (state.screen === 'activity' || state.screen === 'overview')) loadLines().then(() => { if (state.screen === 'overview') renderOverview(); }); }, 2000);
})();
