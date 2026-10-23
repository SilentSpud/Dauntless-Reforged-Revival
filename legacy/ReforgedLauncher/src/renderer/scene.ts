// The built-in background, shown when the host has no art pack. Drawn by Dauntless Revived's scene
// generator (an original night scene: jagged snowy mountains in two ranges, a dark pine forest, mist
// between the ranges, a faint aurora, stars and a little snow), with a different seed and one layer
// of our own: embers from a forge rising from the valley, for "Reforged". The emblem hangs faded in
// the sky (index.html).
//
// It is drawn once, from a fixed seed, into the layer shells in index.html. Every colour comes from
// styles.css (the palette tokens, set through the class names used here). Animation is CSS only and
// moves whole layers with transform and opacity, so the compositor slides finished layers and nothing
// is repainted; styles.css stops it for prefers-reduced-motion, while the game runs, and under a
// host's art.

import { $, s } from "./dom";

const W = 1600;
const H = 900;

type Pt = [number, number];
// One mountain of a range: its summit, the cols on either side, and the ridge down each side from the
// summit (west, the lit side, ends at the left col; east, the shadow side, at the right one).
type Peak = { top: Pt; left: Pt; right: Pt; west: Pt[]; east: Pt[] };

// Park-Miller: the same scene on every start.
function random(seed: number): () => number {
  let x = seed;
  return () => {
    x = (x * 16807) % 2147483647;
    return (x - 1) / 2147483646;
  };
}

const f = (n: number): string => n.toFixed(1);
const path = (pts: Pt[]): string => pts.map(([x, y], i) => `${i ? "L" : "M"}${f(x)} ${f(y)}`).join("");
const shape = (pts: Pt[]): string => `${path(pts)}Z`;
const lerp = (a: Pt, b: Pt, t: number): Pt => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
const group = (id: string): SVGGElement => $(id) as unknown as SVGGElement;

// A jagged ridge line across the whole width: summits of varied height, some with a lower shoulder,
// and a chipped step on each slope, like the cut-paper mountains of the logo.
function ridge(rnd: () => number, base: number, height: number, minW: number, maxW: number): { line: Pt[]; peaks: Peak[] } {
  const line: Pt[] = [];
  const peaks: Peak[] = [];
  const chip = (a: Pt, b: Pt, span: number): Pt => {
    const t = 0.3 + rnd() * 0.35;
    return [a[0] + (b[0] - a[0]) * t + (rnd() - 0.5) * span * 0.16, a[1] + (b[1] - a[1]) * t + span * (0.03 + rnd() * 0.07)];
  };
  let x = -140;
  let prev: Pt = [x, base - height * (0.1 + rnd() * 0.2)];
  line.push(prev);
  while (x < W + 140) {
    const half = minW / 2 + (rnd() * (maxW - minW)) / 2;
    const top: Pt = [x + half * (0.8 + rnd() * 0.4), base - height * (0.35 + rnd() * 0.65)];
    const col: Pt = [x + half * 2, base - height * (0.05 + rnd() * 0.25)];
    const east: Pt[] = [];
    if (rnd() < 0.4) {
      // A shoulder: a dip and a lower second summit on the way down.
      const dip = lerp(top, col, 0.35);
      dip[1] += (col[1] - top[1]) * 0.12;
      const second = lerp(top, col, 0.55);
      second[1] -= (col[1] - top[1]) * 0.18;
      east.push(dip, second, chip(second, col, half));
    } else {
      east.push(chip(top, col, half));
    }
    const west = chip(prev, top, half);
    line.push(west, top, ...east, col);
    peaks.push({ top, left: prev, right: col, west: [top, west, prev], east: [top, ...east, col] });
    prev = col;
    x += half * 2;
  }
  return { line, peaks };
}

// The first part of a polyline, a fraction of its length long.
function along(line: Pt[], fraction: number): Pt[] {
  const lengths = line.slice(1).map((q, i) => Math.hypot(q[0] - line[i][0], q[1] - line[i][1]));
  let left = lengths.reduce((sum, l) => sum + l, 0) * fraction;
  const out: Pt[] = [line[0]];
  for (let i = 0; i < lengths.length; i++) {
    if (left <= lengths[i]) {
      out.push(lerp(line[i], line[i + 1], left / lengths[i]));
      return out;
    }
    left -= lengths[i];
    out.push(line[i + 1]);
  }
  return out;
}

// The snow on a mountain: down both ridges from the summit, further on the lit (west) side, with a torn
// lower edge across the face.
function snowcap(rnd: () => number, p: Peak, reach: number): Pt[] {
  const depth = Math.min(p.left[1], p.right[1]) - p.top[1];
  const west = along(p.west, reach * (0.8 + rnd() * 0.35));
  const east = along(p.east, reach * (0.35 + rnd() * 0.25));
  const a = west[west.length - 1];
  const d = east[east.length - 1];
  const teeth: Pt[] = [];
  const n = 4 + Math.floor(rnd() * 3);
  for (let i = 1; i <= n; i++) {
    const edge = lerp(a, d, i / (n + 1));
    const dip = depth * reach * (i % 2 ? 0.12 + rnd() * 0.26 : -0.02 - rnd() * 0.08);
    teeth.push([edge[0] + (rnd() - 0.5) * 8, edge[1] + dip]);
  }
  return [...west, ...teeth, ...east.reverse().slice(0, -1)];
}

// The shadow side: from the summit along the east ridge to the col, then back up the face along a
// spur to under the summit, which gives the faceted, two-tone look of the logo's peaks.
function shadowFace(rnd: () => number, p: Peak): Pt[] {
  const drop = p.right[1] - p.top[1];
  const spur: Pt = [p.top[0] + (p.right[0] - p.top[0]) * (0.15 + rnd() * 0.2), p.right[1] + drop * (0.35 + rnd() * 0.4)];
  const kink: Pt = [p.top[0] + (spur[0] - p.top[0]) * 0.5 + (rnd() - 0.5) * 10, p.top[1] + (spur[1] - p.top[1]) * (0.45 + rnd() * 0.1)];
  return [...p.east, spur, kink];
}

// Couloirs: thin dark gullies running down from under the summit on the lit side.
function gullies(rnd: () => number, p: Peak): string {
  const depth = Math.min(p.left[1], p.right[1]) - p.top[1];
  let d = "";
  const n = 1 + Math.floor(rnd() * 3);
  for (let i = 0; i < n; i++) {
    const on = along(p.west, 0.06 + rnd() * 0.25);
    const ridgePoint = on[on.length - 1];
    // Start a little inside the face, never on the skyline.
    const start: Pt = [ridgePoint[0] + 4 + rnd() * 6, ridgePoint[1] + 3];
    const len = depth * (0.25 + rnd() * 0.35);
    const w = 2 + rnd() * 4;
    const lean = (rnd() - 0.2) * len * 0.35;
    d += shape([start, [start[0] + w, start[1] + w * 0.6], [start[0] + lean + w * 0.3, start[1] + len], [start[0] + lean - w * 0.2, start[1] + len * 0.92]]);
  }
  return d;
}

function range(g: SVGGElement, rnd: () => number, name: string, base: number, height: number, minW: number, maxW: number, reach: number): void {
  const r = ridge(rnd, base, height, minW, maxW);
  g.appendChild(s("path", { class: `ridge-${name}`, d: `${path(r.line)}L${W + 140} ${H}L-140 ${H}Z` }));
  g.appendChild(s("path", { class: `snow-${name}`, d: r.peaks.map((p) => shape(snowcap(rnd, p, reach))).join("") }));
  g.appendChild(s("path", { class: `gully-${name}`, d: r.peaks.map((p) => gullies(rnd, p)).join("") }));
  g.appendChild(s("path", { class: `shade-${name}`, d: r.peaks.map((p) => shape(shadowFace(rnd, p))).join("") }));
  g.appendChild(s("path", { class: `rim-${name}`, d: path(r.line) }));
}

// A pine: a spire of three or four jagged tiers on a short trunk.
function pine(x: number, base: number, h: number, w: number, tiers: number): Pt[] {
  const left: Pt[] = [];
  const right: Pt[] = [];
  const trunk = h * 0.1;
  for (let i = 0; i < tiers; i++) {
    const tipY = base - h + ((h - trunk) * i) / tiers;
    const skirtY = base - trunk - ((h - trunk) * (tiers - 1 - i)) / tiers / 2.6;
    const spread = (w / 2) * (0.45 + (0.55 * (i + 1)) / tiers);
    if (i > 0) {
      left.push([x - spread * 0.32, tipY]);
      right.push([x + spread * 0.32, tipY]);
    }
    left.push([x - spread, skirtY]);
    right.push([x + spread, skirtY]);
  }
  const top: Pt = [x, base - h];
  const stem = Math.max(1.2, w * 0.05);
  return [top, ...right, [x + stem, base - trunk], [x + stem, base], [x - stem, base], [x - stem, base - trunk], ...left.reverse()];
}

// A row of pines along a ground line, taller towards the edges so they frame the middle.
function treeRow(rnd: () => number, ground: (x: number) => number, minH: number, maxH: number, step: number, frame: number): string {
  let d = "";
  for (let x = -40; x < W + 40; x += step * (0.55 + rnd() * 0.9)) {
    const edge = Math.abs(x - W * 0.52) / (W * 0.52);
    const h = (minH + rnd() * (maxH - minH)) * (1 + frame * edge * edge);
    d += shape(pine(x, ground(x) + rnd() * 6, h, h * (0.34 + rnd() * 0.12), 3 + Math.floor(rnd() * 2)));
  }
  return d;
}

function starField(g: SVGGElement, rnd: () => number, count: number, maxY: number, minR: number, maxR: number): void {
  for (let i = 0; i < count; i++) {
    const y = rnd() * maxY;
    g.appendChild(
      s("circle", {
        class: rnd() < 0.3 ? "star star-glow" : "star",
        cx: f(rnd() * W),
        cy: f(y),
        r: (minR + rnd() * (maxR - minR)).toFixed(2),
        // Fainter towards the horizon.
        opacity: ((0.25 + rnd() * 0.65) * (1 - (0.6 * y) / maxY)).toFixed(2),
      }),
    );
  }
}

// Snow: the same flakes in the top and bottom half of a layer twice the height of the scene, which
// styles.css slides down by half its height, over and over.
function snowField(g: SVGGElement, rnd: () => number, count: number, minR: number, maxR: number): void {
  for (let i = 0; i < count; i++) {
    const x = f(rnd() * W);
    const y = rnd() * H;
    const r = (minR + rnd() * (maxR - minR)).toFixed(2);
    const o = (0.3 + rnd() * 0.5).toFixed(2);
    g.appendChild(s("circle", { cx: x, cy: f(y), r, opacity: o }));
    g.appendChild(s("circle", { cx: x, cy: f(y + H), r, opacity: o }));
  }
}

// Embers: small warm sparks over the valley, in a layer twice the height of the scene that
// styles.css slides up by half its height, like the snow falls.
function emberField(g: SVGGElement, rnd: () => number, count: number): void {
  for (let i = 0; i < count; i++) {
    const x = f(W * (0.12 + rnd() * 0.76));
    const y = H * (0.35 + rnd() * 0.65);
    const r = (0.8 + rnd() * 1.6).toFixed(2);
    const o = (0.25 + rnd() * 0.55).toFixed(2);
    g.appendChild(s("circle", { cx: x, cy: f(y), r, opacity: o }));
    g.appendChild(s("circle", { cx: x, cy: f(y + H), r, opacity: o }));
  }
}

// Curtains of light: a wavy lower edge, a higher and looser upper edge, filled with a gradient that
// is brightest just above the lower edge (the gradients and the blur are in index.html).
function aurora(g: SVGGElement, rnd: () => number): void {
  const bands = [
    { y: 240, tilt: -0.1, depth: 150, cls: "aurora-a" },
    { y: 195, tilt: -0.05, depth: 115, cls: "aurora-b" },
    { y: 300, tilt: -0.14, depth: 90, cls: "aurora-b" },
  ];
  for (const b of bands) {
    const p1 = rnd() * 6;
    const p2 = rnd() * 6;
    const lower: Pt[] = [];
    const upper: Pt[] = [];
    for (let x = -200; x <= W + 200; x += 40) {
      const y = b.y + (x - W / 2) * b.tilt + Math.sin(x / 190 + p1) * 26 + Math.sin(x / 83 + p2) * 9;
      lower.push([x, y]);
      upper.push([x, y - b.depth * (0.6 + 0.4 * Math.sin(x / 140 + p2) ** 2)]);
    }
    g.appendChild(s("path", { class: b.cls, d: `${path(upper)}${path(lower.reverse()).replace("M", "L")}Z` }));
  }
}

// Mist: soft flat clouds (a radial gradient each) along a band, reaching past both sides so the
// layer can drift without showing an edge.
function mist(g: SVGGElement, rnd: () => number, y: number, band: number, count: number): void {
  for (let i = 0; i < count; i++) {
    g.appendChild(
      s("ellipse", {
        class: "mist-puff",
        cx: f(-260 + (i + rnd() * 0.8) * ((W + 520) / count)),
        cy: f(y + (rnd() - 0.5) * band),
        rx: f(230 + rnd() * 300),
        ry: f(24 + rnd() * 34),
        opacity: (0.55 + rnd() * 0.45).toFixed(2),
      }),
    );
  }
}

export function buildScene(): void {
  const rnd = random(1147);

  starField(group("#sc-stars"), rnd, 190, 560, 0.45, 1.35);
  starField(group("#sc-twinkle"), rnd, 28, 430, 1.1, 1.9);
  aurora(group("#sc-aurora"), rnd);

  // Far range: tall and hazy with distance. Near range: darker and sharper, with the forest climbing
  // its foot. Mist lies between them and in front.
  range(group("#sc-far"), rnd, "far", 610, 340, 170, 360, 0.5);
  mist(group("#sc-mist-far"), rnd, 590, 120, 8);
  const mid = group("#sc-mid");
  range(mid, rnd, "mid", 712, 250, 120, 270, 0.42);
  mid.appendChild(s("path", { class: "trees-mid", d: treeRow(rnd, (x) => 676 + Math.sin(x / 210) * 14 + Math.sin(x / 67) * 5, 16, 36, 9, 0.4) }));
  mist(group("#sc-mist-near"), rnd, 712, 70, 6);

  // The forest in front: a back row and a taller front row, darkest of all.
  const forest = group("#sc-forest");
  forest.appendChild(s("path", { class: "trees-back", d: treeRow(rnd, (x) => 756 + Math.sin(x / 240 + 1) * 18, 38, 76, 15, 0.9) }));
  forest.appendChild(s("path", { class: "ground", d: `M-120 ${H}L-120 820Q400 780 800 806T${W + 120} 800L${W + 120} ${H}Z` }));
  forest.appendChild(s("path", { class: "trees-front", d: treeRow(rnd, (x) => 844 + Math.sin(x / 300 + 2) * 16, 60, 122, 26, 1.6) }));

  snowField(group("#sc-snow"), rnd, 70, 0.7, 2.2);
  emberField(group("#sc-embers"), rnd, 46);
}
