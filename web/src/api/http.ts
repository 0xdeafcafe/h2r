// REST helpers. Every URL derives from the chosen server target.

import type { AccountKeyEntry, Frame, GameDetail, GameSummary, MapOutline, WireEvent, AccountSummary, PresenceEntry } from "./types";
import type { ServerTarget } from "./server";

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
    games: (state: "live" | "ended", limit = 100, s?: AbortSignal) =>
      getJson(u("/games", { state, limit }), s).then((b) => page<GameSummary>(b)),
    game: (id: string, s?: AbortSignal) => getJson<GameDetail>(u("/games/" + encodeURIComponent(id)), s),
    /** All recorded frames in a tick range, following cursors (600 per page). */
    frames: async (id: string, from_tick?: number, to_tick?: number, s?: AbortSignal) =>
      (await all<Frame>("/games/" + encodeURIComponent(id) + "/frames", { from_tick, to_tick }, s, 200)).items,
    outline: async (map: string, s?: AbortSignal): Promise<MapOutline | null> => {
      try {
        return await getJson<MapOutline>(u("/maps/" + encodeURIComponent(map) + "/outline"), s);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    /** One page of the log; `from` is exclusive. */
    events: async (q: { from?: number; type?: string; stream?: string; limit?: number; cursor?: string }, s?: AbortSignal) =>
      page<WireEvent>(await getJson(u("/events", q), s)),
    /** Every event of one stream, following cursors. */
    streamEvents: async (stream: string, s?: AbortSignal) => (await all<WireEvent>("/events", { stream, limit: 1000 }, s)).items,
    /** The reader stream, resuming after `from` (exclusive), without the 1 Hz GameFrame events. */
    streamUrl: (from: number | null) => u("/stream", { from: from ?? 0, exclude: "GameFrame" }),
  };
}

export type Api = ReturnType<typeof api>;
