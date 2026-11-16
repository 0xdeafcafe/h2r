// Draws one top-down map frame onto a 2D canvas. World x is right, world y
// is up (so canvas y is flipped), z is height.

import type { MapOutline } from "../api/types";
import type { SampledPlayer, SampledVehicle } from "../state/frames";
import { actorColour, vehicleBox } from "../viewer/models";
import type { MapPalette } from "./palette";
import type { LiveShot } from "./shots";

export interface Bounds { minX: number; minY: number; maxX: number; maxY: number }

export interface RosterEntry { name: string; colour: string }

export interface Marker { x: number; y: number; z: number; alpha: number; colour: string }

export interface Scene {
  players: SampledPlayer[];
  markers: Marker[];
  roster: Map<number, RosterEntry>;
  outline: MapOutline | null;
  /** Bounds seen so far, used when there is no outline. */
  seen: Bounds | null;
  /** Selected height layer index, or null for all. */
  layer: number | null;
  mini: boolean;
  highlight?: number | null;
  /** The flat style (stats side): small solid dots with a dark hairline,
   *  no facing wedge, and labels only for the hovered or selected player. */
  flat?: boolean;
  /** Player index under the pointer, when the canvas is tracking one. */
  hover?: number | null;
  /** AI-controlled bipeds (campaign): small squares in their side's colour, never named. */
  actors?: SampledPlayer[];
  /** Vehicles in use: an outline of about their size under whoever is in them. */
  vehicles?: SampledVehicle[];
  /** Shots on screen now: a fading line from the player along their aim. */
  shots?: LiveShot[];
  /** Players to show as firing with a steady, brighter aim line (reduced
   *  motion, where nothing flashes). */
  firing?: ReadonlySet<number>;
}

/** The line a shot draws on a 2D map: from (x0, y0) to (x1, y1), bright at
 *  the player and fading out along its length, over a dark under-stroke so
 *  it reads on any picture. */
export function strokeTracer(ctx: CanvasRenderingContext2D, x0: number, y0: number, x1: number, y1: number, alpha: number, width = 2) {
  if (!(alpha > 0) || (x0 === x1 && y0 === y1)) return;
  const fade = (rgb: string, a: number) => {
    const g = ctx.createLinearGradient(x0, y0, x1, y1);
    g.addColorStop(0, `rgba(${rgb}, ${a})`);
    g.addColorStop(1, `rgba(${rgb}, 0)`);
    return g;
  };
  ctx.lineCap = "round";
  ctx.globalAlpha = Math.min(1, alpha);
  for (const [style, w] of [[fade("0, 0, 0", 0.55), width + 2], [fade("255, 240, 190", 1), width]] as const) {
    ctx.strokeStyle = style;
    ctx.lineWidth = w;
    ctx.beginPath();
    ctx.moveTo(x0, y0);
    ctx.lineTo(x1, y1);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
}

/** A vehicle seen from above: a rounded outline `len` by `wid` pixels,
 *  turned to `angle` (canvas radians), with a notch of a nose. */
export function drawVehicle(ctx: CanvasRenderingContext2D, x: number, y: number, angle: number, len: number, wid: number, colour: string) {
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(angle);
  ctx.beginPath();
  ctx.roundRect(-len / 2, -wid / 2, len, wid, Math.min(len, wid) * 0.25);
  ctx.fillStyle = "rgba(0, 0, 0, 0.35)";
  ctx.fill();
  ctx.lineWidth = 3.5;
  ctx.strokeStyle = "rgba(0, 0, 0, 0.75)";
  ctx.stroke();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = colour;
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(len / 2, -wid * 0.3); ctx.lineTo(len / 2 + wid * 0.35, 0); ctx.lineTo(len / 2, wid * 0.3);
  ctx.stroke();
  ctx.restore();
}

/** An AI-controlled biped: a small square, so it is never taken for a player's dot. */
export function drawActor(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, colour: string) {
  ctx.fillStyle = colour;
  ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.rect(x - r, y - r, r * 2, r * 2);
  ctx.fill();
  ctx.stroke();
}

/** The colour a vehicle's outline takes: its first player's, else its first actor's side, else grey. */
export function vehicleColour(id: string, players: readonly SampledPlayer[], actors: readonly SampledPlayer[] | undefined, roster: Map<number, RosterEntry>): string {
  const p = players.find((q) => q.alive && q.vehicle === id);
  if (p) return roster.get(p.index)?.colour ?? "#999";
  const a = actors?.find((q) => q.vehicle === id);
  return a ? actorColour(a.actor?.team) : "#9aa3ab";
}

/** A short line from a dot the way its player is looking. */
export function strokeAim(ctx: CanvasRenderingContext2D, x: number, y: number, dx: number, dy: number, from: number, to: number, colour: string, hot = false) {
  const len = Math.hypot(dx, dy);
  if (len < 1e-6) return;
  const ux = dx / len, uy = dy / len;
  ctx.lineCap = "round";
  for (const [style, w] of [["rgba(0, 0, 0, 0.7)", hot ? 4.5 : 3.5], [hot ? "rgb(255, 240, 190)" : colour, hot ? 2.5 : 1.5]] as const) {
    ctx.strokeStyle = style;
    ctx.lineWidth = w;
    ctx.beginPath();
    ctx.moveTo(x + ux * from, y + uy * from);
    ctx.lineTo(x + ux * to, y + uy * to);
    ctx.stroke();
  }
}

export interface View { cx: number; cy: number; scale: number; ready: boolean }

export function newView(): View {
  return { cx: 0, cy: 0, scale: 1, ready: false };
}

export function sceneBounds(s: Scene): Bounds | null {
  if (s.outline) {
    const b = s.outline.bounds;
    return { minX: b.min[0], minY: b.min[1], maxX: b.max[0], maxY: b.max[1] };
  }
  const b = s.seen;
  if (!b || !isFinite(b.minX)) return null;
  // Pad a little and keep a minimum size so 2 players don't zoom to a dot.
  const w = Math.max(30, b.maxX - b.minX), h = Math.max(30, b.maxY - b.minY);
  const cx = (b.minX + b.maxX) / 2, cy = (b.minY + b.maxY) / 2;
  return { minX: cx - w * 0.6, maxX: cx + w * 0.6, minY: cy - h * 0.6, maxY: cy + h * 0.6 };
}

/** Eases the view towards fitting `b` in a w*h canvas. */
export function fitView(v: View, b: Bounds | null, w: number, h: number, pad: number): void {
  if (!b) return;
  const bw = Math.max(1, b.maxX - b.minX), bh = Math.max(1, b.maxY - b.minY);
  const scale = Math.min((w - pad * 2) / bw, (h - pad * 2) / bh);
  const cx = (b.minX + b.maxX) / 2, cy = (b.minY + b.maxY) / 2;
  if (!v.ready) { v.cx = cx; v.cy = cy; v.scale = scale; v.ready = true; return; }
  v.cx += (cx - v.cx) * 0.08;
  v.cy += (cy - v.cy) * 0.08;
  v.scale += (scale - v.scale) * 0.08;
}

function layerOf(o: MapOutline | null, z: number): number | null {
  if (!o) return null;
  for (let i = 0; i < o.layers.length; i++) {
    const [a, b] = o.layers[i]!.z;
    if (z >= a && z <= b) return i;
  }
  return null;
}

export function draw(ctx: CanvasRenderingContext2D, w: number, h: number, s: Scene, v: View, pal: MapPalette): void {
  const sx = (x: number) => w / 2 + (x - v.cx) * v.scale;
  const sy = (y: number) => h / 2 - (y - v.cy) * v.scale;

  // A transparent background lets the page's map texture show through.
  ctx.clearRect(0, 0, w, h);
  if (pal.bg !== "transparent") {
    ctx.fillStyle = pal.bg;
    ctx.fillRect(0, 0, w, h);
  }

  // Grid: one line every "nice" world step.
  const step = niceStep(60 / Math.max(v.scale, 1e-6));
  if (v.ready && isFinite(step) && step > 0) {
    ctx.strokeStyle = pal.grid;
    ctx.lineWidth = 1;
    ctx.beginPath();
    const x0 = v.cx - w / 2 / v.scale, x1 = v.cx + w / 2 / v.scale;
    const y0 = v.cy - h / 2 / v.scale, y1 = v.cy + h / 2 / v.scale;
    for (let x = Math.ceil(x0 / step) * step; x <= x1; x += step) { const p = Math.round(sx(x)) + 0.5; ctx.moveTo(p, 0); ctx.lineTo(p, h); }
    for (let y = Math.ceil(y0 / step) * step; y <= y1; y += step) { const p = Math.round(sy(y)) + 0.5; ctx.moveTo(0, p); ctx.lineTo(w, p); }
    ctx.stroke();
  }

  // Floors: the selected layer solid, the others faint.
  if (s.outline) {
    s.outline.layers.forEach((layer, i) => {
      if (s.layer === null || s.layer === i) return;
      ctx.fillStyle = pal.floorOther;
      for (const poly of layer.polygons) tracePoly(ctx, poly, sx, sy), ctx.fill();
    });
    s.outline.layers.forEach((layer, i) => {
      if (!(s.layer === null || s.layer === i)) return;
      ctx.fillStyle = pal.floor;
      ctx.strokeStyle = pal.floorEdge;
      ctx.lineWidth = s.mini ? 1 : 2;
      ctx.lineJoin = "round";
      for (const poly of layer.polygons) { tracePoly(ctx, poly, sx, sy); ctx.fill(); ctx.stroke(); }
    });
  }

  // Death markers: little crosses that fade.
  const mr = s.mini ? 3 : 6;
  for (const m of s.markers) {
    const x = sx(m.x), y = sy(m.y);
    ctx.globalAlpha = Math.max(0, Math.min(1, m.alpha));
    ctx.strokeStyle = m.colour;
    ctx.lineWidth = s.flat ? (s.mini ? 1 : 1.5) : (s.mini ? 1.5 : 2.5);
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(x - mr, y - mr); ctx.lineTo(x + mr, y + mr);
    ctx.moveTo(x + mr, y - mr); ctx.lineTo(x - mr, y + mr);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;

  const r = s.flat ? (s.mini ? 3 : 5) : s.mini ? 3.5 : 8;

  // Shots: a fading line from the player along their aim (not on thumbnails).
  if (!s.mini) {
    for (const shot of s.shots ?? []) {
      const [ax, ay] = shot.aim, flatLen = Math.hypot(ax, ay);
      if (flatLen < 0.05) continue; // straight up or down: nothing to draw from above
      const len = Math.max(46, 9 * v.scale) * flatLen;
      const x = sx(shot.x), y = sy(shot.y);
      strokeTracer(ctx, x + (ax / flatLen) * r, y - (ay / flatLen) * r, x + (ax / flatLen) * (r + len), y - (ay / flatLen) * (r + len), shot.alpha);
    }
  }

  // Vehicles, then the AI, under the players.
  if (!s.mini) {
    for (const veh of s.vehicles ?? []) {
      const [len, wid] = vehicleBox(veh.kind);
      const k = Math.max(v.scale, 9 / wid);
      drawVehicle(ctx, sx(veh.x), sy(veh.y), Math.atan2(-veh.forward[1], veh.forward[0]), len * k, wid * k, vehicleColour(veh.id, s.players, s.actors, s.roster));
    }
  }
  for (const a of s.actors ?? []) {
    if (!a.alive || a.vehicle) continue;
    drawActor(ctx, sx(a.x), sy(a.y), s.mini ? 1.5 : 2.5, actorColour(a.actor?.team));
  }

  // Players.
  const sorted = s.players.slice().sort((a, b) => a.z - b.z);
  for (const p of sorted) {
    if (!p.alive) continue;
    const info = s.roster.get(p.index);
    const colour = info?.colour ?? "#999";
    const x = sx(p.x), y = sy(p.y);
    const off = s.outline && s.layer !== null && layerOf(s.outline, p.z) !== s.layer;
    ctx.globalAlpha = off ? 0.35 : 1;
    // Facing: a soft wedge ahead of the dot. Yaw 0 = +x, counter-clockwise.
    const a = -p.yaw;
    const picked = s.highlight === p.index || s.hover === p.index;
    // Where they are looking, when the frame says: a short line from the dot.
    if (!s.mini && p.aim) {
      const hot = s.firing?.has(p.index) ?? false;
      strokeAim(ctx, x, y, p.aim[0], -p.aim[1], r, r + (hot ? 22 : 11) * Math.hypot(p.aim[0], p.aim[1]), colour, hot);
    }
    if (s.flat) {
      // Flat style: a small solid dot with a dark hairline, no wedge or
      // nose; the label only for the hovered or selected player.
      ctx.fillStyle = colour;
      ctx.strokeStyle = "rgba(0, 0, 0, 0.85)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(x, y, picked ? r * 1.4 : r, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      if (!s.mini && info && picked) drawLabel(ctx, pal, x, y, r, info.name);
      continue;
    }
    if (!s.mini) {
      ctx.fillStyle = colour;
      ctx.globalAlpha = (off ? 0.35 : 1) * 0.28;
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.arc(x, y, r * 3.2, a - 0.45, a + 0.45);
      ctx.closePath();
      ctx.fill();
      ctx.globalAlpha = off ? 0.35 : 1;
    }
    ctx.fillStyle = colour;
    ctx.strokeStyle = "#fff";
    ctx.lineWidth = s.mini ? 1.25 : 2.5;
    ctx.beginPath();
    ctx.arc(x, y, s.highlight === p.index ? r * 1.35 : r, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    // A nose so facing reads even on the minimap.
    ctx.beginPath();
    ctx.moveTo(x + Math.cos(a) * (r + (s.mini ? 3 : 6)), y + Math.sin(a) * (r + (s.mini ? 3 : 6)));
    ctx.lineTo(x + Math.cos(a + 0.6) * r, y + Math.sin(a + 0.6) * r);
    ctx.lineTo(x + Math.cos(a - 0.6) * r, y + Math.sin(a - 0.6) * r);
    ctx.closePath();
    ctx.fill();
    if (!s.mini && info) drawLabel(ctx, pal, x, y, r, info.name);
  }
  ctx.globalAlpha = 1;
}

/** A player's name in a small dark pill above their dot. */
function drawLabel(ctx: CanvasRenderingContext2D, pal: MapPalette, x: number, y: number, r: number, name: string) {
  ctx.font = "600 12px " + pal.font;
  const tw = ctx.measureText(name).width;
  const ly = y - r - 10;
  ctx.fillStyle = pal.labelBg;
  roundRect(ctx, x - tw / 2 - 6, ly - 9, tw + 12, 18, 9);
  ctx.fill();
  ctx.fillStyle = pal.label;
  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.fillText(name, x, ly + 0.5);
}

function tracePoly(ctx: CanvasRenderingContext2D, poly: [number, number][], sx: (x: number) => number, sy: (y: number) => number) {
  ctx.beginPath();
  poly.forEach(([x, y], i) => (i ? ctx.lineTo(sx(x), sy(y)) : ctx.moveTo(sx(x), sy(y))));
  ctx.closePath();
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

export function niceStep(raw: number): number {
  if (!isFinite(raw) || raw <= 0) return 0;
  const p = 10 ** Math.floor(Math.log10(raw));
  const m = raw / p;
  return (m < 1.5 ? 1 : m < 3.5 ? 2 : m < 7.5 ? 5 : 10) * p;
}
