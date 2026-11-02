/* Ink: a small chart kit in Consort's hand-drawn style.
 *
 * One file. No build step. No dependencies. It defines window.Ink.
 * Each chart is Ink.<name>(target, spec). The target is a selector or an element.
 * Colors come from the theme variables (--chart-1 to --chart-6, --foreground and more),
 * so a theme change needs no redraw. The strokes use a seeded random number generator.
 * The seed comes from the data and the target id, so the same data draws the same strokes.
 * Every chart has a readout on hover, tap and keyboard focus, adapts to the amount of data,
 * and some charts zoom. See "Ink kit" in docs/VISUALS-PLAN.md.
 */
(function (root) {
  'use strict';
  const Ink = {version: 2};
  const {abs, min, max, sin, cos, floor, ceil, round, PI} = Math;

  // ---------- small helpers
  const r1 = (x) => Math.round(x * 10) / 10;
  const clamp = (x, a, b) => (x < a ? a : x > b ? b : x);
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;'})[c]);
  const sum = (a) => a.reduce((s, x) => s + x, 0);
  const fin = (x) => typeof x === 'number' && isFinite(x);
  const ints = (a) => a.every((x) => x == null || Number.isInteger(x));

  function hash(s) {
    s = String(s);
    let h = 2166136261;
    for (let i = 0; i < s.length; i++) {
      h ^= s.charCodeAt(i);
      h = Math.imul(h, 16777619);
    }
    return h >>> 0;
  }
  // mulberry32: a small seeded generator that gives the same numbers in every browser.
  function rng(seed) {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) | 0;
      let t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  // ---------- errors
  class Bad {
    constructor(short, detail) {
      this.short = short;
      this.detail = detail;
    }
  }
  const bad = (short, detail) => {
    throw new Bad(short, detail);
  };
  const none = (detail) => bad('No data to draw', detail);

  // ---------- numbers for labels (one formatter for each pattern: a new one for each call is slow)
  const fmts = {};
  const intl = (k, f) => fmts[k] || (fmts[k] = f());
  const NF = (a, b) => intl(a + '/' + b, () => new Intl.NumberFormat('en-US', {minimumFractionDigits: a, maximumFractionDigits: b}));
  const DF = (o) => intl(JSON.stringify(o), () => new Intl.DateTimeFormat('en-US', Object.assign({timeZone: 'UTC'}, o)));
  function fnum(v, d) {
    if (d == null) {
      const a = abs(v);
      d = a >= 100 ? 0 : a >= 10 ? 1 : 2;
      return NF(0, d).format(+v.toFixed(d));
    }
    return NF(d, d).format(v);
  }
  function fmtc(v) {
    const a = abs(v);
    const t = (x, d) => +x.toFixed(d);
    return a >= 1e9 ? t(v / 1e9, 1) + 'B' : a >= 1e6 ? t(v / 1e6, 1) + 'M' : a >= 1e3 ? t(v / 1e3, a < 1e4 ? 1 : 0) + 'K' : fnum(v);
  }
  // A label formatter from spec.format (a function) or spec.unit ("GB", "$", "%").
  // f(v, true) is for a value label: a currency unit then shows two decimals.
  function mkfmt(spec) {
    if (typeof spec.format === 'function') return (v) => String(spec.format(v));
    const u = spec.unit == null ? '' : String(spec.unit);
    if (!u) return (v) => fnum(v);
    const cur = /^[$€£¥]$/.test(u);
    const tight = /^(%|K|M|B|k|×)$/.test(u);
    return (v, full) => {
      const n = fnum(abs(v), full && cur ? 2 : undefined);
      return (v < 0 ? '-' : '') + (cur ? u + n : tight ? n + u : n + ' ' + u);
    };
  }
  // A "nice" scale for an axis: 3 to 5 steps, a round step, little spare room.
  function scale(lo, hi, zero, int) {
    if (zero) lo = min(0, lo);
    if (hi <= lo) hi = lo + 1;
    const span = hi - lo;
    const pow = Math.pow(10, floor(Math.log10(span)) - 1);
    const pen = {1: 0, 2: 0, 5: 0, 10: 0, 20: 0, 50: 0, 100: 0, 2.5: 0.5, 4: 0.5, 25: 0.5, 40: 0.5, 3: 1, 30: 1, 1.5: 0.8, 15: 0.8};
    let best;
    for (const m of Object.keys(pen)) {
      const st = m * pow;
      const a = zero && lo === 0 ? 0 : floor(lo / st + 1e-9) * st;
      const b = ceil(hi / st - 1e-9) * st;
      const n = round((b - a) / st);
      if (n < 2 || n > 5) continue;
      const score = pen[m] + (n === 5 ? 0.8 : n === 2 ? 0.3 : 0) + (int && st % 1 ? 2 : 0) + (4 * ((b - a) - span)) / (b - a);
      if (!best || score < best.score) best = {score, a, b, st, n};
    }
    if (!best) best = {a: lo, b: hi, st: span / 3, n: 3};
    const t = [];
    for (let k = 0; k <= best.n; k++) t.push(+(best.a + k * best.st).toFixed(10));
    return {lo: best.a, hi: best.b, t};
  }

  // ---------- text measure (a canvas, with the font of the page)
  let mctx, mfam, mcache = {};
  function mw(s, size, wt) {
    s = String(s);
    const k = size + '/' + (wt || 400) + '/' + s;
    if (mcache[k] != null) return mcache[k];
    let w = s.length * size * 0.54;
    try {
      if (!mctx) mctx = document.createElement('canvas').getContext('2d');
      if (!mfam) mfam = getComputedStyle(document.documentElement).getPropertyValue('--font-sans').trim() || 'system-ui, sans-serif';
      mctx.font = (wt || 400) + ' ' + size + 'px ' + mfam;
      w = mctx.measureText(s).width;
    } catch (e) {}
    return (mcache[k] = w);
  }
  function clip(s, w, size, wt) {
    s = String(s);
    if (mw(s, size, wt) <= w) return s;
    while (s.length > 1 && mw(s + '…', size, wt) > w) s = s.slice(0, -1);
    return s.trimEnd() + '…';
  }
  // Break text into at most `n` lines of at most `w` px.
  function wrap(s, w, size, wt, n) {
    const words = String(s).split(/\s+/);
    const out = [];
    let cur = '';
    words.forEach((x) => {
      const t = cur ? cur + ' ' + x : x;
      if (!cur || mw(t, size, wt) <= w) cur = t;
      else {
        out.push(cur);
        cur = x;
      }
    });
    out.push(cur);
    if (out.length > n) {
      out[n - 1] = out.slice(n - 1).join(' ');
      out.length = n;
    }
    return out.map((l) => clip(l, w, size, wt));
  }

  // ---------- the pen: wobbly strokes, as the icons are drawn
  function wob(seed, amp) {
    const r = rng(seed);
    const p1 = r() * 6.28, p2 = r() * 6.28, p3 = r() * 6.28;
    const f1 = 17 + r() * 9, f2 = 6.5 + r() * 3;
    return (s) => amp * (0.6 * sin(s / f1 + p1) + 0.3 * sin(s / f2 + p2) + 0.1 * sin(s / 3.1 + p3));
  }
  function dense(pts, step) {
    const out = [pts[0]];
    for (let i = 1; i < pts.length; i++) {
      const [x0, y0] = pts[i - 1], [x1, y1] = pts[i];
      const n = max(1, floor(Math.hypot(x1 - x0, y1 - y0) / step));
      for (let k = 1; k <= n; k++) out.push([x0 + ((x1 - x0) * k) / n, y0 + ((y1 - y0) * k) / n]);
    }
    return out;
  }
  function perturb(pts, amp, seed, closed) {
    const w = wob(seed, amp), n = pts.length;
    let s = 0;
    return pts.map(([x, y], i) => {
      const a = i > 0 ? pts[i - 1] : closed ? pts[n - 2] : pts[0];
      const b = i < n - 1 ? pts[i + 1] : closed ? pts[1] : pts[n - 1];
      const dx = b[0] - a[0], dy = b[1] - a[1], L = Math.hypot(dx, dy) || 1;
      if (i > 0) s += Math.hypot(x - pts[i - 1][0], y - pts[i - 1][1]);
      const j = w(s);
      return [x - (dy / L) * j, y + (dx / L) * j];
    });
  }
  const pd = (p, z) => 'M' + r1(p[0][0]) + ' ' + r1(p[0][1]) + p.slice(1).map((q) => 'L' + r1(q[0]) + ' ' + r1(q[1])).join('') + (z ? 'Z' : '');
  function catmull(P, per) {
    per = per || 4;
    const out = [], Q = [P[0], ...P, P[P.length - 1]];
    for (let i = 1; i < Q.length - 2; i++) {
      const [p0, p1, p2, p3] = [Q[i - 1], Q[i], Q[i + 1], Q[i + 2]];
      for (let k = 0; k < per; k++) {
        const t = k / per, t2 = t * t, t3 = t2 * t;
        out.push([0, 1].map((j) => 0.5 * (2 * p1[j] + (-p0[j] + p2[j]) * t + (2 * p0[j] - 5 * p1[j] + 4 * p2[j] - p3[j]) * t2 + (-p0[j] + 3 * p1[j] - 3 * p2[j] + p3[j]) * t3)));
      }
    }
    out.push(P[P.length - 1]);
    return out;
  }
  const L = (pts, amp, seed, step, smooth) => pd(perturb(dense(smooth ? catmull(pts, pts.length > 80 ? 2 : 4) : pts, step || 3), amp, seed));
  // A smooth S curve with flat ends, from (xa, ya) to (xb, yb).
  function curve(xa, ya, xb, yb, amp, seed) {
    const k = (xb - xa) * 0.55, pts = [];
    for (let i = 0; i <= 24; i++) {
      const t = i / 24, u = 1 - t;
      pts.push([u * u * u * xa + 3 * u * u * t * (xa + k) + 3 * u * t * t * (xb - k) + t * t * t * xb, u * u * u * ya + 3 * u * u * t * ya + 3 * u * t * t * yb + t * t * t * yb]);
    }
    return pd(perturb(pts, amp, seed));
  }
  function area(pts, by, amp, seed, smooth) {
    const top = perturb(dense(smooth ? catmull(pts, pts.length > 80 ? 2 : 4) : pts, 3), amp, seed);
    return pd([...top, [top[top.length - 1][0], by], [top[0][0], by]], true);
  }
  function rect(x, y, w, h, r, amp, seed, topOnly, step) {
    r = max(0, min(r, w / 2, h / 2));
    const rb = topOnly ? 0 : r, pts = [];
    const arc = (cx, cy, rr, a0, a1) => {
      for (let k = 0; k <= 6; k++) {
        const t = ((a0 + ((a1 - a0) * k) / 6) * PI) / 180;
        pts.push([cx + rr * cos(t), cy + rr * sin(t)]);
      }
    };
    arc(x + r, y + r, r, 180, 270);
    arc(x + w - r, y + r, r, 270, 360);
    if (rb) {
      arc(x + w - rb, y + h - rb, rb, 0, 90);
      arc(x + rb, y + h - rb, rb, 90, 180);
    } else pts.push([x + w, y + h], [x, y + h]);
    pts.push(pts[0]);
    return pd(perturb(dense(pts, step || 5), amp, seed, true), true);
  }
  function arcD(cx, cy, r, a0, a1, amp, seed) {
    const n = max(8, floor(abs(a1 - a0) / 2));
    const pts = [];
    for (let k = 0; k <= n; k++) {
      const t = ((a0 + ((a1 - a0) * k) / n) * PI) / 180;
      pts.push([cx + r * cos(t), cy + r * sin(t)]);
    }
    return pd(perturb(pts, amp, seed));
  }
  function dab(cx, cy, r, seed, n) {
    n = n || 9;
    const R = rng(seed), p = [];
    for (let k = 0; k < n; k++) {
      const a = (2 * PI * k) / n + R() * 0.25, rr = r * (0.86 + R() * 0.24);
      p.push([cx + rr * cos(a), cy + rr * sin(a)]);
    }
    let d = 'M' + r1((p[0][0] + p[1][0]) / 2) + ' ' + r1((p[0][1] + p[1][1]) / 2);
    for (let i = 1; i <= n; i++) {
      const a = p[i % n], b = p[(i + 1) % n];
      d += 'Q' + r1(a[0]) + ' ' + r1(a[1]) + ' ' + r1((a[0] + b[0]) / 2) + ' ' + r1((a[1] + b[1]) / 2);
    }
    return d + 'Z';
  }
  function arrow(x0, y0, x1, y1, head, amp, seed) {
    head = head || 7;
    const a = Math.atan2(y1 - y0, x1 - x0);
    const h1 = [x1 - head * cos(a - 0.5), y1 - head * sin(a - 0.5)], h2 = [x1 - head * cos(a + 0.5), y1 - head * sin(a + 0.5)];
    return L([[x0, y0], [x1, y1]], amp ?? 0.8, seed || 1) + L([h1, [x1, y1], h2], 0.3, (seed || 1) + 7, 2);
  }

  const SUB = 'var(--subtle-foreground)';
  // ---------- pigments
  const pig = (c, d) => (c >= 1 && c <= 6 ? 'var(--chart-' + (c | 0) + ')' : typeof c === 'string' && c ? c : d == null ? 'var(--chart-1)' : pig(d));
  const glaze = (c, p) => 'color-mix(in srgb, ' + pig(c) + ' ' + p + '%, transparent)';

  // ---------- the page sheet (one style element, scoped to .ink)
  const CSS = `:where(:root){--background:#131E1A;--foreground:#F0EBDF;--muted-foreground:#B3BAB1;--subtle-foreground:#868F89;--card:#1B2623;--popover:#222E2A;--border:#27322E;--input:#384440;--chart-1:#77B4ED;--chart-2:#80C7A8;--chart-3:#E7BE62;--chart-4:#B2A0D6;--chart-5:#E3929C;--chart-6:#8E9A94;--chart-ink:#131E1A;--font-sans:"Atkinson Hyperlegible Next",system-ui,sans-serif;--font-mono:"Atkinson Hyperlegible Mono",ui-monospace,monospace}
.ink{display:block;position:relative;width:100%;font:12px/16px var(--font-sans);color:var(--muted-foreground)}
.ink.sp{display:inline-block;width:auto;max-width:100%}
.ink svg{display:block;overflow:visible;max-width:none}
.ink text{font-family:var(--font-sans);font-size:12px;fill:var(--subtle-foreground)}
.ink .m{fill:var(--muted-foreground)}.ink .M{fill:var(--muted-foreground);font-size:13px}
.ink .v{fill:var(--foreground);font-weight:600}.ink .V{fill:var(--foreground);font-weight:600;font-size:13px}
.ink .f{fill:var(--foreground);font-size:13px}.ink .h{fill:var(--foreground);font-weight:700;font-size:15px}
.ink .o{fill:var(--chart-ink);font-weight:700}.ink .O{fill:var(--chart-ink);font-weight:700;font-size:13px}
.ink .g{fill:none;stroke:var(--input);stroke-width:1;stroke-dasharray:2 5;stroke-linecap:round}
.ink .lg{display:flex;flex-wrap:wrap;gap:6px 16px;margin-bottom:16px}
.ink .lg span{display:flex;align-items:center;gap:6px}
.ink-err{padding:12px 16px;border:1px dashed var(--input);border-radius:var(--radius-md,6px);color:var(--muted-foreground);font:13px/20px var(--font-sans)}
.ink-i{touch-action:pan-y;-webkit-tap-highlight-color:transparent}.ink-z .ink-st{overflow:hidden}.ink-zm{cursor:grab}.ink-dr{cursor:grabbing}
.ink .mk{transition:opacity .12s}.ink-dim .mk:not(.on){opacity:.45}.ink-dim path.ln.on{stroke-width:3.4px!important}.ink .hv{pointer-events:none}
.ink-ro{position:absolute;z-index:2;left:0;top:0;display:none;box-sizing:border-box;max-width:240px;padding:6px 8px;border:1px solid var(--border);border-radius:var(--radius-md,6px);background:var(--popover);font:12px/16px var(--font-sans);color:var(--muted-foreground);text-align:left;overflow-wrap:anywhere;pointer-events:none}
.ink-ro b{display:block;font-weight:600;color:var(--foreground)}.ink-ro p{display:flex;align-items:center;gap:6px;margin:0}.ink-ro p svg{flex:none}.ink-ro p span{margin-left:auto;padding-left:12px;white-space:nowrap}
.ink-sr{position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}
.ink-zc{position:absolute;top:0;right:0;z-index:3;display:flex;gap:4px;opacity:0;pointer-events:none;transition:opacity .12s}.ink:hover>.ink-zc,.ink:focus-visible>.ink-zc{opacity:1;pointer-events:auto}
.ink-zc button{display:grid;place-items:center;box-sizing:border-box;width:24px;height:24px;padding:0;border:1px solid var(--border);border-radius:var(--radius-md,6px);background:var(--popover);color:var(--muted-foreground);cursor:pointer}.ink-zc button:hover{color:var(--foreground)}.ink-zc button:disabled{opacity:.4;cursor:default}
.ink-more{display:block;margin:4px 0 0;padding:4px 0;border:0;background:none;font:13px/20px var(--font-sans);color:var(--muted-foreground);cursor:pointer}.ink-more:hover{color:var(--foreground)}
.ink .nums{display:grid;gap:12px}
.ink .tile{display:flex;flex-direction:column;gap:6px;min-width:0;padding:16px;border-radius:var(--radius-lg,10px);background:var(--card)}
.ink .tl{color:var(--subtle-foreground)}.ink .tv{display:flex;align-items:baseline;gap:8px;flex-wrap:wrap}
.ink .big{font:700 30px/36px var(--font-sans);letter-spacing:-.015em;color:var(--foreground)}
.ink .dl{font-weight:600;color:var(--chart-2)}.ink .of{color:var(--subtle-foreground)}.ink .tt{font-size:13px;line-height:20px;color:var(--muted-foreground)}
.ink .tw{overflow-x:auto;overflow-y:hidden;overscroll-behavior-x:contain}.ink table{width:100%;border-collapse:collapse;font:13px/20px var(--font-sans)}
.ink th{padding:0 6px 8px;font-weight:400;font-size:12px;line-height:16px;text-align:left;color:var(--subtle-foreground)}
.ink td{padding:10px 6px;vertical-align:middle}.ink th.r,.ink td.r{text-align:right}.ink td.r{white-space:nowrap}
.ink td>div{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:3;overflow:hidden}.ink .tw.sc th:first-child,.ink .tw.sc td:first-child{position:sticky;left:0;z-index:1;background:var(--background)}.ink .tw.sc td.rl{position:static}.ink .tw.sc th.fr{min-width:160px}
.ink td.mo{font:12px/20px var(--font-mono);white-space:nowrap}.ink td.t0{color:var(--foreground)}.ink td.t1{color:var(--muted-foreground)}.ink td.t2{color:var(--subtle-foreground)}
.ink td.rl{padding:0;line-height:0}.ink td i{display:inline-flex;align-items:center;gap:8px;font-style:normal}
.ink .hm-r{display:flex;flex-wrap:wrap;gap:24px;align-items:flex-start}.ink .hm-h{margin-bottom:12px}.ink .hm-h b{display:block;font:600 13px/20px var(--font-sans);color:var(--foreground)}
.ink .hm-h span,.ink .hm-a,.ink .hm-kn{display:block;color:var(--subtle-foreground)}.ink .hm-a{margin:4px 0}.ink .hm-kn+.hm-kn{margin-top:2px}.ink .hm-kt{display:block;margin-bottom:8px}.ink .hm-kt,.ink .hm-k b{font-weight:600;color:var(--foreground)}
.ink .hm-k{display:flex;flex-wrap:wrap;align-items:flex-start;gap:6px 10px;margin-top:16px;color:var(--subtle-foreground)}.ink .hm-f,.ink .hm-f svg{position:relative}.ink .hm-f canvas{position:absolute}
.ink .hg{fill:none;stroke:color-mix(in srgb,var(--foreground) 14%,transparent);stroke-width:1;stroke-dasharray:2 5;stroke-linecap:round}`;
  function sheet() {
    if (document.getElementById('ink-css')) return;
    const s = document.createElement('style');
    s.id = 'ink-css';
    s.textContent = CSS;
    (document.head || document.documentElement).appendChild(s);
  }

  // ---------- svg pieces
  const svg = (W, H, body, c) => `<svg class="${c || 'pl'}" width="${r1(W)}" height="${r1(H)}" viewBox="0 0 ${r1(W)} ${r1(H)}" aria-hidden="true" focusable="false">${body}</svg>`;
  // A group that the readout can pick out: class "mk" fades when another mark has the focus.
  const G = (k, body, x) => `<g class="mk" data-k="${k}"${x || ''}>${body}</g>`;
  const P = (d, st, a) => `<path${a || ''} d="${d}" style="${st}"/>`;
  const fillP = (d, c, a) => P(d, 'fill:' + c, a);
  const strokeP = (d, c, w, x, a) => P(d, `fill:none;stroke:${c};stroke-width:${w};stroke-linecap:round;stroke-linejoin:round${x ? ';' + x : ''}`, a);
  let uid = 0;
  // Content that pans when the chart is zoomed. Past the zoom it is clipped to the plot.
  const panG = (body, z, x0, x1, H) => {
    if (z <= 1) return `<g class="pan">${body}</g>`;
    const id = 'ink-c' + ++uid;
    return `<clipPath id="${id}"><rect x="${r1(x0)}" y="0" width="${r1(x1 - x0)}" height="${r1(H)}"/></clipPath><g clip-path="url(#${id})"><g class="pan">${body}</g></g>`;
  };
  // Text fits in two lines of w px with no cut.
  const whole2 = (s, w, z, wt) => {
    const t = String(s).split(/\s+/).join(' ');
    return w > 0 && wrap(t, w, z, wt, 2).join(' ') === t;
  };
  const fits = (s, w) => whole2(s, w, 12, 400);
  // The least x in lo..hi where ok(x) is true, to about 1 px.
  function least(ok, lo, hi) {
    if (ok(lo)) return lo;
    if (!ok(hi)) return hi;
    while (hi - lo > 1) {
      const m = (lo + hi) / 2;
      ok(m) ? (hi = m) : (lo = m);
    }
    return hi;
  }
  const T = (x, y, s, c, a, x2) => `<text${c ? ` class="${c}"` : ''} x="${r1(x)}" y="${r1(y)}"${a ? ` text-anchor="${a}"` : ''} dy=".35em"${x2 || ''}>${esc(s)}</text>`;
  const grid = (x0, x1, y, seed) => `<path class="g" d="${L([[x0, y], [x1, y]], 0.5, seed, 24)}"/>`;
  const dabSvg = (c, seed, s) => `<svg width="${s || 12}" height="${s || 12}" viewBox="0 0 12 12" aria-hidden="true" style="flex:none"><path d="${dab(6, 6, 4.6, seed)}" style="fill:${pig(c)}"/></svg>`;
  // A legend row of dabs and names. `k0` gives each name a mark index, so a hover on it shows its series.
  const swatch = (c, dash) => `<svg width="16" height="10" viewBox="0 0 16 10" aria-hidden="true" style="flex:none"><path d="M2 5L14 5" style="fill:none;stroke:${pig(c)};stroke-width:2.2;stroke-linecap:round${dash ? ';stroke-dasharray:3 3' : ''}"/></svg>`;
  const legend = (items, S, k0) => `<div class="lg">${items.map((o, i) => `<span${k0 == null ? '' : ` data-k="${k0 + i}" data-s="${i}"`}>${o.line ? swatch(o.c, o.dash) : dabSvg(o.c, S + i, 10)}${esc(o.n)}</span>`).join('')}</div>`;

  // A y axis with pencil grid lines. Returns the left edge of the plot and the y function.
  function frame(W, top, bot, sc, fmt, S, rm) {
    const lw = ceil(max(...sc.t.map((v) => mw(fmt(v), 12))));
    const x0 = lw + 12, x1 = W - (rm == null ? 4 : rm);
    const Y = (v) => bot - ((v - sc.lo) / (sc.hi - sc.lo)) * (bot - top);
    let g = '';
    sc.t.forEach((v, i) => {
      g += grid(x0, x1, Y(v), S + i) + T(x0 - 8, Y(v), fmt(v), 'k', 'end');
    });
    return {x0, x1, Y, g};
  }
  // Category names under a plot, wrapped to two lines at most. Returns the svg and the line count.
  function cats(names, cx, slot, y, cls, wt) {
    const ls = names.map((n) => wrap(n, slot - 6, 12, wt, 2));
    const n = max(...ls.map((l) => l.length));
    let o = '';
    ls.forEach((l, i) => l.forEach((t, j) => (o += T(cx(i), y + 8 + j * 16, t, cls, 'middle'))));
    return {o, n};
  }
  // Move labels apart on one axis. Keeps the order. `gap` is the least distance.
  function spread(ys, gap, lo, hi) {
    const o = ys.map((y, i) => ({y, i})).sort((a, b) => a.y - b.y);
    for (let it = 0; it < 20; it++)
      for (let k = 1; k < o.length; k++)
        if (o[k].y - o[k - 1].y < gap) {
          const m = (o[k].y + o[k - 1].y) / 2;
          o[k - 1].y = m - gap / 2;
          o[k].y = m + gap / 2;
        }
    o.forEach((q) => (q.y = clamp(q.y, lo, hi)));
    const r = [];
    o.forEach((q) => (r[q.i] = q.y));
    return r;
  }

  // ---------- data readers
  // [[name, value, extra], ...] or [{label, value, color, note}, ...] or labels + values
  function items(spec, name, nulls) {
    let a = spec.data;
    if (!a && Array.isArray(spec.labels) && Array.isArray(spec.values)) a = spec.labels.map((l, i) => [l, spec.values[i]]);
    else if (a && !Array.isArray(a) && typeof a === 'object') a = Object.entries(a);
    if (!Array.isArray(a) || !a.length) none(`Ink.${name}: pass data: [["name", 12], ...] or labels and values.`);
    return a.map((d, i) => {
      const o = Array.isArray(d) ? {n: d[0], v: d[1], x: d[2]} : {n: d.label ?? d.name, v: d.value, c: d.color, x: d.note ?? d.kind};
      o.n = o.n == null ? '' : String(o.n);
      if (o.v == null && nulls) o.v = null;
      else if (o.v == null || !fin((o.v = Number(o.v)))) bad('Bad data', `Ink.${name}: the value of item ${i} ("${o.n}") is not a number.`);
      return o;
    });
  }
  function seriesOf(spec, name) {
    let s = spec.series;
    if (!s && spec.values) s = [{name: spec.name || '', values: spec.values, color: spec.color}];
    if (!Array.isArray(s) || !s.length) none(`Ink.${name}: pass series: [{name, values: [...]}] or values: [...].`);
    return s.map((o, i) => {
      if (!Array.isArray(o.values) || !o.values.length) none(`Ink.${name}: series ${i} has no values.`);
      return {n: o.name == null ? '' : String(o.name), v: o.values.map((x) => (x == null ? null : Number(x))), c: o.color};
    });
  }
  function checkNums(vs, name, what) {
    vs.forEach((v) => {
      if (v != null && !fin(v)) bad('Bad data', `Ink.${name}: ${what} must be numbers.`);
    });
  }
  const list = (it, f) => it.map((o) => `${o.n} ${f(o.v, true)}`).join(', ');
  const seedOf = (name, spec, el) => hash(name + '|' + (el && (el.id || '')) + '|' + JSON.stringify(spec, (k, v) => (typeof v === 'function' ? 'f' : v)));
  // A mark: r is where the pointer finds it, a is where the readout points (default r), t is the first line, l the next lines.
  // A line is a string or [pigment, name, value]. d is the item for spec.readout.
  const M = (r, t, l, d, x) => Object.assign({r, t, l, d}, x);
  const pct = (v, tot) => (tot ? (v / tot < 0.005 && v > 0 ? '<1%' : round((v / tot) * 100) + '%') : '');
  const share = (v, tot) => pct(v, tot) + ' of the total';
  // An even stride for axis labels: the first and the last label always show.
  function thin(N, dx, lw, cap) {
    if (N < 2) return [0];
    const need = lw * 1.2 + 20, s0 = max(1, ceil(need / max(dx, 0.01)), ceil((N - 1) / max(1, cap - 1)));
    let s = s0;
    for (let t = s0; t <= s0 * 1.6 && t < N; t++)
      if ((N - 1) % t === 0 || ((N - 1) % t) * dx >= need) {
        s = t;
        break;
      }
    const out = [];
    for (let i = 0; i < N - 1; i += s) out.push(i);
    if (out.length > 1 && (N - 1 - out[out.length - 1]) * dx < need) out.pop();
    out.push(N - 1);
    return out;
  }
  // Nice round steps between lo and hi, about n of them.
  function steps(lo, hi, n) {
    const raw = (hi - lo) / max(1, n), p = Math.pow(10, floor(Math.log10(raw || 1))), st = [1, 2, 2.5, 5, 10].find((m) => m * p >= raw) * p, out = [];
    for (let t = ceil(lo / st - 1e-9) * st; t <= hi + 1e-9; t += st) out.push(+t.toFixed(10));
    return out;
  }

  // ---------- one layer for every chart: the readout, the keys, the zoom and the pan
  const SMIN = 11 / 12, shown = new Set();
  let wired = false;
  // A tap outside a chart hides its readout. Escape hides every readout, and the app does not get it.
  function wire() {
    if (wired) return;
    wired = true;
    document.addEventListener('pointerdown', (e) => shown.forEach((u) => u.st.how === 'tap' && !u.box.contains(e.target) && u.hide()), true);
    document.addEventListener('keydown', (e) => {
      if (e.key !== 'Escape' || !shown.size) return;
      shown.forEach((u) => u.hide());
      e.preventDefault();
    });
  }
  const ICON = {out: 'M3 6h6', fit: 'M2 4.5V2h2.5M7.5 2H10v2.5M10 7.5V10H7.5M4.5 10H2V7.5', in: 'M3 6h6M6 3v6'};
  const NAME = {out: 'Zoom out', fit: 'Fit', in: 'Zoom in'};
  function ui(box, stage, el, spec, model) {
    const st = {z: 1, pan: 0, py: 0, zmax: 1, cur: -1, kcur: -1, how: '', sel: '', marks: [], r: {}, base: null, v: {s: 1}, W: 0, pz: null};
    const on = spec.readout !== false;
    let ro, live, zc, pend = null, praf = 0, zraf = 0, drag = null;
    const u = {box, hide, render, st};
    const org = () => {
      const sv = st.sv;
      if (!sv || !sv.isConnected) return null;
      const b = box.getBoundingClientRect(), r = sv.getBoundingClientRect();
      return {x: r.left - b.left + (model.whole ? st.pan : 0), y: r.top - b.top, s: st.v.s};
    };
    const disp = (q, p, o) => ({x: o.x + q[0] * o.s - (p ? st.pan : 0), y: o.y + q[1] * o.s - (p ? st.py : 0), w: q[2] * o.s, h: q[3] * o.s});
    const rel = (e) => {
      const b = box.getBoundingClientRect(), r = e.getBoundingClientRect();
      return {x: r.left - b.left, y: r.top - b.top, w: r.width, h: r.height};
    };
    const keys = () => (st.r.order || st.marks.map((m, i) => i)).filter((i) => st.marks[i] && !st.marks[i].nk);
    function render(W, zoomOnly) {
      const whole = model.whole, again = () => u.redraw(true);
      if (!zoomOnly || !st.base || st.W !== W) {
        let v = {z: 1, s: 1, fit: W, again}, r = model.draw(W, box, v);
        if (whole && r.need > W + 1) {
          const s = max(W / r.need, SMIN);
          v = {z: 1, s, fit: W / s, again};
          r = model.draw(v.fit, box, v);
        }
        let zm = r.zoom || 1;
        if (whole && r.need > v.fit * 1.02) zm = max(zm, clamp(r.need / v.fit, 2, 6));
        if (spec.zoom === true) zm = max(zm, 4);
        if (spec.zoom === false || !model.zoomable) zm = 1;
        // The height of the fit view: a zoomed view keeps it.
        const h = /class="pl"[^>]*height="([\d.]+)"/.exec(r.html);
        Object.assign(st, {zmax: zm, base: r, v, W, bh: h ? +h[1] * v.s : 0});
      }
      st.z = clamp(st.z, 1, st.zmax);
      const v = Object.assign({}, st.v, {z: st.z});
      const r = st.z > 1 ? model.draw(whole ? v.fit * st.z : W, box, v) : st.base;
      unmark();
      stage.innerHTML = r.html;
      st.r = r;
      st.sv = stage.querySelector('svg.pl');
      if (whole && st.sv) {
        const sv = st.sv, lw = +sv.getAttribute('width'), lh = +sv.getAttribute('height');
        if (v.s < 1) {
          sv.setAttribute('width', r1(lw * v.s));
          sv.setAttribute('height', r1(lh * v.s));
        }
        stage.style.height = st.z > 1 && st.bh ? st.bh + 'px' : '';
      }
      // r.pan is [x0, x1], or [x0, x1, y0, y1] for a chart that pans up and down too (a heat map).
      st.pz = whole ? {x0: 0, vw: W, w0: W} : r.pan ? {x0: r.pan[0], vw: r.pan[1] - r.pan[0], w0: r.pan[1] - r.pan[0], y0: r.pan[2] || 0, vh: r.pan[3] - r.pan[2] || 0} : null;
      if (r.after) r.after(box, stage);
      st.marks = r.marks || [];
      if (whole) st.marks.forEach((m) => (m.p = true));
      const keyed = on && st.marks.some((m) => !m.nk);
      if (keyed || st.zmax > 1) box.tabIndex = 0;
      else box.removeAttribute('tabindex');
      box.classList.toggle('ink-i', keyed || st.zmax > 1);
      box.classList.toggle('ink-z', st.zmax > 1 && !!whole);
      setPan(st.pan);
      controls();
      st.cur = -1;
      if (st.how === 'key' && st.marks[st.kcur]) show(st.kcur, 'key');
      else hide();
    }
    function unmark() {
      if (st.sel) stage.querySelectorAll('.on').forEach((e) => e.classList.remove('on'));
      box.classList.remove('ink-dim');
      st.sel = '';
    }
    function hide() {
      unmark();
      if (st.cur >= 0 && st.r.hover) st.r.hover(-1, stage);
      st.cur = -1;
      st.how = '';
      if (ro) ro.style.display = 'none';
      shown.delete(u);
    }
    function linesOf(m) {
      const base = [m.t].concat(m.l || []).filter((x) => x != null && x !== '');
      if (typeof spec.readout !== 'function') return base;
      let o;
      try {
        o = spec.readout(Object.assign({}, m.d, {lines: base.map((x) => (Array.isArray(x) ? x.slice(1).filter((y) => y !== '').join(' ') : String(x)))}));
      } catch (e) {
        console.error(e);
        return base;
      }
      if (o == null || o === false || o === '') return null;
      return (Array.isArray(o) ? o : String(o).split('\n')).map(String).filter((x) => x);
    }
    function fill(lines) {
      if (!ro) {
        ro = document.createElement('div');
        ro.className = 'ink-ro';
        ro.setAttribute('aria-hidden', 'true');
      }
      if (ro.parentNode !== box) box.appendChild(ro);
      ro.textContent = '';
      lines.forEach((x, i) => {
        const p = document.createElement(i ? 'p' : 'b');
        if (Array.isArray(x)) {
          p.innerHTML = `<svg width="8" height="8" viewBox="0 0 12 12"><path d="${dab(6, 6, 5, 7 + i)}"/></svg>`;
          p.firstChild.firstChild.style.fill = pig(x[0]);
          p.appendChild(document.createTextNode(x[1]));
          if (x[2] !== '' && x[2] != null) {
            const v = document.createElement('span');
            v.textContent = x[2];
            p.appendChild(v);
          }
        } else p.textContent = x;
        ro.appendChild(p);
      });
    }
    function place(A, side) {
      const bw = box.clientWidth, bh = box.clientHeight, g = 8;
      Object.assign(ro.style, {left: '0px', top: '0px', display: 'block', maxWidth: min(240, bw) + 'px'});
      const rw = ro.offsetWidth, rh = ro.offsetHeight;
      let lo = 0, hi = bh - rh;
      if (hi < lo) {
        const b = box.getBoundingClientRect(), d = document.documentElement.getBoundingClientRect();
        lo = d.top - b.top;
        hi = d.bottom - b.top - rh;
      }
      let x, y;
      if (side) {
        x = A.x + A.w + 14;
        if (x + rw > bw) x = A.x - 14 - rw;
        if (x < 0) x = A.x + A.w / 2 - rw / 2;
        y = A.y - rh - g >= lo ? A.y - rh - g : A.y + g;
      } else {
        const up = A.y - g - rh, dn = A.y + A.h + g;
        x = A.x + A.w / 2 - rw / 2;
        y = up >= lo ? up : dn <= hi ? dn : null;
        if (y == null) {
          // No room above or below, as for a tall bar: beside it, at its top.
          x = A.x + A.w + g + rw <= bw ? A.x + A.w + g : A.x - g - rw >= 0 ? A.x - g - rw : x;
          y = A.y;
        }
      }
      ro.style.left = r1(clamp(x, 0, max(0, bw - rw))) + 'px';
      ro.style.top = r1(clamp(y, lo, max(lo, hi))) + 'px';
    }
    function show(k, how, h) {
      const m = st.marks[k];
      if (!m || !on) return hide();
      const same = k === st.cur && ro && ro.style.display !== 'none';
      st.cur = k;
      st.how = how;
      if (how === 'key') st.kcur = k;
      const sel = typeof m.on === 'function' ? m.on(h && h.ly) : m.on == null ? `[data-k="${k}"]` : m.on;
      if (sel !== st.sel) {
        unmark();
        st.sel = sel || '';
        if (sel) {
          box.classList.add('ink-dim');
          stage.querySelectorAll(sel).forEach((e) => e.classList.add('on'));
        }
      }
      if (st.r.hover) st.r.hover(k, stage);
      const o = org();
      if (!same) {
        const ls = linesOf(m);
        if (!ls || !ls.length) {
          if (ro) ro.style.display = 'none';
          shown.add(u);
          return;
        }
        fill(ls);
        if (how !== 'ptr') {
          if (!live) {
            live = document.createElement('div');
            live.className = 'ink-sr';
            live.setAttribute('aria-live', 'polite');
          }
          if (live.parentNode !== el) el.appendChild(live);
          live.textContent = ls.map((x) => (Array.isArray(x) ? x.slice(1).join(' ') : x)).join('. ');
        }
      } else if (!m.follow || how !== 'ptr') {
        shown.add(u);
        return;
      }
      let A;
      if (h && h.el) A = rel(h.el);
      else if (m.el) A = rel(m.el);
      else if (m.follow && h && h.bx != null) A = {x: h.bx, y: h.by - 10, w: 0, h: 20};
      else if (o && (m.a || m.r)) A = disp(m.a || m.r, m.p, o);
      place(A || {x: box.clientWidth / 2, y: 0, w: 0, h: 0}, m.side);
      shown.add(u);
    }
    function hit(e) {
      const t = e.target;
      if (!t || !t.closest || t.closest('.ink-zc,.ink-more')) return null;
      const b = box.getBoundingClientRect(), bx = e.clientX - b.left, by = e.clientY - b.top;
      const d = t.closest('[data-k]');
      if (d && stage.contains(d) && (!(d instanceof SVGElement) || model.dom) && st.marks[+d.dataset.k]) return {k: +d.dataset.k, el: d instanceof SVGElement ? null : d, bx, by};
      const o = org();
      if (!o) return null;
      const cx = (bx - o.x) / o.s, cy = (by - o.y) / o.s, px = (bx - o.x + st.pan) / o.s, qy = (by - o.y + st.py) / o.s;
      let k = st.r.pick ? st.r.pick(px, cy, cx, qy) : -1, best = Infinity;
      if (k == null || k < 0) {
        k = -1;
        st.marks.forEach((m, i) => {
          const q = m.r, x = m.p ? px : cx, y = m.p ? qy : cy;
          if (q && x >= q[0] && x <= q[0] + q[2] && y >= q[1] && y <= q[1] + q[3] && q[2] * q[3] < best) {
            best = q[2] * q[3];
            k = i;
          }
        });
      }
      return k >= 0 ? {k, bx, by, ly: cy} : null;
    }
    function tick() {
      praf = 0;
      const e = pend;
      if (!e || !box.isConnected || drag) return;
      const h = hit(e);
      if (h) show(h.k, 'ptr', h);
      else if (st.how === 'ptr') hide();
    }
    function reveal(k) {
      const m = st.marks[k], pz = st.pz, o = org(), sc = m && m.el && m.el.closest('.tw');
      if (sc) {
        const a = m.el.getBoundingClientRect(), b = sc.getBoundingClientRect(), f = sc.querySelector('.sc td:first-child');
        const l = b.left + (f && !f.contains(m.el) ? f.offsetWidth : 0);
        if (a.left < l) sc.scrollLeft -= l - a.left + 8;
        else if (a.right > b.right) sc.scrollLeft += a.right - b.right + 8;
      }
      if (!m || !pz || st.z <= 1 || !m.p || !o) return;
      const q = m.a || m.r, a = q[0] * o.s, b = (q[0] + q[2]) * o.s, l = pz.x0 + st.pan, r = l + pz.vw;
      if (a < l + 8) setPan(a - pz.x0 - 24);
      else if (b > r - 8) setPan(b - pz.x0 - pz.vw + 24);
      if (!pz.vh) return;
      const c = q[1] * o.s, d = (q[1] + q[3]) * o.s, t = pz.y0 + st.py;
      if (c < t + 8) setPan(st.pan, c - pz.y0 - 24);
      else if (d > t + pz.vh - 8) setPan(st.pan, d - pz.y0 - pz.vh + 24);
    }
    // p pans across. q pans up and down, only on a chart that gives r.pan a y range.
    function setPan(p, q) {
      const pz = st.pz;
      st.pan = pz ? clamp(p, 0, max(0, pz.w0 * st.z - pz.vw)) : 0;
      st.py = pz && pz.vh ? clamp(q ?? st.py, 0, max(0, pz.vh * st.z - pz.vh)) : 0;
      const t = -r1(st.pan);
      if (model.whole) {
        if (st.sv) st.sv.style.transform = t ? `translateX(${t}px)` : '';
      } else stage.querySelectorAll('.pan').forEach((g) => g.setAttribute('transform', `translate(${t} ${-r1(st.py)})`));
      if (st.r.onpan) st.r.onpan(st.pan, st.py, stage);
      box.classList.toggle('ink-zm', st.z > 1);
      if (zc) zc.children[1].disabled = st.z <= 1;
    }
    function zoomTo(z1, cx, cy) {
      const pz = st.pz;
      z1 = clamp(z1, 1, st.zmax);
      if (!pz || abs(z1 - st.z) < 1e-3) return;
      const o = org(), b = box.getBoundingClientRect(), x0 = (o ? o.x : 0) + pz.x0, y0 = (o ? o.y : 0) + pz.y0;
      const px = cx == null ? pz.vw / 2 : clamp(cx - b.left - x0, 0, pz.vw), c = (px + st.pan) / (pz.w0 * st.z);
      const qy = cy == null ? pz.vh / 2 : clamp(cy - b.top - y0, 0, pz.vh), d = pz.vh ? (qy + st.py) / (pz.vh * st.z) : 0;
      st.z = z1;
      st.pan = c * pz.w0 * z1 - px;
      st.py = d * pz.vh * z1 - qy;
      hide();
      if (!zraf)
        zraf = requestAnimationFrame(() => {
          zraf = 0;
          u.redraw('zoom');
        });
    }
    function controls() {
      if (st.zmax <= 1) {
        if (zc) zc.remove();
        zc = null;
        return;
      }
      if (!zc) {
        zc = document.createElement('div');
        zc.className = 'ink-zc';
        zc.innerHTML = ['out', 'fit', 'in'].map((a) => `<button type="button" tabindex="-1" data-a="${a}" aria-label="${NAME[a]}" title="${NAME[a]}"><svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="${ICON[a]}" style="fill:none;stroke:currentColor;stroke-width:1.5;stroke-linecap:round;stroke-linejoin:round"/></svg></button>`).join('');
        zc.addEventListener('click', (e) => {
          const a = e.target.closest('button');
          if (!a) return;
          a.dataset.a === 'fit' ? zoomTo(1) : zoomTo(st.z * (a.dataset.a === 'in' ? 1.5 : 1 / 1.5));
        });
      }
      if (zc.parentNode !== box) box.appendChild(zc);
      zc.children[0].disabled = st.z <= 1;
      zc.children[1].disabled = st.z <= 1;
      zc.children[2].disabled = st.z >= st.zmax - 1e-3;
    }
    box.addEventListener('pointermove', (e) => {
      if (drag) {
        const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
        if (!drag.on && (abs(dx) > 4 || (st.pz && st.pz.vh && abs(dy) > 4))) {
          drag.on = true;
          try {
            box.setPointerCapture(drag.id);
          } catch (x) {}
          box.classList.add('ink-dr');
          hide();
        }
        if (drag.on) setPan(drag.p - dx, drag.q - dy);
        return;
      }
      if (e.pointerType !== 'mouse' && st.how === 'tap') return;
      pend = e;
      if (!praf) praf = requestAnimationFrame(tick);
    });
    box.addEventListener('pointerleave', () => {
      pend = null;
      if (st.how === 'ptr') hide();
    });
    box.addEventListener('pointerdown', (e) => {
      if (e.target.closest && e.target.closest('.ink-zc,.ink-more')) return;
      if (st.z > 1 && (e.pointerType !== 'mouse' || e.button === 0)) drag = {x: e.clientX, y: e.clientY, p: st.pan, q: st.py, id: e.pointerId, on: false};
      if (e.pointerType !== 'mouse') {
        const h = hit(e);
        h ? show(h.k, 'tap', h) : hide();
      }
    });
    const end = () => {
      if (drag && drag.on) box.classList.remove('ink-dr');
      drag = null;
    };
    box.addEventListener('pointerup', end);
    box.addEventListener('pointercancel', end);
    // A plain vertical wheel always goes on to the page. Only a pinch, Cmd or Ctrl with the wheel, and a sideways wheel stay here.
    box.addEventListener('wheel', (e) => {
      // Shift with a wheel that only goes up and down pans a wide table, where the system does not turn it sideways.
      const tw = e.shiftKey && !e.deltaX && !e.ctrlKey && !e.metaKey && e.target.closest && e.target.closest('.tw');
      if (tw) {
        const l = tw.scrollLeft;
        tw.scrollLeft += e.deltaY * (e.deltaMode ? 16 : 1);
        if (tw.scrollLeft !== l) e.preventDefault();
        return;
      }
      if (st.zmax <= 1) return;
      if (e.ctrlKey || e.metaKey) {
        e.preventDefault();
        zoomTo(st.z * Math.exp(clamp(-e.deltaY * (e.deltaMode ? 0.05 : 0.01), -0.5, 0.5)), e.clientX, e.clientY);
        return;
      }
      const dx = e.shiftKey && !e.deltaX ? e.deltaY : e.deltaX;
      if (st.z > 1 && dx && (e.shiftKey || abs(e.deltaX) > abs(e.deltaY))) {
        const p0 = st.pan;
        setPan(st.pan + dx * (e.deltaMode ? 16 : 1));
        if (st.pan !== p0) {
          e.preventDefault();
          if (st.how === 'ptr') hide();
        }
      }
    }, {passive: false});
    box.addEventListener('keydown', (e) => {
      if (e.target !== box || e.metaKey || e.ctrlKey || e.altKey) return;
      const k = e.key;
      if (st.zmax > 1 && /^[-_+=0]$/.test(k)) {
        e.preventDefault();
        zoomTo(k === '0' ? 1 : st.z * (k === '+' || k === '=' ? 1.5 : 1 / 1.5));
        return;
      }
      if (!on) return;
      const ks = keys(), i = st.marks[st.kcur] ? st.kcur : -1;
      if (!ks.length) return;
      let j = -1;
      if (/^Arrow/.test(k)) {
        j = st.r.nav && i >= 0 ? st.r.nav(i, k) : -1;
        if (j == null || j < 0) {
          const p = ks.indexOf(i);
          j = p < 0 ? ks[0] : ks[clamp(p + (k === 'ArrowRight' || k === 'ArrowDown' ? 1 : -1), 0, ks.length - 1)];
        }
      } else if (k === 'Home') j = ks[0];
      else if (k === 'End') j = ks[ks.length - 1];
      else if ((k === 'Enter' || k === ' ') && i >= 0 && st.marks[i].act) {
        e.preventDefault();
        st.marks[i].act();
        return;
      }
      if (j == null || j < 0) return;
      e.preventDefault();
      reveal(j);
      show(j, 'key');
    });
    box.addEventListener('focus', () => {
      if (!on || !box.matches(':focus-visible')) return;
      const j = st.marks[st.kcur] ? st.kcur : keys()[0];
      if (j == null) return;
      reveal(j);
      show(j, 'key');
    });
    box.addEventListener('blur', () => st.how === 'key' && hide());
    stage.addEventListener('scroll', () => st.how === 'ptr' && hide(), true);
    wire();
    return u;
  }

  // ---------- running a chart: draw now, draw again when the width changes
  const live = new Set();
  function mount(name, target, spec, make, role) {
    const el = typeof target === 'string' ? document.querySelector(target) : target;
    if (!el && typeof target === 'string' && document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', () => mount(name, target, spec, make, role), {once: true});
      return null;
    }
    if (!el || !el.appendChild) {
      console.error(`Ink.${name}: no element matches ${JSON.stringify(String(target))}. Draw after the element exists.`);
      return null;
    }
    sheet();
    if (el._ink) el._ink.stop();
    let model;
    const fail = (short, detail) => {
      console.error(detail || `Ink.${name}: ${short}`);
      el.textContent = '';
      const m = document.createElement('div');
      m.className = 'ink-err';
      m.setAttribute('role', 'img');
      m.setAttribute('aria-label', short);
      m.textContent = short;
      el.appendChild(m);
      return null;
    };
    try {
      if (!spec || typeof spec !== 'object') none(`Ink.${name}: the second argument must be an object with the data.`);
      model = make(spec, seedOf(name, spec, el));
    } catch (e) {
      if (!(e instanceof Bad)) {
        console.error(e);
        return fail('The chart could not be drawn');
      }
      return fail(e.short, e.detail);
    }
    const box = document.createElement('div');
    box.className = 'ink' + (model.cls ? ' ' + model.cls : '');
    box.dataset.ink = name;
    if (role !== 'none') {
      box.setAttribute('role', 'img');
      box.setAttribute('aria-label', spec.alt || model.alt);
    }
    if (model.w) box.style.width = model.w + 'px';
    el.textContent = '';
    el.appendChild(box);
    const stage = document.createElement('div');
    stage.className = 'ink-st';
    box.appendChild(stage);
    const u = ui(box, stage, el, spec, model);
    let last = 0, raf, ro;
    // force: true draws again at the same width, "zoom" draws only the zoomed view again.
    const draw = (force) => {
      const w = box.clientWidth;
      if (!w || (w === last && !force)) return;
      last = w;
      try {
        u.render(w, force === 'zoom');
      } catch (e) {
        console.error(e);
        stage.innerHTML = '<div class="ink-err">The chart could not be drawn</div>';
      }
    };
    u.redraw = draw;
    const ctl = {
      box,
      draw,
      stop() {
        cancelAnimationFrame(raf);
        ro && ro.disconnect();
        removeEventListener('resize', onResize);
        live.delete(ctl);
        u.hide();
        el._ink = null;
      },
    };
    // At most one redraw for each animation frame, so a drag of the width stays cheap.
    const onResize = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => draw());
    };
    el._ink = ctl;
    live.add(ctl);
    draw(true);
    if (typeof ResizeObserver === 'function') {
      ro = new ResizeObserver(onResize);
      ro.observe(box);
    } else addEventListener('resize', onResize);
    return {redraw: () => draw(true), destroy: ctl.stop};
  }
  const def = (name, make, role) => {
    Ink[name] = (target, spec) => mount(name, target, spec, make, role);
  };
  // The font loads after the first draw: text widths change, so draw again.
  function fontsChanged() {
    mcache = {};
    mfam = null;
    live.forEach((c) => (c.box.isConnected ? c.draw(true) : live.delete(c)));
  }
  Ink.redraw = fontsChanged;
  if (root.document && document.fonts) {
    document.fonts.ready.then(fontsChanged);
    document.fonts.addEventListener && document.fonts.addEventListener('loadingdone', fontsChanged);
  }

  // line: one series or several. Names at the line ends, or a legend past six series.
  // The readout lists every series at the x under the pointer. Many points can zoom.
  def('line', (spec, S) => {
    const ss = seriesOf(spec, 'line');
    ss.forEach((s) => checkNums(s.v, 'line', 'values'));
    const many = ss.length > 1, leg = ss.length > 6, labels = (spec.labels || []).map(String);
    const N = max(...ss.map((s) => s.v.length));
    ss.forEach((s, i) => {
      s.dash = s.c == null && i >= 6;
      s.c = s.c ?? (many ? (i % 6) + 1 : 1);
    });
    const all = ss.flatMap((s) => s.v.filter((x) => x != null));
    if (!all.length) none('Ink.line: there are no values.');
    const sc = scale(spec.min ?? min(...all), spec.max ?? max(...all), false);
    const fmt = mkfmt(spec), showArea = spec.area ?? !many, notes = spec.notes || [];
    const vals = (s) => s.v.filter((x) => x != null);
    const nat = (nt) => (typeof nt.at === 'number' ? nt.at : labels.indexOf(String(nt.at)));
    const alt = 'Line chart. ' + ss.map((s) => {
      const v = vals(s);
      return v.length ? `${s.n || 'Values'} from ${fmt(v[0], true)} to ${fmt(v[v.length - 1], true)}, low ${fmt(min(...v), true)}, high ${fmt(max(...v), true)}.` : '';
    }).join(' ') + (labels.length ? ` From ${labels[0]} to ${labels[labels.length - 1]}.` : '');
    return {
      alt,
      zoomable: true,
      draw(W, box, v) {
        const H = clamp(round(W * 0.34), 170, 300), top = 12, bot = H - 28, z = v.z;
        const rm = many && !leg ? clamp(max(...ss.map((s) => mw(s.n, 12, 600))) + 14, 40, W * 0.34) : 8;
        const fr = frame(W, top, bot, sc, fmt, S, rm), {x0, x1, Y} = fr, dx = N > 1 ? ((x1 - x0) * z) / (N - 1) : 0;
        const X = (i) => (N > 1 ? x0 + dx * i : (x0 + x1) / 2);
        const pts = ss.map((s) => s.v.map((v, i) => (v == null ? null : [X(i), Y(v)])).filter(Boolean));
        let o = fr.g, p = '';
        const ends = [];
        ss.forEach((s, k) => {
          const q = pts[k], at = ` class="mk" data-s="${k}"`;
          if (!q.length) return ends.push(bot);
          if (showArea && q.length > 1) p += fillP(area(q, bot, 0.8, S + 10 + k, true), glaze(s.c, many ? 10 : 14), at);
          if (q.length > 1) p += strokeP(L(q, 0.8, S + 20 + k, 6, true), pig(s.c), 2.2, s.dash ? 'stroke-dasharray:7 5' : '', ` class="mk ln" data-s="${k}"`);
          const e = q[q.length - 1];
          p += fillP(dab(e[0], e[1], many ? 4.2 : 5.2, S + 30 + k), pig(s.c), at);
          ends.push(e[1]);
        });
        // Two lines (name and value) at each line end when they fit, else the name only.
        const two = ss.length * 34 <= bot - top + 8, ys = many && !leg ? spread(ends, two ? 34 : 17, top + 6, bot) : [];
        if (many && !leg)
          ss.forEach((s, k) => {
            const lv = vals(s).pop();
            o += `<g class="mk" data-s="${k}">` + T(x1 + 12, ys[k] - (two ? 1 : 0), clip(s.n, rm - 14, 12, 600), 'v') + (two && lv != null ? T(x1 + 12, ys[k] + 15, fmt(lv, true)) : '') + '</g>';
          });
        notes.forEach((nt, k) => {
          const i = nat(nt), v0 = ss[0].v[clamp(i, 0, N - 1)];
          if (i < 0 || v0 == null || !nt.text) return;
          const px = X(i), py = Y(v0), tw = mw(nt.text, 12);
          const ddx = px - 46 - tw - 14 > x0 ? -46 : 46, ddy = py - 40 > top ? -34 : 34;
          const sx = px + ddx, sy = py + ddy, d = Math.hypot(ddx, ddy);
          p += strokeP(L([[sx, sy], [px - (ddx / d) * 6, py - (ddy / d) * 6]], 0.6, S + 40 + k), SUB, 1.3);
          const ex = ddx < 0 ? sx - 6 : sx + 6;
          p += `<rect x="${r1(ddx < 0 ? ex - tw - 4 : ex - 4)}" y="${r1(sy - 8)}" width="${r1(tw + 8)}" height="16" style="fill:var(--background)"/>` + T(ex, sy, nt.text, 'm', ddx < 0 ? 'end' : 'start');
        });
        if (labels.length) {
          const lw = max(...labels.map((l) => mw(l, 12)));
          thin(N, dx, lw, max(2, round(9 * z))).forEach((j) => labels[j] && (p += T(X(j), bot + 18, labels[j], 'k', j === 0 ? 'start' : j === N - 1 ? 'end' : 'middle')));
        }
        p += `<g class="hv" style="display:none">${strokeP(L([[0, top], [0, bot]], 0.4, S + 50, 8), SUB, 1, 'stroke-dasharray:3 4')}${ss.map((s, k) => fillP(dab(0, 0, 5.6, S + 60 + k), pig(s.c))).join('')}</g>`;
        o += panG(p, z, x0 - 8, x1 + 8, H);
        const marks = [];
        for (let i = 0; i < N; i++) {
          const ok = ss.filter((s) => s.v[i] != null), nt = notes.find((x) => nat(x) === i && x.text);
          const ls = many ? ss.map((s, k) => [s.c, s.n || 'Series ' + (k + 1), s.v[i] == null ? 'No value' : fmt(s.v[i], true)]) : [ss[0].v[i] == null ? 'No value' : fmt(ss[0].v[i], true)];
          const near = (ly) => {
            let b = -1, bd = 1e9;
            ss.forEach((s, k) => s.v[i] != null && abs(Y(s.v[i]) - ly) < bd && ((bd = abs(Y(s.v[i]) - ly)), (b = k)));
            return b < 0 || ly == null ? '' : `[data-s="${b}"]`;
          };
          marks.push(M([X(i) - max(dx, 2) / 2, top - 12, max(dx, 2), bot - top + 34], labels[i] || 'Point ' + (i + 1), nt ? ls.concat(nt.text) : ls, {label: labels[i], index: i, values: ss.map((s) => ({name: s.n, value: s.v[i]}))}, {p: true, side: true, a: [X(i), ok.length ? min(...ok.map((s) => Y(s.v[i]))) : bot, 0, 0], on: many ? near : false}));
        }
        if (many)
          ss.forEach((s, k) => {
            const vv = vals(s);
            marks.push(M(leg ? null : [x1 + 8, ys[k] - 10, rm, two ? 34 : 18], s.n || 'Series ' + (k + 1), vv.length ? ['Last ' + fmt(vv[vv.length - 1], true), 'Low ' + fmt(min(...vv), true), 'High ' + fmt(max(...vv), true)] : [], {name: s.n, values: s.v}, {nk: true, on: `[data-s="${k}"]`}));
          });
        const hover = (k, stg) => {
          const hv = stg.querySelector('.hv');
          if (!hv) return;
          if (k < 0 || k >= N) return (hv.style.display = 'none');
          hv.style.display = '';
          hv.children[0].setAttribute('transform', `translate(${r1(X(k))} 0)`);
          ss.forEach((s, j) => {
            const e = hv.children[j + 1], val = s.v[k];
            e.style.display = val == null ? 'none' : '';
            if (val != null) e.setAttribute('transform', `translate(${r1(X(k))} ${r1(Y(val))})`);
          });
        };
        const d0 = N > 1 ? (x1 - x0) / (N - 1) : 99;
        return {html: (leg ? legend(ss.map((s) => ({n: s.n, c: s.c, line: true, dash: s.dash})), S + 800, N) : '') + svg(W, H, o), marks, hover, pan: [x0, x1], zoom: d0 < 5 ? clamp((24 * (N - 1)) / (x1 - x0), 2, 12) : 1};
      },
    };
  });

  // Rows of bars that lie down, with the names at the left: ranked, and bars that are too many to stand.
  // rows: [{n, v (null draws a dashed stub), c, x (a note)}]. Returns the svg body, the height and the rects of each row.
  function hrows(rows, W, S, fmt, mx, c0) {
    const n = rows.length, pitch = n > 12 ? 28 : 34, bh = n > 12 ? 16 : 18;
    const nameW = min(max(...rows.map((o) => mw(o.n, 13))), W * 0.4), bx0 = nameW + 12;
    const bx1 = W - max(0, ...rows.map((o) => (o.v == null ? 0 : mw(fmt(o.v, true), 13, 600)))) - 16;
    let o = '';
    const rs = rows.map((d, i) => {
      const y = i * pitch + (pitch - bh) / 2, cy = y + bh / 2, w = d.v == null ? 24 : max(3, ((bx1 - bx0) * d.v) / mx);
      let g;
      if (d.v == null) g = strokeP(rect(bx0, y, w, bh, 5, 0.6, S + 30 + i, false, 12), SUB, 1.2, 'stroke-dasharray:3 4') + (d.x ? T(bx0 + w + 8, cy, clip(d.x, W - bx0 - w - 8, 12), 'k') : '');
      else g = fillP(rect(bx0, y, w, bh, 5, 0.7, S + 30 + i, false, 12), pig(d.c ?? c0)) + T(bx0 + w + 8, cy, fmt(d.v, true), 'V');
      o += G(i, g + T(0, cy, clip(d.n, nameW, 13), 'M'));
      return {band: [0, i * pitch, W, pitch], bar: [bx0, y, w, bh]};
    });
    return {o, H: n * pitch, rs};
  }

  // bars: a few things side by side. A missing value is a dashed stub.
  // Past about 28 px a bar, or when the names do not fit under the bars, the bars lie down in rows.
  def('bars', (spec, S) => {
    const it = items(spec, 'bars', true), fmt = mkfmt(spec), c0 = spec.color ?? 1;
    const vals = it.filter((o) => o.v != null).map((o) => o.v);
    if (!vals.length) none('Ink.bars: every value is null.');
    if (vals.some((v) => v < 0)) bad('Bad data', 'Ink.bars: bars start at zero, so a value cannot be below zero. Use Ink.waterfall or Ink.line.');
    const sc = scale(0, spec.max ?? max(...vals), true, ints(vals));
    const say = (d) => (d.v == null ? [d.x || 'No value'] : [fmt(d.v, true)].concat(d.x ? [d.x] : []));
    const item = (d, i) => ({name: d.n, value: d.v, note: d.x, index: i});
    return {
      alt: 'Bar chart. ' + it.map((o) => (o.v == null ? `${o.n} ${o.x || 'no value'}` : list([o], fmt))).join(', ') + '.',
      draw(W) {
        const n = it.length, yl = ceil(max(...sc.t.map((v) => mw(fmt(v), 12)))) + 12, slot0 = (W - 4 - yl) / n;
        if (spec.horizontal ?? (slot0 < 28 || it.filter((d) => fits(d.n, slot0 - 6)).length < n * 0.75)) {
          const h = hrows(it, W, S, fmt, spec.max ?? max(...vals), c0);
          return {html: svg(W, h.H, h.o), marks: it.map((d, i) => M(h.rs[i].band, d.n, say(d), item(d, i), {a: h.rs[i].bar}))};
        }
        const top = 36, bot = top + (W < 420 ? 140 : 160);
        const fr = frame(W, top, bot, sc, fmt, S, 4), {x0, x1, Y} = fr;
        const slot = (x1 - x0) / n, cx = (i) => x0 + slot * (i + 0.5), bw = clamp(slot * 0.48, 12, 64);
        const cl = cats(it.map((o) => o.n), cx, slot, bot + 10, 'm', 400);
        let o = fr.g + cl.o;
        const marks = [];
        it.forEach((d, i) => {
          let g = '', a;
          if (d.v == null) {
            g = strokeP(rect(cx(i) - bw / 2, bot - 22, bw, 22, 6, 0.6, S + 90 + i, true), SUB, 1.2, 'stroke-dasharray:3 4');
            if (d.x) g += T(cx(i), bot - 36, clip(d.x, slot, 12), 'k', 'middle');
            a = [cx(i) - bw / 2, bot - 22, bw, 22];
          } else {
            const y = Y(d.v), t = fmt(d.v, true);
            if (bot - y > 1) g = fillP(rect(cx(i) - bw / 2, y, bw, bot - y, min(6, bw / 4), 0.8, S + 80 + i, true, 12), pig(d.c ?? c0));
            if (mw(t, 12, 600) <= slot) g += T(cx(i), y - 14, t, 'v', 'middle');
            a = [cx(i) - bw / 2, y, bw, bot - y];
          }
          o += G(i, g);
          marks.push(M([cx(i) - slot / 2, top - 24, slot, bot - top + 34 + cl.n * 16], d.n, say(d), item(d, i), {a}));
        });
        return {html: svg(W, bot + 20 + cl.n * 16, o), marks};
      },
    };
  });

  // donut: one brushstroke. Five parts at most. The rest fold into Other.
  // Each piece of paint carries the index of its part, so a part can stand out.
  function brush(S, segs) {
    const CX = 110, CY = 110, R = 84, W = 25, TS = -84, TE = 262, BE = TE - 5, out = [];
    const noise = (seed, amp, fq, wt) => {
      const r = rng(seed), ph = fq.map(() => r() * 6.283);
      return (t) => amp * fq.reduce((s, f, i) => s + wt[i] * sin(((t * PI) / 180) * f + ph[i]), 0);
    };
    const nOut = noise(S + 1, 0.9, [2, 5, 11], [0.55, 0.3, 0.15]), nIn = noise(S + 2, 0.9, [2, 5, 11], [0.55, 0.3, 0.15]), nW = noise(S + 3, 0.07, [3, 7], [0.7, 0.3]);
    const hw = (t) => {
      let h = (W / 2) * (1 + nW(t));
      const s = (t - TS) / 14, e = (TE - t) / 34;
      if (s < 1) h *= 1.06 - 0.06 * s;
      if (e < 1) h *= 0.62 + 0.38 * max(e, 0);
      return h;
    };
    const pt = (t, r) => [CX + r * cos((t * PI) / 180), CY + r * sin((t * PI) / 180)];
    const outer = (t) => pt(t, R + hw(t) + nOut(t)), inner = (t) => pt(t, R - hw(t) + nIn(t));
    const fr = (a, b, st) => {
      const n = max(1, floor(abs(b - a) / (st || 2.2)));
      return Array.from({length: n + 1}, (_, k) => a + ((b - a) * k) / n);
    };
    const poly = (p) => pd(p, true);
    const cap = (t) => {
      const a = (t * PI) / 180, Tn = [-sin(a), cos(a)], N = [cos(a), sin(a)], c = pt(t, R), h = hw(t), r = rng(S + 7);
      return Array.from({length: 13}, (_, k) => {
        const ph = (PI * k) / 12, u = -h * 0.72 * sin(ph) * (1 + (r() - 0.5) * 0.12), v = -h * cos(ph);
        return [c[0] + Tn[0] * u + N[0] * v, c[1] + Tn[1] * u + N[1] * v];
      });
    };
    const strand = (t0, t1, r0, w0, seed) => {
      const rd = rng(seed), drift = (rd() - 0.5) * 2.5, ts = fr(t0, t1, 1.1), l = [], r = [];
      ts.forEach((t, i) => {
        const f = i / (ts.length - 1), h = (w0 / 2) * Math.pow(1 - f, 0.8);
        l.push(pt(t, r0 + drift * f + h));
        r.push(pt(t, r0 + drift * f - h));
      });
      return l.concat(r.reverse());
    };
    const spindle = (t0, t1, r0, w0) => {
      const ts = fr(t0, t1, 1.1), l = [], r = [];
      ts.forEach((t, i) => {
        const h = (w0 / 2) * Math.pow(sin((PI * i) / (ts.length - 1)), 0.7);
        l.push(pt(t, r0 + h));
        r.push(pt(t, r0 - h));
      });
      return l.concat(r.reverse());
    };
    const total = sum(segs.map((s) => s.p)), bounds = [TS];
    let t = TS;
    segs.forEach((s) => bounds.push((t += ((BE - TS) * s.p) / total)));
    const last = segs.length - 1;
    segs.forEach((s, i) => {
      const a = bounds[i], b = bounds[i + 1], o = fr(a, b).map(outer), n = fr(b, a).map(inner);
      let p = o.concat(n);
      if (i === 0) p = cap(a).concat(p);
      if (i === last) {
        const r = rng(S + 11), zig = [];
        for (let k = 0; k < 9; k++) {
          const f = k / 8, tt = b + (k % 2 === 0 ? 1.5 + r() * 2.5 : -0.5);
          zig.push(pt(tt, R - hw(b) + nIn(b) + f * (2 * hw(b) + nOut(b) - nIn(b))));
        }
        p = o.concat(zig, n);
      }
      out.push([poly(p), pig(s.c), i]);
    });
    const alpha = [6, 19, 31, 44, 44, 31, 19, 6];
    for (let i = 1; i < segs.length; i++) {
      const b = bounds[i], ca = segs[i - 1].c, cb = segs[i].c;
      for (let k = 0; k < 8; k++) {
        const t0 = b - 4.5 + (9 * k) / 8, t1 = b - 4.5 + (9 * (k + 1)) / 8 + 0.15;
        out.push([poly(fr(t0, t1, 1.2).map(outer).concat(fr(t1, t0, 1.2).map(inner))), glaze(k < 4 ? cb : ca, alpha[k]), k < 4 ? i : i - 1]);
      }
      const r = rng(S + 100 + i), h = hw(b);
      for (let j = 0; j < 6; j++) {
        const sw = 1.4 + r() * 1.4, rr = R - h + sw + 1.5 + r() * (2 * h - 2 * sw - 3), ln = 4 + r() * 7;
        out.push(j % 2 === 0 ? [poly(strand(b - 2, b + ln, rr, sw, S + 200 + 10 * i + j)), glaze(ca, 80), i - 1] : [poly(strand(b + 2, b - ln, rr, sw, S + 300 + 10 * i + j)), glaze(cb, 80), i]);
      }
    }
    const r = rng(S + 42), dry = 'color-mix(in srgb, var(--background) 38%, transparent)';
    segs.forEach((s, i) => {
      const a = bounds[i] + 5, b = bounds[i + 1] - 5, kn = 1 + (b - a > 60 ? 1 : 0) + (i === last ? 2 : 0);
      for (let k = 0; k < kn; k++) {
        const t0 = a + r() * (b - a) * 0.5, t1 = min(b, t0 + (b - a) * (0.2 + r() * 0.35));
        out.push([poly(spindle(t0, t1, R + (r() - 0.5) * 1.2 * (W / 2), 0.7 + r() * 0.8)), dry, i]);
      }
    });
    const rt = rng(S + 77), h = hw(BE);
    for (let j = 0; j < 6; j++) out.push([poly(strand(BE - 2, BE + 3 + rt() * 9, R - h + ((j + 0.5) / 6) * 2 * h + (rt() - 0.5) * 1.5, 2.2 + rt() * 1.6, S + 500 + j)), pig(segs[last].c), last]);
    out.bounds = bounds;
    return out;
  }
  def('donut', (spec, S) => {
    let it = items(spec, 'donut').filter((o) => o.v > 0);
    if (!it.length) none('Ink.donut: every value is zero.');
    const other = it.filter((o) => /^other$/i.test(o.n)), main = it.filter((o) => !/^other$/i.test(o.n)).sort((a, b) => b.v - a.v);
    it = main.concat(other);
    let rest = [];
    if (it.length > 5) {
      rest = it.slice(4).sort((a, b) => b.v - a.v);
      it = it.slice(0, 4).concat({n: 'Other', v: sum(rest.map((o) => o.v)), c: 6});
    } else if (other.length) rest = other.slice();
    it.forEach((o, i) => (o.c = o.c ?? (/^other$/i.test(o.n) ? 6 : i + 1)));
    const total = sum(it.map((o) => o.v)), fmt = spec.format || spec.unit ? mkfmt(spec) : fmtc;
    const center = spec.center ?? fmt(total), clabel = spec.centerLabel ?? '';
    const pc = (v) => pct(v, total);
    return {
      alt: 'Donut chart. ' + it.map((o) => `${o.n} ${pc(o.v)}`).join(', ') + '.',
      draw() {
        const sh = brush(S, it.map((o) => ({p: o.v, c: o.c}))), bd = sh.bounds;
        const rows = it.map((o, i) => `<div class="mk" data-k="${i}" style="display:flex;align-items:center;gap:10px">${dabSvg(o.c, S + 700 + i)}<span style="flex:1;min-width:0;font:13px/20px var(--font-sans);color:var(--foreground)">${esc(o.n)}</span><span style="flex:none;width:48px;text-align:right;font:600 13px/20px var(--font-sans);color:var(--muted-foreground)">${pc(o.v)}</span><span style="flex:none;width:64px;text-align:right;font:12px/20px var(--font-mono);color:var(--subtle-foreground)">${esc(fmt(o.v, true))}</span></div>`).join('');
        const ctr = `<div style="position:absolute;left:30px;width:160px;top:${clabel ? 84 : 94}px;text-align:center;font:700 26px/32px var(--font-sans);color:var(--foreground);pointer-events:none">${esc(center)}</div>` + (clabel ? `<div style="position:absolute;left:30px;width:160px;top:118px;text-align:center;color:var(--subtle-foreground);pointer-events:none">${esc(clabel)}</div>` : '');
        const marks = it.map((o, i) => {
          const m = (bd[i] + bd[i + 1]) / 2, a = (m * PI) / 180, x = 110 + 84 * cos(a), y = 110 + 84 * sin(a), folded = /^other$/i.test(o.n) && rest.length;
          const ls = [fmt(o.v, true), share(o.v, total)];
          if (folded) {
            rest.slice(0, 8).forEach((q) => ls.push(`${q.n} ${fmt(q.v, true)}`));
            if (rest.length > 8) ls.push(`and ${rest.length - 8} more`);
          }
          return M(null, o.n, ls, {name: o.n, value: o.v, share: o.v / total, parts: folded ? rest.map((q) => ({name: q.n, value: q.v})) : undefined}, {a: [x - 12, y - 12, 24, 24]});
        });
        // A point on the brushstroke finds its part by its angle.
        const pick = (x, y) => {
          const dx = x - 110, dy = y - 110, rr = Math.hypot(dx, dy);
          if (rr < 62 || rr > 106) return -1;
          let t = (Math.atan2(dy, dx) * 180) / PI;
          while (t < bd[0] - 10) t += 360;
          if (t > 262 + 8) return -1;
          for (let i = 0; i < it.length; i++) if (t < bd[i + 1] || i === it.length - 1) return i;
          return -1;
        };
        return {html: `<div style="display:flex;flex-wrap:wrap;align-items:center;gap:20px 40px"><div style="position:relative;flex:none;width:220px;height:220px">${svg(220, 220, sh.map(([d, f, k]) => fillP(d, f, ` class="mk" data-k="${k}"`)).join(''))}${ctr}</div><div style="display:flex;flex-direction:column;gap:10px;flex:1 1 260px;max-width:420px;min-width:0">${rows}</div></div>`, marks, pick};
      },
    };
  });

  // ranked: a list, largest first. Names read in full. Any number of rows: the height grows.
  // top: N shows the first N and a row "Show k more" that opens the rest.
  def('ranked', (spec, S) => {
    const all = items(spec, 'ranked').sort((a, b) => b.v - a.v), top = spec.top > 0 ? floor(spec.top) : all.length, st = {open: false};
    const fmt = mkfmt(spec), mx = max(...all.map((o) => o.v));
    if (!(mx > 0)) none('Ink.ranked: no value is above zero.');
    return {
      alt: 'Ranked bars. ' + list(all, fmt) + '.',
      draw(W, box, v) {
        const it = st.open || all.length <= top ? all : all.slice(0, top), more = all.length - it.length, h = hrows(it, W, S, fmt, mx, spec.color ?? 1);
        const marks = it.map((d, i) => M(h.rs[i].band, d.n, [fmt(d.v, true), `Rank ${i + 1} of ${all.length}`], {name: d.n, value: d.v, rank: i + 1}, {a: h.rs[i].bar}));
        const open = () => {
          st.open = true;
          v.again();
        };
        if (more) marks.push(M(null, `${more} more`, ['Press Enter to show them'], {more}, {act: open}));
        return {
          html: svg(W, h.H, h.o) + (more ? `<button type="button" class="ink-more" tabindex="-1">Show ${more} more</button>` : ''),
          marks,
          after(b, stg) {
            const btn = stg.querySelector('.ink-more');
            if (!btn) return;
            btn.onclick = open;
            marks[marks.length - 1].el = btn;
          },
        };
      },
    };
  });

  // grouped and stacked bars: a category axis and a few series.
  // Past about 28 px a category, or when the names do not fit, the bars lie down in rows.
  function multi(name, stack) {
    def(name, (spec, S) => {
      const ss = seriesOf(spec, name), labels = (spec.labels || []).map(String), G = max(...ss.map((s) => s.v.length)), k = ss.length;
      ss.forEach((s) => checkNums(s.v, name, 'values'));
      ss.forEach((s, i) => (s.c = s.c ?? (stack || k === 1 || k > 3 ? i + 1 : [[6, 1], [6, 2, 1]][k - 2][i])));
      const fmt = mkfmt(spec), tot = (g) => sum(ss.map((s) => max(0, s.v[g] || 0)));
      const tots = Array.from({length: G}, (_, g) => tot(g));
      const top = stack ? max(...tots) : max(...ss.flatMap((s) => s.v.map((x) => x || 0)));
      const sc = scale(0, spec.max ?? top, true, ints(ss.flatMap((s) => s.v)));
      const names = labels.length ? labels : Array.from({length: G}, (_, i) => String(i + 1));
      const say = (g) => ss.map((s, j) => [s.c, s.n || 'Series ' + (j + 1), s.v[g] == null ? 'No value' : fmt(s.v[g], true)]).concat(stack ? ['Total ' + fmt(tots[g], true)] : []);
      const item = (g) => ({label: names[g], index: g, values: ss.map((s) => ({name: s.n, value: s.v[g]})), total: stack ? tots[g] : undefined});
      // A hover on a legend name shows the bars of its series.
      const smarks = () =>
        ss.map((s, j) => {
          const vs = s.v.map((x) => x || 0), hi = vs.indexOf(max(...vs));
          return M(null, s.n || 'Series ' + (j + 1), ['Total ' + fmt(sum(vs), true), `Highest ${fmt(vs[hi], true)} in ${names[hi]}`], {name: s.n, values: s.v}, {nk: true, on: `[data-s="${j}"]`});
        });
      return {
        alt: `${stack ? 'Stacked' : 'Grouped'} bar chart. ` + names.map((n, g) => `${n}: ` + ss.map((s) => `${s.n || 'value'} ${s.v[g] == null ? 'none' : fmt(s.v[g], true)}`).join(', ')).join('. ') + '.',
        draw(W) {
          const yl = ceil(max(...sc.t.map((v) => mw(fmt(v), 12)))) + 12, slot0 = (W - 4 - yl) / G, need = stack ? 28 : max(28, (8 * k + 4 * (k - 1)) / 0.8);
          const leg = k > 1 || ss[0].n ? legend(ss.map((s) => ({n: s.n, c: s.c})), S + 800, G) : '';
          const bar = (g, j, body) => `<g class="mk" data-k="${g}" data-s="${j}">${body}</g>`;
          let o = '';
          const marks = [];
          if (spec.horizontal ?? (slot0 < need || names.filter((n) => fits(n, slot0 - 6)).length < G * 0.75)) {
            const nameW = min(max(...names.map((n) => mw(n, 13))), W * 0.4), bx0 = nameW + 12;
            const endW = stack ? max(...tots.map((t) => mw(fmt(t, true), 13, 600))) : max(0, ...ss.flatMap((s) => s.v.filter((x) => x != null).map((x) => mw(fmt(x, true), 12, 600))));
            const bx1 = W - endW - 14, Xv = (x) => ((bx1 - bx0) * max(0, x)) / ((spec.max ?? top) || 1);
            const bh = stack ? (G > 12 ? 16 : 18) : k > 2 ? 9 : 11, gap = stack ? 2 : 3, pitch = stack ? (G > 12 ? 28 : 34) : k * (bh + gap) - gap + 14;
            names.forEach((n, g) => {
              const y0 = g * pitch + (pitch - (stack ? bh : k * (bh + gap) - gap)) / 2;
              let end = 0;
              if (stack) {
                const nz = ss.map((s, j) => (s.v[g] > 0 ? j : -1)).filter((j) => j >= 0);
                let acc = 0;
                nz.forEach((j) => {
                  const v = ss[j].v[g], x = bx0 + Xv(acc), w = max(1, Xv(v) - (j === nz[nz.length - 1] ? 0 : gap));
                  o += bar(g, j, fillP(rect(x, y0, w, bh, j === nz[nz.length - 1] ? min(5, bh / 4) : 1.5, 0.6, S + 50 + g * 7 + j, false, 12), pig(ss[j].c)));
                  acc += v;
                });
                end = Xv(acc);
                if (tots[g] > 0) o += T(bx0 + end + 8, y0 + bh / 2, fmt(tots[g], true), 'V');
              } else
                ss.forEach((s, j) => {
                  const v = s.v[g];
                  if (v == null) return;
                  const y = y0 + j * (bh + gap), w = max(2, Xv(v));
                  end = max(end, w);
                  o += bar(g, j, fillP(rect(bx0, y, w, bh, min(4, bh / 3), 0.6, S + 50 + g * 7 + j, false, 12), pig(s.c)) + T(bx0 + w + 6, y + bh / 2, fmt(v, true), j === k - 1 ? 'v' : 'm'));
                });
              o += T(0, g * pitch + pitch / 2, clip(n, nameW, 13), 'M');
              marks.push(M([0, g * pitch, W, pitch], n, say(g), item(g), {a: [bx0, y0, max(8, end), stack ? bh : k * (bh + gap) - gap]}));
            });
            return {html: leg + svg(W, G * pitch, o), marks: marks.concat(smarks())};
          }
          const top0 = stack ? 10 : 22, bot = top0 + (W < 420 ? 130 : stack ? 160 : 154);
          const fr = frame(W, top0, bot, sc, fmt, S + 5, 4), {x0, x1, Y} = fr, slot = (x1 - x0) / G, cx = (i) => x0 + slot * (i + 0.5);
          const bw = stack ? clamp(slot * 0.52, 10, 56) : clamp((slot * 0.8 - (k - 1) * 4) / k, 8, 44);
          const cl = cats(names, cx, slot, bot + 10, 'm', 400);
          o = fr.g + cl.o;
          for (let g = 0; g < G; g++) {
            let hy = bot, gw = bw;
            if (stack) {
              let acc = 0;
              const nz = ss.map((s, j) => (s.v[g] > 0 ? j : -1)).filter((j) => j >= 0);
              nz.forEach((j) => {
                const v = ss[j].v[g], yt = Y(acc + v), yb = Y(acc), gap = acc ? 2 : 0;
                o += bar(g, j, fillP(rect(cx(g) - bw / 2, yt, bw, max(1, yb - yt - gap), j === nz[nz.length - 1] ? min(5, bw / 4) : 1.5, 0.6, S + 50 + g * 7 + j, true, 12), pig(ss[j].c)));
                acc += v;
                hy = yt;
              });
            } else {
              gw = k * bw + (k - 1) * 4;
              ss.forEach((s, j) => {
                const v = s.v[g];
                if (v == null) return;
                const bx = cx(g) - gw / 2 + j * (bw + 4), y = Y(v), t = fmt(v, true);
                let body = '';
                if (bot - y > 1) body += fillP(rect(bx, y, bw, bot - y, min(5, bw / 4), 0.7, S + 50 + g * 7 + j, true, 12), pig(s.c));
                if (mw(t, 12, 600) <= bw + 8) body += T(bx + bw / 2, y - 12, t, j === k - 1 ? 'v' : 'm', 'middle');
                o += bar(g, j, body);
                hy = min(hy, y);
              });
            }
            marks.push(M([cx(g) - slot / 2, top0 - 12, slot, bot - top0 + 22 + cl.n * 16], names[g], say(g), item(g), {a: [cx(g) - gw / 2, hy, gw, bot - hy]}));
          }
          return {html: leg + svg(W, bot + 20 + cl.n * 16, o), marks: marks.concat(smarks())};
        },
      };
    });
  }
  multi('grouped', false);
  multi('stacked', true);

  // histogram: how values spread. Equal bins that touch. One marked line.
  def('histogram', (spec, S) => {
    let counts, start, width, vals;
    if (Array.isArray(spec.counts) && spec.counts.length) {
      counts = spec.counts.map(Number);
      checkNums(counts, 'histogram', 'counts');
      start = spec.start ?? 0;
      width = spec.width ?? 1;
    } else {
      vals = (spec.values || []).map(Number).filter(fin);
      if (vals.length < 2) none('Ink.histogram: pass values: [numbers] or counts: [per bin] with start and width.');
      const lo = min(...vals), hi = max(...vals), raw = (hi - lo) / clamp(ceil(Math.log2(vals.length) + 1), 5, 30) || 1;
      const p = Math.pow(10, floor(Math.log10(raw)));
      width = spec.width ?? ([1, 2, 2.5, 5, 10].find((m) => m * p >= raw) * p);
      start = spec.start ?? floor(lo / width + 1e-9) * width;
      counts = Array(floor((hi - start) / width + 1e-9) + 1).fill(0);
      vals.forEach((x) => counts[min(counts.length - 1, floor((x - start) / width + 1e-9))]++);
    }
    const fmt = mkfmt(spec), nb = counts.length, sc = scale(0, max(...counts), true, ints(counts));
    let mark = spec.mark;
    if (mark === 'median' && vals) {
      const s = vals.slice().sort((a, b) => a - b), m = s.length >> 1;
      const at = s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
      mark = {at, text: 'Median ' + fmt(at)};
    }
    return {
      alt: `Histogram of ${sum(counts)} values in ${nb} bins from ${fmt(start)} to ${fmt(start + nb * width)}. Tallest bin: ${fmt(start + counts.indexOf(max(...counts)) * width)} with ${max(...counts)}.` + (mark ? ` ${mark.text || ''}` : ''),
      draw(W) {
        const top = 26, bot = top + (W < 420 ? 130 : 150);
        const fr = frame(W, top, bot, sc, (v) => fnum(v), S, 4), {x0, x1, Y} = fr, bw = (x1 - x0) / nb;
        let o = fr.g;
        const all = sum(counts), marks = [];
        counts.forEach((c, i) => {
          if (c > 0) o += G(i, fillP(rect(x0 + i * bw + 1, Y(c), bw - 2, bot - Y(c), min(3, bw / 3), 0.6, S + 30 + i, true, 12), pig(spec.color ?? 2)));
          const a = start + i * width;
          marks.push(M([x0 + i * bw, top - 8, bw, bot - top + 8], `${fmt(a)} to ${fmt(a + width)}`, ['Count ' + fnum(c), pct(c, all) + ' of all'], {from: a, to: a + width, count: c, index: i}, {a: [x0 + i * bw, c > 0 ? Y(c) : bot, bw, c > 0 ? bot - Y(c) : 0]}));
        });
        if (mark && fin(mark.at)) {
          const mx = x0 + ((mark.at - start) / width) * bw, tw = mw(mark.text || '', 12, 600), right = mx + 8 + tw < W;
          o += strokeP(L([[mx, top - 8], [mx, bot]], 0.5, S + 45, 12), 'var(--foreground)', 1.4, 'stroke-dasharray:4 4');
          if (mark.text) o += T(right ? mx + 8 : mx - 8, top - 8, mark.text, 'v', right ? 'start' : 'end');
        }
        const lw = max(...[start, start + nb * width].map((v) => mw(fmt(v), 12))), need = ceil((lw + 16) / bw), st = [1, 2, 5, 10, 20, 50].find((x) => x >= need) || need;
        for (let e = 0; e <= nb; e += st) {
          const t = fmt(start + e * width), over = x0 + e * bw + mw(t, 12) / 2 > W;
          o += T(over ? W : x0 + e * bw, bot + 18, t, 'k', over ? 'end' : 'middle');
        }
        return {html: svg(W, bot + 36, o), marks};
      },
    };
  });

  // waterfall: how one total became another.
  // data: [["Budget", 60, "total"], ["Scrapes", -18.4], ["Left", null, "total"]]
  def('waterfall', (spec, S) => {
    const it = items(spec, 'waterfall', true), fmt = mkfmt(spec);
    let run = 0, lo = 0, hi = 0;
    it.forEach((d) => {
      d.total = d.x === 'total';
      if (d.total) {
        if (d.v == null) d.v = run;
        d.a = 0;
        d.b = run = d.v;
      } else {
        if (d.v == null) bad('Bad data', `Ink.waterfall: the step "${d.n}" needs a value.`);
        d.a = run;
        d.b = run += d.v;
      }
      lo = min(lo, d.a, d.b);
      hi = max(hi, d.a, d.b);
    });
    const sc = scale(lo, hi, true), lab = (d) => (d.total ? fmt(d.v, true) : (d.v > 0 ? '+' : '') + fmt(d.v, true));
    const say = (d) => (d.total ? [fmt(d.v, true), 'Total'] : [lab(d), `From ${fmt(d.a, true)} to ${fmt(d.b, true)}`]);
    const item = (d, i) => ({name: d.n, value: d.v, from: d.a, to: d.b, total: d.total, index: i});
    const col = (d) => d.c ?? (d.total ? 1 : d.v > 0 ? 2 : 5);
    return {
      alt: 'Waterfall chart. ' + it.map((d) => `${d.n} ${lab(d)}`).join(', ') + '.',
      draw(W) {
        const n = it.length, yl = ceil(max(...sc.t.map((v) => mw(fmt(v), 12)))) + 12, slot0 = (W - 4 - yl) / n;
        if (spec.horizontal ?? (slot0 < 28 || it.filter((d) => fits(d.n, slot0 - 6)).length < n * 0.75)) {
          // Rows: each step lies down, and a dashed line carries the running total to the next row.
          const nameW = min(max(...it.map((d) => mw(d.n, 13))), W * 0.4), bx0 = nameW + 12, bx1 = W - max(...it.map((d) => mw(lab(d), 13, 600))) - 16;
          const X = (v) => bx0 + ((v - sc.lo) / (sc.hi - sc.lo)) * (bx1 - bx0), pitch = n > 12 ? 28 : 34, bh = n > 12 ? 16 : 18;
          let o = '';
          const marks = it.map((d, i) => {
            const y = i * pitch + (pitch - bh) / 2, a = X(min(d.a, d.b)), b = X(max(d.a, d.b));
            o += G(i, fillP(rect(a, y, max(2, b - a), bh, min(4, bh / 4), 0.7, S + 30 + i, false, 12), pig(col(d))) + T(b + 8, y + bh / 2, lab(d), 'V')) + T(0, y + bh / 2, clip(d.n, nameW, 13), 'M');
            if (i < n - 1) o += strokeP(L([[X(d.b), y + bh], [X(d.b), y + pitch]], 0.3, S + 60 + i, 12), SUB, 1, 'stroke-dasharray:2 3');
            return M([0, i * pitch, W, pitch], d.n, say(d), item(d, i), {a: [a, y, max(2, b - a), bh]});
          });
          return {html: svg(W, n * pitch, o), marks};
        }
        const top = 26, bot = top + (W < 420 ? 140 : 164);
        const fr = frame(W, top, bot, sc, fmt, S, 4), {x0, x1, Y} = fr, slot = (x1 - x0) / it.length, cx = (i) => x0 + slot * (i + 0.5), bw = clamp(slot * 0.6, 12, 60);
        const cl = cats(it.map((d) => d.n), cx, slot, bot + 10, 'm', 400);
        let o = fr.g + cl.o;
        const marks = it.map((d, i) => {
          const a = Y(max(d.a, d.b)), b = Y(min(d.a, d.b)), t = lab(d);
          o += G(i, fillP(rect(cx(i) - bw / 2, a, bw, max(2, b - a), min(4, bw / 4), 0.7, S + 30 + i, false, 12), pig(col(d))) + (mw(t, 12, 600) <= slot ? T(cx(i), a - 12, t, 'v', 'middle') : ''));
          if (i < it.length - 1) o += strokeP(L([[cx(i) + bw / 2, Y(d.b)], [cx(i + 1) - bw / 2, Y(d.b)]], 0.3, S + 60 + i, 12), SUB, 1, 'stroke-dasharray:2 3');
          return M([cx(i) - slot / 2, top - 20, slot, bot - top + 30 + cl.n * 16], d.n, say(d), item(d, i), {a: [cx(i) - bw / 2, a, bw, max(2, b - a)]});
        });
        return {html: svg(W, bot + 20 + cl.n * 16, o), marks};
      },
    };
  });

  // treemap: what fills a whole when there are many parts. Area is size.
  function squarify(vals, x, y, w, h) {
    const k = (w * h) / sum(vals), a = vals.map((v) => v * k), out = [];
    let i = 0;
    while (i < a.length) {
      const side = min(w, h), worst = (r, s) => max((side * side * max(...r)) / (s * s), (s * s) / (side * side * min(...r)));
      let row = [a[i]], s = a[i], j = i + 1;
      while (j < a.length && worst(row.concat(a[j]), s + a[j]) <= worst(row, s)) {
        row = row.concat(a[j]);
        s += a[j++];
      }
      const th = s / side;
      let off = 0;
      if (w >= h) {
        row.forEach((v) => {
          out.push([x, y + off, th, v / th]);
          off += v / th;
        });
        x += th;
        w -= th;
      } else {
        row.forEach((v) => {
          out.push([x + off, y, v / th, th]);
          off += v / th;
        });
        y += th;
        h -= th;
      }
      i = j;
    }
    return out;
  }
  def('treemap', (spec, S) => {
    const it = items(spec, 'treemap').filter((o) => o.v > 0).sort((a, b) => b.v - a.v), fmt = mkfmt(spec);
    if (!it.length) none('Ink.treemap: no value is above zero.');
    const total = sum(it.map((o) => o.v));
    return {
      alt: 'Treemap. ' + list(it, fmt) + '.',
      draw(W) {
        // The height grows with the count, and more at a narrow width, so the tiles stay near square.
        const n = it.length, H = spec.height ?? round(clamp(max(clamp(W * 0.375, 200, 340), (80 * Math.sqrt(n) - 45) * (W < 500 ? 1.3 : 1)), 200, 680)), g = 4;
        // Tiles too small to see join one slate tile "N more". Its readout lists them.
        let keep = n, rs;
        for (let pass = 0; pass < 6; pass++) {
          const vs = it.slice(0, keep).map((o) => o.v);
          if (keep < n) vs.push(sum(it.slice(keep).map((o) => o.v)));
          rs = squarify(vs, -g / 2, -g / 2, W + g, H + g);
          const j = rs.findIndex(([, , w, h], i) => i < keep && (min(w, h) - g < 10 || (w * h) / (W * H) < 0.003));
          if (j < 0 || j < 1) break;
          keep = j;
        }
        const tiles = it.slice(0, keep), rest = it.slice(keep);
        if (rest.length) tiles.push({n: rest.length + ' more', v: sum(rest.map((o) => o.v)), c: 6, more: rest});
        let o = '';
        const marks = [];
        rs.forEach(([x, y, w, h], i) => {
          x += g / 2;
          y += g / 2;
          w -= g;
          h -= g;
          const d = tiles[i];
          let t = fillP(rect(x, y, w, h, min(7, w / 4, h / 4), 0.8, S + 40 + i, false, 10), pig(d.c ?? (i < 5 ? i + 1 : 6)));
          // Name and value when both fit, the name only when only it fits, and nothing when neither fits.
          const big = w > 90 && h > 50, p = big ? 12 : 8, tw = w - p - 8, nm = clip(d.n, tw, big ? 13 : 12, 700), ok = nm.length > 3 || nm === d.n;
          if (w >= 44 && h >= 34 && ok) {
            t += T(x + p, y + (big ? 18 : 16), nm, big ? 'O' : 'o');
            if (mw(fmt(d.v, true), 12) <= tw) t += T(x + p, y + (big ? 36 : 32), fmt(d.v, true), 'o', '', ' style="font-weight:400"');
          } else if (w >= 30 && h >= 20 && ok) t += T(x + 6, y + h / 2, clip(d.n, w - 10, 12, 700), 'o');
          o += G(i, t);
          const ls = [fmt(d.v, true), share(d.v, total)];
          if (d.more) {
            d.more.slice(0, 8).forEach((q) => ls.push(`${q.n} ${fmt(q.v, true)}`));
            if (d.more.length > 8) ls.push(`and ${d.more.length - 8} more`);
          }
          marks.push(M([x - g / 2, y - g / 2, w + g, h + g], d.n, ls, {name: d.n, value: d.v, share: d.v / total, parts: d.more ? d.more.map((q) => ({name: q.n, value: q.v})) : undefined}, {follow: true, a: [x + w / 2, y + h / 2, 0, 0]}));
        });
        return {html: svg(W, H, o), marks};
      },
    };
  });

  // scatter: how two numbers move together. A pencil trend line.
  // points: [[x, y, name], ...]. notes: [{at: index or [x, y], text}] in gold.
  // Many points get smaller dabs, and past 400 plain circles. The readout takes the nearest point.
  def('scatter', (spec, S) => {
    const pts = (Array.isArray(spec.points) ? spec.points : []).map((p) => (Array.isArray(p) ? {x: +p[0], y: +p[1], n: p[2] == null ? '' : String(p[2])} : {x: +p.x, y: +p.y, c: p.color, n: String(p.name ?? p.label ?? '')}));
    if (pts.length < 2) none('Ink.scatter: pass points: [[x, y], ...] with two points or more.');
    if (pts.some((p) => !fin(p.x) || !fin(p.y))) bad('Bad data', 'Ink.scatter: every point needs a number x and a number y.');
    const xs = pts.map((p) => p.x), ys = pts.map((p) => p.y), fy = mkfmt(spec), fx = mkfmt({unit: spec.xUnit, format: spec.xFormat});
    const zero = (a) => min(...a) >= 0 && min(...a) < 0.5 * max(...a);
    const sx = scale(spec.xMin ?? min(...xs), spec.xMax ?? max(...xs), zero(xs), ints(xs)), sy = scale(spec.min ?? min(...ys), spec.max ?? max(...ys), zero(ys), ints(ys));
    const n = pts.length, mx = sum(xs) / n, my = sum(ys) / n, sxx = sum(xs.map((v) => (v - mx) ** 2));
    const slope = sxx ? sum(pts.map((p) => (p.x - mx) * (p.y - my))) / sxx : 0, trend = (spec.trend ?? n >= 5) && sxx > 0;
    const notes = (spec.notes || []).map((o) => ({p: Array.isArray(o.at) ? {x: o.at[0], y: o.at[1]} : pts[o.at], text: o.text})).filter((o) => o.p);
    const order = pts.map((_, i) => i).sort((a, b) => pts[a].x - pts[b].x || pts[a].y - pts[b].y), rank = [];
    order.forEach((i, k) => (rank[i] = k));
    const xn = spec.xLabel || 'x', yn = spec.yLabel || 'y', many = n > 400, dr = clamp(4.2 * Math.pow(60 / n, 0.3), 2, 4.2);
    return {
      alt: `Scatter chart of ${n} points. x from ${fx(min(...xs))} to ${fx(max(...xs))}, y from ${fy(min(...ys))} to ${fy(max(...ys))}.` + notes.map((o) => ' ' + o.text + '.').join(''),
      zoomable: true,
      draw(W, box, v) {
        const top = 10, bot = top + (W < 420 ? 170 : 196), fr = frame(W, top, bot, sy, fy, S, 8), {x0, x1, Y} = fr, z = v.z, pw = (x1 - x0) * z;
        const X = (val) => x0 + ((val - sx.lo) / (sx.hi - sx.lo)) * pw;
        let o = fr.g, p = '';
        if (trend) p += strokeP(L([[X(min(...xs)), Y(my + slope * (min(...xs) - mx))], [X(max(...xs)), Y(my + slope * (max(...xs) - mx))]].map((q) => [q[0], clamp(q[1], top, bot)]), 0.6, S + 9, 10), SUB, 1.2, 'stroke-dasharray:5 5');
        pts.forEach((q, i) => {
          const k = rank[i], c = pig(q.c ?? spec.color ?? 1);
          p += many ? `<circle class="mk" data-k="${k}" cx="${r1(X(q.x))}" cy="${r1(Y(q.y))}" r="${dr}" style="fill:${c}"/>` : fillP(dab(X(q.x), Y(q.y), dr, S + 100 + i, 6), c, ` class="mk" data-k="${k}"`);
        });
        notes.forEach((nt, i) => {
          const px = X(nt.p.x), py = Y(nt.p.y), tw = mw(nt.text, 12), left = px - 14 - tw > x0;
          p += fillP(dab(px, py, 5.2, S + 300 + i, 7), pig(3)) + T(left ? px - 14 : px + 14, py, nt.text, 'm', left ? 'end' : 'start');
        });
        const tk = z > 1 ? steps(sx.lo, sx.hi, round(4 * z)) : sx.t;
        let lx = -1e9;
        tk.forEach((val) => {
          const t = fx(val), tw = mw(t, 12), over = X(val) + tw / 2 > x0 + pw + 8;
          if (X(val) - tw / 2 < lx + 12) return;
          lx = X(val) + tw / 2;
          p += T(over ? x0 + pw + 8 : X(val), bot + 18, t, 'k', over ? 'end' : 'middle');
        });
        p += `<circle class="hv" r="${r1(dr + 3.5)}" style="display:none;fill:none;stroke:var(--foreground);stroke-width:1.5"/>`;
        o += panG(p, z, x0 - 10, W, bot + 36);
        const marks = order.map((i) => {
          const q = pts[i], nt = notes.find((x) => x.p === q);
          return M(null, q.n || `${fx(q.x)}, ${fy(q.y)}`, [`${xn} ${fx(q.x)}`, `${yn} ${fy(q.y)}`].concat(nt ? [nt.text] : []), {name: q.n, x: q.x, y: q.y, index: i}, {p: true, a: [X(q.x) - dr, Y(q.y) - dr, 2 * dr, 2 * dr], on: false});
        });
        // The nearest point within 14 px.
        const pick = (px, py) => {
          let b = -1, bd = 196;
          order.forEach((i, k) => {
            const d = (X(pts[i].x) - px) ** 2 + (Y(pts[i].y) - py) ** 2;
            if (d < bd) (bd = d), (b = k);
          });
          return b;
        };
        const hover = (k, stg) => {
          const c = stg.querySelector('circle.hv');
          if (!c) return;
          c.style.display = k < 0 ? 'none' : '';
          if (k >= 0) c.setAttribute('cx', r1(X(pts[order[k]].x))), c.setAttribute('cy', r1(Y(pts[order[k]].y)));
        };
        return {html: svg(W, bot + 36, o), marks, pick, hover, pan: [x0, x1], zoom: many ? 6 : 1};
      },
    };
  });

  // slope: before and after for a few things. One line carries the story.
  // data: [["sayso", 18, 34], ...]. labels: ["September", "October"].
  def('slope', (spec, S) => {
    const rows = (Array.isArray(spec.data) ? spec.data : []).map((d) => (Array.isArray(d) ? {n: String(d[0]), a: +d[1], b: +d[2]} : {n: String(d.name ?? d.label), a: +(d.before ?? d.from), b: +(d.after ?? d.to), c: d.color}));
    if (!rows.length) none('Ink.slope: pass data: [["name", before, after], ...].');
    if (rows.some((r) => !fin(r.a) || !fin(r.b))) bad('Bad data', 'Ink.slope: each row needs a number before and a number after.');
    const hl = spec.highlight, f = mkfmt(spec), big = rows.reduce((m, r, i) => (abs(r.b - r.a) > abs(rows[m].b - rows[m].a) ? i : m), 0);
    rows.forEach((r, i) => (r.hi = hl == null ? i === big : [].concat(hl).some((h) => h === i || h === r.n)));
    const sc = scale(0, max(...rows.flatMap((r) => [r.a, r.b])), true), lab = spec.labels || [];
    return {
      alt: 'Slope chart. ' + rows.map((r) => `${r.n} ${f(r.a, true)} to ${f(r.b, true)}`).join(', ') + '.',
      draw(W) {
        const top = 34, bot = top + max(clamp(rows.length * 30 + 42, 150, 400), rows.length * 19 + 20);
        const vw = max(...rows.flatMap((r) => [mw(f(r.a, true), 13, 600), mw(f(r.b, true), 13, 600)]));
        const nw = min(max(...rows.map((r) => mw(r.n, 13, 600))), max(40, (W - 2 * (vw + 22) - 110) / 2)), side = nw + 8 + vw + 14;
        const len = clamp(W - 2 * side - 12, 100, 260), xl = W / 2 - len / 2, xr = W / 2 + len / 2;
        const Y = (v) => bot - ((v - sc.lo) / (sc.hi - sc.lo)) * (bot - top);
        let o = '';
        [xl, xr].forEach((x, i) => {
          o += `<path class="g" d="${L([[x, top - 10], [x, bot + 4]], 0.4, S + i, 14)}"/>`;
          if (lab[i]) o += T(x, 8, lab[i], 'm', 'middle', ' style="font-weight:600"');
        });
        const ya = spread(rows.map((r) => Y(r.a)), 17, top - 8, bot + 8), yb = spread(rows.map((r) => Y(r.b)), 17, top - 8, bot + 8);
        rows.forEach((r, i) => {
          const c = pig(r.c ?? (r.hi ? spec.color ?? 1 : 6)), st = r.hi ? '' : ' style="font-weight:600"', tc = r.hi ? 'V' : 'M';
          const nm = clip(r.n, nw, 13, r.hi ? 600 : 400);
          o += G(i, strokeP(L([[xl, Y(r.a)], [xr, Y(r.b)]], 0.6, S + 10 + i, 8), c, r.hi ? 2.4 : 1.5) + fillP(dab(xl, Y(r.a), r.hi ? 4.6 : 3.6, S + 20 + i, 7), c) + fillP(dab(xr, Y(r.b), r.hi ? 4.6 : 3.6, S + 30 + i, 7), c) + T(xl - 14, ya[i], f(r.a, true), tc, 'end', st) + T(xl - 14 - vw - 8, ya[i], nm, r.hi ? 'V' : 'M', 'end') + T(xr + 14, yb[i], f(r.b, true), tc, 'start', st) + T(xr + 14 + vw + 8, yb[i], nm, r.hi ? 'V' : 'M', 'start'));
        });
        const marks = rows.map((r, i) => {
          const d = r.b - r.a, sg = d > 0 ? '+' : '';
          return M(null, r.n, [`${lab[0] || 'Before'} ${f(r.a, true)}`, `${lab[1] || 'After'} ${f(r.b, true)}`, `Change ${sg}${f(d, true)}` + (r.a ? ` (${sg}${round((d / abs(r.a)) * 100)}%)` : '')], {name: r.n, before: r.a, after: r.b, index: i}, {a: [(xl + xr) / 2 - 2, (Y(r.a) + Y(r.b)) / 2 - 2, 4, 4]});
        });
        // The nearest line under the pointer, or the nearest label beside the lines.
        const pick = (px, py) => {
          let b = -1, bd = 10;
          rows.forEach((r, i) => {
            const y = px < xl ? ya[i] : px > xr ? yb[i] : Y(r.a) + ((Y(r.b) - Y(r.a)) * (px - xl)) / (xr - xl), d = abs(y - py);
            if (d < bd) (bd = d), (b = i);
          });
          return b;
        };
        return {html: svg(W, bot + 10, o), marks, pick};
      },
    };
  });

  // boxplot: how a few groups spread.
  // data: [{name, values: [...]}] or [{name, stats: [min, q1, median, q3, max], outliers: [...]}]
  const quant = (s, q) => {
    const p = (s.length - 1) * q, i = floor(p);
    return s[i] + (s[min(i + 1, s.length - 1)] - s[i]) * (p - i);
  };
  def('boxplot', (spec, S) => {
    const g = (Array.isArray(spec.data) ? spec.data : []).map((d, i) => {
      let st = d.stats, out = d.outliers || [];
      if (!st && Array.isArray(d.values) && d.values.length) {
        const s = d.values.map(Number).filter(fin).sort((a, b) => a - b), q1 = quant(s, 0.25), q3 = quant(s, 0.75), k = 1.5 * (q3 - q1);
        const inn = s.filter((x) => x >= q1 - k && x <= q3 + k);
        st = [inn[0], q1, quant(s, 0.5), q3, inn[inn.length - 1]];
        out = s.filter((x) => x < q1 - k || x > q3 + k);
      }
      if (!st || st.length !== 5 || st.some((x) => !fin(+x))) bad('Bad data', `Ink.boxplot: item ${i} needs values: [...] or stats: [min, q1, median, q3, max].`);
      return {n: String(d.name ?? d.label ?? ''), s: st.map(Number), o: out.map(Number), c: d.color ?? i + 1};
    });
    if (!g.length) none('Ink.boxplot: pass data: [{name, values: [...]}, ...].');
    const f = mkfmt(spec), all = g.flatMap((d) => d.s.concat(d.o)), sc = scale(min(...all), max(...all), min(...all) >= 0, ints(all));
    return {
      alt: 'Box plot. ' + g.map((d) => `${d.n} median ${f(d.s[2], true)}`).join(', ') + '.',
      draw(W) {
        const x0 = min(max(...g.map((d) => mw(d.n, 13))), W * 0.3) + 14, x1 = W - 14, X = (v) => x0 + ((v - sc.lo) / (sc.hi - sc.lo)) * (x1 - x0);
        const yl = 38 + (g.length - 1) * 52, H = yl + 58;
        let o = '';
        sc.t.forEach((v, i) => {
          o += `<path class="g" d="${L([[X(v), 8], [X(v), yl + 26]], 0.4, S + i, 14)}"/>`;
          const t = f(v), over = X(v) + mw(t, 12) / 2 > W;
          o += T(over ? W : X(v), yl + 42, t, 'k', over ? 'end' : 'middle');
        });
        g.forEach((d, i) => {
          const yc = 38 + i * 52, [mn, q1, md, q3, mx] = d.s, c = pig(d.c), line = (a, b, y0, y1, k) => strokeP(L([[a, y0], [b, y1]], 0.4, S + 50 + i * 9 + k, 8), SUB, 1.4);
          let b = T(0, yc, clip(d.n, x0 - 14, 13), 'M');
          b += line(X(mn), X(q1), yc, yc, 0) + line(X(q3), X(mx), yc, yc, 1) + line(X(mn), X(mn), yc - 7, yc + 7, 2) + line(X(mx), X(mx), yc - 7, yc + 7, 3);
          b += strokeP(rect(X(q1), yc - 14, max(3, X(q3) - X(q1)), 28, 5, 0.6, S + 90 + i, false, 8), c, 1.6, 'fill:var(--card)');
          b += strokeP(L([[X(md), yc - 14], [X(md), yc + 14]], 0.3, S + 100 + i, 4), c, 3.2);
          d.o.forEach((v, k) => (b += fillP(dab(X(v), yc, 3.2, S + 120 + i * 5 + k, 7), c)));
          o += G(i, b + T(X(md), yc - 26, f(md, true), 'v', 'middle'));
        });
        const marks = g.map((d, i) => {
          const yc = 38 + i * 52, [mn, q1, md, q3, mx] = d.s, os = d.o.slice().sort((a, b) => a - b);
          return M([0, yc - 26, W, 52], d.n, [`Median ${f(md, true)}`, `Middle half ${f(q1, true)} to ${f(q3, true)}`, `Low ${f(mn, true)}, high ${f(mx, true)}`].concat(os.length ? [`Outliers ${os.slice(0, 6).map((v) => f(v, true)).join(', ')}${os.length > 6 ? ` and ${os.length - 6} more` : ''}`] : []), {name: d.n, min: mn, q1, median: md, q3, max: mx, outliers: os}, {a: [X(q1), yc - 14, max(3, X(q3) - X(q1)), 28]});
        });
        return {html: svg(W, H, o), marks};
      },
    };
  });

  // funnel: how many go on at each step.
  def('funnel', (spec, S) => {
    const it = items(spec, 'funnel'), f = mkfmt(spec), top = max(...it.map((o) => o.v));
    if (!(top > 0)) none('Ink.funnel: no value is above zero.');
    return {
      alt: 'Funnel. ' + list(it, f) + '.',
      draw(W) {
        const nw = min(max(...it.map((o) => mw(o.n, 13))), W * 0.28) + 12, mwid = max(100, (W - nw - 8) * 0.86), cx = nw + mwid / 2;
        let o = '';
        const marks = [];
        it.forEach((d, i) => {
          const y = i * 46 + 4, w = max(24, (mwid * d.v) / top), c = d.c ?? (i === it.length - 1 && it.length > 1 ? 2 : 1);
          let b = fillP(rect(cx - w / 2, y, w, 34, 7, 0.8, S + 40 + i, false, 10), pig(c)) + T(0, y + 17, clip(d.n, nw - 12, 13), 'M') + T(cx, y + 17, f(d.v, true), 'O', 'middle');
          if (i) {
            const full = `${round((100 * d.v) / it[i - 1].v)}% of ${it[i - 1].n.toLowerCase()}`, short = round((100 * d.v) / it[i - 1].v) + '%', ax = cx + w / 2 + 12;
            b += T(ax, y + 17, ax + mw(full, 12) < W ? full : short, 'k');
          }
          o += G(i, b);
          marks.push(M([0, y - 6, W, 46], d.n, [f(d.v, true)].concat(i ? [`${pct(d.v, it[i - 1].v)} of ${it[i - 1].n.toLowerCase()}`] : []).concat(i > 1 ? [`${pct(d.v, it[0].v)} of ${it[0].n.toLowerCase()}`] : []), {name: d.n, value: d.v, index: i}, {a: [cx - w / 2, y, w, 34]}));
        });
        return {html: svg(W, it.length * 46, o), marks};
      },
    };
  });

  // sankey: where an amount went. links: [["from", "to", value], ...]
  // Labels that collide move apart. A layout wider than the box scales down, and past 11 px text it can zoom.
  def('sankey', (spec, S) => {
    const links = (Array.isArray(spec.links) ? spec.links : []).map((l) => ({s: String(l[0]), t: String(l[1]), v: +l[2]}));
    if (!links.length) none('Ink.sankey: pass links: [["from", "to", value], ...].');
    if (links.some((l) => !(l.v > 0))) bad('Bad data', 'Ink.sankey: every link needs a value above zero.');
    const nm = new Map(), node = (n) => nm.get(n) || nm.set(n, {n, i: 0, o: 0, l: 0}).get(n);
    links.forEach((l) => ((node(l.s).o += l.v), (node(l.t).i += l.v)));
    for (let k = 0; k < nm.size; k++) links.forEach((l) => (node(l.t).l = max(node(l.t).l, node(l.s).l + 1)));
    const ns = [...nm.values()], Lc = max(...ns.map((x) => x.l)) + 1;
    if (Lc < 2) bad('Bad data', 'Ink.sankey: the links must lead from one name to another.');
    ns.forEach((x) => (x.v = max(x.i, x.o)));
    const cols = Array.from({length: Lc}, (_, l) => ns.filter((x) => x.l === l).sort((a, b) => b.v - a.v));
    cols[0].forEach((x, i) => (x.c = spec.colors?.[x.n] ?? (i < 5 ? i + 1 : 6)));
    for (let l = 1; l < Lc; l++)
      cols[l].forEach((x) => {
        const top = links.filter((k) => k.t === x.n).sort((a, b) => b.v - a.v)[0];
        x.c = l === Lc - 1 ? 6 : node(top.s).c;
      });
    const order = (n) => cols[node(n).l].indexOf(node(n));
    const sorted = links.slice().sort((a, b) => order(a.s) - order(b.s) || order(a.t) - order(b.t));
    const vt = (x, unit) => (spec.format ? String(spec.format(x.v)) : fmtc(x.v) + (unit && spec.unit ? ' ' + spec.unit : ''));
    const flat = cols.flat();
    flat.forEach((x, i) => (x.k = i));
    const gw = (l) => max(64, ...cols[l].map((x) => mw(x.n, 12) + 30));
    return {
      alt: 'Sankey diagram. ' + links.map((l) => `${l.s} to ${l.t} ${vt({v: l.v}, true)}`).join(', ') + '.',
      whole: true,
      zoomable: true,
      dom: true,
      draw(W) {
        const gap = 10, nw = 10, tot = max(...cols.map((c) => sum(c.map((x) => x.v)))), K = max(150, 42 * max(...cols.map((c) => c.length))) / tot;
        const colH = (c) => sum(c.map((x) => x.v * K)) + gap * (c.length - 1), H = max(...cols.map(colH));
        const lw = max(...cols[0].map((x) => max(mw(x.n, 13, 600), mw(vt(x, true), 12)))), rw = max(...cols[Lc - 1].map((x) => mw(x.n, 13) + 6 + mw(vt(x), 13)));
        let need = lw + 14 + nw + 10 + rw + 4;
        for (let l = 1; l < Lc - 1; l++) need += gw(l);
        need += 70;
        const lx = min(lw + 14, W * 0.3), rw2 = min(rw, W * 0.34), rx = W - rw2 - nw - 10;
        const xs = cols.map((_, l) => lx + ((rx - lx) * l) / (Lc - 1)), cg = (rx - lx) / (Lc - 1);
        cols.forEach((c) => {
          let y = (H - colH(c)) / 2;
          c.forEach((x) => {
            x.y = y;
            x.cs = x.ct = 0;
            y += x.v * K + gap;
          });
          // Label positions: at the middle of each node, moved apart where they would touch.
          const ly = spread(c.map((x) => x.y + (x.v * K) / 2), c === cols[0] ? 34 : 17, c === cols[0] ? 9 : 8, H - (c === cols[0] ? 9 : 8));
          c.forEach((x, i) => (x.ly = ly[i]));
        });
        let o = '';
        const marks = [];
        sorted.forEach((l, k) => {
          const a = node(l.s), b = node(l.t), h = l.v * K, y0 = a.y + a.cs, y1 = b.y + b.ct, xa = xs[a.l] + nw, xb = xs[b.l], xm = (xa + xb) / 2;
          a.cs += h;
          b.ct += h;
          o += P(`M${r1(xa)} ${r1(y0)}C${r1(xm)} ${r1(y0)} ${r1(xm)} ${r1(y1)} ${r1(xb)} ${r1(y1)}L${r1(xb)} ${r1(y1 + h)}C${r1(xm)} ${r1(y1 + h)} ${r1(xm)} ${r1(y0 + h)} ${r1(xa)} ${r1(y0 + h)}Z`, 'fill:' + glaze(a.c, 38), ` class="mk" data-k="${flat.length + k}" data-a="${a.k}" data-b="${b.k}"`);
          marks[flat.length + k] = M(null, `${l.s} to ${l.t}`, [vt(l, true), pct(l.v, a.o) + ' of ' + l.s, pct(l.v, b.i) + ' of what reaches ' + l.t], {from: l.s, to: l.t, value: l.v}, {follow: true, a: [xm - 1, (y0 + y1) / 2, 2, h], on: `[data-k="${flat.length + k}"],[data-n="${a.k}"],[data-n="${b.k}"]`});
        });
        cols.forEach((c, l) =>
          c.forEach((x, i) => {
            const cy = x.ly, hh = max(3, x.v * K);
            let t = fillP(rect(xs[l], x.y, nw, hh, 3, 0.4, S + 60 + l * 9 + i, false, 10), pig(x.c));
            if (l === 0) t += T(xs[l] - 12, cy - 9, clip(x.n, lx - 14, 13, 600), 'V', 'end') + T(xs[l] - 12, cy + 9, vt(x, true), 'k', 'end');
            else if (l === Lc - 1) t += `<text class="M" x="${r1(xs[l] + nw + 10)}" y="${r1(cy)}" dy=".35em">${esc(clip(x.n, rw2 - mw(vt(x), 13) - 6, 13))}<tspan dx="6" style="fill:var(--subtle-foreground)">${esc(vt(x))}</tspan></text>`;
            else t += T(xs[l] + nw + 6, cy, clip(x.n, cg - nw - 22, 12), 'm', 'start', ' style="paint-order:stroke;stroke:var(--background);stroke-width:3px"');
            o += `<g class="mk" data-n="${x.k}">${t}</g>`;
            const ls = (x.i ? [`In ${vt({v: x.i}, true)}`] : []).concat(x.o ? [`Out ${vt({v: x.o}, true)}`] : []);
            const lw0 = l === 0 ? lx : l === Lc - 1 ? 0 : 0, rw0 = l === Lc - 1 ? rw2 + 10 : l === 0 ? 0 : cg - nw - 16;
            marks[x.k] = M([xs[l] - 4 - lw0, min(x.y, cy - 10) - 2, nw + 8 + lw0 + rw0, max(hh, 20) + 4], x.n, ls, {name: x.n, in: x.i, out: x.o}, {a: [xs[l], x.y, nw, hh], on: `[data-n="${x.k}"],[data-a="${x.k}"],[data-b="${x.k}"]`});
          })
        );
        return {html: svg(W, H, o), marks, need};
      },
    };
  });

  // goal: how far along against a target or a limit.
  // data: [["Monid spend", 41.2, 60], {label, value, target, limit, limitLabel, unit, text}]
  def('goal', (spec, S) => {
    const rows = (Array.isArray(spec.data) ? spec.data : []).map((d) => {
      const r = Array.isArray(d) ? {label: d[0], value: d[1], target: d[2], ...(d[3] || {})} : d;
      if (!fin(+r.value) || !(+r.target > 0)) bad('Bad data', `Ink.goal: "${r.label}" needs a number value and a target above zero.`);
      const u = r.unit ?? spec.unit, fm = r.format ?? spec.format, v = +r.value, t = +r.target;
      const f = mkfmt({unit: u, format: fm}), cur = /^[$€£¥]$/.test(u || ''), suffix = u && !fm && !cur && !/^(%|K|M|B|k|×)$/.test(u);
      return {n: String(r.label ?? ''), v, t, lim: r.limit == null ? null : +r.limit, ll: r.limitLabel, c: r.color ?? 1, f, text: r.text ?? (suffix ? `${fnum(v)} of ${fnum(t)} ${u}` : `${f(v, true)} of ${f(t)}`)};
    });
    if (!rows.length) none('Ink.goal: pass data: [["name", value, target], ...].');
    return {
      alt: 'Progress. ' + rows.map((r) => `${r.n}: ${r.text}`).join('; ') + '.',
      draw(W) {
        const lw = min(max(...rows.map((r) => mw(r.n, 13))), W * 0.35), vw = min(max(...rows.map((r) => mw(r.text, 13, 600))), W * 0.4);
        const stack = W - lw - vw - 28 < 100, pitch = stack ? 62 : 48;
        let o = '';
        const marks = rows.map((r, i) => {
          const tx0 = stack ? 5 : lw + 10, tx1 = stack ? W - 5 : W - vw - 18, y = i * pitch + (stack ? 40 : 22), mx = max(r.t, r.lim ?? 0), X = (a) => tx0 + (clamp(a, 0, mx) / mx) * (tx1 - tx0);
          let b = stack ? T(0, y - 22, clip(r.n, W * 0.55, 13), 'M') + T(W, y - 22, r.text, 'V', 'end') : T(0, y, clip(r.n, lw, 13), 'M') + T(tx1 + 18, y, r.text, 'V', 'start');
          b += strokeP(L([[tx0, y], [tx1, y]], 0.5, S + 10 + i, 10), 'var(--input)', 10);
          if (r.v > 0) b += strokeP(L([[tx0, y], [X(r.v), y]], 0.7, S + 20 + i, 8), pig(r.c), 10);
          if (r.lim != null) {
            b += strokeP(L([[X(r.lim), y - 11], [X(r.lim), y + 11]], 0.3, S + 30 + i, 4), pig(5), 2);
            if (r.ll) b += T(X(r.lim), y + 22, r.ll, 'k', 'middle', ' style="fill:var(--chart-5)"');
          }
          o += G(i, b);
          const ls = [r.text, pct(r.v, r.t) + ' of the target'];
          if (r.lim != null) ls.push((r.v > r.lim ? 'Over the limit of ' : 'Limit ') + r.f(r.lim, true) + (r.ll && r.v <= r.lim ? `, ${r.ll}` : ''));
          return M([0, y - pitch / 2, W, pitch], r.n, ls, {name: r.n, value: r.v, target: r.t, limit: r.lim}, {a: [tx0, y - 5, max(4, X(r.v) - tx0), 10]});
        });
        return {html: svg(W, rows.length * pitch + (rows.some((r) => r.ll) ? 8 : 0), o), marks};
      },
    };
  });

  // ---------- dates for calendar and timeline (UTC, so every renderer agrees)
  const DAY = 864e5;
  const pdate = (s) => (typeof s === 'number' ? s : Date.parse(s));
  const fdate = (ms) => DF({month: 'short', day: 'numeric'}).format(ms);

  // calendar: how often, day by day. One dab per day, its size is the count.
  // data: {"2026-07-01": 3, ...} or [["2026-07-01", 3], ...]
  // A long range wraps into rows of whole months, so it never scrolls sideways.
  def('calendar', (spec, S) => {
    const raw = Array.isArray(spec.data) ? spec.data : Object.entries(spec.data || {}), val = new Map();
    raw.forEach((e) => {
      const [k, v] = Array.isArray(e) ? e : [e.date ?? e.day, e.value ?? e.count], d = floor(pdate(k) / DAY);
      if (!fin(d) || !fin(+v)) bad('Bad data', `Ink.calendar: "${k}" must be a date like 2026-07-01 with a number.`);
      val.set(d, (val.get(d) || 0) + +v);
    });
    if (!val.size) none('Ink.calendar: pass data: {"2026-07-01": 3, ...}.');
    const days = [...val.keys()], from = spec.from ? floor(pdate(spec.from) / DAY) : min(...days), to = spec.to ? floor(pdate(spec.to) / DAY) : max(...days);
    const wd = (d) => (d + 3) % 7, w0 = from - wd(from), mx = max(...val.values()), total = sum([...val.values()]);
    const best = [...val.entries()].sort((a, b) => b[1] - a[1])[0];
    const names = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'], yrs = new Date(from * DAY).getUTCFullYear() !== new Date(to * DAY).getUTCFullYear();
    const fd = (d) => DF({weekday: 'short', month: 'short', day: 'numeric', year: yrs ? 'numeric' : undefined}).format(d * DAY);
    const months = [];
    for (let d = from; d <= to; ) {
      const t = new Date(d * DAY), e = floor(Date.UTC(t.getUTCFullYear(), t.getUTCMonth() + 1, 1) / DAY) - 1;
      months.push([d, min(e, to)]);
      d = e + 1;
    }
    const wk = (a, b) => floor((b - (a - wd(a))) / 7) + 1;
    const say = (v) => (v ? (spec.unit ? `${fnum(v)} ${spec.unit}` : 'Count ' + fnum(v)) : 'None');
    return {
      alt: `Calendar of ${val.size} days with activity, ${fnum(total)} in total, from ${fdate(from * DAY)} to ${fdate(to * DAY)}. Busiest day: ${fdate(best[0] * DAY)} with ${fnum(best[1])}.` + (spec.note ? ' ' + spec.note : ''),
      draw(W) {
        const x0 = ceil(mw('Wed', 12)) + 10, avail = W - x0;
        let rows;
        for (let n = 1; ; n++) {
          const per = ceil(months.length / n), rs = [];
          for (let i = 0; i < months.length; i += per) rs.push([months[i][0], months[min(i + per, months.length) - 1][1]]);
          if (n >= months.length || avail / max(...rs.map(([a, b]) => wk(a, b))) >= 14) {
            rows = rs;
            break;
          }
        }
        const cell = min(26, avail / max(...rows.map(([a, b]) => wk(a, b)))), sc = cell / 26, sizes = [0, 3.6, 5.6, 7.6, 9.8], pitch = 7 * cell + 40;
        let o = '';
        const marks = [];
        rows.forEach(([a, b], r) => {
          const y0 = 22 + r * pitch, r0 = a - wd(a), nw = wk(a, b);
          let last = -99, lw = 0;
          for (let w = 0; w < nw; w++) {
            for (let d = 0; d < 7; d++) {
              const day = r0 + w * 7 + d;
              if (day < a || day > b) continue;
              const cx = x0 + w * cell + cell / 2, cy = y0 + d * cell + cell / 2, v = val.get(day) || 0;
              if (v <= 0) o += `<circle cx="${r1(cx)}" cy="${r1(cy)}" r="1.6" style="fill:var(--input)"/>`;
              else o += fillP(dab(cx, cy, sizes[clamp(ceil((4 * v) / mx), 1, 4)] * sc, S + 100 + day - w0, 6), pig(spec.color ?? 1));
              marks[day - from] = M([cx - cell / 2, cy - cell / 2, cell, cell], fd(day), [say(v)], {date: new Date(day * DAY).toISOString().slice(0, 10), value: v}, {a: [cx - 5, cy - 5, 10, 10], on: false});
            }
            let first = null;
            for (let d = 0; d < 7 && first == null; d++) {
              const x = r0 + w * 7 + d;
              if (x >= a && x <= b && new Date(x * DAY).getUTCDate() === 1) first = x;
            }
            if (first == null && w === 0) first = a;
            const ml = first == null ? '' : DF({month: 'short', year: yrs && (w === 0 || new Date(first * DAY).getUTCMonth() === 0) ? 'numeric' : undefined}).format(first * DAY);
            if (ml && x0 + w * cell - last > max(34, lw + 10)) {
              o += T(x0 + w * cell + 4, y0 - 14, ml, 'k');
              last = x0 + w * cell;
              lw = mw(ml, 12);
            }
          }
          [0, 2, 4].forEach((d) => (o += T(0, y0 + d * cell + cell / 2, names[d], 'k')));
        });
        const ly = 22 + (rows.length - 1) * pitch + 7 * cell + 14, note = spec.note ? String(spec.note) : '', nx = x0 + 214, wide = nx + mw(note, 12) <= W;
        o += T(x0, ly + 8, 'Fewer', 'k');
        [1, 2, 3, 4].forEach((s, k) => (o += fillP(dab(x0 + 56 + k * 24, ly + 8, sizes[s], S + 990 + k, 6), pig(spec.color ?? 1))));
        o += T(x0 + 150, ly + 8, 'More', 'k');
        if (note) o += wide ? T(nx, ly + 8, note, 'm') : T(x0, ly + 32, note, 'm');
        o += `<circle class="hv" r="${r1(cell * 0.46)}" style="display:none;fill:none;stroke:var(--foreground);stroke-width:1.5"/>`;
        const hover = (k, stg) => {
          const c = stg.querySelector('circle.hv');
          if (!c) return;
          const a = k >= 0 && marks[k] && marks[k].a;
          c.style.display = a ? '' : 'none';
          if (a) c.setAttribute('cx', r1(a[0] + 5)), c.setAttribute('cy', r1(a[1] + 5));
        };
        // Up and down move a day, left and right a week, as the grid shows them.
        const nav = (k, key) => clamp(k + {ArrowUp: -1, ArrowDown: 1, ArrowLeft: -7, ArrowRight: 7}[key], 0, to - from);
        return {html: svg(W, ly + 22 + (note && !wide ? 24 : 0), o), marks, hover, nav};
      },
    };
  });

  // week: a schedule. Days across, hours down. A gold dab marks a clash.
  // days: ["Mon 3", ...]. events: [{day, start, end, title, color}] (hours as 9.5 or "09:30")
  const hours = (h) => (typeof h === 'string' ? +h.split(':')[0] + (+h.split(':')[1] || 0) / 60 : +h);
  def('week', (spec, S) => {
    const days = (spec.days || []).map(String);
    if (!days.length) none('Ink.week: pass days: ["Mon 3", "Tue 4", ...] and events.');
    const ev = (spec.events || []).map((e, i) => {
      const d = typeof e.day === 'number' ? e.day : days.indexOf(String(e.day)), a = hours(e.start), b = hours(e.end);
      if (d < 0 || d >= days.length || !fin(a) || !fin(b) || b <= a) bad('Bad data', `Ink.week: event ${i} needs a day from days, and a start and end hour with the end after the start.`);
      return {d, a, b, t: String(e.title ?? ''), c: e.color ?? 1, slot: 0, of: 1};
    });
    if (!ev.length) none('Ink.week: pass events: [{day, start, end, title}].');
    const clashes = [];
    for (let d = 0; d < days.length; d++) {
      const es = ev.filter((e) => e.d === d).sort((a, b) => a.a - b.a || a.b - b.b);
      let cl = [], end = -1;
      const flush = () => {
        if (!cl.length) return;
        const cols = [];
        cl.forEach((e) => {
          let k = cols.findIndex((x) => x <= e.a);
          if (k < 0) k = cols.length;
          cols[k] = e.b;
          e.slot = k;
        });
        cl.forEach((e) => (e.of = cols.length));
        if (cols.length > 1 && spec.clash !== false) clashes.push({d, a: cl[0].a});
        cl = [];
      };
      es.forEach((e) => {
        if (e.a >= end) flush();
        cl.push(e);
        end = cl.length === 1 ? e.b : max(end, e.b);
      });
      flush();
    }
    const h0 = spec.from ?? floor(min(...ev.map((e) => e.a))), h1 = spec.to ?? ceil(max(...ev.map((e) => e.b)));
    const hl = (h) => (spec.hours24 ? String(h).padStart(2, '0') + ':00' : `${h % 12 || 12} ${h % 24 < 12 ? 'AM' : 'PM'}`);
    const tm = (x) => {
      const h = floor(x + 1e-6), m = String(round((x - h) * 60)).padStart(2, '0');
      return spec.hours24 ? String(h).padStart(2, '0') + ':' + m : `${h % 12 || 12}:${m} ${h % 24 < 12 ? 'AM' : 'PM'}`;
    };
    const short = min(...ev.map((e) => e.b - e.a));
    return {
      alt: 'Week schedule. ' + days.map((dn, d) => dn + ': ' + (ev.filter((e) => e.d === d).sort((a, b) => a.a - b.a).map((e) => `${e.t} ${hl(floor(e.a))}`).join(', ') || 'nothing')).join('. ') + '.' + (spec.note ? ' ' + spec.note : ''),
      draw(W) {
        // A short event gets room for a line of text: the hour grows, up to 56 px.
        const hh = clamp(14 / short, W < 480 ? 28 : 32, 56), x0 = ceil(mw('10 AM', 12)) + 12, dw = (W - x0) / days.length, top = 26, Yh = (h) => top + (h - h0) * hh;
        let o = '';
        for (let h = h0; h <= h1; h++) {
          o += strokeP(L([[x0, Yh(h)], [W, Yh(h)]], 0.4, S + h, 24), 'var(--border)', 1);
          if (h % 2 === 0) o += T(x0 - 8, Yh(h), hl(h), 'k', 'end');
        }
        days.forEach((d, i) => (o += T(x0 + i * dw + dw / 2, 8, clip(d, dw - 4, 12, 600), 'm', 'middle', ' style="font-weight:600"')));
        const marks = [];
        ev.forEach((e, i) => {
          const w = (dw - 8) / e.of, x = x0 + e.d * dw + 4 + e.slot * w, y = Yh(e.a) + 1.5, h = (e.b - e.a) * hh - 3;
          let b = fillP(rect(x, y, w - 2, h, 5, 0.6, S + 30 + i, false, 10), pig(e.c));
          // A title cut to one letter is no help: one line then, or nothing, and the readout has it.
          const bit = (l) => /…$/.test(l) && l.length < 4;
          let ls = w - 14 >= 18 ? wrap(e.t, w - 14, 12, 600, max(1, floor((h - 4) / 15))) : [];
          if (ls.some(bit)) ls = [clip(e.t, w - 14, 12, 600)];
          if (ls.length && bit(ls[0])) ls = [];
          ls.forEach((l, j) => (b += T(x + 7, h < 20 ? y + h / 2 : y + 13 + j * 15, l, 'o', '', ' style="font-weight:600"')));
          o += G(i, b);
          const ov = ev.filter((q) => q !== e && q.d === e.d && q.a < e.b && q.b > e.a).map((q) => q.t);
          marks.push(M([x, y, w - 2, h], e.t, [days[e.d], `${tm(e.a)} to ${tm(e.b)}`].concat(ov.length ? ['Overlaps ' + ov.join(', ')] : []), {title: e.t, day: days[e.d], start: e.a, end: e.b}, {}));
        });
        const ord = marks.map((_, i) => i).sort((a, b) => ev[a].d - ev[b].d || ev[a].a - ev[b].a);
        clashes.forEach((c, i) => (o += fillP(dab(x0 + (c.d + 1) * dw - 14, Yh(c.a) - 4, 5, S + 200 + i, 8), pig(3))));
        const H = Yh(h1) + (spec.note ? 34 : 10);
        if (spec.note) o += T(x0, H - 10, spec.note, 'm');
        return {html: svg(W, H, o), marks, order: ord};
      },
    };
  });

  // timeline: a plan against the calendar.
  // rows: [{label, start, end, state: "done" | "now" | "next" | "milestone"}] with dates like "2026-10-24"
  // The names stay at the left. A long range or a short span can zoom the time axis.
  def('timeline', (spec, S) => {
    const rows = (spec.rows || []).map((r, i) => {
      const a = pdate(r.start), b = r.end == null ? a : pdate(r.end), st = r.state || (r.end == null ? 'milestone' : 'next');
      if (!fin(a) || !fin(b)) bad('Bad data', `Ink.timeline: row ${i} needs a start like "2026-10-24" (and an end for a span).`);
      return {n: String(r.label ?? ''), a, b: max(a, b), st, c: r.color};
    });
    if (!rows.length) none('Ink.timeline: pass rows: [{label, start, end, state}].');
    const dates = (spec.rows || []).some((r) => typeof r.start === 'string'), today = spec.today == null ? null : pdate(spec.today);
    const lo = spec.from != null ? pdate(spec.from) : min(...rows.map((r) => r.a), today ?? Infinity), hi0 = spec.to != null ? pdate(spec.to) : max(...rows.map((r) => r.b), today ?? -Infinity), hi = spec.to != null ? hi0 : hi0 + (hi0 - lo) * 0.12;
    const fl = (v) => (dates ? fdate(v) : fnum(v)), span = (hi - lo) / DAY;
    const spans = rows.filter((r) => r.b > r.a).map((r) => r.b - r.a), shortest = spans.length ? min(...spans) : hi - lo;
    return {
      alt: 'Timeline. ' + rows.map((r) => `${r.n}: ${r.st === 'milestone' ? fl(r.a) : fl(r.a) + ' to ' + fl(r.b)}, ${r.st}`).join('; ') + '.',
      zoomable: true,
      draw(W, box, v) {
        const lw = min(max(...rows.map((r) => mw(r.n, 13, 600))), W * 0.4), x0 = lw + 12, x1 = W - 4, z = v.z, pw = (x1 - x0) * z, X = (t) => x0 + ((t - lo) / (hi - lo)) * pw;
        const n = rows.length, pitch = n > 16 ? 26 : 32, bh = n > 16 ? 16 : 18, end = 30 + n * pitch;
        let ticks = [];
        if (dates) {
          const room = pw / 74, step = [1, 2, 7, 14].find((s) => span / s <= room);
          if (step) for (let t = step === 7 ? lo + (((8 - new Date(lo).getUTCDay()) % 7) * DAY) : lo; t <= hi; t += step * DAY) ticks.push(t);
          else {
            // Month starts, every k months when they would touch.
            const k = [1, 2, 3, 6, 12].find((m) => span / 30.4 / m <= room) || 12;
            let i = 0;
            for (let d = new Date(lo); d.getTime() <= hi; d = new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1)))
              if (d.getTime() >= lo && (d.getTime() === lo || i++ % k === 0)) ticks.push(d.getTime());
            if (ticks.length > 1 && X(ticks[1]) - X(ticks[0]) < 60) ticks.shift();
          }
        } else ticks = (z > 1 ? steps(lo, hi, round(4 * z)) : scale(lo, hi, false).t).filter((t) => t >= lo && t <= hi);
        let o = '', p = '';
        ticks.forEach((t, k) => {
          p += `<path class="g" d="${L([[X(t), 22], [X(t), end - 4]], 0.4, S + k, 12)}"/>` + T(X(t), 8, fl(t), 'k', 'middle');
        });
        const marks = rows.map((r, i) => {
          const y = 30 + i * pitch, w = max(8, X(r.b) - X(r.a));
          o += T(0, y + bh / 2, clip(r.n, lw, 13, 600), r.st === 'now' ? 'V' : 'M');
          let b, a;
          if (r.st === 'milestone') {
            const lx = X(r.a) + 18, fit = lx + mw(fl(r.a), 12) <= x0 + pw + 4;
            b = fillP(dab(X(r.a) + 4, y + bh / 2, 7, S + 50 + i), pig(r.c ?? 3)) + T(fit ? lx : X(r.a) - 10, y + bh / 2, fl(r.a), 'm', fit ? 'start' : 'end');
            a = [X(r.a) - 3, y + bh / 2 - 7, 14, 14];
          } else {
            b = r.st === 'next' ? strokeP(rect(X(r.a), y, w, bh, 6, 0.7, S + 60 + i, false, 12), pig(r.c ?? 1), 1.3, 'stroke-dasharray:4 4') : fillP(rect(X(r.a), y, w, bh, 6, 0.7, S + 60 + i, false, 12), pig(r.c ?? (r.st === 'done' ? 2 : 1)));
            a = [X(r.a), y, w, bh];
          }
          p += G(i, b);
          const len = dates ? round((r.b - r.a) / DAY) : 0;
          return M([-1e4, y - (pitch - bh) / 2, 2e4, pitch], r.n, [r.st === 'milestone' ? fl(r.a) : `${fl(r.a)} to ${fl(r.b)}` + (dates ? `, ${len} ${len === 1 ? 'day' : 'days'}` : ''), r.st[0].toUpperCase() + r.st.slice(1)], {name: r.n, start: r.a, end: r.b, state: r.st}, {p: true, a});
        });
        let H = end + 6;
        if (today != null && today >= lo && today <= hi) {
          p += strokeP(L([[X(today), 18], [X(today), end + 6]], 0.4, S + 90, 12), 'var(--foreground)', 1.4) + T(X(today), end + 16, 'Today', 'v', 'middle');
          H = end + 24;
        }
        o += panG(p, z, x0 - 4, W, H);
        const ppd = (x1 - x0) / max(span, 1e-9), sp = (shortest / (hi - lo)) * (x1 - x0);
        return {html: svg(W, H, o), marks, pan: [x0, x1], zoom: (dates && ppd < 6) || sp < 10 ? clamp(max(dates ? 16 / ppd : 1, 24 / max(sp, 0.5)), 2, 16) : 1};
      },
    };
  });

  // sequence: messages between parts, top to bottom.
  // parts: ["You", "Hub"]. messages: [["You", "Hub", "Remind me"], ["Hub", "You", "Done", "reply"]]
  // Many parts scale down a little, and past 11 px text the chart can zoom. The rows keep their height.
  def('sequence', (spec, S) => {
    const parts = (spec.parts || []).map(String), idx = (x) => (typeof x === 'number' ? x : parts.indexOf(String(x)));
    if (parts.length < 2) none('Ink.sequence: pass parts: ["You", "Hub", ...] with two names or more.');
    const ms = (spec.messages || []).map((m, i) => {
      const a = idx(m[0]), b = idx(m[1]);
      if (a < 0 || b < 0 || a >= parts.length || b >= parts.length) bad('Bad data', `Ink.sequence: message ${i} must go from one name in parts to another.`);
      return {a, b, t: String(m[2] ?? ''), r: m[3] === 'reply' || m[3] === true};
    });
    if (!ms.length) none('Ink.sequence: pass messages: [["You", "Hub", "text"], ...].');
    // The width where no name and no message is cut. A name too long for the widest box does not count.
    const ok = (p) => parts.every((q) => whole2(q, p < 72 ? p - 10 : min(112, p - 10) - 12, p < 72 ? 12 : 13, 600) || !whole2(q, 100, 13, 600)) && ms.every((m) => m.a === m.b || whole2(m.t, max(60, abs(m.b - m.a) * p - 8), 12, 400));
    const need = () => 8 + parts.length * least(ok, 40, 600);
    return {
      alt: 'Sequence diagram of ' + parts.join(', ') + '. ' + ms.map((m) => `${parts[m.a]} to ${parts[m.b]}: ${m.t}${m.r ? ' (reply)' : ''}`).join('. ') + '.',
      whole: true,
      zoomable: true,
      draw(W, box, v) {
        // Narrow parts get tighter boxes and 12 px names.
        const n = parts.length, pitch = (W - 8) / n, xs = (i) => 4 + pitch * (i + 0.5), fp = ((v.fit || W) - 8) / n, tight = (p) => p < 72;
        const bw = tight(pitch) ? pitch - 4 : min(112, pitch - 10), hz = tight(pitch) ? 12 : 13, pad = tight(pitch) ? 6 : 12, hw = (p) => (tight(p) ? p - 4 - 6 : min(112, p - 10) - 12);
        const heads = parts.map((p) => wrap(p, bw - pad, hz, 600, 2)), bh = parts.some((p, i) => heads[i].length > 1 || wrap(p, hw(fp), tight(fp) ? 12 : 13, 600, 2).length > 1) ? 48 : 34;
        let cur = bh + 38, o = '';
        const rows = ms.map((m) => {
          const self = m.a === m.b, span = self ? 120 : max(60, abs(xs(m.b) - xs(m.a)) - 8), ls = wrap(m.t, span, 12, 400, 2);
          const y = cur + (max(ls.length, wrap(m.t, self ? 120 : max(60, abs(m.b - m.a) * fp - 8), 12, 400, 2).length) - 1) * 14;
          cur = y + 36;
          return {m, ls, y, self};
        });
        const H = cur - 12;
        parts.forEach((p, i) => {
          o += strokeP(rect(xs(i) - bw / 2, 0, bw, bh, 8, 0.6, S + i, false, 10), 'var(--input)', 1.3, 'fill:var(--card)');
          heads[i].forEach((l, j) => (o += T(xs(i), bh / 2 + (j - (heads[i].length - 1) / 2) * 15, l, 'V', 'middle', hz < 13 ? ' style="font-size:12px"' : '')));
          o += strokeP(L([[xs(i), bh + 6], [xs(i), H]], 0.4, S + 10 + i, 14), 'var(--input)', 1, 'stroke-dasharray:3 5');
        });
        const marks = rows.map(({m, ls, y, self}, i) => {
          const dir = m.b > m.a ? 1 : -1, xa = xs(m.a) + (self ? 5 : dir * 5), xb = self ? xs(m.a) + 44 : xs(m.b) - dir * 6;
          let b = strokeP(arrow(xa, y, xb, y, 7, 0.6, S + 30 + i), m.r ? SUB : 'var(--muted-foreground)', 1.5, m.r ? 'stroke-dasharray:4 4' : '');
          ls.forEach((l, j) => (b += T((xa + xb) / 2, y - 12 - (ls.length - 1 - j) * 14, l, m.r ? 'm' : '', 'middle', m.r ? '' : ' style="fill:var(--foreground)"')));
          o += G(i, b);
          const x = min(xa, xb), w = max(abs(xb - xa), self ? 60 : 0), top = y - 20 - (ls.length - 1) * 14;
          return M([x - 4, top, w + 8, y + 10 - top], m.t || 'Message ' + (i + 1), [self ? `${parts[m.a]} to itself` : `${parts[m.a]} to ${parts[m.b]}`, m.r ? 'Reply' : '', `Message ${i + 1} of ${ms.length}`], {text: m.t, from: parts[m.a], to: parts[m.b], reply: m.r, index: i}, {a: [x, top, w, y + 4 - top]});
        });
        // A part name, which a narrow box may cut.
        parts.forEach((p, i) => {
          const c = ms.filter((m) => m.a === i || m.b === i).length;
          marks.push(M([xs(i) - bw / 2, 0, bw, bh], p, [`${c} ${c === 1 ? 'message' : 'messages'}`], {name: p, index: i}, {nk: true, on: false}));
        });
        return {html: svg(W, H, o), marks, need: need()};
      },
    };
  });

  // tree: parts within parts, left to right.
  // {name: "homelab", children: [{name: "Storage", children: [{name: "truenas", detail: "Offline"}]}]}
  // A wide tree scales down a little, then cuts its longest names. Past 11 px text it can zoom.
  def('tree', (spec, S) => {
    const root = spec.root || spec;
    if (!root || root.name == null) none('Ink.tree: pass {name, children: [{name, detail, children}]}.');
    const nodes = [];
    let maxD = 0;
    const walk = (n, d, c, par) => {
      const o = {n: String(n.name), det: n.detail ? String(n.detail) : '', d, c, par, kids: [], i: nodes.length};
      nodes.push(o);
      maxD = max(maxD, d);
      (n.children || []).forEach((k, i) => o.kids.push(walk(k, d + 1, d === 0 ? k.color ?? (i % 6) + 1 : k.color ?? c, o)));
      return o;
    };
    const top = walk(root, 0, root.color ?? 1, null);
    return {
      alt: 'Tree. ' + (function say(n) {
        return n.n + (n.det ? ` (${n.det})` : '') + (n.kids.length ? ': ' + n.kids.map(say).join(', ') : '');
      })(top) + '.',
      whole: true,
      zoomable: true,
      draw(W) {
        const D = maxD + 1, sz = (d) => (d ? 13 : 15), wt = (d) => [700, 600, 400][min(d, 2)], off = (d) => (d ? 12 : 14);
        const Lw = Array.from({length: D}, (_, d) => max(...nodes.filter((n) => n.d === d).map((n) => mw(n.n, sz(d), wt(d))))), dw = max(0, ...nodes.map((n) => (n.det ? mw(n.det, 12) : 0)));
        const width = () => 8 + sum(Lw.slice(0, -1).map((l, d) => off(d) + l + 18)) + off(D - 1) + Lw[D - 1] + 4, need = width() + (D - 1) * 24;
        if (need > W) {
          // Cut the longest names first, down to 40 px a level.
          const fix = need - sum(Lw), c = least((x) => fix + sum(Lw.map((l) => min(l, x))) > W, 40, max(...Lw)) - 1;
          Lw.forEach((l, d) => (Lw[d] = min(l, max(40, c))));
        }
        const fixed = width();
        const hasDet = dw > 0, room = (extra) => (D > 1 ? (W - fixed - extra) / (D - 1) : 0);
        const side = hasDet && room(dw + 16) >= 24, g = clamp(room(side ? dw + 16 : 0), 24, 100);
        const dx = [8];
        for (let d = 0; d < D - 1; d++) dx.push(dx[d] + off(d) + Lw[d] + 8 + g + 10);
        const wide = !side && hasDet && nodes.some((n) => n.det && !n.kids.length && wrap(n.det, W - dx[n.d] - off(n.d), 12, 400, 2).length > 1), pitch = wide ? 56 : nodes.filter((n) => !n.kids.length).length > 16 ? 30 : 40;
        let y = 22;
        const place = (n) => {
          if (!n.kids.length) {
            n.y = y;
            y += pitch;
          } else {
            n.kids.forEach(place);
            n.y = (n.kids[0].y + n.kids[n.kids.length - 1].y) / 2;
          }
        };
        if (top.kids.length)
          top.kids.forEach((b) => {
            place(b);
            y += 4;
          });
        else place(top);
        if (top.kids.length) top.y = sum(top.kids.map((k) => k.y)) / top.kids.length;
        const lastY = max(...nodes.map((n) => n.y));
        let o = '';
        nodes.forEach((n) => {
          if (n.par) {
            const p = n.par, xa = dx[p.d] + off(p.d) + Lw[p.d] + 8, xb = dx[n.d] - 10, w = xb - xa;
            o += strokeP(curve(xa, p.y, xb, n.y, 0.5, S + n.i * 3), pig(n.c), n.d === 1 ? 1.8 : 1.3, '', ` class="mk" data-k="${n.i}"`);
          }
        });
        const marks = nodes.map((n) => {
          const x = dx[n.d], lx = x + off(n.d), below = n.det && !side && !n.kids.length, nm = clip(n.n, Lw[n.d] + 2, sz(n.d), wt(n.d));
          let b = fillP(dab(x, n.y, n.d === 0 ? 7 : n.d === 1 ? 5.2 : 3.4, S + 500 + n.i), pig(n.c));
          b += T(lx, n.y + (below ? -8 : 0), nm, n.d === 0 ? 'h' : n.d === 1 ? 'V' : 'M');
          let ex = mw(nm, sz(n.d), wt(n.d)), lines = 0;
          if (n.det && side) (b += T(lx + Lw[n.d] + 16, n.y, n.det, 'k')), (ex = Lw[n.d] + 16 + dw);
          else if (n.det) wrap(n.det, W - lx, 12, 400, 2).forEach((l, j) => ((b += T(lx, n.y + (n.kids.length ? 12 : 9) + j * 14, l, 'k')), (ex = max(ex, mw(l, 12))), (lines = j + 1)));
          o += G(n.i, b);
          const t0 = n.y - (below ? 16 : 10), h = (below ? 8 : 10) + 10 + lines * 14 + (below ? 0 : 0);
          return M([x - 9, t0, off(n.d) + ex + 13, max(20, h)], n.n, [n.det, n.kids.length ? `${n.kids.length} ${n.kids.length === 1 ? 'child' : 'children'}` : '', n.par ? 'Part of ' + n.par.n : ''], {name: n.n, detail: n.det, children: n.kids.length, depth: n.d}, {a: [x - 6, t0, off(n.d) + mw(nm, sz(n.d), wt(n.d)) + 6, below ? 16 : 20]});
        });
        // Up and down follow the list, left goes to the parent and right to the first child.
        const nav = (k, key) => (key === 'ArrowLeft' ? (nodes[k].par ? nodes[k].par.i : k) : key === 'ArrowRight' ? (nodes[k].kids[0] ? nodes[k].kids[0].i : k) : -1);
        return {html: svg(W, lastY + 24, o), marks, nav, need};
      },
    };
  });

  // flow: steps left to right with boxes and arrows. Down the page when it is narrow.
  // steps: [["You", "this Mac"], ...]. arrows: ["message", ...]. back: "the answer comes back". highlight: 3
  // Too many steps for one row wrap into rows that read like text.
  def('flow', (spec, S) => {
    const st = (spec.steps || []).map((s) => (Array.isArray(s) ? {n: String(s[0]), sub: s[1] ? String(s[1]) : ''} : typeof s === 'string' ? {n: s, sub: ''} : {n: String(s.name), sub: s.sub ? String(s.sub) : ''}));
    if (st.length < 2) none('Ink.flow: pass steps: [["You", "this Mac"], ["Hub", "VPS"], ...] with two steps or more.');
    const ar = (spec.arrows || []).map(String), back = spec.back ? String(spec.back) : '', hi = spec.highlight, n = st.length;
    const say = (i) => M(null, st[i].n, [st[i].sub, `Step ${i + 1} of ${n}`, ar[i] ? 'Then ' + ar[i] : ''], {name: st[i].n, sub: st[i].sub, index: i});
    // The way back: a dashed lapis line from the last step to the left of the first, then its text.
    const backL = (pts, x, W, H, S) => {
      const ls = wrap(back, W - 4, 12, 400, 2);
      return {o: strokeP(L(pts, 0.6, S + 40, 6), 'var(--chart-1)', 1.5, 'stroke-dasharray:4 5') + strokeP(L([[x - 12, pts[pts.length - 1][1] - 6], [x - 5, pts[pts.length - 1][1]], [x - 12, pts[pts.length - 1][1] + 6]], 0.3, S + 41, 2), 'var(--chart-1)', 1.5) + ls.map((l, j) => T(0, H + 18 + j * 16, l, 'k', 'start', ' style="fill:var(--chart-1)"')).join(''), H: H + 10 + ls.length * 16};
    };
    return {
      alt: 'Flow. ' + st.map((s, i) => s.n + (s.sub ? ` (${s.sub})` : '') + (i < n - 1 ? ' to' + (ar[i] ? ` (${ar[i]})` : '') : '')).join(' ') + '.',
      whole: true,
      zoomable: true,
      draw(W, box, v) {
        const bh = 60, F = v.fit || W, marks = [], boxS = (x, y, w, i) => {
          const c = i === hi ? 'var(--chart-1)' : 'var(--input)';
          let o = strokeP(rect(x + 1, y, w - 2, bh, 10, 0.7, S + i, false, 10), c, 1.4, 'fill:var(--card)');
          if (st[i].sub) o += T(x + w / 2, y + 21, clip(st[i].n, w - 12, 13, 600), 'V', 'middle') + T(x + w / 2, y + 40, clip(st[i].sub, w - 12, 12), 'k', 'middle');
          else o += T(x + w / 2, y + bh / 2, clip(st[i].n, w - 12, 13, 600), 'V', 'middle');
          marks[i] = Object.assign(say(i), {r: [x, y, w, bh]});
          return G(i, o);
        };
        let o = '';
        const per = floor((F - (back ? 24 : 0) + 44) / 136);
        if (spec.direction ? spec.direction === 'right' : F >= n * 92 + (n - 1) * 44) {
          const bw = clamp((W - 2 - (n - 1) * 44) / n, 80, 112), gap = (W - 2 - n * bw) / (n - 1), y = 40, xs = (i) => i * (bw + gap);
          st.forEach((s, i) => {
            o += boxS(xs(i), y, bw, i);
            if (i < n - 1) {
              o += strokeP(arrow(xs(i) + bw + 6, y + bh / 2, xs(i + 1) - 6, y + bh / 2, 7, 0.8, S + 20 + i), SUB, 1.5);
              if (ar[i]) o += T(xs(i) + bw + gap / 2, y + 16, clip(ar[i], gap + 6, 12), 'm', 'middle');
            }
          });
          let H = y + bh + 14;
          if (back) {
            const xw = xs(n - 1) + bw / 2, xy = xs(0) + bw / 2, b = y + bh + 6, low = y + bh + 34;
            o += strokeP(L([[xw, b], [xw, low - 10], [xw - 10, low], [xy + 10, low], [xy, low - 10], [xy, b + 2]], 0.7, S + 40, 6), 'var(--chart-1)', 1.5, 'stroke-dasharray:4 5');
            o += strokeP(L([[xy - 5, b + 8], [xy, b + 1], [xy + 5, b + 8]], 0.3, S + 41, 2), 'var(--chart-1)', 1.5);
            o += T((xy + xw) / 2, low + 14, clip(back, W, 12), 'k', 'middle', ' style="fill:var(--chart-1)"');
            H = low + 36;
          }
          return {html: svg(W, H, o), marks};
        }
        if (!spec.direction && per >= 3) {
          // Rows: each row runs left to right, and an arrow leads down to the start of the next row.
          const rows = ceil(n / per), k = ceil(n / rows), m = back ? 24 : 0, bw = clamp((W - m - 2 - (k - 1) * 44) / k, 80, 112), gap = k > 1 ? (W - m - 2 - k * bw) / (k - 1) : 0, pitch = bh + 64;
          const X = (i) => m + (i % k) * (bw + gap), Yr = (i) => floor(i / k) * pitch + 4;
          st.forEach((s, i) => {
            o += boxS(X(i), Yr(i), bw, i);
            if (i === n - 1) return;
            if ((i + 1) % k) {
              o += strokeP(arrow(X(i) + bw + 6, Yr(i) + bh / 2, X(i + 1) - 6, Yr(i) + bh / 2, 7, 0.8, S + 20 + i), SUB, 1.5);
              if (ar[i]) o += T(X(i) + bw + gap / 2, Yr(i) + 16, clip(ar[i], gap + 6, 12), 'm', 'middle');
            } else {
              const xa = X(i) + bw / 2, xb = X(i + 1) + bw / 2, y0 = Yr(i) + bh + 6, ym = Yr(i) + bh + 32, y1 = Yr(i + 1) - 6;
              o += strokeP(L([[xa, y0], [xa, ym - 8], [xa - 8, ym], [xb + 8, ym], [xb, ym + 8], [xb, y1]], 0.7, S + 20 + i, 6), SUB, 1.5);
              o += strokeP(L([[xb - 5, y1 - 7], [xb, y1], [xb + 5, y1 - 7]], 0.3, S + 60 + i, 2), SUB, 1.5);
              if (ar[i]) o += T((xa + xb) / 2, ym - 10, clip(ar[i], abs(xa - xb) - 24, 12), 'm', 'middle');
            }
          });
          let H = Yr(n - 1) + bh + 2;
          if (back) {
            // Under the last row, then up the left side to the first step.
            const xl = X(n - 1) + bw / 2, yb = Yr(n - 1) + bh + 26, k = backL([[xl, Yr(n - 1) + bh + 6], [xl, yb - 8], [xl - 8, yb], [16, yb], [8, yb - 8], [8, bh / 2 + 4], [m - 6, bh / 2 + 4]], m, W, yb + 4, S);
            o += k.o;
            H = k.H;
          }
          return {html: svg(W, H, o), marks};
        }
        const m = back ? 24 : 0, bw = min(220, W - m - 40), x = m + 1, cx = x + bw / 2, pitch = bh + 48;
        st.forEach((s, i) => {
          o += boxS(x, i * pitch, bw, i);
          if (i < n - 1) {
            o += strokeP(arrow(cx, i * pitch + bh + 6, cx, (i + 1) * pitch - 6, 7, 0.8, S + 20 + i), SUB, 1.5);
            if (ar[i]) o += T(cx + 12, i * pitch + bh + 24, clip(ar[i], W - cx - 14, 12), 'm');
          }
        });
        const ly = (n - 1) * pitch + bh / 2;
        let H = (n - 1) * pitch + bh + 2;
        if (back) {
          const k = backL([[x, ly], [8, ly], [8, bh / 2], [x - 6, bh / 2]], x, W, H, S);
          o += k.o;
          H = k.H;
        }
        return {html: svg(W, H, o), marks};
      },
    };
  });

  // heatmap: a value at each cell of a grid: a map, a profile under a path, or a matrix of two kinds of thing.
  // grid: rows of numbers from the top (null is no data), or {cols, rows, steps: "0123-."}. steps: the edges of the steps.
  // A field draws its cells on a canvas, so a big grid stays fast and crisp. It zooms and pans in x and y.
  // overlays: {paths: [{points, dashFrom}], markers: [{at, kind, label}], notes: [{at, lines, side}]} in data units, in ink.
  const NOD = 99, BG = 'var(--background)', FG = 'var(--foreground)';
  const mn = (s) => String(s).replace(/^-/, '−');
  const nice = (x) => {
    const p = Math.pow(10, floor(Math.log10(x > 0 ? x : 1)));
    return [1, 2, 5, 10].find((m) => m * p >= x - 1e-9) * p;
  };
  // Ticks on a round step, about n of them.
  const hticks = (a, b, n) => {
    const st = nice((b - a) / max(1, n)), o = [];
    for (let t = ceil(a / st - 1e-9) * st; t <= b + 1e-9; t += st) o.push(+t.toFixed(10));
    return o;
  };
  // A smooth line through the points that never overshoots: cubic Hermite, with tangents scaled to the chord lengths.
  // Returns the points and the index of each given point among them.
  function hsmooth(Q) {
    const n = Q.length, out = [], at = [], len = (a, b) => Math.hypot(b[0] - a[0], b[1] - a[1]) || 1e-6;
    const u = Q.map((p, i) => {
      const a = Q[max(0, i - 1)], b = Q[min(n - 1, i + 1)], d = (i > 0 ? len(a, p) : 0) + (i < n - 1 ? len(p, b) : 0);
      return [(b[0] - a[0]) / d, (b[1] - a[1]) / d];
    });
    for (let i = 0; i < n - 1; i++) {
      const a = Q[i], b = Q[i + 1], s = len(a, b), k = max(2, ceil(s / 6));
      at.push(out.length);
      for (let j = 0; j < k; j++) {
        const t = j / k, t2 = t * t, t3 = t2 * t;
        out.push([0, 1].map((c) => (2 * t3 - 3 * t2 + 1) * a[c] + (t3 - 2 * t2 + t) * s * u[i][c] + (3 * t2 - 2 * t3) * b[c] + (t3 - t2) * s * u[i + 1][c]));
      }
    }
    at.push(out.length);
    out.push(Q[n - 1]);
    return [out, at];
  }
  const rrect = (x, y, w, h, r) => `M${r1(x + r)} ${r1(y)}h${r1(w - 2 * r)}a${r} ${r} 0 0 1 ${r} ${r}v${r1(h - 2 * r)}a${r} ${r} 0 0 1 ${-r} ${r}h${r1(2 * r - w)}a${r} ${r} 0 0 1 ${-r} ${-r}v${r1(2 * r - h)}a${r} ${r} 0 0 1 ${r} ${-r}Z`;
  // One panel's grid: {c, r, v: values (NaN is no data)} or {c, r, s: step indexes (-1 no data, -2 below the first step)}.
  function hgrid(g, w) {
    const B = (d) => bad('Bad data', `Ink.heatmap: ${w}${d}`);
    if (Array.isArray(g)) {
      const r = g.length, c = Array.isArray(g[0]) ? g[0].length : 0, v = new Float64Array(r * c);
      if (!c) none(`Ink.heatmap: ${w}the grid has no cells.`);
      g.forEach((row, j) => {
        if (!Array.isArray(row) || row.length !== c) B(`row ${j} has ${row && row.length} cells, and row 0 has ${c}.`);
        row.forEach((x, i) => {
          if (x != null && !fin(+x)) B(`the cell in row ${j}, column ${i} is not a number.`);
          v[j * c + i] = x == null ? NaN : +x;
        });
      });
      return {c, r, v};
    }
    const c = g ? g.cols | 0 : 0, r = g ? g.rows | 0 : 0, n = c * r;
    if (!(c > 0 && r > 0)) none('Ink.heatmap: pass grid: [[1, 2], [3, null]] (rows of numbers) or {cols, rows, steps: "0123..."}.');
    // One character for each cell. "3(12)" is a run of 12 cells of step 3. White space is ignored.
    const t = String(g.steps || '').replace(/\s+/g, ''), s = new Int16Array(n);
    let k = 0;
    for (let i = 0; i < t.length; i++) {
      const ch = t[i], q = ch === '.' ? -1 : ch === '-' ? -2 : /\w/.test(ch) ? parseInt(ch, 36) : NaN;
      let m = 1;
      if (q !== q) B(`"${ch}" is not a step. Use 0 to 9 and A to Z, "." for no data and "-" for below the first step.`);
      if (t[i + 1] === '(') {
        const e = t.indexOf(')', i);
        m = +t.slice(i + 2, e);
        if (!(m >= 1)) B(`a run must be a step and a count, such as 3(12).`);
        i = e;
      }
      s.fill(q, min(k, n), min(k + m, n));
      k += m;
    }
    if (k !== n) B(`the grid string has ${k} cells, and cols times rows is ${n}.`);
    return {c, r, s};
  }
  // A theme change (an attribute on the page) paints the canvases again. The SVG follows the variables by itself.
  const paints = new Set();
  let mo = null, pctx = null;
  function watch(f) {
    paints.add(f);
    if (mo || typeof MutationObserver !== 'function') return;
    mo = new MutationObserver(() => paints.forEach((g) => g()));
    [document.documentElement, document.body].forEach((n) => n && mo.observe(n, {attributes: true}));
  }
  // A pigment as [r, g, b, css]: a probe resolves the CSS color, and a 1 px canvas reads it.
  function rgb(c, el) {
    const i = document.createElement('i');
    i.style.color = pig(c);
    el.appendChild(i);
    const s = getComputedStyle(i).color;
    i.remove();
    if (!pctx) pctx = document.createElement('canvas').getContext('2d', {willReadFrequently: true});
    pctx.clearRect(0, 0, 1, 1);
    pctx.fillStyle = '#000';
    pctx.fillStyle = s;
    pctx.fillRect(0, 0, 1, 1);
    const d = pctx.getImageData(0, 0, 1, 1).data;
    return [d[0], d[1], d[2], s];
  }
  def('heatmap', (spec, S) => {
    const own = ['grid', 'values', 'x', 'y', 'origin', 'aspect', 'height', 'overlays', 'rows', 'columns'], top = {};
    own.forEach((k) => spec[k] != null && (top[k] = spec[k]));
    const many = Array.isArray(spec.panels) && spec.panels.length > 0;
    const ps = (many ? spec.panels : [{}]).map((p, i) => {
      const pn = Object.assign({}, top, p);
      pn.g = hgrid(pn.grid ?? pn.values, many ? `panel ${i}: ` : '');
      return pn;
    });
    const div = !!spec.diverging, dv = typeof spec.diverging === 'object' ? spec.diverging : {};
    let lo = Infinity, hi = -Infinity, e = spec.steps;
    ps.forEach((pn) => pn.g.v && pn.g.v.forEach((x) => ((x < lo && (lo = x)), x > hi && (hi = x))));
    // The edges of the steps. A value at or past the last edge is in the last step.
    if (Array.isArray(e)) {
      e = e.map(Number);
      if (div) e = e.filter((x) => x > 0);
      if (e.length < 2 || e.length > 37 || e.some((x, i) => !fin(x) || (i && x <= e[i - 1]))) bad('Bad data', 'Ink.heatmap: steps must be 2 to 37 numbers that go up, such as [10, 20, 30].');
    } else {
      if (lo > hi) bad('Bad data', ps.some((pn) => pn.g.v) ? 'Ink.heatmap: the grid has no numbers.' : 'Ink.heatmap: a grid of step indexes needs steps: the edges of its steps, such as [10, 15, 20].');
      const k = clamp(fin(e) ? round(e) : div ? 3 : 5, 1, 9);
      if (div) {
        const st = nice(max(abs(lo), abs(hi)) / k);
        e = [st / 4];
        for (let i = 1; i <= k; i++) e.push(st * i);
      } else {
        if (hi <= lo) hi = lo + 1;
        const st = nice((hi - lo) / k), a = floor(lo / st + 1e-9) * st;
        e = [a];
        while (e[e.length - 1] < hi - 1e-9) e.push(+(a + e.length * st).toFixed(10));
      }
    }
    // Counts from zero: a zero is not painted, as a cell with nothing in it.
    const n = e.length - 1, zn = !Array.isArray(spec.steps) && !div && lo === 0;
    const pa = div ? dv.above ?? 1 : spec.color ?? 1, pb = dv.below ?? 5;
    const a0 = max(10, 30 - 2 * n), a1 = div ? 85 : 100;
    const alpha = (k) => round(n < 2 ? a1 : a0 + (a1 - a0) * Math.pow((k - 1) / (n - 1), 1.15));
    const al = Array.from({length: n + 1}, (_, k) => round(alpha(k || 1) * 2.55));
    const gz = (b) => glaze(b < 0 ? pb : pa, alpha(abs(b)));
    const band = (v) => {
      if (v !== v) return NOD;
      const a = div ? abs(v) : v;
      if (a < e[0] || (zn && a <= 0)) return 0;
      let k = 1;
      while (k < n && a >= e[k]) k++;
      return div && v < 0 ? -k : k;
    };
    const vf0 = mkfmt(spec), cur = /^[$€£¥]$/.test(spec.unit || '') || !!spec.format;
    const vf = (v) => mn((div && v > 0 ? '+' : '') + vf0(v));
    const bare = (v) => (cur ? vf(v) : mn((div && v > 0 ? '+' : '') + fnum(v)));
    const rg = (b) => {
      if (!b) return div ? 'Near zero' : 'Below ' + vf(e[0]);
      const k = abs(b);
      return b < 0 ? bare(-e[k]) + ' to ' + vf(-e[k - 1]) : bare(e[k - 1]) + ' to ' + vf(e[k]);
    };
    // The key notes: [[20, "light rain"], ...]. A note holds from its value to the next note.
    const kn = (spec.notes || []).map((x) => (Array.isArray(x) ? [+x[0], String(x[1])] : [+x.at, String(x.text)])).filter((x) => fin(x[0])).sort((a, b) => a[0] - b[0]);
    const noteOf = (v) => {
      if (v !== v) return '';
      const ok = kn.filter(([a]) => (div && v < 0 ? a < 0 && a >= v : a <= v && (!div || a >= 0)));
      const t = ok.length ? ok[div && v < 0 ? 0 : ok.length - 1][1] : '';
      return t && t[0].toUpperCase() + t.slice(1);
    };
    const mat = (spec.kind || (ps[0].rows || ps[0].columns ? 'matrix' : 'field')) === 'matrix';
    let zero = false;
    const CELL = {r: null, p: true, side: true, on: false, get a() { return this.g.at(this.k); }, get t() { return this.g.txt(this.k)[0]; }, get l() { return this.g.txt(this.k).slice(1); }, get d() { return this.g.item(this.k); }};
    const panel = (pn, pi) => {
      const {c: C, r: R} = pn.g, N = C * R, bot = pn.origin === 'bottom', bands = new Int8Array(N), vals = pn.g.v ? new Float64Array(N) : null;
      let best = -1, vlo = Infinity;
      for (let k = 0; k < N; k++) {
        const j = (bot ? R - 1 - floor(k / C) : floor(k / C)) * C + (k % C);
        if (vals) {
          const v = (vals[k] = pn.g.v[j]);
          bands[k] = band(v);
          if (v === v && (best < 0 || v > vals[best])) best = k;
          if (v < vlo) vlo = v;
        } else {
          const q = pn.g.s[j], b = q === -1 ? NOD : q === -2 ? 0 : div ? q - n : q + 1;
          if (q >= 0 && (div ? q > 2 * n : q >= n)) bad('Bad data', `Ink.heatmap: the grid uses step ${q.toString(36).toUpperCase()}, and steps makes only ${div ? 2 * n + 1 : n} steps. Give one edge more than the steps.`);
          bands[k] = b;
        }
        if (!bands[k]) zero = true;
      }
      const xa = pn.x || {}, ya = pn.y || {}, cn = pn.columns, rn = pn.rows, ov = pn.overlays || {};
      const xl = xa.min ?? 0, xh = xa.max ?? C, yl = ya.min ?? 0, yh = ya.max ?? R, sx = xh - xl || 1, sy = yh - yl || 1;
      const xm = mkfmt(xa), ym = mkfmt(ya), xf = (v) => mn(xm(v)), yf = (v) => mn(ym(v));
      const cxv = (i) => xl + ((i + 0.5) * sx) / C, cyv = (r) => yh - ((r + 0.5) * sy) / R, rname = (r) => rn && rn[bot ? R - 1 - r : r];
      const where = (k) => {
        const r = floor(k / C), i = k % C;
        return rn || cn ? [rname(r), cn && cn[i]].filter((x) => x != null).join(', ') : `${xa.name || 'x'} ${xf(cxv(i))}, ${ya.name || 'y'} ${yf(cyv(r))}`;
      };
      // The readout: the value or the step, what the step means, and where the cell is.
      let lk = -1, lt = null;
      const note = (k, b = bands[k]) => (b === NOD ? '' : noteOf(vals ? vals[k] : b ? Math.sign(b) * e[abs(b) - 1] : NaN));
      const txt = (k) => {
        if (k === lk) return lt;
        const b = bands[k], v = vals ? vals[k] : NaN, nd = b === NOD, nt = note(k);
        const val = nd ? 'No data' : vals ? vf(v) : rg(b), more = nt || (vals && !nd ? (b ? 'Step ' + rg(b) : rg(0)) : '');
        lk = k;
        return (lt = rn || cn ? [where(k), val, more] : [val, more, where(k)]);
      };
      const item = (k) => {
        const r = floor(k / C), i = k % C, b = bands[k], v = vals ? vals[k] : NaN;
        return {panel: pi, row: bot ? R - 1 - r : r, col: i, value: v === v ? v : null, step: b === NOD ? null : b, note: note(k), x: cn ? cn[i] : cxv(i), y: rn ? rname(r) : cyv(r)};
      };
      const D = {};
      const at = (k) => [D.x0 + (k % C) * D.pX, D.y0 + floor(k / C) * D.pY, D.cw, D.ch];
      const geo = {at, txt, item}, marks = Array.from({length: N}, (_, k) => Object.assign(Object.create(CELL), {g: geo, k}));
      const hover = (k, stg) => {
        const h = stg.querySelector('.hv');
        if (!h) return;
        h.style.display = k < 0 ? 'none' : '';
        if (k < 0) return;
        const q = at(k), o = D.tl ? 1 : 0, w = max(5, q[2]) + 2 * o, hh = max(5, q[3]) + 2 * o;
        [['x', q[0] + q[2] / 2 - w / 2], ['y', q[1] + q[3] / 2 - hh / 2], ['width', w], ['height', hh]].forEach(([a, x]) => h.setAttribute(a, r1(x)));
      };
      const pick = (px, cy, cx, qy) => (cx < D.x0 || cx > D.x0 + D.pw || cy < D.y0 || cy > D.y0 + D.ph ? -1 : clamp(floor((qy - D.y0) / D.pY), 0, R - 1) * C + clamp(floor((px - D.x0) / D.pX), 0, C - 1));
      const nav = (k, key) => {
        const r = floor(k / C), i = k % C;
        return key === 'ArrowLeft' ? k - (i > 0) : key === 'ArrowRight' ? k + (i < C - 1) : key === 'ArrowUp' ? (r ? k - C : k) : r < R - 1 ? k + C : k;
      };
      let img = null;
      const warned = new Set();
      // The cells as one pixel each, in the resolved pigments. Drawn again only when a pigment changes.
      const image = (el) => {
        const A = rgb(pa, el), B = div ? rgb(pb, el) : A, key = A[3] + B[3];
        if (img && img.k === key) return img.c;
        const c = document.createElement('canvas');
        c.width = C;
        c.height = R;
        const x = c.getContext('2d'), id = x.createImageData(C, R), dd = id.data;
        for (let k = 0; k < N; k++) {
          const b = bands[k], q = b < 0 ? B : A, o = k * 4;
          if (!b || b === NOD) continue;
          dd[o] = q[0];
          dd[o + 1] = q[1];
          dd[o + 2] = q[2];
          dd[o + 3] = al[abs(b)];
        }
        x.putImageData(id, 0, 0);
        img = {k: key, c};
        return c;
      };
      const hv = `<rect class="hv" style="display:none;fill:none;stroke:${FG};stroke-width:1.5"/>`;
      // A matrix with room for tiles: a gap, a radius of 4, an outline for an empty step, and values when they fit.
      const tiles = (W) => {
        const lw = rn ? min(ceil(max(...rn.map((s) => mw(s, 12)))), round(W * 0.3)) + 12 : 0, g = (W - lw) / C >= 15 ? 3 : 1, cw = min(72, (W - lw) / C - g);
        if (cw < 5 || N > 20000) return null;
        let wd = 0;
        if (vals && cw >= 40) for (let k = 0; k < N; k++) vals[k] === vals[k] && (wd = max(wd, mw(vf(vals[k]), 12, 600)));
        const sv = vals && cw >= max(40, wd + 12), th = sv ? 30 : clamp(round(cw * 0.8), 10, 28), pX = cw + g, pY = th + g, y0 = cn ? 24 : 0, rr = r1(min(4, cw / 4, th / 4));
        Object.assign(D, {tl: true, x0: lw, y0, pX, pY, cw, ch: th, pw: C * pX - g, ph: R * pY - g});
        const by = {};
        let o = '', tx = '';
        for (let k = 0; k < N; k++) {
          const b = bands[k], x = lw + (k % C) * pX, y = y0 + floor(k / C) * pY;
          if (b === NOD) continue;
          (by[b] = by[b] || []).push(b ? rrect(x, y, cw, th, rr) : rrect(x + 0.5, y + 0.5, cw - 1, th - 1, rr));
          if (sv && vals[k] === vals[k]) {
            const a = b ? alpha(abs(b)) : 0;
            tx += T(x + cw / 2, y + th / 2, vf(vals[k]), a >= 80 ? 'o' : a >= 45 ? 'v' : 'm', 'middle');
          }
        }
        Object.keys(by).forEach((b) => (o += +b ? fillP(by[b].join(''), gz(+b)) : P(by[b].join(''), 'fill:none;stroke:var(--input)')));
        if (cn) thin(C, pX, max(...cn.map((s) => mw(s, 12))), 99).forEach((i) => (o += T(lw + i * pX + cw / 2, 8, cn[i], '', 'middle')));
        if (rn) for (let r = 0; r < R; r++) o += T(0, y0 + r * pY + th / 2, clip(rname(r), lw - 12, 12), 'm');
        return {html: svg(W, y0 + D.ph + 2, `<g class="hp">${o + tx + hv}</g>`), marks, hover, pick, nav};
      };
      // A field: the cells touch, on a canvas under an SVG with the grid, the ticks and the overlays.
      // The plot of a field in a panel W px wide: the left edge, the size, and the top in the panel.
      // A plot with no fixed aspect grows taller (at most 1.8 times) when its notes find no room.
      const grown = {};
      const size = (W) => {
        const l0 = rn ? rn.map(String) : (ya.ticks || hticks(yl, yh, 6)).map(yf), lw = l0.length ? min(ceil(max(...l0.map((s) => mw(s, 12)))), round(W * 0.3)) + 10 : 4;
        let pw = W - lw - 8, ph;
        if (pn.aspect === 'equal') {
          ph = (pw * sy) / sx;
          const cap = max(320, min(800, pw * 1.25));
          if (ph > cap) (pw = (cap * sx) / sy), (ph = cap);
        } else ph = fin(pn.aspect) ? max(140, pw * pn.aspect) : fin(pn.height) ? pn.height : clamp(round((pw * R) / C), 120, min(560, pw * 0.75));
        return {lw, pw: round(pw), ph: round(ph * (pn.aspect === 'equal' ? 1 : grown[W] || 1)), top: (ya.label ? 24 : 0) + 8};
      };
      const field = (W, z) => {
        const {lw, pw, ph} = size(W), x0 = lw, y0 = 8;
        const PW = pw * z, PH = ph * z, cw = PW / C, ch = PH / R, by = y0 + ph, s0 = S + pi * 997;
        const X = (q) => x0 + ((q - xl) / sx) * PW, Y = (q) => y0 + ((yh - q) / sy) * PH;
        Object.assign(D, {tl: false, x0, y0, pX: cw, pY: ch, cw, ch, pw, ph});
        const gl = (a, b, c, d, k) => `<path class="hg" d="${L([[a, b], [c, d]], 0.4, s0 + k, 24)}"/>`;
        let gx = '', tx = '', ty = '', last = -1e9;
        if (cn) thin(C, cw, max(...cn.map((s) => mw(s, 12))), 99).forEach((i) => (tx += T(x0 + (i + 0.5) * cw, by + 16, cn[i], '', 'middle')));
        else
          (xa.ticks || hticks(xl, xh, PW / 56)).forEach((q, k) => {
            const px = X(q), t = xf(q), w = mw(t, 12), end = z === 1 && px + w / 2 > W;
            if (q > xl && q < xh) gx += gl(px, y0, px, y0 + PH, k);
            if (px - w / 2 < last + 10) return;
            last = px + w / 2;
            tx += T(end ? W : px, by + 16, t, '', end ? 'end' : 'middle');
          });
        if (rn) thin(R, ch, 16, 99).forEach((r) => (ty += T(x0 - 8, y0 + (r + 0.5) * ch, clip(rname(r), lw - 8, 12), '', 'end')));
        else
          (ya.ticks || hticks(yl, yh, PH / 44)).forEach((q, k) => {
            ty += T(x0 - 8, Y(q), yf(q), '', 'end');
            if (q > yl && q < yh) gx += gl(x0, Y(q), x0 + PW, Y(q), 40 + k);
          });
        // Paths: ink with a canvas halo, solid to dashFrom and dashed after it.
        let hal = '', ink = '', mk = '', lead = '', lab = '';
        const dots = [], boxes = [];
        (ov.paths || []).forEach((p, j) => {
          const pts = (p.points || []).map((q) => [X(+q[0]), Y(+q[1])]).filter((q) => fin(q[0]) && fin(q[1]));
          if (pts.length < 2) return;
          const [base, at] = p.smooth === false ? [pts, pts.map((_, i) => i)] : hsmooth(pts), w = p.width || 2;
          const ci = at[p.dash === true ? 0 : fin(p.dashFrom) ? clamp(round(p.dashFrom), 0, pts.length - 1) : pts.length - 1];
          [base.slice(0, ci + 1), base.slice(ci)].forEach((h, q) => {
            if (h.length < 2) return;
            const dd = perturb(dense(h, 3), 0.5, s0 + 100 + j * 2 + q), d = pd(dd);
            dd.forEach((pt, m) => m % 2 || dots.push(pt));
            hal += strokeP(d, 'color-mix(in srgb,var(--background) 75%,transparent)', w + 3);
            ink += strokeP(d, p.color == null ? FG : pig(p.color), w, q ? 'stroke-dasharray:5 4' : '');
          });
        });
        // Markers: a gold dab for the moment, ink dots for points, a square for a place, an X for an end.
        (ov.markers || []).forEach((m, j) => {
          const x = X(+(m.at || [])[0]), y = Y(+(m.at || [])[1]), s = s0 + 200 + j * 3, k = m.kind || 'point';
          if (!fin(x) || !fin(y)) return;
          if (k === 'moment') mk += fillP(dab(x, y, 7.1, s), BG) + fillP(dab(x, y, 5.5, s + 1), pig(m.color ?? 3));
          else if (k === 'place') mk += P(rect(x - 4.5, y - 4.5, 9, 9, 1.5, 0.3, s, 0, 3), `fill:var(--popover);stroke:${FG};stroke-width:1.4`);
          else if (k === 'end') {
            const d = L([[x - 5, y - 5], [x + 5, y + 5]], 0.4, s, 2) + L([[x - 5, y + 5], [x + 5, y - 5]], 0.4, s + 1, 2);
            mk += strokeP(d, BG, 5.4) + strokeP(d, FG, 2.4);
          } else mk += fillP(dab(x, y, 4.6, s), BG) + fillP(dab(x, y, 3, s + 1), FG);
          boxes.push([x - 7, y - 7, 14, 14]);
          if (m.label != null) {
            const t = String(m.label), tw = mw(t, 12, 600), rt = x + 9 + tw < x0 + PW;
            mk += T(rt ? x + 9 : x - 9, y - 10, t, 'v', rt ? 'start' : 'end', ` style="paint-order:stroke;stroke:${BG};stroke-width:3px;stroke-linejoin:round"`);
            boxes.push([rt ? x + 7 : x - 11 - tw, y - 19, tw + 4, 18]);
          }
        });
        // Notes on paper labels. Each takes the nearest place in the plot that covers no other note, marker or leader,
        // and at most a little of a path: around its point, then at the top or the bottom of the plot with a leader,
        // then with its first line only. A note with no place is drawn anyway, and the console names it.
        const nts = (ov.notes || []).map((nt) => {
          const ls = (Array.isArray(nt.lines) ? nt.lines : String(nt.text ?? '').split('\n')).map(String).filter((x) => x);
          return {px: X(+(nt.at || [])[0]), py: Y(+(nt.at || [])[1]), ls, sd: nt.side};
        }).filter((n) => n.ls.length && fin(n.px) && fin(n.py));
        const fit = (n, ls, dt, bx0) => {
          const {px, py} = n, two = ls.length > 1, w = ceil(max(...ls.map((t, i) => mw(t, 12, i || !two ? 400 : 600)))) + 12, h = ls.length * 16 + 6, cs = [];
          // Far places come only after the near ones have no room.
          [8, 22, 44, 72, 110, 160].forEach((d) => {
            const c = d * 0.7, f = d > 72 ? 1e3 : 0;
            cs.push(['above', px - w / 2, py - d - h, f], ['below', px - w / 2, py + d, f], ['right', px + d, py - h / 2, f], ['left', px - d - w, py - h / 2, f], ['above', px - c - w, py - c - h, f], ['above', px + c, py - c - h, f], ['below', px - c - w, py + c, f], ['below', px + c, py + c, f]);
          });
          [px - w / 2, px - w - 8, px + 8].forEach((x) => cs.push(['above', x, y0 + 2, 1e3], ['below', x, y0 + PH - h - 2, 1e3]));
          let bst = [1e9];
          cs.forEach(([sd, bx, bt, far = 0]) => {
            const qx = clamp(px, bx, bx + w), qy = clamp(py, bt, bt + h), on = dt.filter((q) => q[0] > bx - 3 && q[0] < bx + w + 3 && q[1] > bt - 3 && q[1] < bt + h + 3).length;
            let sc = far + Math.hypot(px - qx, py - qy) + (n.sd && n.sd !== sd ? 40 : 0) + (bx < x0 + 2 || bt < y0 + 2 || bx + w > x0 + PW - 2 || bt + h > y0 + PH - 2 ? 1e4 : 0) + (on > 4 ? 2000 + on * 10 : on * 60);
            bx0.forEach((b) => {
              // A box in the way, or a note on the leader line from this note to its point.
              if (b[0] < bx + w + 4 && b[0] + b[2] + 4 > bx && b[1] < bt + h + 2 && b[1] + b[3] + 2 > bt) sc += b[4] ? 4000 : 600;
              else if (b[4] && [0.2, 0.4, 0.6, 0.8].some((t) => (t = [qx + (px - qx) * t, qy + (py - qy) * t]) && t[0] > b[0] && t[0] < b[0] + b[2] && t[1] > b[1] && t[1] < b[1] + b[3])) sc += 2000;
            });
            if (sc < bst[0]) bst = [sc, bx, bt, w, h, ls, two, qx, qy];
          });
          return bst;
        };
        const place = (order) => {
          const dt = dots.slice(), bx0 = boxes.slice(), out = [];
          let fails = 0;
          order.forEach((n) => {
            let f = fit(n, n.ls, dt, bx0);
            if (f[0] >= 2000 && n.ls.length > 1) {
              const g = fit(n, n.ls.slice(0, 1), dt, bx0);
              if (g[0] < 2000 || g[0] < f[0] - 1000) f = g;
            }
            if (f[0] >= 2000) fails++;
            const [, bx, bt, w, h, , , qx, qy] = f, dl = Math.hypot(n.px - qx, n.py - qy);
            bx0.push([bx, bt, w, h, 1]);
            for (let t = 0.1; dl > 12 && t < 0.9; t += 6 / dl) dt.push([qx + (n.px - qx) * t, qy + (n.py - qy) * t]);
            out.push([n, f]);
          });
          return {out, fails};
        };
        // Notes that find no place in the given order go first in a second try.
        let pl = place(nts);
        if (pl.fails) {
          const first = pl.out.filter(([, f]) => f[0] >= 2000).map(([n]) => n), p2 = place(first.concat(nts.filter((n) => !first.includes(n))));
          if (p2.fails < pl.fails) pl = p2;
        }
        if (pl.fails && z === 1 && pn.aspect !== 'equal' && (grown[W] || 1) < 1.8) {
          grown[W] = (grown[W] || 1) * 1.35;
          return field(W, z);
        }
        pl.out.forEach(([n, [sc, bx, bt, w, h, ls, two, qx, qy]], j) => {
          const {px, py} = n, dl = Math.hypot(px - qx, py - qy);
          if (sc >= 2000 && !warned.has(n.ls[0])) warned.add(n.ls[0]), console.warn(`Ink.heatmap: the note "${n.ls[0]}" has no free place, so it covers other marks. Give the chart more width or make the note shorter.`);
          if (dl > 12) lead += strokeP(L([[qx, qy], [px - ((px - qx) / dl) * 6, py - ((py - qy) / dl) * 6]], 0.4, s0 + 300 + j, 3), 'var(--muted-foreground)', 1.2);
          lab += `<rect x="${r1(bx)}" y="${r1(bt)}" width="${w}" height="${h}" rx="4" style="fill:color-mix(in srgb,var(--background) 84%,transparent)"/>` + ls.map((t, i) => T(bx + 6, bt + 11 + i * 16, t, i || !two ? 'm' : 'v')).join('');
        });
        const id = 'ink-h' + ++uid, cg = (k, x, y, w, h, body, cls, always) => (z > 1 || always ? `<clipPath id="${id + k}"><rect x="${r1(x)}" y="${r1(y)}" width="${r1(w)}" height="${r1(h)}"/></clipPath><g clip-path="url(#${id + k})"><g class="${cls}">${body}</g></g>` : `<g class="${cls}">${body}</g>`);
        const body = cg('a', x0, y0, pw, ph, gx + hal + ink + mk + lead + lab + hv, 'hp', 1) + cg('b', x0 - 40, by, pw + 80, 28, tx, 'hx') + cg('c', -40, y0 - 9, x0 + 40, ph + 18, ty, 'hy');
        // Panning moves the overlays and the ticks, and draws the visible part of the cells on the canvas.
        const onpan = (pan, py, stg) => {
          const tr = (s, a, b) => stg.querySelectorAll(s).forEach((g) => g.setAttribute('transform', `translate(${-r1(a)} ${-r1(b)})`));
          tr('.hp', pan, py);
          tr('.hx', pan, 0);
          tr('.hy', 0, py);
          const cv = stg.querySelector('canvas');
          if (!cv) return;
          const src = image(stg), dpr = root.devicePixelRatio || 1, w = round(pw * dpr), h = round(ph * dpr), x = cv.getContext('2d');
          if (cv.width !== w || cv.height !== h) (cv.width = w), (cv.height = h);
          x.imageSmoothingEnabled = false;
          x.clearRect(0, 0, w, h);
          x.drawImage(src, pan / cw, py / ch, pw / cw, ph / ch, 0, 0, w, h);
        };
        const cell = min(pw / C, ph / R);
        return {
          html: (ya.label ? `<div class="hm-a">${esc(ya.label)}</div>` : '') + `<div class="hm-f"><canvas style="left:${x0}px;top:${y0}px;width:${pw}px;height:${ph}px"></canvas>${svg(W, by + 26, body)}</div>` + (xa.label ? `<div class="hm-a" style="padding-left:${x0}px">${esc(xa.label)}</div>` : ''),
          marks, hover, pick, nav, onpan,
          pan: [x0, x0 + pw, y0, y0 + ph],
          zoom: cell < 6 ? clamp(24 / cell, 2, 12) : 1,
        };
      };
      const lov = vals && best >= 0 ? `, from ${vf(vlo)} to ${vf(vals[best])}. The highest is at ${where(best)}.` : '.';
      const self = {
        alt: `${pn.title ? pn.title + '. ' : ''}Heat map of ${C} by ${R} cells${lov}`,
        zoomable: true,
        size,
        draw(W, box, v) {
          const t = mat ? tiles(W) : null;
          self.zoomable = !t;
          return t || field(W, v.z);
        },
      };
      return self;
    };
    const models = ps.map(panel), subs = [];
    // The key: each step from faint to full, with its edges. Notes say what a step means.
    const bl = [];
    for (let k = n; k >= 1; k--) bl.push(k);
    if (div) for (let k = 0; k <= n; k++) bl.push(-k);
    const kt = spec.unit ? String(spec.unit) : '', kk = typeof spec.key === 'object' ? spec.key : {};
    const knt = kn.map(([a, t]) => bare(a) + ' ' + t).concat(spec.note ? [String(spec.note)] : []);
    const edges = [];
    for (let i = 0; i <= n; i++) edges.push(e[n - i]);
    if (div) for (let i = 0; i <= n; i++) edges.push(-e[i]);
    const vkw = () => ceil(max(kt ? mw(kt, 12, 600) : 0, 22 + max(...edges.map((x) => mw(bare(x), 12))), ...knt.map((t) => min(150, mw(t, 12))))) + 2;
    // The column of steps spans hc px: the height of the plot.
    const vkey = (kw, hc) => {
      const sh = hc / bl.length, s = ceil(28 / sh);
      let o = '';
      bl.forEach((b, i) => b && (o += `<path d="M0 ${r1(4 + i * sh)}h14v${r1(sh)}h-14Z" style="fill:${gz(b)}"/>`));
      o += P(rect(0, 4, 14, hc, 1, 0.35, S + 7, 0, 6), 'fill:none;stroke:var(--input)');
      edges.forEach((v, i) => (i % s === 0 || i === edges.length - 1) && (o += T(22, 4 + i * sh, bare(v))));
      return (kt ? `<b class="hm-kt">${esc(kt)}</b>` : '') + svg(kw, hc + 12, o, 'sk') + knt.map((t) => `<div class="hm-kn">${esc(t)}</div>`).join('');
    };
    const hkey = () => {
      const words = mat || div, lst = (mat && !div && zero ? [0] : []).concat(bl.slice().reverse()), sq = words && !div;
      const sw = sq ? 14 : div ? 27 : 24, sh = sq ? 14 : 12, gp = sq ? 6 : 3, pt = sw + gp, lab = words ? [] : edges.slice().reverse(), ox = lab.length ? ceil(mw(bare(lab[0]), 12) / 2) : 0;
      let o = '';
      lst.forEach((b, i) => (o += P(rect(ox + i * pt, 1, sw, sh, sq ? 3 : 2.5, 0.25, S + 700 + i, 0, 4), b ? `fill:${gz(b)}` : 'fill:none;stroke:var(--input)')));
      const s = ceil((max(0, ...lab.map((x) => mw(bare(x), 12))) + 8) / pt);
      lab.forEach((v, i) => (i % s === 0 || i === lab.length - 1) && (o += T(ox + i * pt - (i ? gp / 2 : 0), sh + 12, bare(v), '', 'middle')));
      const sp = (t) => `<span>${esc(t)}</span>`;
      return `<div class="hm-k">${kt && !words ? `<b>${esc(kt)}</b>` : ''}${words ? sp(kk.low ?? 'Less') : ''}${svg(2 * ox + lst.length * pt - gp, sh + (words ? 2 : 20), o, 'sk')}${words ? sp(kk.high ?? 'More') : ''}${spec.note ? sp(spec.note) : ''}</div>` + (kn.length ? `<div class="hm-k" style="margin-top:4px">${kn.map(([a, t]) => sp(bare(a) + ' ' + t)).join('')}</div>` : '');
    };
    const alt = models.map((m) => m.alt).join(' ') + (kt ? ` The key is in ${kt}.` : '');
    return {
      alt,
      // The panels sit side by side, each 300 px or more, with the key at the right. Its column of steps spans the plot,
      // with 12 px or more for each step. Else the panels stack as the width allows, and the key is a row below them.
      draw(W) {
        const np = ps.length, gap = 24, key = spec.key !== false, kw = key && !mat ? vkw() : 0, th = (kt ? 24 : 0) + 4;
        let right = key && !mat && W >= np * 300 + np * gap + kw, per, pw, hh, kt0 = 0, hc = 0;
        const lay = () => {
          per = right ? np : clamp(floor((W + gap) / (300 + gap)), 1, np);
          pw = floor((W - (right ? kw + gap : 0) - (per - 1) * gap) / per);
          hh = 0;
          ps.forEach((pn) => {
            const t = (pn.title ? wrap(pn.title, pw, 13, 600, 3).length * 20 : 0) + (pn.note ? wrap(pn.note, pw, 12, 400, 4).length * 16 : 0);
            hh = max(hh, t ? t + 12 : 0);
          });
        };
        lay();
        if (right) {
          const g = models[0].size(pw), top = hh + g.top;
          kt0 = max(0, top - th);
          hc = top + g.ph - kt0 - th;
          if (hc < 12 * bl.length) (right = false), lay();
        }
        const head = (pn) => (hh ? `<div class="hm-h" style="min-height:${hh - 12}px">${pn.title ? `<b>${esc(pn.title)}</b>` : ''}${pn.note ? `<span>${esc(pn.note)}</span>` : ''}</div>` : '');
        const html = `<div class="hm-r">${ps.map((pn) => `<div style="width:${pw}px">${head(pn)}<div class="hm-s"></div></div>`).join('')}${right ? `<div style="width:${kw}px;padding-top:${kt0}px">${vkey(kw, hc)}</div>` : ''}</div>` + (key && !right ? hkey() : '');
        return {
          html,
          marks: [],
          // Each panel is a chart of its own in the shared layer, with its readout, keys and zoom.
          after(b, stg) {
            b.setAttribute('role', 'group');
            b.setAttribute('aria-label', spec.alt || alt);
            stg.querySelectorAll('.hm-s').forEach((slot, i) => {
              let q = subs[i];
              if (!q) {
                const pb = document.createElement('div'), ps2 = document.createElement('div');
                pb.className = 'ink';
                ps2.className = 'ink-st';
                pb.appendChild(ps2);
                pb.setAttribute('role', 'img');
                pb.setAttribute('aria-label', models[i].alt);
                q = subs[i] = {b: pb, w: 0, u: ui(pb, ps2, pb, spec, models[i])};
                q.u.redraw = (f) => q.u.render(q.w, f === 'zoom');
                const f = () => (pb.isConnected ? q.u.st.r.onpan && q.u.st.r.onpan(q.u.st.pan, q.u.st.py, ps2) : !b.isConnected && paints.delete(f));
                watch(f);
              }
              slot.appendChild(q.b);
              q.w = pw;
              q.u.render(pw);
            });
          },
        };
      },
    };
  }, 'none');

  // spark and meter pieces. Ink.spark draws a small line with a dab at its end.
  function sparkSvg(v, w, h, c, S) {
    const lo = min(...v), hi = max(...v), pts = v.map((x, i) => [2 + (v.length > 1 ? (i * (w - 8)) / (v.length - 1) : 0), 4 + (1 - (hi > lo ? (x - lo) / (hi - lo) : 0.5)) * (h - 10)]);
    const e = pts[pts.length - 1];
    return svg(w, h, (pts.length > 1 ? strokeP(L(pts, 0.6, S, 5, true), pig(c), 2) : '') + fillP(dab(e[0] + 1, e[1], 3.6, S + 1), pig(c)), 'sk');
  }
  const meterSvg = (frac, w, c, S) =>
    svg(w, 12, strokeP(L([[4, 6], [w - 4, 6]], 0.5, S, 6), 'var(--input)', 6) + (frac > 0 ? strokeP(L([[4, 6], [4 + clamp(frac, 0, 1) * (w - 8), 6]], 0.6, S + 1, 6), pig(c), 6) : ''), 'sk');
  def('spark', (spec, S) => {
    const v = (spec.values || []).map(Number);
    if (v.length < 2 || v.some((x) => !fin(x))) none('Ink.spark: pass values: [numbers] with two numbers or more.');
    return {alt: `Sparkline from ${fnum(v[0])} to ${fnum(v[v.length - 1])}, low ${fnum(min(...v))}, high ${fnum(max(...v))}.`, cls: 'sp', w: spec.width ?? 168, draw: (W) => ({html: sparkSvg(v, W, spec.height ?? 38, spec.color ?? 1, S)})};
  });

  // numbers: stat tiles. More tiles wrap into even rows.
  // tiles: [{label, value, of, delta, text, spark: [...], meter: 0.69 or [41.2, 60]}]
  def('numbers', (spec, S) => {
    const ts = (spec.tiles || []).map((t, i) => {
      if (t == null || t.value == null) bad('Bad data', `Ink.numbers: tile ${i} needs a value (a string like "$41.20").`);
      return t;
    });
    if (!ts.length) none('Ink.numbers: pass tiles: [{label, value}, ...].');
    return {
      alt: ts.map((t) => `${t.label || ''} ${t.value}${t.of ? ' ' + t.of : ''}${t.delta ? ', ' + t.delta : ''}`.trim()).join('. ') + '.',
      draw(W) {
        const most = clamp(floor((W + 12) / 172), 1, ts.length), cols = ceil(ts.length / ceil(ts.length / most)), inner = floor((W - 12 * (cols - 1)) / cols - 32);
        const marks = [];
        const html = ts.map((t, i) => {
          const m = Array.isArray(t.meter) ? t.meter[0] / t.meter[1] : t.meter, sp = Array.isArray(t.spark) && t.spark.length > 1 ? t.spark.map(Number) : null;
          marks.push(M(null, t.label || String(t.value), [t.label ? String(t.value) + (t.of ? ' ' + t.of : '') : '', t.delta ? String(t.delta) : '', sp ? `From ${fnum(sp[0])} to ${fnum(sp[sp.length - 1])}, low ${fnum(min(...sp))}, high ${fnum(max(...sp))}` : '', fin(m) ? round(m * 100) + '% full' : ''], {label: t.label, value: t.value, index: i}, {on: false}));
          return `<div class="tile" data-k="${i}">${t.label ? `<div class="tl">${esc(t.label)}</div>` : ''}<div class="tv"><span class="big">${esc(t.value)}</span>${t.delta ? `<span class="dl" style="color:${pig(t.deltaColor, 2)}">${esc(t.delta)}</span>` : ''}${t.of ? `<span class="of">${esc(t.of)}</span>` : ''}</div>${t.text ? `<div class="tt">${esc(t.text)}</div>` : ''}${Array.isArray(t.spark) && t.spark.length > 1 ? `<div role="img" aria-label="Trend, ${fnum(t.spark[0])} to ${fnum(t.spark[t.spark.length - 1])}">${sparkSvg(t.spark.map(Number), inner, 38, t.color ?? 1, S + i * 5)}</div>` : ''}${fin(m) ? `<div role="img" aria-label="${round(m * 100)} percent" style="margin-top:8px">${meterSvg(m, inner, t.color ?? 2, S + i * 5)}</div>` : ''}</div>`;
        });
        return {html: `<div class="nums" style="grid-template-columns:repeat(${cols},minmax(0,1fr))">${html.join('')}</div>`, marks, after: (b, stg) => stg.querySelectorAll('.tile').forEach((t, i) => (marks[i].el = t))};
      },
    };
  }, 'none');

  // table: a real <table>. Rows on the canvas, one inked rule under the head.
  // columns: ["PR", {name: "Age", align: "right", tone: 2}]. rows: [["#62", "Fix the build", {text: "Ready", color: 2}]]
  def('table', (spec, S) => {
    const cols = (spec.columns || []).map((c) => (typeof c === 'string' ? {name: c} : {...c, name: c.name ?? c.label ?? ''}));
    const rows = spec.rows || [];
    if (!cols.length || !rows.length) none('Ink.table: pass columns: ["A", "B"] and rows: [["a", "b"], ...].');
    if (rows.some((r) => !Array.isArray(r))) bad('Bad data', 'Ink.table: each row must be an array with one cell for each column.');
    const cell = (x) => (x != null && typeof x === 'object' ? x : {text: x == null ? '' : x});
    cols.forEach((c, j) => {
      const txt = rows.map((r) => String(cell(r[j]).text)).filter(Boolean), dabs = rows.some((r) => cell(r[j]).color != null);
      const mono = c.mono ?? (txt.length > 0 && txt.every((s) => /^[#$€£+\-]?[\d.,:]+[%KMBkmb]?$/.test(s))), pure = txt.length > 0 && txt.every((s) => /^[+\-$€£]?[\d.,]+[%KMB]?$/.test(s));
      Object.assign(c, {mono, right: c.align ? c.align === 'right' : pure, dab: dabs, avg: txt.length ? sum(txt.map((s) => s.length)) / txt.length : 0});
    });
    const free = cols.filter((c) => c.tone == null && !c.mono && !c.dab).sort((a, b) => b.avg - a.avg)[0];
    cols.forEach((c) => (c.tone = c.tone ?? (c === free || c.dab ? 0 : 1)));
    const most = spec.maxRows > 0 ? floor(spec.maxRows) : 12, st = {open: false};
    const rule = (w) => svg(w, 4, strokeP(L([[1, 2], [w - 1, 2]], 0.5, S, 8), 'var(--input)', 1.2), 'sk');
    return {
      alt: `Table with ${rows.length} rows. Columns: ${cols.map((c) => c.name).join(', ')}.`,
      draw(W, box, v) {
        const rs = st.open || rows.length <= most ? rows : rows.slice(0, most);
        const head = cols.map((c) => `<th scope="col"${c.right ? ' class="r"' : c === free ? ' class="fr"' : ''}${c.width ? ` style="width:${+c.width}px"` : ''}>${esc(c.name)}</th>`).join('');
        const body = rs.map((r, i) => `<tr>${cols.map((c, j) => {
          const x = cell(r[j]);
          return `<td class="${[c.right ? 'r' : '', c.mono ? 'mo' : '', 't' + c.tone].join(' ').replace(/\s+/g, ' ').trim()}"><div data-r="${i}" data-c="${j}">${x.color != null ? `<i>${dabSvg(x.color, S + i * 9 + j)}${esc(x.text)}</i>` : esc(x.text)}</div></td>`;
        }).join('')}</tr>`).join('');
        const marks = [];
        return {
          html: `<div class="tw"><table aria-label="${esc(spec.alt || '')}"><thead><tr>${head}</tr><tr role="presentation"><td class="rl" colspan="${cols.length}" role="presentation">${rule(W)}</td></tr></thead><tbody>${body}</tbody></table></div>` + (rs.length < rows.length ? `<button type="button" class="ink-more">Show all ${rows.length} rows</button>` : ''),
          marks,
          // After layout: a wide table scrolls in its own box with the first column fixed, and a cut cell gets a readout.
          after(b, stg) {
            const tw = stg.querySelector('.tw'), tb = tw.firstChild, btn = stg.querySelector('.ink-more');
            if (btn)
              btn.onclick = () => {
                st.open = true;
                v.again();
              };
            if (tb.offsetWidth > tw.clientWidth + 1) {
              tw.classList.add('sc');
              tb.querySelector('.rl').innerHTML = rule(tb.offsetWidth);
            }
            stg.querySelectorAll('td>div').forEach((d) => {
              if (d.scrollHeight <= d.clientHeight + 1 && d.scrollWidth <= d.clientWidth + 1) return;
              const x = cell(rs[+d.dataset.r][+d.dataset.c]);
              d.dataset.k = marks.length;
              marks.push(M(null, cols[+d.dataset.c].name || 'Cell', [String(x.text)], {row: +d.dataset.r, column: +d.dataset.c, text: String(x.text)}, {el: d, on: false}));
            });
          },
        };
      },
    };
  }, 'none');

  // underline: a hand-drawn rule. On an empty element it fills the width.
  // On an element with text it draws under the text.
  Ink.underline = (target, spec) => {
    spec = spec || {};
    const el = typeof target === 'string' ? document.querySelector(target) : target;
    if (!el) return console.error(`Ink.underline: no element matches ${JSON.stringify(String(target))}.`), null;
    const col = spec.color == null ? 'var(--input)' : pig(spec.color);
    if (!el.textContent.trim())
      return mount('underline', el, spec, (sp, S) => ({alt: 'Divider', draw: (W) => ({html: svg(W, 4, strokeP(L([[1, 2], [W - 1, 2]], 0.5, S, 8), col, 1.2))})}), 'none');
    sheet();
    const seed = hash(el.textContent), wrapEl = document.createElement('span');
    wrapEl.style.cssText = 'position:relative;display:inline-block';
    while (el.firstChild) wrapEl.appendChild(el.firstChild);
    el.appendChild(wrapEl);
    const sv = document.createElement('span');
    sv.style.cssText = 'position:absolute;left:0;right:0;bottom:0;height:4px;pointer-events:none';
    sv.setAttribute('aria-hidden', 'true');
    wrapEl.appendChild(sv);
    const draw = () => {
      const w = wrapEl.clientWidth;
      if (w) sv.innerHTML = svg(w, 4, strokeP(L([[1, 2], [w - 1, 2]], 0.5, seed, 8), col, spec.width ?? 1.6));
    };
    draw();
    typeof ResizeObserver === 'function' && new ResizeObserver(draw).observe(wrapEl);
    return {redraw: draw};
  };

  // ---------- the pen, for custom drawings: each call returns a path "d" string
  Ink.pen = {
    line: (pts, o = {}) => L(pts, o.amp ?? 0.9, o.seed ?? 1, o.step ?? 3, o.smooth),
    area: (pts, baseY, o = {}) => area(pts, baseY, o.amp ?? 0.9, o.seed ?? 1, o.smooth),
    rect: (x, y, w, h, o = {}) => rect(x, y, w, h, o.r ?? 4, o.amp ?? 0.7, o.seed ?? 1, o.topOnly, o.step),
    arc: (cx, cy, r, a0, a1, o = {}) => arcD(cx, cy, r, a0, a1, o.amp ?? 0.8, o.seed ?? 1),
    dab: (cx, cy, r, o = {}) => dab(cx, cy, r, o.seed ?? 1, o.n),
    arrow: (x0, y0, x1, y1, o = {}) => arrow(x0, y0, x1, y1, o.head, o.amp, o.seed),
    catmull: (pts) => catmull(pts),
    color: (n) => pig(n),
    glaze: (n, pct) => glaze(n, pct),
    rng,
    hash,
  };
  root.Ink = Ink;
})(typeof window !== 'undefined' ? window : this);
