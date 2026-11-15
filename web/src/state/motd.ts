// The current server's message of the day, for the views that show it (the
// dashboard's main menu, the stats site's Did You Know? box). Fetched when a
// view mounts and every few minutes after; null when none is published or
// the fetch fails, so the views just hide.

import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";
import { getMotd } from "../admin/api";
import type { MotdDoc } from "../admin/validate";
import { live } from "./live";

export const MOTD_REFRESH_MS = 5 * 60_000;

/** Call inside a component: the MOTD, refreshed while it is mounted. */
export function useMotd(refreshMs = MOTD_REFRESH_MS): Accessor<MotdDoc | null> {
  const [doc, setDoc] = createSignal<MotdDoc | null>(null);
  let gen = 0;
  const load = () => {
    const mine = ++gen;
    getMotd().then(
      (d) => { if (mine === gen) setDoc(d); },
      () => { if (mine === gen) setDoc(null); },
    );
  };
  createEffect(() => {
    live.server().base;
    setDoc(null);
    load();
  });
  const t = setInterval(load, refreshMs);
  onCleanup(() => { gen++; clearInterval(t); });
  return doc;
}
