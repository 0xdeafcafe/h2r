// The frames' scenario objects (`Frame.objects`: placements moved or gone)
// in the 3D viewer, against the level's placements (`ScenarioObjects` in
// scenario3d.ts: `apply(list)` moves and hides, `restAll()` puts every
// object back). A frame lists every object out of place, so one that drops
// off the list is back where the scenario put it: then everything is put
// back and the list applied again.
//
// The frames draw every vehicle in the world themselves (`Frame.vehicles`),
// so once they carry vehicles the level's own copies of its vehicles (the
// `vehi` placements, and the multiplayer vehicle spawns, `netgame` entries
// whose item is a vehicle) are hidden from the first frame, standing or
// moved: otherwise the copy stays at the spawn while the real one drives
// off (multiplayer vehicles come from netgame spawns, which the frames
// report by spawn, not as `vehi`). It stays that way until a new level, so
// a frame whose world scan failed doesn't bring the copies back for a
// moment.
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
  /** Hides (true) or shows the level's own vehicles. */
  setVehiclesHidden(hidden: boolean): void;
}

const key = (o: ObjectEntry) => `${o.k}:${o.i}`;

export class FrameObjects {
  /** What the last update left out of place. */
  private applied = new Set<string>();
  /** The frames have carried vehicles. */
  private framesVehicles = false;

  constructor(private readonly target: ScenarioTarget) {}

  /** Places the level's objects as the frames have them now. `list` is
   *  null or empty when nothing is out of place (or the frames predate
   *  objects); `framesDrawVehicles` when the frames carry vehicles. */
  update(list: readonly ObjectEntry[] | null | undefined, framesDrawVehicles = false) {
    if (framesDrawVehicles && !this.framesVehicles) {
      this.framesVehicles = true;
      this.target.setVehiclesHidden(true);
    }
    const now = list ?? [];
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
    if (this.framesVehicles) this.target.setVehiclesHidden(false);
    this.framesVehicles = false;
  }
}
