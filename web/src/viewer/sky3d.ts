// A level's sky and fog in the 3D view. The map glb's `asset.extras.sky`
// names the sky, its model (`sky.glb`, beside the map glb), its clear
// colour and its fog (docs/live/live-art.md, "Sky"). The model is drawn around
// the camera, behind everything (no depth), moving with the camera but not
// turning with it; behind it is the clear colour, else the fog's. The fog
// is the game's, measured from the point the camera looks at (the player
// followed, or the middle of the view), so what is in focus reads as it
// does in the game and what lies further off fades. `skyOf`, `skyColour`
// and `fogRange` are pure.
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

export interface SkyInfo {
  name: string;
  /** The sky model, relative to the map glb. */
  glb?: string;
  clear?: [number, number, number];
  fog?: SkyFog;
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
  return out;
}

/** What is behind the sky model: the clear colour, else the fog's, else
 *  null (the viewer's own sky). sRGB 0..1. */
export function skyColour(sky: SkyInfo | null): [number, number, number] | null {
  return sky?.clear ?? sky?.fog?.colour ?? null;
}

/** Linear fog from `near` to `far` (scene distances from the camera) that
 *  is as thick as the game's at the same distance from the point in focus
 *  `focus` units away: none up to `start` past it, `density` at `end`.
 *  The game's fog stops at `density`; three's goes on to full, so when the
 *  level reaches further than that (`reach`, the camera's distance to the
 *  level's far side) the fog is stretched to be no thicker than `density`
 *  there: a view from afar fades the far side, it does not hide it. */
export function fogRange(fog: SkyFog, focus: number, reach = 0): { near: number; far: number } {
  const d = Math.max(0, focus);
  const near = d + fog.start;
  const density = Math.max(0.05, fog.density);
  const far = near + (fog.end - fog.start) / density;
  return { near, far: reach > near ? Math.max(far, near + (reach - near) / density) : far };
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
