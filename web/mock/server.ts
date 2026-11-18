// Mock h2 live server: the docs/live/live-api.md endpoints and both WebSockets,
// backed by a simulated world. `bun run mock` (port 8075, or H2_MOCK_PORT).
// H2_MOCK_TAMPER=<n> corrupts every nth event's data to show bad signatures.
// H2_MOCK_STATIC=<dir> also serves a built app (SPA fallback to index.html).
// H2_MOCK_GAME_ART=1 serves a fake /art/game manifest (see mock/gameart.ts);
// without it /art/game has no pictures, like a server with no game art.
// H2_MOCK_GAME_ART_DIR=<dir> serves a real extraction (`h2 live assets
// extract --out <dir>`, from your own disc) at /art/game instead, as is,
// for looking at the pages with it; nothing of it is kept.
// The mock's own map geometry (mock/arena.ts: map.glb, views.json and view
// pictures for the maps that have an arena) is served under /art/game either
// way, in a manifest of its own when there is no game art;
// H2_MOCK_MAP_GEOMETRY=0 turns it off, and then /art/game answers 404.
// H2_MOCK_MODELS=1 adds stand-in player and weapon models (mock/models.ts:
// block figures, skinned, with a few animation clips) to the manifest;
// H2_MOCK_MODELS=static makes them the static version 1 kind; without it
// players are capsules.
// H2_MOCK_FURNITURE=0 leaves out the map furniture (mock/fixtures.ts: a
// moving lift platform and a walker in the frames' `machines`, teleporters
// with links and a gravity lift in each glb's extras).
// H2_MOCK_ACTION=0 leaves stances, actions and damage (mock/action.ts) out
// of the frames, like a sender from before they existed.
// H2_MOCK_ACTORS=1 puts the campaign's AI and vehicles (mock/campaign.ts)
// into every game's frames, not only games whose mode is "campaign".
// H2_MOCK_ADMIN=1 makes every account signed up in the browser an admin, for
// the dashboard's server screen and the stats site's admin area (mock/admin.ts).
// H2_MOCK_INDOOR=1 shuts each arena's glb inside a busy building (walls, a
// ceiling, an upper floor, pillars, machines), for the 3D view's cutaways.

import type { ServerWebSocket } from "bun";
import type { Frame, WireEvent } from "../src/api/types";
import { World, HOST } from "./world";
import { createHash } from "node:crypto";
import { openAccountRequest } from "./account";
import { mockGameArt } from "./gameart";
import { Social } from "./social";
import { Sessions } from "./sessions";
import { streamAuthStep } from "./streamauth";
import { mockMapGeometry } from "./arena";
import { mockModels } from "./models";
import { campaignHook } from "./campaign";
import { actionHooks } from "./action";
import { machineHooks } from "./fixtures";
import { Admin } from "./admin";
import { MockRecovery } from "./recovery";
import { Content, customFor } from "./content";
import { Reports } from "./reports";
import { Bans } from "./bans";
import { Mutes } from "./mutes";
import { CloudSaves } from "./cloudsaves";
import { hasAllSkulls, parseSkullsParam } from "../src/lib/skulls";
import { StatsFold, archive } from "./stats";

const port = Number(process.env.H2_MOCK_PORT ?? 8075);
const world = new World();
world.tamperEvery = Number(process.env.H2_MOCK_TAMPER ?? 0);
// Some games on custom map packages (mock/content.ts); H2_MOCK_CUSTOM_MAPS=0 for none.
const customMaps = process.env.H2_MOCK_CUSTOM_MAPS !== "0";
if (customMaps) world.customMap = customFor;
const historyMin = Number(process.env.H2_MOCK_HISTORY_MIN ?? 20);
const t0 = performance.now();
world.seed(historyMin * 60_000);
console.log(`[mock] seeded ${world.log.length} events (${historyMin} min of history) in ${Math.round(performance.now() - t0)} ms`);
// Friends and teams (mock/social.ts): seeded before the world goes live.
const social = new Social(world);
social.seed();
// H2_MOCK_HOPPER=1: a stand-in playlist file (GET/PUT /v1/titles/4D530064/hopper)
// so the Halo admin's playlist builder has something to edit. Invented
// playlists and plain numbers: no game data. Without it the endpoint is a
// 404, like a server where nobody has imported the game's file yet.
const MOCK_PLAYLISTS: Array<[string, string, Array<[number, string, number, boolean]>]> = [
  ["Rumble Pit", "Every player for themselves.", [[1, "Slayer", 2, false], [2, "Oddball", 3, false]]],
  ["Team Rumble", "Small teams, quick games.", [[3, "Team Slayer", 2, true], [1, "Team CTF", 1, true], [4, "Team King", 4, true]]],
  ["Big Team Picnic", "Large teams on large maps.", [[5, "Team Assault", 9, true], [6, "Territories", 8, true]]],
  ["Doubles", "Two on two.", [[2, "Doubles Slayer", 2, true]]],
];
let hoppers = Array.from({ length: 16 }, (_, index) => {
  const p = MOCK_PLAYLISTS[index];
  const recs = (p?.[2] ?? [[1, "Slayer", 2, false]]).map(([id, name, engine, team]) => ({
    id, string: "", flags: 0, dwords: [100, 0] as [number, number], variant: { name, engine, team },
  }));
  return {
    index, type: p ? 2 : 1, status: 1, record_count: recs.length, name: p ? `playlist_${index}` : "",
    title: p?.[0] ?? `Hidden ${index + 1}`, description: p?.[1] ?? "", ranges: [0, 0, 0] as [number, number, number],
    dwords: Array.from({ length: 18 }, () => 0), records: recs,
  };
});

world.listeners.add({ onEvent: (ev) => { if (ev.type === "AccountCreated") social.welcome((ev.data as { xuid: string }).xuid); }, onFrame: () => {} });
// Open sessions for the dashboard's "find a game" (mock/sessions.ts).
// The stats summary (mock/stats.ts): a made-up month before the simulated
// history, then the log, folded as it grows.
const stats = new StatsFold();
let statsAt = 0;
if (process.env.H2_MOCK_STATS_ARCHIVE !== "0") {
  const first = Date.parse(world.log.find((e) => e.stream.startsWith("game-"))?.at ?? new Date().toISOString());
  for (const e of archive([...world.accounts.values()], first)) stats.apply(e);
}
function statsSummary(window: string) {
  for (; statsAt < world.log.length; statsAt++) stats.apply(world.log[statsAt]!);
  return stats.summary(window, Date.now(), world.log.length);
}

const sessions = new Sessions(world, Math.max(0, Number(process.env.H2_MOCK_SESSIONS ?? 5) || 0), customMaps ? customFor : undefined);
sessions.seed();
setInterval(() => sessions.step(), 2500);
// AI and vehicles in the frames of campaign games; H2_MOCK_ACTORS=1 puts them in every game.
const actorsEverywhere = !!process.env.H2_MOCK_ACTORS && process.env.H2_MOCK_ACTORS !== "0";
world.frameHooks.push(campaignHook(actorsEverywhere ? () => true : undefined));
// Stances, actions and damage in every game's frames (mock/action.ts); H2_MOCK_ACTION=0 leaves them out, like an old sender.
if (process.env.H2_MOCK_ACTION !== "0") {
  const action = actionHooks();
  world.frameHooks.push(action.frame);
  world.recordHooks.push(action.record);
}
// Map furniture (mock/fixtures.ts): a lift platform and a walker in the frames' `machines`, teleporters and a lift in the glb; H2_MOCK_FURNITURE=0 for none.
const furniture = process.env.H2_MOCK_FURNITURE !== "0";
if (furniture) {
  const machines = machineHooks();
  world.frameHooks.push(machines.frame);
  world.recordHooks.push(machines.record);
}
world.run();

const admin = new Admin(world, !!process.env.H2_MOCK_ADMIN && process.env.H2_MOCK_ADMIN !== "0");
// Account recovery (mock/recovery.ts): TOTP codes and taking an account over.
const recovery = new MockRecovery(world);
const content = new Content(world, admin);
// Player reports and the admin queue (mock/reports.ts).
const reports = new Reports(world, admin);
// Bans, ban notices and commendation counts (mock/bans.ts).
const bans = new Bans(world, admin, reports);
// Mute lists (mock/mutes.ts).
const mutes = new Mutes(world, admin);
// Cloud saves (mock/cloudsaves.ts).
const cloudSaves = new CloudSaves(world);
if (admin.everyoneAdmin) console.log("[mock] H2_MOCK_ADMIN: accounts signed up in the browser are admins");
const staticDir = process.env.H2_MOCK_STATIC;
const artDir = process.env.H2_MOCK_GAME_ART_DIR || null;
const art = process.env.H2_MOCK_GAME_ART && process.env.H2_MOCK_GAME_ART !== "0" ? mockGameArt(new URL("../public/art", import.meta.url).pathname) : null;
const indoor = !!process.env.H2_MOCK_INDOOR && process.env.H2_MOCK_INDOOR !== "0";
const geometry = process.env.H2_MOCK_MAP_GEOMETRY === "0" ? null : mockMapGeometry({ indoor, furniture });
const models = process.env.H2_MOCK_MODELS && process.env.H2_MOCK_MODELS !== "0" ? mockModels(process.env.H2_MOCK_MODELS !== "static") : null;

interface StreamData {
  kind: "stream";
  games: Set<string> | null;
  from: number | null;
  exclude: Set<string> | null;
  types: Set<string> | null;
  /** `?auth=1`: waiting for the auth message before the replay. */
  waiting: boolean;
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

  const rec = await recovery.handle(req, u);
  if (rec) return json(rec.body, rec.status);
  const soc = await social.handle(req, u);
  if (soc) return json(soc.body, soc.status);
  const ses = sessions.handle(req, u);
  if (ses) return json(ses.body, ses.status);
  const adm = await admin.handle(req, u);
  if (adm) return json(adm.body, adm.status);
  const rep = await reports.handle(req, u);
  if (rep) return json(rep.body, rep.status);
  const ban = await bans.handle(req, u);
  if (ban) return json(ban.body, ban.status);
  const mut = mutes.handle(req, u);
  if (mut) return json(mut.body, mut.status);
  const cs = cloudSaves.handle(req, u);
  if (cs) return cs instanceof Response ? cs : json(cs.body, cs.status);
  const con = await content.handle(req, u, (bytes) => new Response(bytes as Uint8Array<ArrayBuffer>, { headers: { "content-type": "image/png", "cache-control": "max-age=3600", ...CORS } }));
  if (con) return con instanceof Response ? con : json(con.body, con.status);

  let m: RegExpMatchArray | null;
  if (path === "/v1/accounts" && req.method === "GET") {
    return list([...world.accounts.values()].map((a) => ({ ...world.publicAccount(a), ...(admin.isAdmin(a) ? { admin: true } : {}) })).slice(0, limitOf(u)));
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
    return json({ xuid: a.xuid, gamertag: a.gamertag, id: `acct:${a.xuid}@${HOST}`, recovery: await recovery.setup(a.xuid, a.gamertag) }, 201);
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
    return json({ ...world.publicAccount(a), ...(admin.isAdmin(a) ? { admin: true } : {}), recent_games: recent.map((g) => world.gameSummary(g)), position: world.log.length });
  }
  if (path === "/v1/stats/summary") {
    const sum = statsSummary(u.searchParams.get("window") ?? "7d");
    return sum ? json(sum) : err(400, "invalid", "window is today, 7d, 30d or all");
  }
  if (path === "/v1/presence") {
    return list([...world.accounts.values()].filter((a) => a.online).map((a) => ({
      xuid: a.xuid, gamertag: a.gamertag, state: a.state, game_id: a.game_id ?? undefined, since: a.since,
    })));
  }
  if (path === "/v1/games" && req.method === "GET") {
    const state = u.searchParams.get("state");
    // `map` is the base map (modded or not); `map_id` one package exactly, or `none` for retail.
    const map = u.searchParams.get("map"), mapId = u.searchParams.get("map_id");
    // `skulls` is a comma-separated list; a game must have had all of them on.
    const skulls = parseSkullsParam(u.searchParams.get("skulls"));
    const all = [...world.games.values()].reverse().filter((g) => !state || (state === "live" ? !g.ended_at : !!g.ended_at))
      .map((g) => world.gameSummary(g) as ReturnType<World["gameSummary"]> & { map_id?: string; skulls?: number[] })
      .filter((g) => (!map || g.map === map) && (!mapId || (mapId === "none" ? !g.map_id : g.map_id === mapId)) && hasAllSkulls(g.skulls, skulls));
    return list(all.slice(0, limitOf(u)));
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
  if ((m = path.match(/^\/v1\/titles\/([^/]+)\/hopper$/)) && process.env.H2_MOCK_HOPPER === "1") {
    // Mock only: no token check; a PUT replaces the hoppers it names (variants kept by position).
    if (req.method === "PUT") {
      type Put = Omit<(typeof hoppers)[number], "records"> & { records: Array<{ id: number; string: string; flags: number; dwords: [number, number]; from?: number }> };
      const b = (await req.json().catch(() => null)) as { hoppers?: Put[] } | null;
      if (!b || !Array.isArray(b.hoppers)) return err(400, "invalid", "{hoppers} expected");
      for (const h of b.hoppers) {
        const old = hoppers[h.index];
        if (!old) continue;
        hoppers[h.index] = { ...h, record_count: h.records.length, records: h.records.map(({ from: _from, ...r }, i) => ({ ...r, variant: old.records[_from ?? i]?.variant ?? old.records[0]!.variant })) };
      }
      const bytes = JSON.stringify(hoppers);
      return json({ hoppers, published: { name: "multiplayer_hopper.dat", sha256: createHash("sha256").update(bytes).digest("hex"), size: bytes.length } });
    }
    return json({ hoppers });
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
    if (artDir) {
      const file = Bun.file(`${artDir}/${name}`);
      const safe = name.split("/").every((seg) => seg && seg !== "." && seg !== ".." && !seg.includes("\\"));
      return safe && (await file.exists())
        ? new Response(file, { headers: { "cache-control": "no-cache", ...CORS } })
        : err(404, "not_found", "no game art");
    }
    if (name === "manifest.json" && (art || geometry || models)) {
      const base = art?.manifest ?? { version: 1, source: "mock-geometry", images: {} };
      return json({ ...base, ...(geometry ? { maps: geometry.maps } : {}), ...(models ? { models: models.models } : {}) });
    }
    const f = models?.file(name) ?? geometry?.file(name) ?? (art ? await art.file(name) : null);
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
        waiting: ["1", "true"].includes(u.searchParams.get("auth") ?? ""),
      };
      if (srv.upgrade(req, { data })) return undefined;
      return err(400, "invalid", "expected a WebSocket upgrade");
    }
    return handle(req, srv);
  },
  websocket: {
    open(ws) {
      if (ws.data.kind !== "stream" || ws.data.waiting) return;
      replay(ws);
    },
    message(ws, raw) {
      if (ws.data.kind === "stream") {
        const d = ws.data as StreamData;
        const step = streamAuthStep(d.waiting, raw);
        if (step === "close") ws.close(4401, "first message must be auth");
        else if (step === "replay") { d.waiting = false; replay(ws); }
        return;
      }
      ingestMessage(ws, raw);
    },
    close(ws) {
      readers.delete(ws);
    },
  },
});

/** Replays events after `from` (exclusive), then caught_up, then live. */
function replay(ws: import("bun").ServerWebSocket<WsData>) {
  const d = ws.data as StreamData;
  const from = d.from;
  const start = from === null ? world.log.length : Math.max(0, from);
  for (let i = start; i < world.log.length; i++) {
    const ev = world.log[i]!;
    if (wants(d, ev.type)) ws.send(JSON.stringify({ kind: "event", event: ev }));
  }
  ws.send(JSON.stringify({ kind: "caught_up", position: world.log.length }));
  readers.add(ws);
}

/** An ingest socket's message: auth first, then frames and events. */
function ingestMessage(ws: import("bun").ServerWebSocket<WsData>, raw: string | Buffer) {
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
}

console.log(`[mock] h2 live mock on http://${server.hostname}:${server.port} (${world.liveGames().length} live games)`);
