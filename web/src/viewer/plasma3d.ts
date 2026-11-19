// Additive level plasma, including lift beams. The map glTF carries the
// pass's two noise textures, mask, texture transforms and colour constants.

import * as THREE from "three";
import type { GLTF } from "three/examples/jsm/loaders/GLTFLoader.js";

type Pair = [number, number];
type RGB = [number, number, number];
interface Scroll { from: number; to: number; period: number }
interface Transform { scale: Pair; offset: Pair; scroll: [Scroll | null, Scroll | null] }
interface PlasmaSpec { noise: Pair; uv: [Transform, Transform, Transform]; colors: [RGB, RGB, RGB] }

const finite = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const pair = (v: unknown): v is Pair => Array.isArray(v) && v.length === 2 && v.every(finite);
const rgb = (v: unknown): v is RGB => Array.isArray(v) && v.length === 3 && v.every((n) => finite(n) && n >= 0 && n <= 1);
const scroll = (v: unknown): v is Scroll => {
  const s = v as Partial<Scroll> | null | undefined;
  return !!s && finite(s.from) && finite(s.to) && finite(s.period) && s.period > 0;
};
const transform = (v: unknown): v is Transform => {
  const u = v as Partial<Transform> | null | undefined;
  return !!u && pair(u.scale) && pair(u.offset) && Array.isArray(u.scroll) &&
    u.scroll.length === 2 && u.scroll.every((s) => s === null || scroll(s));
};

export function plasmaSpec(extras: unknown): PlasmaSpec | null {
  const { plasma, noise } = (extras ?? {}) as { plasma?: Partial<PlasmaSpec>; noise?: unknown };
  if (!pair(noise) || !noise.every((i) => Number.isSafeInteger(i) && i >= 0)) return null;
  const { uv, colors } = plasma ?? {};
  if (!Array.isArray(uv) || uv.length !== 3 || !uv.every(transform)) return null;
  if (!Array.isArray(colors) || colors.length !== 3 || !colors.every(rgb)) return null;
  return { noise, uv, colors };
}

export function plasmaOffset(s: Scroll, time: number): number {
  const phase = (time % s.period) / s.period;
  return s.from + (s.to - s.from) * phase;
}

const VARYING = /* glsl */ "varying vec2 vPlasmaUv;";
const FRAGMENT_HEAD = /* glsl */ `
${VARYING}
uniform sampler2D plasmaA;
uniform sampler2D plasmaB;
uniform vec4 plasmaUv[3];
uniform vec3 plasmaColor[3];
vec2 plasmaTransform(vec4 transform) {
  return vPlasmaUv * transform.xy + transform.zw;
}
`;
const FRAGMENT_MAP = /* glsl */ `
float a = texture2D(plasmaA, plasmaTransform(plasmaUv[0])).b;
float b = texture2D(plasmaB, plasmaTransform(plasmaUv[1])).b;
float coverage = texture2D(map, plasmaTransform(plasmaUv[2])).a;
float q = 0.5 - abs(b - a);
float intensity = clamp(4.0 * q * q, 0.0, 1.0);
vec3 tint = mix(plasmaColor[1], plasmaColor[0], intensity) + plasmaColor[2];
// The combiner and tint work on stored values. Three's output stage
// expects linear colour. The mask shapes the contribution to RGB.
diffuseColor = sRGBTransferEOTF(vec4(clamp(tint * intensity * coverage, 0.0, 1.0), 1.0));
`;

interface AnimatedOffset { row: THREE.Vector4; u: Scroll | null; v: Scroll | null }
interface PlasmaMaterial {
  material: THREE.MeshBasicMaterial;
  textures: THREE.Texture[];
  offsets: AnimatedOffset[];
}

function dress(material: THREE.MeshBasicMaterial, map: THREE.Texture, noise: [THREE.Texture, THREE.Texture], spec: PlasmaSpec): PlasmaMaterial {
  material.transparent = true;
  material.blending = THREE.CustomBlending;
  material.blendEquation = THREE.AddEquation;
  material.blendSrc = THREE.OneFactor;
  material.blendDst = THREE.OneFactor;
  material.blendSrcAlpha = THREE.ZeroFactor;
  material.blendDstAlpha = THREE.OneFactor;
  material.depthTest = true;
  material.depthWrite = false;
  material.side = THREE.DoubleSide;
  material.forceSinglePass = true;
  material.fog = false;
  material.toneMapped = false;

  const uniforms = {
    plasmaA: { value: noise[0] },
    plasmaB: { value: noise[1] },
    plasmaUv: { value: spec.uv.map(({ scale, offset, scroll: [u, v] }) =>
      new THREE.Vector4(...scale, u?.from ?? offset[0], v?.from ?? offset[1])) },
    plasmaColor: { value: spec.colors.map((rgb) => new THREE.Vector3(...rgb)) },
  };
  const before = material.onBeforeCompile;
  material.onBeforeCompile = (shader, renderer) => {
    before.call(material, shader, renderer);
    Object.assign(shader.uniforms, uniforms);
    // Read untransformed UVs. The GLTFLoader can already have applied the
    // mask's tiling to vMapUv, while each of these three stages has its own.
    shader.vertexShader = VARYING + "\n" + shader.vertexShader
      .replace("#include <uv_vertex>", "#include <uv_vertex>\n vPlasmaUv = uv;");
    shader.fragmentShader = FRAGMENT_HEAD + shader.fragmentShader.replace("#include <map_fragment>", FRAGMENT_MAP);
  };
  material.customProgramCacheKey = () => "h2-plasma";
  material.needsUpdate = true;
  const offsets = spec.uv.flatMap(({ scroll: [u, v] }, i) => u || v ? [{ row: uniforms.plasmaUv.value[i]!, u, v }] : []);
  return { material, textures: [...noise, map], offsets };
}

export class LevelPlasma {
  readonly textures: ReadonlySet<THREE.Texture>;
  private shown = true;
  private time = 0;

  constructor(private materials: ReadonlyMap<THREE.Material, PlasmaMaterial | null>) {
    this.textures = new Set([...materials.values()].flatMap((look) => look?.textures ?? []));
    this.refresh();
  }

  has(material: THREE.Material): boolean { return this.materials.has(material); }
  any(): boolean { return this.materials.size > 0; }

  setShown(shown: boolean) {
    this.shown = shown;
    this.refresh();
  }

  /** Called when deferred pictures arrive, including after a failed fetch. */
  refresh() {
    for (const [material, look] of this.materials) {
      const visible = this.shown && !!look && look.textures.every((t) => t.image != null);
      if (visible && !material.visible) this.time = NaN;
      material.visible = visible;
    }
  }

  /** Update just the scrolling materials, once per material shared by the
   *  level or its instances. Says whether they need another frame. */
  animate(time: number): boolean {
    if (!this.shown || time === this.time) return false;
    this.time = time;
    let moving = false;
    for (const look of this.materials.values()) {
      if (!look?.material.visible) continue;
      for (const { row, u, v } of look.offsets) {
        if (u) row.z = plasmaOffset(u, time);
        if (v) row.w = plasmaOffset(v, time);
        moving = true;
      }
    }
    return moving;
  }
}

/** Resolve the auxiliary textures through the same parser as the mask.
 *  For map GLBs the deferred texture loader collects their URLs here. */
export async function loadPlasma(gltf: GLTF): Promise<LevelPlasma> {
  const materials = new Map<THREE.Material, PlasmaMaterial | null>();
  const loads: Promise<void>[] = [];
  gltf.scene.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh || !(mesh.material instanceof THREE.MeshBasicMaterial)) return;
    const material = mesh.material;
    const mask = material.map;
    if (!mask || materials.has(material)) return;
    const spec = plasmaSpec(material.userData);
    if (!spec) return;
    materials.set(material, null);
    const get = (i: number): Promise<THREE.Texture | null> => gltf.parser.getDependency("texture", i);
    loads.push(Promise.all([get(spec.noise[0]), get(spec.noise[1])]).then(([a, b]) => {
      if (a && b) {
        a.colorSpace = b.colorSpace = THREE.NoColorSpace;
        materials.set(material, dress(material, mask, [a, b], spec));
      }
    }, () => { /* a missing auxiliary image leaves this effect hidden */ }));
  });
  await Promise.all(loads);
  return new LevelPlasma(materials);
}
