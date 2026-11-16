// Open sessions on the current server, polled while a screen is showing
// them. Sessions aren't part of the snapshot or the reader stream's model,
// so this asks `GET /v1/sessions` every few seconds; it stops while the tab
// is hidden and starts over when the server changes.

import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";
import { ApiError } from "../api/http";
import { parseSession, parseSessions, type Session } from "../lib/sessions";
import { live } from "./live";

export const SESSIONS_REFRESH_MS = 4000;

export type SessionsStatus = "loading" | "ok" | "unsupported" | "error";

/** Runs `load` now and every `everyMs` while the tab is visible, again at
 *  once when it becomes visible, and from scratch when the server changes. */
function poll(load: (signal: AbortSignal) => Promise<void>, reset: () => void, everyMs: number) {
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

export type SessionStatus = "loading" | "ok" | "missing" | "error";

/** One session by id, refreshed the same way. A closed or expired session
 *  is still returned (its `state` says so); `missing` is an unknown id. */
export function useSession(id: Accessor<string>, everyMs = SESSIONS_REFRESH_MS): { session: Accessor<Session | null>; status: Accessor<SessionStatus> } {
  const [session, setSession] = createSignal<Session | null>(null);
  const [status, setStatus] = createSignal<SessionStatus>("loading");
  createEffect(() => { id(); setSession(null); setStatus("loading"); });
  poll(async (signal) => {
    const want = id();
    try {
      const s = parseSession((await live.api().session(want, signal)).session);
      if (id() !== want) return;
      setSession(s);
      setStatus(s ? "ok" : "missing");
    } catch (e) {
      if (signal.aborted || id() !== want) return;
      if (e instanceof ApiError && e.status === 404) { setSession(null); setStatus("missing"); } else setStatus("error");
    }
  }, () => { setSession(null); setStatus("loading"); }, everyMs);
  return { session, status };
}
