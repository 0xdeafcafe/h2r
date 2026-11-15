// Mock h2 live server: the docs/live-api.md endpoints and both WebSockets,
// backed by a simulated world. `bun run mock` (port 8075, or H2_MOCK_PORT).
// H2_MOCK_TAMPER=<n> corrupts every nth event's data to show bad signatures.
// H2_MOCK_STATIC=<dir> also serves a built app (SPA fallback to index.html).
// H2_MOCK_GAME_ART=1 serves a fake /art/game manifest (see mock/gameart.ts);
// without it /art/game answers 404, like a server with no game art.

import type { ServerWebSocket } from "bun";
import type { Frame, WireEvent } from "../src/api/types";
import { World, HOST } from "./world";
import { createHash } from "node:crypto";
import { openAccountRequest } from "./account";
import { mockGameArt } from "./gameart";

const port = Number(process.env.H2_MOCK_PORT ?? 8075);
const world = new World();
world.tamperEvery = Number(process.env.H2_MOCK_TAMPER ?? 0);
const historyMin = Number(process.env.H2_MOCK_HISTORY_MIN ?? 20);
const t0 = performance.now();
world.seed(historyMin * 60_000);
console.log(`[mock] seeded ${world.log.length} events (${historyMin} min of history) in ${Math.round(performance.now() - t0)} ms`);
world.run();

const staticDir = process.env.H2_MOCK_STATIC;
const art = process.env.H2_MOCK_GAME_ART && process.env.H2_MOCK_GAME_ART !== "0" ? mockGameArt(new URL("../public/art", import.meta.url).pathname) : null;

interface StreamData {
  kind: "stream";
  games: Set<string> | null;
  from: number | null;
  exclude: Set<string> | null;
  types: Set<string> | null;
}

function wants(d: StreamData, type: string) {
  return (!d.exclude || !d.exclude.has(type)) && (!d.types || d.types.has(type));
}
const csv = (v: string | null) => (v ? new Set(v.split(",").filter(Boolean)) : null);
interface IngestData { kind: "ingest"; authed: boolean }
type WsData = StreamData | IngestData;

const readers = new Set<ServerWebSocket<WsData>>();

// The message of the day (GET/PUT /v1/titles/4D530064/motd), a sample.
let motd = {
  header: "Welcome to h2 live on the mock server",
  messages: [
    "The stats site updates as games are played: open a live game to watch it on the map.",
    "Every event on this server is signed. The Signed Event Log shows each one and checks it.",
    "Melee from behind is an instant kill. Watch your back in close quarters.",
    "Grenades bounce. Bank a plasma grenade off a wall to reach round a corner.",
    "You can dual wield most small arms. Hold Y over a second one to pick it up in your other hand.",
  ],
};

world.listeners.add({
  onEvent(ev: WireEvent) {
    const msg = JSON.stringify({ kind: "event", event: ev });
    for (const ws of readers) if (wants(ws.data as StreamData, ev.type)) ws.send(msg);
  },
  onFrame(f: Frame) {
    const msg = JSON.stringify({ kind: "frame", frame: f });
    for (const ws of readers) {
      const d = ws.data as StreamData;
      if (!d.games || d.games.has(f.game_id)) ws.send(msg);
    }
  },
});

const CORS = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, HEAD, POST, OPTIONS",
  "access-control-allow-headers": "content-type, authorization",
};

function json(body: unknown, status = 200, extra: Record<string, string> = {}) {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json", ...CORS, ...extra } });
}
function err(status: number, error: string, message: string) {
  return json({ error, message }, status);
}
function list(items: unknown[], next: string | null = null) {
  return json({ items, next, position: world.log.length });
}
function limitOf(u: URL) {
  return Math.min(1000, Math.max(1, Number(u.searchParams.get("limit") ?? 100) || 100));
}

async function handle(req: Request, server: Bun.Server<WsData>): Promise<Response | undefined> {
  const u = new URL(req.url);
  const path = u.pathname;
  if (req.method === "OPTIONS") return new Response(null, { status: 204, headers: CORS });

  if (path === "/healthz") return json({ ok: true, position: world.log.length });
  if (path === "/.well-known/h2live.json") {
    return json({
      // Relative, resolved against this document's URL (contract clarification 12).
      id: HOST, api: "/v1", keys: [world.serverKey.jwk],
      software: "h2recomp web mock 0.1", federation: false,
    });
  }

  if (path === "/v1/ingest") {
    if (server.upgrade(req, { data: { kind: "ingest", authed: false } satisfies IngestData })) return undefined;
    return err(400, "invalid", "expected a WebSocket upgrade");
  }

  let m: RegExpMatchArray | null;
  if (path === "/v1/accounts" && req.method === "GET") {
    return list([...world.accounts.values()].map((a) => world.publicAccount(a)).slice(0, limitOf(u)));
  }
  if (path === "/v1/accounts" && req.method === "POST") {
    // A JWS signed by the new key over {gamertag, jwk, invite?, aud, iat}
    // (live-proto AccountRequest), sent as application/jose.
    const opened = await openAccountRequest(await req.text(), HOST, Math.floor(Date.now() / 1000));
    if ("error" in opened) return err(400, "invalid", opened.error);
    const tag = opened.gamertag;
    if (!/^[A-Za-z][A-Za-z0-9]*( [A-Za-z0-9]+)*$/.test(tag) || tag.length > 15) return err(400, "invalid", "that gamertag isn't valid");
    if (world.accountByTag(tag)) return err(409, "conflict", "gamertag taken");
    const a = world.createExternalAccount(tag, opened.jwk);
    return json({ xuid: a.xuid, gamertag: a.gamertag, id: `acct:${a.xuid}@${HOST}` }, 201);
  }
  if ((m = path.match(/^\/v1\/accounts\/by-xuid\/([0-9a-fA-F]{16})\/keys$/))) {
    const a = world.accounts.get(m[1]!.toLowerCase());
    return a ? json(world.accountKeys(a)) : err(404, "not_found", "no such account");
  }
  if ((m = path.match(/^\/v1\/accounts\/([^/]+)\/keys$/))) {
    const a = world.accountByTag(decodeURIComponent(m[1]!));
    return a ? json(world.accountKeys(a)) : err(404, "not_found", "no such account");
  }
  if ((m = path.match(/^\/v1\/accounts\/([^/]+)$/))) {
    const a = world.accountByTag(decodeURIComponent(m[1]!));
    if (!a) return err(404, "not_found", "no such account");
    const recent = [...world.games.values()].filter((g) => g.players.some((p) => p.p.xuid === a.xuid)).slice(-20).reverse();
    return json({ ...world.publicAccount(a), recent_games: recent.map((g) => world.gameSummary(g)), position: world.log.length });
  }
  if (path === "/v1/presence") {
    return list([...world.accounts.values()].filter((a) => a.online).map((a) => ({
      xuid: a.xuid, gamertag: a.gamertag, state: a.state, game_id: a.game_id ?? undefined, since: a.since,
    })));
  }
  if (path === "/v1/games" && req.method === "GET") {
    const state = u.searchParams.get("state");
    const all = [...world.games.values()].reverse().filter((g) => !state || (state === "live" ? !g.ended_at : !!g.ended_at));
    return list(all.slice(0, limitOf(u)).map((g) => world.gameSummary(g)));
  }
  if (path === "/v1/games" && req.method === "POST") {
    // Mock only: accepts GameStarted data and appends it unsigned by a console.
    const b = (await req.json().catch(() => null)) as Record<string, unknown> | null;
    if (!b || typeof b.map !== "string") return err(400, "invalid", "GameStarted data expected");
    const id = crypto.randomUUID().replace(/-/g, "").slice(0, 16);
    world.append(`game-${id}`, [{ type: "GameStarted", data: { ...b, game_id: id } }], null);
    return json({ game_id: id }, 201);
  }
  if ((m = path.match(/^\/v1\/games\/([^/]+)\/events$/)) && req.method === "POST") {
    const b = (await req.json().catch(() => null)) as Array<{ type: string; data: unknown }> | null;
    if (!Array.isArray(b)) return err(400, "invalid", "array of {type, data} expected");
    world.append(`game-${m[1]}`, b, null);
    return json({ ok: true, position: world.log.length });
  }
  if ((m = path.match(/^\/v1\/games\/([^/]+)\/frames$/))) {
    const g = world.games.get(m[1]!);
    if (!g) return err(404, "not_found", "no such game");
    // Oldest first, at most 600 per page; the cursor is the last tick sent.
    const cursor = u.searchParams.get("cursor");
    const from = cursor !== null ? Number(cursor) + 1 : Number(u.searchParams.get("from_tick") ?? -Infinity);
    const to = Number(u.searchParams.get("to_tick") ?? Infinity);
    const all = g.frames.filter((f) => f.tick >= from && f.tick <= to);
    const page = all.slice(0, 600);
    return list(page, all.length > page.length ? String(page[page.length - 1]!.tick) : null);
  }
  if ((m = path.match(/^\/v1\/games\/([^/]+)$/))) {
    const g = world.games.get(m[1]!);
    return g ? json(world.gameDetail(g)) : err(404, "not_found", "no such game");
  }
  if ((m = path.match(/^\/v1\/maps\/([^/]+)\/outline$/))) {
    const name = decodeURIComponent(m[1]!);
    if (req.method === "PUT") {
      world.outlines.set(name, await req.json());
      return json({ ok: true, position: world.log.length });
    }
    const o = world.outlines.get(name);
    return o ? json({ ...o, position: world.log.length }) : err(404, "not_found", "no outline uploaded for this map yet");
  }
  if (path === "/v1/events") {
    // `from` is exclusive; `cursor` (from a previous `next`) is the next `from`.
    const from = Number(u.searchParams.get("cursor") ?? u.searchParams.get("from") ?? 0);
    const type = u.searchParams.get("type");
    const stream = u.searchParams.get("stream");
    const limit = limitOf(u);
    const items: WireEvent[] = [];
    let next: string | null = null;
    for (let i = Math.max(0, from); i < world.log.length; i++) {
      const ev = world.log[i]!;
      if (type && ev.type !== type) continue;
      if (stream && ev.stream !== stream) continue;
      if (items.length === limit) { next = String(items[items.length - 1]!.position); break; }
      items.push(ev);
    }
    return list(items, next);
  }
  if ((m = path.match(/^\/v1\/titles\/([^/]+)\/motd$/))) {
    // Mock only: no token check on PUT; the doc is kept in memory.
    if (req.method === "PUT") {
      const b = (await req.json().catch(() => null)) as typeof motd | null;
      if (!b || typeof b.header !== "string" || !Array.isArray(b.messages)) return err(400, "invalid", "{header, messages} expected");
      motd = { header: b.header, messages: b.messages.map(String) };
      const bytes = JSON.stringify(motd);
      return json({ published: { name: "message_of_the_day.dat", sha256: createHash("sha256").update(bytes).digest("hex"), size: bytes.length }, motd });
    }
    return m[1]!.toUpperCase() === "4D530064" ? json(motd) : err(404, "not_found", "no message of the day");
  }
  if ((m = path.match(/^\/v1\/titles\/([^/]+)\/files\/([^/]+)$/))) {
    const key = `${m[1]!.toLowerCase()}/${m[2]}`;
    if (req.method === "PUT") {
      const bytes = new Uint8Array(await req.arrayBuffer());
      const sha256 = createHash("sha256").update(bytes).digest("hex");
      world.files.set(key, { bytes, sha256 });
      world.append(`title-${m[1]!.toLowerCase()}-file-${m[2]}`, [{ type: "TitleFilePublished", data: { title_id: m[1], name: m[2], sha256, size: bytes.length } }], null);
      return json({ title_id: m[1], name: m[2], sha256, size: bytes.length });
    }
    const f = world.files.get(key);
    return f ? new Response(f.bytes, { headers: { etag: f.sha256, ...CORS } }) : err(404, "not_found", "no such file");
  }
  if (path.startsWith("/v1/")) return err(404, "not_found", `no route ${req.method} ${path}`);

  if (path.startsWith("/art/game/")) {
    const name = decodeURIComponent(path.slice("/art/game/".length));
    if (art && name === "manifest.json") return json(art.manifest);
    const f = art ? await art.file(name) : null;
    return f
      ? new Response(f.bytes as Uint8Array<ArrayBuffer>, { headers: { "content-type": f.type, "cache-control": "max-age=3600", ...CORS } })
      : err(404, "not_found", "no game art");
  }

  if (staticDir) {
    const file = Bun.file(staticDir + (path === "/" ? "/index.html" : path));
    if (await file.exists()) return new Response(file);
    return new Response(Bun.file(staticDir + "/index.html"));
  }
  return new Response("h2 live mock: API only. Run `bun run dev` for the app.\n", { status: 404 });
}

// The stream is upgraded here so `from` travels in the socket data to `open`.
const server = Bun.serve<WsData>({
  port,
  hostname: "127.0.0.1",
  async fetch(req, srv) {
    const u = new URL(req.url);
    if (u.pathname === "/v1/stream") {
      const from = u.searchParams.get("from");
      const games = u.searchParams.get("games");
      const data: StreamData = {
        kind: "stream", games: csv(games), from: from === null ? null : Number(from),
        exclude: csv(u.searchParams.get("exclude")), types: csv(u.searchParams.get("types")),
      };
      if (srv.upgrade(req, { data })) return undefined;
      return err(400, "invalid", "expected a WebSocket upgrade");
    }
    return handle(req, srv);
  },
  websocket: {
    open(ws) {
      if (ws.data.kind !== "stream") return;
      const from = ws.data.from;
      // Replay events after `from` (exclusive), then caught_up, then live.
      const d = ws.data as StreamData;
      const start = from === null ? world.log.length : Math.max(0, from);
      for (let i = start; i < world.log.length; i++) {
        const ev = world.log[i]!;
        if (wants(d, ev.type)) ws.send(JSON.stringify({ kind: "event", event: ev }));
      }
      ws.send(JSON.stringify({ kind: "caught_up", position: world.log.length }));
      readers.add(ws);
    },
    message(ws, raw) {
      if (ws.data.kind !== "ingest") return;
      let msg: { kind?: string; token?: string; frame?: Frame; game_id?: string; events?: Array<{ type: string; data: unknown }> };
      try { msg = JSON.parse(String(raw)); } catch { return; }
      const d = ws.data as IngestData;
      if (!d.authed) {
        if (msg.kind === "auth" && msg.token) { d.authed = true; return; }
        ws.close(4401, "first message must be auth");
        return;
      }
      if (msg.kind === "frame" && msg.frame) {
        const s = JSON.stringify({ kind: "frame", frame: msg.frame });
        for (const r of readers) r.send(s);
      } else if (msg.kind === "events" && msg.game_id && msg.events) {
        world.append(`game-${msg.game_id}`, msg.events, null);
      }
    },
    close(ws) {
      readers.delete(ws);
    },
  },
});

console.log(`[mock] h2 live mock on http://${server.hostname}:${server.port} (${world.liveGames().length} live games)`);
