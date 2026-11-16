// Textured or flat: the level and its objects drawn with their surface
// pictures, or in the plain class colours (floor, wall, metal, ...) the
// viewer had before. The map glb gives a textured primitive the picture's
// material and names its class material in the primitive's extras
// (`{class, flat}`, docs/live/live-art.md, "Textures"); both are kept per mesh,
// so the switch is instant. The choice is remembered in localStorage.
//
// `texturesPref`, `savePref` and `flatOf` are pure; `loadFlats` and
// `TextureSwap` need three.js, so this file is only ever loaded lazily.

import type * as THREE from "three";
import type { GLTF } from "three/examples/jsm/loaders/GLTFLoader.js";

export const TEXTURES_KEY = "h2.viewer.textures";

interface Store { getItem(k: string): string | null; setItem(k: string, v: string): void }

function store(): Store | null {
  try { return typeof localStorage === "undefined" ? null : localStorage; } catch { return null; }
}

/** Whether textures are on: on unless turned off before. */
export function texturesPref(s: Store | null = store()): boolean {
  try { return s?.getItem(TEXTURES_KEY) !== "off"; } catch { return true; }
}

export function savePref(on: boolean, s: Store | null = store()) {
  try { s?.setItem(TEXTURES_KEY, on ? "on" : "off"); } catch { /* private mode: not remembered */ }
}

/** The class material a textured primitive names (its glTF material index),
 *  from the primitive's extras (three's geometry `userData`). */
export function flatOf(extras: unknown): number | null {
  const f = (extras as { flat?: unknown } | null | undefined)?.flat;
  return typeof f === "number" && Number.isInteger(f) && f >= 0 ? f : null;
}

/** Per mesh of a loaded glb whose primitive is textured, its class
 *  material (loaded through the glb's parser, so shared like any other). */
export async function loadFlats(gltf: GLTF): Promise<Map<THREE.Mesh, THREE.Material>> {
  const want: Array<[THREE.Mesh, number]> = [];
  gltf.scene.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh || Array.isArray(mesh.material)) return;
    const f = flatOf(mesh.geometry.userData);
    if (f !== null) want.push([mesh, f]);
  });
  const out = new Map<THREE.Mesh, THREE.Material>();
  await Promise.all(want.map(async ([mesh, f]) => {
    try {
      out.set(mesh, await gltf.parser.getDependency("material", f) as THREE.Material);
    } catch { /* no class material: the mesh stays textured */ }
  }));
  return out;
}

export interface TexturePair { mesh: THREE.Mesh; textured: THREE.Material; flat: THREE.Material }

/** The meshes that can be drawn either way, and which way they are. */
export class TextureSwap {
  private pairs: TexturePair[] = [];
  private on = true;

  adopt(pairs: readonly TexturePair[]) {
    for (const p of pairs) this.add(p.mesh, p.textured, p.flat);
  }

  add(mesh: THREE.Mesh, textured: THREE.Material, flat: THREE.Material) {
    this.pairs.push({ mesh, textured, flat });
    mesh.material = this.on ? textured : flat;
  }

  /** The pair a mesh is in, so a copy of it (an instanced batch) can join. */
  pairOf(mesh: THREE.Mesh): TexturePair | undefined {
    return this.pairs.find((p) => p.mesh === mesh);
  }

  set(on: boolean): boolean {
    if (on === this.on) return false;
    this.on = on;
    for (const p of this.pairs) p.mesh.material = on ? p.textured : p.flat;
    return this.pairs.length > 0;
  }

  get enabled(): boolean { return this.on; }
  /** Whether the level has anything textured. */
  has(): boolean { return this.pairs.length > 0; }

  /** Forgets the meshes; disposes the class materials no mesh holds now
   *  (the ones in use go with the map). */
  clear() {
    for (const p of this.pairs) if (p.mesh.material !== p.flat) p.flat.dispose();
    for (const p of this.pairs) if (p.mesh.material !== p.textured) p.textured.dispose();
    this.pairs = [];
  }
}
