// The stats site's banner: which shots of which maps, and how they are laid
// side by side into one picture (docs/live/live-art.md, "Stats banners").
//
// Only the rules are here. The pictures are drawn from the owner's own
// extracted maps by `bun run banner` (banner/run.ts) and written next to the
// rest of the game art; nothing drawn from the disc is in the repository.
// Everything in this file is pure, so it can be tested.

export type Vec3 = [number, number, number];
export interface Box3 { min: Vec3; max: Vec3 }

/** A placed object where the level puts it (the viewer's `spots()`). */
export interface Spot { kind: string; item: string | null; at: Vec3 }

/** One camera shot of one map. Shots name no coordinates from the disc:
 *  a camera is placed either relative to the level's playable bounds
 *  (`at`, `azimuth`, `elevation`, `distance`) or at one of the level's
 *  placed objects (`spot`, as the viewer keys them: `netgame:12` is the
 *  13th multiplayer item or vehicle spawn), looking at another spot or
 *  across the level. */
export type Shot = BoundsShot | SpotShot;

interface ShotBase {
  id: string;
  /** Map stem, as the art manifest keys it (`map/<stem>/...`). */
  map: string;
  /** Vertical field of view, degrees. */
  fov: number;
}

export interface BoundsShot extends ShotBase {
  /** What the camera looks at: fractions of the bounds along x, y and z. */
  at: Vec3;
  /** Where the camera stands, seen from `at`: degrees from +x (east) towards
   *  +y (north), and up from level. */
  azimuth: number;
  elevation: number;
  /** How far from `at`, as a share of the bounds' half-diagonal. */
  distance: number;
}

export interface SpotShot extends ShotBase {
  spot: string;
  /** What to look at: another spot, or (absent) the bounds' centre at
   *  `level` of their height (default 0.3). */
  toward?: string;
  level?: number;
  /** Game units: the camera stands `back` behind the spot (away from what
   *  it looks at) and `lift` above it, and aims `drop` below the target. */
  back: number;
  lift: number;
  drop?: number;
  /** Then turned `turn` degrees to the left (negative: right). */
  turn?: number;
  /** Players standing in the shot, on the spot's floor. */
  cast?: Cast[];
}

/** A player in a shot, drawn with the server's own model of the species. */
export interface Cast {
  species: string;
  /** Weapon stem (`battle_rifle`), as the frames name it. */
  weapon?: string;
  /** Team colour, `#rrggbb`. */
  colour: string;
  /** Game units ahead of the camera and to its left. */
  ahead: number;
  left: number;
  /** Which way they face: degrees from the camera's line of sight, to the
   *  left (180 faces the camera). */
  face: number;
  crouch?: boolean;
}

/** Where a shot's cast stands, game coordinates and yaw (radians from +x
 *  towards +y), for a camera at `eye` looking at `at` over a floor at
 *  height `floor`. */
export function placeCast(cast: Cast[], eye: Vec3, at: Vec3, floor: number): Array<Cast & { x: number; y: number; z: number; yaw: number }> {
  let fx = at[0] - eye[0], fy = at[1] - eye[1];
  const len = Math.hypot(fx, fy) || 1;
  fx /= len; fy /= len;
  const heading = Math.atan2(fy, fx);
  return cast.map((c) => ({
    ...c,
    x: eye[0] + fx * c.ahead - fy * c.left,
    y: eye[1] + fy * c.ahead + fx * c.left,
    z: floor,
    yaw: heading + (c.face * Math.PI) / 180,
  }));
}

export const isSpotShot = (s: Shot): s is SpotShot => "spot" in s;

/** A picture made of shots, side by side. */
export interface Composition {
  /** Manifest key (`banner/<id>`) and file (`banner/<id>.jpg`). */
  id: string;
  width: number;
  height: number;
  /** Shot ids, left to right. */
  shots: string[];
  /** How far each seam leans, as a share of the height (positive: the top
   *  is further right than the bottom). */
  slant: number;
  /** How wide each seam's blend is, as a share of a panel's width. */
  feather: number;
  /** Where the wordmark sits: the picture is darkened under it. */
  mark: "right" | "none";
}

// Chosen from contact sheets (`bun run banner --sheet`) of a retail disc.
const RED = "#b8392e", BLUE = "#2f5fb3";
export const SHOTS: Shot[] = [
  { id: "containment", map: "containment", spot: "netgame:64", back: 3, lift: 0.8, drop: -2, fov: 50 },
  { id: "coagulation", map: "coagulation", spot: "netgame:50", back: 3, lift: 1.4, fov: 52 },
  { id: "zanzibar", map: "zanzibar", spot: "netgame:66", back: 3, lift: 0.9, fov: 50,
    cast: [
      { species: "spartan", weapon: "battle_rifle", colour: RED, ahead: 2.2, left: -0.3, face: -30 },
      { species: "elite", weapon: "plasma_rifle", colour: BLUE, ahead: 6, left: 1.2, face: 200 },
    ] },
  { id: "beavercreek", map: "beavercreek", spot: "netgame:39", back: 3, lift: 1.6, fov: 52 },
  { id: "turf", map: "turf", spot: "netgame:46", back: 3, lift: 1.6, fov: 52 },
  { id: "burial_mounds", map: "burial_mounds", spot: "netgame:37", back: 3, lift: 1.4, fov: 36 },
  { id: "zanzibar-beach", map: "zanzibar", spot: "netgame:65", back: 3, lift: 0.8, fov: 40,
    cast: [{ species: "spartan", weapon: "sniper_rifle", colour: BLUE, ahead: 2.6, left: 0.5, face: 20 }] },
  { id: "coagulation-wide", map: "coagulation", spot: "netgame:19", back: 3, lift: 1.6, fov: 36 },
  { id: "headlong", map: "headlong", at: [0.5, 0.5, 0.3], azimuth: 270, elevation: 20, distance: 0.7, fov: 36 },
];

export const COMPOSITIONS: Composition[] = [
  // The home page's banner: 902 x 180 CSS pixels, drawn at twice that.
  { id: "home", width: 1804, height: 360, shots: ["containment", "coagulation", "zanzibar", "beavercreek", "turf"], slant: 0.18, feather: 0.22, mark: "right" },
  // The smaller strip over the other stats pages: 902 x 96.
  { id: "strip", width: 1804, height: 192, shots: ["burial_mounds", "zanzibar-beach", "coagulation-wide", "headlong"], slant: 0.18, feather: 0.25, mark: "right" },
];

/** A seam between two panels: a slanted line from (`top`, 0) to
 *  (`bottom`, height), blended over `feather` pixels across it. */
export interface Seam { top: number; bottom: number; feather: number }

/** Where one panel is drawn: its seams (null at the picture's own edges)
 *  and the box its shot is fitted into. */
export interface Panel { shot: string; left: Seam | null; right: Seam | null; box: { x: number; y: number; width: number; height: number } }

/** The panels of a composition, left to right. The seams split the width
 *  evenly at mid-height and lean by `slant`; each panel's box reaches past
 *  its seams far enough to cover the lean and the blend. */
export function layout(c: Composition): Panel[] {
  const n = c.shots.length;
  if (n === 0) return [];
  const step = c.width / n;
  const lean = c.slant * c.height;
  const feather = Math.max(1, c.feather * step);
  const seams: Seam[] = [];
  for (let i = 1; i < n; i++) {
    const mid = i * step;
    seams.push({ top: mid + lean / 2, bottom: mid - lean / 2, feather });
  }
  const reach = lean / 2 + feather / 2;
  return c.shots.map((shot, i) => {
    const left = i > 0 ? seams[i - 1]! : null;
    const right = i < n - 1 ? seams[i]! : null;
    const x0 = left ? Math.max(0, Math.floor(i * step - reach)) : 0;
    const x1 = right ? Math.min(c.width, Math.ceil((i + 1) * step + reach)) : c.width;
    return { shot, left, right, box: { x: x0, y: 0, width: x1 - x0, height: c.height } };
  });
}

/** How much of a panel shows at (x, y), 0 to 1: full inside its seams,
 *  fading across each seam's blend. The left seam fades the panel in, the
 *  right one out, so neighbours' weights sum to one along a seam. */
export function panelWeight(p: Panel, x: number, y: number, height: number): number {
  const along = (s: Seam) => s.bottom + (s.top - s.bottom) * (1 - y / height);
  const ramp = (d: number, f: number) => Math.min(1, Math.max(0, d / f + 0.5));
  let w = 1;
  if (p.left) w *= ramp(x - along(p.left), p.left.feather);
  if (p.right) w *= 1 - ramp(x - along(p.right), p.right.feather);
  return w;
}

/** Where a shot's camera stands and what it looks at, in game coordinates,
 *  for a level with these bounds and placed objects; null when the shot's
 *  spot is not in the level. */
export function camera(s: Shot, b: Box3, spots: ReadonlyMap<string, { at: Vec3 }> = new Map()): { eye: Vec3; at: Vec3 } | null {
  const size: Vec3 = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
  const inBox = (f: Vec3): Vec3 => [0, 1, 2].map((k) => b.min[k]! + size[k]! * f[k]!) as Vec3;
  if (isSpotShot(s)) {
    const from = spots.get(s.spot)?.at;
    if (!from) return null;
    let target: Vec3;
    if (s.toward) {
      const to = spots.get(s.toward)?.at;
      if (!to) return null;
      target = [...to];
    } else {
      target = inBox([0.5, 0.5, s.level ?? 0.3]);
    }
    let dx = target[0] - from[0], dy = target[1] - from[1];
    const len = Math.hypot(dx, dy) || 1;
    dx /= len; dy /= len;
    const eye: Vec3 = [from[0] - dx * s.back, from[1] - dy * s.back, from[2] + s.lift];
    const t = ((s.turn ?? 0) * Math.PI) / 180;
    const rx = dx * Math.cos(t) - dy * Math.sin(t), ry = dx * Math.sin(t) + dy * Math.cos(t);
    const reach = Math.max(1, Math.hypot(target[0] - eye[0], target[1] - eye[1]));
    return { eye, at: [eye[0] + rx * reach, eye[1] + ry * reach, target[2] - (s.drop ?? 0)] };
  }
  const at = inBox(s.at);
  const r = Math.max(1, Math.hypot(...size) / 2) * s.distance;
  const az = (s.azimuth * Math.PI) / 180, el = (s.elevation * Math.PI) / 180;
  const eye: Vec3 = [at[0] + Math.cos(el) * Math.cos(az) * r, at[1] + Math.cos(el) * Math.sin(az) * r, at[2] + Math.sin(el) * r];
  return { eye, at };
}

/** Checks a plan: every composition's shots exist, ids are unique, sizes
 *  are sane. Returns the problems (empty when fine). */
export function check(shots: Shot[], comps: Composition[]): string[] {
  const out: string[] = [];
  const ids = new Set<string>();
  for (const s of shots) {
    if (ids.has(s.id)) out.push(`shot ${s.id} is listed twice`);
    ids.add(s.id);
    if (!/^[a-z0-9_]+$/.test(s.map)) out.push(`shot ${s.id}: map ${s.map} is not a stem`);
    if (!(s.fov > 5 && s.fov < 120)) out.push(`shot ${s.id}: fov out of range`);
    if (isSpotShot(s)) {
      if (![s.spot, s.toward ?? "x:0"].every((k) => /^[a-z]+:\d+$/.test(k))) out.push(`shot ${s.id}: a spot is not <kind>:<index>`);
    } else {
      if (s.at.some((f) => f < 0 || f > 1)) out.push(`shot ${s.id}: at is outside the bounds`);
      if (!(s.distance > 0)) out.push(`shot ${s.id}: distance out of range`);
    }
  }
  const comp = new Set<string>();
  for (const c of comps) {
    if (comp.has(c.id)) out.push(`composition ${c.id} is listed twice`);
    comp.add(c.id);
    if (!/^[a-z0-9_-]+$/.test(c.id)) out.push(`composition ${c.id}: not a file name`);
    if (!(c.width >= 64 && c.height >= 32)) out.push(`composition ${c.id}: too small`);
    if (!c.shots.length) out.push(`composition ${c.id}: no shots`);
    for (const id of c.shots) if (!ids.has(id)) out.push(`composition ${c.id}: no shot ${id}`);
  }
  return out;
}

/** The maps a plan needs. */
export function mapsOf(shots: Shot[], comps: Composition[]): string[] {
  const used = new Set(comps.flatMap((c) => c.shots));
  return [...new Set(shots.filter((s) => used.has(s.id)).map((s) => s.map))];
}

/** Shots to choose new ones from (`bun run banner --sheet`): from each of
 *  up to `count` of the level's multiplayer spawns (vehicles first, then
 *  spread out), a little behind and above it, looking across the level.
 *  Ids are `<map>-<kind>-<index>`. */
export function candidates(map: string, spots: ReadonlyMap<string, Spot>, count = 16): Shot[] {
  const all = [...spots].filter(([, s]) => s.kind === "netgame" || s.kind === "vehi");
  const isVehicle = ([, s]: [string, Spot]) => s.kind === "vehi" || s.item === "vehi";
  const picked = all.filter(isVehicle).slice(0, count);
  const rest = all.filter((e) => !isVehicle(e));
  // The rest in farthest-point order, so the picks spread over the level.
  const dist = (a: Vec3, b: Vec3) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
  while (picked.length < count && rest.length) {
    let best = 0, bestD = -1;
    rest.forEach(([, s], i) => {
      const d = picked.length ? Math.min(...picked.map(([, p]) => dist(p.at, s.at))) : 0;
      if (d > bestD) { bestD = d; best = i; }
    });
    picked.push(rest.splice(best, 1)[0]!);
  }
  return picked.map(([k]) => ({ id: `${map}-${k.replace(":", "-")}`, map, spot: k, back: 3, lift: 1.6, fov: 56 }));
}
