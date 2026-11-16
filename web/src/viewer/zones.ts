// Which of a level glb's top parts are shown. A campaign level's structure
// comes as one node per zone (`bsp_<n>`); the game has one loaded at a time
// and says which in its frames, so the view follows it, unless a zone (or
// all of them) is picked by hand. Zones only ever used by cutscenes are
// marked in the glb and stay hidden unless asked for, and `backdrop`
// geometry (far scenery around the playable space) is hidden unless asked
// for. Pure.

export interface ZoneNode {
  name: string;
  /** Marked in the glb as never loaded during play. */
  cinematic: boolean;
  /** The zone's own name in the glb (`extras.name`: its BSP's tag file
   *  name), when it has one. */
  label?: string;
}

/** Which zone the view shows: the one the frames name (`auto`), every
 *  one (`all`), or one picked by number. */
export type ZonePick = "auto" | "all" | number;

export interface ZoneOptions {
  /** The far scenery. */
  backdrop?: boolean;
  /** Zones only cutscenes use: shown in `all`, and pickable. */
  cinematic?: boolean;
  pick?: ZonePick;
}

/** The zone a node is: 3 for `bsp_3`; null for anything else. */
export function bspNumber(name: string): number | null {
  const m = /^bsp_?(\d+)$/i.exec(name);
  return m ? Number(m[1]) : null;
}

/** Far scenery: `backdrop`, `backdrop_<n>`, and `bsp_<n>_backdrop` as
 *  earlier extractions named it. */
export const isBackdrop = (name: string) => /^(backdrop|sky(box)?)(_\d+)?$/i.test(name) || /^bsp_?\d+_backdrop$/i.test(name);

/** A zone's decals (`decals`, `decals_<n>`) or effects (`effects`, `effects_<n>`). */
export function layerNode(name: string): "decals" | "effects" | null {
  const m = /^(decals|effects)(_\d+)?$/i.exec(name);
  return m ? (m[1]!.toLowerCase() as "decals" | "effects") : null;
}

/** A readable name for a zone: its BSP's tag name with the separators
 *  made spaces and the first letter raised (`earthcity_2` is
 *  "Earthcity 2"); a name that only says it is a BSP (`bsp_01`), or none,
 *  is "Zone <n>" (counting from 1). */
export function zoneLabel(bsp: number, label?: string | null): string {
  const words = (label ?? "").replace(/[_\-.]+/g, " ").replace(/\s+/g, " ").trim();
  if (!words || /^(bsp|zone|structure)( ?\d+)?$/i.test(words)) return `Zone ${bsp + 1}`;
  return words[0]!.toUpperCase() + words.slice(1);
}

export interface ZoneEntry {
  bsp: number;
  label: string;
  cinematic: boolean;
}

/** The zones to offer in a picker, in BSP order: the playable ones, and
 *  the cutscene ones too when asked. */
export function zoneList(nodes: readonly ZoneNode[], opts: { cinematic?: boolean } = {}): ZoneEntry[] {
  const out: ZoneEntry[] = [];
  for (const n of nodes) {
    const bsp = bspNumber(n.name);
    if (bsp === null || (n.cinematic && !opts.cinematic) || out.some((e) => e.bsp === bsp)) continue;
    out.push({ bsp, label: zoneLabel(bsp, n.label), cinematic: n.cinematic });
  }
  return out.sort((a, b) => a.bsp - b.bsp);
}

/** Whether a glb node's extras mark it cinematic-only. The extractor's
 *  key isn't settled, so the likely spellings all count. */
export function isCinematic(extras: unknown, name = "", assetExtras?: unknown): boolean {
  const e = (extras ?? {}) as Record<string, unknown>;
  if (e.cinematic === true || e.cinematic_only === true || e.cinematicOnly === true) return true;
  const n = bspNumber(name), list = (assetExtras as { cinematic_bsps?: unknown } | null | undefined)?.cinematic_bsps;
  return n !== null && Array.isArray(list) && list.includes(n);
}

/** For each node, whether to show it.
 *  - Zones: a picked zone the glb has, alone; `all`: every zone (the
 *    cutscene ones too only when asked); otherwise (`auto`) only the active
 *    one when the frames name a zone the glb has, else every zone that
 *    isn't cinematic-only (all of them when cutscene zones are asked for).
 *    If that would leave no zone at all, all of them (better too much than
 *    an empty level).
 *  - Backdrop: only when asked for.
 *  - Anything else (a single `structure`, `machines`): always. */
export function zoneVisibility(nodes: readonly ZoneNode[], active: number | null | undefined, opts: ZoneOptions = {}): boolean[] {
  const zones = nodes.map((n) => bspNumber(n.name));
  const pick = opts.pick ?? "auto";
  const want = typeof pick === "number" && zones.includes(pick) ? pick : pick === "auto" && active != null && zones.includes(active) ? active : null;
  let show = nodes.map((n, i) => {
    const z = zones[i]!;
    if (z === null) return isBackdrop(n.name) ? !!opts.backdrop : true;
    return want !== null ? z === want : !n.cinematic || !!opts.cinematic;
  });
  if (zones.some((z) => z !== null) && !show.some((s, i) => s && zones[i] !== null)) show = show.map((s, i) => (zones[i] !== null ? true : s));
  return show;
}

/** The zone picker's first choice: following the game, and the zone it
 *  has loaded when the frames say. */
export function zoneInPlay(info: { active: number | null; zones: readonly ZoneEntry[] } | null): string {
  const z = info?.active != null ? info.zones.find((e) => e.bsp === info.active) : undefined;
  return z ? `Zone in play: ${z.label}` : "Zone in play";
}

// ---- the zone in play ----

/** A zone's extent, game coordinates (z up). */
export interface ZoneBox {
  bsp: number;
  min: [number, number, number];
  max: [number, number, number];
  cinematic: boolean;
}

/** How long a different zone has to be the one in play before the view
 *  moves to it (ms): a player stepping back and forth over the line where
 *  two zones meet doesn't flick the level between them. */
export const ZONE_DWELL_MS = 800;
/** How far outside its box a point still counts as in a zone (game units):
 *  zones meet in shared corridors, and feet stand on the floor's edge. */
export const ZONE_MARGIN = 1;

type Point = { x: number; y: number; z: number };

const inside = (b: ZoneBox, p: Point, m = ZONE_MARGIN) =>
  p.x >= b.min[0] - m && p.x <= b.max[0] + m && p.y >= b.min[1] - m && p.y <= b.max[1] + m && p.z >= b.min[2] - m && p.z <= b.max[2] + m;

/** What the zone in play is worked out from, at one moment. */
export interface ZoneMoment {
  /** The zone the game says it has loaded (`Frame.bsp`), if it says. */
  reported: number | null;
  /** The player the view is about (followed, else highlighted), alive. */
  focus: Point | null;
  /** Every living player (and nothing else: AI stand in zones not loaded). */
  players: readonly Point[];
}

/** The zone the view should show for a moment, before any dwell: the one
 *  the game has loaded when it is a playable zone and the players are in
 *  it (a cutscene's zone, or one nobody is in, is not where play is); else
 *  the one the focus player is in, else the one most players are in; the
 *  zone already shown wins any tie, so overlapping zones don't swap. Null
 *  when nothing says. */
export function zoneWanted(boxes: readonly ZoneBox[], m: ZoneMoment, current: number | null, cinematic = false): number | null {
  const playable = boxes.filter((b) => !b.cinematic || cinematic);
  if (!playable.length) return null;
  const by = (n: number | null) => (n === null ? undefined : playable.find((b) => b.bsp === n));
  const who = m.focus ? [m.focus] : m.players;
  const reported = by(m.reported);
  if (!who.length) return reported ? reported.bsp : current;
  const count = (b: ZoneBox) => who.filter((p) => inside(b, p)).length;
  if (reported && count(reported) > 0 && count(reported) >= Math.max(...playable.map(count))) return reported.bsp;
  const cur = by(current);
  let best: ZoneBox | null = cur && count(cur) > 0 ? cur : null;
  for (const b of playable) if (count(b) > (best ? count(best) : 0)) best = b;
  if (best) return best.bsp;
  // Nobody is inside any zone (falling, or a sender's odd position): the game's word, else stay.
  return reported ? reported.bsp : current;
}

/** The zone in play over time: `zoneWanted`, held until a new one has
 *  been wanted for `ZONE_DWELL_MS` (at once the first time, and on a seek). */
export class ZoneFollower {
  current: number | null = null;
  private next: { bsp: number | null; since: number } | null = null;

  update(now: number, boxes: readonly ZoneBox[], m: ZoneMoment, opts: { cinematic?: boolean; snap?: boolean } = {}): number | null {
    const want = zoneWanted(boxes, m, this.current, opts.cinematic);
    if (want === this.current) { this.next = null; return this.current; }
    if (this.current === null || opts.snap) { this.current = want; this.next = null; return want; }
    if (!this.next || this.next.bsp !== want) this.next = { bsp: want, since: now };
    if (now - this.next.since >= ZONE_DWELL_MS) { this.current = want; this.next = null; }
    return this.current;
  }

  reset() { this.current = null; this.next = null; }
}
