// The latest appearance (emblem layers, profile colours) each player's
// games carried, by XUID, so every emblem on the site can show a player's
// real one rather than a pick derived from their XUID.

import { createMemo, createRoot } from "solid-js";
import type { Appearance } from "../lib/gameart";
import { live } from "./live";

const byXuid = createRoot(() =>
  createMemo(() => {
    const out = new Map<string, Appearance>();
    // gameOrder is newest first, so the first appearance seen is the latest.
    for (const id of live.state.gameOrder) {
      const g = live.state.games[id];
      if (!g) continue;
      for (const p of Object.values(g.players)) {
        if (p.xuid && p.appearance && !out.has(p.xuid)) out.set(p.xuid, p.appearance);
      }
    }
    return out;
  }),
);

/** The player's latest known appearance, or null. */
export function appearanceOf(xuid: string | null | undefined): Appearance | null {
  return (xuid && byXuid().get(xuid)) || null;
}
