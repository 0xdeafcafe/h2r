// The event-to-state reducer. Pure: it mutates a plain state object, so it
// runs inside Solid's `produce` in the app and on plain objects in tests.

import type {
  AccountCreated, AccountSummary, BotIssued, GameDetail, GameKindFields, GameEnded, GamePlayer, GameStarted, GameSummary, KillKind,
  KillRecord, PlayerJoined, PlayerKilled, PlayerLeft, PlayerScore, PresenceData, PresenceEntry, ScoreChanged,
  WireEvent, Winner, Xuid, GameId,
} from "../api/types";
import { clankerAppearance, normalizeAppearance, type Appearance } from "../lib/gameart";
import { normalizeSkulls } from "../lib/skulls";
import type { DedicatedHostFields } from "../lib/hosts";

export interface Presence {
  state: string;
  game_id: GameId | null;
  /** What the player is doing ("Co-op: Outskirts"), when the game said. */
  activity: string | null;
  since: string;
}

export interface AccountState {
  xuid: Xuid;
  gamertag: string;
  created_at: string;
  online: boolean;
  presence: Presence | null;
  last_seen: string | null;
  /** A bot account (docs/game/bots.md §6): its own page and career, no social,
   *  never in a human's played-with or the players list. */
  bot: boolean;
}

export interface Kill {
  position: number;
  at: string;
  tick: number;
  victim: number;
  killer: number | null;
  weapon: string | null;
  kind: KillKind;
  /** Victim position, when the event carried it. */
  pos: [number, number, number] | null;
}

export interface GamePlayerState extends GamePlayer {
  /** What the site shows: the account's gamertag for a player linked to an
   *  account (by `xuid`), otherwise the name the game sent. */
  name: string;
  /** The in-game (profile) name, when it differs from the gamertag shown. */
  game_name: string | null;
  left: boolean;
  /** The player's appearance (emblem layers, profile colours), when roster
   *  events have carried one. */
  appearance: Appearance | null;
}

export interface GameState extends DedicatedHostFields {
  archiveRevision?: number;
  game_id: GameId;
  console: string | null;
  /** Who started the game: `acct:<xuid>@<server>`, when the server said. */
  host: string | null;
  /** The base map's name (a modded Lockout is "lockout"). */
  map: string;
  /** The map package played, null for retail maps (lib/maps.ts). */
  map_id: string | null;
  mode: string;
  /** `custom`, `matchmaking` or `campaign` when the game said; see `gameKind`. */
  kind: string | null;
  /** Matchmaking only: the playlist's name and id. */
  playlist: string | null;
  playlist_id: string | null;
  /** Campaign only: the difficulty, when the game said. */
  difficulty: string | null;
  /** Campaign only: true for co-op over the network. */
  online: boolean | null;
  /** Skulls that were on, sorted indices (none when the game said nothing). */
  skulls: number[];
  variant: string | null;
  team_play: boolean;
  score_limit: number | null;
  time_limit_s: number | null;
  started_at: string;
  ended_at: string | null;
  status: "live" | "ended";
  end_reason: string | null;
  winner: Winner | null;
  /** Game tick the game started at. */
  start_tick: number;
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
  /** Games the stream started that may be private (a Live co-op campaign
   *  game from a friends-only or invite-only session), held back until the
   *  reader's own `GET /v1/games/{id}` confirms it may see them
   *  (live-api.md "Co-op games"). The live connection takes them. */
  held: GameId[];
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
    held: [],
  };
}

/** Whether a started game can't be shown from the raw stream: a Live co-op
 *  campaign game (`online: true`) may come from a friends-only or
 *  invite-only session, which the server's filtered reads hide from some
 *  readers. Everything else is public. */
export function needsConfirm(d: GameKindFields & { mode?: string | null }): boolean {
  const kind = str(d.kind)?.toLowerCase() ?? (d.mode === "campaign" ? "campaign" : null);
  return kind === "campaign" && d.online === true;
}

function emptyScore(index: number): PlayerScore {
  return { index, score: 0, kills: 0, deaths: 0, assists: 0 };
}

function streamGameId(stream: string): GameId | null {
  return stream.startsWith("game-") ? stream.slice(5) : null;
}

/** An account's gamertag, if the account itself is known (a placeholder
 *  made from presence alone carries its XUID as the gamertag). */
function gamertagOf(s: LiveState, xuid: Xuid | null | undefined): string | null {
  const a = xuid ? s.accounts[xuid] : undefined;
  return a && a.gamertag !== a.xuid ? a.gamertag : null;
}

/** A player linked to an account is shown by gamertag; the game's own name
 *  (the profile name on the console) is kept beside it. */
function displayName(p: { name: string; game_name: string | null }, tag: string | null) {
  if (!tag || tag === p.name) return;
  p.game_name ??= p.name;
  p.name = tag;
}

function addPlayer(s: LiveState, g: GameState, p: GamePlayer) {
  // Telemetry may carry the player's appearance; the API type doesn't know
  // it yet, so read it softly. A roster repeat that omits it keeps what the
  // game already learned.
  const wire = p as GamePlayer & { appearance?: unknown };
  const prev = g.players[p.index];
  // A bot is never an account's player: it keeps the name the game sent.
  const tag = p.bot ? null : gamertagOf(s, p.xuid);
  const botAccount = p.bot && p.xuid ? s.accounts[p.xuid] : undefined;
  g.players[p.index] = {
    ...p,
    name: tag ?? p.name,
    game_name: tag && tag !== p.name ? p.name : null,
    xuid: p.xuid ?? null,
    left: false,
    appearance: botAccount?.bot ? clankerAppearance(botAccount.gamertag)
      : normalizeAppearance(wire.appearance) ?? (p.bot ? clankerAppearance(p.name) : prev?.appearance ?? null),
  };
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
        bot: prev?.bot ?? false,
      };
      // Games that named this account before it was known (a replay, or an
      // account seen first through presence) now show its gamertag.
      for (const g of Object.values(s.games)) {
        for (const p of Object.values(g.players)) if (p.xuid === d.xuid && !p.bot) displayName(p, d.gamertag);
      }
      break;
    }
    case "BotIssued": {
      // A bot account (docs/game/bots.md §6). Never replaces a human account
      // with the same xuid (the 0x000B and 0x0009 ranges can't collide).
      const d = ev.data as BotIssued;
      const prev = s.accounts[d.xuid];
      if (prev && !prev.bot) break;
      s.accounts[d.xuid] = {
        xuid: d.xuid, gamertag: d.gamertag, created_at: prev?.created_at || ev.at,
        online: false, presence: null, last_seen: null, bot: true,
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
        activity: d.activity ?? null,
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
      // Already known from a snapshot: the snapshot is newer, keep it.
      if (s.games[id]) break;
      // Maybe private: not from the stream; the reader's own read decides.
      if (needsConfirm(d)) {
        if (!s.held.includes(id)) s.held.push(id);
        break;
      }
      const g: GameState = {
        game_id: id,
        console: d.console ?? null,
        host: d.host ?? null,
        host_lease: str(d.host_lease),
        dedicated_region: str(d.dedicated_region),
        map: d.map,
        map_id: mapId(d.map_id),
        mode: d.mode,
        ...kindFields(d),
        variant: d.variant ?? null,
        team_play: !!d.team_play,
        score_limit: d.score_limit ?? null,
        time_limit_s: d.time_limit_s ?? null,
        started_at: ev.at,
        ended_at: null,
        status: "live",
        end_reason: null,
        winner: null,
        start_tick: d.tick ?? 0,
        players: {},
        scores: {},
        teams: {},
        kills: [],
        last_tick: d.tick ?? 0,
        first_position: ev.position,
        positions: [ev.position],
      };
      for (const p of d.players ?? []) addPlayer(s, g, p);
      if (g.team_play) for (const p of Object.values(g.players)) g.teams[p.team] ??= 0;
      s.games[id] = g;
      s.gameOrder = [id, ...s.gameOrder.filter((x) => x !== id)];
      break;
    }
    case "PlayerJoined": {
      const g = gameFor(s, ev);
      const d = ev.data as PlayerJoined;
      if (!g || !d.player) break;
      addPlayer(s, g, d.player);
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
      addKill(g, { ...d, position: ev.position, at: ev.at });
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
    case "ReplayArchivePublished":
    case "ReplayArchiveRolledBack": {
      const g = gameFor(s, ev);
      if (g) g.archiveRevision = ev.position;
      break;
    }
    case "FrameChunk": {
      // A stored chunk of recorded frames (api/chunk.ts): its last tick.
      const g = gameFor(s, ev);
      const d = ev.data as { to_tick?: number };
      if (g && typeof d?.to_tick === "number" && d.to_tick > g.last_tick) g.last_tick = d.to_tick;
      break;
    }
    case "GameEnded": {
      const g = gameFor(s, ev);
      if (!g) break;
      const d = ev.data as GameEnded;
      g.status = "ended";
      g.ended_at = ev.at;
      g.end_reason = d.reason ?? "finished";
      g.winner = normalizeWinner(d.winner);
      g.last_tick = Math.max(g.last_tick, d.tick ?? 0);
      applyScores(g, d.players ?? undefined);
      break;
    }
    default:
      break;
  }
  return true;
}

const str = (v: unknown): string | null => (typeof v === "string" && v.trim() ? v.trim() : null);
/** A map_id as sent: 64 hex, kept lower case; anything else is none. */
const mapId = (v: unknown): string | null => (typeof v === "string" && /^[0-9a-f]{64}$/i.test(v) ? v.toLowerCase() : null);

/** The optional game-type fields, read softly: wrong types become null. */
function kindFields(d: GameKindFields) {
  return {
    kind: str(d.kind)?.toLowerCase() ?? null,
    playlist: str(d.playlist),
    playlist_id: str(d.playlist_id),
    difficulty: str(d.difficulty)?.toLowerCase() ?? null,
    online: typeof d.online === "boolean" ? d.online : null,
    skulls: normalizeSkulls(d.skulls),
  };
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

export function normalizeWinner(w: unknown): Winner | null {
  if (!w || typeof w !== "object") return null;
  const o = w as { team?: unknown; player?: unknown };
  if (typeof o.team === "number") return { team: o.team };
  if (typeof o.player === "number") return { player: o.player };
  return null;
}

/** Whether a game has teammates, so a kill can be a betrayal: a team game
 *  (the variant's team flag), or a campaign, where the players are all on
 *  one side. Free-for-all has none, whatever team index its players carry
 *  (live-proto `NewGame::has_teammates`). */
export function hasTeammates(g: Pick<GameState, "team_play" | "mode">): boolean {
  return g.team_play || g.mode === "campaign";
}

/** Whether `killer` betrayed `victim`: a game with teammates, a killer who
 *  is not the victim, both known and on one team (live-proto `is_betrayal`). */
export function isBetrayal(g: Pick<GameState, "team_play" | "mode" | "players">, killer: number | null, victim: number): boolean {
  if (!hasTeammates(g) || killer === null || killer === victim) return false;
  const k = g.players[killer], v = g.players[victim];
  return !!k && !!v && k.team === v.team;
}

/** A recorded kill's kind as it should read: a `kill` or `betrayal` by
 *  another player is settled by the teams, since hosts before the fix sent
 *  free-for-all kills as betrayals; `suicide` and `world` stay as recorded
 *  (live-proto `settled_kill_kind`). */
export function settledKillKind(
  g: Pick<GameState, "team_play" | "mode" | "players">, k: Pick<Kill, "kind" | "killer" | "victim">,
): KillKind {
  if ((k.kind !== "kill" && k.kind !== "betrayal") || k.killer === null || k.killer === k.victim) return k.kind;
  return isBetrayal(g, k.killer, k.victim) ? "betrayal" : "kill";
}

/** Adds a kill unless one at the same log position is already there
 *  (a snapshot and the stream can both carry it). */
function addKill(g: GameState, k: KillRecord) {
  if (k.position !== undefined && g.kills.some((x) => x.position === k.position)) return;
  const kill: Kill = {
    position: k.position ?? 0,
    at: k.at ?? "",
    tick: k.tick,
    victim: k.victim,
    killer: k.killer ?? null,
    weapon: k.weapon ?? null,
    kind: k.kind ?? "kill",
    pos: k.pos ?? null,
  };
  kill.kind = settledKillKind(g, kill);
  // Keep kills in log order even if a snapshot's arrive after streamed ones.
  let i = g.kills.length;
  while (i > 0 && g.kills[i - 1]!.position > kill.position && kill.position > 0) i--;
  g.kills.splice(i, 0, kill);
  // Snapshot kills count towards the game's signature summary too.
  if (kill.position > 0 && !g.positions.includes(kill.position)) g.positions.push(kill.position);
  g.last_tick = Math.max(g.last_tick, k.tick ?? 0);
}

function ensureAccount(s: LiveState, xuid: Xuid, at: string): AccountState {
  let a = s.accounts[xuid];
  if (!a) {
    // Presence for an account we haven't seen created (stream started late).
    a = { xuid, gamertag: xuid, created_at: at, online: false, presence: null, last_seen: null, bot: false };
    s.accounts[xuid] = a;
  }
  return a;
}

export function applyEvents(s: LiveState, evs: WireEvent[]) {
  for (const ev of evs) applyEvent(s, ev);
}

// ---------- selectors (pure, used by pages and tests) ----------

/** The two halves of the stats: a game is campaign or multiplayer. Anything
 *  that isn't `campaign` (an older log, an unknown mode) counts as
 *  multiplayer, which every stats page shows by default. */
export type GameMode = "multiplayer" | "campaign";

export function modeOf(g: Pick<GameState, "mode">): GameMode {
  return g.mode === "campaign" ? "campaign" : "multiplayer";
}

export function isCampaign(g: Pick<GameState, "mode">): boolean {
  return modeOf(g) === "campaign";
}

export function liveGames(s: LiveState, mode?: GameMode): GameState[] {
  return s.gameOrder.map((id) => s.games[id]!).filter((g) => g && g.status === "live" && (!mode || modeOf(g) === mode));
}

/** Ended games, newest first; `mode` narrows them to one half of the stats. */
export function endedGames(s: LiveState, limit = 20, mode?: GameMode): GameState[] {
  const out: GameState[] = [];
  for (const id of s.gameOrder) {
    if (out.length >= limit) break;
    const g = s.games[id];
    if (g && g.status === "ended" && (!mode || modeOf(g) === mode)) out.push(g);
  }
  return out;
}

export interface Standing {
  index: number;
  name: string;
  /** The in-game name, when the gamertag shown differs from it. */
  game_name: string | null;
  team: number;
  xuid: Xuid | null;
  /** A Clanker bot: no player page, a BOT mark beside the name. */
  bot: boolean;
  left: boolean;
  /** The player's appearance, when known (for `<Emblem appearance=…>`). */
  appearance: Appearance | null;
  score: PlayerScore;
}

/** Players sorted by score, then kills, then fewest deaths. */
export function standings(g: GameState): Standing[] {
  return Object.values(g.players)
    .map((p) => ({
      index: p.index,
      name: p.name,
      game_name: p.game_name ?? null,
      team: p.team,
      xuid: p.xuid ?? null,
      bot: p.bot === true,
      left: p.left,
      appearance: p.appearance ?? null,
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

/** Lifetime multiplayer stats for an account, derived from game events.
 *  Campaign games never count towards them. */
export function playerStats(s: LiveState, xuid: Xuid): PlayerStats {
  const st: PlayerStats = { games: 0, kills: 0, deaths: 0, wins: 0, live: 0 };
  for (const g of gamesFor(s, xuid, "multiplayer")) {
    st.games += 1;
    if (g.status === "live") st.live += 1;
    // Scoreboards are authoritative (and complete, unlike a snapshot's
    // last 50 kills), so totals come from them.
    for (const p of Object.values(g.players)) {
      if (p.xuid !== xuid) continue;
      const sc = g.scores[p.index];
      if (sc) { st.kills += sc.kills; st.deaths += sc.deaths; }
      if (g.status === "ended" && isWinner(g, p)) st.wins += 1;
    }
  }
  return st;
}

export function isWinner(g: Pick<GameState, "winner">, p: { index: number; team: number }): boolean {
  const w = g.winner;
  if (!w) return false;
  return "team" in w ? p.team === w.team : p.index === w.player;
}

/** An account's games, newest first; `mode` keeps one half of the stats. */
export function gamesFor(s: LiveState, xuid: Xuid, mode?: GameMode): GameState[] {
  return s.gameOrder
    .map((id) => s.games[id]!)
    .filter((g) => g && (!mode || modeOf(g) === mode) && Object.values(g.players).some((p) => p.xuid === xuid));
}

export function accountByGamertag(s: LiveState, tag: string): AccountState | undefined {
  const k = tag.toLowerCase();
  return Object.values(s.accounts).find((a) => a.gamertag.toLowerCase() === k);
}

// ---------- snapshots (REST reads joined to the stream by position) ----------

export function applyAccountsSnapshot(s: LiveState, accounts: AccountSummary[]) {
  for (const a of accounts) {
    const prev = s.accounts[a.xuid];
    s.accounts[a.xuid] = {
      xuid: a.xuid,
      gamertag: a.gamertag,
      created_at: a.created_at ?? prev?.created_at ?? "",
      online: a.online ?? prev?.online ?? false,
      presence: prev?.presence ?? null,
      last_seen: prev?.last_seen ?? null,
      // From the summary; a bot stays a bot once known.
      bot: a.bot === true || prev?.bot === true,
    };
  }
}

export function applyPresenceSnapshot(s: LiveState, entries: PresenceEntry[]) {
  for (const e of entries) {
    const a = ensureAccount(s, e.xuid, e.since);
    if (a.gamertag === a.xuid && e.gamertag) a.gamertag = e.gamertag;
    a.online = true;
    a.presence = { state: e.state, game_id: e.game_id ?? null, activity: e.activity ?? null, since: e.since };
    a.last_seen = e.since;
  }
}

/** A game from `GET /v1/games` (summary) or `GET /v1/games/{id}` (detail). */
export function applyGameSnapshot(s: LiveState, d: GameSummary | GameDetail) {
  const det = d as GameDetail;
  const teamPlay = det.team_play ?? !!(d.teams && d.teams.length);
  let g = s.games[d.game_id];
  if (!g) {
    g = {
      game_id: d.game_id,
      console: det.console ?? null,
      host: det.host ?? null,
      host_lease: str(d.host_lease),
      dedicated_region: str(d.dedicated_region),
      map: d.map,
      map_id: mapId(d.map_id),
      mode: d.mode,
      ...kindFields(d),
      variant: d.variant ?? null,
      team_play: teamPlay,
      score_limit: det.score_limit ?? null,
      time_limit_s: det.time_limit_s ?? null,
      started_at: d.started_at,
      ended_at: d.ended_at ?? null,
      status: d.ended_at ? "ended" : "live",
      end_reason: det.end_reason ?? null,
      winner: normalizeWinner(det.winner),
      start_tick: det.tick ?? 0,
      players: {},
      scores: {},
      teams: {},
      kills: [],
      last_tick: det.last_tick ?? det.tick ?? 0,
      first_position: 0,
      positions: [],
    };
    s.games[d.game_id] = g;
    s.gameOrder = insertByStart(s, d.game_id, d.started_at);
  } else {
    const k = kindFields(d);
    g.map_id ??= mapId(d.map_id);
    g.host_lease ??= str(d.host_lease);
    g.dedicated_region ??= str(d.dedicated_region);
    g.kind ??= k.kind; g.playlist ??= k.playlist; g.playlist_id ??= k.playlist_id;
    g.difficulty ??= k.difficulty; g.online ??= k.online;
    if (!g.skulls.length) g.skulls = k.skulls;
    if (det.team_play !== undefined) g.team_play = det.team_play;
    if (det.score_limit !== undefined) g.score_limit = det.score_limit ?? null;
    if (det.time_limit_s !== undefined) g.time_limit_s = det.time_limit_s ?? null;
    if (det.console !== undefined) g.console = det.console ?? null;
    if (det.host != null) g.host = det.host;
    if (det.tick != null && !g.start_tick) g.start_tick = det.tick;
    if (det.end_reason !== undefined) g.end_reason = det.end_reason ?? null;
    if (det.winner !== undefined) g.winner = normalizeWinner(det.winner);
    if (d.ended_at) { g.ended_at = d.ended_at; g.status = "ended"; }
    if (det.last_tick) g.last_tick = Math.max(g.last_tick, det.last_tick);
  }
  for (const p of d.players ?? []) if (!g.players[p.index]) addPlayer(s, g, p);
  for (const t of d.teams ?? []) g.teams[t.team] = t.score;
  if (g.team_play) for (const p of Object.values(g.players)) g.teams[p.team] ??= 0;
  if (det.scoreboard) applyScores(g, det.scoreboard);
  for (const k of det.kills ?? []) addKill(g, k);
  // The snapshot may have brought the teams or the team flag.
  for (const k of g.kills) k.kind = settledKillKind(g, k);
}

function insertByStart(s: LiveState, id: GameId, startedAt: string): GameId[] {
  const t = Date.parse(startedAt);
  const order = s.gameOrder.filter((x) => x !== id);
  const i = order.findIndex((x) => Date.parse(s.games[x]?.started_at ?? "") < t);
  order.splice(i < 0 ? order.length : i, 0, id);
  return order;
}

/** Records the log positions of a game's events fetched outside the stream
 *  (a replay's REST read), so the game's signature summary covers them.
 *  Returns the positions that were new. */
export function addGamePositions(s: LiveState, gameId: GameId, evs: WireEvent[]): number[] {
  const g = s.games[gameId];
  if (!g) return [];
  const have = new Set(g.positions);
  const added: number[] = [];
  for (const ev of evs) {
    if (ev.stream !== "game-" + gameId || ev.position <= 0 || have.has(ev.position)) continue;
    have.add(ev.position);
    added.push(ev.position);
  }
  if (added.length) g.positions = [...g.positions, ...added].sort((a, b) => a - b);
  return added;
}

/** Fills the event page's tail with events from before the stream's start. */
export function seedTail(s: LiveState, evs: WireEvent[]) {
  const have = new Set(s.tail.map((e) => e.position));
  const merged = [...evs.filter((e) => !have.has(e.position)), ...s.tail].sort((a, b) => a.position - b.position);
  s.tail = merged.slice(-TAIL_LIMIT);
}
