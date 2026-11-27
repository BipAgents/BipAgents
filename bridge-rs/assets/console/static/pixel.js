// The Bips for the websites, drawn like the apps draw them (PixelEngine.swift / PixelEngine.kt, the design page
// « Les Bips se déguisent »): every number comes from scripts/pixel/pixel.json, which the build writes next to this
// file as pixel-data.js. Body cells of P, face pixels of U = P/3, the mouth in the eyes' colour, each open eye one
// solid rounded square; states with key times (idle, think, talk, listen, work, happy, sleep, surprised, error, grumpy), the
// tap reactions, and the costumes with their seasons. Same « Animalese » as BipBabble.swift.
import D from "./pixel-data.js";

const { P, U, GAP, BOTTOM, CX, INK, LIGHT, COLORS, BODIES, EYE_STYLES, MOUTHS, GLYPHS, SPARK_FRAMES, SPIRAL_FRAMES, STATES, REACTIONS,
  REACTION_ORDER, MOUTH_MIN, MOUTH_MAX, COSTUMES, SEASONS } = D;
export { INK, COSTUMES, SEASONS };

const W = typeof window === "object" ? window : null;
export const REDUCED = !!(W && W.matchMedia && W.matchMedia("(prefers-reduced-motion: reduce)").matches);

// Each kind of agent's colour, as in the apps (AgentCategory.pixelColor); tint is the site's pale card ground.
// main/deep come from pixel.json (deep is for drawing only: as text use site.css's --<tone>-deep).
export const CATEGORIES = {
  wellness: { color: "menthe", tint: "#EEF6EA", pitch: 520, voice: "mousse" },
  finance: { color: "miel", tint: "#FAF3E3", pitch: 410, voice: "ours" },
  daily: { color: "peche", tint: "#FBEFEA", pitch: 600, voice: "colibri" },
  work: { color: "bleuet", tint: "#ECF0FD", pitch: 450, voice: "galet" },
  learning: { color: "lavande", tint: "#F2EFFB", pitch: 560, voice: "lumen" },
  home: { color: "lagon", tint: "#E9F5F4", pitch: 490, voice: "ours" },
  creative: { color: "rose", tint: "#FBEDF2", pitch: 640, voice: "lumen" },
  tech: { color: "perle", tint: "#F1F2F5", pitch: 470, voice: "galet" },
};
Object.entries(CATEGORIES).forEach(([id, c], i) => {
  Object.assign(c, { id, index: i, main: COLORS[c.color].body, deep: COLORS[c.color].deep });
});

// The site's words for a Bip's mood → the apps' states.
const MOODS = {
  happy: "idle", idle: "idle", thinking: "think", think: "think", speaking: "talk", talk: "talk", listening: "listen",
  listen: "listen", working: "work", work: "work", sleeping: "sleep", sleep: "sleep", content: "happy",
  giggling: "happy", surprised: "surprised", error: "error", grumpy: "grumpy", sulking: "grumpy",
};
const stateOf = mood => STATES[MOODS[mood] || mood] || STATES.idle;

// -- Engine (the page's evalState / drawPixel / viewBoxOf) ------------------------------------------------------------

// spin: the dizzy eyes' frame (SPIRAL_FRAMES); stars: the dizzy stars' step around the head.
const FX = ["dots", "sound", "sparks", "scan", "heart", "z", "bang", "spin", "stars"];
function stepAt(keys, t) { let v = keys[0][1]; for (const [k, val] of keys) { if (k <= t) v = val; else break; } return v; }
function smoothAt(keys, t) {
  if (t <= keys[0][0]) return keys[0][1];
  for (let i = 0; i < keys.length - 1; i++) {
    const [a, va] = keys[i], [b, vb] = keys[i + 1];
    if (t < b) { let u = (t - a) / (b - a); u = u * u * (3 - 2 * u); return va + (vb - va) * u; }
  }
  return keys[keys.length - 1][1];
}
function evalState(st, t) {
  const tr = st.tracks, d = {}, m = { x: 0, y: 0, rot: 0, sx: 1, sy: 1 };
  const eyes = tr.eyes ? stepAt(tr.eyes, t) : "open";
  d.eyeL = tr.eyeL ? stepAt(tr.eyeL, t) : eyes; d.eyeR = tr.eyeR ? stepAt(tr.eyeR, t) : eyes;
  d.gaze = tr.gaze ? stepAt(tr.gaze, t) : [0, 0];
  const both = tr.brows ? stepAt(tr.brows, t) : [0, 0];
  d.browL = tr.browL ? stepAt(tr.browL, t) : both; d.browR = tr.browR ? stepAt(tr.browR, t) : both;
  d.mouth = tr.mouth ? stepAt(tr.mouth, t) : "smile";
  d.glitch = tr.glitch ? stepAt(tr.glitch, t) : null;
  d.fx = {};
  for (const k of FX) if (tr[k] && tr[k].length) { const v = stepAt(tr[k], t); if (v !== null && v !== -1) d.fx[k] = v; }
  for (const k of ["x", "y", "rot", "sx", "sy"]) if (tr[k]) m[k] = smoothAt(tr[k], t);
  if (st.wave) for (const [k, [a, p]] of Object.entries(st.wave)) m[k] += a * Math.sin(2 * Math.PI * t / p);
  return { d, m };
}

const fmt = n => Math.round(n * 100) / 100;
const glyphCache = new Map();
function glyph(rows) {
  const key = rows.join("/");
  if (glyphCache.has(key)) return glyphCache.get(key);
  const h = rows.length, w = rows[0].length, pts = [];
  rows.forEach((row, y) => [...row].forEach((ch, x) => { if (ch === "#") pts.push([x - (w - 1) / 2, y - (h - 1) / 2]); }));
  const g = { pts, w, h };
  glyphCache.set(key, g);
  return g;
}
const EYE = EYE_STYLES.petit;
function palette(key) {
  const c = COLORS[key] || COLORS.peche;
  if (key === "charbon") return { body: c.body, deep: "#555963", eye: LIGHT, brow: "#D9DCE3", mouth: LIGHT, hl: null, outline: "#3A3D45", scan: "#FFFFFF", scanA: 0.14, fx: "#8E94A1" };
  if (key === "craie") return { body: "#FFFFFF", deep: c.deep, eye: INK, brow: INK, mouth: INK, hl: "#FFFFFF", outline: "#C9CDD5", scan: "#BFC3CC", scanA: 0.4, fx: "#8C929F" };
  return { body: c.body, deep: c.deep, eye: INK, brow: INK, mouth: INK, hl: "#FFFFFF", outline: null, scan: "#FFFFFF", scanA: 0.55, fx: c.deep };
}
function sq(x, y, s, fill, extra, r = 0.3) {
  return `<rect x="${fmt(x - s / 2)}" y="${fmt(y - s / 2)}" width="${fmt(s)}" height="${fmt(s)}" rx="${fmt(s * r)}" fill="${fill}"${extra || ""}/>`;
}
// Glyph squares that touch (« plein », 2026-10-29: u + 0.06), as the apps draw them.
function drawGlyph(rows, x, y, pitch, fill, extra) {
  const s = pitch + 0.06;
  return glyph(rows).pts.map(([gx, gy]) => sq(x + gx * pitch, y + gy * pitch, s, fill, extra, 0.12)).join("");
}
// The page's `mix` (each channel rounded).
function mix(a, b, t) {
  const pa = [1, 3, 5].map(i => parseInt(a.slice(i, i + 2), 16)), pb = [1, 3, 5].map(i => parseInt(b.slice(i, i + 2), 16));
  return "#" + pa.map((x, k) => Math.round(x + (pb[k] - x) * t).toString(16).padStart(2, "0")).join("");
}
// A body block « plein » (« comme sur notre icône »): a square of pitch `s` touching its neighbours, its seam a deeper
// tone of its own colour (the palette's outline when it has one), never the ground showing through.
function block(x, y, s, fill, deep, outline, extra = "", rot = 0) {
  const seam = outline || mix(fill, deep, 0.28), inner = s * 0.86;
  const turn = rot ? ` transform="rotate(${fmt(rot)} ${fmt(x)} ${fmt(y)})"` : "";
  return sq(x, y, s + 0.3, seam, extra + turn, 0.08 * s / (s + 0.3)) + sq(x, y, inner, fill, extra + turn, 0.12 / 0.86);
}
// One eye centred at (x, y): open and wide eyes one solid rounded square, closed a bar, the others pixel lines.
function eyeShape(shape, x, y, gx, gy, fill, extra = "", spin = 0, mirrored = false) {
  const ax = x + gx * U;
  switch (shape) {
    case "spiral": {   // a square spiral turning; the right eye mirrored, so the two turn opposite ways
      const rows = SPIRAL_FRAMES[((spin % SPIRAL_FRAMES.length) + SPIRAL_FRAMES.length) % SPIRAL_FRAMES.length];
      return drawGlyph(mirrored ? rows.map(r => [...r].reverse().join("")) : rows, ax, y, U * 0.75, fill, extra);
    }
    case "wide": return sq(ax, y + gy * U, 2.6 * U, fill, extra, 0.25);
    case "closed": return `<rect x="${fmt(ax - U)}" y="${fmt(y)}" width="${fmt(2 * U)}" height="${fmt(0.6 * U)}" rx="${fmt(0.3 * U)}" fill="${fill}"${extra}/>`;
    case "happy": return drawGlyph([".#.", "#.#"], ax, y, U * 0.8, fill, extra);
    case "sleep": return drawGlyph(["#.#", ".#."], ax, y + 0.5 * U, U * 0.8, fill, extra);
    case "dash": return drawGlyph(["###"], ax, y + gy * U, U, fill, extra);
    default: return sq(ax, y + gy * U, glyph(EYE.open).w * U * 0.96, fill, extra, 0.22);
  }
}
const costumeOf = look => (look.costume && COSTUMES[look.costume]) || null;
function bodyOf(CO, bf) {
  const ob = CO && CO.body;
  let B = BODIES.standard;
  if (ob && (ob.rows || ob.frames)) {
    B = Object.assign({}, B, { rows: ob.frames ? ob.frames[bf || 0] : ob.rows });
    for (const k of ["eyeRow", "eyeCols", "mouthRow"]) if (ob[k] !== undefined) B[k] = ob[k];
  }
  return B;
}
function paletteOf(look, CO) {
  const ob = CO && CO.body;
  const base = palette((ob && ob.color) || look.color);
  return ob && ob.colors ? Object.assign({}, base, ob.colors) : base;
}
function geom(B) {
  const cols = B.rows[0].length, rows = B.rows.length;
  const left = CX - cols * P / 2, top = BOTTOM - rows * P, right = CX + cols * P / 2;
  const cx = c => left + c * P + P / 2, cy = r => top + r * P + P / 2;
  return { cols, rows, left, top, right, cx, cy, eyeY: cy(B.eyeRow), mouthY: cy(B.mouthRow) };
}
// The mouth never thinner than MOUTH_MIN screen px per face pixel when drawn small (as in the apps).
const mouthScale = ppu => ppu ? Math.min(MOUTH_MAX, Math.max(1, MOUTH_MIN / (U * ppu))) : 1;
function anchorPt(at, G, mouth, mk) {
  switch (at) {
    case "bottom": return [CX, BOTTOM];
    case "eyes": return [CX, G.eyeY];
    case "mouth": return [CX, G.mouthY];
    case "mouthBottom": {
      const M = MOUTHS[mouth] || MOUTHS.smile, mu = U * (mk || 1);
      return [CX + (M.dx || 0) * mu, G.mouthY + (M.dy || 0) * mu + M.g.length * mu / 2];
    }
    default: return [CX, G.top];
  }
}
function frameState(CO, clock) {
  if (!CO) return null;
  const f = i => Math.floor(clock / i.period) % i.frames.length;
  return { b: CO.body && CO.body.frames ? f(CO.body) : 0, i: CO.items.map(it => it.frames ? f(it) : 0) };
}
function drawItem(it, idx, G, d, CO, fr, mk) {
  let rows = it.frames ? it.frames[fr.i[idx]] : it.rows, rot = it.rot || 0, dx = it.dx, dy = it.dy;
  const w = it.when && (it.when[d.eyeL] || it.when[d.eyeR]);
  if (w) { rows = w.rows || rows; if (w.rot !== undefined) rot = w.rot; if (w.dx !== undefined) dx = w.dx; if (w.dy !== undefined) dy = w.dy; }
  const pitch = U * (it.scale || 1), [ax, ay] = anchorPt(it.at, G, d.mouth, mk);
  const ox = ax + dx * U, oy = ay + dy * U;
  const pv = it.pivot || [rows[0].length / 2, rows.length];
  const pvx = ox + pv[0] * pitch, pvy = oy + pv[1] * pitch;
  let s = "";
  rows.forEach((row, j) => [...row].forEach((ch, i) => {
    if (ch === "." || ch === " ") return;
    const p = CO.palette[ch];
    if (!p) return;
    const [fill, stroke] = Array.isArray(p) ? p : [p];
    s += sq(ox + (i + 0.5) * pitch, oy + (j + 0.5) * pitch, pitch * 0.94, fill, stroke ? ` stroke="${stroke}" stroke-width="${fmt(pitch * 0.14)}"` : "");
  }));
  if (rot) s = `<g transform="rotate(${rot} ${fmt(pvx)} ${fmt(pvy)})">${s}</g>`;
  return `<g data-it="${idx}" data-pv="${fmt(pvx)} ${fmt(pvy)}"${it.alpha ? ` opacity="${it.alpha}"` : ""}>${s}</g>`;
}
function itemDyn(it, clock, m) {
  let x = 0, y = 0, rot = 0, op = 1;
  if (it.wave) for (const [k, [a, p, ph]] of Object.entries(it.wave)) {
    const v = a * Math.sin(2 * Math.PI * (clock / p + (ph || 0)));
    if (k === "x") x += v; else if (k === "y") y += v; else rot += v;
  }
  if (it.lift) y += it.lift * m.y;
  if (it.blink) { const [p, on, ph] = it.blink; op = ((clock / p + (ph || 0)) % 1) < on ? 1 : 0; }
  return { x, y, rot, op };
}

/** The figure's squares for a look ({color, costume, noHL}) and a pose, as SVG markup (no <svg>). */
function drawPixel(look, pose, fr, ppu) {
  const CO = costumeOf(look), B = bodyOf(CO, fr ? fr.b : 0), C = paletteOf(look, CO), d = pose.d, fx = d.fx;
  const G = geom(B), { left, top, right, cx, cy } = G;
  const o = [], mk = mouthScale(ppu);
  const stroke = "";
  const cell = (x, y, fill, extra) => block(x, y, P, fill, C.deep, C.outline, extra || "");
  const layer = name => { if (CO) CO.items.forEach((it, i) => { if ((it.layer || "top") === name) o.push(drawItem(it, i, G, d, CO, fr, mk)); }); };
  const g = d.glitch;
  layer("back");
  const bodyCells = [];
  B.rows.forEach((row, r) => [...row].forEach((ch, c) => {
    if (ch === ".") return;
    const p = ch === "#" ? C.body : (CO.palette[ch] || C.body), fill = Array.isArray(p) ? p[0] : p;
    if (g && g.r === r && g.c === c) { bodyCells.push(cell(cx(c), cy(r), fill, ` fill-opacity="0.35"`)); return; }
    bodyCells.push(cell(cx(c), cy(r), fill, stroke));
    if (fx.scan === r) bodyCells.push(sq(cx(c), cy(r), P * 0.86, C.scan, ` fill-opacity="${C.scanA}"`, 0.12 / 0.86));
  }));
  const op = CO && CO.body && CO.body.opacity;
  o.push(op ? `<g opacity="${op}">${bodyCells.join("")}</g>` : bodyCells.join(""));
  if (g) o.push(cell(cx(g.c) + g.dx * U, cy(g.r) + g.dy * U, C.deep));
  layer("body");
  // Eyes: an open (or wide) eye is one solid rounded square, a closed one a bar; happy, sleeping and busy eyes are lines.
  const eyeY = G.eyeY, openG = glyph(EYE.open), face = U + 0.06;
  [0, 1].forEach(side => {
    const shape = side ? d.eyeR : d.eyeL, flat = shape === "closed" || shape === "happy" || shape === "sleep" || shape === "spiral";
    o.push(eyeShape(shape, cx(B.eyeCols[side]), eyeY, d.gaze[0], flat ? 0 : d.gaze[1], C.eye, "", fx.spin || 0, side === 1));
  });
  layer("face");
  // Brows show when they move.
  const bl = d.browL, br = d.browR;
  if (bl[0] || bl[1] || br[0] || br[1]) {
    const base = -(openG.h - 1) / 2 - 2 + Math.min(0, d.gaze[1]);
    [[0, bl], [1, br]].forEach(([side, [dy, tilt]]) => {
      const ax = cx(B.eyeCols[side]);
      [-1, 0, 1].forEach(i => {
        const outer = side === 0 ? i === -1 : i === 1, inner = side === 0 ? i === 1 : i === -1;
        const lift = tilt > 0 && outer ? tilt : tilt < 0 && inner ? -tilt : 0;
        o.push(sq(ax + i * U, eyeY + (base + dy - lift) * U, face, C.brow));
      });
    });
  }
  const M = MOUTHS[d.mouth] || MOUTHS.smile;
  o.push(drawGlyph(M.g, CX + (M.dx || 0) * U * mk, G.mouthY + (M.dy || 0) * U * mk, U * mk, C.mouth));
  layer("top");
  // Effects
  if (fx.dots) { const spots = [[-0.5, -1.6, 0.85], [1.4, -3.6, 1.1], [3.7, -5.8, 1.4]]; for (let i = 0; i < fx.dots; i++) { const [x, y, s] = spots[i]; o.push(sq(right + x * U, top + y * U, s * U, C.fx)); } }
  if (fx.sound) for (let j = 0; j < fx.sound; j++) { const n = 2 + j, x = left - (1.8 + 1.5 * j) * U; for (let k = 0; k < n; k++) o.push(sq(x, eyeY + (k - (n - 1) / 2) * U * 0.95, U * 0.8, C.fx)); }
  if (fx.sparks !== undefined) for (const [x, y, big] of SPARK_FRAMES[fx.sparks % SPARK_FRAMES.length]) {
    const px = CX + x * U, py = top + y * U, s = U * 0.8;
    o.push(sq(px, py, s, "#E7B43C"));
    if (big) for (const [ox, oy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) o.push(sq(px + ox * s, py + oy * s, s * 0.8, "#E7B43C", ` fill-opacity="0.75"`));
  }
  if (fx.heart !== undefined && fx.heart >= 0) o.push(drawGlyph(GLYPHS.heart, right + 1.6 * U, top - (2 + fx.heart) * U, U * 0.62, "#EC6F8F", fx.heart >= 4 ? ` fill-opacity="0.5"` : ""));
  if (fx.z !== undefined) for (let i = 0; i < 3; i++) { const k = (fx.z + i * 3) % 8; if (k >= 7) continue; o.push(drawGlyph(GLYPHS.z, right - 0.5 * U + k * 0.7 * U, top - (0.5 + k * 1.3) * U, U * (0.5 + k * 0.07), C.fx, k >= 5 ? ` fill-opacity="0.5"` : "")); }
  if (fx.bang) o.push(drawGlyph(GLYPHS.bang, right + 1.4 * U, top - 2.6 * U, U * 0.95, C.fx));
  // The dizzy stars: three on a flat ring above the head, an eighth of a turn per step; those behind it fainter.
  if (fx.stars !== undefined) for (let i = 0; i < 3; i++) {
    const a = 2 * Math.PI * (fx.stars / 8 + i / 3);
    o.push(drawGlyph(GLYPHS.star, CX + Math.cos(a) * 5.5 * U, top + (-2.8 + Math.sin(a) * 1.4) * U, U * 0.7, "#E7B43C", Math.sin(a) < 0 ? ` fill-opacity="0.55"` : ""));
  }
  return o.join("");
}

// -- The animations of 2026-10-29 (design page « Animations des Bips », internal/design/bip-animations-2026-10-29.html)
// Each a pure function of time, from the page's formulas (its ANIMS and FUN), the same as the apps' BipMotions.swift and
// BipMotions.kt. A frame lists what to draw: blocks {k:"cell", i, j, dx, dy, rot, s, op, fill?}, the face {k:"face", dx,
// dy, op, gaze, eyes, mouth}, glyphs, rounded rects and polygons. site/check-motions.mjs compares the frames to
// scripts/pixel/motion-frames.json (the page's own, at 0.4, 1.2 and 2.0 s).

/** The thinking animations, alternating (« today » is the think state's own); the small ones read at 22–56 px. */
export const THINKING = ["today", "chantier", "vague", "construit", "bulle", "puzzle", "jongle", "scanner", "relief", "empile"];
export const THINKING_SMALL = ["vague", "chantier", "jongle", "scanner", "construit"];
/** The moments played once: their length (s). */
export const MOMENTS = { done: 2.2, oops: 3.6, wave: 1.8, born: 3.8, sent: 2.4 };
const MOTION_STILL = { construit: 2.4, empile: 2.6, done: 0.4, oops: 0.3, born: 3.0, sent: 1.0 };
const M_LEFT = 60 - 3 * P, M_TOP = 106 - 6 * P, CS = P - GAP, GOLD = "#FFC93C", CHEEK = "#F58BA6";
const CELLS = [];
BODIES.standard.rows.forEach((r, j) => [...r].forEach((c, i) => { if (c === "#") CELLS.push([i, j]); }));
const CELLSET = new Set(CELLS.map(c => c.join()));
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const lerp = (a, b, t) => a + (b - a) * t;
const easeOut = t => 1 - Math.pow(1 - clamp(t, 0, 1), 3);
const easeInOut = t => { t = clamp(t, 0, 1); return t < .5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2; };
const back = t => { t = clamp(t, 0, 1); const c1 = 1.7, c3 = c1 + 1; return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2); };
function rng(seed) { let s = seed >>> 0; return () => ((s = (s * 1664525 + 1013904223) >>> 0) / 4294967296); }
function plan(seed) {
  const r = rng(seed), order = CELLS.map((c, n) => [r(), n]).sort((a, b) => a[0] - b[0]).map(x => x[1]);
  const dirs = CELLS.map(() => { const a = r() * Math.PI * 2, d = 46 + r() * 26; return [Math.cos(a) * d, Math.sin(a) * d - 10, (r() - .5) * 260]; });
  return { dirs, rank: order.reduce((m, n, k) => (m[n] = k, m), {}) };
}
const cellX = i => M_LEFT + i * P + GAP / 2, cellY = j => M_TOP + j * P + GAP / 2;
const P_BUILD = plan(7), P_NEW = plan(21);
const SWAPS = [[[1, 1], [4, 3]], [[2, 4], [5, 2]], [[0, 3], [3, 1]], [[4, 0], [1, 5]], [[5, 4], [2, 1]]];
const cellM = (out, i, j, o = {}) => { const it = { k: "cell", i, j, dx: o.dx || 0, dy: o.dy || 0, rot: o.rot || 0, s: o.s ?? 1, op: o.op ?? 1 }; if (o.fill) it.fill = o.fill; out.push(it); };
const bodyM = (out, o = {}) => CELLS.forEach(([i, j], n) => cellM(out, i, j, o.each ? o.each(i, j, n) : o));
const faceM = (out, o = {}) => out.push({ k: "face", dx: o.dx || 0, dy: o.dy || 0, op: o.op ?? 1, gaze: o.gaze || [0, 0], eyes: o.eyes || "open", mouth: o.mouth || "smile" });
const glyphM = (out, rows, cx, cy, u, fill, op = 1) => out.push({ k: "glyph", rows, cx, cy, u, fill, op });
const rectM = (out, x, y, w, h, rx, fill, o = {}) => { const it = { k: "rect", x, y, w, h, rx, fill, op: o.op ?? 1, rot: o.rot || 0 }; if (o.ox !== undefined) { it.ox = o.ox; it.oy = o.oy; } if (o.stroke) it.stroke = o.stroke; out.push(it); };
const MOTIONS = {
  chantier(t, out) {
    const fly = [[4, 0, 0], [5, 1, 1.1]], D = 2.2;
    CELLS.forEach(([i, j]) => { const fl = fly.find(f => f[0] === i && f[1] === j);
      if (!fl) return cellM(out, i, j);
      const k = ((t + fl[2]) % D) / D, up = k < .5 ? easeOut(k / .5) : 1 - easeInOut((k - .5) / .5);
      cellM(out, i, j, { dx: up * (10 + 4 * fl[2]), dy: -up * (16 + 6 * fl[2]), rot: up * (fl[2] ? -40 : 35), s: 1 - up * .18 }); });
    faceM(out, { gaze: [1, -1], mouth: "hmm" });
  },
  vague(t, out) {
    const lift = i => Math.max(0, Math.sin(2 * Math.PI * (t / 1.3 - i / 7))) * 3.4;
    bodyM(out, { each: i => ({ dy: -lift(i) }) }); faceM(out, { gaze: [Math.sin(t * 2.4) > 0 ? 1 : -1, -1], mouth: "hmm", dy: -(lift(1) + lift(4)) / 2 });
  },
  construit(t, out) {
    t %= 4.2;
    CELLS.forEach(([i, j], n) => { const r = P_BUILD.rank[n], [ddx, ddy, rr] = P_BUILD.dirs[n];
      const tin = clamp((t - r * 0.045) / .5, 0, 1), tout = clamp((t - 3.0 - (33 - r) * 0.025) / .45, 0, 1);
      const a = back(tin) * (1 - easeInOut(tout)); if (a <= 0.001) return;
      cellM(out, i, j, { dx: ddx * (1 - a), dy: ddy * (1 - a), rot: rr * (1 - a), s: .55 + .45 * a, op: clamp(a * 1.4, 0, 1) }); });
    const fop = clamp((t - 1.7) / .3, 0, 1) * (1 - clamp((t - 2.95) / .2, 0, 1));
    if (fop > 0) faceM(out, { op: fop, gaze: [0, -1], mouth: "smile" });
  },
  bulle(t, out) {
    bodyM(out); faceM(out, { gaze: [1, -1], mouth: "hmm" });
    [[90, 27, 3.2], [96, 20, 4.2]].forEach(([x, y, s]) => rectM(out, x, y, s, s, s * .3, "#C9CED9"));
    rectM(out, 94, 0, 26, 14, 4, "#FFFFFF", { op: .96 }); rectM(out, 94, 0, 26, 14, 4, "none", { stroke: "#C9CED9" });
    for (let k = 0; k < 3; k++) { const on = Math.floor((t * 2.6) % 4) > k; rectM(out, 94 + 5 + k * 6.5, 5, 3.6, 3.6, 1, on ? "#6B7180" : "#D9DCE3"); }
  },
  puzzle(t, out, body) {
    const D = .9, n = Math.floor(t / D) % SWAPS.length, k = (t % D) / D, e = easeInOut(clamp(k / .7, 0, 1));
    const [[ai, aj], [bi, bj]] = SWAPS[n];
    CELLS.forEach(([i, j]) => { if (!(i === ai && j === aj) && !(i === bi && j === bj)) cellM(out, i, j); });
    const hop = Math.sin(Math.PI * e) * 14;
    cellM(out, ai, aj, { dx: (bi - ai) * P * e, dy: (bj - aj) * P * e - hop, rot: e * 180, fill: mix(body, "#FFFFFF", .25) });
    cellM(out, bi, bj, { dx: (ai - bi) * P * e, dy: (aj - bj) * P * e - hop * .6, rot: -e * 180 });
    faceM(out, { gaze: [((ai + bi) / 2 - 2.5) > 0 ? 1 : -1, -1], mouth: "hmm" });
  },
  jongle(t, out) {
    const D = 1.1, k = (t % D) / D, h = 4 * k * (1 - k), y = 22 - h * 30, x = 60 + Math.sin(t * 2.9) * 10, land = k < .08 || k > .92 ? 1.2 : 0;
    bodyM(out, { each: (i, j) => ({ dy: j <= 1 && Math.abs(cellX(i) + CS / 2 - x) < 13 ? land : 0 }) });
    faceM(out, { gaze: [x > 60 ? 1 : -1, h > .5 ? -1 : 0], mouth: "smile" });
    rectM(out, x - 4, y - 4, 8, 8, 2, GOLD, { rot: t * 300 });
  },
  scanner(t, out, body) {
    const D = 1.8, bx = lerp(-12, 132, (t % D) / D);
    bodyM(out, { each: (i, j) => { const cx = cellX(i) + CS / 2 + (j - 2.5) * 4; return { fill: mix(body, "#FFFFFF", clamp(1 - Math.abs(cx - bx) / 16, 0, 1) * .7) }; } });
    faceM(out, { gaze: [bx < 60 ? -1 : 1, 0], mouth: "hmm" });
  },
  relief(t, out, body, deep) {
    const top = mix(body, "#FFFFFF", .4), side = mix(body, deep, .78);
    const k = (i, j) => 2 + 3.2 * (0.5 + 0.5 * Math.sin(2 * Math.PI * (t / 1.6 - (i + j) / 10)));
    CELLS.slice().sort((a, b) => (a[0] - b[0]) || (b[1] - a[1])).forEach(([i, j]) => { const x = cellX(i), y = cellY(j), kk = k(i, j);
      if (!CELLSET.has([i + 1, j].join())) out.push({ k: "poly", pts: [[x + CS, y], [x + CS + kk, y - kk], [x + CS + kk, y + CS - kk], [x + CS, y + CS]], fill: side, op: 1 });
      if (!CELLSET.has([i, j - 1].join())) out.push({ k: "poly", pts: [[x, y], [x + kk, y - kk], [x + CS + kk, y - kk], [x + CS, y]], fill: top, op: 1 }); });
    CELLS.forEach(([i, j]) => rectM(out, cellX(i) - .2, cellY(j) - .2, CS + .4, CS + .4, 1.2, body));
    faceM(out, { gaze: [1, -1], mouth: "hmm" });
  },
  empile(t, out) {
    t %= 3.6;
    CELLS.forEach(([i, j]) => { const start = (5 - j) * .22 + (i % 3) * .05, k = clamp((t - start) / .45, 0, 1), fall = 1 - k * k;
      const bounce = k >= 1 ? Math.max(0, Math.sin((t - start - .45) * 14) * 1.2 * Math.exp(-(t - start - .45) * 6)) : 0;
      if (k <= 0) return; cellM(out, i, j, { dy: -fall * 90 - bounce, op: 1 - clamp((t - 3.05) / .4, 0, 1) }); });
    if (t > 1.6) faceM(out, { op: clamp((t - 1.6) / .3, 0, 1) * (1 - clamp((t - 3.05) / .4, 0, 1)), gaze: [0, -1], mouth: "smile" });
  },
  done(t, out) {
    t %= 2.2; const jump = t < .55 ? Math.sin(Math.PI * t / .55) * 14 : 0;
    bodyM(out, { dy: -jump }); faceM(out, { dy: -jump, eyes: "happy", mouth: "grin" });
    const r = rng(5), cols = [GOLD, "#FF7A59", "#8FA8FF", "#5BD46A", CHEEK];
    for (let k = 0; k < 16; k++) { const a = r() * Math.PI * 2, v = 30 + r() * 40, tt = clamp(t - .2, 0, 2), x = 60 + Math.cos(a) * v * tt, y = 50 + Math.sin(a) * v * tt - 40 * tt + 45 * tt * tt;
      if (t > .2) rectM(out, x, y, 4, 4, 1, cols[k % 5], { op: 1 - clamp((t - 1.4) / .6, 0, 1), rot: tt * 400 }); }
  },
  oops(t, out) {
    t %= 3.6; const r = rng(9);
    CELLS.forEach(([i, j], n) => { const jitter = (r() - .5) * 30, floor = 108 - (j % 2) * 9 - (i % 3) * 3, y0 = cellY(j);
      let dy = 0, dx = 0, rot = 0;
      if (t > .5 && t < 1.6) { const k = clamp((t - .5 - n * .008) / .5, 0, 1); dy = (floor - CS - y0) * k * k; dx = jitter * k; rot = jitter * 3 * k; }
      else if (t >= 1.6 && t < 2.4) { dy = floor - CS - y0; dx = jitter; rot = jitter * 3; }
      else if (t >= 2.4) { const k = 1 - back(clamp((t - 2.4 - n * .01) / .6, 0, 1)); dy = (floor - CS - y0) * k; dx = jitter * k; rot = jitter * 3 * k; }
      cellM(out, i, j, { dx, dy, rot }); });
    if (t < .5) faceM(out, { eyes: "wide", mouth: "o" }); else if (t > 3) faceM(out, { mouth: "smile", op: clamp((t - 3) / .2, 0, 1) });
  },
  sleep(t, out) {
    const br = Math.sin(t * 2) * .8; bodyM(out, { dy: br }); faceM(out, { eyes: "closed", mouth: "sleep", dy: br });
    for (let k = 0; k < 2; k++) { const kk = ((t / 2.4) + k * .5) % 1; glyphM(out, ["####", "..#.", ".#..", "####"], 94 + kk * 12, 26 - kk * 26, 1.6 + kk * 1.4, "#8C93A3", 1 - kk); }
  },
  wave(t, out, body) {
    bodyM(out); faceM(out, { eyes: "happy", mouth: "smile" });
    const a = Math.sin(t * 7) * 22 - 10, sx = M_LEFT + 6 * P - 1, sy = M_TOP + 3.2 * P;
    [[0, 0], [1, 0], [1, -1], [1, -2], [1, -3]].forEach(([cx, cy]) => rectM(out, sx + cx * 4.6, sy + cy * 4.6, 4.6, 4.6, 1.2, body, { rot: a, ox: sx, oy: sy + 2.3 }));
  },
  listen(t, out, body, deep) {
    bodyM(out); faceM(out, { eyes: "wide", mouth: "o", gaze: [1, 0] });
    for (let k = 0; k < 4; k++) { const h = 4 + Math.abs(Math.sin(t * (5 + k * 1.7) + k)) * 18; rectM(out, 104 + k * 5.2, 60 - h / 2, 3.6, h, 1.4, deep, { op: .85 }); }
  },
  born(t, out) {
    t %= 3.8; const last = [[4, 0], [5, 1]];
    CELLS.forEach(([i, j], n) => { const isLast = last.findIndex(l => l[0] === i && l[1] === j), r = P_NEW.rank[n], [ddx, ddy, rr] = P_NEW.dirs[n];
      const start = isLast < 0 ? r * .035 : 1.35 + isLast * .3, tin = clamp((t - start) / (isLast < 0 ? .45 : .55), 0, 1), a = back(tin);
      if (tin <= 0) return; const fx = isLast < 0 ? ddx : 26, fy = isLast < 0 ? ddy : -34;
      cellM(out, i, j, { dx: fx * (1 - a), dy: fy * (1 - a), rot: (isLast < 0 ? rr : 200) * (1 - a), s: .55 + .45 * a, op: clamp(tin * 1.6, 0, 1) }); });
    if (t > 1.0) faceM(out, { op: clamp((t - 1.0) / .3, 0, 1), gaze: t < 2.2 ? [1, -1] : [0, 0], mouth: t > 2.2 ? "grin" : "smile", eyes: t > 2.3 && t < 3.2 ? "happy" : "open" });
    if (t > 2.15 && t < 2.9) { const k = (t - 2.15) / .75; for (let q = 0; q < 4; q++) { const a = q * Math.PI / 2 + .6, d = 8 + k * 12; rectM(out, 104 + Math.cos(a) * d - 2, 22 + Math.sin(a) * d - 2, 4, 4, 1, GOLD, { op: 1 - k }); } }
  },
  sent(t, out) {
    t %= 2.4; bodyM(out); const k = clamp((t - .3) / 1.4, 0, 1);
    faceM(out, { gaze: k > 0 ? [1, -1] : [1, 0], mouth: k > 0 ? "grin" : "smile" });
    const x = 98 + k * 40, y = 50 - k * 70, op = 1 - clamp((k - .7) / .3, 0, 1), plane = ["#...", "###.", "####", "###.", "#..."];
    glyphM(out, plane, x, y, 2.4, "#FFFFFF", op); glyphM(out, plane, x + .4, y + .4, 2.4, "#8C93A3", op * .35);
  },
};
/** A motion's frame `t` seconds in, for a body colour and its deep shade (null for an unknown id, « today »). */
export function motionFrame(id, t, body, deep) {
  const f = MOTIONS[id];
  if (!f) return null;
  const out = [];
  f(Math.max(0, t), out, body, deep);
  return out;
}
/** A thinking episode's animation: the `episode`-th of the rotation (small Bips: the ones readable small). */
export const thinkingVariant = (episode, small) => { const l = small ? THINKING_SMALL : THINKING; return l[((episode % l.length) + l.length) % l.length]; };
/** How long a variant plays before the next one takes over within the same episode (Alex, 2026-10-29): whole loops of
 *  its own, about 7–9 s, so it changes where its loop starts again. */
export const THINKING_TURN = { today: 7, chantier: 2.2 * 4, vague: 1.3 * 6, construit: 4.2 * 2, bulle: 4 / 2.6 * 5, puzzle: 0.9 * 8,
  jongle: 1.1 * 7, scanner: 1.8 * 4, relief: 1.6 * 5, empile: 3.6 * 2 };
/** What an episode plays `elapsed` seconds in: its variant, then the next ones in turn, and the time within it. */
export function thinkingNow(episode, elapsed, small) {
  const l = small ? THINKING_SMALL : THINKING, round = l.reduce((a, v) => a + THINKING_TURN[v], 0);
  let t = Math.max(0, elapsed) % round;
  for (let k = episode; ; k++) {
    const v = thinkingVariant(k, small);
    if (t < THINKING_TURN[v]) return [v, t];
    t -= THINKING_TURN[v];
  }
}

/** A motion frame as SVG markup, with the look's colours and costume (only on a whole Bip). */
function drawMotion(look, items, fr, ppu, pose) {
  const CO = costumeOf(look), B = bodyOf(CO, fr ? fr.b : 0), C = paletteOf(look, CO), G = geom(B), mk = mouthScale(ppu);
  const cells = items.filter(it => it.k === "cell");
  const dressed = CO && cells.length >= CELLS.length && Math.min(...cells.map(c => c.op)) >= 0.6;
  const layer = name => dressed ? CO.items.map((it, i) => (it.layer || "top") === name ? drawItem(it, i, G, pose.d, CO, fr, mk) : "").join("") : "";
  const op = v => v < 1 ? ` opacity="${fmt(v)}"` : "";
  let s = items.filter(it => it.k === "poly").map(p => `<polygon points="${p.pts.map(q => fmt(q[0]) + "," + fmt(q[1])).join(" ")}" fill="${p.fill}"/>`).join("");
  s += layer("back");
  for (const c of cells) s += block(G.cx(c.i) + c.dx, G.cy(c.j) + c.dy, P * c.s, c.fill || C.body, C.deep, C.outline, op(c.op), c.rot);
  s += layer("body");
  for (const f of items) if (f.k === "face") {
    const extra = op(f.op), ey = G.eyeY + f.dy;
    [0, 1].forEach(side => { const flat = f.eyes === "closed" || f.eyes === "happy";
      s += eyeShape(f.eyes, G.cx(B.eyeCols[side]) + f.dx, ey, f.gaze[0] * 0.8, flat ? 0 : f.gaze[1] * 0.8, C.eye, extra); });
    const M = MOUTHS[f.mouth === "sleep" ? "sleepm" : f.mouth] || MOUTHS.smile;
    s += drawGlyph(M.g, CX + f.dx + (M.dx || 0) * U * mk, G.mouthY + f.dy + (M.dy || 0) * U * mk, U * mk, C.mouth, extra);
  }
  s += layer("face") + layer("top");
  for (const it of items) {
    if (it.k === "rect") {
      const ox = it.ox ?? it.x + it.w / 2, oy = it.oy ?? it.y + it.h / 2;
      s += `<rect x="${fmt(it.x)}" y="${fmt(it.y)}" width="${fmt(it.w)}" height="${fmt(it.h)}" rx="${fmt(it.rx)}" fill="${it.fill}"${op(it.op)}`
        + (it.stroke ? ` stroke="${it.stroke}" stroke-width="1"` : "") + (it.rot ? ` transform="rotate(${fmt(it.rot)} ${fmt(ox)} ${fmt(oy)})"` : "") + "/>";
    } else if (it.k === "glyph") s += drawGlyph(it.rows, it.cx, it.cy, it.u, it.fill, op(it.op));
  }
  return s;
}
const memo = {};
/** The next thinking episode of `key` (a category): counted and kept in this browser when it can, so the animations
 *  vary from one visit to the next. */
export function nextEpisode(key) {
  let n = (memo[key] || 0) + 1;
  try { n = (+localStorage.getItem("bips.thinking." + key) || 0) + 1; localStorage.setItem("bips.thinking." + key, String(n)); } catch { /* none */ }
  memo[key] = n;
  return n;
}
// The costumes that reshape the body (the pumpkin, the vampire, the ghost) keep the states' own animations.
const reshapes = costume => { const ob = costume && COSTUMES[costume] && COSTUMES[costume].body; return !!(ob && (ob.rows || ob.frames)); };

// The frame grows upward and sideways to hold a costume (at most 152), the ground stays at 120.
const MAXBOX = 152, boxCache = new Map();
function frameOf(costume) {
  const CO = costume && COSTUMES[costume], k = costume || "";
  if (boxCache.has(k)) return boxCache.get(k);
  let size = 120;
  if (CO) {
    const G = geom(bodyOf(CO, 0));
    let minX = G.left, maxX = G.right, minY = G.top;
    CO.items.forEach(it => {
      const rows = it.rows || it.frames[0], pitch = U * (it.scale || 1), [ax, ay] = anchorPt(it.at, G, "smile");
      minX = Math.min(minX, ax + it.dx * U); maxX = Math.max(maxX, ax + it.dx * U + rows[0].length * pitch); minY = Math.min(minY, ay + it.dy * U);
    });
    const half = Math.max(60, 60 - minX + 3, maxX + 3 - 60);
    size = Math.min(MAXBOX, Math.max(120, 120 - (minY - 4), 2 * half));
  }
  boxCache.set(k, size);
  return size;
}
const viewBox = size => `${fmt(60 - size / 2)} ${fmt(120 - size)} ${fmt(size)} ${fmt(size)}`;

/** A pose: the state (or reaction) at its time, the costume's own motion, its frames. */
function poseOf(st, t, costume, clock, still) {
  const tt = still ? st.still : t;
  const pose = evalState(st, tt);
  const CO = costume && COSTUMES[costume];
  if (CO && CO.wave) for (const [k, [a, p]] of Object.entries(CO.wave)) pose.m[k] += a * Math.sin(2 * Math.PI * clock / p);
  return pose;
}
const figTransform = m => `translate(${fmt(m.x)} ${fmt(m.y)}) translate(60 ${BOTTOM}) rotate(${fmt(m.rot)}) scale(${fmt(m.sx)} ${fmt(m.sy)}) translate(-60 ${-BOTTOM})`;
const SHADOW = `<ellipse cx="60" cy="${BOTTOM + 3}" rx="30" ry="4" fill="${INK}" fill-opacity="0.1"/>`;

/** A whole Bip as SVG markup: { color | category, mood or state, t (seconds), costume, size (px, for the mouth) }. */
/** `motion`: one of the animations of 2026-10-29 (a thinking variant from `thinkingVariant`, a moment of MOMENTS),
 *  `mt` seconds in; asleep and listening Bips take the page's « Dodo » and « Écoute » by themselves. `reaction`: a tap's
 *  reaction (REACTIONS: « boing »… « vertige »), `t` seconds after the tap. */
export function pixelSVG({ category = "daily", color, mood = "idle", t = 0, costume = null, size = 48, still = REDUCED, shadow = true,
  motion = null, mt = t, reaction = null } = {}) {
  const c = CATEGORIES[category] || CATEGORIES.daily, st = (reaction && REACTIONS[reaction]) || stateOf(mood);
  const clock = still ? 0.6 : t, loop = st === stateOf(mood) ? ((t % st.dur) + st.dur) % st.dur : Math.min(Math.max(0, t), st.dur);
  const CO = costume && COSTUMES[costume];
  const box = frameOf(costume), look = { color: color || c.color, costume };
  const fr = frameState(CO, clock);
  const m = reshapes(costume) || reaction ? null : motion && MOTIONS[motion] ? motion : st === STATES.sleep ? "sleep" : st === STATES.listen ? "listen" : null;
  if (m) {
    const C = paletteOf(look, CO), pose = poseOf(STATES.idle, 0, costume, clock, true);
    pose.m = { x: 0, y: 0, rot: 0, sx: 1, sy: 1 };
    const inner = drawMotion(look, motionFrame(m, still ? (MOTION_STILL[m] ?? 1.2) : mt, C.body, C.deep), fr, size / box, pose);
    return `<svg viewBox="${viewBox(box)}" aria-hidden="true" focusable="false">${shadow ? SHADOW : ""}<g>${inner}</g></svg>`;
  }
  const pose = poseOf(st, loop, costume, clock, still);
  let inner = drawPixel(look, pose, fr, size / box);
  if (CO) inner = inner.replace(/<g data-it="(\d+)" data-pv="([\d.\- ]+)"/g, (all, i, pv) => {
    const it = CO.items[+i];
    if (!it.wave && !it.lift && !it.blink) return all;
    const q = itemDyn(it, clock, pose.m);
    return `${all} transform="translate(${fmt(q.x)} ${fmt(q.y)}) rotate(${fmt(q.rot)} ${pv})"${q.op ? "" : ` opacity="0"`}`;
  });
  return `<svg viewBox="${viewBox(box)}" aria-hidden="true" focusable="false">${shadow ? SHADOW : ""}`
    + `<g transform="${figTransform(pose.m)}">${inner}</g></svg>`;
}

/** A still Bip (avatars, small marks): the state's still moment. */
export function staticBip(category, mood = "happy", costume = null, size = 48) {
  return pixelSVG({ category, mood, costume, size, still: true });
}

/** The old call (site.js' working line): a category's Bip in a mood at time t. */
export function renderBip(c, s, t) {
  return pixelSVG({ category: c.id, mood: s.mood, t, still: REDUCED });
}

// -- The app icon C1 « Corail Solaire + Ondes Live » (Alex, 27/11) --------------------------------------------------

// The apps' icon (App/Assets.xcassets/AppIcon.appiconset/icon-1024.png, the C1 pack's master, Blender render kept in
// internal/icons/c1/) at 384 px as JPEG, so the brand mark, the link previews and kerfystudio.com need no other file:
//     sips -Z 384 -s format jpeg -s formatOptions 78 App/Assets.xcassets/AppIcon.appiconset/icon-1024.png --out c1.jpg
//     base64 -i c1.jpg
const APP_ICON = "data:image/jpeg;base64,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";

let iconId = 0;
/** The app's icon as SVG: the C1 picture (the coral Bip with the Live bars on a graphite ground) in a 120-unit square.
 *  `round` = the corner radius in icon units (0: a full square, as the stores mask it). */
export function iconSVG(round = 27) {
  const id = "ic" + (iconId++) + "c";
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120" aria-hidden="true" focusable="false">`
    + `<defs><clipPath id="${id}"><rect width="120" height="120" rx="${round}"/></clipPath></defs>`
    + `<image href="${APP_ICON}" width="120" height="120" clip-path="url(#${id})"/></svg>`;
}

// -- Seasons: the costume of the day (PixelCostumes.season in the apps) ------------------------------------------------

function easter(y) {
  const a = y % 19, b = Math.floor(y / 100), c = y % 100, d = Math.floor(b / 4), e = b % 4, f = Math.floor((b + 8) / 25);
  const g = Math.floor((b - f + 1) / 3), h = (19 * a + b - d - g + 15) % 30, i = Math.floor(c / 4), k = c % 4;
  const l = (32 + 2 * e + 2 * i - h - k) % 7, m = Math.floor((a + 11 * h + 22 * l) / 451);
  return new Date(y, Math.floor((h + l - 7 * m + 114) / 31) - 1, (h + l - 7 * m + 114) % 31 + 1);
}
/** Today's event (Halloween, Christmas…), or null. `?costumes=<event id>` or `?costumes=none` overrides it. */
export function seasonOn(date = new Date()) {
  const forced = W && new URLSearchParams(W.location.search).get("costumes");
  if (forced) return SEASONS.find(s => s.id === forced) || null;
  const md = String(date.getMonth() + 1).padStart(2, "0") + "-" + String(date.getDate()).padStart(2, "0");
  for (const s of SEASONS) {
    if (s.birthday) continue;
    if (s.easter) {
      const e = easter(date.getFullYear()), day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
      const days = Math.round((day - e) / 86400000);
      if (days >= s.easter[0] && days <= s.easter[1]) return s;
    } else if (s.from <= s.to ? md >= s.from && md <= s.to : md >= s.from || md <= s.to) return s;
  }
  return null;
}
let dressed = 0;
/** The next costume of today's event, so Bips side by side wear different ones; null outside events. */
export function nextCostume() {
  const s = seasonOn();
  return s && s.costumes.length ? s.costumes[dressed++ % s.costumes.length] : null;
}

// -- Animalese ----------------------------------------------------------------------------------------------------

let audio;
export const sound = { on: true };
const VOWELS = { a: [800, 1250], e: [480, 1850], i: [300, 2300], o: [520, 900], u: [330, 850], y: [300, 2100] };
const PLOSIVES = new Set("bdgkpqt"), FRICATIVES = new Set("cfhjsvxz");

export function audioContext() {
  audio ??= new (W.AudioContext || W.webkitAudioContext)();
  if (audio.state === "suspended") audio.resume();
  return audio;
}

function voiceFor(c, mood) {
  const v = { base: c.pitch, syl: 0.055, spread: 2.5, drift: 0, bounce: 0, vol: 0.28 };
  if (mood === "rire") { v.base *= 1.25; v.syl = 0.045; v.bounce = 3; }
  if (mood === "surpris" || mood === "boing") { v.base *= 1.15; v.drift = 5; }
  if (mood === "happy") { v.base *= 0.92; v.syl = 0.075; v.spread = 1.2; v.vol = 0.22; }
  if (mood === "vertige") { v.syl = 0.07; v.drift = -7; v.spread = 3.5; }   // BipBabble's « dizzy »
  return v;
}

function syllable(f0, [f1, f2], dur, rate, onset, vol) {
  const n = Math.floor(rate * dur), out = new Float32Array(n), harmonics = [];
  for (let k = 1; k * f0 < Math.min(5000, rate / 2.2); k++) {
    const f = k * f0, shape = Math.exp(-(((f - f1) / 260) ** 2)) + 0.7 * Math.exp(-(((f - f2) / 320) ** 2)) + 0.06;
    harmonics.push([k, shape / k ** 0.6]);
  }
  const norm = harmonics.reduce((s, h) => s + h[1], 0), attack = rate * 0.004, release = rate * 0.018;
  const onsetLen = onset ? Math.floor(rate * (onset === "p" ? 0.006 : 0.014)) : 0;
  let phase = 0;
  for (let i = 0; i < n; i++) {
    phase += 2 * Math.PI * f0 * (1 + 0.04 * i / n) / rate;
    let value = 0;
    for (const [k, a] of harmonics) value += a * Math.sin(k * phase);
    let sample = (value / norm) * Math.min(1, i / attack) * Math.min(1, (n - i) / release);
    if (i < onsetLen) {
      const burst = (Math.random() * 2 - 1) * (onset === "p" ? 0.5 : 0.22);
      sample = sample * i / onsetLen + burst * (1 - i / onsetLen);
    }
    out[i] = sample * vol;
  }
  return out;
}

export function babble(text, c, mood) {
  if (!sound.on || !W) return;
  const ctx = audioContext(), rate = ctx.sampleRate, v = voiceFor(c, mood);
  const letters = [...text.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "")];
  const sayable = letters.filter(ch => /\p{L}/u.test(ch));
  if (!sayable.length) return;
  const ending = [...letters].reverse().find(ch => !/\s|♡/.test(ch));
  const drift = v.drift + (ending === "?" ? 4 : ending === "!" ? 2 : ending === "…" ? -3 : 0);
  const out = [];
  let spoken = 0, last = VOWELS.a;
  for (const ch of letters) {
    if (/\s|,/.test(ch)) { out.push(new Float32Array(Math.floor(rate * v.syl * 0.8))); continue; }
    if (!/\p{L}/u.test(ch)) continue;
    const progress = sayable.length > 1 ? spoken / (sayable.length - 1) : 0;
    const semis = ((ch.codePointAt(0) % 5) - 2) / 2 * v.spread + drift * progress + (spoken % 2 === 0 ? v.bounce : -v.bounce / 2);
    if (VOWELS[ch]) last = VOWELS[ch];
    out.push(syllable(v.base * Math.pow(2, semis / 12), VOWELS[ch] ?? last, v.syl, rate, PLOSIVES.has(ch) ? "p" : FRICATIVES.has(ch) ? "f" : "", v.vol));
    spoken++;
  }
  const total = out.reduce((n, a) => n + a.length, 0), buffer = ctx.createBuffer(1, total, rate), data = buffer.getChannelData(0);
  let at = 0;
  for (const part of out) { data.set(part, at); at += part.length; }
  const src = ctx.createBufferSource();
  src.buffer = buffer;
  src.connect(ctx.destination);
  src.start();
}

// -- A Bip on the page ----------------------------------------------------------------------------------------------

const pick = a => a[Math.floor(Math.random() * a.length)];
const living = new Set();
let pointer = null, pointerAt = 0;   // the page's pointer: idle Bips glance at it
if (W) {
  W.addEventListener("pointermove", e => { pointer = { x: e.clientX, y: e.clientY }; pointerAt = performance.now(); }, { passive: true });
  W.addEventListener("pointerleave", () => { pointer = null; });
}
// What a reaction says (the site's lines) and how it sounds.
const SAYS = { boing: "boing", clin: "wink", rire: "giggle", regard: "hop", surpris: "surprised", vertige: "dizzy" };
// Taps: three quick ones (1.2 s) make a happy jump; eight within 6 s (the old Bips' count) make it dizzy.
const QUICK = [3, 1200], DIZZY = [8, 6000];

/**
 * `new Bip(element, "daily", { interactive, lines, mood, costume })`: draws a pixel Bip in `element` (any size: the
 * SVG fills it), breathing and blinking in its state; when interactive, a tap plays one of the apps' reactions (three
 * quick taps: a happy jump with a heart; eight within six seconds: dizzy, « Tout tourne… »). `costume`: a costume id, or true for today's event (Halloween…).
 * `bip.mood` changes its state from outside.
 */
export class Bip {
  constructor(element, category, options = {}) {
    this.c = CATEGORIES[category] || CATEGORIES.daily;
    this.el = element;
    this.lines = options.lines ?? {};
    this.base = options.mood ?? "idle";
    this.costume = options.costume === true ? nextCostume() : (options.costume || null);
    this.t = this.c.index * 1.37;
    this.clock = this.t;
    this.reaction = null;
    this.visible = false;
    this.key = "";
    this.el.classList.add("bip");
    this.box = document.createElement("div");
    this.box.className = "bip-svg";
    this.el.append(this.box);
    const size = frameOf(this.costume);
    this.box.innerHTML = `<svg viewBox="${viewBox(size)}" aria-hidden="true" focusable="false">${SHADOW}<g></g></svg>`;
    this.fig = this.box.querySelector("g");
    this.items = [];
    if (options.interactive) this.listen();
    // off screen a Bip stands still: no frame drawn for it, its element's own CSS motion (a bob…) paused
    if ("IntersectionObserver" in W) new IntersectionObserver(([entry]) => {
      this.visible = entry.isIntersecting;
      this.el.style.animationPlayState = this.visible ? "" : "paused";
      if (this.visible) wake();
    }, { rootMargin: "120px" }).observe(this.el);
    else this.visible = true;
    living.add(this);
    wake();
    this.render();
  }

  set mood(value) {
    const was = this.thinking;
    this.base = value; this.t = 0;
    if (this.thinking && !was) { this.episode = nextEpisode(this.c.id); this.mt = 0; }   // the next thinking animation
  }
  get thinking() { const s = stateOf(this.base); return s === STATES.think || s === STATES.work; }
  /** Plays a moment once (« done », « oops », « wave », « born », « sent »), then back to its state. */
  play(moment) { if (MOMENTS[moment]) { this.moment = moment; this.momentT = 0; if (REDUCED) setTimeout(() => { this.moment = null; this.render(); }, 1400); this.render(); } }
  /** What it plays now among the animations of 2026-10-29, if any. */
  motionNow() {
    if (reshapes(this.costume) || this.reaction) return null;
    if (this.moment) return [this.moment, this.momentT];
    const st = stateOf(this.base);
    if (this.thinking) {
      if (this.episode == null) { this.episode = nextEpisode(this.c.id); this.mt = 0; }
      const [v, t] = thinkingNow(this.episode, this.mt, (this.el.clientWidth || 48) <= 56);
      return MOTIONS[v] ? [v, t] : null;
    }
    if (st === STATES.sleep) return ["sleep", this.t];
    if (st === STATES.listen) return ["listen", this.t];
    return null;
  }
  /** Stops drawing it and empties its element. */
  remove() { living.delete(this); this.el.innerHTML = ""; }
  get mood() { return this.base; }

  react(name) {
    this.reaction = REACTIONS[name] ? name : pick(REACTION_ORDER);
    this.rt = 0;
    if (REDUCED) { clearTimeout(this.still); this.still = setTimeout(() => { this.reaction = null; this.render(); }, 1400); this.render(); }
  }

  say(text) {
    if (!text) return;
    if (!this.bubble) {
      this.bubble = document.createElement("div");
      this.bubble.className = "bip-bubble";
      this.el.append(this.bubble);
    }
    this.bubble.textContent = text;
    this.bubble.classList.add("show");
    clearTimeout(this.bubbleTimer);
    this.bubbleTimer = setTimeout(() => this.bubble.classList.remove("show"), 1600);
    babble(text, this.c, this.reaction);
  }
  line(kind) { return pick(this.lines[kind] ?? [""]); }

  tapped() {
    const now = performance.now();
    if (this.reaction === "vertige") return;   // still dizzy: it waits until the world stops turning
    this.taps = (this.taps || []).filter(t => now - t < QUICK[1]).concat(now);
    this.pokes = (this.pokes || []).filter(t => now - t < DIZZY[1]).concat(now);
    if (this.pokes.length >= DIZZY[0]) {
      this.taps = []; this.pokes = [];
      this.react("vertige");
      this.say(this.line("dizzy"));
      return;
    }
    if (this.taps.length >= QUICK[0]) {
      this.taps = [];
      this.reaction = "happyJump"; this.rt = 0;
      this.say(this.line("thanks"));
      if (REDUCED) { clearTimeout(this.still); this.still = setTimeout(() => { this.reaction = null; this.render(); }, 1400); this.render(); }
      return;
    }
    this.react();
    this.say(this.line(SAYS[this.reaction]));
  }

  listen() {
    this.el.tabIndex = 0;
    this.el.setAttribute("role", "button");
    this.el.addEventListener("pointerdown", () => { try { audioContext(); } catch { /* no sound */ } });
    this.el.addEventListener("click", () => this.tapped());
    this.el.addEventListener("keydown", e => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); this.tapped(); } });
  }

  tick(dt) {
    this.t += dt; this.clock += dt; this.mt = (this.mt || 0) + dt;
    if (this.moment) { this.momentT += dt; if (this.momentT >= MOMENTS[this.moment]) this.moment = null; }
    if (this.reaction) {
      this.rt += dt;
      const st = this.reaction === "happyJump" ? STATES.happy : REACTIONS[this.reaction];
      if (this.rt >= st.dur) this.reaction = null;
    }
  }

  render() {
    const still = REDUCED;
    const motion = this.motionNow();
    if (motion) {
      const [id, mt] = motion, CO = this.costume && COSTUMES[this.costume], clock = still ? 0.6 : this.clock;
      const look = { color: this.c.color, costume: this.costume }, C = paletteOf(look, CO), fr = frameState(CO, clock);
      const pose = poseOf(STATES.idle, 0, this.costume, clock, true);
      this.fig.innerHTML = drawMotion(look, motionFrame(id, still ? (MOTION_STILL[id] ?? 1.2) : mt, C.body, C.deep), fr,
        (this.el.clientWidth || 48) / frameOf(this.costume), pose);
      this.fig.removeAttribute("transform");
      this.items = []; this.key = still ? "motion:" + id : "";
      return;
    }
    let st, t;
    if (this.reaction) { st = this.reaction === "happyJump" ? STATES.happy : REACTIONS[this.reaction]; t = still ? st.still : this.rt; }
    else { st = stateOf(this.base); t = still ? st.still : ((this.t % st.dur) + st.dur) % st.dur; }
    const clock = still ? 0.6 : this.clock;
    const pose = poseOf(st, t, this.costume, clock, false);
    // At rest, an eye on the pointer when it moves nearby (one face pixel at most, like the states' own glances).
    if (!this.reaction && st === STATES.idle && pointer && performance.now() - pointerAt < 2500) {
      const r = this.el.getBoundingClientRect();
      const dx = pointer.x - (r.left + r.width / 2), dy = pointer.y - (r.top + r.height / 2);
      if (Math.hypot(dx, dy) < 600) pose.d.gaze = [Math.abs(dx) > r.width * 0.6 ? Math.sign(dx) : 0, Math.abs(dy) > r.height * 0.6 ? Math.sign(dy) : 0];
    }
    const CO = this.costume && COSTUMES[this.costume], fr = frameState(CO, clock);
    const size = this.el.clientWidth || 48, box = frameOf(this.costume);
    const look = { color: this.c.color, costume: this.costume };
    const key = JSON.stringify([pose.d, fr, Math.round(size)]);
    if (key !== this.key) {
      this.key = key;
      this.fig.innerHTML = drawPixel(look, pose, fr, size / box);
      this.items = [...this.fig.querySelectorAll("[data-it]")];
    }
    if (CO) for (const n of this.items) {
      const it = CO.items[+n.dataset.it];
      if (!it.wave && !it.lift && !it.blink) continue;
      const q = itemDyn(it, clock, pose.m);
      n.setAttribute("transform", `translate(${fmt(q.x)} ${fmt(q.y)}) rotate(${fmt(q.rot)} ${n.dataset.pv})`);
      n.style.opacity = q.op ? "" : "0";
    }
    this.fig.setAttribute("transform", figTransform(pose.m));
  }
}

// At most 30 frames a second, and none at all while no Bip is on screen (the loop stops; a Bip coming into view
// starts it again; the browser stops frames in a hidden tab).
let raf = 0, last = 0;
function frame(now) {
  raf = 0;
  if (![...living].some(bip => bip.visible)) { last = 0; return; }
  raf = requestAnimationFrame(frame);
  if (last && now - last < 32) return;
  const dt = last ? Math.min(0.1, (now - last) / 1000) : 0;
  last = now;
  for (const bip of living) {
    if (!bip.visible) continue;
    if (!REDUCED) bip.tick(dt); else if (bip.reaction || bip.moment) bip.tick(dt);
    if (!REDUCED || bip.reaction || !bip.key) bip.render();
  }
}
function wake() { if (W && !raf) raf = requestAnimationFrame(frame); }
