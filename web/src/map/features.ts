// A map's fixed features, from its glb's `asset.extras.features`:
// teleporters (with the links between them) and gravity lifts, in game
// coordinates. The 3D view and the 2D maps draw them their own way. Pure,
// and without three.js: the 2D map reads the extras from the head of the
// glb itself, never the meshes.

type V3 = [number, number, number];

export type TeleporterRole = "sender" | "receiver" | "two_way";

export interface Teleporter { id: string; x: number; y: number; z: number; facing: number; role: TeleporterRole; channel: number | null }
export interface TeleporterLink { from: string; to: string }
export interface Lift {
  id: string; x: number; y: number; z: number;
  /** Which way it pushes (unit), how far, and how wide. */
  dir: V3; length: number; radius: number;
}
export interface MapFeatures { teleporters: Teleporter[]; links: TeleporterLink[]; lifts: Lift[] }

export const MAX_FEATURES = 256;

const isV3 = (v: unknown): v is V3 => Array.isArray(v) && v.length === 3 && v.every((n) => typeof n === "number" && Number.isFinite(n));
const num = (v: unknown, fallback: number) => (typeof v === "number" && Number.isFinite(v) ? v : fallback);
const idOf = (v: unknown) => (typeof v === "string" && v.length > 0 && v.length <= 64 ? v : null);

/** A glb's `asset.extras` (or its `features`) as features; null when it has none worth drawing. */
export function parseFeatures(extras: unknown): MapFeatures | null {
  const raw = ((extras as { features?: unknown } | null | undefined)?.features ?? extras) as { teleporters?: unknown; links?: unknown; lifts?: unknown } | null | undefined;
  if (!raw || typeof raw !== "object") return null;
  const list = (v: unknown) => (Array.isArray(v) ? v.slice(0, MAX_FEATURES) : []) as Array<Record<string, unknown>>;
  const teleporters: Teleporter[] = [];
  for (const t of list(raw.teleporters)) {
    const id = idOf(t?.id);
    if (!id || !isV3(t.pos) || teleporters.some((q) => q.id === id)) continue;
    const role = t.role === "sender" || t.role === "receiver" ? t.role : "two_way";
    teleporters.push({ id, x: t.pos[0], y: t.pos[1], z: t.pos[2], facing: num(t.facing, 0), role, channel: typeof t.channel === "number" && Number.isFinite(t.channel) ? t.channel : null });
  }
  const known = new Set(teleporters.map((t) => t.id));
  const links: TeleporterLink[] = [];
  for (const l of list(raw.links)) {
    const from = idOf(l?.from), to = idOf(l?.to);
    if (from && to && from !== to && known.has(from) && known.has(to) && !links.some((q) => q.from === from && q.to === to)) links.push({ from, to });
  }
  const lifts: Lift[] = [];
  for (const l of list(raw.lifts)) {
    const id = idOf(l?.id);
    if (!id || !isV3(l.pos)) continue;
    const d = isV3(l.dir) ? l.dir : [0, 0, 1] as V3, len = Math.hypot(d[0], d[1], d[2]);
    lifts.push({
      id, x: l.pos[0], y: l.pos[1], z: l.pos[2],
      dir: len > 1e-6 ? [d[0] / len, d[1] / len, d[2] / len] : [0, 0, 1],
      length: Math.max(0.5, Math.min(200, num(l.length, 4))), radius: Math.max(0.2, Math.min(20, num(l.radius, 0.8))),
    });
  }
  return teleporters.length || lifts.length ? { teleporters, links, lifts } : null;
}

export interface LinkEnds { from: Teleporter; to: Teleporter; /** Goes both ways: no arrowhead needed at one end only. */ both: boolean }

/** The links as pairs of teleporters, a there-and-back pair folded into one. */
export function linkEnds(f: MapFeatures): LinkEnds[] {
  const by = new Map(f.teleporters.map((t) => [t.id, t]));
  const out: LinkEnds[] = [];
  for (const l of f.links) {
    const from = by.get(l.from), to = by.get(l.to);
    if (!from || !to) continue;
    const back = out.find((q) => q.from === to && q.to === from);
    if (back) { back.both = true; continue; }
    out.push({ from, to, both: from.role === "two_way" && to.role === "two_way" });
  }
  return out;
}

/** A link's path through the air: `n` points from one teleporter to the
 *  other, rising in the middle by a fifth of the distance (at least 1). */
export function linkArc(a: { x: number; y: number; z: number }, b: { x: number; y: number; z: number }, n = 24): V3[] {
  const rise = Math.max(1, Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z) * 0.2);
  return Array.from({ length: n }, (_, i) => {
    const t = i / (n - 1);
    return [a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t + rise * 4 * t * (1 - t)] as V3;
  });
}

/** Where a lift lets go. */
export function liftEnd(l: Lift): V3 {
  return [l.x + l.dir[0] * l.length, l.y + l.dir[1] * l.length, l.z + l.dir[2] * l.length];
}

export const TELEPORTER_COLOUR: Record<TeleporterRole, string> = { sender: "#ffb347", receiver: "#62b6ff", two_way: "#63e6c4" };
export const LIFT_COLOUR = "#9c8cff";

// ---- reading a glb's extras without its meshes ----

/** How much of a glb's head has to be read to have its JSON: null when it
 *  isn't a glb, else the byte the JSON chunk ends at. */
export function glbJsonEnd(head: Uint8Array): number | null {
  if (head.length < 20) return null;
  const dv = new DataView(head.buffer, head.byteOffset, head.byteLength);
  if (dv.getUint32(0, true) !== 0x46546c67 || dv.getUint32(16, true) !== 0x4e4f534a) return null;
  return 20 + dv.getUint32(12, true);
}

/** A glb's `asset.extras` from its first bytes; null when they aren't a glb's or aren't all there. */
export function glbExtras(bytes: Uint8Array): unknown {
  const end = glbJsonEnd(bytes);
  if (end === null || bytes.length < end) return null;
  try {
    return (JSON.parse(new TextDecoder().decode(bytes.subarray(20, end))) as { asset?: { extras?: unknown } }).asset?.extras ?? null;
  } catch {
    return null;
  }
}

/** Fetches a glb only as far as its JSON and returns its features. The
 *  rest of the file (the meshes) is left on the server. */
export async function fetchFeatures(url: string, get: typeof fetch = fetch): Promise<MapFeatures | null> {
  try {
    const res = await get(url);
    if (!res.ok || !res.body) return null;
    const reader = res.body.getReader();
    let have = new Uint8Array(0), end: number | null = null;
    // A level's JSON is a few hundred kB at most; give up on anything silly.
    while (have.length < (end ?? 20) && have.length < 32 * 1024 * 1024) {
      const { done, value } = await reader.read();
      if (done) break;
      const next = new Uint8Array(have.length + value.length);
      next.set(have); next.set(value, have.length);
      have = next;
      if (end === null && have.length >= 20) { end = glbJsonEnd(have); if (end === null) break; }
    }
    void reader.cancel().catch(() => {});
    return parseFeatures(glbExtras(have));
  } catch {
    return null;
  }
}
