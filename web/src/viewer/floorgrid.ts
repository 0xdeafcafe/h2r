// Straight-down probes into a level, fast. The feet of every biped on its
// feet are put on the floor under them (ground.ts), up to 16 probes an
// update. A raycast against the level's meshes tests every triangle of
// every mesh the ray's bounding spheres let through, and a level's meshes
// (one per material, spanning the map) all do: on a campaign level that
// was a 145k-triangle scan per probe, 12 to 18 ms spikes per update.
//
// Here the level's triangles (world space, glTF axes: y up) are put once
// into a grid of columns over x and z, and a probe tests only its column.
// A triangle wider than many columns (far terrain planes) is kept in a
// short list every probe tests instead. A probe finds the nearest
// triangle facing up (normal y > 0) the ray down meets within its reach,
// skipping triangles of a source the caller says is hidden (a zone not in
// play). Pure: no three.js, so it can be tested.

/** One source of triangles: positions (x, y, z per vertex, world space) and
 *  optional indices; `id` is handed back to `shown` at probe time. */
export interface FloorSource { id: number; positions: ArrayLike<number>; index?: ArrayLike<number> | null }

/** What a probe found: where, the unit normal (world space), and its source. */
export interface FloorHit { x: number; y: number; z: number; nx: number; ny: number; nz: number; id: number }

/** A triangle covering more columns than this is tested by every probe. */
const WIDE = 64;
/** Columns per side at most. */
const MAX_SIDE = 512;

export class FloorGrid {
  /** Triangles' corners, 9 floats each, and their sources. */
  private tri: Float32Array;
  private src: Int32Array;
  /** Per column, where its triangle list starts in `cells` (CSR). */
  private start: Uint32Array;
  private cells: Uint32Array;
  private wide: Uint32Array;
  private x0 = 0; private z0 = 0; private size = 1; private nx = 1; private nz = 1;
  readonly triangles: number;

  constructor(sources: readonly FloorSource[]) {
    // Every triangle that could be stood on: wound to face up (as a
    // raycast's face normal has it, whatever the material's sides), not
    // vertical, not degenerate.
    const corners: number[] = [];
    const ids: number[] = [];
    let minX = Infinity, minZ = Infinity, maxX = -Infinity, maxZ = -Infinity;
    for (const s of sources) {
      const p = s.positions, idx = s.index;
      const n = idx ? idx.length : p.length / 3;
      for (let t = 0; t + 2 < n; t += 3) {
        const a = idx ? idx[t]! : t, b = idx ? idx[t + 1]! : t + 1, c = idx ? idx[t + 2]! : t + 2;
        const ax = p[a * 3]!, ay = p[a * 3 + 1]!, az = p[a * 3 + 2]!;
        const bx = p[b * 3]!, by = p[b * 3 + 1]!, bz = p[b * 3 + 2]!;
        const cx = p[c * 3]!, cy = p[c * 3 + 1]!, cz = p[c * 3 + 2]!;
        // Normal's y: (b - a) x (c - a), y component.
        const ny = (bz - az) * (cx - ax) - (bx - ax) * (cz - az);
        if (!(ny > 0)) continue;
        corners.push(ax, ay, az, bx, by, bz, cx, cy, cz);
        ids.push(s.id);
        minX = Math.min(minX, ax, bx, cx); maxX = Math.max(maxX, ax, bx, cx);
        minZ = Math.min(minZ, az, bz, cz); maxZ = Math.max(maxZ, az, bz, cz);
      }
    }
    this.tri = new Float32Array(corners);
    this.src = new Int32Array(ids);
    const count = ids.length;
    this.triangles = count;
    if (!count) {
      this.start = new Uint32Array(2); this.cells = new Uint32Array(0); this.wide = new Uint32Array(0);
      return;
    }
    // About four triangles a column, columns square.
    const w = Math.max(maxX - minX, 1e-3), d = Math.max(maxZ - minZ, 1e-3);
    this.size = Math.max(Math.sqrt((w * d) / Math.max(1, count / 4)), w / MAX_SIDE, d / MAX_SIDE, 1e-3);
    this.nx = Math.min(MAX_SIDE, Math.max(1, Math.ceil(w / this.size)));
    this.nz = Math.min(MAX_SIDE, Math.max(1, Math.ceil(d / this.size)));
    this.x0 = minX; this.z0 = minZ;
    const columns = this.nx * this.nz;
    const counts = new Uint32Array(columns + 1);
    const wide: number[] = [];
    const span = (t: number): [number, number, number, number] => {
      const o = t * 9, c = this.tri;
      return [
        this.col(Math.min(c[o]!, c[o + 3]!, c[o + 6]!), this.x0, this.nx), this.col(Math.max(c[o]!, c[o + 3]!, c[o + 6]!), this.x0, this.nx),
        this.col(Math.min(c[o + 2]!, c[o + 5]!, c[o + 8]!), this.z0, this.nz), this.col(Math.max(c[o + 2]!, c[o + 5]!, c[o + 8]!), this.z0, this.nz),
      ];
    };
    const isWide = (s: [number, number, number, number]) => (s[1] - s[0] + 1) * (s[3] - s[2] + 1) > WIDE;
    for (let t = 0; t < count; t++) {
      const s = span(t);
      if (isWide(s)) { wide.push(t); continue; }
      for (let j = s[2]; j <= s[3]; j++) for (let i = s[0]; i <= s[1]; i++) counts[j * this.nx + i + 1]!++;
    }
    for (let k = 1; k <= columns; k++) counts[k]! += counts[k - 1]!;
    this.start = counts;
    this.cells = new Uint32Array(counts[columns]!);
    const fill = counts.slice(0, columns);
    for (let t = 0; t < count; t++) {
      const s = span(t);
      if (isWide(s)) continue;
      for (let j = s[2]; j <= s[3]; j++) for (let i = s[0]; i <= s[1]; i++) this.cells[fill[j * this.nx + i]!++] = t;
    }
    this.wide = new Uint32Array(wide);
  }

  private col(v: number, origin: number, n: number): number {
    return Math.max(0, Math.min(n - 1, Math.floor((v - origin) / this.size)));
  }

  /** The first floor straight down from (x, y, z) within `reach`, whose
   *  source `shown` allows; null for none. */
  probe(x: number, y: number, z: number, reach: number, shown: (id: number) => boolean = () => true): FloorHit | null {
    if (!this.triangles) return null;
    let best = -1, bestY = -Infinity;
    const test = (t: number) => {
      const o = t * 9, c = this.tri;
      const ax = c[o]!, ay = c[o + 1]!, az = c[o + 2]!, bx = c[o + 3]!, by = c[o + 4]!, bz = c[o + 5]!, cx = c[o + 6]!, cy = c[o + 7]!, cz = c[o + 8]!;
      // Barycentric coordinates of (x, z) in the triangle's footprint.
      const det = (bz - cz) * (ax - cx) + (cx - bx) * (az - cz);
      if (det === 0) return;
      const u = ((bz - cz) * (x - cx) + (cx - bx) * (z - cz)) / det;
      const v = ((cz - az) * (x - cx) + (ax - cx) * (z - cz)) / det;
      const e = -1e-6;
      if (u < e || v < e || u + v > 1 - e) return;
      const h = u * ay + v * by + (1 - u - v) * cy;
      if (h > y || h < y - reach || h <= bestY) return;
      if (!shown(this.src[t]!)) return;
      best = t; bestY = h;
    };
    if (x >= this.x0 && z >= this.z0 && x <= this.x0 + this.nx * this.size && z <= this.z0 + this.nz * this.size) {
      const k = this.col(z, this.z0, this.nz) * this.nx + this.col(x, this.x0, this.nx);
      for (let q = this.start[k]!, end = this.start[k + 1]!; q < end; q++) test(this.cells[q]!);
    }
    for (const t of this.wide) test(t);
    if (best < 0) return null;
    const o = best * 9, c = this.tri;
    const e1x = c[o + 3]! - c[o]!, e1y = c[o + 4]! - c[o + 1]!, e1z = c[o + 5]! - c[o + 2]!;
    const e2x = c[o + 6]! - c[o]!, e2y = c[o + 7]! - c[o + 1]!, e2z = c[o + 8]! - c[o + 2]!;
    const nx = e1y * e2z - e1z * e2y, ny = e1z * e2x - e1x * e2z, nz = e1x * e2y - e1y * e2x;
    const len = Math.hypot(nx, ny, nz) || 1;
    return { x, y: bestY, z, nx: nx / len, ny: ny / len, nz: nz / len, id: this.src[best]! };
  }
}
