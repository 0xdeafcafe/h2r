// Player models, the parts that need no three.js: reading the art
// manifest's `models` table, choosing a model (or the capsule) for a
// player, and the team-colour maths. Pure, so it can be tested and can sit
// in the main bundle; viewer/players3d.ts does the loading and drawing.
//
// The contract: bipeds are glbs posed once, standing on the origin facing
// +X, with empty nodes `weapon` (where the held weapon's origin goes) and
// `head` (the eye point); weapons have an empty `muzzle` node. A biped's
// COLOR_0 says what takes the player's colour: R is the weight of the
// primary colour and G of the secondary at that vertex.

export interface PlayerModels {
  /** glb file by species id, relative to the manifest. */
  bipeds: Record<string, string>;
  /** glb file by weapon stem, relative to the manifest. */
  weapons: Record<string, string>;
  /** glb file by vehicle stem, relative to the manifest. */
  vehicles: Record<string, string>;
  /** glb file by tag stem for other big movers (machines not in the map glb, a scarab). */
  objects: Record<string, string>;
  /** The animation class of each weapon stem (version 2): which clip set a biped holding it plays. */
  classes: Record<string, string>;
  /** The table's version: 1 is static models, 2 skinned and animated. */
  version: number;
  /** Whether any biped lists animation clips. */
  animated: boolean;
  /** Changes with each extraction (the manifest's `source`); see `modelUrl`. */
  stamp?: string;
  /** Per weapon stem, what its shots are and how fast they fly, from the
   *  weapon's projectile on the disc (viewer/projectiles.ts); absent when
   *  the art has none. */
  ballistics?: Record<string, { kind?: string; speed?: number; final_speed?: number; rate?: number }>;
  /** Per vehicle stem, its seats in the game's order (a unit's `seat` is
   *  an index into it): the seat's animation mode and the glb's `seat_`
   *  node for it (extractor 22; absent before). */
  seats?: Record<string, SeatInfo[]>;
}

export interface SeatInfo { label: string | null; node: string | null }

export const MODELS_VERSION = 2;
/** The species drawn for one this server has no model of. */
export const DEFAULT_SPECIES = "spartan";

const safeFile = (file: unknown): file is string =>
  typeof file === "string" && !!file && !/^([a-z][a-z0-9+.-]*:|\/|\\)/i.test(file) && !file.split(/[\\/]/).includes("..");

function table(raw: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (!raw || typeof raw !== "object") return out;
  for (const [id, v] of Object.entries(raw as Record<string, unknown>)) {
    const glb = (v as { glb?: unknown } | null)?.glb;
    if (/^[a-z0-9_]{1,32}$/.test(id) && safeFile(glb)) out[id] = glb;
  }
  return out;
}

/** A manifest's `models` table; null when it has none this client can
 *  read (then every player is a capsule). */
export function parseModels(raw: unknown): PlayerModels | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as { version?: unknown; bipeds?: unknown; weapons?: unknown; vehicles?: unknown; objects?: unknown };
  // Version 2 adds animation clips and weapon classes; its files still read as version 1's.
  if (r.version !== 1 && r.version !== MODELS_VERSION) return null;
  const bipeds = table(r.bipeds), weapons = table(r.weapons), vehicles = table(r.vehicles), objects = table(r.objects);
  const classes: Record<string, string> = {};
  if (r.weapons && typeof r.weapons === "object") {
    for (const [id, v] of Object.entries(r.weapons as Record<string, unknown>)) {
      const c = (v as { class?: unknown } | null)?.class;
      if (/^[a-z0-9_]{1,32}$/.test(id) && typeof c === "string" && /^[a-z0-9_]{1,32}$/.test(c)) classes[id] = c;
    }
  }
  const ballistics: NonNullable<PlayerModels["ballistics"]> = {};
  if (r.weapons && typeof r.weapons === "object") {
    for (const [id, v] of Object.entries(r.weapons as Record<string, unknown>)) {
      const b = (v as { ballistics?: unknown } | null)?.ballistics as Record<string, unknown> | undefined;
      if (!/^[a-z0-9_]{1,32}$/.test(id) || !b || typeof b !== "object") continue;
      const num = (x: unknown) => (typeof x === "number" && Number.isFinite(x) && x > 0 ? x : undefined);
      const kind = typeof b.kind === "string" && /^[a-z]{1,16}$/.test(b.kind) ? b.kind : undefined;
      ballistics[id] = { kind, speed: num(b.speed), final_speed: num(b.final_speed), rate: num(b.rate) };
    }
  }
  const seats: Record<string, SeatInfo[]> = {};
  const rawSeats = (raw as { seats?: unknown }).seats;
  if (rawSeats && typeof rawSeats === "object") {
    for (const [id, list] of Object.entries(rawSeats as Record<string, unknown>)) {
      if (!/^[a-z0-9_]{1,32}$/.test(id) || !Array.isArray(list)) continue;
      seats[id] = list.slice(0, 32).map((e) => {
        const { label, node } = (e ?? {}) as { label?: unknown; node?: unknown };
        return {
          label: typeof label === "string" && /^[a-z0-9_]{1,48}$/.test(label) ? label : null,
          node: typeof node === "string" && /^seat_[a-z0-9_]{1,48}$/.test(node) ? node : null,
        };
      });
    }
  }
  const animated = !!r.bipeds && typeof r.bipeds === "object" &&
    Object.values(r.bipeds as Record<string, unknown>).some((b) => Array.isArray((b as { clips?: unknown } | null)?.clips) && (b as { clips: unknown[] }).clips.length > 0);
  return [bipeds, weapons, vehicles, objects].some((t) => Object.keys(t).length) ? { bipeds, weapons, vehicles, objects, classes, version: r.version, animated, ...(Object.keys(ballistics).length ? { ballistics } : {}), ...(Object.keys(seats).length ? { seats } : {}) } : null;
}

/** The glb to draw a player with: their species', else the spartan's.
 *  Null means the capsule: no models, or neither of those. */
export function bipedFile(models: PlayerModels | null | undefined, species: string | null | undefined): string | null {
  if (!models) return null;
  return (species ? models.bipeds[species] : undefined) ?? models.bipeds[DEFAULT_SPECIES] ?? null;
}

/** The glb of a held weapon, or null when the server has no model of it
 *  (the player is then drawn empty-handed, the weapon still named). */
export function weaponFile(models: PlayerModels | null | undefined, weapon: string | null | undefined): string | null {
  return (models && weapon ? models.weapons[weapon] : undefined) ?? null;
}

/** The glb of a vehicle, or null when the server has no model of it (a
 *  box is drawn then). */
export function vehicleFile(models: PlayerModels | null | undefined, kind: string | null | undefined): string | null {
  return (models && kind ? models.vehicles[kind] : undefined) ?? null;
}

// ---- seats ----

/** Words a seat's animation mode ends in, and the seat marker word for each. */
const SEAT_ROLES: Array<[RegExp, string]> = [[/_d$/, "driver"], [/_g$/, "gunner"], [/_p(_[a-z0-9]+)?$/, "passenger"]];

/** Which of a vehicle's `seat_` nodes a rider sits at: the seat table's
 *  node for their seat index (or for their animation's mode), else the one
 *  their mode's role names (`warthog_d` the driver's, `_g` the gunner's,
 *  `_p` a passenger's, `scorpion_p_lf` `seat_passenger_lf` first). With
 *  neither (an older sender), a rider of a mounted turret is its gunner
 *  and the vehicle's first rider its driver. Null: nowhere to put them
 *  (the vehicle stands for them). */
export function seatNodeFor(
  nodes: readonly string[],
  rider: { seat?: number | null; mode?: string | null; turret?: boolean; first?: boolean },
  table?: readonly SeatInfo[] | null,
): string | null {
  const have = (n: string | null | undefined): n is string => !!n && nodes.includes(n);
  if (table && rider.seat != null && have(table[rider.seat]?.node)) return table[rider.seat]!.node;
  if (table && rider.mode) { const e = table.find((s) => s.label === rider.mode && have(s.node)); if (e) return e.node; }
  let role: string | null = null, side = "";
  if (rider.mode) {
    for (const [re, r] of SEAT_ROLES) {
      const m = re.exec(rider.mode);
      if (m) { role = r; side = m[1] ?? ""; break; }
    }
  } else if (rider.seat == null) {
    role = rider.turret ? "gunner" : rider.first ? "driver" : null;
  }
  if (!role) return null;
  if (side && have(`seat_${role}${side}`)) return `seat_${role}${side}`;
  if (have(`seat_${role}`)) return `seat_${role}`;
  return nodes.find((n) => n.startsWith(`seat_${role}`)) ?? null;
}

/** The seat pose a rider without a mode of their own takes: the clip
 *  state of the vehicle's driver or gunner mode (`seat_warthog_d`), as the
 *  graphs name a vehicle's seats after it. */
export function seatState(kind: string, node: string): string {
  const role = node.startsWith("seat_gunner") ? "g" : node.startsWith("seat_passenger") ? "p" : "d";
  return `seat_${kind}_${role}`;
}

// ---- actors ----

/** The ring colour of an AI-controlled biped, by the side telemetry says
 *  it is on. An unknown side (a raw team index) is a neutral grey-blue. */
export function actorColour(team: string | null | undefined): string {
  switch (team) {
    case "covenant": case "prophet": return "#a66be0";
    case "human": case "player": return "#58b368";
    case "flood": return "#c9a66b";
    case "sentinel": return "#9aa3ab";
    default: return "#7f8fa3";
  }
}

/** How dark a vehicle nobody is in is drawn, beside one in use. */
export const EMPTY_VEHICLE_SHADE = 0.5;

/** A vehicle's box when there is no model of it: [length, width, height]
 *  in game units, by kind, so a tank isn't drawn the size of a bike. */
export function vehicleBox(kind: string): [number, number, number] {
  if (/scorpion|wraith|tank/.test(kind)) return [3.2, 2.2, 1.1];
  if (/ghost|mongoose|bike/.test(kind)) return [1.5, 0.8, 0.5];
  if (/banshee|hornet|phantom|pelican/.test(kind)) return [2.2, 2.6, 0.6];
  return [2.2, 1.3, 0.8];
}

/** A vehicle's turn as three unit axes in the glTF frame the scene uses,
 *  from the frame's forward and up (game coordinates): x is forward, y up,
 *  z right. The up is squared off against the forward, so a pair that isn't
 *  quite perpendicular still makes a clean turn; one that is parallel falls
 *  back to the world's up. */
export function vehicleBasis(forward: Rgb, up: Rgb): { x: Rgb; y: Rgb; z: Rgb } {
  const gl = (v: Rgb): Rgb => [v[0], v[2], -v[1]];
  const unit = (v: Rgb): Rgb | null => { const l = Math.hypot(v[0], v[1], v[2]); return l > 1e-6 ? [v[0] / l, v[1] / l, v[2] / l] : null; };
  const cross = (a: Rgb, b: Rgb): Rgb => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  const x = unit(gl(forward)) ?? [1, 0, 0];
  let z = unit(cross(x, gl(up))) ?? unit(cross(x, [0, 1, 0])) ?? unit(cross(x, [1, 0, 0]))!;
  const y = cross(z, x);
  return { x, y, z };
}

/** A model file's address under the art root. `stamp` (the manifest's
 *  `source`, which changes with every extraction) is added as a query, so a
 *  browser never shows a model it cached from an older extraction. */
export function modelUrl(root: string, file: string, stamp?: string | null): string {
  const url = root + file.split("/").map(encodeURIComponent).join("/");
  return stamp ? `${url}?v=${encodeURIComponent(stamp.slice(0, 16))}` : url;
}

/** Whether the server's player models can animate: version 2 tables carry
 *  clips. Version 1 art (extracted by an older `h2`) is static; the
 *  viewer says so, since the cure is extracting the art again. */
export function modelsAnimate(models: PlayerModels | null | undefined): boolean {
  return !!models && models.version >= 2 && models.animated;
}

/** A weapon stem as people read it: `battle_rifle` → "battle rifle". */
export function weaponLabel(weapon: string | null | undefined): string {
  return (weapon ?? "").replace(/_+/g, " ").trim();
}

/** Models are drawn at the game's own size: the glbs are in world units
 *  (a Spartan about 0.7 tall, a Warthog about 2 long), the same as the
 *  map, and nothing rescales them, however far away the camera is. */
export const MODEL_SCALE = 1;

// ---- finding the contract's nodes ----

interface Named<T> { name: string; children: T[] }

/** The first node called `name` under `root` (itself included), depth
 *  first. Loaders may add a suffix to a repeated name (`weapon_1`), and
 *  exporters vary the case, so those match too, after an exact one. */
export function findNode<T extends Named<T>>(root: T, name: string): T | null {
  const walk = (n: T, ok: (s: string) => boolean): T | null => {
    if (ok(n.name)) return n;
    for (const c of n.children) { const hit = walk(c, ok); if (hit) return hit; }
    return null;
  };
  const loose = new RegExp(`^${name}(_\\d+)?$`, "i");
  return walk(root, (s) => s === name) ?? walk(root, (s) => loose.test(s));
}

// ---- colour ----

export type Rgb = [number, number, number];

/** How dark the secondary colour is beside the primary (linear light). */
export const SECONDARY_SHADE = 0.38;
/** How much of the player's colour a model without tint regions takes. */
export const BODY_TINT = 0.78;

const clamp01 = (v: number) => (v > 1 ? 1 : v > 0 ? v : 0);

/** The secondary change colour: a darker shade of the player's. */
export function secondaryOf(primary: Rgb): Rgb {
  return [primary[0] * SECONDARY_SHADE, primary[1] * SECONDARY_SHADE, primary[2] * SECONDARY_SHADE];
}

/** How a vertex's colour is shared out: [primary, secondary, untinted].
 *  The weights are held to 0..1, and scaled down together when they
 *  add up to more than 1. */
export function tintWeights(r: number, g: number): [number, number, number] {
  let p = clamp01(r), s = clamp01(g);
  const sum = p + s;
  if (sum > 1) { p /= sum; s /= sum; }
  return [p, s, Math.max(0, 1 - p - s)];
}

/** The colour a vertex's texture is multiplied by: white where nothing
 *  changes, the player's colour where the primary weight is 1, the darker
 *  shade where the secondary is. Linear light. */
export function tintAt(r: number, g: number, primary: Rgb, secondary: Rgb = secondaryOf(primary)): Rgb {
  const [p, s, base] = tintWeights(r, g);
  return [base + primary[0] * p + secondary[0] * s, base + primary[1] * p + secondary[1] * s, base + primary[2] * p + secondary[2] * s];
}

/** The per-vertex multipliers for a whole mesh, three floats a vertex.
 *  `weight(i)` gives vertex i's [R, G] change-colour weights. */
export function tintColours(count: number, weight: (i: number) => readonly [number, number], primary: Rgb, secondary: Rgb = secondaryOf(primary)): Float32Array {
  const out = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    const [r, g] = weight(i);
    const c = tintAt(r, g, primary, secondary);
    out[i * 3] = c[0]; out[i * 3 + 1] = c[1]; out[i * 3 + 2] = c[2];
  }
  return out;
}

/** The multiplier for a model with no tint regions: mostly the player's
 *  colour, a little white kept so its own shading still shows. */
export function bodyTint(primary: Rgb): Rgb {
  const k = BODY_TINT;
  return [1 - k + primary[0] * k, 1 - k + primary[1] * k, 1 - k + primary[2] * k];
}
