// REST helpers. Every URL derives from the chosen server target.

import type { Frame, GameSummary, MapOutline, WireEvent, AccountSummary, PresenceEntry } from "./types";
import type { ServerTarget } from "./server";
import type { AccountKey } from "../state/verify";

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
      // The doc says {error: "<code>", message}; the current server sends
      // {error: {code, message}}. Accept both.
      if (typeof b?.error === "string") { code = b.error; message = b.message ?? message; }
      else if (b?.error?.code) { code = b.error.code; message = b.error.message ?? message; }
    } catch { /* not JSON */ }
    throw new ApiError(r.status, code, message);
  }
  return (await r.json()) as T;
}

/** Lists are `{items, next}` per the doc; tolerate bare arrays and the
 *  `{accounts: [...]}` the server currently returns. */
export function listItems<T>(body: unknown, ...keys: string[]): T[] {
  if (Array.isArray(body)) return body as T[];
  const o = body as Record<string, unknown> | null;
  if (o && Array.isArray(o.items)) return o.items as T[];
  for (const k of keys) if (o && Array.isArray(o[k])) return o[k] as T[];
  return [];
}

export function nextCursor(body: unknown): string | number | null {
  const o = body as { next?: string | number | null } | null;
  return o && !Array.isArray(o) ? o.next ?? null : null;
}

export function api(t: ServerTarget) {
  const u = (path: string, q?: Record<string, string | number | undefined | null>) => {
    const url = new URL(t.api + path);
    for (const [k, v] of Object.entries(q ?? {})) if (v !== undefined && v !== null && v !== "") url.searchParams.set(k, String(v));
    return url.toString();
  };
  return {
    health: (s?: AbortSignal) => getJson<{ ok: boolean; position: number }>(t.base + "/healthz", s),
    accounts: async (s?: AbortSignal) => listItems<AccountSummary>(await getJson(u("/accounts", { limit: 1000 }), s), "accounts"),
    account: (tag: string, s?: AbortSignal) =>
      getJson<AccountSummary & { recent_games?: GameSummary[] }>(u("/accounts/" + encodeURIComponent(tag)), s),
    accountKeys: async (tag: string, s?: AbortSignal) =>
      listItems<AccountKey>(await getJson(u("/accounts/" + encodeURIComponent(tag) + "/keys"), s), "keys"),
    presence: async (s?: AbortSignal) => listItems<PresenceEntry>(await getJson(u("/presence"), s)),
    games: async (state: "live" | "ended", s?: AbortSignal) => listItems<GameSummary>(await getJson(u("/games", { state }), s), "games"),
    game: (id: string, s?: AbortSignal) => getJson<GameSummary & Record<string, unknown>>(u("/games/" + encodeURIComponent(id)), s),
    frames: async (id: string, from_tick?: number, to_tick?: number, s?: AbortSignal) =>
      listItems<Frame>(await getJson(u("/games/" + encodeURIComponent(id) + "/frames", { from_tick, to_tick }), s), "frames"),
    outline: async (map: string, s?: AbortSignal): Promise<MapOutline | null> => {
      try {
        return await getJson<MapOutline>(u("/maps/" + encodeURIComponent(map) + "/outline"), s);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    events: async (q: { from?: number; type?: string; stream?: string; limit?: number }, s?: AbortSignal) => {
      const body = await getJson(u("/events", q), s);
      return { items: listItems<WireEvent>(body, "events"), next: nextCursor(body) };
    },
    /** Every event of one stream, following `next` cursors. */
    streamEvents: async (stream: string, s?: AbortSignal) => {
      const out: WireEvent[] = [];
      let from: number | undefined = undefined;
      for (let page = 0; page < 50; page++) {
        const body: unknown = await getJson(u("/events", { stream, from, limit: 1000 }), s);
        const items = listItems<WireEvent>(body, "events");
        out.push(...items);
        const next = nextCursor(body);
        if (next === null || items.length === 0) break;
        from = Number(next);
      }
      return out;
    },
    streamUrl: (from: number | null) => u("/stream", { from: from ?? 0 }),
  };
}

export type Api = ReturnType<typeof api>;
