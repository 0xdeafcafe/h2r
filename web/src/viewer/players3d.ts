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
import { EMPTY_VEHICLE_SHADE, bodyTint, findNode, secondaryOf, tintColours, vehicleTint, type Rgb } from "./models";

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

  constructor(root: THREE.Object3D, private clips: ReadonlyMap<string, THREE.AnimationClip>) {
    this.mixer = new THREE.AnimationMixer(root);
    this.names = new Set(clips.keys());
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
    this.mixer.update(dt);
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

interface Loaded { scene: THREE.Object3D; asset?: { extras?: unknown }; animations?: THREE.AnimationClip[] }
export type LoadGlb = (url: string) => Promise<Loaded>;

const meshes = (root: THREE.Object3D): THREE.Mesh[] => {
  const out: THREE.Mesh[] = [];
  root.traverse((o) => { if ((o as THREE.Mesh).isMesh) out.push(o as THREE.Mesh); });
  return out;
};

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
  for (const m of all) { m.castShadow = true; m.receiveShadow = true; }
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

  constructor(private onReady: () => void = () => {}, private load: LoadGlb = (url) => new GLTFLoader().loadAsync(url)) {}

  private get<T>(map: Map<string, Entry<T>>, url: string, prepare: (g: Loaded) => T | null): T | null {
    const e = map.get(url);
    if (e) return e.state === "ready" ? e.template : null;
    map.set(url, { state: "loading" });
    this.load(url).then(prepare, () => null).then((template) => {
      if (this.disposed) return;
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

  /** A copy of a vehicle, washed lightly with its player's colour (null
   *  for none: an AI's, or nobody's), or darkened when nobody is in it. */
  instantiateVehicle(t: VehicleTemplate, colour: string | null, empty = false): THREE.Object3D {
    const object = t.root.clone(true);
    if (!colour && !empty) return object;
    const key = empty ? "vehicle|empty" : "vehicle|" + colour;
    let set = this.tinted.get(key);
    if (!set) { set = { geo: new Map(), mat: new Map() }; this.tinted.set(key, set); }
    const k = EMPTY_VEHICLE_SHADE, c = new THREE.Color(colour ?? "#fff"), tint: Rgb = empty ? [k, k, k] : vehicleTint([c.r, c.g, c.b]), mats = set.mat;
    for (const m of meshes(object)) {
      m.material = mapMaterials(m.material, (mat) => {
        let out = mats.get(mat);
        if (!out) {
          out = mat.clone();
          const col = (out as THREE.MeshStandardMaterial).color;
          if (col) col.setRGB(col.r * tint[0], col.g * tint[1], col.b * tint[2]);
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
          if (!out) { out = mat.clone(); (out as THREE.MeshStandardMaterial).vertexColors = false; mats.set(mat, out); }
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
      if (weights) {
        let geo = set.geo.get(m.geometry);
        if (!geo) { geo = tintGeometry(m.geometry, primary); set.geo.set(m.geometry, geo); }
        m.geometry = geo;
        m.material = mapMaterials(m.material, (mat) => {
          let out = set.mat.get(mat);
          if (!out) {
            // One shared copy: the weights are in the geometry, not the material.
            out = mat.clone();
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
            out = mat.clone();
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
