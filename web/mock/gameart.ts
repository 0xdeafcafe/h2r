// A stand-in for a real server's optional game art (docs/live-api.md
// "Optional game art"), so the app's game-art path can be run and tested
// without a disc. Nothing here comes from the game: rank icons and medals
// are the art pack's own, the palette is ours, and the emblem masks,
// game-type icons, weapon masks and map previews are drawn below from
// simple shapes and the mock's invented maps.
//
// Emblem layers follow the real encoding: mask a in red (and green), mask b
// in blue, alpha a copy of mask b (so alpha is NOT coverage).

import { deflateSync } from "node:zlib";
import { EMBLEM_COLOURS, type GameArtManifest } from "../src/lib/gameart";
import { MAPS } from "./maps";
import { LEVELS } from "./coop";

// ---- a minimal PNG encoder (8-bit RGBA) ----

const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC[(c ^ b) & 255]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + data.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  out.set(data, 8);
  view.setUint32(8 + data.length, crc32(out.subarray(4, 8 + data.length)));
  return out;
}

export function encodePng(rgba: Uint8Array | Uint8ClampedArray, width: number, height: number): Uint8Array {
  const head = new Uint8Array(13);
  const hv = new DataView(head.buffer);
  hv.setUint32(0, width); hv.setUint32(4, height);
  head[8] = 8; head[9] = 6; // 8 bits, RGBA
  const raw = new Uint8Array((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) raw.set(rgba.subarray(y * width * 4, (y + 1) * width * 4), y * (width * 4 + 1) + 1);
  const parts = [
    Uint8Array.of(0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a),
    chunk("IHDR", head), chunk("IDAT", new Uint8Array(deflateSync(raw))), chunk("IEND", new Uint8Array(0)),
  ];
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
}

// ---- drawing: a picture is a function of (x, y) in -1..1, supersampled ----

type Px = [number, number, number, number]; // 0..1
function render(w: number, h: number, at: (x: number, y: number) => Px): Uint8Array {
  const out = new Uint8Array(w * h * 4), S = 3;
  for (let py = 0; py < h; py++) for (let px = 0; px < w; px++) {
    const sum = [0, 0, 0, 0];
    for (let sy = 0; sy < S; sy++) for (let sx = 0; sx < S; sx++) {
      const p = at(((px + (sx + 0.5) / S) / w) * 2 - 1, ((py + (sy + 0.5) / S) / h) * 2 - 1);
      for (let c = 0; c < 4; c++) sum[c]! += p[c]!;
    }
    for (let c = 0; c < 4; c++) out[(py * w + px) * 4 + c] = Math.round((sum[c]! / (S * S)) * 255);
  }
  return out;
}

/** An emblem layer from its two masks: a in red and green, b in blue and alpha. */
const layer = (size: number, a: (x: number, y: number) => boolean, b: (x: number, y: number) => boolean) =>
  render(size, size, (x, y) => { const ma = a(x, y) ? 1 : 0, mb = b(x, y) ? 1 : 0; return [ma, ma, mb, mb]; });

const rot = (x: number, y: number, t: number): [number, number] => [x * Math.cos(t) - y * Math.sin(t), x * Math.sin(t) + y * Math.cos(t)];

/** Eight shape families, each in eight sizes/turns: 64 foregrounds. */
function foreground(n: number): Uint8Array {
  const kind = n % 8, v = n >> 3, turn = (v % 4) * (Math.PI / 8), r = 0.5 + (v >> 2) * 0.16;
  const shape = (x0: number, y0: number, s: number): boolean => {
    const [x, y] = rot(x0 / s, y0 / s, turn);
    const d = Math.hypot(x, y), ang = Math.atan2(y, x);
    switch (kind) {
      case 0: return d < 1;
      case 1: return d < 1 && d > 0.62;
      case 2: return Math.abs(x) + Math.abs(y) < 1;
      case 3: return (Math.abs(x) < 0.3 && Math.abs(y) < 1) || (Math.abs(y) < 0.3 && Math.abs(x) < 1);
      case 4: return Math.abs(x) < 1 && Math.abs(y) < 1 && Math.floor((y + 1) * 2.5) % 2 === 0;
      case 5: return Math.abs(x) < 1 && y < 0.9 - Math.abs(x) * 0.9 && y > 0.3 - Math.abs(x) * 0.9;
      case 6: return d < 0.55 + 0.45 * Math.cos(ang * 5);
      default: return Math.max(Math.abs(x), Math.abs(y)) < 1 && Math.max(Math.abs(x), Math.abs(y)) > 0.6;
    }
  };
  // Mask a is the shape; mask b a smaller copy of it, nudged, where a isn't.
  return layer(96, (x, y) => shape(x, y, r), (x, y) => !shape(x, y, r) && shape(x - 0.1, y - 0.1, r * 1.22));
}

/** Eight pattern families, four variations each: 32 backgrounds. */
function background(n: number): Uint8Array {
  const kind = n % 8, v = n >> 3, t = v * (Math.PI / 4);
  const a = (x0: number, y0: number): boolean => {
    const [x, y] = rot(x0, y0, t);
    switch (kind) {
      case 0: return true;
      case 1: return y < 0;
      case 2: return Math.floor((x + 2) * 3) % 2 === 0;
      case 3: return x * y > 0;
      case 4: return Math.hypot(x, y) < 0.8;
      case 5: return y < x * 0.6;
      case 6: return (Math.floor((x + 2) * 2.5) + Math.floor((y + 2) * 2.5)) % 2 === 0;
      default: return Math.floor(Math.hypot(x, y) * 4) % 2 === 0;
    }
  };
  // Mask b fills what a leaves, except on the plain one (a alone).
  return layer(96, a, (x, y) => kind !== 0 && !a(x, y));
}

const GAMETYPES = ["slayer", "king_of_the_hill", "oddball", "juggernaut", "ctf", "assault", "territories"];

/** White glyphs on transparency: seven named types, then four plain ones. */
function gametype(n: number): Uint8Array {
  const on = (x: number, y: number): boolean => {
    const d = Math.hypot(x, y);
    switch (n) {
      case 0: return (Math.abs(x - y) < 0.2 || Math.abs(x + y) < 0.2) && d < 0.95; // crossed
      case 1: return y > -0.7 && y < 0.8 && Math.abs(x) < (y + 0.7) * 0.6; // a hill
      case 2: return d < 0.75; // a ball
      case 3: return Math.abs(x) + Math.abs(y) < 0.9 && !(d < 0.3); // a marked one
      case 4: return (x > -0.7 && x < -0.5 && Math.abs(y) < 0.9) || (x >= -0.5 && x < 0.7 && y > -0.85 && y < -0.05); // a flag
      case 5: return (d < 0.8 && d > 0.5) || d < 0.22; // a target
      case 6: return Math.abs(x) < 0.85 && Math.abs(y) < 0.85 && Math.abs(x) > 0.12 && Math.abs(y) > 0.12; // four plots
      default: return Math.abs(y) < 0.8 && Math.abs(x) < 0.9 && Math.floor((x + 1) * (n - 4)) % 2 === 0;
    }
  };
  return render(32, 32, (x, y) => (on(x, y) ? [1, 1, 1, 1] : [1, 1, 1, 0]));
}

const WEAPONS = ["energy_sword", "sniper_rifle", "smg", "rocket_launcher"];

/** Weapon silhouettes in the real encoding: one mask, in blue and alpha,
 *  where "full" is only about a quarter. */
function weapon(n: number): Uint8Array {
  const box = (x: number, y: number, x0: number, y0: number, x1: number, y1: number) => x > x0 && x < x1 && y > y0 && y < y1;
  const on = (x: number, y: number): boolean => {
    switch (n) {
      case 0: return (Math.abs(y) < 0.5 * (1 - (x + 0.9) / 1.3) && x > -0.9 && x < 0.4) || box(x, y, 0.4, -0.8, 0.52, 0.8) || box(x, y, 0.52, -0.2, 0.95, 0.2);
      case 1: return box(x, y, -0.95, -0.2, 0.95, 0.1) || box(x, y, -0.2, -0.6, 0.35, -0.2) || box(x, y, 0.45, 0.1, 0.95, 0.5) || box(x, y, 0.1, 0.1, 0.25, 0.8);
      case 2: return box(x, y, -0.6, -0.5, 0.6, 0) || box(x, y, 0.2, 0, 0.5, 0.9) || box(x, y, -0.3, 0, -0.1, 0.6);
      default: return box(x, y, -0.95, -0.5, 0.7, 0.2) || box(x, y, 0.7, -0.75, 0.95, 0.45) || box(x, y, -0.1, 0.2, 0.15, 0.85);
    }
  };
  // Faint, like the real ones, which peak at 68 of 255.
  return render(62, 30, (x, y) => { const m = on(x, y) ? 68 / 255 : 0; return [0, 0, m, m]; });
}

/** The pack's medal for each of the game's medal names our derived medals map onto. */
const PACK_MEDALS: Array<[alias: string, pack: string]> = [
  ["killing_spree", "hot-streak"], ["running_riot", "wildfire"], ["rampage", "untouchable"],
  ["double_kill", "twofer"], ["triple_kill", "hat-trick"], ["killtacular", "full-house"], ["kill_frenzy", "encore"],
];

const hex = (c: readonly number[]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");

function inPoly(x: number, y: number, poly: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i]!, [xj, yj] = poly[j]!;
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

/** A "postcard" of a mock map: its floor plan tilted away under a sky. */
function mapPreview(index: number): Uint8Array | null {
  const o = MAPS[index]!.outline;
  if (!o) return null;
  const hue = [[0.95, 0.62, 0.3], [0.45, 0.75, 0.4], [0.5, 0.6, 0.95]][index % 3]!;
  const [x0, y0] = o.bounds.min, [x1, y1] = o.bounds.max;
  return render(320, 180, (sx, sy) => {
    const sky: Px = [0.1 + 0.25 * (sy + 1) * hue[0]!, 0.14 + 0.25 * (sy + 1) * hue[1]!, 0.22 + 0.3 * (sy + 1) * hue[2]!, 1];
    if (sy < -0.25) return sky;
    // Ground plane: rows further up the picture are further away.
    const depth = 1 / (sy + 0.45), gx = sx * depth * 0.62, gy = depth * 0.9 - 1.6;
    const wx = x0 + ((gx + 1) / 2) * (x1 - x0), wy = y0 + ((gy + 1) / 2) * (y1 - y0);
    let lit = 0;
    o.layers.forEach((l, li) => { if (l.polygons.some((p) => inPoly(wx, wy, p))) lit = li + 1; });
    const fade = Math.min(1, Math.max(0, (sy + 0.25) * 2.2));
    const base = lit ? 0.45 + 0.2 * lit : 0.12;
    return [base * hue[0]! * fade + sky[0] * (1 - fade), base * hue[1]! * fade + sky[1] * (1 - fade), base * hue[2]! * fade + sky[2] * (1 - fade), 1];
  });
}

const PACK_RANKS = ["01-newcomer", "02-regular", "03-contender", "04-veteran", "05-ace", "06-headliner", "07-vanguard-spare", "08-legend"];

export interface MockGameArt {
  manifest: GameArtManifest;
  /** The bytes of a file the manifest names, or null. */
  file(name: string): Promise<{ bytes: Uint8Array; type: string } | null>;
}

/** Builds the fake manifest. `packDir` is web/public/art. Pictures are drawn
 *  on first request. */
export function mockGameArt(packDir: string): MockGameArt {
  const images: GameArtManifest["images"] = {};
  const make = new Map<string, () => Uint8Array | Promise<Uint8Array>>();
  const add = (key: string, file: string, width: number, height: number, draw: () => Uint8Array | Promise<Uint8Array>) => {
    images[key] = { file, width, height };
    make.set(file, draw);
  };
  for (let n = 0; n < 64; n++) add(`emblem/fg/${n}`, `emblem/fg/${n}.png`, 96, 96, () => encodePng(foreground(n), 96, 96));
  for (let n = 0; n < 32; n++) add(`emblem/bg/${n}`, `emblem/bg/${n}.png`, 96, 96, () => encodePng(background(n), 96, 96));
  for (let level = 1; level <= 50; level++) {
    // The pack's eight badges spread over the fifty levels.
    const badge = PACK_RANKS[Math.min(PACK_RANKS.length - 1, Math.floor(((level - 1) / 50) * PACK_RANKS.length))]!;
    add(`rank/${level}`, `rank/${level}.webp`, 128, 128, async () => new Uint8Array(await Bun.file(`${packDir}/ranks/${badge}@2x.webp`).arrayBuffer()));
  }
  for (let n = 0; n < 11; n++) add(`gametype/${n}`, `gametype/${n}.png`, 32, 32, () => encodePng(gametype(n), 32, 32));
  for (let n = 0; n < WEAPONS.length; n++) add(`weapon/${n}`, `weapon/${n}.png`, 62, 30, () => encodePng(weapon(n), 62, 30));
  PACK_MEDALS.forEach(([, pack], n) => add(`medal/${n}`, `medal/${n}.webp`, 128, 128, async () => new Uint8Array(await Bun.file(`${packDir}/medals/${pack}@2x.webp`).arrayBuffer())));
  const skipped: Record<string, string> = { "live/": "the mock draws no status icons" };
  MAPS.forEach((m, i) => {
    if (m.outline) add(`map/${m.name}/preview`, `map/${m.name}.png`, 320, 180, () => encodePng(mapPreview(i)!, 320, 180));
    else skipped[`map/${m.name}/preview`] = "the mock map has no outline to draw";
  });
  // Campaign levels (mock/coop.ts) borrow their ground's picture; one is
  // left without, so the pages' fallbacks show.
  LEVELS.forEach((l, n) => {
    const i = MAPS.findIndex((m) => m.outline === l.outline);
    if (n % 4 !== 3 && i >= 0 && l.outline) add(`map/${l.name}/preview`, `map/${l.name}.png`, 320, 180, () => encodePng(mapPreview(i)!, 320, 180));
  });
  const manifest: GameArtManifest = {
    version: 1,
    source: "mock",
    images,
    aliases: Object.fromEntries([
      ...GAMETYPES.map((name, n) => [`gametype/${name}`, `gametype/${n}`]),
      ...WEAPONS.map((name, n) => [`weapon/${name}`, `weapon/${n}`]),
      ...PACK_MEDALS.map(([alias], n) => [`medal/${alias}`, `medal/${n}`]),
    ]),
    // Our own colours, in the reverse of the app's built-in order so a test can tell which was used.
    palette: [...EMBLEM_COLOURS].reverse().map(hex),
    emblem: {
      layers: { background: "emblem/bg/<n>", foreground: "emblem/fg/<n>" },
      colour: "per pixel, with channels in 0..1: rgb = colour_a * red + colour_b * blue; ignore the image's alpha",
      coverage: { background: "opaque", foreground: "max(red, blue); draw it over the background" },
    },
    skipped,
  };
  const done = new Map<string, Promise<Uint8Array>>();
  return {
    manifest,
    async file(name) {
      const draw = make.get(name);
      if (!draw) return null;
      let p = done.get(name);
      if (!p) { p = Promise.resolve(draw()); done.set(name, p); }
      return { bytes: await p, type: name.endsWith(".webp") ? "image/webp" : "image/png" };
    },
  };
}
