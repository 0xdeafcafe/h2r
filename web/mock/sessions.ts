// Made-up session advertisements for the mock (docs/live/live-api.md
// "Sessions"): a handful of open games that people drift in and out of,
// that fill up, close and get replaced for the stats site's Open Games
// table. Read-only: `GET
// /v1/sessions` and `GET /v1/sessions/{id}`, in the server's shapes.
// H2_MOCK_SESSIONS=<n> sets how many it keeps open (default 5; 0 for none,
// to see the empty state). The mock writes no session events to the log.
//
// Some are co-op campaign sessions (`flags: "coop"`, a `campaign`, friends
// only, as the game makes them; `ping_ms` once a guest is in), and a co-op
// session a guest joined becomes a finished co-op game when it closes, for
// `GET /v1/coop/games` (live-api.md "Co-op games"), with a few from before
// the mock started. The mock answers as an admin would: it lists friends-only
// sessions and games to everyone.

import { MAPS } from "./maps";
import { HOST, type World } from "./world";

// A generator of its own, so sessions don't shift the world's (seeded) dice.
let seed = 0x51e5510;
function rand(): number {
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}

interface Slots { public_filled: number; public_open: number; private_filled: number; private_open: number }

interface MockSession {
  session_id: string;
  host: string;
  host_xuid: string;
  host_gamertag: string;
  xnkid: string;
  endpoint: { addr: string; port: number; source: string };
  advertisement: {
    addr: string | null; port: number; host_address: number[]; slots: Slots;
    attributes: Record<string, { type: string; value?: unknown }>;
    flags: string; hopper_id: number | null; map: string | null; map_id?: string; name: string | null; language: string | null;
    visibility: "public" | "friends" | "invite";
    campaign?: { level: string; name: string; difficulty: number };
    ping_ms?: number;
  };
  since: string;
  last_activity: number;
  state: string;
  players: Record<string, string>;
}

type Reply = { status: number; body: unknown };

interface Who { xuid: string; gamertag: string }

/** A finished co-op game as `GET /v1/coop/games` lists it. */
export interface MockCoopGame {
  session_id: string;
  host: Who;
  players: Who[];
  campaign: { level: string; name: string; difficulty: number };
  started: string;
  ended: string;
  end_reason: "closed" | "host_left" | "expired";
  visibility: "public" | "friends" | "invite";
}

/** Invented campaign levels (stem and name), as a co-op host advertises them. */
export const COOP_LEVELS: Array<[level: string, name: string]> = [
  ["01a_long_dock", "Long Dock"], ["02b_glass_canyon", "Glass Canyon"], ["03a_harbor_dawn", "Harbor Dawn"],
  ["04b_quiet_relay", "Quiet Relay"], ["05a_ember_vault", "Ember Vault"],
];

const GAMES: Array<[name: string, flags: string, size: number]> = [
  ["Team Slayer", "custom", 8], ["Slayer", "custom", 8], ["Capture the Cushion", "custom", 12], ["Oddball Party", "custom", 6],
  ["Hill Hopping", "custom", 8], ["Big Team Picnic", "hopper", 16], ["Team Rumble", "hopper", 8], ["Rumble", "ranked", 8],
];
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)]!;
const hex = (n: number) => Array.from({ length: n }, () => Math.floor(rand() * 16).toString(16)).join("");

export class Sessions {
  readonly all = new Map<string, MockSession>();
  /** Finished co-op games, newest first. */
  readonly coopGames: MockCoopGame[] = [];
  /** Every guest that joined each open co-op session, with the first join's time. */
  private joined = new Map<string, { guests: string[]; first: string | null }>();

  /** `custom` picks a custom map's package for a session (mock/content.ts). */
  constructor(private world: World, private target = 5, private custom?: (key: string, map: string) => string | undefined) {}

  private open(): MockSession[] {
    return [...this.all.values()].filter((s) => s.state === "open");
  }

  /** Accounts free to be in a session: not hosting one and not on a roster. */
  private free() {
    const busy = new Set(this.open().flatMap((s) => Object.keys(s.players)));
    return [...this.world.accounts.values()].filter((a) => !busy.has(a.xuid));
  }

  private tag(xuid: string): Who {
    return { xuid, gamertag: this.world.accounts.get(xuid)?.gamertag ?? "guest" };
  }

  /** A co-op campaign session: two seats, friends only, a level and a difficulty. */
  private startCoop(): MockSession | null {
    const free = this.free();
    if (!free.length) return null;
    const host = pick(free);
    const id = hex(16);
    const [level, name] = pick(COOP_LEVELS);
    const s: MockSession = {
      session_id: id,
      host: `acct:${host.xuid}@${HOST}`, host_xuid: host.xuid, host_gamertag: host.gamertag,
      xnkid: id,
      endpoint: { addr: `203.0.113.${1 + Math.floor(rand() * 250)}`, port: 3074, source: "observed" },
      advertisement: {
        addr: null, port: 3074, host_address: Array.from({ length: 36 }, () => 0),
        slots: { public_filled: 1, public_open: 1, private_filled: 0, private_open: 0 },
        attributes: {}, flags: "coop", hopper_id: null, map: level, name: null, language: "en",
        visibility: "friends", campaign: { level, name, difficulty: Math.floor(rand() * 4) },
      },
      since: new Date(this.world.now).toISOString(),
      last_activity: Math.floor(this.world.now / 1000),
      state: "open",
      players: { [host.xuid]: "public" },
    };
    this.all.set(id, s);
    this.joined.set(id, { guests: [], first: null });
    return s;
  }

  /** A guest joins a co-op session: named on the roster, and the host
   *  measures its round trip. */
  private joinCoop(s: MockSession): void {
    const slots = s.advertisement.slots;
    const who = this.free();
    if (slots.public_open < 1 || !who.length) return;
    const guest = pick(who);
    slots.public_open -= 1; slots.public_filled += 1;
    s.players[guest.xuid] = "public";
    s.advertisement.ping_ms = 18 + Math.floor(rand() * 110);
    const j = this.joined.get(s.session_id);
    if (j) {
      if (!j.guests.includes(guest.xuid)) j.guests.push(guest.xuid);
      j.first ??= new Date(this.world.now).toISOString();
    }
  }

  private leaveCoop(s: MockSession): void {
    const guest = Object.keys(s.players).find((x) => x !== s.host_xuid);
    if (!guest) return;
    delete s.players[guest];
    s.advertisement.slots.public_open += 1; s.advertisement.slots.public_filled -= 1;
    delete s.advertisement.ping_ms;
  }

  /** A co-op session closes: a finished game when a guest ever joined. */
  private closeCoop(s: MockSession, reason: MockCoopGame["end_reason"]): void {
    const j = this.joined.get(s.session_id);
    this.joined.delete(s.session_id);
    if (!j?.first || !s.advertisement.campaign) return;
    this.coopGames.unshift({
      session_id: s.session_id, host: this.tag(s.host_xuid),
      players: [s.host_xuid, ...j.guests].map((x) => this.tag(x)),
      campaign: { ...s.advertisement.campaign }, started: j.first, ended: new Date(this.world.now).toISOString(),
      end_reason: reason, visibility: s.advertisement.visibility,
    });
    this.coopGames.splice(200);
  }

  /** Finished co-op games from before the mock started, oldest last. */
  private seedCoopHistory(n: number): void {
    const accounts = [...this.world.accounts.values()];
    if (accounts.length < 2) return;
    let ended = this.world.now;
    for (let i = 0; i < n; i++) {
      ended -= (20 + Math.floor(rand() * 90)) * 60_000;
      const host = pick(accounts);
      const guest = pick(accounts.filter((a) => a.xuid !== host.xuid));
      const [level, name] = pick(COOP_LEVELS);
      this.coopGames.push({
        session_id: hex(16), host: this.tag(host.xuid), players: [this.tag(host.xuid), this.tag(guest.xuid)],
        campaign: { level, name, difficulty: Math.floor(rand() * 4) },
        started: new Date(ended - (12 + Math.floor(rand() * 40)) * 60_000).toISOString(),
        ended: new Date(ended).toISOString(),
        end_reason: rand() < 0.8 ? "closed" : "host_left",
        visibility: rand() < 0.6 ? "friends" : "public",
      });
    }
  }

  private start(): void {
    if (rand() < 0.25) { this.startCoop(); return; }
    const free = this.free();
    if (!free.length) return;
    const host = pick(free);
    const [name, flags, size] = pick(GAMES);
    const id = hex(16);
    // A private slot or two on some custom games.
    const priv = flags === "custom" && rand() < 0.4 ? 2 : 0;
    const s: MockSession = {
      session_id: id,
      host: `acct:${host.xuid}@${HOST}`, host_xuid: host.xuid, host_gamertag: host.gamertag,
      xnkid: id,
      // TEST-NET-3: a documentation address, not anybody's.
      endpoint: { addr: `203.0.113.${1 + Math.floor(rand() * 250)}`, port: 3074, source: "observed" },
      advertisement: {
        addr: null, port: 3074, host_address: Array.from({ length: 36 }, () => 0),
        slots: { public_filled: 1, public_open: size - priv - 1, private_filled: 0, private_open: priv },
        attributes: {}, flags, hopper_id: flags === "custom" ? null : 1 + Math.floor(rand() * 6),
        map: pick(MAPS).name, name, language: "en", visibility: "public",
      },
      since: new Date(this.world.now).toISOString(),
      last_activity: Math.floor(this.world.now / 1000),
      state: "open",
      players: { [host.xuid]: "public" },
    };
    const custom = this.custom?.(id, s.advertisement.map!);
    if (custom) s.advertisement.map_id = custom;
    this.all.set(id, s);
  }

  /** Someone takes a public slot. The slots are the host's own count; the
   *  roster only names the players the host reported, so (as on a real
   *  server) it can be shorter. The mock names one while accounts are spare. */
  private join(s: MockSession): void {
    if (s.advertisement.flags === "coop") return this.joinCoop(s);
    const slots = s.advertisement.slots;
    if (slots.public_open < 1) return;
    slots.public_open -= 1; slots.public_filled += 1;
    const who = this.free();
    if (who.length > this.target && rand() < 0.7) s.players[pick(who).xuid] = "public";
  }

  private leave(s: MockSession): void {
    if (s.advertisement.flags === "coop") return this.leaveCoop(s);
    const slots = s.advertisement.slots;
    if (slots.public_filled < 2) return;
    const named = Object.keys(s.players).filter((x) => x !== s.host_xuid);
    // An unnamed player goes first; a named one when only those are left.
    if (named.length && named.length + 1 >= slots.public_filled) delete s.players[pick(named)];
    slots.public_filled -= 1; slots.public_open += 1;
  }

  /** Some history to start with: sessions of different ages and fullness. */
  seed(): void {
    this.seedCoopHistory(6);
    for (let i = 0; i < this.target; i++) {
      // The first two are co-op: one with its guest in (and a ping), one
      // waiting for a guest.
      if (i < 2) {
        const c = this.startCoop();
        if (!c) break;
        c.since = new Date(this.world.now - Math.floor(rand() * 15 * 60_000)).toISOString();
        if (i === 0) this.joinCoop(c);
        continue;
      }
      this.start();
      const s = [...this.all.values()].pop();
      if (!s) break;
      s.since = new Date(this.world.now - Math.floor(rand() * 25 * 60_000)).toISOString();
      const n = Math.floor(rand() * (s.advertisement.slots.public_open + 1));
      for (let k = 0; k < n; k++) this.join(s);
    }
  }

  /** One step of coming and going; call every couple of seconds. */
  step(): void {
    const open = this.open();
    for (const s of open) {
      const r = rand();
      if (r < 0.3) this.join(s);
      else if (r < 0.42) this.leave(s);
      else if (r < 0.45 && open.length >= this.target) {
        s.state = rand() < 0.5 ? "deleted" : "expired";
        if (s.advertisement.flags === "coop") this.closeCoop(s, s.state === "expired" ? "expired" : "closed");
      } else continue;
      s.last_activity = Math.floor(this.world.now / 1000);
    }
    if (this.open().length < this.target && rand() < 0.5) this.start();
    // Closed ones stay readable by id for a while, as on the server, but not for ever.
    const closed = [...this.all.values()].filter((s) => s.state !== "open");
    for (const s of closed.slice(0, Math.max(0, closed.length - 20))) this.all.delete(s.session_id);
  }

  handle(req: Request, u: URL): Reply | null {
    if (req.method !== "GET") return null;
    const position = this.world.log.length;
    if (u.pathname === "/v1/sessions") {
      const q = u.searchParams;
      const state = q.get("state") ?? "open", map = q.get("map"), host = q.get("host")?.toLowerCase(), after = q.get("cursor") ?? "";
      const limit = Number(q.get("limit") ?? 100);
      if (!Number.isInteger(limit) || limit < 1 || limit > 1000) return { status: 400, body: { error: "invalid", message: "session limit is 1..1000" } };
      const hopper = q.get("hopper"), flags = q.get("flags");
      const items = [...this.all.values()]
        .filter((s) => s.state === state && s.session_id > after
          && (!map || s.advertisement.map === map)
          && (!hopper || s.advertisement.hopper_id === Number(hopper))
          && (!flags || s.advertisement.flags === flags)
          && (!host || s.host === host || s.host_gamertag.toLowerCase() === host))
        .sort((a, b) => a.session_id.localeCompare(b.session_id));
      const page = items.slice(0, limit);
      return { status: 200, body: { items: page, next: items.length > page.length ? page[page.length - 1]!.session_id : null, position } };
    }
    if (u.pathname === "/v1/coop/games") return this.coopPage(u.searchParams, position);
    const m = u.pathname.match(/^\/v1\/sessions\/([^/]+)$/);
    if (!m) return null;
    const s = this.all.get(m[1]!);
    return s ? { status: 200, body: { session: s, position } } : { status: 404, body: { error: "not_found", message: "no such session" } };
  }

  /** `GET /v1/coop/games`: newest first; `player` (gamertag or XUID) keeps
   *  the games it hosted or joined; the cursor is an offset. */
  private coopPage(q: URLSearchParams, position: number): Reply {
    const limit = Math.min(1000, Math.max(1, Number(q.get("limit") || 100) || 100));
    const from = Number(q.get("cursor") || 0);
    if (!Number.isInteger(from) || from < 0) return { status: 400, body: { error: "invalid", message: "bad cursor" } };
    const p = q.get("player")?.toLowerCase();
    const all = this.coopGames.filter((g) => !p || g.players.some((x) => x.xuid === p || x.gamertag.toLowerCase() === p));
    const items = all.slice(from, from + limit);
    return { status: 200, body: { items, next: from + limit < all.length ? String(from + limit) : null, position } };
  }
}
