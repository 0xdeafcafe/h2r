// Stand-in map geometry for the mock (the "map geometry contract": a
// `map.glb`, a `views.json` and pre-drawn `views/<id>.png` per map), so the
// 3D viewer and the pictured 2D map can be run without a disc. Nothing here
// comes from the game: the arenas are built from the mock's own invented
// floor plans (mock/maps.ts) as slabs, ramps, pillars and a few cones, and
// the pictures are drawn from those triangles by the small rasteriser below.
//
// Coordinates here are game ones (z up); the glb writer converts to glTF's
// (x, z, -y) as the contract says.

import { encodePng } from "./gameart";
import { MAPS, type MockMap } from "./maps";

type V3 = [number, number, number];
type Rgb = [number, number, number]; // linear, 0..1

type MaterialName = "floor" | "wall" | "metal" | "foliage" | "default" | "shell";

const MATERIALS: Record<MaterialName, { roughness: number; metallic: number }> = {
  floor: { roughness: 0.92, metallic: 0 },
  wall: { roughness: 0.85, metallic: 0 },
  metal: { roughness: 0.45, metallic: 0.7 },
  foliage: { roughness: 0.95, metallic: 0 },
  default: { roughness: 0.9, metallic: 0 },
  // The indoor shell: written double-sided, the worst a glb could say of a
  // level that is really single-sided.
  shell: { roughness: 0.9, metallic: 0 },
};

interface Tri { a: V3; b: V3; c: V3; n: V3; colour: Rgb; material: MaterialName; /** The glb node it goes in; `structure` when not said. */ node?: string }

export interface Arena {
  name: string;
  tris: Tri[];
  bounds: { min: V3; max: V3 };
  /** Geometry that is only in the glb, not in the pre-drawn views (an
   *  indoor level's shell would hide everything from above). */
  extra?: Tri[];
  /** The glb's bounds with `extra` counted in. */
  outer?: { min: V3; max: V3 };
  /** Extras for the glb's nodes, by node name. */
  nodeExtras?: Record<string, Record<string, unknown>>;
}

// ---- small vector helpers ----

const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const norm = (a: V3): V3 => { const l = Math.hypot(a[0], a[1], a[2]) || 1; return [a[0] / l, a[1] / l, a[2] / l]; };

// ---- building ----

class Builder {
  tris: Tri[] = [];

  /** A triangle, counter-clockwise seen from its front. */
  tri(a: V3, b: V3, c: V3, colour: Rgb, material: MaterialName) {
    const n = cross(sub(b, a), sub(c, a));
    if (Math.hypot(n[0], n[1], n[2]) < 1e-9) return;
    this.tris.push({ a, b, c, n: norm(n), colour, material });
  }

  quad(a: V3, b: V3, c: V3, d: V3, colour: Rgb, material: MaterialName) {
    this.tri(a, b, c, colour, material);
    this.tri(a, c, d, colour, material);
  }

  /** A flat-topped prism: `poly` (any winding) from `top` down to `bottom`,
   *  the sides a little darker than the top. */
  slab(poly: [number, number][], top: number, bottom: number, colour: Rgb, material: MaterialName, side: Rgb = shade(colour, 0.8)) {
    const p = ccw(poly);
    for (const [i, j, k] of triangulate(p)) {
      this.tri([...p[i]!, top], [...p[j]!, top], [...p[k]!, top], colour, material);
      this.tri([...p[k]!, bottom], [...p[j]!, bottom], [...p[i]!, bottom], side, material);
    }
    for (let i = 0; i < p.length; i++) {
      const a = p[i]!, b = p[(i + 1) % p.length]!;
      this.quad([a[0], a[1], bottom], [b[0], b[1], bottom], [b[0], b[1], top], [a[0], a[1], top], side, material);
    }
  }

  box(x0: number, y0: number, z0: number, x1: number, y1: number, z1: number, colour: Rgb, material: MaterialName) {
    this.slab([[x0, y0], [x1, y0], [x1, y1], [x0, y1]], z1, z0, colour, material);
  }

  /** A ramp over a rectangle, rising from `z0` at its (x0, y0) end to `z1`
   *  at the far end along `axis`; solid underneath down to `base`. */
  ramp(x0: number, y0: number, x1: number, y1: number, axis: "x" | "y", z0: number, z1: number, base: number, colour: Rgb, material: MaterialName) {
    const h = (x: number, y: number) => {
      const t = axis === "x" ? (x - x0) / (x1 - x0) : (y - y0) / (y1 - y0);
      return z0 + (z1 - z0) * t;
    };
    const lx = Math.min(x0, x1), hx = Math.max(x0, x1), ly = Math.min(y0, y1), hy = Math.max(y0, y1);
    const c: [number, number][] = [[lx, ly], [hx, ly], [hx, hy], [lx, hy]];
    const top = c.map(([x, y]): V3 => [x, y, h(x, y)]);
    const bot = c.map(([x, y]): V3 => [x, y, base]);
    const side = shade(colour, 0.8);
    this.quad(top[0]!, top[1]!, top[2]!, top[3]!, colour, material);
    this.quad(bot[3]!, bot[2]!, bot[1]!, bot[0]!, side, material);
    for (let i = 0; i < 4; i++) {
      const j = (i + 1) % 4;
      this.quad(bot[i]!, bot[j]!, top[j]!, top[i]!, side, material);
    }
  }

  /** A cone or a tapered column: radius `r0` at `z0`, `r1` at `z1`. */
  column(cx: number, cy: number, r0: number, r1: number, z0: number, z1: number, sides: number, colour: Rgb, material: MaterialName) {
    const ring = (r: number, z: number) => Array.from({ length: sides }, (_, i): V3 => {
      const a = (i / sides) * Math.PI * 2;
      return [cx + Math.cos(a) * r, cy + Math.sin(a) * r, z];
    });
    const lo = ring(r0, z0), hi = ring(r1, z1);
    for (let i = 0; i < sides; i++) {
      const j = (i + 1) % sides;
      // Shade each face a touch differently so the facets read.
      const c = shade(colour, 0.86 + 0.14 * ((i * 7) % sides) / sides);
      this.quad(lo[i]!, lo[j]!, hi[j]!, hi[i]!, c, material);
      if (r1 > 1e-6) this.tri([cx, cy, z1], hi[i]!, hi[j]!, colour, material);
      this.tri([cx, cy, z0], lo[j]!, lo[i]!, shade(colour, 0.7), material);
    }
  }

  done(name: string): Arena {
    const min: V3 = [Infinity, Infinity, Infinity], max: V3 = [-Infinity, -Infinity, -Infinity];
    for (const t of this.tris) for (const v of [t.a, t.b, t.c]) for (let i = 0; i < 3; i++) {
      if (v[i]! < min[i]!) min[i] = v[i]!;
      if (v[i]! > max[i]!) max[i] = v[i]!;
    }
    return { name, tris: this.tris, bounds: { min, max } };
  }
}

const shade = (c: Rgb, k: number): Rgb => [c[0] * k, c[1] * k, c[2] * k];

function area2(p: [number, number][]): number {
  let s = 0;
  for (let i = 0; i < p.length; i++) { const a = p[i]!, b = p[(i + 1) % p.length]!; s += a[0] * b[1] - b[0] * a[1]; }
  return s;
}

function ccw(p: [number, number][]): [number, number][] {
  return area2(p) < 0 ? [...p].reverse() : p;
}

/** Ear clipping for a simple counter-clockwise polygon. */
export function triangulate(p: [number, number][]): [number, number, number][] {
  const idx = p.map((_, i) => i);
  const out: [number, number, number][] = [];
  const turn = (a: number, b: number, c: number) =>
    (p[b]![0] - p[a]![0]) * (p[c]![1] - p[a]![1]) - (p[b]![1] - p[a]![1]) * (p[c]![0] - p[a]![0]);
  const inside = (a: number, b: number, c: number, q: number) => turn(a, b, q) > 0 && turn(b, c, q) > 0 && turn(c, a, q) > 0;
  let guard = p.length * p.length;
  while (idx.length > 3 && guard-- > 0) {
    let cut = false;
    for (let i = 0; i < idx.length; i++) {
      const a = idx[(i + idx.length - 1) % idx.length]!, b = idx[i]!, c = idx[(i + 1) % idx.length]!;
      if (turn(a, b, c) <= 0) continue;
      if (idx.some((q) => q !== a && q !== b && q !== c && inside(a, b, c, q))) continue;
      out.push([a, b, c]);
      idx.splice(i, 1);
      cut = true;
      break;
    }
    if (!cut) idx.splice(0, 1); // a degenerate corner: drop it and carry on
  }
  if (idx.length === 3) out.push([idx[0]!, idx[1]!, idx[2]!]);
  return out;
}

// A seeded generator, so the same arena comes out every run.
function rng(seed: number) {
  let s = seed >>> 0;
  return () => { s = (Math.imul(s, 1664525) + 1013904223) >>> 0; return s / 2 ** 32; };
}

const STONE: Rgb = [0.52, 0.5, 0.46];
const STONE_WARM: Rgb = [0.6, 0.52, 0.42];
const SLATE: Rgb = [0.3, 0.34, 0.4];
const STEEL: Rgb = [0.45, 0.5, 0.56];
const MOSS: Rgb = [0.3, 0.42, 0.24];
const LEAF: Rgb = [0.2, 0.42, 0.18];
const BARK: Rgb = [0.3, 0.22, 0.15];

/** Teacup: the cross-shaped courtyard, its four corner pads, and a ring of
 *  balconies on pillars above, reached by four ramps. */
function teacup(m: MockMap): Arena {
  const b = new Builder();
  const ground = m.outline!.layers[0]!.polygons, upper = m.outline!.layers[1]!.polygons;
  const G = 0.5, U = 8;
  ground.forEach((poly, i) => b.slab(poly, G, G - (i < 5 ? 3 : 5), i === 0 ? STONE : i < 5 ? shade(STONE, 0.93) : STONE_WARM, "floor"));
  for (const poly of upper) b.slab(poly, U, U - 0.8, SLATE, "metal", shade(SLATE, 0.75));
  // A parapet round the outside of the ring, and pillars under its corners.
  const P = 30, w = 0.5, hgt = 1.1;
  b.box(-P, P, U, P + w, P + w, U + hgt, STEEL, "wall");
  b.box(-P - w, -P - w, U, P, -P, U + hgt, STEEL, "wall");
  b.box(P, -P - w, U, P + w, P, U + hgt, STEEL, "wall");
  b.box(-P - w, -P, U, -P, P + w, U + hgt, STEEL, "wall");
  for (const sx of [-1, 1]) for (const sy of [-1, 1]) b.column(sx * 26, sy * 26, 1.6, 1.3, G - 5, U - 0.8, 10, STONE_WARM, "wall");
  // Ramps from the courtyard up to the ring, one per side, turning the same way.
  b.ramp(13, 5, 17, 22, "y", G, U, G, shade(STONE, 1.08), "floor");
  b.ramp(-13, -5, -17, -22, "y", G, U, G, shade(STONE, 1.08), "floor");
  b.ramp(-5, 13, -22, 17, "x", G, U, G, shade(STONE, 1.08), "floor");
  b.ramp(5, -13, 22, -17, "x", G, U, G, shade(STONE, 1.08), "floor");
  // A low dais in the middle and a post on each corner pad.
  b.column(0, 0, 4.2, 3.6, G, G + 0.35, 16, STONE_WARM, "floor");
  for (const sx of [-1, 1]) for (const sy of [-1, 1]) {
    b.column(sx * 34, sy * 34, 1.1, 0.8, G, G + 4.5, 8, STEEL, "metal");
    b.column(sx * 34, sy * 34, 1.9, 0, G + 4.5, G + 6.2, 8, [0.75, 0.42, 0.2], "metal");
  }
  // Low cover blocks at the ends of the arms.
  for (const [x, y] of [[39, 0], [-39, 0], [0, 39], [0, -39]] as const) b.box(x - 1.5, y - 1.5, G, x + 1.5, y + 1.5, G + 1.4, shade(STONE, 0.85), "wall");
  return b.done(m.name);
}

/** Orchard: the islands as thick mossy slabs, thin bridges between them,
 *  and cone trees and rocks dotted about. */
function orchard(m: MockMap): Arena {
  const b = new Builder();
  const polys = m.outline!.layers[0]!.polygons;
  const G = 1;
  polys.forEach((poly, i) => {
    if (poly.length > 4) b.slab(poly, G, G - 4 - (i % 3), shade(MOSS, 0.9 + 0.08 * (i % 3)), "floor", [0.36, 0.3, 0.24]);
    else b.slab(poly, G + 0.02, G - 0.5, [0.5, 0.4, 0.28], "wall");
  });
  const r = rng(7);
  const islands = polys.filter((p) => p.length > 4);
  for (const poly of islands) {
    const cx = poly.reduce((s, p) => s + p[0], 0) / poly.length, cy = poly.reduce((s, p) => s + p[1], 0) / poly.length;
    const rad = Math.min(...poly.map((p) => Math.hypot(p[0] - cx, p[1] - cy)));
    const trees = 3 + Math.floor(r() * 3);
    for (let i = 0; i < trees; i++) {
      const a = r() * Math.PI * 2, d = rad * (0.45 + 0.4 * r());
      const x = cx + Math.cos(a) * d, y = cy + Math.sin(a) * d, s = 0.8 + r() * 0.7;
      b.column(x, y, 0.35 * s, 0.25 * s, G, G + 1.6 * s, 6, BARK, "wall");
      b.column(x, y, 1.9 * s, 0, G + 1.3 * s, G + 4.6 * s, 8, shade(LEAF, 0.85 + 0.3 * r()), "foliage");
      b.column(x, y, 1.3 * s, 0, G + 3.2 * s, G + 6.0 * s, 8, shade(LEAF, 1 + 0.3 * r()), "foliage");
    }
    const a = r() * Math.PI * 2;
    b.column(cx + Math.cos(a) * rad * 0.25, cy + Math.sin(a) * rad * 0.25, 1.4, 0.7, G, G + 1.1, 5, [0.42, 0.42, 0.44], "default");
  }
  return b.done(m.name);
}

const BUILDERS: Record<string, (m: MockMap) => Arena> = { teacup, orchard };

/** The mock's arena for a map, or null when it has none (a map with no
 *  outline has no geometry either, so the app's fallbacks get exercised). */
export function buildArena(name: string): Arena | null {
  const m = MAPS.find((x) => x.name === name);
  const make = BUILDERS[name];
  return m && m.outline && make ? make(m) : null;
}

// ---- an indoor level ----

/** The arena shut inside a busy building, for trying the 3D view's ways of
 *  seeing into a level (H2_MOCK_INDOOR=1): an outer shell of walls and a
 *  ceiling that faces inwards like a real level's structure (and is written
 *  double-sided, so only the viewer's culling opens it), partition walls
 *  with doorways, an upper floor over half the rooms and rows of pillars,
 *  split over two `bsp_<n>` nodes (zones), a third marked cinematic-only,
 *  a `backdrop` of far hills; and two outward-facing "device machines"
 *  under a `machines` node. Only the glb has these: the pre-drawn views
 *  stay the open arena. */
export function enclose(a: Arena): Arena {
  const { min, max } = a.bounds;
  const x0 = min[0] - 4, x1 = max[0] + 4, y0 = min[1] - 4, y1 = max[1] + 4;
  const floor = min[2], ground = Math.min(max[2], floor + 5), top = Math.max(max[2], ground) + 6.5, upper = ground + 3.4;
  const WALL: Rgb = [0.62, 0.6, 0.56], DARK: Rgb = [0.4, 0.42, 0.47];
  const extra: Tri[] = [];
  const add = (node: string, build: (b: Builder) => void, flip = false) => {
    const b = new Builder();
    build(b);
    for (const t of b.tris) extra.push(flip ? { ...t, b: t.c, c: t.b, n: [-t.n[0], -t.n[1], -t.n[2]], node } : { ...t, node });
  };
  // The shell: a box seen from inside.
  add("bsp_0", (b) => b.box(x0, y0, floor - 0.5, x1, y1, top, WALL, "shell"), true);
  const N = 5, cw = (x1 - x0) / N, ch = (y1 - y0) / N, T = 0.35;
  add("bsp_1", (b) => {
    for (let i = 1; i < N; i++) for (let j = 0; j < N; j++) {
      // Partition walls along both axes, each with a doorway in the middle.
      const x = x0 + i * cw, ya = y0 + j * ch, door = ch * 0.3;
      b.box(x - T, ya, floor, x + T, ya + (ch - door) / 2, top, shade(WALL, 0.92), "wall");
      b.box(x - T, ya + (ch + door) / 2, floor, x + T, ya + ch, top, shade(WALL, 0.92), "wall");
      const y = y0 + i * ch, xa = x0 + j * cw, gap = cw * 0.3;
      b.box(xa, y - T, floor, xa + (cw - gap) / 2, y + T, top, shade(WALL, 0.84), "wall");
      b.box(xa + (cw + gap) / 2, y - T, floor, xa + cw, y + T, top, shade(WALL, 0.84), "wall");
    }
    // An upper floor over every other room.
    for (let i = 0; i < N; i++) for (let j = 0; j < N; j++) {
      if ((i + j) % 2) b.box(x0 + i * cw, y0 + j * ch, upper, x0 + (i + 1) * cw, y0 + (j + 1) * ch, upper + 0.3, DARK, "floor");
    }
  });
  add("bsp_0", (b) => {
    for (let i = 0; i < N; i++) for (let j = 0; j < N; j++) {
      const cx = x0 + (i + 0.5) * cw, cy = y0 + (j + 0.5) * ch;
      for (const [dx, dy] of [[-0.3, -0.3], [0.3, 0.3]] as const) b.column(cx + dx * cw, cy + dy * ch, 0.45, 0.45, floor, top, 8, STEEL, "metal");
    }
  });
  // Machines: ordinary closed models, facing outwards.
  add("machines/wheel", (b) => b.column((x0 + x1) / 2, (y0 + y1) / 2, 3.2, 3.2, upper + 1, upper + 2.2, 20, [0.75, 0.42, 0.2], "metal"));
  add("machines/dish", (b) => b.column(x0 + cw * 0.5, y0 + ch * 0.5, 0.4, 2.4, ground, ground + 2.6, 14, [0.3, 0.55, 0.7], "metal"));
  // A zone only a cutscene uses (marked so in its node's extras): a stage hung over the middle.
  add("bsp_2", (b) => b.box((x0 + x1) / 2 - 6, (y0 + y1) / 2 - 6, top - 2.5, (x0 + x1) / 2 + 6, (y0 + y1) / 2 + 6, top - 1.5, [0.8, 0.25, 0.6], "metal"));
  // Far scenery: a ring of hills well outside the walls.
  add("backdrop", (b) => {
    const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2, far = Math.max(x1 - x0, y1 - y0) * 1.1;
    for (let i = 0; i < 12; i++) {
      const t = (i / 12) * Math.PI * 2, tall = 14 + (i % 3) * 7;
      b.column(cx + Math.cos(t) * far, cy + Math.sin(t) * far, 12, 2, floor - 0.5, floor + tall, 7, [0.36, 0.42, 0.34], "default");
    }
  });
  return { ...a, extra, outer: { min: [x0, y0, floor - 0.5], max: [x1, y1, top] }, nodeExtras: { bsp_2: { cinematic: true } } };
}

// ---- glb ----

const round4 = (v: number) => Math.round(v * 1e4) / 1e4;

/** The arena as a binary glTF: one `structure` mesh, a primitive per
 *  material with POSITION, NORMAL and COLOR_0, vertices already in glTF
 *  axes (game (x, y, z) → (x, z, -y)). */
export function encodeGlb(a: Arena): Uint8Array {
  const gl = (v: V3): V3 => [v[0], v[2], -v[1]];
  const all = a.extra ? [...a.tris, ...a.extra] : a.tris;
  const names = [...new Set(all.map((t) => t.material))];
  const nodeNames = [...new Set(all.map((t) => t.node ?? "structure"))];
  const chunks: Uint8Array[] = [];
  let offset = 0;
  const bufferViews: unknown[] = [], accessors: unknown[] = [];
  const meshes: Array<{ name: string; primitives: unknown[] }> = [];
  const push = (data: Float32Array | Uint32Array, target: number) => {
    const bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: bytes.length, target });
    chunks.push(bytes);
    offset += bytes.length; // multiples of 4 already
    return bufferViews.length - 1;
  };
  for (const nodeName of nodeNames) {
   const primitives: unknown[] = [];
   meshes.push({ name: nodeName.split("/").pop()!, primitives });
   names.forEach((name, mi) => {
    const tris = all.filter((t) => t.material === name && (t.node ?? "structure") === nodeName);
    if (!tris.length) return;
    const n = tris.length * 3;
    const pos = new Float32Array(n * 3), nor = new Float32Array(n * 3), col = new Float32Array(n * 3), idx = new Uint32Array(n);
    const min: V3 = [Infinity, Infinity, Infinity], max: V3 = [-Infinity, -Infinity, -Infinity];
    tris.forEach((t, ti) => {
      const nn = gl(t.n);
      [t.a, t.b, t.c].forEach((v, vi) => {
        const p = gl(v), o = (ti * 3 + vi) * 3;
        for (let k = 0; k < 3; k++) {
          pos[o + k] = p[k]!; nor[o + k] = nn[k]!; col[o + k] = t.colour[k]!;
          if (p[k]! < min[k]!) min[k] = p[k]!;
          if (p[k]! > max[k]!) max[k] = p[k]!;
        }
        idx[ti * 3 + vi] = ti * 3 + vi;
      });
    });
    const first = accessors.length;
    accessors.push(
      { bufferView: push(pos, 34962), componentType: 5126, count: n, type: "VEC3", min: Array.from(new Float32Array(min)), max: Array.from(new Float32Array(max)) },
      { bufferView: push(nor, 34962), componentType: 5126, count: n, type: "VEC3" },
      { bufferView: push(col, 34962), componentType: 5126, count: n, type: "VEC3" },
      { bufferView: push(idx, 34963), componentType: 5125, count: n, type: "SCALAR" },
    );
    primitives.push({ attributes: { POSITION: first, NORMAL: first + 1, COLOR_0: first + 2 }, indices: first + 3, material: mi, mode: 4 });
   });
  }
  // A node per mesh; `machines/<name>` ones hang under one `machines` node.
  const nodes: Array<{ name: string; mesh?: number; children?: number[]; extras?: Record<string, unknown> }> = [];
  const roots: number[] = [];
  let machines: { name: string; children: number[] } | null = null;
  nodeNames.forEach((nodeName, i) => {
    nodes.push({ name: meshes[i]!.name, mesh: i, ...(a.nodeExtras?.[nodeName] ? { extras: a.nodeExtras[nodeName] } : {}) });
    if (!nodeName.startsWith("machines/")) { roots.push(nodes.length - 1); return; }
    if (!machines) { machines = { name: "machines", children: [] }; }
    machines.children.push(nodes.length - 1);
  });
  if (machines) { nodes.push(machines); roots.push(nodes.length - 1); }
  const json = {
    asset: {
      version: "2.0", generator: "h2 live mock",
      extras: { generator: "h2 live mock", geometry_version: 1, bounds: { min: (a.outer ?? a.bounds).min.map(round4), max: (a.outer ?? a.bounds).max.map(round4) } },
    },
    scene: 0,
    scenes: [{ nodes: roots }],
    nodes,
    meshes,
    materials: names.map((name) => ({
      name,
      ...(name === "shell" ? { doubleSided: true } : {}),
      pbrMetallicRoughness: { baseColorFactor: [1, 1, 1, 1], metallicFactor: MATERIALS[name].metallic, roughnessFactor: MATERIALS[name].roughness },
    })),
    buffers: [{ byteLength: offset }],
    bufferViews, accessors,
  };
  let text = new TextEncoder().encode(JSON.stringify(json));
  if (text.length % 4) { const padded = new Uint8Array(text.length + 4 - (text.length % 4)).fill(0x20); padded.set(text); text = padded; }
  const total = 12 + 8 + text.length + 8 + offset;
  const out = new Uint8Array(total);
  const dv = new DataView(out.buffer);
  dv.setUint32(0, 0x46546c67, true); dv.setUint32(4, 2, true); dv.setUint32(8, total, true);
  dv.setUint32(12, text.length, true); dv.setUint32(16, 0x4e4f534a, true);
  out.set(text, 20);
  let at = 20 + text.length;
  dv.setUint32(at, offset, true); dv.setUint32(at + 4, 0x004e4942, true);
  at += 8;
  for (const c of chunks) { out.set(c, at); at += c.length; }
  return out;
}

// ---- views ----

export interface ViewSpec {
  id: string;
  width: number;
  height: number;
  /** Screen right, screen up and towards-the-viewer, unit, game coordinates. */
  right: V3; up: V3; toward: V3;
  scale: number; ox: number; oy: number;
  north?: boolean;
}

const TOP_SIZE = 1024, ISO_W = 1024, ISO_H = 768, MARGIN = 24;

function viewFor(id: string, a: Arena): ViewSpec {
  let right: V3, up: V3, toward: V3, width: number, height: number;
  if (id === "top") {
    right = [1, 0, 0]; up = [0, 1, 0]; toward = [0, 0, 1]; width = height = TOP_SIZE;
  } else {
    // The viewer stands off in the named compass corner, 30° up.
    const az = ({ "iso-ne": 45, "iso-se": 135, "iso-sw": 225, "iso-nw": 315 } as Record<string, number>)[id]! * Math.PI / 180;
    const el = 30 * Math.PI / 180;
    toward = [Math.cos(el) * Math.sin(az), Math.cos(el) * Math.cos(az), Math.sin(el)];
    const fwd: V3 = [-toward[0], -toward[1], -toward[2]];
    right = norm(cross(fwd, [0, 0, 1]));
    up = norm(cross(right, fwd));
    width = ISO_W; height = ISO_H;
  }
  // Fit the bounds' eight corners in the picture.
  let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
  for (const x of [a.bounds.min[0], a.bounds.max[0]]) for (const y of [a.bounds.min[1], a.bounds.max[1]]) for (const z of [a.bounds.min[2], a.bounds.max[2]]) {
    const u = dot(right, [x, y, z]), v = dot(up, [x, y, z]);
    x0 = Math.min(x0, u); x1 = Math.max(x1, u); y0 = Math.min(y0, v); y1 = Math.max(y1, v);
  }
  const scale = Math.min((width - MARGIN * 2) / (x1 - x0), (height - MARGIN * 2) / (y1 - y0));
  return {
    id, width, height, right, up, toward, scale,
    ox: width / 2 - scale * (x0 + x1) / 2,
    oy: height / 2 + scale * (y0 + y1) / 2,
    north: id === "top",
  };
}

export const VIEW_IDS = ["top", "iso-ne", "iso-nw", "iso-se", "iso-sw"];

export function worldToPixelOf(v: ViewSpec): [[number, number, number, number], [number, number, number, number]] {
  const s = v.scale;
  return [
    [round4(s * v.right[0]), round4(s * v.right[1]), round4(s * v.right[2]), round4(v.ox)],
    [round4(-s * v.up[0]), round4(-s * v.up[1]), round4(-s * v.up[2]), round4(v.oy)],
  ];
}

export function viewsJson(a: Arena) {
  return {
    geometry_version: 1,
    map: a.name,
    bounds: { min: a.bounds.min.map(round4), max: a.bounds.max.map(round4) },
    views: VIEW_IDS.map((id) => {
      const v = viewFor(id, a);
      return {
        id, kind: "ortho", width: v.width, height: v.height, world_to_pixel: worldToPixelOf(v), file: `views/${id}.png`,
        ...(v.north ? { up: "north" } : { depth_sorted: true }),
      };
    }),
  };
}

const SUN = norm([-0.45, -0.35, 0.82]);
const toSrgb = (v: number) => (v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055);

/** Draws one view: an orthographic z-buffered picture of the arena, lit by
 *  a sun and an ambient term with a height tint, on a transparent
 *  background. Supersampled 2×. */
export function drawView(a: Arena, id: string): { rgba: Uint8Array; width: number; height: number } {
  const v = viewFor(id, a);
  const S = 2, W = v.width * S, H = v.height * S;
  const depth = new Float32Array(W * H).fill(-Infinity);
  const colour = new Float32Array(W * H * 3);
  const z0 = a.bounds.min[2], zr = Math.max(1e-6, a.bounds.max[2] - z0);
  const px = (p: V3) => (v.scale * dot(v.right, p) + v.ox) * S;
  const py = (p: V3) => (-v.scale * dot(v.up, p) + v.oy) * S;
  for (const t of a.tris) {
    if (dot(t.n, v.toward) <= 1e-6) continue;
    const ax = px(t.a), ay = py(t.a), bx = px(t.b), by = py(t.b), cx = px(t.c), cy = py(t.c);
    const da = dot(v.toward, t.a), db = dot(v.toward, t.b), dc = dot(v.toward, t.c);
    const area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    if (Math.abs(area) < 1e-9) continue;
    const lit = 0.42 + 0.58 * Math.max(0, dot(t.n, SUN)) + 0.1 * Math.max(0, t.n[2]);
    const xa = Math.max(0, Math.floor(Math.min(ax, bx, cx))), xb = Math.min(W - 1, Math.ceil(Math.max(ax, bx, cx)));
    const ya = Math.max(0, Math.floor(Math.min(ay, by, cy))), yb = Math.min(H - 1, Math.ceil(Math.max(ay, by, cy)));
    for (let y = ya; y <= yb; y++) for (let x = xa; x <= xb; x++) {
      const qx = x + 0.5, qy = y + 0.5;
      const w0 = ((bx - qx) * (cy - qy) - (by - qy) * (cx - qx)) / area;
      const w1 = ((cx - qx) * (ay - qy) - (cy - qy) * (ax - qx)) / area;
      const w2 = 1 - w0 - w1;
      if (w0 < 0 || w1 < 0 || w2 < 0) continue;
      const d = w0 * da + w1 * db + w2 * dc;
      const i = y * W + x;
      if (d <= depth[i]!) continue;
      depth[i] = d;
      // Higher ground a little brighter, so floors read apart from above.
      const z = w0 * t.a[2] + w1 * t.b[2] + w2 * t.c[2];
      const k = lit * (0.8 + 0.28 * (z - z0) / zr);
      colour[i * 3] = t.colour[0] * k; colour[i * 3 + 1] = t.colour[1] * k; colour[i * 3 + 2] = t.colour[2] * k;
    }
  }
  const rgba = new Uint8Array(v.width * v.height * 4);
  for (let y = 0; y < v.height; y++) for (let x = 0; x < v.width; x++) {
    let r = 0, g = 0, b = 0, n = 0;
    for (let sy = 0; sy < S; sy++) for (let sx = 0; sx < S; sx++) {
      const i = (y * S + sy) * W + x * S + sx;
      if (depth[i] === -Infinity) continue;
      r += colour[i * 3]!; g += colour[i * 3 + 1]!; b += colour[i * 3 + 2]!; n++;
    }
    if (!n) continue;
    const o = (y * v.width + x) * 4;
    rgba[o] = Math.round(255 * Math.min(1, toSrgb(r / n)));
    rgba[o + 1] = Math.round(255 * Math.min(1, toSrgb(g / n)));
    rgba[o + 2] = Math.round(255 * Math.min(1, toSrgb(b / n)));
    rgba[o + 3] = Math.round(255 * n / (S * S));
  }
  return { rgba, width: v.width, height: v.height };
}

// ---- what the mock server serves ----

export interface MockMapGeometry {
  /** The manifest's per-map table: `maps[stem] = { geometry, views }`. */
  maps: Record<string, { geometry: string; views: string }>;
  file(name: string): { bytes: Uint8Array; type: string } | null;
}

/** The geometry files for every mock map that has an arena. Each file is
 *  made on first request and kept. */
export function mockMapGeometry(opts: { indoor?: boolean } = {}): MockMapGeometry {
  const maps: MockMapGeometry["maps"] = {};
  const arenas = new Map<string, Arena>();
  for (const m of MAPS) {
    if (!BUILDERS[m.name] || !m.outline) continue;
    maps[m.name] = { geometry: `map/${m.name}/map.glb`, views: `map/${m.name}/views.json` };
  }
  const arena = (name: string) => {
    let a = arenas.get(name);
    if (!a && maps[name]) { a = buildArena(name)!; if (opts.indoor) a = enclose(a); arenas.set(name, a); }
    return a ?? null;
  };
  const done = new Map<string, { bytes: Uint8Array; type: string }>();
  return {
    maps,
    file(name) {
      const hit = done.get(name);
      if (hit) return hit;
      const m = /^map\/([^/]+)\/(map\.glb|views\.json|views\/([a-z-]+)\.png)$/.exec(name);
      const a = m ? arena(m[1]!) : null;
      if (!m || !a) return null;
      let out: { bytes: Uint8Array; type: string };
      if (m[2] === "map.glb") out = { bytes: encodeGlb(a), type: "model/gltf-binary" };
      else if (m[2] === "views.json") out = { bytes: new TextEncoder().encode(JSON.stringify(viewsJson(a))), type: "application/json" };
      else {
        if (!VIEW_IDS.includes(m[3]!)) return null;
        const pic = drawView(a, m[3]!);
        out = { bytes: encodePng(pic.rgba, pic.width, pic.height), type: "image/png" };
      }
      done.set(name, out);
      return out;
    },
  };
}
