// Draws one top-down map frame onto a 2D canvas. World x is right, world y
// is up (so canvas y is flipped), z is height.

import type { MapOutline } from "../api/types";
import type { SampledPlayer, SampledVehicle } from "../state/frames";
import { actorColour, vehicleBox } from "../viewer/models";
import type { MapPalette } from "./palette";
import type { LiveShot } from "./shots";
import { entityIndex, hitColour, type LiveHit } from "./events";
import { projectileColour, type ProjectileTrail } from "../state/projectile-trails";
import { LIFT_COLOUR, TELEPORTER_COLOUR, linkEnds, type MapFeatures, type TeleporterRole } from "./features";
import { bodyColour, vitalsNumber, type Vitals } from "./vitals";

export interface Bounds { minX: number; minY: number; maxX: number; maxY: number }

/** `bot`: a Clanker bot, labelled with a BOT mark (docs/game/bots.md §6). */
export interface RosterEntry { name: string; colour: string; bot?: boolean }

/** A player's map label: the name, and a BOT mark for a bot. */
export function rosterLabel(info: RosterEntry): string {
  return info.bot ? `${info.name} · BOT` : info.name;
}

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
  /** The map's teleporters, their links and its lifts. */
  features?: MapFeatures | null;
  /** Reduced motion: nothing floats or flashes. */
  still?: boolean;
  damage?: MapDamage;
  /** Recorded projectile paths up to the frame's render tick. */
  projectileTrails?: ProjectileTrail[];
}

type Px = readonly [number, number];

export interface MapDamage {
  vitals: ReadonlyMap<number, Vitals>;
  hits: readonly LiveHit[];
  numbers: boolean;
  followed: number | null;
}

export interface DamageDot {
  at: Px;
  radius: number;
  colour: string;
  vehicle?: boolean;
}

const ARC_START = Math.PI * 0.75;
const ARC_LENGTH = Math.PI * 1.5;

/** The two vitality arcs leave a quarter of the circle clear at the bottom. */
export function drawVitals(ctx: CanvasRenderingContext2D, x: number, y: number, r: number, vitals: Vitals) {
  ctx.lineCap = "round";
  const bar = (radius: number, width: number, amount: number, edge: number | null, colour: string) => {
    const full = Math.min(1, amount);
    const chip = Math.min(1, edge ?? full);
    ctx.lineWidth = width;
    const arc = (start: number, end: number, ink: string) => {
      if (start >= end) return;
      ctx.strokeStyle = ink;
      ctx.beginPath();
      ctx.arc(x, y, radius, ARC_START + ARC_LENGTH * start, ARC_START + ARC_LENGTH * end);
      ctx.stroke();
    };
    arc(0, 1, "rgba(0, 0, 0, 0.6)");
    arc(0, full, colour);
    arc(full, chip, "rgba(223, 243, 255, 0.8)");
  };
  bar(r + 3, 1.5, vitals.shield, vitals.shieldChip, "#8fd4ff");
  if (vitals.body < 0.995) bar(r + 1, 1, vitals.body, vitals.bodyChip, bodyColour(vitals.body));
}

export function damageLabel(name: string, vitals: Vitals | undefined, showNumbers: boolean): string {
  const number = showNumbers ? vitalsNumber(vitals) : "";
  return number ? name + " " + number : name;
}

/** Live hits leave a directional wedge. Focused exchanges and kills also
 *  connect to the attacker, and a kill gets the map's knockout cross. */
export function drawHits(
  ctx: CanvasRenderingContext2D, hits: readonly LiveHit[], at: (x: number, y: number, z: number) => Px,
  dots: ReadonlyMap<string, DamageDot>, roster: ReadonlyMap<number, RosterEntry>,
  followed: number | null, highlight: number | null, still = false,
): void {
  const focused = (id: number | null) => id !== null && (id === followed || id === highlight);
  for (const hit of hits) {
    const attacker = hit.fromId ? entityIndex(hit.fromId) : null;
    const victim = hit.toId ? entityIndex(hit.toId) : null;
    const dot = hit.toId ? dots.get(hit.toId) : undefined;
    const killColour = (attacker === null ? undefined : roster.get(attacker)?.colour)
      ?? (hit.fromId ? dots.get(hit.fromId)?.colour : undefined);
    drawHit(ctx, hit.from ? at(hit.from.x, hit.from.y, hit.from.z) : null, at(hit.x, hit.y, hit.z), hit,
      dot, focused(attacker) || focused(victim), killColour, still);
  }
}

function drawHit(ctx: CanvasRenderingContext2D, from: Px | null, to: Px, h: LiveHit,
  dot: DamageDot | undefined, focused: boolean, killColour: string | undefined, still: boolean,
) {
  const kill = h.kill && !h.vehicle && !dot?.vehicle;
  const colour = kill ? killColour ?? hitColour(h) : hitColour({ kind: h.kind, kill: false });
  const [x, y] = kill ? to : dot?.at ?? to;
  ctx.lineCap = "round";
  if (from && !h.vehicle && !dot?.vehicle && (kill || focused) && h.line > 0) {
    ctx.globalAlpha = still ? 0.5 : h.line;
    for (const [style, w] of [["rgba(0, 0, 0, 0.6)", kill ? 4.5 : 3.5], [colour, kill ? 2.5 : 1.5]] as const) {
      ctx.strokeStyle = style; ctx.lineWidth = w;
      ctx.beginPath(); ctx.moveTo(from[0], from[1]); ctx.lineTo(x, y); ctx.stroke();
    }
  }

  const directionAlpha = h.direction;
  const [vx, vy] = dot?.at ?? to;
  if (from && directionAlpha > 0 && Math.hypot(from[0] - vx, from[1] - vy) > 1e-6) {
    const a = Math.atan2(from[1] - vy, from[0] - vx);
    const r = (dot?.radius ?? 5) + 3;
    const ux = Math.cos(a), uy = Math.sin(a);
    const bx = vx + r * ux, by = vy + r * uy;
    ctx.globalAlpha = directionAlpha;
    ctx.fillStyle = hitColour({ kind: h.kind, kill: false });
    ctx.strokeStyle = "rgba(0, 0, 0, 0.8)";
    ctx.lineWidth = 1;
    ctx.lineJoin = "round";
    ctx.beginPath();
    ctx.moveTo(bx - 3 * uy, by + 3 * ux);
    ctx.lineTo(bx + 6 * ux, by + 6 * uy);
    ctx.lineTo(bx + 3 * uy, by - 3 * ux);
    ctx.closePath();
    ctx.fill();
    ctx.stroke();
  }
  if (kill) {
    ctx.globalAlpha = h.mark;
    for (const [style, w] of [["rgba(0, 0, 0, 0.8)", 4], [colour, 2]] as const) {
      ctx.strokeStyle = style; ctx.lineWidth = w;
      ctx.beginPath();
      ctx.moveTo(x - 6, y - 6); ctx.lineTo(x + 6, y + 6);
      ctx.moveTo(x + 6, y - 6); ctx.lineTo(x - 6, y + 6);
      ctx.stroke();
    }
  }
  ctx.globalAlpha = 1;
}

/** Recorded positions projected into a 2D map, with a dot at the head. */
export function drawProjectileTrail(ctx: CanvasRenderingContext2D, pts: readonly Px[], colour: string) {
  if (!pts.length) return;
  ctx.lineCap = "round";
  if (pts.length > 1) {
    ctx.setLineDash([3, 4]);
    for (const [style, w] of [["rgba(0, 0, 0, 0.6)", 3], [colour, 1.5]] as const) {
      ctx.strokeStyle = style; ctx.lineWidth = w;
      ctx.beginPath();
      pts.forEach(([px, py], i) => (i ? ctx.lineTo(px, py) : ctx.moveTo(px, py)));
      ctx.stroke();
    }
    ctx.setLineDash([]);
  }
  const [x, y] = pts[pts.length - 1]!;
  ctx.fillStyle = colour; ctx.strokeStyle = "rgba(0, 0, 0, 0.85)"; ctx.lineWidth = 1;
  ctx.beginPath(); ctx.arc(x, y, 3, 0, Math.PI * 2); ctx.fill(); ctx.stroke();
}

/** A teleporter on a 2D map: a ring; a sender has a dot in it, a receiver is open, a two-way has both rings. */
export function drawTeleporter(ctx: CanvasRenderingContext2D, x: number, y: number, role: TeleporterRole, r = 7) {
  const colour = TELEPORTER_COLOUR[role];
  for (const [style, w] of [["rgba(0, 0, 0, 0.7)", 4.5], [colour, 2]] as const) {
    ctx.strokeStyle = style; ctx.lineWidth = w;
    ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.stroke();
    if (role === "two_way") { ctx.lineWidth = w * 0.6; ctx.beginPath(); ctx.arc(x, y, r * 0.5, 0, Math.PI * 2); ctx.stroke(); }
  }
  if (role === "sender") { ctx.fillStyle = colour; ctx.beginPath(); ctx.arc(x, y, r * 0.3, 0, Math.PI * 2); ctx.fill(); }
}

/** A teleporter link on a 2D map: a dashed curve from one ring to the
 *  other, bowed to one side so a there-and-back pair doesn't hide the
 *  floor between them, with an arrowhead where it arrives (both ends for
 *  a two-way link). */
export function drawLink(ctx: CanvasRenderingContext2D, from: Px, to: Px, both: boolean, r = 7) {
  const dx = to[0] - from[0], dy = to[1] - from[1], len = Math.hypot(dx, dy);
  if (len < r * 2.5) return;
  const cx = (from[0] + to[0]) / 2 - dy * 0.18, cy = (from[1] + to[1]) / 2 + dx * 0.18;
  // Start and end on the rings, not under them.
  const edge = (p: Px): Px => { const ex = cx - p[0], ey = cy - p[1], el = Math.hypot(ex, ey) || 1; return [p[0] + (ex / el) * (r + 2), p[1] + (ey / el) * (r + 2)]; };
  const a = edge(from), b = edge(to);
  ctx.lineCap = "round";
  for (const [style, w] of [["rgba(0, 0, 0, 0.55)", 3.5], ["rgba(214, 238, 255, 0.9)", 1.5]] as const) {
    ctx.strokeStyle = style; ctx.lineWidth = w;
    ctx.setLineDash([5, 5]);
    ctx.beginPath(); ctx.moveTo(a[0], a[1]); ctx.quadraticCurveTo(cx, cy, b[0], b[1]); ctx.stroke();
    ctx.setLineDash([]);
    for (const tip of both ? [b, a] : [b]) {
      const ang = Math.atan2(tip[1] - cy, tip[0] - cx);
      ctx.beginPath();
      ctx.moveTo(tip[0] - Math.cos(ang - 0.5) * 7, tip[1] - Math.sin(ang - 0.5) * 7);
      ctx.lineTo(tip[0], tip[1]);
      ctx.lineTo(tip[0] - Math.cos(ang + 0.5) * 7, tip[1] - Math.sin(ang + 0.5) * 7);
      ctx.stroke();
    }
  }
}

/** A gravity lift on a 2D map: a translucent disc `r` pixels across its
 *  radius with chevrons pointing where it sends you. `to` is where it lets
 *  go, on the canvas; a lift going straight up from above has its chevrons
 *  pointing up the page. */
export function drawLift(ctx: CanvasRenderingContext2D, at: Px, to: Px, r: number) {
  const [x, y] = at;
  r = Math.max(6, r);
  ctx.fillStyle = "rgba(156, 140, 255, 0.28)";
  ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();
  let dx = to[0] - x, dy = to[1] - y;
  const len = Math.hypot(dx, dy), across = len > r * 0.75;
  if (!across) { dx = 0; dy = -1; } else { dx /= len; dy /= len; }
  for (const [style, w] of [["rgba(0, 0, 0, 0.7)", 4], [LIFT_COLOUR, 1.75]] as const) {
    ctx.strokeStyle = style; ctx.lineWidth = w; ctx.lineCap = "round"; ctx.lineJoin = "round";
    ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.stroke();
    if (across) { ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(to[0], to[1]); ctx.stroke(); }
    // Two chevrons, one behind the other, along the push.
    const s = Math.min(r * 0.55, 6);
    for (const k of across ? [len] : [-s * 0.6, s * 0.6]) {
      const px = x + dx * (across ? k : -k), py = y + dy * (across ? k : -k);
      ctx.beginPath();
      ctx.moveTo(px - dx * s - dy * s, py - dy * s + dx * s);
      ctx.lineTo(px, py);
      ctx.lineTo(px - dx * s + dy * s, py - dy * s - dx * s);
      ctx.stroke();
    }
  }
}

/** All of a map's features, given how to put a game position on the canvas and how many pixels a game unit is. */
export function drawFeatures(ctx: CanvasRenderingContext2D, f: MapFeatures, at: (x: number, y: number, z: number) => Px, unit: number) {
  for (const l of f.lifts) drawLift(ctx, at(l.x, l.y, l.z), at(l.x + l.dir[0] * l.length, l.y + l.dir[1] * l.length, l.z + l.dir[2] * l.length), l.radius * unit);
  for (const l of linkEnds(f)) drawLink(ctx, at(l.from.x, l.from.y, l.from.z), at(l.to.x, l.to.y, l.to.z), l.both);
  for (const t of f.teleporters) { const [x, y] = at(t.x, t.y, t.z); drawTeleporter(ctx, x, y, t.role); }
}

/** The colour and look of a vehicle nobody is in: grey and faint. */
export const EMPTY_VEHICLE_COLOUR = "#9aa3ab";
export const EMPTY_VEHICLE_ALPHA = 0.5;

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
  const damage = s.mini ? undefined : s.damage;
  const dots = damage?.hits.length ? new Map<string, DamageDot>() : undefined;

  // The map's own furniture, under everything that moves.
  if (!s.mini && s.features) drawFeatures(ctx, s.features, (x, y) => [sx(x), sy(y)], v.scale);

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
      // A turret is drawn as the vehicle it is mounted on.
      if (veh.parent) continue;
      const [len, wid] = vehicleBox(veh.kind);
      const k = Math.max(v.scale, 9 / wid);
      const x = sx(veh.x), y = sy(veh.y);
      const colour = veh.occupied ? vehicleColour(veh.id, s.players, s.actors, s.roster) : EMPTY_VEHICLE_COLOUR;
      if (!veh.occupied) ctx.globalAlpha = EMPTY_VEHICLE_ALPHA;
      drawVehicle(ctx, x, y, Math.atan2(-veh.forward[1], veh.forward[0]), len * k, wid * k, colour);
      dots?.set(veh.id, { at: [x, y], radius: wid * k / 2, colour, vehicle: true });
      ctx.globalAlpha = 1;
    }
  }
  for (const a of s.actors ?? []) {
    if (!a.alive || a.vehicle) continue;
    const x = sx(a.x), y = sy(a.y), radius = s.mini ? 1.5 : 2.5;
    const colour = actorColour(a.actor?.team);
    drawActor(ctx, x, y, radius, colour);
    if (a.actor) dots?.set(a.actor.id, { at: [x, y], radius, colour });
    const vitals = damage?.vitals.get(a.index);
    if (vitals) drawVitals(ctx, x, y, radius, vitals);
  }

  // Players.
  const sorted = s.players.slice().sort((a, b) => a.z - b.z);
  for (const p of sorted) {
    const vitals = damage?.vitals.get(p.index);
    if (!p.alive) {
      const info = s.roster.get(p.index);
      if (vitals && vitals.chipFrom !== null) {
        const x = sx(p.x), y = sy(p.y);
        drawVitals(ctx, x, y, r, vitals);
        if (info && damage?.numbers) drawLabel(ctx, pal, x, y, r + 4, damageLabel(rosterLabel(info), vitals, true));
      }
      continue;
    }
    const info = s.roster.get(p.index);
    const colour = info?.colour ?? "#999";
    const x = sx(p.x), y = sy(p.y);
    const off = s.outline && s.layer !== null && layerOf(s.outline, p.z) !== s.layer;
    ctx.globalAlpha = off ? 0.35 : 1;
    // Facing: a soft wedge ahead of the dot. Yaw 0 = +x, counter-clockwise.
    const a = -p.yaw;
    const picked = s.highlight === p.index || s.hover === p.index;
    const radius = s.flat && picked ? r * 1.4 : s.highlight === p.index ? r * 1.35 : r;
    const labelRadius = vitals ? radius + 4 : r;
    if (dots) dots.set("p" + p.index, { at: [x, y], radius, colour });
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
      ctx.arc(x, y, radius, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      if (vitals) drawVitals(ctx, x, y, radius, vitals);
      if (!s.mini && info && (picked || (damage?.numbers && vitalsNumber(vitals)))) {
        drawLabel(ctx, pal, x, y, labelRadius, damageLabel(rosterLabel(info), vitals, !!damage?.numbers));
      }
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
    ctx.arc(x, y, radius, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    // A nose so facing reads even on the minimap.
    ctx.beginPath();
    ctx.moveTo(x + Math.cos(a) * (r + (s.mini ? 3 : 6)), y + Math.sin(a) * (r + (s.mini ? 3 : 6)));
    ctx.lineTo(x + Math.cos(a + 0.6) * r, y + Math.sin(a + 0.6) * r);
    ctx.lineTo(x + Math.cos(a - 0.6) * r, y + Math.sin(a - 0.6) * r);
    ctx.closePath();
    ctx.fill();
    if (vitals) drawVitals(ctx, x, y, radius, vitals);
    if (!s.mini && info) drawLabel(ctx, pal, x, y, labelRadius, damageLabel(rosterLabel(info), vitals, !!damage?.numbers));
  }
  ctx.globalAlpha = 1;

  // Over everyone: recorded projectile paths, then damage.
  if (!s.mini) {
    for (const p of s.projectileTrails ?? []) drawProjectileTrail(ctx, p.points.map(([x, y]) => [sx(x), sy(y)] as const), projectileColour(p.kind));
    if (damage && dots) drawHits(ctx, damage.hits, (x, y) => [sx(x), sy(y)], dots, s.roster, damage.followed, s.highlight ?? null, s.still);
  }
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
