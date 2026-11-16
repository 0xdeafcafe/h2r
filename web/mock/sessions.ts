// Made-up session advertisements for the mock (docs/live-api.md
// "Sessions"): a handful of open games that people drift in and out of,
// that fill up, close and get replaced, so the dashboard's "find a game"
// screen has something to show without the game. Read-only: `GET
// /v1/sessions` and `GET /v1/sessions/{id}`, in the server's shapes.
// H2_MOCK_SESSIONS=<n> sets how many it keeps open (default 5; 0 for none,
// to see the empty state). The mock writes no session events to the log.

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
    flags: string; hopper_id: number | null; map: string | null; name: string | null; language: string | null;
  };
  since: string;
  last_activity: number;
  state: string;
  players: Record<string, string>;
}

type Reply = { status: number; body: unknown };

const GAMES: Array<[name: string, flags: string, size: number]> = [
  ["Team Slayer", "custom", 8], ["Slayer", "custom", 8], ["Capture the Cushion", "custom", 12], ["Oddball Party", "custom", 6],
  ["Hill Hopping", "custom", 8], ["Big Team Picnic", "hopper", 16], ["Team Rumble", "hopper", 8], ["Rumble", "ranked", 8],
];
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)]!;
const hex = (n: number) => Array.from({ length: n }, () => Math.floor(rand() * 16).toString(16)).join("");

export class Sessions {
  readonly all = new Map<string, MockSession>();

  constructor(private world: World, private target = 5) {}

  private open(): MockSession[] {
    return [...this.all.values()].filter((s) => s.state === "open");
  }

  /** Accounts free to be in a session: not hosting one and not on a roster. */
  private free() {
    const busy = new Set(this.open().flatMap((s) => Object.keys(s.players)));
    return [...this.world.accounts.values()].filter((a) => !busy.has(a.xuid));
  }

  private start(): void {
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
        map: pick(MAPS).name, name, language: "en",
      },
      since: new Date(this.world.now).toISOString(),
      last_activity: Math.floor(this.world.now / 1000),
      state: "open",
      players: { [host.xuid]: "public" },
    };
    this.all.set(id, s);
  }

  /** Someone takes a public slot. The slots are the host's own count; the
   *  roster only names the players the host reported, so (as on a real
   *  server) it can be shorter. The mock names one while accounts are spare. */
  private join(s: MockSession): void {
    const slots = s.advertisement.slots;
    if (slots.public_open < 1) return;
    slots.public_open -= 1; slots.public_filled += 1;
    const who = this.free();
    if (who.length > this.target && rand() < 0.7) s.players[pick(who).xuid] = "public";
  }

  private leave(s: MockSession): void {
    const slots = s.advertisement.slots;
    if (slots.public_filled < 2) return;
    const named = Object.keys(s.players).filter((x) => x !== s.host_xuid);
    // An unnamed player goes first; a named one when only those are left.
    if (named.length && named.length + 1 >= slots.public_filled) delete s.players[pick(named)];
    slots.public_filled -= 1; slots.public_open += 1;
  }

  /** Some history to start with: sessions of different ages and fullness. */
  seed(): void {
    for (let i = 0; i < this.target; i++) {
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
      else if (r < 0.45 && open.length >= this.target) s.state = rand() < 0.5 ? "deleted" : "expired";
      else continue;
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
      const hopper = q.get("hopper");
      const items = [...this.all.values()]
        .filter((s) => s.state === state && s.session_id > after
          && (!map || s.advertisement.map === map)
          && (!hopper || s.advertisement.hopper_id === Number(hopper))
          && (!host || s.host === host || s.host_gamertag.toLowerCase() === host))
        .sort((a, b) => a.session_id.localeCompare(b.session_id));
      const page = items.slice(0, limit);
      return { status: 200, body: { items: page, next: items.length > page.length ? page[page.length - 1]!.session_id : null, position } };
    }
    const m = u.pathname.match(/^\/v1\/sessions\/([^/]+)$/);
    if (!m) return null;
    const s = this.all.get(m[1]!);
    return s ? { status: 200, body: { session: s, position } } : { status: 404, body: { error: "not_found", message: "no such session" } };
  }
}
