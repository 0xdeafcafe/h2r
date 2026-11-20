// Open sessions on the current server, polled while a screen is showing
// them. Sessions aren't part of the snapshot or the reader stream's model,
// so this asks `GET /v1/sessions` every few seconds; it stops while the tab
// is hidden and starts over when the server changes.

import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";
import { ApiError } from "../api/http";
import { parseCoopGames, type CoopGame } from "../lib/coop";
import { parseSessions, type Session } from "../lib/sessions";
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

export type SessionsStatus = "loading" | "ok" | "unsupported" | "error";

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

/** The server's open sessions. `unsupported` is a server that has no
 *  session list at all (an older one answers 404). On a failed refresh the
 *  last list stays up and the status says so. */
export function useSessions(everyMs = SESSIONS_REFRESH_MS): { items: Accessor<Session[]>; status: Accessor<SessionsStatus> } {
  const [items, setItems] = createSignal<Session[]>([]);
  const [status, setStatus] = createSignal<SessionsStatus>("loading");
  poll(async (signal) => {
    try {
      const page = await live.api().sessions(signal);
      setItems(parseSessions(page.items));
      setStatus("ok");
    } catch (e) {
      if (signal.aborted) return;
      if (e instanceof ApiError && e.status === 404) { setItems([]); setStatus("unsupported"); } else setStatus("error");
    }
  }, () => { setItems([]); setStatus("loading"); }, everyMs);
  return { items, status };
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
