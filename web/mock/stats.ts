// The mock's `GET /v1/stats/summary` (docs/live/live-api.md "Stats summary"):
// the same counting rules as crates/live/live-server/src/stats.rs, folded over
// the mock's log. The log only reaches back the simulated history (twenty
// minutes by default), so a made-up archive of the 30 days before it (game
// starts, kills and ends only, no frames, never in the log) is folded in
// first so the stats home has weeks to show; `H2_MOCK_STATS_ARCHIVE=0`
// leaves it out.

import type { WireEvent } from "../src/api/types";
import { MAPS } from "./maps";
import { LEVELS, multiplayerKind, mockSkulls } from "./coop";

const HOUR = 3_600_000;
const DAY = 86_400_000;
const KEEP_DAYS = 30;
const MAX_GAME_MS = 12 * HOUR;
export const WINDOWS: Record<string, number | null> = { today: 1, "7d": 7, "30d": 30, all: null };

type Ev = Pick<WireEvent, "stream" | "type" | "at" | "data" | "position">;
/** A bot's slot has no account (`xuid` null) and `bot` set: game-only, as in stats.rs. */
interface Slot { xuid: string | null; name: string; team: number; bot?: boolean }
interface Game {
  started: number; campaign: boolean; map: string; map_id: string | null; difficulty: string | null;
  slots: Map<number, Slot>; xuids: Set<string>; spree: Map<number, number>;
}

interface PlayerAgg {
  name: string; named: number; games: number; multiplayer: number; campaign: number; wins: number; kills: number;
  deaths: number; betrayals: number; best_spree: number; seconds: number; legendary: Set<string>;
}
type Totals = Record<"games" | "multiplayer" | "campaign" | "matchmaking" | "custom" | "coop" | "kills" | "deaths" | "betrayals" | "suicides" | "campaign_deaths" | "player_seconds" | "legendary", number>;
const zero = (): Totals => ({ games: 0, multiplayer: 0, campaign: 0, matchmaking: 0, custom: 0, coop: 0, kills: 0, deaths: 0, betrayals: 0, suicides: 0, campaign_deaths: 0, player_seconds: 0, legendary: 0 });

/** One counted thing at a time: summaries filter these by time. */
type Fact =
  | { k: "start"; at: number; t: Partial<Totals>; map: string; map_id: string | null; campaign: boolean; playlist: string | null; skulls: number[]; players: Array<[string, string]> }
  | { k: "join"; at: number; xuid: string; name: string; campaign: boolean }
  | { k: "kill"; at: number; t: Partial<Totals>; victim: [string, string] | null; killer: [string, string] | null; scoring: boolean; betrayal: boolean; spree: number; weapon: string | null; map: string; map_id: string | null }
  | { k: "end"; at: number; secs: number; people: number; players: Array<[string, string]>; winners: Set<string>; legendary: string | null };

export class StatsFold {
  private games = new Map<string, Game>();
  readonly facts: Fact[] = [];

  apply(e: Ev) {
    if (!e.stream.startsWith("game-")) return;
    const id = e.stream.slice(5);
    const at = Date.parse(e.at);
    if (Number.isNaN(at)) return;
    const d = (e.data ?? {}) as Record<string, any>;
    switch (e.type) {
      case "GameStarted": {
        const campaign = d.mode === "campaign";
        const kind = campaign ? "campaign" : d.kind === "matchmaking" ? "matchmaking" : "custom";
        type Wire = { index: number; name: string; xuid?: string; team?: number; bot?: boolean };
        // A bot has no account here, whatever xuid it carries.
        const players: Wire[] = (Array.isArray(d.players) ? (d.players as Wire[]) : [])
          .map((p) => (p.bot ? { ...p, xuid: undefined } : p));
        const g: Game = {
          started: at, campaign, map: String(d.map), map_id: typeof d.map_id === "string" ? d.map_id : null,
          difficulty: typeof d.difficulty === "string" ? d.difficulty.toLowerCase() : null,
          slots: new Map(players.map((p) => [p.index, { xuid: p.xuid ?? null, name: p.name, team: p.team ?? 0, bot: p.bot === true }])),
          xuids: new Set(players.flatMap((p) => (p.xuid ? [p.xuid] : []))), spree: new Map(),
        };
        this.games.set(id, g);
        const coop = campaign && (players.length > 1 || d.online === true);
        this.facts.push({
          k: "start", at, campaign, map: g.map, map_id: g.map_id,
          t: { games: 1, [campaign ? "campaign" : "multiplayer"]: 1, ...(kind === "campaign" ? {} : { [kind]: 1 }), ...(coop ? { coop: 1 } : {}) },
          playlist: kind === "matchmaking" && typeof d.playlist === "string" ? d.playlist : null,
          skulls: Array.isArray(d.skulls) ? d.skulls : [],
          players: players.flatMap((p) => (p.xuid ? [[p.xuid, p.name] as [string, string]] : [])),
        });
        return;
      }
      case "PlayerJoined": {
        const g = this.games.get(id);
        const p = d.player;
        if (!g || !p) return;
        g.slots.set(p.index, { xuid: p.bot ? null : p.xuid ?? null, name: p.name, team: p.team ?? 0, bot: p.bot === true });
        g.spree.delete(p.index);
        if (p.xuid && !p.bot && !g.xuids.has(p.xuid)) {
          g.xuids.add(p.xuid);
          this.facts.push({ k: "join", at, xuid: p.xuid, name: p.name, campaign: g.campaign });
        }
        return;
      }
      case "PlayerKilled": {
        const g = this.games.get(id);
        if (!g) return;
        const v = g.slots.get(d.victim);
        if (g.campaign) {
          this.facts.push({ k: "kill", at, t: { campaign_deaths: 1 }, victim: null, killer: null, scoring: false, betrayal: false, spree: 0, weapon: null, map: g.map, map_id: g.map_id });
          return;
        }
        g.spree.set(d.victim, 0);
        const killerSlot = typeof d.killer === "number" && d.killer !== d.victim ? d.killer : null;
        const scoring = d.kind === "kill" && killerSlot !== null;
        // A bot on either side: the totals only, nobody's row.
        const bot = v?.bot === true || (typeof d.killer === "number" && g.slots.get(d.killer)?.bot === true);
        let spree = 0;
        if (scoring && !bot) { spree = (g.spree.get(killerSlot!) ?? 0) + 1; g.spree.set(killerSlot!, spree); }
        const ks = killerSlot !== null && !bot ? g.slots.get(killerSlot) : undefined;
        this.facts.push({
          k: "kill", at,
          t: { deaths: 1, ...(scoring ? { kills: 1 } : d.kind === "betrayal" ? { betrayals: 1 } : d.kind === "suicide" ? { suicides: 1 } : {}) },
          victim: v?.xuid && !bot ? [v.xuid, v.name] : null, killer: ks?.xuid ? [ks.xuid, ks.name] : null,
          scoring, betrayal: d.kind === "betrayal", spree, weapon: scoring && typeof d.weapon === "string" && d.weapon ? d.weapon : null,
          map: g.map, map_id: g.map_id,
        });
        return;
      }
      case "GameEnded": {
        const g = this.games.get(id);
        if (!g) return;
        this.games.delete(id);
        const w = d.winner;
        const winners = new Set<string>();
        if (!g.campaign && w && typeof w.team === "number") for (const s of g.slots.values()) { if (s.team === w.team && s.xuid) winners.add(s.xuid); }
        if (!g.campaign && w && typeof w.player === "number") { const s = g.slots.get(w.player); if (s?.xuid) winners.add(s.xuid); }
        const names = new Map([...g.slots.values()].flatMap((s) => (s.xuid ? [[s.xuid, s.name] as [string, string]] : [])));
        this.facts.push({
          k: "end", at, secs: Math.round(Math.min(MAX_GAME_MS, Math.max(0, at - g.started)) / 1000),
          people: Math.max(g.slots.size, g.xuids.size), players: [...g.xuids].map((x) => [x, names.get(x) ?? ""]), winners,
          legendary: g.campaign && d.reason === "finished" && g.difficulty === "legendary" ? g.map_id ?? g.map : null,
        });
        return;
      }
    }
  }

  summary(window: string, now: number, position: number) {
    if (!(window in WINDOWS)) return null;
    const days = WINDOWS[window]!;
    const today = Math.floor(now / DAY);
    const from = days === null ? null : (today - days + 1) * DAY;
    const nowH = Math.floor(now / HOUR);
    const keepFrom = (today - KEEP_DAYS + 1) * DAY;
    const actFrom = Math.max(from ?? 0, (nowH - 24 * Math.min(days ?? KEEP_DAYS, KEEP_DAYS) + 1) * HOUR, keepFrom);
    const totals = zero(), last24 = zero(), all = zero();
    const players = new Map<string, PlayerAgg>();
    const maps = new Map<string, { map: string; map_id?: string; campaign: boolean; games: number; kills: number }>();
    const weapons = new Map<string, number>(), playlists = new Map<string, number>(), skulls = new Map<number, number>();
    const activity = new Map<number, { games: number; kills: number }>();
    const add = (t: Totals, d: Partial<Totals>) => { for (const [k, v] of Object.entries(d)) t[k as keyof Totals] += v!; };
    const player = (x: string, name: string, at: number) => {
      let p = players.get(x);
      if (!p) players.set(x, p = { name, named: at, games: 0, multiplayer: 0, campaign: 0, wins: 0, kills: 0, deaths: 0, betrayals: 0, best_spree: 0, seconds: 0, legendary: new Set() });
      if (at >= p.named) { p.name = name; p.named = at; }
      return p;
    };
    const mapOf = (m: string, id: string | null, campaign: boolean) => {
      const key = `${id ?? m}|${campaign}`;
      let r = maps.get(key);
      if (!r) maps.set(key, r = { map: m, ...(id ? { map_id: id } : {}), campaign, games: 0, kills: 0 });
      return r;
    };
    for (const f of this.facts) {
      const delta: Partial<Totals> = f.k === "start" || f.k === "kill" ? f.t
        : f.k === "end" ? { player_seconds: f.secs * f.people, ...(f.legendary ? { legendary: 1 } : {}) } : {};
      add(all, delta);
      if (f.at >= Math.max(keepFrom, (nowH - 23) * HOUR) && f.at < (nowH + 1) * HOUR) add(last24, delta);
      if (f.at >= actFrom && (f.k === "start" || (f.k === "kill" && f.scoring))) {
        const h = Math.floor(f.at / HOUR);
        const a = activity.get(h) ?? { games: 0, kills: 0 };
        if (f.k === "start") a.games++; else a.kills++;
        activity.set(h, a);
      }
      // `all` counts everything; windows only reach the kept days.
      if (from !== null && (f.at < Math.max(from, keepFrom) || f.at >= (today + 1) * DAY)) continue;
      add(totals, delta);
      switch (f.k) {
        case "start":
          mapOf(f.map, f.map_id, f.campaign).games++;
          if (f.playlist) playlists.set(f.playlist, (playlists.get(f.playlist) ?? 0) + 1);
          for (const s of f.skulls) skulls.set(s, (skulls.get(s) ?? 0) + 1);
          for (const [x, n] of f.players) { const p = player(x, n, f.at); p.games++; p[f.campaign ? "campaign" : "multiplayer"]++; }
          break;
        case "join": { const p = player(f.xuid, f.name, f.at); p.games++; p[f.campaign ? "campaign" : "multiplayer"]++; break; }
        case "kill":
          if (f.victim) player(...f.victim, f.at).deaths++;
          if (f.scoring) { mapOf(f.map, f.map_id, false).kills++; if (f.weapon) weapons.set(f.weapon, (weapons.get(f.weapon) ?? 0) + 1); }
          if (f.killer) {
            const p = player(...f.killer, f.at);
            if (f.scoring) { p.kills++; p.best_spree = Math.max(p.best_spree, f.spree); } else if (f.betrayal) p.betrayals++;
          }
          break;
        case "end":
          for (const [x, n] of f.players) {
            const p = player(x, n, f.at);
            p.seconds += f.secs;
            if (f.winners.has(x)) p.wins++;
            if (f.legendary) p.legendary.add(f.legendary);
          }
          break;
      }
    }
    const ranked = (m: Map<string, number>) => [...m].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
    return {
      window, from: from === null ? null : new Date(from).toISOString(), to: new Date(now).toISOString(), position,
      totals, last_24h: last24, all_time: all,
      players: [...players].map(([xuid, p]) => ({
        xuid, name: p.name, games: p.games, multiplayer: p.multiplayer, campaign: p.campaign, wins: p.wins, kills: p.kills,
        deaths: p.deaths, betrayals: p.betrayals, best_spree: p.best_spree, seconds: p.seconds, legendary: p.legendary.size,
      })).sort((a, b) => b.kills - a.kills || b.games - a.games || a.xuid.localeCompare(b.xuid)).slice(0, 500),
      maps: [...maps.values()].sort((a, b) => b.games - a.games || b.kills - a.kills || a.map.localeCompare(b.map)),
      weapons: ranked(weapons).map(([weapon, kills]) => ({ weapon, kills })),
      playlists: ranked(playlists).map(([playlist, games]) => ({ playlist, games })),
      skulls: [...skulls].sort((a, b) => a[0] - b[0]).map(([skull, games]) => ({ skull, games })),
      activity: [...activity].sort((a, b) => a[0] - b[0]).map(([h, a]) => ({ hour: new Date(h * HOUR).toISOString(), ...a })),
    };
  }
}

const WEAPONS = ["rifle", "rifle", "smg", "blaster", "sniper", "shotgun", "rocket", "sword", "grenade", "pistol"];
const DIFFICULTIES = ["normal", "heroic", "heroic", "legendary"];

/** Made-up games for the 30 days before `until`: more in the evenings and at
 *  weekends, between the given accounts. Deterministic for a seed. */
export function archive(accounts: Array<{ xuid: string; gamertag: string }>, until: number, seed = 7): Ev[] {
  let h = seed >>> 0;
  const r = () => { h = (Math.imul(h, 1664525) + 1013904223) >>> 0; return h / 0x100000000; };
  const pick = <T,>(xs: readonly T[]) => xs[Math.floor(r() * xs.length)]!;
  const out: Ev[] = [];
  const push = (id: string, type: string, at: number, data: unknown) => out.push({ stream: `game-${id}`, type, at: new Date(at).toISOString(), data, position: 0 });
  if (accounts.length < 2) return out;
  // Some players play far more than others.
  const weight = accounts.map((_, i) => 1 / (1 + i * 0.35));
  const choose = (n: number) => {
    const pool = accounts.map((a, i) => ({ a, w: weight[i]! * r() }));
    return pool.sort((x, y) => y.w - x.w).slice(0, n).map((x) => x.a);
  };
  let n = 0;
  for (let t = until - 30 * DAY; t < until - HOUR; t += HOUR) {
    const d = new Date(t);
    const hour = d.getUTCHours(), day = d.getUTCDay();
    const evening = hour >= 18 && hour <= 23 ? 1.6 : hour >= 12 ? 0.8 : hour <= 2 ? 0.7 : 0.12;
    const weekend = day === 0 || day === 6 ? 1.5 : day === 5 ? 1.25 : 1;
    let games = Math.floor(evening * weekend * r() * 3 + 0.15);
    while (games-- > 0) {
      const id = `arch${(n++).toString(16).padStart(6, "0")}`;
      const start = t + Math.floor(r() * 50) * 60_000;
      const campaign = r() < 0.15;
      if (campaign) {
        const who = choose(1 + Math.floor(r() * r() * 3));
        const level = pick(LEVELS).name, difficulty = pick(DIFFICULTIES);
        push(id, "GameStarted", start, { game_id: id, map: level, mode: "campaign", kind: "campaign", difficulty, team_play: false, skulls: mockSkulls(id, true), players: who.map((a, i) => ({ index: i, name: a.gamertag, xuid: a.xuid, team: 0 })) });
        const len = (20 + r() * 40) * 60_000;
        for (let k = Math.floor(r() * 12); k > 0; k--) push(id, "PlayerKilled", start + r() * len, { tick: 0, victim: Math.floor(r() * who.length), kind: r() < 0.2 ? "world" : "kill", weapon: "plasma_rifle" });
        push(id, "GameEnded", start + len, { tick: 0, reason: r() < 0.75 ? "finished" : "quit" });
        continue;
      }
      const who = choose(Math.min(accounts.length, 4 + Math.floor(r() * 5)));
      const team_play = r() < 0.65;
      const map = pick(MAPS).name;
      const kind = multiplayerKind(r);
      push(id, "GameStarted", start, {
        game_id: id, map, mode: "multiplayer", ...kind, team_play, skulls: mockSkulls(id, false),
        players: who.map((a, i) => ({ index: i, name: a.gamertag, xuid: a.xuid, team: team_play ? i % 2 : i })),
      });
      const len = (6 + r() * 10) * 60_000;
      const skill = who.map((_, i) => 0.6 + r() * (1.6 - i * 0.05));
      const kills = 20 + Math.floor(r() * 50);
      const score = new Map<number, number>();
      for (let k = 0; k < kills; k++) {
        const at = start + (len * (k + r())) / kills;
        const killer = skill.map((s, i) => [s * r(), i] as const).sort((a, b) => b[0] - a[0])[0]![1];
        let victim = Math.floor(r() * who.length);
        if (victim === killer) victim = (victim + 1) % who.length;
        const roll = r();
        const kindOf = roll < 0.03 ? "suicide" : team_play && roll < 0.05 && killer % 2 === victim % 2 ? "betrayal" : "kill";
        if (kindOf === "suicide") push(id, "PlayerKilled", at, { tick: 0, victim: killer, killer, kind: "suicide", weapon: "grenade" });
        else {
          push(id, "PlayerKilled", at, { tick: 0, victim, killer, kind: kindOf, weapon: pick(WEAPONS) });
          if (kindOf === "kill") { const side = team_play ? killer % 2 : killer; score.set(side, (score.get(side) ?? 0) + 1); }
        }
      }
      const best = [...score].sort((a, b) => b[1] - a[1])[0];
      push(id, "GameEnded", start + len, { tick: 0, reason: "finished", winner: best ? (team_play ? { team: best[0] } : { player: best[0] }) : null });
    }
  }
  return out.sort((a, b) => Date.parse(a.at) - Date.parse(b.at));
}
