// REST helpers. Every URL derives from the chosen server target.

import type { AccountKeyEntry, Frame, GameDetail, GameSummary, MapOutline, WireEvent, AccountSummary, PresenceEntry } from "./types";
import type { SimDetail, SimFilter, SimFilterOptions, SimRun } from "../admin/simRuns";
import type { ServerTarget } from "./server";
import { formatSkullsParam } from "../lib/skulls";
import { openHostFilm } from "./host-film";
import { decodeKeyframes } from "./pack";
import { CHUNK_TYPE, chunkFrames, chunkIndex, chunksIn, type ChunkIndex, type RecordingChunk } from "./chunk";

export class ApiError extends Error {
  constructor(readonly status: number, readonly code: string, message: string) {
    super(message);
  }
}

/** A read sent with a token gets `auth=1` in its URL: the server answers it
 *  `private, no-store`, and a CDN that keys on the URL never hands it the
 *  anonymous copy (or the anonymous reader this one). */
export function withAuthMark(url: string): string {
  const u = new URL(url);
  u.searchParams.set("auth", "1");
  return u.toString();
}

export async function requireOk(r: Response): Promise<Response> {
  if (r.ok) return r;
  let code = "http_" + r.status, message = r.statusText || `HTTP ${r.status}`;
  try {
    const b = await r.json();
    // {error: "<code>", message} (contract clarification 4).
    if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
  } catch { /* not JSON */ }
  throw new ApiError(r.status, code, message);
}

export async function getJson<T>(url: string, signal?: AbortSignal, bearer?: string | null): Promise<T> {
  const headers: Record<string, string> = { accept: "application/json" };
  if (bearer) {
    headers.authorization = `Bearer ${bearer}`;
    url = withAuthMark(url);
  }
  const r = await requireOk(await fetch(url, { signal, headers }));
  return (await r.json()) as T;
}

/** The packed frames container's content type (docs/live/live-telemetry.md). */
export const PACKED_FRAMES = "application/x-h2-frames";

/** Recorded frames, packed: one keyframe record each, the cursor in
 *  `x-next`, following it to the end (bounded). A server that answers JSON
 *  (it predates packed frames) is read as JSON pages. */
export async function packedFrames(
  url: (cursor: string | null) => string,
  fetcher: (url: string, init: RequestInit) => Promise<Response>,
  signal?: AbortSignal,
  maxPages = 200,
): Promise<Frame[]> {
  const out: Frame[] = [];
  let cursor: string | null = null;
  for (let i = 0; i < maxPages; i++) {
    const r = await fetcher(url(cursor), { signal, headers: { accept: `${PACKED_FRAMES}, application/json` } });
    if (!r.ok) {
      let code = "http_" + r.status, message = r.statusText;
      try {
        const b = await r.json();
        if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
      } catch { /* not JSON */ }
      throw new ApiError(r.status, code, message);
    }
    let items: Frame[];
    if ((r.headers.get("content-type") ?? "").startsWith(PACKED_FRAMES)) {
      items = decodeKeyframes(new Uint8Array(await r.arrayBuffer()));
      cursor = r.headers.get("x-next");
    } else {
      const p = page<Frame>(await r.json());
      items = p.items;
      cursor = p.next;
    }
    out.push(...items);
    if (cursor === null || items.length === 0) break;
  }
  return out;
}

/** A list response, `{items, next, position}` (contract clarification 3). */
export interface Page<T> { items: T[]; next: string | null; position: number | null }

export function page<T>(body: unknown): Page<T> {
  const o = body as { items?: unknown; next?: unknown; position?: unknown } | null;
  return {
    items: o && Array.isArray(o.items) ? (o.items as T[]) : [],
    next: o && o.next !== undefined && o.next !== null ? String(o.next) : null,
    position: o && typeof o.position === "number" ? o.position : null,
  };
}

/** A one-use `read` token for whoever reads (the signed-in account), or
 *  null for an anonymous read. */
export type Reader = () => Promise<string | null>;

/** The REST calls against one server. Reads whose answer depends on the
 *  reader (games, accounts, the log, stats: a friends-only co-op game shows
 *  only to who may see it) send `reader`'s token when it gives one. */
export function api(t: ServerTarget, reader: Reader = async () => null) {
  const who = (): Promise<string | null> => reader().catch(() => null);
  const u = (path: string, q?: Record<string, string | number | undefined | null>) => {
    const url = new URL(t.api + path);
    for (const [k, v] of Object.entries(q ?? {})) if (v !== undefined && v !== null && v !== "") url.searchParams.set(k, String(v));
    return url.toString();
  };
  /** Follows `next` cursors to the end (bounded). */
  async function all<T>(path: string, q: Record<string, string | number | undefined | null>, s?: AbortSignal, maxPages = 50): Promise<Page<T>> {
    const out: T[] = [];
    let cursor: string | null = null;
    let position: number | null = null;
    for (let i = 0; i < maxPages; i++) {
      const p: Page<T> = page<T>(await getJson(u(path, { ...q, cursor }), s, await who()));
      out.push(...p.items);
      position ??= p.position;
      // A page can be empty and still have a `next`: the log leaves out
      // events the reader may not see, but its cursor moves past them.
      if (p.next === null || p.next === cursor) { cursor = null; break; }
      cursor = p.next;
    }
    return { items: out, next: cursor, position };
  }
  /** A read with the reader's token, if any. */
  const reading = async (url: string, init: RequestInit): Promise<Response> => {
    const token = await who();
    return token
      ? fetch(withAuthMark(url), { ...init, headers: { ...(init.headers as Record<string, string>), authorization: `Bearer ${token}` } })
      : fetch(url, init);
  };
  async function chunks(id: string, s?: AbortSignal, revision?: number): Promise<ChunkIndex | null> {
    try {
      return chunkIndex(await getJson<unknown>(u("/games/" + encodeURIComponent(id) + "/chunks", { format: "archive", revision }), s, await who()));
    } catch (e) {
      // A server before chunks answers 404 for the route, as a missing
      // game does: read both as no chunks and let `/frames` say which.
      if (e instanceof ApiError && e.status === 404) return null;
      throw e;
    }
  }
  async function chunk(id: string, c: RecordingChunk, s?: AbortSignal): Promise<Uint8Array> {
    const path = c.codec === "archive" ? "/archive/" + c.index + "/" + c.ordinal : "/chunks/" + c.from_tick;
    const accept = c.codec === "archive" ? "application/x-h2-replay" : CHUNK_TYPE;
    const query = c.codec !== "archive" && c.repeatedStart ? {sha256:c.sha256} : undefined;
    const r = await reading(u("/games/" + encodeURIComponent(id) + path, query), { signal: s, headers: { accept } });
    if (!r.ok) throw new ApiError(r.status, "http_" + r.status, `frame chunk ${c.from_tick}: ${r.statusText}`);
    return new Uint8Array(await r.arrayBuffer());
  }
  return {
    hostFilm: (id: string, s?: AbortSignal) => {
      const path = "/games/" + encodeURIComponent(id) + "/film";
      let rev: string | undefined;
      return openHostFilm(async (range, signal) => {
        const query = rev ? {rev, span: range.slice("bytes=".length)} : undefined;
        const response = await reading(u(path, query), {signal, headers: {Range: range}});
        if (response.status === 206) {
          const observed = /^"film-([0-9a-f]{64})"$/.exec(response.headers.get("etag") ?? "")?.[1];
          if (rev && rev !== observed) {
            await response.body?.cancel();
            throw new Error("Host film revision changed during playback");
          }
          if (range === "bytes=0-31") rev = observed;
        }
        return response;
      }, s);
    },
    health: (s?: AbortSignal) => getJson<{ ok: boolean; position: number }>(t.base + "/healthz", s),
    accounts: (s?: AbortSignal) => all<AccountSummary>("/accounts", { limit: 1000 }, s),
    account: async (tag: string, s?: AbortSignal) =>
      getJson<AccountSummary & { recent_games?: GameSummary[]; position?: number }>(u("/accounts/" + encodeURIComponent(tag)), s, await who()),
    /** Keys by XUID (events name their author by XUID). */
    accountKeys: async (xuid: string, s?: AbortSignal) =>
      (await getJson<{ keys?: AccountKeyEntry[] }>(u("/accounts/by-xuid/" + encodeURIComponent(xuid) + "/keys"), s)).keys ?? [],
    presence: (s?: AbortSignal) => all<PresenceEntry>("/presence", {}, s),
    /** `skulls` keeps only games that had all of those skulls on (`?skulls=10,14`). */
    games: async (state: "live" | "ended", limit = 100, s?: AbortSignal, skulls?: readonly number[]) =>
      getJson(u("/games", { state, limit, skulls: skulls ? formatSkullsParam(skulls) : undefined }), s, await who()).then((b) => page<GameSummary>(b)),
    /** One game, as the reader (a private co-op game answers 404 to anyone
     *  who may not see it); `bearer` overrides the reader's token. */
    game: async (id: string, s?: AbortSignal, bearer?: string | null) =>
      getJson<GameDetail>(u("/games/" + encodeURIComponent(id)), s, bearer === undefined ? await who() : bearer),
    simRuns: async (filters: SimFilter, s?: AbortSignal) => {
      const body = await getJson<Page<SimRun> & { options?: SimFilterOptions }>(
        u("/admin/sim-runs", { ...filters }), s, await who(),
      );
      return { ...page<SimRun>(body), options: body.options };
    },
    simRun: async (id: string, s?: AbortSignal) =>
      getJson<SimDetail>(u("/admin/sim-runs/" + encodeURIComponent(id)), s, await who()),
    /** All recorded frames in a tick range: from the game's frame chunks
     *  (docs/live/live-api.md, "Frame chunks"), or, for a game recorded
     *  before them (its index is empty, or the server has none), its
     *  recorded keyframes from `/frames`, following cursors. */
    frames: async (id: string, from_tick?: number, to_tick?: number, s?: AbortSignal, knownIndex?: ChunkIndex | null): Promise<Frame[]> => {
      const index = knownIndex === undefined ? await chunks(id, s) : knownIndex;
      if (index && index.items.length) {
        const frames = await chunkFrames(chunksIn(index.items, from_tick, to_tick), (c) => chunk(id, c, s));
        return frames.filter((f) => (from_tick === undefined || f.tick >= from_tick) && (to_tick === undefined || f.tick <= to_tick));
      }
      return packedFrames(
        (cursor) => u("/games/" + encodeURIComponent(id) + "/frames", { from_tick, to_tick, cursor, format: "packed" }),
        reading,
        s,
      );
    },
    /** A game's frame chunk index, oldest first; null from a server
     *  without chunks. */
    chunks,
    /** One chunk's bytes (decode with `decodeChunk`). */
    chunk,
    outline: async (map: string, s?: AbortSignal): Promise<MapOutline | null> => {
      try {
        return await getJson<MapOutline>(u("/maps/" + encodeURIComponent(map) + "/outline"), s);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    /** Published map packages (docs/game/maps.md §5); one page. `sort`
     *  is `new` (the default) or `popular`; `q` matches name, description,
     *  credit, author and base map. Sent without a token. */
    maps: async (q: { sort?: "new" | "popular"; base?: string; author?: string; category?: string; q?: string; limit?: number; cursor?: string | null } = {}, s?: AbortSignal) =>
      page<unknown>(await getJson(u("/maps", q), s)),
    /** Every published package, following cursors (bounded). */
    allMaps: (q: { base?: string; author?: string } = {}, s?: AbortSignal) => all<unknown>("/maps", { ...q, limit: 100 }, s, 20),
    /** One package by map_id, as status and body: 404 (not a package) and
     *  410 (taken down) are answers, not errors (lib/maps.ts reads them). */
    mapPackage: async (id: string, s?: AbortSignal): Promise<{ status: number; body?: unknown }> => {
      const r = await fetch(u("/maps/" + encodeURIComponent(id)), { signal: s, headers: { accept: "application/json" } });
      let body: unknown;
      try { body = await r.json(); } catch { /* not JSON */ }
      return { status: r.status, body };
    },
    /** A package's preview image (only when its summary has `image`). */
    mapImageUrl: (id: string) => u("/maps/" + encodeURIComponent(id) + "/image"),
    /** Open session advertisements (docs/live/live-api.md "Sessions"), following cursors. */
    sessions: (s?: AbortSignal) => all<unknown>("/sessions", { limit: 100 }, s, 10),
    /** Finished co-op campaign games, newest first (docs/live/live-api.md "Co-op games"); one page.
     *  `player` is a gamertag or XUID. A server without the endpoint answers 404. `bearer`, a
     *  `read` token, says who reads: friends-only games show to their players and the host's
     *  friends, invite-only ones to their players; without it only public ones. */
    coopGames: async (q: { player?: string; limit?: number; cursor?: string } = {}, s?: AbortSignal, bearer?: string | null) =>
      page<unknown>(await getJson(u("/coop/games", q), s, bearer === undefined ? await who() : bearer)),
    /** One session by id, open or not. */
    session: (id: string, s?: AbortSignal) => getJson<{ session: unknown; position?: number }>(u("/sessions/" + encodeURIComponent(id)), s),
    /** A player's level and experience per ranked playlist (docs/live/live-api.md "Ranks"); the raw body. */
    ranks: async (player: string, s?: AbortSignal) => getJson<unknown>(u("/accounts/" + encodeURIComponent(player) + "/ranks"), s, await who()),
    /** How many players commended `player` (a gamertag or XUID), by kind
     *  (docs/live/live-api.md "Bans"); the raw body. Older servers answer 404. */
    commendations: async (player: string, s?: AbortSignal) =>
      getJson<unknown>(u("/accounts/" + encodeURIComponent(player) + "/commendations"), s, await who()),
    /** One playlist's standing and its history, newest first; the raw body. */
    rankHistory: async (player: string, playlist: string, s?: AbortSignal) =>
      getJson<unknown>(u("/accounts/" + encodeURIComponent(player) + "/ranks/" + encodeURIComponent(playlist)), s, await who()),
    /** A ranked playlist's leaderboard; the raw body. */
    leaderboard: async (playlist: string, limit = 100, s?: AbortSignal) =>
      getJson<unknown>(u("/ranks/" + encodeURIComponent(playlist), { limit }), s, await who()),
    /** Which hoppers are clanker ones (`GET /v1/hoppers/clankers`, by
     *  descriptor index); the raw body. */
    clankerHoppers: async (s?: AbortSignal) => getJson<unknown>(u("/hoppers/clankers"), s, await who()),
    /** The hopper file as the editors read it (`GET /v1/titles/4D530064/hopper`);
     *  the raw body. Null when none is published — the server then serves
     *  (and ranks) the built-in lineup. */
    hopperDoc: async (s?: AbortSignal): Promise<unknown | null> => {
      try {
        return await getJson<unknown>(u("/titles/4D530064/hopper"), s);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    /** Server-wide stats over a window (docs/live/live-api.md "Stats summary"); the raw body. */
    statsSummary: async (window: string, s?: AbortSignal) => getJson<unknown>(u("/stats/summary", { window }), s, await who()),
    /** What a page load needs, in one read (`GET /v1/snapshot`); a server
     *  without it answers 404. */
    snapshot: async (s?: AbortSignal) => getJson<SnapshotBody>(u("/snapshot"), s, await who()),
    /** One page of the log; `from` is exclusive. */
    events: async (q: { from?: number; type?: string; stream?: string; limit?: number; cursor?: string; exclude?: string }, s?: AbortSignal) =>
      page<WireEvent>(await getJson(u("/events", q), s, await who())),
    /** Every event of one stream, following cursors; `exclude` leaves out
     *  types (comma-separated, `GameFrame` for a replay that reads its
     *  frames from `/frames`). */
    streamEvents: async (stream: string, s?: AbortSignal, opts: { exclude?: string } = {}) =>
      (await all<WireEvent>("/events", { stream, limit: 1000, exclude: opts.exclude }, s)).items,
    /** The reader stream, resuming after `from` (exclusive), without the 1 Hz GameFrame events. */
    /** `auth`: the first message will be a `read` token (`?auth=1`). */
    streamUrl: (from: number | null, auth = false, game?: string) => u("/stream", {
      from: from ?? 0, exclude: "GameFrame", frames: "packed", frame_version: 8, auth: auth ? 1 : undefined,
      games: game, stream: game ? "game-" + game : undefined,
    }),
    /** The reader's token now, for a stream's auth message. */
    readToken: who,
  };
}

export type Api = ReturnType<typeof api>;

/** `GET /v1/snapshot`: each part as its own endpoint answers it. */
export interface SnapshotBody {
  position: number;
  accounts: Page<AccountSummary>;
  presence: Page<PresenceEntry>;
  live: Page<GameSummary>;
  ended: Page<GameSummary>;
  /** Every live game's detail and the newest ended ones'. */
  details: GameDetail[];
  /** The log before `position`, without GameFrames. */
  tail: WireEvent[];
}
