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
}

export const MODELS_VERSION = 1;
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
  const r = raw as { version?: unknown; bipeds?: unknown; weapons?: unknown };
  if (r.version !== MODELS_VERSION) return null;
  const bipeds = table(r.bipeds), weapons = table(r.weapons);
  return Object.keys(bipeds).length || Object.keys(weapons).length ? { bipeds, weapons } : null;
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

/** A model file's address under the art root. */
export function modelUrl(root: string, file: string): string {
  return root + file.split("/").map(encodeURIComponent).join("/");
}

/** A weapon stem as people read it: `battle_rifle` → "battle rifle". */
export function weaponLabel(weapon: string | null | undefined): string {
  return (weapon ?? "").replace(/_+/g, " ").trim();
}

/** How much bigger than life to draw a model `height` tall seen from
 *  `distance`: life size while it fills enough of the view, then growing
 *  with the distance so it stays findable, never past the map's marker
 *  scale `limit`. */
export function modelScale(distance: number, height: number, limit: number): number {
  const k = (distance * 0.03) / Math.max(height, 1e-3);
  return Math.min(Math.max(1, limit), Math.max(1, k));
}

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
