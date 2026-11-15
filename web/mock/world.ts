// The mock's world: an in-memory signed event log plus a little simulation
// of people signing in and out and playing multiplayer games.

import type { Frame, GamePlayer, MapOutline, PlayerScore, WireEvent } from "../src/api/types";
import { MAPS, type MockMap } from "./maps";
import { chainHashSync, newSigningKey, signJws, type SigningKey } from "./sign";

export const HOST = "mock.h2live.local";
const TICKS = 30;
const FRAME_HZ = 15;
const STEP_TICKS = TICKS / FRAME_HZ;

const TAGS = [
  "Sneaky Pete", "Mochi", "Captain Waffles", "Pixel Pop", "Noodle", "Biscuit", "Zoomer",
  "Tofu Tank", "Lemon Drop", "Grumbles", "Sprinkles", "Bean Machine", "Moss", "Pancake Pal",
];
const WEAPONS = ["blaster", "rifle", "sniper", "shotgun", "rocket", "sword", "grenade", "pistol", "smg"];
const VARIANTS_FFA = ["Free for All", "Rumble", "Snowball Fight", "Oddball Party"];
const VARIANTS_TEAM = ["Team Tag", "Team Rumble", "Capture the Cushion", "Big Team Picnic"];

interface Account {
  xuid: string;
  gamertag: string;
  key: SigningKey;
  keyFrom: number;
  created_at: string;
  online: boolean;
  state: string;
  game_id: string | null;
  since: string;
  /** Made by a real client (the web sign-up): the mock has no private key,
   *  so the simulation leaves it alone. */
  external?: boolean;
}

interface SimPlayer {
  p: GamePlayer;
  pos: [number, number, number];
  yaw: number;
  target: number;
  alive: boolean;
  respawnAt: number;
  speed: number;
  score: PlayerScore;
}

interface SimGame {
  id: string;
  map: MockMap;
  team_play: boolean;
  variant: string;
  score_limit: number;
  time_limit_s: number;
  tick: number;
  players: SimPlayer[];
  teams: Map<number, number>;
  host: Account;
  started_at: string;
  ended_at: string | null;
  kills: Array<Record<string, unknown>>;
  winner: { team: number } | { player: number } | null;
  end_reason: string | null;
  frames: Frame[];
  killRate: number;
}

export type Listener = { onEvent(ev: WireEvent): void; onFrame(f: Frame): void };

let rngState = 0x2f6e2b1;
export function rand(): number {
  // mulberry32, deterministic per run so screenshots are comparable.
  rngState |= 0; rngState = (rngState + 0x6d2b79f5) | 0;
  let t = Math.imul(rngState ^ (rngState >>> 15), 1 | rngState);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)]!;
const hex = (n: number) => Array.from({ length: n }, () => Math.floor(rand() * 16).toString(16)).join("");

export class World {
  readonly serverKey = newSigningKey();
  readonly log: WireEvent[] = [];
  readonly streams = new Map<string, { version: number; hash: string | null }>();
  readonly accounts = new Map<string, Account>();
  readonly games = new Map<string, SimGame>();
  readonly outlines = new Map<string, MapOutline>();
  readonly files = new Map<string, { bytes: Uint8Array<ArrayBuffer>; sha256: string }>();
  readonly listeners = new Set<Listener>();
  /** Virtual clock in ms; real time once caught up. */
  now = Date.now();
  live = false;
  tamperEvery = 0;
  private nextGameAt = 0;
  private nextPresenceAt = 0;
  private targetGames = 2;

  author(a: Account | null): { author: string; key: SigningKey } {
    return a ? { author: `acct:${a.xuid}@${HOST}`, key: a.key } : { author: `server:${HOST}`, key: this.serverKey };
  }

  /** Appends events to one stream, signed by one author in one JWS when batched. */
  append(stream: string, items: Array<{ type: string; data: unknown }>, by: Account | null): WireEvent[] {
    const { author, key } = this.author(by);
    const at = new Date(this.now).toISOString();
    const bodies = items.map((it) => ({ stream, type: it.type, data: it.data, author, kid: key.kid, at, nonce: hex(16) }));
    const jws = signJws(key, bodies.length === 1 ? bodies[0] : bodies);
    const out: WireEvent[] = [];
    for (const [i, b] of bodies.entries()) {
      const st = this.streams.get(stream) ?? { version: 0, hash: null };
      st.version += 1;
      st.hash = chainHashSync(st.hash, jws);
      this.streams.set(stream, st);
      const ev: WireEvent = {
        position: this.log.length + 1, stream, version: st.version, type: b.type, at,
        data: b.data, author, kid: key.kid, jws, hash: st.hash,
      };
      if (bodies.length > 1) ev.jws_index = i;
      if (this.tamperEvery && ev.position % this.tamperEvery === 0 && b.type !== "GameFrame" && b.type !== "AccountCreated") {
        // Demo of a forged copy: the data no longer matches the signature.
        ev.data = { ...(b.data as object), tampered: true };
      }
      this.log.push(ev);
      out.push(ev);
      if (this.live) for (const l of this.listeners) l.onEvent(ev);
    }
    return out;
  }

  // ---------- accounts and presence ----------

  /** An account registered by a client with its own key (POST /v1/accounts). */
  createExternalAccount(gamertag: string, jwk: { kty: "OKP"; crv: "Ed25519"; x: string; kid: string }): Account {
    const xuid = "0009" + hex(12);
    const at = new Date(this.now).toISOString();
    const a: Account = {
      xuid, gamertag, key: { kid: jwk.kid, jwk, privateKey: undefined as never }, keyFrom: this.log.length + 1,
      created_at: at, online: false, state: "offline", game_id: null, since: at, external: true,
    };
    this.accounts.set(xuid, a);
    // The mock can't sign as the new key, so it records the creation itself.
    this.append(`account-${xuid}`, [{ type: "AccountCreated", data: { xuid, gamertag } }], null);
    this.append(`gamertag-${gamertag.toLowerCase()}`, [{ type: "GamertagReserved", data: { xuid } }], null);
    return a;
  }

  createAccount(gamertag: string, createdAt = this.now): Account {
    const xuid = "0009" + hex(12);
    const a: Account = {
      xuid, gamertag, key: newSigningKey(), keyFrom: this.log.length + 1, created_at: new Date(createdAt).toISOString(),
      online: false, state: "offline", game_id: null, since: new Date(createdAt).toISOString(),
    };
    this.accounts.set(xuid, a);
    const save = this.now;
    this.now = createdAt;
    this.append(`account-${xuid}`, [{ type: "AccountCreated", data: { xuid, gamertag } }], a);
    this.append(`gamertag-${gamertag.toLowerCase()}`, [{ type: "GamertagReserved", data: { xuid } }], null);
    this.now = save;
    return a;
  }

  private presence(a: Account, type: string, state: string, game_id: string | null, by: Account | null = a) {
    const was = a.online;
    a.online = type === "SignedIn" || type === "PresenceChanged";
    a.state = a.online ? state : "offline";
    a.game_id = a.online ? game_id : null;
    if (!was || !a.online) a.since = new Date(this.now).toISOString();
    const data: Record<string, unknown> = { xuid: a.xuid, state: a.state };
    if (game_id) data.game_id = game_id;
    this.append(`presence-${a.xuid}`, [{ type, data }], by);
  }

  private presenceChurn() {
    const all = [...this.accounts.values()].filter((a) => !a.external);
    const offline = all.filter((a) => !a.online);
    const idle = all.filter((a) => a.online && !a.game_id);
    const r = rand();
    if (offline.length && (r < 0.45 || idle.length < 3)) {
      this.presence(pick(offline), "SignedIn", "online", null);
    } else if (idle.length > 2 && r < 0.7) {
      const a = pick(idle);
      if (rand() < 0.2) this.presence(a, "PresenceExpired", "offline", null, null);
      else this.presence(a, "SignedOut", "offline", null);
    } else if (idle.length) {
      const a = pick(idle);
      this.presence(a, "PresenceChanged", a.state === "away" ? "online" : "away", null);
    }
  }

  // ---------- games ----------

  startGame(): SimGame | null {
    const free = [...this.accounts.values()].filter((a) => !a.game_id && !a.external);
    if (free.length < 2) return null;
    const map = pick(MAPS);
    const team_play = rand() < 0.5;
    const n = Math.min(16, 4 + Math.floor(rand() * 9));
    const humans = free.sort(() => rand() - 0.5).slice(0, Math.min(free.length, Math.max(2, Math.floor(n * 0.6))));
    const id = hex(16);
    const players: GamePlayer[] = [];
    for (let i = 0; i < n; i++) {
      const a = humans[i];
      players.push({
        index: i, name: a ? a.gamertag : `Guest ${i + 1}`, xuid: a?.xuid, team: team_play ? i % 2 : i, local: i === 0,
      });
    }
    const host = humans[0]!;
    const g: SimGame = {
      id, map, team_play, variant: pick(team_play ? VARIANTS_TEAM : VARIANTS_FFA),
      score_limit: team_play ? 40 : 15, time_limit_s: 300, tick: 0, host,
      players: players.map((p) => {
        const w = pick(map.waypoints);
        return {
          p, pos: [w[0], w[1], w[2]] as [number, number, number], yaw: rand() * Math.PI * 2,
          target: Math.floor(rand() * map.waypoints.length), alive: true, respawnAt: 0, speed: 3.5 + rand() * 2.5,
          score: { index: p.index, score: 0, kills: 0, deaths: 0, assists: 0 },
        };
      }),
      teams: new Map(team_play ? [[0, 0], [1, 0]] : []),
      started_at: new Date(this.now).toISOString(), ended_at: null, kills: [], frames: [], winner: null, end_reason: null,
      killRate: 0.12 + n * 0.035,
    };
    this.games.set(id, g);
    for (const a of humans) if (!a.online) this.presence(a, "SignedIn", "online", null);
    if (map.outline && !this.outlines.has(map.name)) this.outlines.set(map.name, map.outline);
    this.append(`game-${id}`, [{
      type: "GameStarted",
      data: {
        game_id: id, console: host.xuid, map: map.name, mode: "multiplayer", variant: g.variant, team_play,
        score_limit: g.score_limit, time_limit_s: g.time_limit_s, tick: 0, players,
      },
    }], host);
    for (const a of humans) this.presence(a, "PresenceChanged", "playing", id);
    return g;
  }

  private stepGame(g: SimGame) {
    g.tick += STEP_TICKS;
    const dt = STEP_TICKS / TICKS;
    const wps = g.map.waypoints;
    for (const sp of g.players) {
      if (!sp.alive) {
        if (g.tick >= sp.respawnAt) {
          const w = pick(wps);
          sp.pos = [w[0], w[1], w[2]];
          sp.target = Math.floor(rand() * wps.length);
          sp.alive = true;
        }
        continue;
      }
      const t = wps[sp.target]!;
      const dx = t[0] - sp.pos[0], dy = t[1] - sp.pos[1], dz = t[2] - sp.pos[2];
      const d = Math.hypot(dx, dy);
      if (d < 1) {
        // Next waypoint: a nearby one, so paths follow the floor.
        const near = wps.map((w, i) => [i, Math.hypot(w[0] - t[0], w[1] - t[1]) + Math.abs(w[2] - t[2]) * 2] as const)
          .filter(([, dd]) => dd > 3 && dd < 8);
        sp.target = near.length ? pick(near)[0] : Math.floor(rand() * wps.length);
        continue;
      }
      const step = Math.min(d, sp.speed * dt);
      sp.pos = [sp.pos[0] + (dx / d) * step, sp.pos[1] + (dy / d) * step, sp.pos[2] + (dz / Math.max(d, 1)) * step];
      const want = Math.atan2(dy, dx) + Math.sin(g.tick / 20 + sp.p.index) * 0.4;
      let diff = want - sp.yaw;
      while (diff > Math.PI) diff -= Math.PI * 2;
      while (diff < -Math.PI) diff += Math.PI * 2;
      sp.yaw += diff * 0.25;
    }

    if (rand() < g.killRate * dt) this.kill(g);

    const frame: Frame = {
      game_id: g.id, tick: g.tick,
      players: g.players.map((sp) => ({
        index: sp.p.index, alive: sp.alive,
        pos: [round(sp.pos[0]), round(sp.pos[1]), round(sp.pos[2])], yaw: round(sp.yaw), vehicle: null,
      })),
    };
    if (this.live) for (const l of this.listeners) l.onFrame(frame);
    if (g.tick % TICKS === 0) {
      g.frames.push(frame);
      this.append(`game-${g.id}`, [{ type: "GameFrame", data: { ...frame, source: `acct:${g.host.xuid}@${HOST}` } }], null);
    }

    const leader = g.team_play ? Math.max(...g.teams.values()) : Math.max(...g.players.map((p) => p.score.score));
    if (leader >= g.score_limit || g.tick >= g.time_limit_s * TICKS) this.endGame(g, "finished");
  }

  private kill(g: SimGame) {
    const alive = g.players.filter((p) => p.alive);
    if (alive.length < 2) return;
    const victim = pick(alive);
    const r = rand();
    let kind: "kill" | "suicide" | "betrayal" | "world" = "kill";
    let killer: SimPlayer | null = null;
    if (r < 0.05) kind = "suicide";
    else if (r < 0.08) kind = "world";
    else {
      const foes = alive.filter((p) => p !== victim && (!g.team_play || p.p.team !== victim.p.team || r < 0.11));
      if (!foes.length) return;
      killer = foes.sort((a, b) => dist(a, victim) - dist(b, victim))[0]!;
      if (g.team_play && killer.p.team === victim.p.team) kind = "betrayal";
    }
    victim.alive = false;
    victim.respawnAt = g.tick + 3 * TICKS;
    victim.score.deaths += 1;
    if (kind === "kill" && killer) {
      killer.score.kills += 1;
      killer.score.score += 1;
      if (g.team_play) g.teams.set(killer.p.team, (g.teams.get(killer.p.team) ?? 0) + 1);
      const helper = g.players.find((p) => p !== killer && p !== victim && p.alive && (!g.team_play || p.p.team === killer!.p.team) && rand() < 0.3);
      if (helper) helper.score.assists += 1;
    } else if (kind === "betrayal" && killer) {
      killer.score.score -= 1;
      if (g.team_play) g.teams.set(killer.p.team, (g.teams.get(killer.p.team) ?? 0) - 1);
    } else if (kind === "suicide") {
      victim.score.score -= 1;
      if (g.team_play) g.teams.set(victim.p.team, (g.teams.get(victim.p.team) ?? 0) - 1);
    }
    const kd: Record<string, unknown> = {
      tick: g.tick, victim: victim.p.index, kind,
      pos: [round(victim.pos[0]), round(victim.pos[1]), round(victim.pos[2])],
    };
    if (killer) { kd.killer = killer.p.index; kd.weapon = pick(WEAPONS); }
    // Kill and score in one signed batch, as the game's ingest would send them.
    const appended = this.append(`game-${g.id}`, [
      { type: "PlayerKilled", data: kd },
      { type: "ScoreChanged", data: this.scoreData(g) },
    ], g.host);
    const kev = appended[0]!;
    g.kills.push({ ...kd, position: kev.position, at: kev.at });
  }

  private scoreData(g: SimGame) {
    const d: Record<string, unknown> = { tick: g.tick, players: g.players.map((p) => ({ ...p.score })) };
    if (g.team_play) d.teams = [...g.teams.entries()].map(([team, score]) => ({ team, score }));
    return d;
  }

  endGame(g: SimGame, reason: string) {
    let winner: { team: number } | { player: number } | null = null;
    if (g.team_play) {
      const best = [...g.teams.entries()].sort((a, b) => b[1] - a[1]);
      if (best[0] && (!best[1] || best[0][1] > best[1][1])) winner = { team: best[0][0] };
    } else {
      const best = [...g.players].sort((a, b) => b.score.score - a.score.score);
      if (best[0] && (!best[1] || best[0].score.score > best[1].score.score)) winner = { player: best[0].p.index };
    }
    g.ended_at = new Date(this.now).toISOString();
    const data: Record<string, unknown> = { tick: g.tick, reason, winner, players: g.players.map((p) => ({ ...p.score })) };
    g.winner = winner;
    g.end_reason = reason;
    this.append(`game-${g.id}`, [{ type: "GameEnded", data }], g.host);
    for (const sp of g.players) {
      const a = sp.p.xuid ? this.accounts.get(sp.p.xuid) : undefined;
      if (a && a.game_id === g.id) this.presence(a, "PresenceChanged", "online", null);
    }
    this.nextGameAt = this.now + 4000 + rand() * 8000;
    this.targetGames = 1 + Math.floor(rand() * 3);
  }

  liveGames(): SimGame[] {
    return [...this.games.values()].filter((g) => !g.ended_at);
  }

  /** One simulation step (1/15 s of game time). */
  step() {
    for (const g of this.liveGames()) this.stepGame(g);
    if (this.now >= this.nextPresenceAt) {
      this.presenceChurn();
      this.nextPresenceAt = this.now + 4000 + rand() * 9000;
    }
    if (this.liveGames().length < this.targetGames && this.now >= this.nextGameAt) {
      this.startGame();
      this.nextGameAt = this.now + 3000 + rand() * 5000;
    }
  }

  /** Seeds accounts and runs `historyMs` of simulated time instantly. */
  seed(historyMs: number) {
    const start = Date.now() - historyMs;
    TAGS.forEach((t, i) => this.createAccount(t, start - (TAGS.length - i) * 86400_000 * 1.7));
    this.now = start;
    for (const a of [...this.accounts.values()].slice(0, 7)) this.presence(a, "SignedIn", "online", null);
    while (this.now < Date.now()) {
      this.now += 1000 / FRAME_HZ;
      this.step();
    }
  }

  /** Runs in real time, emitting frames and events to listeners. */
  run() {
    this.live = true;
    setInterval(() => {
      this.now = Date.now();
      this.step();
    }, 1000 / FRAME_HZ);
  }

  // ---------- read models for REST ----------

  accountByTag(tag: string): Account | undefined {
    const k = tag.toLowerCase();
    return [...this.accounts.values()].find((a) => a.gamertag.toLowerCase() === k);
  }

  accountKeys(a: Account) {
    return { keys: [{ kid: a.key.kid, jwk: a.key.jwk, valid_from: a.keyFrom, valid_to: null }] };
  }

  gameSummary(g: SimGame) {
    return {
      game_id: g.id, map: g.map.name, mode: "multiplayer", variant: g.variant, started_at: g.started_at,
      ended_at: g.ended_at, players: g.players.map((p) => p.p),
      teams: g.team_play ? [...g.teams.entries()].map(([team, score]) => ({ team, score })) : null,
    };
  }

  gameDetail(g: SimGame) {
    return {
      ...this.gameSummary(g), console: g.host.xuid, team_play: g.team_play, score_limit: g.score_limit,
      time_limit_s: g.time_limit_s, tick: 0, last_tick: g.tick, winner: g.winner, end_reason: g.end_reason,
      scoreboard: g.players.map((p) => ({ ...p.score })), kills: g.kills.slice(-50), position: this.log.length,
    };
  }

  publicAccount(a: Account) {
    return { xuid: a.xuid, gamertag: a.gamertag, created_at: a.created_at, online: a.online };
  }
}

function dist(a: SimPlayer, b: SimPlayer) {
  return Math.hypot(a.pos[0] - b.pos[0], a.pos[1] - b.pos[1]);
}

function round(n: number) {
  return Math.round(n * 100) / 100;
}
