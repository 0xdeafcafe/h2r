// Open sessions on the current server, polled while a screen is showing
// them. Sessions aren't part of the snapshot or the reader stream's model,
// so this asks `GET /v1/sessions` every few seconds; it stops while the tab
// is hidden and starts over when the server changes.

import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";
import { ApiError } from "../api/http";
import { parseCoopGames, type CoopGame } from "../lib/coop";
import { parseSessions, type Session } from "../lib/sessions";
import { readSessionFilters, sessionFiltersParam, type SessionFilters } from "../lib/sessionFilters";
import { live } from "./live";
import { identity } from "./identity";
import { issueToken } from "../admin/token";
import { READ } from "../lib/coop";

/** A one-use `read` token for the signed-in account, or null when nobody is
 *  signed in or the server hasn't said who it is: then reads are anonymous. */
async function readToken(): Promise<string | null> {
  const me = identity.me(), wk = live.wellKnown();
  if (!me || !wk) return null;
  try { return await issueToken(me, wk.id, READ); } catch { return null; }
}

export const SESSIONS_REFRESH_MS = 4000;

export type SessionsStatus = "loading" | "ok" | "unfiltered" | "unsupported" | "error";

/** Runs `load` now and every `everyMs` while the tab is visible, again at
 *  once when it becomes visible, and from scratch when the server changes. */
export function poll(load: (signal: AbortSignal) => Promise<void>, reset: () => void, everyMs: number) {
  createEffect(() => {
    live.server().api; // a different server: start over
    reset();
    const ac = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let busy = false;
    const run = async () => {
      clearTimeout(timer);
      if (ac.signal.aborted) return;
      if (!document.hidden && !busy) {
        busy = true;
        try { await load(ac.signal); } finally { busy = false; }
      }
      if (!ac.signal.aborted) timer = setTimeout(run, everyMs);
    };
    const seen = () => { if (!document.hidden) void run(); };
    document.addEventListener("visibilitychange", seen);
    void run();
    onCleanup(() => { ac.abort(); clearTimeout(timer); document.removeEventListener("visibilitychange", seen); });
  });
}

/** The server's open sessions. A changed filter aborts the old read and
 *  starts a fresh list. On a failed refresh the last list stays up. */
export function useSessions(everyMs = SESSIONS_REFRESH_MS, filters?: Accessor<SessionFilters>) {
  const [items, setItems] = createSignal<Session[]>([]);
  const [status, setStatus] = createSignal<SessionsStatus>("loading");
  const [variants, setVariants] = createSignal<string[]>([]);
  poll(async (signal) => {
    try {
      const active = filters?.();
      const page = await live.api().sessions(signal, active);
      if (signal.aborted) return;
      setItems(parseSessions(page.items));
      setVariants(page.variants ?? []);
      setStatus(active && page.variants === undefined && sessionFiltersParam(active) ? "unfiltered" : "ok");
    } catch (e) {
      if (signal.aborted) return;
      if (e instanceof ApiError && e.status === 404) {
        setItems([]);
        setVariants([]);
        setStatus("unsupported");
      } else {
        setStatus("error");
      }
    }
  }, () => {
    identity.me();
    filters?.();
    setItems([]);
    setVariants([]);
    setStatus("loading");
  }, everyMs);
  return { items, status, variants };
}

const FILTERS_KEY = "h2live.sessionFilters";

export function useSessionFilters() {
  const [filters, setFilters] = createSignal<SessionFilters>(savedSessionFilters());
  function apply(next: SessionFilters) {
    const value = readSessionFilters(next);
    const param = sessionFiltersParam(value);
    if (param === sessionFiltersParam(filters())) return;
    setFilters(value);
    try {
      if (param) localStorage.setItem(FILTERS_KEY, param);
      else localStorage.removeItem(FILTERS_KEY);
    } catch { /* storage unavailable */ }
  }
  return { filters, apply };
}

function savedSessionFilters(): SessionFilters {
  try { return readSessionFilters(JSON.parse(localStorage.getItem(FILTERS_KEY) ?? "{}")); }
  catch { return {}; }
}

/** The reader's current link for a specific game, including a game opened by a deep link. */
export function useGameJoin(id: Accessor<string>): Accessor<string | null> {
  const [session, setSession] = createSignal<string | null>(null);
  poll(async (signal) => {
    try {
      const result = await live.api().gameJoin(id(), signal);
      if (!signal.aborted) setSession(result.session_id);
    } catch {
      if (!signal.aborted) setSession(null);
    }
  }, () => { id(); identity.me(); setSession(null); }, SESSIONS_REFRESH_MS);
  return session;
}

export type CoopStatus = "loading" | "ok" | "unsupported" | "error";

/** Finished co-op games, newest first, for one player (a gamertag) or all.
 *  Refreshed slowly: they only change when a game ends. Signed in, it asks
 *  as that account, so friends-only games it may see are in the list. */
export function useCoopGames(player?: Accessor<string | undefined>, limit = 50, everyMs = 15000): { items: Accessor<CoopGame[]>; status: Accessor<CoopStatus> } {
  const [items, setItems] = createSignal<CoopGame[]>([]);
  const [status, setStatus] = createSignal<CoopStatus>("loading");
  poll(async (signal) => {
    const who = player?.();
    try {
      const page = await live.api().coopGames({ player: who, limit }, signal, await readToken());
      if (player?.() !== who) return;
      setItems(parseCoopGames(page.items));
      setStatus("ok");
    } catch (e) {
      if (signal.aborted) return;
      if (e instanceof ApiError && e.status === 404) { setItems([]); setStatus("unsupported"); } else setStatus("error");
    }
  }, () => { player?.(); identity.me(); setItems([]); setStatus("loading"); }, everyMs);
  return { items, status };
}
