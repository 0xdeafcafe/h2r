// Map outlines, fetched once per map per server and shared.

import { createEffect, createSignal, type Accessor } from "solid-js";
import type { MapOutline } from "../api/types";
import { live } from "./live";

const cache = new Map<string, Promise<MapOutline | null>>();

export function fetchOutline(map: string): Promise<MapOutline | null> {
  const key = live.server().api + "|" + map;
  let p = cache.get(key);
  if (!p) {
    p = live.api().outline(map).catch(() => { cache.delete(key); return null; });
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
