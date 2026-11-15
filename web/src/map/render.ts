// Draws one top-down map frame onto a 2D canvas. World x is right, world y
// is up (so canvas y is flipped), z is height.

import type { MapOutline } from "../api/types";
import type { SampledPlayer } from "../state/frames";
import type { MapPalette } from "./palette";

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
    ctx.lineWidth = s.mini ? 1.5 : 2.5;
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(x - mr, y - mr); ctx.lineTo(x + mr, y + mr);
    ctx.moveTo(x + mr, y - mr); ctx.lineTo(x - mr, y + mr);
    ctx.stroke();
  }
  ctx.globalAlpha = 1;

  // Players.
  const r = s.mini ? 3.5 : 8;
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
    if (!s.mini && info) {
      ctx.font = "600 12px " + pal.font;
      const tw = ctx.measureText(info.name).width;
      const ly = y - r - 10;
      ctx.fillStyle = pal.labelBg;
      roundRect(ctx, x - tw / 2 - 6, ly - 9, tw + 12, 18, 9);
      ctx.fill();
      ctx.fillStyle = pal.label;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(info.name, x, ly + 0.5);
    }
  }
  ctx.globalAlpha = 1;
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
