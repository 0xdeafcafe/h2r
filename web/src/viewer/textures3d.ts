// Textured or flat: the level and its objects drawn with their surface
// pictures, or in the plain class colours (floor, wall, metal, ...) the
// viewer had before. The map glb gives a textured primitive the picture's
// material and names its class material in the primitive's extras
// (`{class, flat}`, docs/live/live-art.md, "Textures"); both are kept per mesh,
// so the switch is instant. The choice is remembered in localStorage.
//
// The pictures are files beside the glb (`tex/<bitmap>.webp`): the glb
// loads without them ([`deferTextures`]), the level is drawn in its class
// colours at once, and each picture is fetched, decoded off the main thread
// and put on its surfaces as it arrives ([`streamTextures`]).
//
// `texturesPref`, `savePref` and `flatOf` are pure; the rest needs
// three.js, so this file is only ever loaded lazily.

import * as THREE from "three";
import type { GLTF, GLTFLoader, GLTFLoaderPlugin, GLTFParser } from "three/examples/jsm/loaders/GLTFLoader.js";

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

/** Whether a material's picture is there to draw: it has none, or its
 *  image has arrived. A picture still on its way samples as black. */
export function pictureReady(m: THREE.Material): boolean {
  const map = (m as THREE.Material & { map?: THREE.Texture | null }).map;
  return !map || map.image != null;
}

/** The meshes that can be drawn either way, and which way they are. A
 *  mesh whose picture has not arrived yet is drawn flat either way. */
export class TextureSwap {
  private pairs: TexturePair[] = [];
  private on = true;

  adopt(pairs: readonly TexturePair[]) {
    for (const p of pairs) this.add(p.mesh, p.textured, p.flat);
  }

  add(mesh: THREE.Mesh, textured: THREE.Material, flat: THREE.Material) {
    this.pairs.push({ mesh, textured, flat });
    mesh.material = this.on && pictureReady(textured) ? textured : flat;
  }

  /** Puts arrived pictures on their meshes. Returns whether any changed. */
  refresh(): boolean {
    if (!this.on) return false;
    let changed = false;
    for (const p of this.pairs) {
      if (p.mesh.material === p.flat && pictureReady(p.textured)) { p.mesh.material = p.textured; changed = true; }
    }
    return changed;
  }

  /** The pair a mesh is in, so a copy of it (an instanced batch) can join. */
  pairOf(mesh: THREE.Mesh): TexturePair | undefined {
    return this.pairs.find((p) => p.mesh === mesh);
  }

  set(on: boolean): boolean {
    if (on === this.on) return false;
    this.on = on;
    for (const p of this.pairs) p.mesh.material = on && pictureReady(p.textured) ? p.textured : p.flat;
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

// ---- pictures beside the glb ----

/** A picture the glb names by `uri`: where it is, and the texture its
 *  materials hold (with any copies a texture transform made of it). */
export interface PendingTexture { url: string; texture: THREE.Texture }

const WRAP: Record<number, THREE.Wrapping> = { 33071: THREE.ClampToEdgeWrapping, 33648: THREE.MirroredRepeatWrapping, 10497: THREE.RepeatWrapping };
const FILTER: Record<number, THREE.TextureFilter> = {
  9728: THREE.NearestFilter, 9729: THREE.LinearFilter, 9984: THREE.NearestMipmapNearestFilter,
  9985: THREE.LinearMipmapNearestFilter, 9986: THREE.NearestMipmapLinearFilter, 9987: THREE.LinearMipmapLinearFilter,
};

/** Where a glb's relative `uri` is: beside the glb, with the glb's query
 *  (the art's version stamp, so both are cached alike). */
export function pictureUrl(glbUrl: string, uri: string): string {
  const q = glbUrl.indexOf("?");
  const path = q >= 0 ? glbUrl.slice(0, q) : glbUrl;
  const query = q >= 0 ? glbUrl.slice(q) : "";
  if (/^([a-z][a-z0-9+.-]*:|\/)/i.test(uri)) return uri;
  return path.slice(0, path.lastIndexOf("/") + 1) + uri + query;
}

interface GltfJson {
  textures?: Array<{ source?: number; sampler?: number }>;
  images?: Array<{ uri?: string; bufferView?: number }>;
  samplers?: Array<{ magFilter?: number; minFilter?: number; wrapS?: number; wrapT?: number }>;
}

/** A GLTFLoader plugin: a texture whose image is a file (not in the glb's
 *  binary chunk) is handed out at once without its image, and listed in
 *  `out` for [`streamTextures`]. Embedded images load as usual. */
class DeferredTextures implements GLTFLoaderPlugin {
  readonly name = "h2_deferred_textures";
  constructor(private parser: GLTFParser, private glbUrl: string, private out: PendingTexture[]) {}

  loadTexture(index: number): Promise<THREE.Texture> | null {
    const json = this.parser.json as GltfJson;
    const def = json.textures?.[index];
    const image = def?.source !== undefined ? json.images?.[def.source] : undefined;
    if (!image?.uri || image.uri.startsWith("data:")) return null;
    const texture = new THREE.Texture();
    texture.flipY = false;
    texture.name = image.uri;
    const sampler = def?.sampler !== undefined ? json.samplers?.[def.sampler] : undefined;
    texture.magFilter = (FILTER[sampler?.magFilter ?? 9729] ?? THREE.LinearFilter) as THREE.MagnificationTextureFilter;
    texture.minFilter = FILTER[sampler?.minFilter ?? 9987] ?? THREE.LinearMipmapLinearFilter;
    texture.wrapS = WRAP[sampler?.wrapS ?? 10497] ?? THREE.RepeatWrapping;
    texture.wrapT = WRAP[sampler?.wrapT ?? 10497] ?? THREE.RepeatWrapping;
    texture.generateMipmaps = true;
    this.parser.associations.set(texture, { textures: index });
    this.out.push({ url: pictureUrl(this.glbUrl, image.uri), texture });
    return Promise.resolve(texture);
  }
}

/** The anisotropic filtering level for a level's pictures, given the
 *  GPU's most: all of it, up to 16 (where the gain stops). */
export function mapAnisotropy(max: number): number {
  return Number.isFinite(max) && max >= 1 ? Math.min(16, Math.floor(max)) : 1;
}

/** Makes `loader` leave a glb's picture files for later: the returned list
 *  fills in while it parses `glbUrl`. */
export function deferTextures(loader: GLTFLoader, glbUrl: string): PendingTexture[] {
  const out: PendingTexture[] = [];
  loader.register((parser) => new DeferredTextures(parser, glbUrl, out));
  return out;
}

/** A decoded picture three can upload. */
export type Picture = ImageBitmap | HTMLImageElement;

/** Fetches and decodes a picture off the main thread (`createImageBitmap`),
 *  as the glTF loader would (no flip, straight alpha). Null on failure. */
export async function fetchPicture(url: string, signal?: AbortSignal): Promise<Picture | null> {
  try {
    const res = await fetch(url, { signal });
    if (!res.ok) return null;
    const blob = await res.blob();
    if (typeof createImageBitmap === "function") {
      return await createImageBitmap(blob, { imageOrientation: "none", premultiplyAlpha: "none", colorSpaceConversion: "none" });
    }
    const img = new Image();
    img.src = URL.createObjectURL(blob);
    try { await img.decode(); } finally { URL.revokeObjectURL(img.src); }
    return img;
  } catch {
    return null;
  }
}

/** Every texture that draws `texture`'s image: itself and the copies a
 *  texture transform made (they share its source). */
export function texturesSharing(texture: THREE.Texture, materials: Iterable<THREE.Material>): THREE.Texture[] {
  const out = new Set<THREE.Texture>([texture]);
  for (const m of materials) {
    const map = (m as THREE.Material & { map?: THREE.Texture | null }).map;
    if (map && map.source === texture.source) out.add(map);
  }
  return [...out];
}

/** Fetches the pending pictures, `concurrency` at a time, and hands each
 *  to `arrived` as it is decoded (in the order they finish). Stops early
 *  when `stopped()` says so. Resolves with how many arrived. */
export async function streamTextures(
  pending: readonly PendingTexture[],
  arrived: (p: PendingTexture, picture: Picture) => void,
  opts: { concurrency?: number; load?: (url: string) => Promise<Picture | null>; stopped?: () => boolean } = {},
): Promise<number> {
  const load = opts.load ?? ((u: string) => fetchPicture(u));
  const stopped = opts.stopped ?? (() => false);
  let next = 0, count = 0;
  const worker = async () => {
    while (next < pending.length && !stopped()) {
      const p = pending[next++]!;
      const picture = await load(p.url);
      if (stopped()) return;
      if (picture) { arrived(p, picture); count++; }
    }
  };
  await Promise.all(Array.from({ length: Math.max(1, opts.concurrency ?? 6) }, worker));
  return count;
}
