// The frames' scenario objects (`Frame.objects`: placements moved or gone)
// in the 3D viewer, against the level's placements (`ScenarioObjects` in
// scenario3d.ts: `apply(list)` moves and hides, `restAll()` puts every
// object back). A frame lists every object out of place, so one that drops
// off the list is back where the scenario put it: then everything is put
// back and the list applied again.
//
// The frames draw every vehicle themselves (`Frame.vehicles`), so a placed
// vehicle the frames say has moved is hidden here rather than drawn twice.
//
// Pure: no three.js, so it is tested without a scene.

/** A frame's object entry, as `sampleObjects` gives them. */
export interface ObjectEntry {
  k: string;
  i: number;
  p?: [number, number, number];
  f?: [number, number, number];
  u?: [number, number, number];
  gone?: boolean;
}

/** What the level's placements offer (scenario3d.ts `ScenarioObjects`). */
export interface ScenarioTarget {
  apply(list: readonly ObjectEntry[]): boolean;
  restAll(): void;
}

const key = (o: ObjectEntry) => `${o.k}:${o.i}`;

export class FrameObjects {
  /** What the last update left out of place. */
  private applied = new Set<string>();

  constructor(private readonly target: ScenarioTarget) {}

  /** Places the level's objects as the frames have them now. `list` is
   *  null or empty when nothing is out of place (or the frames predate
   *  objects); `framesDrawVehicles` when the frames carry vehicles. */
  update(list: readonly ObjectEntry[] | null | undefined, framesDrawVehicles = false) {
    const now = (list ?? []).map((o) => (framesDrawVehicles && o.k === "vehi" && !o.gone ? { k: o.k, i: o.i, gone: true } : o));
    const keys = new Set(now.map(key));
    let back = false;
    for (const k of this.applied) if (!keys.has(k)) { back = true; break; }
    if (back) this.target.restAll();
    if (now.length) this.target.apply(now);
    this.applied = keys;
  }

  /** A new level (or a seek that starts over): nothing applied yet. */
  reset() {
    this.applied.clear();
  }
}
