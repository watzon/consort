// Three voices mark: writes every animation frame as a standalone SVG.
// Port of the draw code in the Three voices prototype. The math and the
// rounding are the same. The only change: the filter id is fixed ("f"),
// so the output is byte-identical between runs.
//
// Run from the repo root: node design/mark/generate.mjs

import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, '..', '..');
const OUT = join(repo, 'crates', 'consort-desktop', 'assets', 'mark');
const LOGO = join(repo, 'design', 'logo');

const DARK = { gold: '#E7BE62', verd: '#80C7A8', lapis: '#77B4ED', blend: 'screen' };
const LIGHT = { gold: '#D9A93E', verd: '#3E9A78', lapis: '#3F82C8', blend: 'multiply' };

export function jit(seed, i, amp) {
  const x = Math.sin(seed * 12.9898 + i * 78.233) * 43758.5453;
  return ((x - Math.floor(x)) - 0.5) * 2 * amp;
}

// A painted dab. `boil` (0, 1 or 2) picks one of three hand-drawn edge versions.
export function dab(cx, cy, r, boil, i, rough) {
  const n = 10, pts = [];
  for (let k = 0; k < n; k++) {
    const a = (k / n) * Math.PI * 2 + i;
    const rr = r * (1 + jit(boil + 1, i * 17 + k, rough));
    pts.push([cx + Math.cos(a) * rr, cy + Math.sin(a) * rr * 0.93]);
  }
  let d = `M${(pts[0][0] + pts[1][0]) / 2},${(pts[0][1] + pts[1][1]) / 2}`;
  for (let k = 1; k <= n; k++) {
    const p = pts[k % n], q = pts[(k + 1) % n];
    d += ` Q${p[0].toFixed(2)},${p[1].toFixed(2)} ${((p[0] + q[0]) / 2).toFixed(2)},${((p[1] + q[1]) / 2).toFixed(2)}`;
  }
  return d + 'Z';
}

// Pose of the three voices for a state at frame f. Returns [{x, y, r, op}].
const BASE = [-Math.PI / 2, Math.PI / 6, 5 * Math.PI / 6];
const RADII = [17, 14.5, 12.5];
export function pose(state, f) {
  return RADII.map((r, i) => {
    let a = BASE[i], d = 15, op = 0.94, rr = r;
    if (state === 'working') {
      const step = f % 12;
      const speed = [1, -1.5, 2][i];
      a += step * speed * (Math.PI / 6);
      d = [10, 22, 24][i];
    }
    if (state === 'needs') {
      const beat = Math.floor(f / 4) % 2;
      d = i === 0 ? 0 : 28; op = i === 0 ? 1 : 0.55;
      rr = i === 0 ? r + (beat ? 2.5 : 0) : r * 0.6;
    }
    if (state === 'done') { d = 5; }
    return { x: 50 + Math.cos(a) * d, y: 51 + Math.sin(a) * d, r: rr, op };
  });
}
// Done holds still. Idle keeps its pose and boils its edges (the app plays
// it at half speed); working and needs-you boil as they move.
export function boilFor(state, f) { return state === 'done' ? 0 : f % 3; }

const UID = 'f';

// The inner markup (defs and group) for one frame.
export function markBody(state, f, pal, opts = {}) {
  const sharp = opts.sharp, boil = boilFor(state, f);
  const filt = sharp ? '' : `<defs><filter id="${UID}" x="-25%" y="-25%" width="150%" height="150%"><feTurbulence type="fractalNoise" baseFrequency="0.11" numOctaves="2" seed="${boil + 3}"/><feDisplacementMap in="SourceGraphic" scale="${opts.scale || 4}"/></filter></defs>`;
  const cols = [pal.gold, pal.verd, pal.lapis];
  const body = pose(state, f).map((p, i) =>
    `<path d="${dab(p.x, p.y, p.r, boil, i, sharp ? 0.03 : 0.1)}" fill="${cols[i]}" fill-opacity="${p.op}" style="mix-blend-mode:${pal.blend}"/>`
  ).join('');
  return filt + `<g style="isolation:isolate" ${sharp ? '' : `filter="url(#${UID})"`}>${body}</g>`;
}

const svg = (attrs, inner) => `<svg xmlns="http://www.w3.org/2000/svg" ${attrs}>${inner}</svg>\n`;

// Frames per state: idle loops over its 3 edge versions, done holds still.
// Working loops over 12, needs over 24.
const STATES = [['idle', 3], ['working', 12], ['needs', 24], ['done', 1]];

const sets = [
  ['dark', DARK, false],
  ['light', LIGHT, false],
  ['dark-sharp', DARK, true],
  ['light-sharp', LIGHT, true],
];

let total = 0;
const counts = {};
for (const [dir, pal, sharp] of sets) {
  mkdirSync(join(OUT, dir), { recursive: true });
  for (const [state, n] of STATES) {
    for (let f = 0; f < n; f++) {
      const still = state === 'idle' || state === 'done';
      const viewBox = sharp && still ? '20 21 60 60' : '10 11 80 80';
      const size = sharp ? 20 : 44;
      const name = `${state}-${String(f).padStart(2, '0')}.svg`;
      writeFileSync(join(OUT, dir, name), svg(`width="${size}" height="${size}" viewBox="${viewBox}"`, markBody(state, f, pal, { sharp })));
      total++;
      counts[dir] = (counts[dir] || 0) + 1;
    }
  }
}

// Logo and app icon (idle pose, filtered).
mkdirSync(LOGO, { recursive: true });
const TILE = { dark: '#1B2623', light: '#E9F2ED' };
// Fit the idle bounding box (centre of the three dabs) into 70/96 of the tile.
const idle = pose('idle', 0);
const x0 = Math.min(...idle.map((p) => p.x - p.r)), x1 = Math.max(...idle.map((p) => p.x + p.r));
const y0 = Math.min(...idle.map((p) => p.y - p.r * 0.93)), y1 = Math.max(...idle.map((p) => p.y + p.r * 0.93));
const S = 1024, fit = (70 / 96) * S / Math.max(x1 - x0, y1 - y0);
const tx = S / 2 - ((x0 + x1) / 2) * fit, ty = S / 2 - ((y0 + y1) / 2) * fit;
for (const [mode, pal] of [['dark', DARK], ['light', LIGHT]]) {
  writeFileSync(join(LOGO, `mark-${mode}.svg`), svg('width="100" height="100" viewBox="0 0 100 100"', markBody('idle', 0, pal)));
  const tile = `<rect width="${S}" height="${S}" rx="${(S * 22 / 96).toFixed(2)}" fill="${TILE[mode]}"/>`;
  const mark = `<g transform="translate(${tx.toFixed(2)} ${ty.toFixed(2)}) scale(${fit.toFixed(4)})">${markBody('idle', 0, pal)}</g>`;
  writeFileSync(join(LOGO, `app-icon-${mode}.svg`), svg(`width="${S}" height="${S}" viewBox="0 0 ${S} ${S}"`, tile + mark));
}

// Extent check: every dab of every state plus the displacement must sit inside 10 11 80 80.
let minX = 1e9, minY = 1e9, maxX = -1e9, maxY = -1e9;
for (const [state, n] of STATES) for (let f = 0; f < n; f++) for (const p of pose(state, f)) {
  const m = p.r * 1.1; // edge roughness 0.1
  minX = Math.min(minX, p.x - m); maxX = Math.max(maxX, p.x + m);
  minY = Math.min(minY, p.y - m * 0.93); maxY = Math.max(maxY, p.y + m * 0.93);
}
const disp = 4 / 2; // feDisplacementMap moves a pixel by at most scale/2
console.log(JSON.stringify(counts), `total ${total}`);
console.log(`extent x ${(minX - disp).toFixed(2)}..${(maxX + disp).toFixed(2)} y ${(minY - disp).toFixed(2)}..${(maxY + disp).toFixed(2)} (viewBox x 10..90 y 11..91)`);
