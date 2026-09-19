// Drawing the tap's numbers. Shared by the Listen screen and the float
// window, so both read the same scale and the same colours.
(function () {
  const FMIN = 20, FMAX = 20000, DBMIN = -72, DBMAX = 0;
  const TICKS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];

  const cssVar = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim() || '#888';
  // Bands are geometric, so band i occupies exactly one slot of the log axis.
  const xOf = (f) => Math.log(f / FMIN) / Math.log(FMAX / FMIN);
  const yOf = (db, top, h) => top + (DBMAX - Math.max(DBMIN, Math.min(DBMAX, db))) / (DBMAX - DBMIN) * h;

  /// Match the backing store to the CSS box so nothing is blurred on a
  /// Retina screen. Returns the box in CSS pixels.
  function fit(canvas) {
    const r = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const w = Math.max(1, Math.round(r.width * dpr));
    const h = Math.max(1, Math.round(r.height * dpr));
    if (canvas.width !== w || canvas.height !== h) { canvas.width = w; canvas.height = h; }
    return { w, h, dpr };
  }

  function drawSpectrum(canvas, frame, opts) {
    const o = opts || {};
    const labels = o.labels !== false;
    const { w, h, dpr } = fit(canvas);
    const c = canvas.getContext('2d');
    const padL = labels ? 34 * dpr : 0, padR = labels ? 10 * dpr : 0;
    const padT = labels ? 18 * dpr : 4 * dpr, padB = labels ? 22 * dpr : 4 * dpr;
    const gw = w - padL - padR, gh = h - padT - padB;

    c.fillStyle = cssVar('--scope');
    c.fillRect(0, 0, w, h);
    if (gw <= 0 || gh <= 0) return;

    c.strokeStyle = cssVar('--scope-grid');
    c.lineWidth = 1;
    c.font = `${11 * dpr}px ${cssVar('--mono')}`;
    c.fillStyle = cssVar('--scope-ink');
    c.textBaseline = 'middle';
    for (let db = DBMAX; db >= DBMIN; db -= 12) {
      const y = Math.round(yOf(db, padT, gh)) + 0.5;
      c.beginPath(); c.moveTo(padL, y); c.lineTo(w - padR, y); c.stroke();
      if (labels) { c.textAlign = 'right'; c.fillText(String(db), padL - 6 * dpr, y); }
    }
    c.textAlign = 'center'; c.textBaseline = 'alphabetic';
    for (const f of TICKS) {
      const x = Math.round(padL + xOf(f) * gw) + 0.5;
      c.beginPath(); c.moveTo(x, padT); c.lineTo(x, padT + gh); c.stroke();
      if (labels) c.fillText(f >= 1000 ? f / 1000 + 'k' : String(f), x, h - 7 * dpr);
    }
    if (labels) { c.textAlign = 'left'; c.fillText('dBFS', 4 * dpr, padT - 6 * dpr); }

    const bands = (frame && frame.bands) || [];
    if (!bands.length) return;
    const bw = gw / bands.length;
    const band = cssVar('--band'), hot = cssVar('--band-hot'), hold = cssVar('--hold');
    for (let i = 0; i < bands.length; i++) {
      const v = bands[i];
      if (v <= DBMIN) continue;
      const y = yOf(v, padT, gh);
      const x = padL + i * bw;
      c.fillStyle = v > -6 ? hot : band;
      c.fillRect(x + 0.5 * dpr, y, Math.max(1, bw - dpr), padT + gh - y);
    }
    const holds = (frame && frame.hold) || [];
    c.fillStyle = hold;
    c.globalAlpha = 0.85;
    for (let i = 0; i < holds.length; i++) {
      const v = holds[i];
      if (v <= DBMIN) continue;
      c.fillRect(padL + i * bw + 0.5 * dpr, yOf(v, padT, gh) - dpr, Math.max(1, bw - dpr), 2 * dpr);
    }
    c.globalAlpha = 1;
  }

  /// Master level: RMS as the solid bar, peak as the lighter one behind it,
  /// with a held line on top. 0 dB at the top, like Live's own meters.
  function drawMeter(canvas, frame) {
    const { w, h, dpr } = fit(canvas);
    const c = canvas.getContext('2d');
    c.fillStyle = cssVar('--scope');
    c.fillRect(0, 0, w, h);
    const padT = 14 * dpr, padB = 6 * dpr, scaleW = 24 * dpr;
    const gh = h - padT - padB;
    if (gh <= 0) return;
    const span = 60;
    const y = (db) => padT + Math.min(span, Math.max(0, -db)) / span * gh;

    c.font = `${10 * dpr}px ${cssVar('--mono')}`;
    c.textBaseline = 'middle';
    for (let db = 0; db >= -60; db -= 6) {
      const yy = Math.round(y(db)) + 0.5;
      c.fillStyle = cssVar('--scope-grid');
      c.fillRect(0, yy, w - scaleW, 1);
      if (db % 12 === 0) {
        c.fillStyle = cssVar('--scope-ink');
        c.textAlign = 'right';
        c.fillText(String(db), w - 4 * dpr, yy);
      }
    }
    const peak = (frame && frame.peak) || [-80, -80];
    const rms = (frame && frame.rms) || [-80, -80];
    const laneW = (w - scaleW - 12 * dpr) / 2;
    for (let i = 0; i < 2; i++) {
      const x = 4 * dpr + i * (laneW + 4 * dpr);
      const yp = y(peak[i]), yr = y(rms[i]);
      c.fillStyle = peak[i] > -3 ? cssVar('--band-hot') : cssVar('--band');
      c.globalAlpha = 0.45;
      c.fillRect(x, yp, laneW, padT + gh - yp);
      c.globalAlpha = 1;
      c.fillStyle = cssVar('--band');
      c.fillRect(x, yr, laneW, padT + gh - yr);
    }
    if (frame && frame.peak_hold > -80) {
      c.fillStyle = cssVar('--hold');
      c.fillRect(4 * dpr, y(frame.peak_hold) - dpr, (w - scaleW - 8 * dpr), 2 * dpr);
    }
    c.fillStyle = cssVar('--scope-ink');
    c.textAlign = 'center';
    c.textBaseline = 'alphabetic';
    c.fillText('L', 4 * dpr + laneW / 2, 11 * dpr);
    c.fillText('R', 8 * dpr + laneW * 1.5, 11 * dpr);
  }

  /// −3.2, +0.7, −∞ — the way every level in this product is written.
  function db(v) {
    if (v == null || v <= -79.5) return '−∞';
    return (v > 0 ? '+' : '−') + Math.abs(v).toFixed(1);
  }

  function hz(f) {
    if (f == null) return '—';
    return f >= 1000 ? (f / 1000).toFixed(f >= 10000 ? 0 : 1) + ' kHz' : Math.round(f) + ' Hz';
  }

  /// The centre frequency of the loudest band, for the readout.
  function loudestBand(frame) {
    const bands = (frame && frame.bands) || [];
    if (!bands.length) return null;
    let best = 0;
    for (let i = 1; i < bands.length; i++) if (bands[i] > bands[best]) best = i;
    if (bands[best] <= -79.5) return null;
    const edge = (i) => FMIN * Math.pow(FMAX / FMIN, i / bands.length);
    return { freq: Math.sqrt(edge(best) * edge(best + 1)), db: bands[best] };
  }

  /// A frame that decays instead of jumping, so the eye can follow it.
  /// "slow" is for reading the balance, "fast" for catching transients.
  function smooth(prev, next, release) {
    if (!prev || !prev.bands || prev.bands.length !== next.bands.length) return next;
    const k = release === 'slow' ? 0.08 : 0.3;
    const out = Object.assign({}, next);
    out.bands = next.bands.map((v, i) => (v >= prev.bands[i] ? v : prev.bands[i] + (v - prev.bands[i]) * k));
    return out;
  }

  window.AMMSpectrum = { drawSpectrum, drawMeter, db, hz, loudestBand, smooth, FMIN, FMAX };
})();
