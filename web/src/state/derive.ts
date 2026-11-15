// Stats the service doesn't serve yet, derived client-side from game
// events: places, medals (multi-kills, sprees and the like), weapon
// breakdowns, service-record totals, per-player history and "friends"
// (people you've played with). Everything here is pure and marked as
// derived in the UI.

import type { GameState, Kill, LiveState, Standing } from "./reducer";
import { gamesFor, isWinner, standings, teamStandings } from "./reducer";
import type { Xuid } from "../api/types";

/** Ticks per second, and the window that chains kills into a multi-kill. */
export const TICKS = 30;
export const MULTI_WINDOW_TICKS = 4 * TICKS;
export const PAYBACK_WINDOW_TICKS = 10 * TICKS;

// ---------- medals ----------

export type MedalId =
  | "opening-act" | "last-word" | "payback"
  | "twofer" | "hat-trick" | "full-house" | "encore"
  | "hot-streak" | "wildfire" | "untouchable";

export interface MedalDef { id: MedalId; name: string; blurb: string; tier: 0 | 1 | 2 | 3 }

/** Original medal names and descriptions. */
export const MEDALS: MedalDef[] = [
  { id: "opening-act", name: "Opening Act", blurb: "First knockout of the game", tier: 1 },
  { id: "last-word", name: "Last Word", blurb: "Final knockout of the game", tier: 1 },
  { id: "payback", name: "Payback", blurb: "Knocked out whoever just got you, within 10 s", tier: 0 },
  { id: "twofer", name: "Twofer", blurb: "2 knockouts, each within 4 s of the last", tier: 0 },
  { id: "hat-trick", name: "Hat Trick", blurb: "3 knockouts in a chain", tier: 1 },
  { id: "full-house", name: "Full House", blurb: "4 knockouts in a chain", tier: 2 },
  { id: "encore", name: "Encore", blurb: "5 or more knockouts in a chain", tier: 3 },
  { id: "hot-streak", name: "Hot Streak", blurb: "5 knockouts without being knocked out", tier: 1 },
  { id: "wildfire", name: "Wildfire", blurb: "10 knockouts without being knocked out", tier: 2 },
  { id: "untouchable", name: "Untouchable", blurb: "15 knockouts without being knocked out", tier: 3 },
];
export const MEDAL_BY_ID = Object.fromEntries(MEDALS.map((m) => [m.id, m])) as Record<MedalId, MedalDef>;

export interface MedalAward { medal: MedalId; slot: number; tick: number }

const MULTI: MedalId[] = ["twofer", "hat-trick", "full-house", "encore"];
const SPREES: Array<[number, MedalId]> = [[5, "hot-streak"], [10, "wildfire"], [15, "untouchable"]];

/** A kill that counts for the killer: not a suicide, betrayal or world death. */
export function scoringKill(k: Kill): boolean {
  return k.kind === "kill" && k.killer !== null && k.killer !== k.victim;
}

/** Medals for every slot in one game, from its kills in tick order. A
 *  multi-kill chain awards each step (2, 3, 4, then 5+ once per kill). */
export function gameMedals(kills: readonly Kill[]): MedalAward[] {
  const sorted = [...kills].sort((a, b) => a.tick - b.tick || a.position - b.position);
  const out: MedalAward[] = [];
  const chain = new Map<number, { count: number; last: number }>();
  const spree = new Map<number, number>();
  const lastKilledBy = new Map<number, { by: number; tick: number }>();
  const scoring = sorted.filter(scoringKill);
  const first = scoring[0];
  const last = scoring[scoring.length - 1];
  for (const k of sorted) {
    // A death (any kind) ends the victim's chain and spree.
    spree.set(k.victim, 0);
    chain.delete(k.victim);
    if (!scoringKill(k)) continue;
    const s = k.killer!;
    if (k === first) out.push({ medal: "opening-act", slot: s, tick: k.tick });
    const c = chain.get(s);
    const count = c && k.tick - c.last <= MULTI_WINDOW_TICKS ? c.count + 1 : 1;
    chain.set(s, { count, last: k.tick });
    if (count >= 2) out.push({ medal: MULTI[Math.min(count, 5) - 2]!, slot: s, tick: k.tick });
    const n = (spree.get(s) ?? 0) + 1;
    spree.set(s, n);
    for (const [at, id] of SPREES) if (n === at) out.push({ medal: id, slot: s, tick: k.tick });
    const lk = lastKilledBy.get(s);
    if (lk && lk.by === k.victim && k.tick - lk.tick <= PAYBACK_WINDOW_TICKS) {
      out.push({ medal: "payback", slot: s, tick: k.tick });
      lastKilledBy.delete(s);
    }
    lastKilledBy.set(k.victim, { by: s, tick: k.tick });
    if (k === last && sorted.length > 1) out.push({ medal: "last-word", slot: s, tick: k.tick });
  }
  return out;
}

/** Longest run of scoring kills without dying, per slot. */
export function bestSprees(kills: readonly Kill[]): Map<number, number> {
  const sorted = [...kills].sort((a, b) => a.tick - b.tick || a.position - b.position);
  const cur = new Map<number, number>();
  const best = new Map<number, number>();
  for (const k of sorted) {
    cur.set(k.victim, 0);
    if (!scoringKill(k)) continue;
    const n = (cur.get(k.killer!) ?? 0) + 1;
    cur.set(k.killer!, n);
    best.set(k.killer!, Math.max(best.get(k.killer!) ?? 0, n));
  }
  return best;
}

export function countMedals(awards: readonly MedalAward[], slots?: Set<number>): Map<MedalId, number> {
  const m = new Map<MedalId, number>();
  for (const a of awards) if (!slots || slots.has(a.slot)) m.set(a.medal, (m.get(a.medal) ?? 0) + 1);
  return m;
}

// ---------- weapons ----------

export interface WeaponRow { weapon: string; kills: number; share: number }

/** Kills by weapon for the given slots (or everyone), most used first. */
export function weaponBreakdown(kills: readonly Kill[], slots?: Set<number>): WeaponRow[] {
  const m = new Map<string, number>();
  let total = 0;
  for (const k of kills) {
    if (!scoringKill(k) || (slots && !slots.has(k.killer!))) continue;
    const w = k.weapon ?? "unknown";
    m.set(w, (m.get(w) ?? 0) + 1);
    total++;
  }
  return [...m.entries()]
    .map(([weapon, n]) => ({ weapon, kills: n, share: total ? n / total : 0 }))
    .sort((a, b) => b.kills - a.kills || a.weapon.localeCompare(b.weapon));
}

/** Deaths by weapon for the given slots: what knocked them out, most often first. */
export function weaponsAgainst(kills: readonly Kill[], slots: Set<number>): WeaponRow[] {
  const m = new Map<string, number>();
  let total = 0;
  for (const k of kills) {
    if (!scoringKill(k) || !slots.has(k.victim)) continue;
    const w = k.weapon ?? "unknown";
    m.set(w, (m.get(w) ?? 0) + 1);
    total++;
  }
  return [...m.entries()]
    .map(([weapon, n]) => ({ weapon, kills: n, share: total ? n / total : 0 }))
    .sort((a, b) => b.kills - a.kills || a.weapon.localeCompare(b.weapon));
}

export function mergeWeapons(rows: WeaponRow[][]): WeaponRow[] {
  const m = new Map<string, number>();
  for (const rs of rows) for (const r of rs) m.set(r.weapon, (m.get(r.weapon) ?? 0) + r.kills);
  const total = [...m.values()].reduce((a, b) => a + b, 0);
  return [...m.entries()]
    .map(([weapon, kills]) => ({ weapon, kills, share: total ? kills / total : 0 }))
    .sort((a, b) => b.kills - a.kills || a.weapon.localeCompare(b.weapon));
}

// ---------- places ----------

/** 1-based place per slot. FFA ranks players by score (ties share a
 *  place); team games give every member their team's place. */
export function places(g: GameState): Map<number, number> {
  const out = new Map<number, number>();
  if (g.team_play) {
    const ts = teamStandings(g);
    // Ties between teams share the better place.
    let prev: { score: number; place: number } | null = null;
    ts.forEach((t, i) => {
      const place = prev && prev.score === t.score ? prev.place : i + 1;
      for (const p of t.players) out.set(p.index, place);
      prev = { score: t.score, place };
    });
    return out;
  }
  const st = standings(g);
  let prev: { s: Standing; place: number } | null = null;
  st.forEach((s, i) => {
    const place = prev && prev.s.score.score === s.score.score ? prev.place : i + 1;
    out.set(s.index, place);
    prev = { s, place };
  });
  return out;
}

export function ordinal(n: number): string {
  const s = n % 100 >= 11 && n % 100 <= 13 ? "th" : ["th", "st", "nd", "rd"][n % 10] ?? "th";
  return n + s;
}

// ---------- per-player ----------

export interface HistoryRow {
  game: GameState;
  slots: number[];
  team: number | null;
  place: number | null;
  score: number;
  kills: number;
  deaths: number;
  assists: number;
  won: boolean;
  bestSpree: number;
}

export function slotsFor(g: GameState, xuid: Xuid): number[] {
  return Object.values(g.players).filter((p) => p.xuid === xuid).map((p) => p.index);
}

/** One row per game the account played in, newest first. */
export function gameHistory(s: LiveState, xuid: Xuid): HistoryRow[] {
  return gamesFor(s, xuid).map((g) => {
    const slots = slotsFor(g, xuid);
    const pl = places(g);
    const sprees = bestSprees(g.kills);
    const row: HistoryRow = {
      game: g, slots, team: null, place: null, score: 0, kills: 0, deaths: 0, assists: 0, won: false, bestSpree: 0,
    };
    for (const i of slots) {
      const p = g.players[i]!;
      const sc = g.scores[i];
      if (g.team_play) row.team = p.team;
      row.place = pl.get(i) ?? row.place;
      if (sc) { row.score += sc.score; row.kills += sc.kills; row.deaths += sc.deaths; row.assists += sc.assists; }
      if (g.status === "ended" && isWinner(g, p)) row.won = true;
      row.bestSpree = Math.max(row.bestSpree, sprees.get(i) ?? 0);
    }
    return row;
  });
}

export interface ServiceRecord {
  games: number;
  completed: number;
  wins: number;
  kills: number;
  deaths: number;
  assists: number;
  kd: number | null;
  bestSpree: number;
  firstPlayed: string | null;
  lastPlayed: string | null;
  exp: number;
  rank: string;
  weapons: WeaponRow[];
  /** What knocked this player out, by weapon (`kills` counts their deaths). */
  weaponsAgainst: WeaponRow[];
  medals: Map<MedalId, number>;
  /** Kills the totals were derived from (snapshots carry at most 50 per game). */
  killsSeen: number;
}

/** Original rank ladder, by derived EXP (one point per game completed,
 *  two more per win). */
export const RANKS: Array<[number, string]> = [
  [0, "Newcomer"], [5, "Regular"], [15, "Contender"], [30, "Veteran"], [60, "Ace"], [120, "Headliner"], [250, "Legend"],
];

export function rankFor(exp: number): string {
  return RANKS[rankIndex(exp)]![1];
}

/** Position on the rank ladder, 0-based. */
export function rankIndex(exp: number): number {
  let r = 0;
  RANKS.forEach(([at], i) => { if (exp >= at) r = i; });
  return r;
}

/** EXP the next rank needs, or null at the top of the ladder. */
export function nextRankAt(exp: number): number | null {
  return RANKS[rankIndex(exp) + 1]?.[0] ?? null;
}

/** Totals for an account; `only` narrows them to some of its games (a map, a playlist). */
export function serviceRecord(s: LiveState, xuid: Xuid, only?: (g: GameState) => boolean): ServiceRecord {
  const all = gameHistory(s, xuid);
  const hist = only ? all.filter((h) => only(h.game)) : all;
  const rec: ServiceRecord = {
    games: hist.length, completed: 0, wins: 0, kills: 0, deaths: 0, assists: 0, kd: null, bestSpree: 0,
    firstPlayed: null, lastPlayed: null, exp: 0, rank: RANKS[0]![1], weapons: [], weaponsAgainst: [], medals: new Map(), killsSeen: 0,
  };
  const weapons: WeaponRow[][] = [];
  const against: WeaponRow[][] = [];
  for (const h of hist) {
    const g = h.game;
    if (g.status === "ended") rec.completed++;
    if (h.won) rec.wins++;
    rec.kills += h.kills; rec.deaths += h.deaths; rec.assists += h.assists;
    rec.bestSpree = Math.max(rec.bestSpree, h.bestSpree);
    if (!rec.firstPlayed || g.started_at < rec.firstPlayed) rec.firstPlayed = g.started_at;
    if (!rec.lastPlayed || g.started_at > rec.lastPlayed) rec.lastPlayed = g.started_at;
    const slots = new Set(h.slots);
    weapons.push(weaponBreakdown(g.kills, slots));
    against.push(weaponsAgainst(g.kills, slots));
    for (const [m, n] of countMedals(gameMedals(g.kills), slots)) rec.medals.set(m, (rec.medals.get(m) ?? 0) + n);
    rec.killsSeen += g.kills.filter((k) => k.killer !== null && slots.has(k.killer)).length;
  }
  rec.weapons = mergeWeapons(weapons);
  rec.weaponsAgainst = mergeWeapons(against);
  rec.kd = rec.deaths ? rec.kills / rec.deaths : null;
  rec.exp = rec.completed + rec.wins * 2;
  rec.rank = rankFor(rec.exp);
  return rec;
}

export interface Friend { xuid: Xuid; games: number; lastPlayed: string }

/** People an account has shared a game with, most games first. */
export function playedWith(s: LiveState, xuid: Xuid): Friend[] {
  const m = new Map<Xuid, Friend>();
  for (const g of gamesFor(s, xuid)) {
    const seen = new Set<Xuid>();
    for (const p of Object.values(g.players)) {
      if (!p.xuid || p.xuid === xuid || seen.has(p.xuid)) continue;
      seen.add(p.xuid);
      const f = m.get(p.xuid) ?? { xuid: p.xuid, games: 0, lastPlayed: g.started_at };
      f.games++;
      if (g.started_at > f.lastPlayed) f.lastPlayed = g.started_at;
      m.set(p.xuid, f);
    }
  }
  return [...m.values()].sort((a, b) => b.games - a.games || b.lastPlayed.localeCompare(a.lastPlayed));
}

/** A short service tag from a gamertag, e.g. "Sneaky Pete" -> "SP7". */
export function serviceTag(gamertag: string, xuid: string): string {
  const letters = gamertag.split(/\s+/).map((w) => w[0] ?? "").join("").toUpperCase();
  const digit = parseInt(xuid.slice(-2), 16) % 10;
  return (letters + gamertag.replace(/[^A-Za-z]/g, "").toUpperCase()).slice(0, 2) + digit;
}
