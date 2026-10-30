// Redraws the clean icon geometry in source.mjs by hand, three times each.
//
// The hand: points along each stroke drift a little sideways (two slow
// waves), corners stay corners, a closed shape runs on past its start like a
// pen that did not lift in time, and dots are painted dabs like the Three
// voices mark. Each icon gets three versions (0, 1, 2) for the stop-motion
// shimmer.
//
// Run from the repo root:
//   node design/icons/build.mjs <node_modules dir with svg-path-properties>
// Writes, next to this script: svg/<name>.svg (currentColor, version 0),
// versions/<name>-<v>.svg (specimen colours) and specimen.html.
// Writes the app icons (currentColor) to crates/consort-desktop/assets/icons:
// <name>.svg (version 0), v1/<name>.svg and v2/<name>.svg.

import { createRequire } from 'node:module';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { groups } from './source.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(process.argv[2] || here, 'x.js'));
const { svgPathProperties } = require('svg-path-properties');

const STROKE = 1.9;
const STEP = 0.8;
const OVERRUN = 1.2;
const TONES = { text: '#E6E1D5', gold: '#E7BE62', lapis: '#77B4ED', verdigris: '#80C7A8', vermilion: '#F1735A', lilac: '#B2A0D6' };

function rng(seed) {
  let s = 0;
  for (const c of seed) s = (s * 31 + c.charCodeAt(0)) >>> 0;
  return () => { s = (s * 1664525 + 1013904223) >>> 0; return s / 4294967296; };
}
const f1 = (v) => (Math.round(v * 100) / 100).toString();

// One hand-drawn subpath: returns an SVG path string.
function handPath(sub, rand) {
  const props = new svgPathProperties(sub);
  const parts = props.getParts().filter((p) => p.length > 0.01);
  if (!parts.length) return '';
  const closed = /z\s*$/i.test(sub.trim());
  const ph1 = rand() * 6.28, ph2 = rand() * 6.28;
  const fr1 = 0.28 + rand() * 0.14, fr2 = 0.9 + rand() * 0.4;
  const amp1 = 0.2 + rand() * 0.07, amp2 = 0.08;
  const wob = (s) => amp1 * Math.sin(s * fr1 + ph1) + amp2 * Math.sin(s * fr2 + ph2);

  // Points with a corner flag. Arc length runs across the whole subpath.
  const pts = [];
  let s0 = 0;
  const push = (part, t, s) => {
    const p = part.getPointAtLength(t), tg = part.getTangentAtLength(t);
    const n = Math.hypot(tg.x, tg.y) || 1;
    const w = wob(s);
    pts.push({ x: p.x - (tg.y / n) * w, y: p.y + (tg.x / n) * w, tx: tg.x / n, ty: tg.y / n });
  };
  parts.forEach((part, i) => {
    const n = Math.max(1, Math.ceil(part.length / STEP));
    for (let k = i === 0 ? 0 : 1; k <= n; k++) push(part, (part.length * k) / n, s0 + (part.length * k) / n);
    s0 += part.length;
  });
  if (closed) {
    // The pen runs on past its start.
    let left = OVERRUN, s = s0;
    for (const part of parts) {
      const take = Math.min(left, part.length);
      const n = Math.max(1, Math.ceil(take / STEP));
      for (let k = 1; k <= n; k++) push(part, (take * k) / n, s + (take * k) / n);
      left -= take; s += take;
      if (left <= 0) break;
    }
  }
  // Corners: where the direction turns sharply, the curve does not smooth.
  for (let i = 1; i < pts.length - 1; i++) {
    const a = pts[i - 1], b = pts[i], c = pts[i + 1];
    const ux = b.x - a.x, uy = b.y - a.y, vx = c.x - b.x, vy = c.y - b.y;
    const cos = (ux * vx + uy * vy) / ((Math.hypot(ux, uy) * Math.hypot(vx, vy)) || 1);
    b.corner = cos < 0.8;
  }
  // Catmull-Rom to cubic Beziers, without smoothing across a corner.
  let d = `M${f1(pts[0].x)} ${f1(pts[0].y)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[i - 1] && !pts[i].corner ? pts[i - 1] : pts[i];
    const p1 = pts[i], p2 = pts[i + 1];
    const p3 = pts[i + 2] && !p2.corner ? pts[i + 2] : p2;
    const c1x = p1.x + (p2.x - p0.x) / 6, c1y = p1.y + (p2.y - p0.y) / 6;
    const c2x = p2.x - (p3.x - p1.x) / 6, c2y = p2.y - (p3.y - p1.y) / 6;
    d += `C${f1(c1x)} ${f1(c1y)} ${f1(c2x)} ${f1(c2y)} ${f1(p2.x)} ${f1(p2.y)}`;
  }
  return d;
}

// A painted dab, the same hand as the Three voices mark.
function dab(cx, cy, r, rand) {
  const n = 9, pts = [];
  const rot = rand() * 6.28;
  for (let k = 0; k < n; k++) {
    const a = (k / n) * Math.PI * 2 + rot;
    const rr = r * (1 + (rand() - 0.5) * 0.24);
    pts.push([cx + Math.cos(a) * rr, cy + Math.sin(a) * rr]);
  }
  let d = `M${f1((pts[0][0] + pts[1][0]) / 2)} ${f1((pts[0][1] + pts[1][1]) / 2)}`;
  for (let k = 1; k <= n; k++) {
    const p = pts[k % n], q = pts[(k + 1) % n];
    d += `Q${f1(p[0])} ${f1(p[1])} ${f1((p[0] + q[0]) / 2)} ${f1((p[1] + q[1]) / 2)}`;
  }
  return d + 'Z';
}

function drawIcon(name, icon, version, colour) {
  const rand = rng(`${name}:${version}`);
  const subs = (d) => (d || '').split(/(?=M)/).filter((s) => s.trim());
  const stroke = `fill="none" stroke="${colour}" stroke-width="${STROKE}" stroke-linecap="round" stroke-linejoin="round"`;
  let body = '';
  if (icon.faint) body += `<path d="${subs(icon.faint).map((s) => handPath(s, rand)).join('')}" ${stroke} stroke-opacity="0.3"/>`;
  if (icon.d) {
    const d = subs(icon.d).map((s) => handPath(s, rand)).join('');
    body += icon.fill ? `<path d="${d}" fill="${colour}" stroke="${colour}" stroke-width="${STROKE}" stroke-linejoin="round"/>` : `<path d="${d}" ${stroke}/>`;
  }
  for (const [x, y, r] of icon.dots || []) body += `<path d="${dab(x, y, r, rand)}" fill="${colour}"/>`;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24">${body}</svg>`;
}

const appIcons = join(here, '..', '..', 'crates', 'consort-desktop', 'assets', 'icons');
mkdirSync(join(appIcons, 'v1'), { recursive: true });
mkdirSync(join(appIcons, 'v2'), { recursive: true });
mkdirSync(join(here, 'svg'), { recursive: true });
mkdirSync(join(here, 'versions'), { recursive: true });
const all = [];
for (const [group, icons] of groups) {
  for (const [name, icon] of Object.entries(icons)) {
    all.push({ group, name, icon });
    const app = [0, 1, 2].map((v) => drawIcon(name, icon, v, 'currentColor') + '\n');
    writeFileSync(join(here, 'svg', `${name}.svg`), app[0]);
    writeFileSync(join(appIcons, `${name}.svg`), app[0]);
    writeFileSync(join(appIcons, 'v1', `${name}.svg`), app[1]);
    writeFileSync(join(appIcons, 'v2', `${name}.svg`), app[2]);
    const colour = TONES[icon.tone || 'text'];
    for (let v = 0; v < 3; v++) writeFileSync(join(here, 'versions', `${name}-${v}.svg`), drawIcon(name, icon, v, colour));
  }
}

// The specimen page. Images are absolute paths so the preview can inline them.
const base = process.env.IMG_BASE || join(here, 'versions');
const one = (name, size) => `<img width="${size}" height="${size}" src="${base}/${name}-0.svg">`;
const img = (name, size) => [0, 1, 2].map((v) => `<img class="v v${v}" width="${size}" height="${size}" src="${base}/${name}-${v}.svg">`).join('');
const tile = (name) => `<div class="tile"><span class="ic">${img(name, 24)}</span><span class="nm">${name}</span></div>`;
const heroes = ['needs-input', 'skill', 'routine', 'connector', 'mail', 'settings'];
let html = `<!doctype html><html><head><meta charset="utf-8"><style>
*{box-sizing:border-box}body{margin:0}
.panel{background:#131E1A;border:1px solid #27322E;border-radius:var(--radius);padding:16px;color:#F0EBDF;margin-bottom:12px}
.lbl{font-size:12px;color:#868F89;margin:2px 0 10px}
.ic{position:relative;display:inline-block;flex-shrink:0}
.ic .v{display:block}.ic .v1,.ic .v2{position:absolute;left:0;top:0;opacity:0}
@keyframes b0{0%,33.2%{opacity:1}33.3%,100%{opacity:0}}
@keyframes b1{0%,33.2%{opacity:0}33.3%,66.5%{opacity:1}66.6%,100%{opacity:0}}
@keyframes b2{0%,66.5%{opacity:0}66.6%,100%{opacity:1}}
.boil .v0,.tile:hover .v0{animation:b0 .375s infinite steps(1)}
.boil .v1,.tile:hover .v1{animation:b1 .375s infinite steps(1)}
.boil .v2,.tile:hover .v2{animation:b2 .375s infinite steps(1)}
.heroes{display:flex;justify-content:space-between;padding:4px 6px}
.grid{display:grid;grid-template-columns:repeat(8,1fr);gap:2px}
.tile{display:flex;flex-direction:column;align-items:center;gap:6px;padding:10px 2px 8px;border-radius:8px;cursor:default}
.tile:hover{background:#1B2623}
.nm{font-size:10.5px;color:#868F89;text-align:center;line-height:13px}
.small{display:flex;flex-wrap:wrap;gap:14px;background:#0F1815;border-radius:8px;padding:12px}
.ctx{display:flex;gap:16px}
.side{width:200px;background:#0F1815;border-radius:8px;padding:8px}
.row{display:flex;align-items:center;gap:8px;padding:5px 8px;border-radius:6px;font-size:13px;color:#B3BAB1}
.row.sel{background:#293732;color:#F0EBDF}.row .r{margin-left:auto}
.comp{flex:1;display:flex;flex-direction:column;gap:6px;justify-content:flex-end}
.field{border:1px solid #384440;background:#1B2623;border-radius:14px;padding:9px 10px 9px 14px;font-size:13px;color:#868F89;display:flex;align-items:center}
.field .ic{margin-left:auto}
.tools{display:flex;align-items:center;gap:12px;padding:0 6px;font-size:12px;color:#868F89}
</style></head><body>
<div class="panel"><div class="lbl">At 64 px, always shimmering. In the app an icon shimmers only on hover or while its state is live.</div><div class="heroes boil">${heroes.map((n) => `<span class="ic">${img(n, 64)}</span>`).join('')}</div></div>
`;
for (const [group, icons] of groups) {
  html += `<div class="panel"><div class="lbl">${group}. Point at an icon to see it shimmer.</div><div class="grid">${Object.keys(icons).map(tile).join('')}</div></div>\n`;
}
html += `<div class="panel"><div class="lbl">Every icon at 16 px, the size of the sidebar and the menus.</div><div class="small">${all.map((a) => one(a.name, 16)).join('')}</div></div>
<div class="panel"><div class="lbl">In place.</div><div class="ctx"><div class="side">
<div class="row"><span class="ic">${img('search', 16)}</span>Search</div>
<div class="row sel"><span class="ic">${img('channel', 16)}</span>sayso<span class="r ic">${img('needs-input', 16)}</span></div>
<div class="row"><span class="ic">${img('channel', 16)}</span>miss-lucy-tg<span class="r ic">${img('done', 16)}</span></div>
<div class="row"><span class="ic">${img('channel', 16)}</span>homelab<span class="r ic boil">${img('working', 16)}</span></div>
<div class="row"><span class="ic">${img('plus', 16)}</span>New channel</div>
<div class="row"><span class="ic">${img('worker', 16)}</span>bigboi<span class="r ic">${img('routine', 16)}</span></div>
<div class="row"><span class="ic">${img('settings', 16)}</span>Settings</div>
</div><div class="comp"><div class="field">Message sayso<span class="ic">${img('send', 16)}</span></div>
<div class="tools"><span class="ic">${img('plus', 16)}</span><span class="ic">${img('voice', 16)}</span><span class="ic">${img('attach', 16)}</span><span class="ic">${img('effort', 16)}</span>High</div></div></div></div>
</body></html>`;
writeFileSync(join(here, 'specimen.html'), html);
console.log(`${all.length} icons`);
