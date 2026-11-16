// Halo 2's fifteen skulls (gameplay modifiers). Games report which were on
// as `skulls`, sorted unique indices 0..14, absent when none were. This is
// the name and effect table, and the helpers for the `?skulls=` query.

export type SkullTier = "gold" | "silver";

export interface SkullDef {
  index: number;
  name: string;
  /** One line on what it does, for tooltips. */
  effect: string;
  /** Gold skulls raise the score multiplier above 1.00x, silver ones do not. */
  tier: SkullTier;
  /** Plays in multiplayer too (the rest are campaign only). */
  multiplayer?: boolean;
}

export const SKULLS: readonly SkullDef[] = [
  { index: 0, tier: "silver", name: "Envy", effect: "your flashlight is swapped for active camouflage" },
  { index: 1, tier: "silver", name: "Grunt Birthday Party", effect: "a Grunt headshot bursts into confetti" },
  { index: 2, tier: "gold", name: "Assassins", effect: "every enemy wears active camouflage" },
  { index: 3, tier: "gold", name: "Thunderstorm", effect: "enemies are promoted to higher ranks" },
  { index: 4, tier: "gold", name: "Famine", effect: "weapons hold far less ammo; in multiplayer: weapons placed on the map start with half ammo", multiplayer: true },
  { index: 5, tier: "silver", name: "IWHBYD", effect: "rare combat lines play far more often; full name \"I Would Have Been Your Daddy\"" },
  { index: 6, tier: "gold", name: "Blind", effect: "your HUD and first-person weapon are hidden", multiplayer: true },
  { index: 7, tier: "silver", name: "Ghost", effect: "enemies no longer flinch" },
  { index: 8, tier: "gold", name: "Black Eye", effect: "shields recharge only on a melee hit" },
  { index: 9, tier: "gold", name: "Catch", effect: "enemies throw far more grenades" },
  { index: 10, tier: "silver", name: "Sputnik", effect: "explosions and melee send things flying" },
  { index: 11, tier: "gold", name: "Iron", effect: "a death sends you back to the last checkpoint" },
  { index: 12, tier: "gold", name: "Mythic", effect: "enemies have twice the health" },
  { index: 13, tier: "gold", name: "Angry", effect: "enemies fire faster" },
  { index: 14, tier: "silver", name: "Bandana", effect: "unlimited ammo and grenades" },
];

export const SKULL_COUNT = SKULLS.length;
/** Highest index the query and the reducer accept; indices past the table
 *  (custom skulls a later lane adds) are kept and shown generically. */
export const MAX_SKULL = 63;

export const skullName = (i: number): string => SKULLS[i]?.name ?? `Skull ${i}`;
export const skullEffect = (i: number): string => SKULLS[i]?.effect ?? "";
/** Unknown indices are silver. */
export const skullTier = (i: number): SkullTier => SKULLS[i]?.tier ?? "silver";
export const tierName = (t: SkullTier): string => (t === "gold" ? "Gold skull" : "Silver skull");
/** "Name: effect (Gold skull)", the tooltip and accessible label. */
export const skullTitle = (i: number): string =>
  SKULLS[i] ? `${skullName(i)}: ${skullEffect(i)} (${tierName(skullTier(i))})` : `${skullName(i)} (${tierName("silver")})`;

/** Indices as sent: whole numbers 0..63, sorted, unique; anything else dropped. */
export function normalizeSkulls(v: unknown): number[] {
  if (!Array.isArray(v)) return [];
  const out = new Set<number>();
  for (const n of v) if (typeof n === "number" && Number.isInteger(n) && n >= 0 && n <= MAX_SKULL) out.add(n);
  return [...out].sort((a, b) => a - b);
}

/** Display order: gold first, then silver, engine order within each. */
export function tierOrder(skulls: Iterable<number>): number[] {
  const l = [...new Set(skulls)].sort((a, b) => a - b);
  return [...l.filter((i) => skullTier(i) === "gold"), ...l.filter((i) => skullTier(i) !== "gold")];
}

/** The table's skulls of one tier, in engine order. */
export const skullsOfTier = (t: SkullTier): SkullDef[] => SKULLS.filter((s) => s.tier === t);

/** A `?skulls=10,14` value: junk and repeats dropped, sorted. */
export function parseSkullsParam(s: string | null | undefined): number[] {
  if (!s) return [];
  return normalizeSkulls(s.split(",").map((p) => (/^\s*\d+\s*$/.test(p) ? Number(p) : NaN)));
}

/** The `?skulls=` value for some indices, or undefined for none (leave the param out). */
export function formatSkullsParam(skulls: Iterable<number>): string | undefined {
  const l = normalizeSkulls([...skulls]);
  return l.length ? l.join(",") : undefined;
}

/** Whether the skulls include every wanted one (what the server's filter keeps). */
export function hasAllSkulls(have: readonly number[] | null | undefined, wanted: readonly number[]): boolean {
  if (!wanted.length) return true;
  const set = new Set(have ?? []);
  return wanted.every((i) => set.has(i));
}

/** Keeps the items that had all the wanted skulls on. */
export function filterBySkulls<T>(items: readonly T[], wanted: readonly number[], skullsOf: (t: T) => readonly number[] | null | undefined): T[] {
  return wanted.length ? items.filter((t) => hasAllSkulls(skullsOf(t), wanted)) : [...items];
}

/** Toggles one index in a selection, keeping it sorted. */
export function toggleSkull(sel: readonly number[], i: number): number[] {
  return normalizeSkulls(sel.includes(i) ? sel.filter((s) => s !== i) : [...sel, i]);
}

export interface SkullCount { index: number; games: number }

/** The skulls across some games, each with how many games had it on, gold first, then silver. */
export function countSkulls<T>(items: Iterable<T>, skullsOf: (t: T) => readonly number[] | null | undefined): SkullCount[] {
  const n = new Map<number, number>();
  for (const t of items) for (const i of new Set(skullsOf(t) ?? [])) n.set(i, (n.get(i) ?? 0) + 1);
  return tierOrder(n.keys()).map((index) => ({ index, games: n.get(index)! }));
}
