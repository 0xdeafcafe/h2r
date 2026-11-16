// REST helpers. Every URL derives from the chosen server target.

import type { AccountKeyEntry, Frame, GameDetail, GameSummary, MapOutline, WireEvent, AccountSummary, PresenceEntry } from "./types";
import type { ServerTarget } from "./server";
import { formatSkullsParam } from "../lib/skulls";
import { decodeKeyframes } from "./pack";

export class ApiError extends Error {
  constructor(readonly status: number, readonly code: string, message: string) {
    super(message);
  }
}

async function getJson<T>(url: string, signal?: AbortSignal): Promise<T> {
  const r = await fetch(url, { signal, headers: { accept: "application/json" } });
  if (!r.ok) {
    let code = "http_" + r.status, message = r.statusText;
    try {
      const b = await r.json();
      // {error: "<code>", message} (contract clarification 4).
      if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
    } catch { /* not JSON */ }
    throw new ApiError(r.status, code, message);
  }
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

export function api(t: ServerTarget) {
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
      const p: Page<T> = page<T>(await getJson(u(path, { ...q, cursor }), s));
      out.push(...p.items);
      position ??= p.position;
      cursor = p.next;
      if (cursor === null || p.items.length === 0) break;
    }
    return { items: out, next: cursor, position };
  }
  return {
    health: (s?: AbortSignal) => getJson<{ ok: boolean; position: number }>(t.base + "/healthz", s),
    accounts: (s?: AbortSignal) => all<AccountSummary>("/accounts", { limit: 1000 }, s),
    account: (tag: string, s?: AbortSignal) =>
      getJson<AccountSummary & { recent_games?: GameSummary[]; position?: number }>(u("/accounts/" + encodeURIComponent(tag)), s),
    /** Keys by XUID (events name their author by XUID). */
    accountKeys: async (xuid: string, s?: AbortSignal) =>
      (await getJson<{ keys?: AccountKeyEntry[] }>(u("/accounts/by-xuid/" + encodeURIComponent(xuid) + "/keys"), s)).keys ?? [],
    presence: (s?: AbortSignal) => all<PresenceEntry>("/presence", {}, s),
    /** `skulls` keeps only games that had all of those skulls on (`?skulls=10,14`). */
    games: (state: "live" | "ended", limit = 100, s?: AbortSignal, skulls?: readonly number[]) =>
      getJson(u("/games", { state, limit, skulls: skulls ? formatSkullsParam(skulls) : undefined }), s).then((b) => page<GameSummary>(b)),
    game: (id: string, s?: AbortSignal) => getJson<GameDetail>(u("/games/" + encodeURIComponent(id)), s),
    /** All recorded frames in a tick range, following cursors (600 per page). */
    frames: (id: string, from_tick?: number, to_tick?: number, s?: AbortSignal) =>
      packedFrames(
        (cursor) => u("/games/" + encodeURIComponent(id) + "/frames", { from_tick, to_tick, cursor, format: "packed" }),
        (url, init) => fetch(url, init),
        s,
      ),
    outline: async (map: string, s?: AbortSignal): Promise<MapOutline | null> => {
      try {
        return await getJson<MapOutline>(u("/maps/" + encodeURIComponent(map) + "/outline"), s);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    /** Published map packages, newest first (docs/game/maps.md §5); one page. */
    maps: async (q: { base?: string; author?: string; limit?: number; cursor?: string } = {}, s?: AbortSignal) =>
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
    /** One session by id, open or not. */
    session: (id: string, s?: AbortSignal) => getJson<{ session: unknown; position?: number }>(u("/sessions/" + encodeURIComponent(id)), s),
    /** Server-wide stats over a window (docs/live/live-api.md "Stats summary"); the raw body. */
    statsSummary: (window: string, s?: AbortSignal) => getJson<unknown>(u("/stats/summary", { window }), s),
    /** One page of the log; `from` is exclusive. */
    events: async (q: { from?: number; type?: string; stream?: string; limit?: number; cursor?: string }, s?: AbortSignal) =>
      page<WireEvent>(await getJson(u("/events", q), s)),
    /** Every event of one stream, following cursors. */
    streamEvents: async (stream: string, s?: AbortSignal) => (await all<WireEvent>("/events", { stream, limit: 1000 }, s)).items,
    /** The reader stream, resuming after `from` (exclusive), without the 1 Hz GameFrame events. */
    streamUrl: (from: number | null) => u("/stream", { from: from ?? 0, exclude: "GameFrame", frames: "packed" }),
  };
}

export type Api = ReturnType<typeof api>;
