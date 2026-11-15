// Map outlines, fetched once per map per server and shared.

import { createEffect, createSignal, type Accessor } from "solid-js";
import type { MapOutline } from "../api/types";
import { live } from "./live";

const cache = new Map<string, Promise<MapOutline | null>>();

// A map nobody has uploaded an outline for answers 404 (by design), and the
// browser logs every such response. Remember the miss for a while so a
// reload, or another page, doesn't ask (and log) again.
const MISS_TTL_MS = 5 * 60_000;
const missKey = (key: string) => "h2live.no-outline." + key;

function missed(key: string): boolean {
  try { return Date.now() - Number(sessionStorage.getItem(missKey(key)) ?? 0) < MISS_TTL_MS; } catch { return false; }
}

export function fetchOutline(map: string): Promise<MapOutline | null> {
  const key = live.server().api + "|" + map;
  let p = cache.get(key);
  if (!p) {
    p = missed(key)
      ? Promise.resolve(null)
      : live.api().outline(map).then(
        (o) => {
          if (!o) {
            try { sessionStorage.setItem(missKey(key), String(Date.now())); } catch { /* private mode */ }
            // Ask again once the miss has aged out (the game may upload one).
            setTimeout(() => cache.delete(key), MISS_TTL_MS);
          }
          return o;
        },
        () => { cache.delete(key); return null; },
      );
    cache.set(key, p);
  }
  return p;
}

/** The outline for a map, or null until (or unless) the server has one. */
export function outlineFor(map: Accessor<string | undefined>): Accessor<MapOutline | null> {
  const [o, set] = createSignal<MapOutline | null>(null);
  createEffect(() => {
    const m = map();
    set(null);
    if (!m) return;
    void fetchOutline(m).then((x) => { if (map() === m) set(x); });
  });
  return o;
}
