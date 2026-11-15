// The event-to-state reducer. Pure: it mutates a plain state object, so it
// runs inside Solid's `produce` in the app and on plain objects in tests.

import type {
  AccountCreated, GameEnded, GamePlayer, GameStarted, KillKind, PlayerJoined, PlayerKilled,
  PlayerLeft, PlayerScore, PresenceData, ScoreChanged, WireEvent, Xuid, GameId,
} from "../api/types";

export interface Presence {
  state: string;
  game_id: GameId | null;
  since: string;
}

export interface AccountState {
  xuid: Xuid;
  gamertag: string;
  created_at: string;
  online: boolean;
  presence: Presence | null;
  last_seen: string | null;
}

export interface Kill {
  position: number;
  at: string;
  tick: number;
  victim: number;
  killer: number | null;
  weapon: string | null;
  kind: KillKind;
}

export interface GamePlayerState extends GamePlayer {
  left: boolean;
}

export interface GameState {
  game_id: GameId;
  console: string | null;
  map: string;
  mode: string;
  variant: string | null;
  team_play: boolean;
  score_limit: number | null;
  time_limit_s: number | null;
  started_at: string;
  ended_at: string | null;
  status: "live" | "ended";
  end_reason: string | null;
  winner: number | null;
  players: Record<number, GamePlayerState>;
  scores: Record<number, PlayerScore>;
  teams: Record<number, number>;
  kills: Kill[];
  last_tick: number;
  first_position: number;
  /** Log positions of this game's events (for signature summaries). */
  positions: number[];
}

export interface LiveState {
  position: number;
  caughtUp: boolean;
  accounts: Record<Xuid, AccountState>;
  games: Record<GameId, GameState>;
  /** Game ids, newest first. */
  gameOrder: GameId[];
  /** Tail of the raw log for the event page, oldest first. */
  tail: WireEvent[];
  typeCounts: Record<string, number>;
  eventCount: number;
}

export const TAIL_LIMIT = 500;

export function initialState(): LiveState {
  return {
    position: 0,
    caughtUp: false,
    accounts: {},
    games: {},
    gameOrder: [],
    tail: [],
    typeCounts: {},
    eventCount: 0,
  };
}

function emptyScore(index: number): PlayerScore {
  return { index, score: 0, kills: 0, deaths: 0, assists: 0 };
}

function streamGameId(stream: string): GameId | null {
  return stream.startsWith("game-") ? stream.slice(5) : null;
}

function addPlayer(g: GameState, p: GamePlayer) {
  g.players[p.index] = { ...p, xuid: p.xuid ?? null, left: false };
  if (!g.scores[p.index]) g.scores[p.index] = emptyScore(p.index);
}

function gameFor(s: LiveState, ev: WireEvent): GameState | undefined {
  const d = ev.data as { game_id?: string } | null;
  const id = d?.game_id ?? streamGameId(ev.stream);
  return id ? s.games[id] : undefined;
}

/** Applies one event. Events at or below the current position are ignored,
 *  so a resumed stream that repeats an event is harmless. */
export function applyEvent(s: LiveState, ev: WireEvent): boolean {
  if (ev.position <= s.position) return false;
  s.position = ev.position;
  s.eventCount += 1;
  s.typeCounts[ev.type] = (s.typeCounts[ev.type] ?? 0) + 1;
  s.tail.push(ev);
  if (s.tail.length > TAIL_LIMIT) s.tail.splice(0, s.tail.length - TAIL_LIMIT);

  if (ev.type !== "GameStarted" && ev.stream.startsWith("game-")) {
    const g = s.games[ev.stream.slice(5)];
    if (g) g.positions.push(ev.position);
  }

  switch (ev.type) {
    case "AccountCreated": {
      const d = ev.data as AccountCreated;
      const prev = s.accounts[d.xuid];
      s.accounts[d.xuid] = {
        xuid: d.xuid,
        gamertag: d.gamertag,
        created_at: ev.at,
        online: prev?.online ?? false,
        presence: prev?.presence ?? null,
        last_seen: prev?.last_seen ?? null,
      };
      break;
    }
    case "SignedIn":
    case "PresenceChanged": {
      const d = ev.data as PresenceData;
      const a = ensureAccount(s, d.xuid, ev.at);
      const wasOnline = a.online && a.presence;
      a.online = true;
      a.presence = {
        state: d.state ?? "online",
        game_id: d.game_id ?? null,
        since: ev.type === "SignedIn" || !wasOnline ? ev.at : a.presence!.since,
      };
      a.last_seen = ev.at;
      break;
    }
    case "SignedOut":
    case "PresenceExpired": {
      const d = ev.data as PresenceData;
      const a = ensureAccount(s, d.xuid, ev.at);
      a.online = false;
      a.presence = null;
      a.last_seen = ev.at;
      break;
    }
    case "GameStarted": {
      const d = ev.data as GameStarted;
      const id = d.game_id ?? streamGameId(ev.stream);
      if (!id) break;
      const g: GameState = {
        game_id: id,
        console: d.console ?? null,
        map: d.map,
        mode: d.mode,
        variant: d.variant ?? null,
        team_play: !!d.team_play,
        score_limit: d.score_limit ?? null,
        time_limit_s: d.time_limit_s ?? null,
        started_at: ev.at,
        ended_at: null,
        status: "live",
        end_reason: null,
        winner: null,
        players: {},
        scores: {},
        teams: {},
        kills: [],
        last_tick: 0,
        first_position: ev.position,
        positions: [ev.position],
      };
      for (const p of d.players ?? []) addPlayer(g, p);
      if (g.team_play) for (const p of Object.values(g.players)) g.teams[p.team] ??= 0;
      s.games[id] = g;
      s.gameOrder = [id, ...s.gameOrder.filter((x) => x !== id)];
      break;
    }
    case "PlayerJoined": {
      const g = gameFor(s, ev);
      const d = ev.data as PlayerJoined;
      if (!g || !d.player) break;
      addPlayer(g, d.player);
      if (g.team_play) g.teams[d.player.team] ??= 0;
      break;
    }
    case "PlayerLeft": {
      const g = gameFor(s, ev);
      const d = ev.data as PlayerLeft;
      const p = g?.players[d.index];
      if (p) p.left = true;
      break;
    }
    case "PlayerKilled": {
      const g = gameFor(s, ev);
      if (!g) break;
      const d = ev.data as PlayerKilled;
      g.kills.push({
        position: ev.position,
        at: ev.at,
        tick: d.tick,
        victim: d.victim,
        killer: d.killer ?? null,
        weapon: d.weapon ?? null,
        kind: d.kind ?? "kill",
      });
      g.last_tick = Math.max(g.last_tick, d.tick ?? 0);
      break;
    }
    case "ScoreChanged": {
      const g = gameFor(s, ev);
      if (!g) break;
      const d = ev.data as ScoreChanged;
      applyScores(g, d.players);
      for (const t of d.teams ?? []) g.teams[t.team] = t.score;
      g.last_tick = Math.max(g.last_tick, d.tick ?? 0);
      break;
    }
    case "GameFrame": {
      const g = gameFor(s, ev);
      const d = ev.data as { tick?: number };
      if (g && typeof d?.tick === "number" && d.tick > g.last_tick) g.last_tick = d.tick;
      break;
    }
    case "GameEnded": {
      const g = gameFor(s, ev);
      if (!g) break;
      const d = ev.data as GameEnded;
      g.status = "ended";
      g.ended_at = ev.at;
      g.end_reason = d.reason ?? "finished";
      g.winner = d.winner ?? null;
      g.last_tick = Math.max(g.last_tick, d.tick ?? 0);
      // `players` is the final scoreboard; tolerate a roster too.
      const final = (d.players ?? []) as Array<Partial<PlayerScore> & { index: number }>;
      if (final.length && final.every((p) => typeof p.kills === "number")) {
        applyScores(g, final as PlayerScore[]);
      }
      break;
    }
    default:
      break;
  }
  return true;
}

function applyScores(g: GameState, players: PlayerScore[] | undefined) {
  for (const p of players ?? []) {
    g.scores[p.index] = {
      index: p.index,
      score: p.score ?? 0,
      kills: p.kills ?? 0,
      deaths: p.deaths ?? 0,
      assists: p.assists ?? 0,
    };
  }
}

function ensureAccount(s: LiveState, xuid: Xuid, at: string): AccountState {
  let a = s.accounts[xuid];
  if (!a) {
    // Presence for an account we haven't seen created (stream started late).
    a = { xuid, gamertag: xuid, created_at: at, online: false, presence: null, last_seen: null };
    s.accounts[xuid] = a;
  }
  return a;
}

export function applyEvents(s: LiveState, evs: WireEvent[]) {
  for (const ev of evs) applyEvent(s, ev);
}

// ---------- selectors (pure, used by pages and tests) ----------

export function liveGames(s: LiveState): GameState[] {
  return s.gameOrder.map((id) => s.games[id]!).filter((g) => g && g.status === "live");
}

export function endedGames(s: LiveState, limit = 20): GameState[] {
  const out: GameState[] = [];
  for (const id of s.gameOrder) {
    const g = s.games[id];
    if (g && g.status === "ended") out.push(g);
    if (out.length >= limit) break;
  }
  return out;
}

export interface Standing {
  index: number;
  name: string;
  team: number;
  xuid: Xuid | null;
  left: boolean;
  score: PlayerScore;
}

/** Players sorted by score, then kills, then fewest deaths. */
export function standings(g: GameState): Standing[] {
  return Object.values(g.players)
    .map((p) => ({
      index: p.index,
      name: p.name,
      team: p.team,
      xuid: p.xuid ?? null,
      left: p.left,
      score: g.scores[p.index] ?? emptyScore(p.index),
    }))
    .sort((a, b) =>
      b.score.score - a.score.score || b.score.kills - a.score.kills ||
      a.score.deaths - b.score.deaths || a.index - b.index);
}

export interface TeamStanding { team: number; score: number; players: Standing[] }

export function teamStandings(g: GameState): TeamStanding[] {
  const by = new Map<number, Standing[]>();
  for (const st of standings(g)) {
    if (!by.has(st.team)) by.set(st.team, []);
    by.get(st.team)!.push(st);
  }
  for (const t of Object.keys(g.teams)) if (!by.has(+t)) by.set(+t, []);
  return [...by.entries()]
    .map(([team, players]) => ({
      team,
      players,
      score: g.teams[team] ?? players.reduce((n, p) => n + p.score.score, 0),
    }))
    .sort((a, b) => b.score - a.score || a.team - b.team);
}

export interface PlayerStats { games: number; kills: number; deaths: number; wins: number; live: number }

/** Lifetime stats for an account, derived from game events. */
export function playerStats(s: LiveState, xuid: Xuid): PlayerStats {
  const st: PlayerStats = { games: 0, kills: 0, deaths: 0, wins: 0, live: 0 };
  for (const g of gamesFor(s, xuid)) {
    st.games += 1;
    if (g.status === "live") st.live += 1;
    const slots = new Set(Object.values(g.players).filter((p) => p.xuid === xuid).map((p) => p.index));
    for (const k of g.kills) {
      if (k.killer !== null && slots.has(k.killer) && k.kind === "kill" && k.killer !== k.victim) st.kills += 1;
      if (slots.has(k.victim)) st.deaths += 1;
    }
    if (g.status === "ended" && g.winner !== null) {
      for (const i of slots) {
        const p = g.players[i]!;
        if (g.team_play ? p.team === g.winner : p.index === g.winner) st.wins += 1;
      }
    }
  }
  return st;
}

export function gamesFor(s: LiveState, xuid: Xuid): GameState[] {
  return s.gameOrder
    .map((id) => s.games[id]!)
    .filter((g) => g && Object.values(g.players).some((p) => p.xuid === xuid));
}

export function accountByGamertag(s: LiveState, tag: string): AccountState | undefined {
  const k = tag.toLowerCase();
  return Object.values(s.accounts).find((a) => a.gamertag.toLowerCase() === k);
}
