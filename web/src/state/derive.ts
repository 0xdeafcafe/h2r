// Stats the service doesn't serve yet, derived client-side from game
// events: places, weapon breakdowns, service-record totals, per-player
// history and "friends" (people you've played with). Medals are the
// server's (each game's `medals`); here they are only named and summed.
// Everything here is pure.

import type { GameMode, GameState, Kill, LiveState, Standing } from "./reducer";
import { gamesFor, isWinner, standings, teamStandings } from "./reducer";
import { gameSeconds } from "./length";
import { mapGroupKey } from "../lib/maps";
import type { Xuid } from "../api/types";
import { countSkulls, type SkullCount } from "../lib/skulls";

// ---------- medals ----------

/** Halo 2's own medals, the ones the game awards and the host reports
 *  (`GameMedal`), in the game's order: the ids of
 *  crates/live/live-proto/src/film/events/medals.rs. */
export type OfficialMedalId =
  | "double-kill" | "triple-kill" | "killtacular" | "kill-frenzy" | "killtrocity" | "killimanjaro"
  | "sniper-kill" | "roadkill" | "bonecracker" | "stealth-kill" | "vehicle-kill" | "carjack" | "stick-it"
  | "killing-spree" | "running-riot" | "rampage" | "berserker" | "overkill"
  | "flag-taken" | "flag-save" | "flag-returned" | "bomb-planted" | "bomb-save" | "bomb-diffused";

/** The superbounce medals (docs/game/superbounce.md, "Medals"), which Live
 *  works out from a game's bounces and bounce deaths. */
export type BounceMedalId =
  | "superbounce" | "double-bounce" | "triple-bounce" | "quad-bounce" | "chain-bounce" | "bouncetacular"
  | "bounced" | "monster-bounce";

/** Every medal Live awards: Halo 2's own, then our modes' (the bounce
 *  family and `incel`). A server may send an id not listed here (a newer
 *  medal); it is shown by its id. */
export type MedalId = OfficialMedalId | BounceMedalId | "incel";

export interface MedalDef { id: MedalId; name: string; blurb: string; tier: 0 | 1 | 2 | 3 }

export const MEDALS: MedalDef[] = [
  { id: "double-kill", name: "Double Kill", blurb: "Two kills in quick succession", tier: 0 },
  { id: "triple-kill", name: "Triple Kill", blurb: "Three kills in quick succession", tier: 1 },
  { id: "killtacular", name: "Killtacular", blurb: "Four kills in quick succession", tier: 2 },
  { id: "kill-frenzy", name: "Kill Frenzy", blurb: "Five kills in quick succession", tier: 3 },
  { id: "killtrocity", name: "Killtrocity", blurb: "Six kills in quick succession", tier: 3 },
  { id: "killimanjaro", name: "Killimanjaro", blurb: "Seven or more kills in quick succession", tier: 3 },
  { id: "sniper-kill", name: "Sniper Kill", blurb: "A kill with a sniper rifle", tier: 0 },
  { id: "roadkill", name: "Roadkill", blurb: "A kill with a vehicle", tier: 0 },
  { id: "bonecracker", name: "Bonecracker", blurb: "A melee kill", tier: 0 },
  { id: "stealth-kill", name: "Stealth Kill", blurb: "A melee kill from behind", tier: 0 },
  { id: "vehicle-kill", name: "Vehicle Kill", blurb: "Destroyed an occupied vehicle", tier: 0 },
  { id: "carjack", name: "Carjack", blurb: "Boarded an enemy's vehicle", tier: 0 },
  { id: "stick-it", name: "Stick It", blurb: "A kill with a stuck grenade", tier: 0 },
  { id: "killing-spree", name: "Killing Spree", blurb: "Five kills without dying", tier: 1 },
  { id: "running-riot", name: "Running Riot", blurb: "Ten kills without dying", tier: 2 },
  { id: "rampage", name: "Rampage", blurb: "Fifteen kills without dying", tier: 3 },
  { id: "berserker", name: "Berserker", blurb: "Twenty kills without dying", tier: 3 },
  { id: "overkill", name: "Overkill", blurb: "Twenty-five kills without dying", tier: 3 },
  { id: "flag-taken", name: "Flag Taken", blurb: "Took the enemy flag", tier: 0 },
  { id: "flag-save", name: "Flag Save", blurb: "Killed the flag carrier", tier: 0 },
  { id: "flag-returned", name: "Flag Returned", blurb: "Returned the flag", tier: 0 },
  { id: "bomb-planted", name: "Bomb Planted", blurb: "Planted the bomb", tier: 0 },
  { id: "bomb-save", name: "Bomb Save", blurb: "Killed the bomb carrier", tier: 0 },
  { id: "bomb-diffused", name: "Bomb Diffused", blurb: "Defused the bomb", tier: 0 },
  { id: "superbounce", name: "Superbounce", blurb: "Bounced off a superbounce crease", tier: 0 },
  { id: "double-bounce", name: "Double Bounce", blurb: "2 bounces with no ground between", tier: 0 },
  { id: "triple-bounce", name: "Triple Bounce", blurb: "3 bounces with no ground between", tier: 1 },
  { id: "quad-bounce", name: "Quad Bounce", blurb: "4 bounces with no ground between", tier: 2 },
  { id: "chain-bounce", name: "Chain Bounce", blurb: "5 bounces with no ground between", tier: 2 },
  { id: "bouncetacular", name: "Bouncetacular", blurb: "6 bounces with no ground between", tier: 3 },
  { id: "bounced", name: "Bounced", blurb: "Fell out of the map trying a bounce", tier: 0 },
  { id: "monster-bounce", name: "Monster Bounce", blurb: "A bounce that launches well above a normal one", tier: 2 },
  { id: "incel", name: "Incel", blurb: "Completed a game without teabagging or getting teabagged", tier: 1 },
];
/** The bounce medals, in table order. */
export const BOUNCE_MEDALS: BounceMedalId[] = [
  "superbounce", "double-bounce", "triple-bounce", "quad-bounce", "chain-bounce", "bouncetacular", "bounced",
  "monster-bounce",
];
export const isBounceMedal = (id: string): id is BounceMedalId => (BOUNCE_MEDALS as string[]).includes(id);
export const MEDAL_BY_ID = Object.fromEntries(MEDALS.map((m) => [m.id, m])) as Record<MedalId, MedalDef>;

/** The site's entry for a medal id, or undefined for one it doesn't know. */
export const medalDef = (id: string): MedalDef | undefined => (MEDAL_BY_ID as Record<string, MedalDef | undefined>)[id];
/** A medal's name, or its id when the site doesn't know it. */
export const medalName = (id: string): string => medalDef(id)?.name ?? id;

/** Medal counts by id, as the server sends them. */
export type MedalCounts = Record<string, number>;

/** Adds `from`'s counts into `into`. */
export function addMedals(into: Map<string, number>, from: MedalCounts | undefined) {
  for (const [id, n] of Object.entries(from ?? {})) if (n > 0) into.set(id, (into.get(id) ?? 0) + n);
}

/** A game's medals (the server's, `GameState.medals`) for the given slots,
 *  or everyone's. */
export function gameMedalCounts(g: Pick<GameState, "medals">, slots?: Iterable<number>): Map<string, number> {
  const m = new Map<string, number>();
  const only = slots ? new Set(slots) : null;
  for (const [slot, counts] of Object.entries(g.medals ?? {})) if (!only || only.has(+slot)) addMedals(m, counts);
  return m;
}

/** The `n` most earned, most first (ties in the site's medal order, then by id). */
export function topMedalCounts(m: MedalCounts | Map<string, number>, n = Infinity): Array<[string, number]> {
  const order = (id: string) => { const i = MEDALS.findIndex((d) => d.id === id); return i < 0 ? MEDALS.length : i; };
  return [...(m instanceof Map ? m.entries() : Object.entries(m))]
    .filter(([, c]) => c > 0)
    .sort((a, b) => b[1] - a[1] || order(a[0]) - order(b[0]) || a[0].localeCompare(b[0]))
    .slice(0, n);
}

/** An account's medals from `GET /v1/accounts/{who}/medals`, read
 *  defensively: anything that isn't a positive count is left out. */
export function parseAccountMedals(raw: unknown): Map<string, number> {
  const o = raw && typeof raw === "object" ? (raw as { medals?: unknown }).medals : null;
  const m = new Map<string, number>();
  if (o && typeof o === "object") {
    for (const [id, n] of Object.entries(o as Record<string, unknown>)) if (typeof n === "number" && n > 0) m.set(id, n);
  }
  return m;
}

/** `rec` with the account's career medals (the server's, all time) in
 *  place of the sum over its loaded games, once they are known. */
export function withCareerMedals<T extends { medals: Map<string, number> }>(rec: T, career: Map<string, number> | null | undefined): T {
  return career ? { ...rec, medals: career } : rec;
}

/** A kill that counts for the killer: not a suicide, betrayal or world death. */
export function scoringKill(k: Kill): boolean {
  return k.kind === "kill" && k.killer !== null && k.killer !== k.victim;
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

export type GameWeaponRow = Omit<WeaponRow, "weapon"> & { weapon: string | null };

/** Keep missing kills in mixed totals. Leave the breakdown empty when
 *  none of the scoring kills names a weapon. */
export function gameWeaponReport(kills: readonly Kill[], limit = Infinity): GameWeaponRow[] {
  const recorded: GameWeaponRow[] = [];
  const missing: GameWeaponRow = { weapon: null, kills: 0, share: 0 };
  for (const row of weaponBreakdown(kills)) {
    const weapon = row.weapon.trim();
    if (weapon && weapon.toLowerCase() !== "unknown") recorded.push(row);
    else {
      missing.kills += row.kills;
      missing.share += row.share;
    }
  }
  if (!recorded.length || limit <= 0) return [];
  return missing.kills ? [...recorded.slice(0, limit - 1), missing] : recorded.slice(0, limit);
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

/** One row per multiplayer game the account played in, newest first
 *  (campaign games have their own rows, `campaignHistory`). */
export function gameHistory(s: LiveState, xuid: Xuid): HistoryRow[] {
  return gamesFor(s, xuid, "multiplayer").map((g) => {
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
  /** The server's medals for the record's games, summed over the
   *  account's slots in each (bots' medals are the bot's own). */
  medals: Map<string, number>;
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

/** Multiplayer totals for an account (campaign games never count); `only`
 *  narrows them to some of its games (a map, a playlist). */
/** The knockouts of `g` a career counts: none with a Clanker bot on either
 *  side (docs/game/bots.md §6). The game record keeps them all; a kill of a
 *  bot, or a death to one, is game-only. */
export function careerKills(g: GameState): Kill[] {
  const bot = (i: number | null) => i !== null && g.players[i]?.bot === true;
  return g.kills.filter((k) => !bot(k.victim) && !bot(k.killer));
}

export function serviceRecord(s: LiveState, xuid: Xuid, only?: (g: GameState) => boolean): ServiceRecord {
  const all = gameHistory(s, xuid);
  const hist = only ? all.filter((h) => only(h.game)) : all;
  const rec: ServiceRecord = {
    games: hist.length, completed: 0, wins: 0, kills: 0, deaths: 0, assists: 0, kd: null, bestSpree: 0,
    firstPlayed: null, lastPlayed: null, exp: 0, rank: RANKS[0]![1], weapons: [], weaponsAgainst: [], medals: new Map(), killsSeen: 0,
  };
  // A bot account's own career counts everything it did, against humans and
  // bots alike (docs/game/bots.md §6); a human's leaves every bot out.
  const botAcct = s.accounts[xuid]?.bot === true;
  const weapons: WeaponRow[][] = [];
  const against: WeaponRow[][] = [];
  for (const h of hist) {
    const g = h.game;
    if (g.status === "ended") rec.completed++;
    if (h.won) rec.wins++;
    const slots = new Set(h.slots);
    // The scoreboard's counts less what bots gave or took (as far as the
    // loaded knockouts tell).
    const kills = botAcct ? g.kills : careerKills(g);
    const isBot = (i: number | null) => i !== null && g.players[i]?.bot === true;
    const botKills = botAcct ? 0 : g.kills.filter((k) => k.kind === "kill" && k.killer !== null && slots.has(k.killer) && isBot(k.victim)).length;
    const botDeaths = botAcct ? 0 : g.kills.filter((k) => slots.has(k.victim) && isBot(k.killer)).length;
    rec.kills += Math.max(0, h.kills - botKills); rec.deaths += Math.max(0, h.deaths - botDeaths); rec.assists += h.assists;
    for (const i of h.slots) rec.bestSpree = Math.max(rec.bestSpree, bestSprees(kills).get(i) ?? 0);
    if (!rec.firstPlayed || g.started_at < rec.firstPlayed) rec.firstPlayed = g.started_at;
    if (!rec.lastPlayed || g.started_at > rec.lastPlayed) rec.lastPlayed = g.started_at;
    weapons.push(weaponBreakdown(kills, slots));
    against.push(weaponsAgainst(kills, slots));
    for (const [m, n] of gameMedalCounts(g, slots)) rec.medals.set(m, (rec.medals.get(m) ?? 0) + n);
    rec.killsSeen += kills.filter((k) => k.killer !== null && slots.has(k.killer)).length;
  }
  rec.weapons = mergeWeapons(weapons);
  rec.weaponsAgainst = mergeWeapons(against);
  rec.kd = rec.deaths ? rec.kills / rec.deaths : null;
  rec.exp = rec.completed + rec.wins * 2;
  rec.rank = rankFor(rec.exp);
  return rec;
}

export interface Friend { xuid: Xuid; games: number; lastPlayed: string }

/** People an account has shared a game with, most games first; `mode`
 *  keeps one half of the stats (campaign: co-op partners). */
export function playedWith(s: LiveState, xuid: Xuid, mode?: GameMode): Friend[] {
  const m = new Map<Xuid, Friend>();
  for (const g of gamesFor(s, xuid, mode)) {
    const seen = new Set<Xuid>();
    for (const p of Object.values(g.players)) {
      if (!p.xuid || p.bot || p.xuid === xuid || seen.has(p.xuid)) continue;
      seen.add(p.xuid);
      const f = m.get(p.xuid) ?? { xuid: p.xuid, games: 0, lastPlayed: g.started_at };
      f.games++;
      if (g.started_at > f.lastPlayed) f.lastPlayed = g.started_at;
      m.set(p.xuid, f);
    }
  }
  return [...m.values()].sort((a, b) => b.games - a.games || b.lastPlayed.localeCompare(a.lastPlayed));
}

// ---------- campaign ----------

/** How a campaign game went for the record: `finished` is a completed
 *  level, anything else (quit, connection lost) is not, and a live game is
 *  still being played. */
export type CampaignOutcome = "complete" | "playing" | "quit" | "lost" | "ended";

/** `ended` is a game whose end reason isn't known yet (a summary read
 *  before its detail): not counted as complete. */
export function campaignOutcome(g: Pick<GameState, "status" | "end_reason">): CampaignOutcome {
  if (g.status === "live") return "playing";
  switch (g.end_reason) {
    case "finished": return "complete";
    case "lost": return "lost";
    case "quit": return "quit";
    default: return "ended";
  }
}

/** A `lost` game is one whose console went quiet without saying how it
 *  ended (closed, crashed, or cut off); the server closed it after waiting.
 *  It reads "Unfinished", not "Connection lost": closing the game is the
 *  usual way it happens. */
export const OUTCOME_WORDS: Record<CampaignOutcome, string> = {
  complete: "Level complete", playing: "Playing now", quit: "Quit", lost: "Unfinished", ended: "Ended",
};

/** A tooltip for an outcome that needs one. */
export const LOST_NOTE = "The console stopped reporting before the game ended, so the server closed it";

export interface CampaignRow {
  game: GameState;
  slots: number[];
  outcome: CampaignOutcome;
  /** Game time, in seconds. */
  seconds: number;
  /** Times the account's players died: the scoreboard's count, or the
   *  recorded death events when those are more. */
  deaths: number;
  /** Kills the scoreboard credits (in campaign these are of the level's AI,
   *  when the game reports them), or null when it reports none. */
  kills: number | null;
  /** Everyone else in the game, by display name. */
  partners: string[];
}

/** Deaths for some slots of a game: the scoreboard's count, or the death
 *  events seen when there are more of those (a scoreboard may lag). */
export function deathsOf(g: GameState, slots: Iterable<number>): number {
  const set = new Set(slots);
  let board = 0;
  for (const i of set) board += g.scores[i]?.deaths ?? 0;
  const seen = g.kills.filter((k) => set.has(k.victim)).length;
  return Math.max(board, seen);
}

/** One row per campaign game the account played in, newest first. */
export function campaignHistory(s: LiveState, xuid: Xuid, now: number): CampaignRow[] {
  return gamesFor(s, xuid, "campaign").map((g) => {
    const slots = slotsFor(g, xuid);
    let kills = 0;
    for (const i of slots) kills += g.scores[i]?.kills ?? 0;
    return {
      game: g,
      slots,
      outcome: campaignOutcome(g),
      seconds: gameSeconds(g, now),
      deaths: deathsOf(g, slots),
      kills: kills > 0 ? kills : null,
      partners: Object.values(g.players).filter((p) => !slots.includes(p.index)).map((p) => p.name),
    };
  });
}

export interface LevelRow {
  /** The level's base map; with `map_id` set, a modded copy of it. */
  map: string;
  /** The map package played, kept apart from the retail level (mapGroupKey). */
  map_id: string | null;
  plays: number;
  completions: number;
  deaths: number;
  /** Seconds played on the level, all attempts. */
  seconds: number;
  /** The quickest completed run, in seconds, or null before one. */
  fastest: number | null;
  lastPlayed: string;
}

export interface CampaignRecord {
  games: number;
  completed: number;
  /** Games that ended any other way than finished. */
  unfinished: number;
  live: number;
  /** Games with more than one player. */
  coop: number;
  deaths: number;
  /** Scoreboard kills, or null when no game reported any. */
  kills: number | null;
  seconds: number;
  firstPlayed: string | null;
  lastPlayed: string | null;
  /** Levels played, most recently played first. */
  levels: LevelRow[];
  /** Skulls played with, in index order, each with the number of games it was on in. */
  skulls: SkullCount[];
  /** Accounts the player has shared campaign games with. */
  partners: Friend[];
}

/** The skulls across a player's campaign games, each with a game count. */
export function skullsPlayedWith(rows: Array<{ game: Pick<GameState, "skulls"> }>): SkullCount[] {
  return countSkulls(rows, (r) => r.game.skulls);
}

/** Campaign totals for an account. Nothing here counts towards the
 *  multiplayer record, and nothing multiplayer counts here. */
export function campaignRecord(s: LiveState, xuid: Xuid, now: number): CampaignRecord {
  const rows = campaignHistory(s, xuid, now);
  const rec: CampaignRecord = {
    games: rows.length, completed: 0, unfinished: 0, live: 0, coop: 0, deaths: 0, kills: null, seconds: 0,
    firstPlayed: null, lastPlayed: null, levels: [], skulls: skullsPlayedWith(rows), partners: playedWith(s, xuid, "campaign"),
  };
  const levels = new Map<string, LevelRow>();
  for (const r of rows) {
    const g = r.game;
    if (r.outcome === "complete") rec.completed++;
    else if (r.outcome === "playing") rec.live++;
    else rec.unfinished++;
    if (Object.keys(g.players).length > 1) rec.coop++;
    rec.deaths += r.deaths;
    if (r.kills !== null) rec.kills = (rec.kills ?? 0) + r.kills;
    rec.seconds += r.seconds;
    if (!rec.firstPlayed || g.started_at < rec.firstPlayed) rec.firstPlayed = g.started_at;
    if (!rec.lastPlayed || g.started_at > rec.lastPlayed) rec.lastPlayed = g.started_at;
    const key = mapGroupKey(g);
    const l = levels.get(key) ?? { map: g.map, map_id: g.map_id ?? null, plays: 0, completions: 0, deaths: 0, seconds: 0, fastest: null, lastPlayed: g.started_at };
    l.plays++;
    l.deaths += r.deaths;
    l.seconds += r.seconds;
    if (r.outcome === "complete") {
      l.completions++;
      l.fastest = l.fastest === null ? r.seconds : Math.min(l.fastest, r.seconds);
    }
    if (g.started_at > l.lastPlayed) l.lastPlayed = g.started_at;
    levels.set(key, l);
  }
  rec.levels = [...levels.values()].sort((a, b) => b.lastPlayed.localeCompare(a.lastPlayed));
  return rec;
}

/** A short service tag from a gamertag, e.g. "Sneaky Pete" -> "SP7". */
export function serviceTag(gamertag: string, xuid: string): string {
  const letters = gamertag.split(/\s+/).map((w) => w[0] ?? "").join("").toUpperCase();
  const digit = parseInt(xuid.slice(-2), 16) % 10;
  return (letters + gamertag.replace(/[^A-Za-z]/g, "").toUpperCase()).slice(0, 2) + digit;
}
