// Map geometry and pre-drawn views (the extractor's "map geometry
// contract"): where a server keeps a map's `map.glb` and `views.json`, how
// to plot a game position on a pre-drawn view, and how game coordinates
// become glTF ones. Everything here is pure.
//
// Game coordinates are z-up, as telemetry frames give them. A view's
// `world_to_pixel` is a 2x4 affine matrix: px = M[0]·(x,y,z,1),
// py = M[1]·(x,y,z,1), pixel origin top-left. glTF is y-up: a game
// position (x, y, z) is the glTF point (x, z, -y).

export type Vec3 = [number, number, number];
export type Row4 = [number, number, number, number];
export type Mat2x4 = [Row4, Row4];

export interface MapViewDef {
  id: string;
  kind: string;
  width: number;
  height: number;
  world_to_pixel: Mat2x4;
  /** The picture, relative to views.json; `views/<id>.png` when not given. */
  file?: string;
  up?: string;
  depth_sorted?: boolean;
}

export interface MapViews {
  geometry_version: number;
  map: string;
  bounds: { min: Vec3; max: Vec3 };
  views: MapViewDef[];
}

/** The geometry version this client reads. */
export const GEOMETRY_VERSION = 1;

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isVec3 = (v: unknown): v is Vec3 => Array.isArray(v) && v.length === 3 && v.every(isNum);
const isRow4 = (v: unknown): v is Row4 => Array.isArray(v) && v.length === 4 && v.every(isNum);

/** Checks a views.json; null for anything this client can't plot on. Views
 *  with a bad matrix or size are dropped rather than failing the lot. */
export function parseViews(raw: unknown): MapViews | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (r.geometry_version !== GEOMETRY_VERSION || !Array.isArray(r.views)) return null;
  const b = r.bounds as { min?: unknown; max?: unknown } | undefined;
  if (!b || !isVec3(b.min) || !isVec3(b.max)) return null;
  const views: MapViewDef[] = [];
  for (const v of r.views as Array<Record<string, unknown>>) {
    if (!v || typeof v.id !== "string" || !/^[a-z0-9][a-z0-9_-]*$/i.test(v.id)) continue;
    const m = v.world_to_pixel;
    if (!Array.isArray(m) || m.length !== 2 || !isRow4(m[0]) || !isRow4(m[1])) continue;
    if (!isNum(v.width) || !isNum(v.height) || v.width <= 0 || v.height <= 0) continue;
    views.push({
      id: v.id, kind: typeof v.kind === "string" ? v.kind : "ortho", width: v.width, height: v.height,
      world_to_pixel: [m[0], m[1]],
      file: safeFile(v.file) ? v.file : undefined,
      up: typeof v.up === "string" ? v.up : undefined,
      depth_sorted: v.depth_sorted === true,
    });
  }
  if (!views.length) return null;
  return { geometry_version: GEOMETRY_VERSION, map: typeof r.map === "string" ? r.map : "", bounds: { min: b.min, max: b.max }, views };
}

/** A game position's pixel on a pre-drawn view. */
export function worldToPixel(m: Mat2x4, x: number, y: number, z: number): [number, number] {
  const a = m[0], b = m[1];
  return [a[0] * x + a[1] * y + a[2] * z + a[3], b[0] * x + b[1] * y + b[2] * z + b[3]];
}

/** The direction a view looks along, towards the viewer (unit, game
 *  coordinates): larger `depthOf` is nearer the viewer, so drawn later. */
export function viewAxis(m: Mat2x4): Vec3 {
  const [a, b] = m;
  // Pixel y grows downwards, so screen-right × screen-up = towards the viewer.
  const ux = -b[0], uy = -b[1], uz = -b[2];
  const c: Vec3 = [a[1] * uz - a[2] * uy, a[2] * ux - a[0] * uz, a[0] * uy - a[1] * ux];
  const len = Math.hypot(c[0], c[1], c[2]) || 1;
  return [c[0] / len, c[1] / len, c[2] / len];
}

export function depthOf(axis: Vec3, x: number, y: number, z: number): number {
  return axis[0] * x + axis[1] * y + axis[2] * z;
}

/** Pixels per world unit on a view (the length of its screen-right row). */
export function viewScale(m: Mat2x4): number {
  return Math.hypot(m[0][0], m[0][1], m[0][2]);
}

/** The pixel direction of a yaw (0 = +x, counter-clockwise seen from
 *  above) on a view, as an angle for canvas drawing (y down). */
export function yawOnView(m: Mat2x4, yaw: number): number {
  const c = Math.cos(yaw), s = Math.sin(yaw);
  return Math.atan2(m[1][0] * c + m[1][1] * s, m[0][0] * c + m[0][1] * s);
}

// ---- axes ----

/** Game (x, y, z), z up → glTF (x, z, -y), y up. */
export function gameToGl(x: number, y: number, z: number): Vec3 {
  return [x, z, -y];
}

/** glTF (x, y, z) → game (x, -z, y). */
export function glToGame(x: number, y: number, z: number): Vec3 {
  return [x, -z, y];
}

/** A game yaw (0 = +x, counter-clockwise about z) is the same angle about
 *  glTF's y axis, for a model that faces +x: rotating +x by θ about y gives
 *  (cos θ, 0, -sin θ), which is game (cos θ, sin θ, 0). */
export function yawToGl(yaw: number): number {
  return yaw;
}

// ---- where a server keeps a map's files ----

export interface MapFiles {
  geometry?: string;
  views?: string;
  /** `multiplayer` or `campaign`, when the manifest says. */
  kind?: "multiplayer" | "campaign";
  /** `halo1` for a Halo 1 map converted to run in Halo 2; absent for Halo 2's own. */
  game?: string;
  /** Its display name, when the manifest carries one (Halo 1 maps do). */
  name?: string;
}

/** The map's stem, as the art manifest keys it: the file name without path
 *  or `.map`, lower case. */
export function mapStem(map: string): string {
  return map.split(/[\\/]/).pop()!.replace(/\.map$/i, "").trim().toLowerCase();
}

/** Halo 1's campaign levels by the stem the game reports them by (the
 *  level's own short name; the import keeps it). File stems only, no names:
 *  without game art nothing else tells a Halo 1 level from a modded Halo 2
 *  one, and none of these is a Halo 2 stem (those are `00a_`..`08b_`). */
const HALO1_LEVELS = new Set(["a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40"]);

/** True for a Halo 1 map: the manifest says so, or its stem is the
 *  converter's `h1_<map>` or one of Halo 1's campaign levels (so a server
 *  without game art still tells them apart). */
export function isHalo1(stem: string, files?: MapFiles | null): boolean {
  const s = mapStem(stem);
  return files?.game === "halo1" || /^h1_./.test(s) || HALO1_LEVELS.has(s);
}

function safeFile(file: unknown): file is string {
  return typeof file === "string" && !!file && !/^([a-z][a-z0-9+.-]*:|\/|\\)/i.test(file) && !file.split(/[\\/]/).includes("..");
}

/** The per-map geometry files a raw art manifest names. The contract says
 *  the manifest "gains, per map" a `geometry` and a `views` file; this reads
 *  the shapes that can take: a `maps` table by stem, or the two names on
 *  the map's `map/<stem>/preview` image entry (or on a `map/<stem>` one). */
export function parseMapFiles(raw: unknown): Record<string, MapFiles> {
  const out: Record<string, MapFiles> = {};
  if (!raw || typeof raw !== "object") return out;
  const put = (stem: string, v: unknown) => {
    if (!v || typeof v !== "object") return;
    const e = v as { geometry?: unknown; views?: unknown; kind?: unknown; game?: unknown; name?: unknown };
    const f: MapFiles = {};
    if (safeFile(e.geometry)) f.geometry = e.geometry;
    if (safeFile(e.views)) f.views = e.views;
    if (!f.geometry && !f.views) return;
    if (e.kind === "multiplayer" || e.kind === "campaign") f.kind = e.kind;
    if (typeof e.game === "string" && /^[a-z0-9_]{1,16}$/.test(e.game)) f.game = e.game;
    if (typeof e.name === "string" && e.name.trim()) f.name = e.name.trim().slice(0, 64);
    out[stem.toLowerCase()] = { ...out[stem.toLowerCase()], ...f };
  };
  const r = raw as { maps?: unknown; images?: unknown };
  if (r.images && typeof r.images === "object") {
    for (const [key, v] of Object.entries(r.images as Record<string, unknown>)) {
      const m = /^map\/([^/]+)(?:\/preview)?$/.exec(key);
      if (m) put(m[1]!, v);
    }
  }
  if (r.maps && typeof r.maps === "object") {
    for (const [stem, v] of Object.entries(r.maps as Record<string, unknown>)) put(stem, v);
  }
  return out;
}

/** A view's picture, beside its views.json: the view's `file`, else
 *  `views/<id>.png`. */
export function viewImageFile(viewsFile: string, view: Pick<MapViewDef, "id" | "file">): string {
  const dir = viewsFile.includes("/") ? viewsFile.slice(0, viewsFile.lastIndexOf("/") + 1) : "";
  return dir + (view.file ?? `views/${view.id}.png`);
}

/** The view to open on: `top`, else the first. */
export function defaultView(v: MapViews): MapViewDef {
  return v.views.find((x) => x.id === "top") ?? v.views[0]!;
}

/** The straight-down view, for a thumbnail; null when there isn't one. */
export function topView(v: MapViews | null | undefined): MapViewDef | null {
  return v?.views.find((x) => x.id === "top") ?? null;
}

/** The view a `?map=` value asks for: that view when the map has it, else
 *  the default. */
export function pickView(v: MapViews, want: string | null | undefined): MapViewDef {
  return v.views.find((x) => x.id === want) ?? defaultView(v);
}

/** What `?map=` should say for a chosen view: its id, or nothing for the
 *  default view (and for an id the map doesn't have), so the plain URL stays plain. */
export function viewParam(v: MapViews, id: string): string | undefined {
  const picked = v.views.find((x) => x.id === id);
  return picked && picked.id !== defaultView(v).id ? picked.id : undefined;
}

/** A short name for a view id: `iso-ne` → "NE". */
export function viewLabel(id: string): string {
  if (id === "top") return "Top";
  const m = /^iso-(.+)$/.exec(id);
  return m ? m[1]!.toUpperCase() : id;
}

// ---- pan and zoom over a picture ----

/** Where a picture sits in its box: picture pixel (px, py) is drawn at
 *  (x + px * k, y + py * k). */
export interface PanZoom { x: number; y: number; k: number }

/** The transform that fits a w×h picture in a W×H box with `pad` around it. */
export function fitPicture(W: number, H: number, w: number, h: number, pad = 0): PanZoom {
  const k = Math.max(1e-6, Math.min((W - pad * 2) / w, (H - pad * 2) / h));
  return { x: (W - w * k) / 2, y: (H - h * k) / 2, k };
}

/** Zooms by `factor` keeping the box point (cx, cy) over the same picture
 *  pixel, with the zoom held between `min` and `max`. */
export function zoomAbout(p: PanZoom, cx: number, cy: number, factor: number, min: number, max: number): PanZoom {
  const k = Math.min(max, Math.max(min, p.k * factor));
  const f = k / p.k;
  return { x: cx - (cx - p.x) * f, y: cy - (cy - p.y) * f, k };
}

/** Keeps the picture from being dragged out of its box: when it is smaller
 *  than the box it stays centred, otherwise its edges stay outside. */
export function clampPan(p: PanZoom, W: number, H: number, w: number, h: number): PanZoom {
  const pw = w * p.k, ph = h * p.k;
  const x = pw <= W ? (W - pw) / 2 : Math.min(0, Math.max(W - pw, p.x));
  const y = ph <= H ? (H - ph) / 2 : Math.min(0, Math.max(H - ph, p.y));
  return { x, y, k: p.k };
}

/** Moves `cur` towards `target`, frame-rate independent (`rate` per
 *  second); lands exactly once it is close. */
export function easePanZoom(cur: PanZoom, target: PanZoom, dtMs: number, rate = 14): PanZoom {
  const t = 1 - Math.exp(-rate * Math.max(0, dtMs) / 1000);
  const k = cur.k + (target.k - cur.k) * t;
  const x = cur.x + (target.x - cur.x) * t;
  const y = cur.y + (target.y - cur.y) * t;
  if (Math.abs(target.k - k) < target.k * 1e-3 && Math.abs(target.x - x) < 0.25 && Math.abs(target.y - y) < 0.25) return { ...target };
  return { x, y, k };
}
