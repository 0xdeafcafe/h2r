// A level's sky and fog in the 3D view. The map glb's `asset.extras.sky`
// names the sky, its model (`sky.glb`, beside the map glb), its clear
// colour and its fog (docs/live/live-art.md, "Sky"). The model is drawn around
// the camera, behind everything (no depth), moving with the camera but not
// turning with it; behind it is the clear colour, else the fog's. The fog
// is the game's atmospheric fog (linear from its start to its opaque
// distance, never thicker than its maximum density: `useGameFog` patches
// three's fog to that), measured from the point the camera looks at (the
// player followed, or the middle of the view), so what is in focus reads
// as it does in the game and what lies further off fades. `skyOf`,
// `skyColour`, `gameFog` and `fogAmount` are pure.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";

export interface SkyFog {
  colour: [number, number, number];
  /** How thick it gets (0..1), reached at `end`. */
  density: number;
  start: number;
  end: number;
}

/** The sun-lit air of the sky's light: its colour, how thick (0..1), and
 *  the light's heading and pitch (radians, game frame). */
export interface SkyGlow {
  colour: [number, number, number];
  density: number;
  heading: number;
  pitch: number;
}

export interface SkyInfo {
  name: string;
  /** The sky model, relative to the map glb. */
  glb?: string;
  clear?: [number, number, number];
  fog?: SkyFog;
  glow?: SkyGlow;
}

const rgb = (v: unknown): [number, number, number] | undefined =>
  Array.isArray(v) && v.length === 3 && v.every((x) => typeof x === "number" && Number.isFinite(x)) ? (v as [number, number, number]) : undefined;

/** The sky from a map glb's `asset.extras`, or null. */
export function skyOf(assetExtras: unknown): SkyInfo | null {
  const s = (assetExtras as { sky?: unknown } | null | undefined)?.sky as Record<string, unknown> | undefined;
  if (!s || typeof s !== "object") return null;
  const out: SkyInfo = { name: typeof s.name === "string" ? s.name : "" };
  if (typeof s.glb === "string" && /^[\w.\-/]+\.glb$/.test(s.glb) && !s.glb.includes("..")) out.glb = s.glb;
  const clear = rgb(s.clear);
  if (clear) out.clear = clear;
  const f = s.fog as Record<string, unknown> | undefined;
  const fc = rgb(f?.colour);
  if (f && fc && [f.density, f.start, f.end].every((x) => typeof x === "number" && Number.isFinite(x)) && (f.end as number) > (f.start as number) && (f.density as number) > 0) {
    out.fog = { colour: fc, density: Math.min(1, f.density as number), start: Math.max(0, f.start as number), end: f.end as number };
  }
  const g = s.glow as Record<string, unknown> | undefined;
  const gc = rgb(g?.colour);
  if (g && gc && [g.density, g.heading, g.pitch].every((x) => typeof x === "number" && Number.isFinite(x)) && (g.density as number) > 0) {
    out.glow = { colour: gc, density: Math.min(1, g.density as number), heading: g.heading as number, pitch: g.pitch as number };
  }
  return out;
}

/** The exponential haze for a level without fog but with a glow, in a
 *  level `radius` across: the glow's density 0.2 gives about a quarter of
 *  its colour at the level's far side; thicker glows give more. */
export function glowHaze(glow: SkyGlow, radius: number): number {
  const d = Math.min(1, Math.max(0.05, glow.density));
  return (1.3 * d) / Math.max(1, radius);
}

/** What is behind the sky model: the clear colour, else the fog's, else
 *  null (the viewer's own sky). sRGB 0..1. */
export function skyColour(sky: SkyInfo | null): [number, number, number] | null {
  return sky?.clear ?? sky?.fog?.colour ?? null;
}

/** The game's atmospheric fog as the view draws it: none up to `near`,
 *  thickening linearly to `cap` (the sky's maximum density) at `far`, and
 *  no thicker beyond (scene distances from the camera). Unlike three's
 *  linear fog, which smoothsteps on to full, a level seen from afar fades
 *  its far side, it does not hide it. */
export interface GameFog { near: number; far: number; cap: number }

/** The game's fog measured from the point in focus `focus` units away, so
 *  what is in focus reads as it does in the game: clear to `start` past
 *  it, `density` at `end` past it and beyond. */
export function gameFog(fog: SkyFog, focus: number): GameFog {
  const d = Math.max(0, focus);
  return { near: d + fog.start, far: d + Math.max(fog.end, fog.start + 1e-3), cap: Math.min(1, Math.max(0, fog.density)) };
}

/** How much of the fog's colour a point `depth` away takes (0..cap): what
 *  the patched `fog_fragment` computes. */
export function fogAmount(depth: number, f: GameFog): number {
  const t = (depth - f.near) / Math.max(1e-4, f.far - f.near);
  return f.cap * Math.min(1, Math.max(0, t));
}

/** The shared uniform every material's fog reads: 1 - the fog's cap, so a
 *  material that never got it (0) fogs fully, as three's own fog does. */
export const FOG_CLEAR = { value: 0 };

const FOG_PARS_FIND = "uniform float fogFar;";
const FOG_FIND = "float fogFactor = smoothstep( fogNear, fogFar, vFogDepth );";
const FOG_PATCH = "float fogFactor = ( 1.0 - fogClear ) * clamp( ( vFogDepth - fogNear ) / max( fogFar - fogNear, 1e-4 ), 0.0, 1.0 );";
let fogPatched = false;

/** Turns three's linear fog into the game's (`fogAmount`), for every
 *  material: the fog chunks are patched once and every material's
 *  `onBeforeCompile` hands its shader the shared `FOG_CLEAR` (one that
 *  replaces `onBeforeCompile` must chain the prototype's, as the viewer's
 *  shader patches do). The exponential haze is left as it is. Says whether
 *  the chunks were as expected (a three.js that changed them keeps its
 *  own fog). */
export function useGameFog(): boolean {
  if (fogPatched) return true;
  const chunks = THREE.ShaderChunk as unknown as Record<string, string>;
  if (!chunks.fog_pars_fragment?.includes(FOG_PARS_FIND) || !chunks.fog_fragment?.includes(FOG_FIND)) return false;
  chunks.fog_pars_fragment = chunks.fog_pars_fragment.replace(FOG_PARS_FIND, FOG_PARS_FIND + "\n\t\tuniform float fogClear;");
  chunks.fog_fragment = chunks.fog_fragment.replace(FOG_FIND, FOG_PATCH);
  const base = THREE.Material.prototype.onBeforeCompile;
  THREE.Material.prototype.onBeforeCompile = function (this: THREE.Material, shader, renderer) {
    base.call(this, shader, renderer);
    shader.uniforms.fogClear = FOG_CLEAR;
  };
  fogPatched = true;
  return true;
}

/** The sky glb's address beside the map's, with the map's query (the
 *  art's version stamp). */
export function skyUrl(mapUrl: string, glb: string): string {
  const q = mapUrl.indexOf("?");
  const path = q >= 0 ? mapUrl.slice(0, q) : mapUrl;
  const cut = path.lastIndexOf("/");
  return (cut >= 0 ? path.slice(0, cut + 1) : "") + glb + (q >= 0 ? mapUrl.slice(q) : "");
}

/** The sky model, drawn first and around the camera. */
export class SkyDome {
  readonly group = new THREE.Group();
  private radius = 1;

  constructor() {
    this.group.name = "sky";
    this.group.userData.noAo = true;
  }

  /** Loads the model. Resolves false when it can't be. */
  async load(url: string): Promise<boolean> {
    let gltf;
    try {
      gltf = await new GLTFLoader().loadAsync(url);
    } catch {
      return false;
    }
    const blend = ((gltf.asset as { extras?: { blend?: unknown } }).extras?.blend ?? []) as unknown[];
    const parser = gltf.parser as unknown as { associations: Map<THREE.Material, { materials?: number }> };
    const meshes: Array<{ mesh: THREE.Mesh; size: number }> = [];
    gltf.scene.updateMatrixWorld(true);
    const box = new THREE.Box3().setFromObject(gltf.scene);
    this.radius = Math.max(1, box.getBoundingSphere(new THREE.Sphere()).radius + box.getCenter(new THREE.Vector3()).length());
    gltf.scene.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (!mesh.isMesh) return;
      const src = mesh.material as THREE.MeshStandardMaterial;
      const i = parser.associations?.get(src)?.materials;
      const how = typeof i === "number" ? blend[i] : undefined;
      const m = new THREE.MeshBasicMaterial({
        map: src.map ?? null,
        color: src.map ? 0xffffff : src.color,
        side: THREE.DoubleSide,
        depthTest: false,
        depthWrite: false,
        fog: false,
        toneMapped: false,
      });
      // Drawn with the opaque things, first: blending is custom so it is on
      // there too (three only blends ordinary materials when transparent).
      // Additive layers (haze, glows) are left out: without the game's
      // shader constants they wash the whole sky out to white.
      if (how === "additive") mesh.visible = false;
      if (how === "alpha") {
        m.blending = THREE.CustomBlending;
        m.blendSrc = THREE.SrcAlphaFactor;
        m.blendDst = THREE.OneMinusSrcAlphaFactor;
      } else {
        m.blending = THREE.NoBlending;
      }
      src.dispose();
      mesh.material = m;
      mesh.frustumCulled = false;
      mesh.castShadow = mesh.receiveShadow = false;
      mesh.userData.noAo = true;
      mesh.geometry.computeBoundingSphere();
      meshes.push({ mesh, size: mesh.geometry.boundingSphere?.radius ?? 0 });
    });
    // The biggest (the dome) first, the nearer layers over it.
    meshes.sort((a, b) => b.size - a.size).forEach(({ mesh }, k) => { mesh.renderOrder = -1000 + k; });
    this.group.add(gltf.scene);
    return meshes.length > 0;
  }

  /** Puts the sky around the camera, inside its far plane. */
  follow(camera: THREE.PerspectiveCamera) {
    this.group.position.copy(camera.position);
    this.group.scale.setScalar((camera.far * 0.5) / this.radius);
  }

  dispose() {
    this.group.traverse((o) => {
      const mesh = o as THREE.Mesh;
      if (!mesh.isMesh) return;
      mesh.geometry.dispose();
      const m = mesh.material as THREE.MeshBasicMaterial;
      m.map?.dispose();
      m.dispose();
    });
    this.group.removeFromParent();
  }
}
