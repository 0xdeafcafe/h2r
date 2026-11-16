// Which of a level glb's top parts are shown. A campaign level's structure
// comes as one node per zone (`bsp_<n>`); the game has one loaded at a time
// and says which in its frames, so the view shows that one. Zones only ever
// used by cutscenes are marked in the glb and stay hidden, and `backdrop`
// geometry (far scenery around the playable space) is hidden unless asked
// for. Pure.

export interface ZoneNode {
  name: string;
  /** Marked in the glb as never loaded during play. */
  cinematic: boolean;
}

/** The zone a node is: 3 for `bsp_3`; null for anything else. */
export function bspNumber(name: string): number | null {
  const m = /^bsp_?(\d+)$/i.exec(name);
  return m ? Number(m[1]) : null;
}

export const isBackdrop = (name: string) => /^(backdrop|sky(box)?)(_\d+)?$/i.test(name);

/** Whether a glb node's extras mark it cinematic-only. The extractor's
 *  key isn't settled, so the likely spellings all count. */
export function isCinematic(extras: unknown, name = "", assetExtras?: unknown): boolean {
  const e = (extras ?? {}) as Record<string, unknown>;
  if (e.cinematic === true || e.cinematic_only === true || e.cinematicOnly === true) return true;
  const n = bspNumber(name), list = (assetExtras as { cinematic_bsps?: unknown } | null | undefined)?.cinematic_bsps;
  return n !== null && Array.isArray(list) && list.includes(n);
}

/** For each node, whether to show it.
 *  - Zones: only the active one when the frames name a zone the glb has;
 *    otherwise every zone that isn't cinematic-only. If that would leave no
 *    zone at all, all of them (better too much than an empty level).
 *  - Backdrop: only when asked for.
 *  - Anything else (a single `structure`, `machines`): always. */
export function zoneVisibility(nodes: readonly ZoneNode[], active: number | null | undefined, opts: { backdrop?: boolean } = {}): boolean[] {
  const zones = nodes.map((n) => bspNumber(n.name));
  const has = active != null && zones.includes(active);
  let show = nodes.map((n, i) => {
    const z = zones[i]!;
    if (z === null) return isBackdrop(n.name) ? !!opts.backdrop : true;
    return has ? z === active : !n.cinematic;
  });
  if (zones.some((z) => z !== null) && !show.some((s, i) => s && zones[i] !== null)) show = show.map((s, i) => (zones[i] !== null ? true : s));
  return show;
}
