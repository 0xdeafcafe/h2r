// Player and weapon models for the 3D view: each glb is fetched once and
// kept as a template, and every player gets a clone of it that shares the
// template's geometry and textures. A biped is coloured by multiplying its
// texture by a per-vertex colour worked out from the glb's change-colour
// weights (viewer/models.ts); that colour set is made once per player
// colour, so team mates share it. A skinned biped (models version 2) gets
// its own bones and a BipedAnimator to play the glb's clips on them; a
// static one stays in the pose it was exported in.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { clone as cloneSkinned } from "three/examples/jsm/utils/SkeletonUtils.js";
import { AIM_SHARES, MAX_PITCH, MAX_TWIST } from "./anim";
import { EMPTY_VEHICLE_SHADE, bodyTint, findNode, secondaryOf, tintColours, type Rgb } from "./models";

export interface BipedTemplate {
  kind: "biped";
  root: THREE.Object3D;
  /** Feet to the top of the head, game units. */
  height: number;
  /** Whether the glb marks what takes the player's colour. */
  regions: boolean;
  /** The glb's animation clips by name (`<weapon_class>:<state>`); empty for a static model. */
  clips: Map<string, THREE.AnimationClip>;
  skinned: boolean;
}

export interface WeaponTemplate {
  kind: "weapon";
  root: THREE.Object3D;
}

export interface VehicleTemplate {
  kind: "vehicle";
  root: THREE.Object3D;
  /** Its top above its origin, game units: where a label clears it. */
  height: number;
}

export interface BipedInstance {
  object: THREE.Object3D;
  /** The eye point. */
  head: THREE.Object3D;
  /** Where a held weapon's origin goes. */
  hand: THREE.Object3D;
  height: number;
  /** Plays the glb's clips on this copy; null for a static (version 1) model. */
  animator: BipedAnimator | null;
}

/** One biped's clips on one copy of its model: a looping clip that
 *  cross-fades when it changes, and one-shots played once over it. */
export class BipedAnimator {
  readonly mixer: THREE.AnimationMixer;
  readonly names: ReadonlySet<string>;
  private actions = new Map<string, THREE.AnimationAction>();
  private loop: string | null = null;
  private shot: THREE.AnimationAction | null = null;
  /** How strongly each looping clip shows: the current one rises to 1, the rest fall away. */
  private weights = new Map<string, number>();

  /** The bones the aim turns, their shares, and their pose before it was added. */
  private aimBones: Array<{ bone: THREE.Object3D; pitch: number; twist: number; base: THREE.Quaternion }> = [];
  private pitch = 0;
  private twist = 0;

  constructor(root: THREE.Object3D, private clips: ReadonlyMap<string, THREE.AnimationClip>) {
    this.mixer = new THREE.AnimationMixer(root);
    this.names = new Set(clips.keys());
    // Parents before children: the order the shares are listed in.
    for (const [re, pitch, twist] of AIM_SHARES) {
      let bone: THREE.Object3D | null = null;
      root.traverse((o) => { if (!bone && re.test(o.name) && !(o as THREE.Mesh).isMesh) bone = o; });
      if (bone) this.aimBones.push({ bone, pitch, twist, base: (bone as THREE.Object3D).quaternion.clone() });
    }
    // Whatever bones there are carry the whole aim between them.
    const sp = this.aimBones.reduce((a, b) => a + b.pitch, 0), st = this.aimBones.reduce((a, b) => a + b.twist, 0);
    for (const a of this.aimBones) { a.pitch /= sp || 1; a.twist /= st || 1; }
  }

  /** Points the upper body: `pitch` up (radians) and `twist` (radians, the
   *  turn back from the legs to the aim, about the model's up), shared out
   *  over the spine, neck and head on top of whatever clip is playing. */
  aim(pitch: number, twist: number): void {
    this.pitch = Math.max(-MAX_PITCH, Math.min(MAX_PITCH, pitch));
    this.twist = Math.max(-MAX_TWIST, Math.min(MAX_TWIST, twist));
  }

  private static q0 = new THREE.Quaternion();
  private static q1 = new THREE.Quaternion();
  private static q2 = new THREE.Quaternion();
  private static AXIS_Y = new THREE.Vector3(0, 1, 0);
  private static AXIS_Z = new THREE.Vector3(0, 0, 1);

  /** Adds the aim to the bones the mixer just posed. The model faces +x
   *  with +y up, so pitching up turns about +z and twisting about +y; each
   *  turn is taken in the model's frame and brought into the bone's parent's. */
  private applyAim(): void {
    for (const a of this.aimBones) a.base.copy(a.bone.quaternion);
    if (!this.pitch && !this.twist) return;
    const root = this.mixer.getRoot() as THREE.Object3D;
    root.updateWorldMatrix(true, true);
    const rootInv = root.getWorldQuaternion(BipedAnimator.q0).invert();
    const turn = BipedAnimator.q1, rel = BipedAnimator.q2;
    for (const a of this.aimBones) {
      if (!a.bone.parent) continue;
      turn.setFromAxisAngle(BipedAnimator.AXIS_Y, this.twist * a.twist)
        .multiply(new THREE.Quaternion().setFromAxisAngle(BipedAnimator.AXIS_Z, this.pitch * a.pitch));
      // The parent's turn within the model: the aim, brought into its frame.
      a.bone.parent.getWorldQuaternion(rel).premultiply(rootInv);
      const delta = rel.clone().invert().multiply(turn).multiply(rel);
      a.bone.quaternion.premultiply(delta);
      a.bone.updateMatrixWorld(true);
    }
  }

  /** Takes the aim back off, so the mixer (or a still model) starts from the clip's own pose. */
  private clearAim(): void {
    for (const a of this.aimBones) a.bone.quaternion.copy(a.base);
  }

  private action(name: string): THREE.AnimationAction | null {
    let a = this.actions.get(name);
    if (!a) {
      const clip = this.clips.get(name);
      if (!clip) return null;
      a = this.mixer.clipAction(clip);
      this.actions.set(name, a);
    }
    return a;
  }

  /** A clip's length in ms, or null when the glb hasn't got it. */
  duration(name: string | null): number | null {
    const c = name ? this.clips.get(name) : undefined;
    return c ? c.duration * 1000 : null;
  }

  /** The looping clip now playing. */
  get looping(): string | null { return this.loop; }

  /** Makes `name` the looping clip (null: back to the bind pose); the last
   *  one fades out as it fades in. `hold` plays it once and stays on its
   *  last frame (dying). */
  setLoop(name: string | null, timeScale = 1, hold = false): void {
    if (name !== this.loop) {
      const to = name ? this.action(name) : null;
      if (to && name) {
        to.reset();
        to.setLoop(hold ? THREE.LoopOnce : THREE.LoopRepeat, Infinity);
        to.clampWhenFinished = hold;
        // The first clip starts at full strength: there is nothing to fade from but the bind pose.
        const w = this.weights.size ? this.weights.get(name) ?? 0 : 1;
        this.weights.set(name, w);
        to.setEffectiveWeight(w).play();
      }
      this.loop = to ? name : null;
    }
    const cur = this.loop ? this.action(this.loop) : null;
    if (cur) cur.timeScale = timeScale;
  }

  /** Plays a clip once over the loop; false when the glb hasn't got it.
   *  `upperOnly` leaves the legs to the loop: a reload on the run keeps running. */
  playOnce(name: string | null, upperOnly = false): boolean {
    let a = name ? this.action(name) : null;
    if (!a || !name) return false;
    if (upperOnly && a.getClip().blendMode !== THREE.AdditiveAnimationBlendMode) {
      const key = name + "|upper";
      let up = this.actions.get(key);
      if (!up) { up = this.mixer.clipAction(upperBody(a.getClip())); this.actions.set(key, up); }
      a = up;
    }
    if (this.shot && this.shot !== a) this.shot.stop();
    a.reset();
    a.setLoop(THREE.LoopOnce, 1);
    a.clampWhenFinished = false;
    a.setEffectiveWeight(0).play();
    this.shot = a;
    return true;
  }

  /** How strongly the one-shot shows `t` seconds into a clip `d` long: in and out over the fade. */
  static envelope(t: number, d: number): number {
    return Math.max(0, Math.min(1, t / FADE_S, (d - t) / FADE_S));
  }

  update(dtMs: number): void {
    const dt = Math.max(0, dtMs) / 1000;
    let over = 0;
    if (this.shot) {
      if (!this.shot.isRunning()) this.shot = null;
      else {
        over = BipedAnimator.envelope(this.shot.time + dt, this.shot.getClip().duration);
        // The loop keeps its full weight, so bones the one-shot doesn't move
        // (a fire clip's legs) carry on; on the bones it does move it
        // outweighs the loop by as much as the envelope says.
        const share = 0.96 * over;
        // (An additive clip only adds its change: its weight is simply how much of it.)
        const additive = this.shot.getClip().blendMode === THREE.AdditiveAnimationBlendMode;
        this.shot.setEffectiveWeight(additive ? over : Math.max(share / (1 - share), 1e-3));
      }
    }
    for (const [name, w] of this.weights) {
      const want = name === this.loop ? 1 : 0;
      const next = want ? Math.min(1, w + dt / FADE_S) : Math.max(0, w - dt / FADE_S);
      const a = this.action(name)!;
      if (next === 0 && want === 0) { a.stop(); this.weights.delete(name); continue; }
      this.weights.set(name, next);
      a.setEffectiveWeight(next);
    }
    this.clearAim();
    this.mixer.update(dt);
    this.applyAim();
  }

  dispose(): void {
    this.mixer.stopAllAction();
    this.mixer.uncacheRoot(this.mixer.getRoot());
  }
}

/** Joints that carry the body over the ground: the legs and what they hang from. */
const LOWER_BODY = /(^|_)(root|pelvis|thigh|calf|shin|knee|foot|toe|leg)(_|$)/i;
const uppers = new WeakMap<THREE.AnimationClip, THREE.AnimationClip>();

/** A clip without its lower-body tracks, made once per clip. */
export function upperBody(clip: THREE.AnimationClip): THREE.AnimationClip {
  let out = uppers.get(clip);
  if (!out) {
    out = new THREE.AnimationClip(clip.name + "|upper", clip.duration, clip.tracks.filter((t) => !LOWER_BODY.test(t.name.slice(0, t.name.lastIndexOf(".")))), clip.blendMode);
    uppers.set(clip, out);
  }
  return out;
}

/** Seconds a clip takes to blend into the next. */
const FADE_S = 0.14;

export interface WeaponInstance {
  object: THREE.Object3D;
  /** The barrel's end; null when the glb doesn't mark one. */
  muzzle: THREE.Object3D | null;
}

interface Loaded {
  scene: THREE.Object3D;
  asset?: { extras?: unknown };
  animations?: THREE.AnimationClip[];
  /** The loader's parser, for textures a material names only in its extras. */
  parser?: { getDependency(type: "texture", index: number): Promise<THREE.Texture> };
}
export type LoadGlb = (url: string) => Promise<Loaded>;

const meshes = (root: THREE.Object3D): THREE.Mesh[] => {
  const out: THREE.Mesh[] = [];
  root.traverse((o) => { if ((o as THREE.Mesh).isMesh) out.push(o as THREE.Mesh); });
  return out;
};

// ---- surfaces ----

/** What the extractor says about a material (glTF `extras.h2`). */
interface H2Look { template?: string; change_colour?: number; blend?: string; env?: { perpendicular: Rgb; parallel: Rgb } }
const lookOf = (m: THREE.Material): H2Look => ((m.userData as { h2?: H2Look }).h2 ?? {});

/** How strongly the environment shows on a model: its own, brighter than the map's. */
export const MODEL_ENV_INTENSITY = 0.9;

/** Fetches the textures materials name only in their extras (the
 *  change-colour map), onto `userData.ccMap`. One that can't be read is
 *  left out: the vertex weights still colour the model. */
export async function resolveLooks(g: Loaded): Promise<Loaded> {
  const jobs: Promise<void>[] = [];
  g.scene.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh) return;
    for (const mat of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) {
      const idx = lookOf(mat).change_colour;
      if (typeof idx !== "number" || !g.parser || mat.userData.ccMap) continue;
      jobs.push(g.parser.getDependency("texture", idx).then((t) => {
        // Weights, not colour; the glb's own orientation.
        t.colorSpace = THREE.NoColorSpace;
        t.flipY = false;
        mat.userData.ccMap = t;
      }, () => {}));
    }
  });
  await Promise.all(jobs);
  return g;
}

/** Sets a material up as its shader's look says: additive glows add, see-
 *  through surfaces don't hide what's behind them, and everything takes the
 *  environment's reflections. */
export function applyLook(mat: THREE.Material, env: THREE.Texture | null): void {
  const look = lookOf(mat);
  const std = mat as THREE.MeshStandardMaterial;
  if (look.blend === "add") {
    mat.blending = THREE.AdditiveBlending;
    mat.transparent = true;
    mat.depthWrite = false;
  } else if (mat.transparent) {
    mat.depthWrite = false;
  }
  if (std.isMeshStandardMaterial && env) {
    std.envMap = env;
    std.envMapIntensity = MODEL_ENV_INTENSITY;
  }
  patchEnv(mat);
  mat.needsUpdate = true;
}

/** Materials whose shader already has the environment tint. */
const envPatched = new WeakSet<THREE.Material>();

/** Gives an environment-mapped look its tinted reflection (`ENV_FRAGMENT`), once. */
function patchEnv(mat: THREE.Material): void {
  const env = lookOf(mat).env;
  if (!env || envPatched.has(mat) || !(mat as THREE.MeshStandardMaterial).isMeshStandardMaterial) return;
  envPatched.add(mat);
  addShaderPatch(mat, "h2-env", (shader) => {
    shader.uniforms.envPerpendicular = { value: new THREE.Color(...env.perpendicular) };
    shader.uniforms.envParallel = { value: new THREE.Color(...env.parallel) };
    shader.fragmentShader = shader.fragmentShader
      .replace("#include <common>", "#include <common>\n" + ENV_UNIFORMS)
      .replace("#include <lights_fragment_end>", ENV_FRAGMENT + "\n#include <lights_fragment_end>");
  });
}

/** A copy of a material with its look's shader steps (a clone loses them). */
export function cloneMaterial(src: THREE.Material): THREE.Material {
  const out = src.clone();
  patchEnv(out);
  return out;
}

/** Adds a step to a material's shader compile, after any it has (a tinted
 *  copy keeps its environment tint, and the other way round). */
function addShaderPatch(mat: THREE.Material, key: string, patch: (shader: THREE.WebGLProgramParametersWithUniforms) => void): void {
  const before = shaderPatches.get(mat) ?? [];
  const patches = [...before, { key, patch }];
  shaderPatches.set(mat, patches);
  mat.onBeforeCompile = (shader) => { for (const p of patches) p.patch(shader); };
  // Programs are shared by what the patches are.
  mat.customProgramCacheKey = () => patches.map((p) => p.key).join("|");
}
const shaderPatches = new WeakMap<THREE.Material, { key: string; patch: (shader: THREE.WebGLProgramParametersWithUniforms) => void }[]>();

/** How strongly an environment-mapped shader's tinted reflection shows. */
export const ENV_STRENGTH = 0.5;
export const ENV_UNIFORMS = "uniform vec3 envPerpendicular; uniform vec3 envParallel;";
/** Halo 2's environment-mapped shaders (the Covenant's sheen): the
 *  reflection, where the specular mask (red of the metallic-roughness
 *  texture) allows it, tinted from the perpendicular colour facing the eye
 *  to the parallel one at a grazing angle. Added to what is lit. */
export const ENV_FRAGMENT = `#if defined( USE_ENVMAP ) && defined( USE_METALNESSMAP )
{
  float envMask = texture2D(metalnessMap, vMetalnessMapUv).r;
  float envFacing = saturate(dot(geometryNormal, geometryViewDir));
  vec3 envTint = mix(envParallel, envPerpendicular, envFacing);
  // The game's cube maps are low range: a bright environment is clamped.
  reflectedLight.indirectSpecular += min(getIBLRadiance(geometryViewDir, geometryNormal, 0.2), vec3(1.0)) * envTint * envMask * ${ENV_STRENGTH.toFixed(2)};
}
#endif`;

/** A copy of `src` whose base colour is tinted per pixel by its change-colour
 *  map: red weighs the primary colour, green the secondary (linear light,
 *  the same sharing out as `tintAt`). */
export function changeColourMaterial(src: THREE.Material, cc: THREE.Texture, primary: Rgb, secondary: Rgb = secondaryOf(primary)): THREE.Material {
  const out = cloneMaterial(src);
  out.userData = { ...src.userData, cc: { primary, secondary } };
  (out as THREE.MeshStandardMaterial).vertexColors = false;
  addShaderPatch(out, "h2-change-colour", (shader) => {
    shader.uniforms.ccMap = { value: cc };
    shader.uniforms.ccPrimary = { value: new THREE.Color(...primary) };
    shader.uniforms.ccSecondary = { value: new THREE.Color(...secondary) };
    shader.fragmentShader = shader.fragmentShader
      .replace("#include <common>", "#include <common>\n" + CC_UNIFORMS)
      .replace("#include <map_fragment>", "#include <map_fragment>\n" + CC_FRAGMENT);
  });
  return out;
}

export const CC_UNIFORMS = "uniform sampler2D ccMap; uniform vec3 ccPrimary; uniform vec3 ccSecondary;";
/** Needs the base map's uv (`vMapUv`): only materials with a base map get a per-pixel tint. */
export const CC_FRAGMENT = `{
  vec4 ccw = texture2D(ccMap, vMapUv);
  float ccp = clamp(ccw.r, 0.0, 1.0), ccs = clamp(ccw.g, 0.0, 1.0);
  float ccn = ccp + ccs;
  if (ccn > 1.0) { ccp /= ccn; ccs /= ccn; }
  diffuseColor.rgb *= vec3(1.0 - ccp - ccs) + ccPrimary * ccp + ccSecondary * ccs;
}`;

/** The change-colour map of a material that can be tinted per pixel. */
const ccMapOf = (m: THREE.Material): THREE.Texture | null =>
  ((m as THREE.MeshStandardMaterial).map && (m.userData.ccMap as THREE.Texture | undefined)) || null;

/** Makes a loaded biped ready to clone: finds (or, for a glb that lacks
 *  them, places) the `head` and `weapon` nodes and measures its height. */
export function prepareBiped(g: Loaded): BipedTemplate | null {
  const root = g.scene;
  const all = meshes(root);
  if (!all.length) return null;
  root.updateWorldMatrix(true, true);
  const box = new THREE.Box3().setFromObject(root);
  if (box.isEmpty()) return null;
  const extras = (g.asset?.extras ?? {}) as { height?: unknown };
  const height = typeof extras.height === "number" && extras.height > 0 ? extras.height : Math.max(0.01, box.max.y);
  if (!findNode(root, "head")) {
    const head = new THREE.Object3D();
    head.name = "head";
    head.position.set(0, height * 0.91, 0);
    root.add(head);
  }
  if (!findNode(root, "weapon")) {
    // About where a right hand holds a rifle: ahead, chest high, to the right.
    const hand = new THREE.Object3D();
    hand.name = "weapon";
    hand.position.set(height * 0.22, height * 0.66, height * 0.1);
    root.add(hand);
  }
  let regions = false;
  for (const m of all) {
    m.castShadow = true;
    m.receiveShadow = true;
    if (m.geometry.getAttribute("color")) regions = true;
  }
  // Version 2: skinned, with named clips. The bind pose is the old static one.
  const clips = new Map<string, THREE.AnimationClip>();
  for (const c of g.animations ?? []) {
    if (!c.name || !(c.duration > 0)) continue;
    // A fire clip is an overlay: what it changes from its first frame is
    // added to whatever else is playing, rather than replacing it.
    clips.set(c.name, c.name.endsWith(":fire") ? THREE.AnimationUtils.makeClipAdditive(c.clone()) : c);
  }
  let skinned = false;
  for (const m of all) if ((m as THREE.SkinnedMesh).isSkinnedMesh) { skinned = true; m.frustumCulled = false; }
  return { kind: "biped", root, height, regions, clips, skinned };
}

export function prepareWeapon(g: Loaded): WeaponTemplate | null {
  const all = meshes(g.scene);
  if (!all.length) return null;
  for (const m of all) m.castShadow = true;
  return { kind: "weapon", root: g.scene };
}

export function prepareVehicle(g: Loaded): VehicleTemplate | null {
  const all = meshes(g.scene);
  if (!all.length) return null;
  for (const m of all) {
    m.castShadow = true;
    m.receiveShadow = true;
    // COLOR_0 holds change-colour weights, not a colour: the loader would
    // multiply the vehicle by them (a banshee green and red, its weightless
    // parts black). Its tint comes from the change-colour map instead.
    if (m.geometry.getAttribute("color")) {
      for (const mat of Array.isArray(m.material) ? m.material : [m.material]) (mat as THREE.MeshStandardMaterial).vertexColors = false;
    }
  }
  g.scene.updateWorldMatrix(true, true);
  const box = new THREE.Box3().setFromObject(g.scene);
  return box.isEmpty() ? null : { kind: "vehicle", root: g.scene, height: Math.max(0.1, box.max.y) };
}

type Entry<T> = { state: "loading" } | { state: "ready"; template: T } | { state: "failed" };

/** The glbs loaded so far, by address, and the coloured parts made from
 *  them. `onReady` is called when one arrives, so the scene can draw again. */
export class ModelLibrary {
  private bipeds = new Map<string, Entry<BipedTemplate>>();
  private weapons = new Map<string, Entry<WeaponTemplate>>();
  private vehicles = new Map<string, Entry<VehicleTemplate>>();
  /** Coloured geometry and materials, by colour then by what they were made from. */
  private tinted = new Map<string, { geo: Map<THREE.BufferGeometry, THREE.BufferGeometry>; mat: Map<THREE.Material, THREE.Material> }>();
  private disposed = false;
  /** The reflections models take (the scene's environment), once given. */
  private env: THREE.Texture | null = null;

  constructor(private onReady: () => void = () => {}, private load: LoadGlb = (url) => new GLTFLoader().loadAsync(url)) {}

  /** Gives every model, loaded and to come, the scene's environment to reflect. */
  setEnvironment(env: THREE.Texture | null): void {
    this.env = env;
    for (const map of [this.bipeds, this.weapons, this.vehicles] as Map<string, Entry<{ root: THREE.Object3D }>>[]) {
      for (const e of map.values()) if (e.state === "ready") this.dressSurfaces(e.template.root);
    }
    for (const set of this.tinted.values()) for (const m of set.mat.values()) applyLook(m, env);
  }

  private dressSurfaces(root: THREE.Object3D): void {
    for (const m of meshes(root)) {
      for (const mat of Array.isArray(m.material) ? m.material : [m.material]) {
        applyLook(mat, this.env);
        // Glows and glass cast no shadow.
        if (mat.transparent) m.castShadow = false;
      }
    }
  }

  private get<T extends { root: THREE.Object3D }>(map: Map<string, Entry<T>>, url: string, prepare: (g: Loaded) => T | null): T | null {
    const e = map.get(url);
    if (e) return e.state === "ready" ? e.template : null;
    map.set(url, { state: "loading" });
    this.load(url).then(resolveLooks).then(prepare, () => null).then((template) => {
      if (this.disposed) return;
      if (template) this.dressSurfaces(template.root);
      map.set(url, template ? { state: "ready", template } : { state: "failed" });
      if (template) this.onReady();
    });
    return null;
  }

  /** The biped at `url` once it has loaded; null until then, and for good
   *  when it can't be read (the caller keeps the capsule). Asking starts
   *  the one fetch. */
  biped(url: string): BipedTemplate | null { return this.get(this.bipeds, url, prepareBiped); }
  weapon(url: string): WeaponTemplate | null { return this.get(this.weapons, url, prepareWeapon); }
  vehicle(url: string): VehicleTemplate | null { return this.get(this.vehicles, url, prepareVehicle); }

  /** Whether `url` was tried and can't be used. */
  failed(url: string): boolean {
    return [this.bipeds, this.weapons, this.vehicles].some((m) => m.get(url)?.state === "failed");
  }

  /** A copy of a vehicle in its own colours, or darkened when nobody is in
   *  it. Halo 2 does not repaint a vehicle for its driver's team: its
   *  colours are the object's own, which the extractor paints into its base
   *  maps (the Banshee's violet). `colour` (the driver's) is not used for
   *  the paint. */
  instantiateVehicle(t: VehicleTemplate, _colour: string | null, empty = false): THREE.Object3D {
    const object = t.root.clone(true);
    if (!empty) return object;
    let set = this.tinted.get("vehicle|empty");
    if (!set) { set = { geo: new Map(), mat: new Map() }; this.tinted.set("vehicle|empty", set); }
    const k = EMPTY_VEHICLE_SHADE, mats = set.mat;
    for (const m of meshes(object)) {
      m.material = mapMaterials(m.material, (mat) => {
        let out = mats.get(mat);
        if (!out) {
          out = cloneMaterial(mat);
          const col = (out as THREE.MeshStandardMaterial).color;
          if (col) col.setRGB(col.r * k, col.g * k, col.b * k);
          mats.set(mat, out);
        }
        return out;
      });
    }
    return object;
  }

  /** A copy of a biped in a player's colour, or as it comes (null: an AI,
   *  which wears its own colours). It shares the template's geometry and
   *  textures; nothing in it needs disposing. */
  instantiateBiped(t: BipedTemplate, colour: string | null): BipedInstance {
    // A skinned model's copy needs its own bones; a static one just its own nodes.
    const object = t.skinned ? cloneSkinned(t.root) : t.root.clone(true);
    const animator = t.clips.size ? new BipedAnimator(object, t.clips) : null;
    if (colour === null) {
      // The loader multiplies by COLOR_0 as if it were a colour; here it holds weights.
      let plain = this.tinted.get("");
      if (!plain) { plain = { geo: new Map(), mat: new Map() }; this.tinted.set("", plain); }
      const mats = plain.mat;
      for (const m of meshes(object)) {
        if (!m.geometry.getAttribute("color")) continue;
        m.material = mapMaterials(m.material, (mat) => {
          let out = mats.get(mat);
          if (!out) { out = cloneMaterial(mat); (out as THREE.MeshStandardMaterial).vertexColors = false; mats.set(mat, out); }
          return out;
        });
      }
      return { object, head: findNode(object, "head")!, hand: findNode(object, "weapon")!, height: t.height, animator };
    }
    let set = this.tinted.get(colour);
    if (!set) { set = { geo: new Map(), mat: new Map() }; this.tinted.set(colour, set); }
    const c = new THREE.Color(colour);
    const primary: Rgb = [c.r, c.g, c.b];
    for (const m of meshes(object)) {
      const weights = m.geometry.getAttribute("color");
      const single = Array.isArray(m.material) ? null : m.material;
      const cc = single ? ccMapOf(single) : null;
      if (single && cc) {
        // Per pixel, from the shader's own change-colour map.
        let out = set.mat.get(single);
        if (!out) { out = changeColourMaterial(single, cc, primary); set.mat.set(single, out); }
        m.material = out;
      } else if (weights) {
        let geo = set.geo.get(m.geometry);
        if (!geo) { geo = tintGeometry(m.geometry, primary); set.geo.set(m.geometry, geo); }
        m.geometry = geo;
        m.material = mapMaterials(m.material, (mat) => {
          let out = set.mat.get(mat);
          if (!out) {
            // One shared copy: the weights are in the geometry, not the material.
            out = cloneMaterial(mat);
            (out as THREE.MeshStandardMaterial).vertexColors = true;
            set.mat.set(mat, out);
          }
          return out;
        });
      } else if (!t.regions) {
        const tint = bodyTint(primary);
        m.material = mapMaterials(m.material, (mat) => {
          let out = set.mat.get(mat);
          if (!out) {
            out = cloneMaterial(mat);
            const col = (out as THREE.MeshStandardMaterial).color;
            if (col) col.setRGB(col.r * tint[0], col.g * tint[1], col.b * tint[2]);
            set.mat.set(mat, out);
          }
          return out;
        });
      }
    }
    return { object, head: findNode(object, "head")!, hand: findNode(object, "weapon")!, height: t.height, animator };
  }

  instantiateWeapon(t: WeaponTemplate): WeaponInstance {
    const object = t.root.clone(true);
    return { object, muzzle: findNode(object, "muzzle") };
  }

  dispose(): void {
    this.disposed = true;
    for (const set of this.tinted.values()) {
      for (const g of set.geo.values()) g.dispose();
      for (const m of set.mat.values()) m.dispose();
    }
    this.tinted.clear();
    for (const map of [this.bipeds, this.weapons, this.vehicles] as Map<string, Entry<{ root: THREE.Object3D }>>[]) {
      for (const e of map.values()) {
        if (e.state !== "ready") continue;
        for (const m of meshes(e.template.root)) {
          m.geometry.dispose();
          for (const mat of Array.isArray(m.material) ? m.material : [m.material]) {
            (mat as THREE.MeshStandardMaterial).map?.dispose();
            mat.dispose();
          }
        }
      }
      map.clear();
    }
  }
}

function mapMaterials(m: THREE.Material | THREE.Material[], f: (m: THREE.Material) => THREE.Material): THREE.Material | THREE.Material[] {
  return Array.isArray(m) ? m.map(f) : f(m);
}

/** The same mesh with its change-colour weights turned into the colour
 *  each vertex's texture is multiplied by. Shares every other attribute. */
export function tintGeometry(src: THREE.BufferGeometry, primary: Rgb): THREE.BufferGeometry {
  const w = src.getAttribute("color");
  const out = new THREE.BufferGeometry();
  for (const [name, attr] of Object.entries(src.attributes)) if (name !== "color") out.setAttribute(name, attr);
  out.setIndex(src.index);
  for (const g of src.groups) out.addGroup(g.start, g.count, g.materialIndex);
  out.boundingBox = src.boundingBox;
  out.boundingSphere = src.boundingSphere;
  // getX/getY undo a normalised byte encoding, so both forms the contract allows read 0..1.
  out.setAttribute("color", new THREE.BufferAttribute(tintColours(w.count, (i) => [w.getX(i), w.getY(i)], primary, secondaryOf(primary)), 3));
  return out;
}
