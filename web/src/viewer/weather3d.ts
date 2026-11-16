// The level's weather in the 3D view: rain, snow or drifting motes where
// the map's own weather falls. The map glb's `asset.extras.weather` lists
// each weather system with the boxes it falls in (the BSP clusters it is
// set on) and the volumes it is kept out of (under roofs and overhangs;
// docs/live/live-art.md, "Weather"). The particles are points in a box
// around what the camera looks at, fixed in the world and wrapping as it
// moves, falling and drifting on the GPU; each is drawn only inside one of
// its system's boxes and outside all of its kept-out volumes. `weatherOf`
// and `inside` are pure.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";

export type WeatherKind = "rain" | "snow" | "motes";

export interface WeatherSystem {
  name: string;
  kind: WeatherKind;
  /** 0.5 light, 1, 2 heavy. */
  amount: number;
  /** glTF frame: min x, y, z, max x, y, z. */
  boxes: Array<[number, number, number, number, number, number]>;
  /** Convex volumes, each planes (nx, ny, nz, d): inside when n·p <= d for all. */
  exclude: Array<Array<[number, number, number, number]>>;
}

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const tuple = <N extends number>(v: unknown, n: N): number[] | null =>
  Array.isArray(v) && v.length === n && v.every(finite) ? (v as number[]) : null;

/** The weather from a map glb's `asset.extras`: well-formed systems only. */
export function weatherOf(assetExtras: unknown): WeatherSystem[] {
  const list = (assetExtras as { weather?: unknown } | null | undefined)?.weather;
  if (!Array.isArray(list)) return [];
  const out: WeatherSystem[] = [];
  for (const w of list as Array<Record<string, unknown>>) {
    if (!w || typeof w !== "object") continue;
    const kind = w.kind === "rain" || w.kind === "snow" || w.kind === "motes" ? w.kind : null;
    if (!kind) continue;
    const boxes = (Array.isArray(w.boxes) ? w.boxes : []).map((b) => tuple(b, 6)).filter((b): b is number[] => !!b) as WeatherSystem["boxes"];
    if (!boxes.length) continue;
    const exclude = (Array.isArray(w.exclude) ? w.exclude : [])
      .map((p) => (Array.isArray(p) ? p.map((q) => tuple(q, 4)).filter((q): q is number[] => !!q) : []))
      .filter((p) => p.length >= 4) as WeatherSystem["exclude"];
    const amount = finite(w.amount) ? Math.min(3, Math.max(0.25, w.amount)) : 1;
    out.push({ name: typeof w.name === "string" ? w.name : "", kind, amount, boxes, exclude });
  }
  return out;
}

/** Whether a point (glTF frame) is where a system's weather falls. */
export function inside(w: WeatherSystem, p: [number, number, number]): boolean {
  const inBox = w.boxes.some((b) => p[0] >= b[0] && p[1] >= b[1] && p[2] >= b[2] && p[0] <= b[3] && p[1] <= b[4] && p[2] <= b[5]);
  if (!inBox) return false;
  return !w.exclude.some((planes) => planes.every((q) => q[0] * p[0] + q[1] * p[1] + q[2] * p[2] <= q[3]));
}

/** How each kind looks and moves: particles at amount 1, fall speed and
 *  sideways drift (units a second), streak length (units), size (pixels at
 *  10 units), colour and opacity. */
const LOOK: Record<WeatherKind, { count: number; fall: number; drift: number; streak: number; size: number; colour: number; alpha: number }> = {
  rain: { count: 9000, fall: 16, drift: 0.6, streak: 0.9, size: 34, colour: 0xc8d2dc, alpha: 0.55 },
  snow: { count: 7000, fall: 1.1, drift: 0.5, streak: 0, size: 9, colour: 0xffffff, alpha: 0.85 },
  motes: { count: 2500, fall: 0.15, drift: 0.35, streak: 0, size: 6, colour: 0xd8d2c0, alpha: 0.45 },
};

const MAX_BOXES = 256;
const MAX_PLANES = 1024;
const MAX_VOLUMES = 128;

const VERTEX = /* glsl */ `
uniform float uTime;
uniform vec3 uOrigin;
uniform float uSize;
uniform float uFall;
uniform float uDrift;
uniform float uPoint;
uniform float uStreak;
uniform sampler2D uBoxes;
uniform int uBoxCount;
uniform sampler2D uPlanes;
uniform sampler2D uVolumes;
uniform int uVolumeCount;
attribute vec4 seed;
varying float vAlpha;
varying float vStreak;

vec4 texel(sampler2D t, int i) { return texelFetch(t, ivec2(i, 0), 0); }

bool fallsHere(vec3 p) {
  bool inBox = false;
  for (int i = 0; i < ${MAX_BOXES}; i++) {
    if (i >= uBoxCount) break;
    vec3 lo = texel(uBoxes, 2 * i).xyz, hi = texel(uBoxes, 2 * i + 1).xyz;
    if (all(greaterThanEqual(p, lo)) && all(lessThanEqual(p, hi))) { inBox = true; break; }
  }
  if (!inBox) return false;
  for (int v = 0; v < ${MAX_VOLUMES}; v++) {
    if (v >= uVolumeCount) break;
    vec4 h = texel(uVolumes, v);
    int first = int(h.x), count = int(h.y);
    bool within = true;
    for (int k = 0; k < 64; k++) {
      if (k >= count) break;
      vec4 q = texel(uPlanes, first + k);
      if (dot(q.xyz, p) > q.w) { within = false; break; }
    }
    if (within) return false;
  }
  return true;
}

void main() {
  float t = uTime * (0.8 + 0.4 * seed.w);
  vec3 move = vec3(sin(t * 0.7 + seed.x * 40.0) * uDrift, -uFall * (0.85 + 0.3 * seed.w), cos(t * 0.5 + seed.z * 40.0) * uDrift) * t;
  vec3 p = mod(seed.xyz * uSize + move - uOrigin, uSize) + uOrigin;
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  gl_Position = projectionMatrix * mv;
  float near = smoothstep(0.0, 1.5, -mv.z);
  // Thin out at the edges of the box, so it has none.
  vec3 e = abs((p - uOrigin) / uSize - 0.5) * 2.0;
  float edge = 1.0 - smoothstep(0.75, 1.0, max(e.x, max(e.y, e.z)));
  vAlpha = fallsHere(p) ? near * edge : 0.0;
  vStreak = uStreak;
  gl_PointSize = vAlpha > 0.0 ? clamp(uPoint * 10.0 / max(0.5, -mv.z), 1.0, 64.0) : 0.0;
}
`;

const FRAGMENT = /* glsl */ `
uniform vec3 uColour;
uniform float uAlpha;
varying float vAlpha;
varying float vStreak;
void main() {
  vec2 c = gl_PointCoord - 0.5;
  float a;
  if (vStreak > 0.0) a = (1.0 - smoothstep(0.02, 0.07, abs(c.x))) * (1.0 - smoothstep(0.3, 0.5, abs(c.y)));
  else a = 1.0 - smoothstep(0.2, 0.5, length(c));
  a *= vAlpha * uAlpha;
  if (a < 0.01) discard;
  gl_FragColor = vec4(uColour, a);
}
`;

function floatTexture(data: Float32Array, width: number): THREE.DataTexture {
  const t = new THREE.DataTexture(data, Math.max(1, width), 1, THREE.RGBAFormat, THREE.FloatType);
  t.needsUpdate = true;
  return t;
}

/** One system's particles. */
class Fall {
  readonly points: THREE.Points;
  private material: THREE.ShaderMaterial;
  private textures: THREE.DataTexture[];

  constructor(w: WeatherSystem) {
    const look = LOOK[w.kind];
    const count = Math.round(look.count * w.amount);
    const seeds = new Float32Array(count * 4);
    for (let i = 0; i < seeds.length; i++) seeds[i] = Math.random();
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("seed", new THREE.BufferAttribute(seeds, 4));
    // Positions are made in the shader; three still wants one.
    geo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(count * 3), 3));
    geo.boundingSphere = new THREE.Sphere(new THREE.Vector3(), Infinity);
    const boxes = w.boxes.slice(0, MAX_BOXES);
    const boxData = new Float32Array(Math.max(1, boxes.length * 2) * 4);
    boxes.forEach((b, i) => { boxData.set([b[0], b[1], b[2], 0, b[3], b[4], b[5], 0], i * 8); });
    const volumes = w.exclude.slice(0, MAX_VOLUMES);
    const planes: number[] = [], heads: number[] = [];
    for (const v of volumes) {
      const vs = v.slice(0, 64);
      if (planes.length / 4 + vs.length > MAX_PLANES) break;
      heads.push(planes.length / 4, vs.length, 0, 0);
      for (const q of vs) planes.push(...q);
    }
    this.textures = [
      floatTexture(boxData, boxes.length * 2),
      floatTexture(new Float32Array(planes.length ? planes : [0, 0, 0, 0]), planes.length / 4),
      floatTexture(new Float32Array(heads.length ? heads : [0, 0, 0, 0]), heads.length / 4),
    ];
    this.material = new THREE.ShaderMaterial({
      vertexShader: VERTEX,
      fragmentShader: FRAGMENT,
      transparent: true,
      depthWrite: false,
      blending: THREE.NormalBlending,
      uniforms: {
        uTime: { value: 0 },
        uOrigin: { value: new THREE.Vector3() },
        uSize: { value: 40 },
        uFall: { value: look.fall },
        uDrift: { value: look.drift },
        uPoint: { value: look.size },
        uStreak: { value: look.streak },
        uBoxes: { value: this.textures[0] },
        uBoxCount: { value: boxes.length },
        uPlanes: { value: this.textures[1] },
        uVolumes: { value: this.textures[2] },
        uVolumeCount: { value: heads.length / 4 },
        uColour: { value: new THREE.Color(look.colour) },
        uAlpha: { value: look.alpha },
      },
    });
    this.points = new THREE.Points(geo, this.material);
    this.points.frustumCulled = false;
    this.points.renderOrder = 5;
  }

  /** The box of particles: `size` units across, its corner at `origin`. */
  set(time: number, origin: THREE.Vector3, size: number) {
    const u = this.material.uniforms;
    u.uTime!.value = time;
    (u.uOrigin!.value as THREE.Vector3).copy(origin);
    u.uSize!.value = size;
  }

  dispose() {
    this.points.geometry.dispose();
    this.material.dispose();
    for (const t of this.textures) t.dispose();
  }
}

/** A level's weather: made from its extras, placed each frame. */
export class WeatherLook {
  readonly group = new THREE.Group();
  private falls: Fall[] = [];
  private on = true;
  private origin = new THREE.Vector3();

  constructor() { this.group.name = "weather"; }

  adopt(systems: readonly WeatherSystem[]) {
    this.clear();
    for (const w of systems) {
      const f = new Fall(w);
      this.falls.push(f);
      this.group.add(f.points);
    }
    this.group.visible = this.on;
  }

  /** Whether the level has weather. */
  has(): boolean { return this.falls.length > 0; }
  /** Whether it is drawn now (and the view must keep moving). */
  active(): boolean { return this.on && this.falls.length > 0; }

  setOn(on: boolean) { this.on = on; this.group.visible = on; }

  /** Places the particles around `focus` (what the camera looks at), in a
   *  box that grows with the camera's distance to it. `time` in seconds. */
  place(time: number, focus: THREE.Vector3, distance: number) {
    const size = Math.min(160, Math.max(24, distance * 1.6));
    this.origin.copy(focus).subScalar(size / 2);
    for (const f of this.falls) f.set(time, this.origin, size);
  }

  clear() {
    for (const f of this.falls) { this.group.remove(f.points); f.dispose(); }
    this.falls = [];
  }

  dispose() { this.clear(); }
}
