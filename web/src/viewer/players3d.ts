// Player and weapon models for the 3D view: each glb is fetched once and
// kept as a template, and every player gets a clone of it that shares the
// template's geometry and textures. A biped is coloured by multiplying its
// texture by a per-vertex colour worked out from the glb's change-colour
// weights (viewer/models.ts); that colour set is made once per player
// colour, so team mates share it. Nothing here animates: the model is the
// pose it was exported in.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { bodyTint, findNode, secondaryOf, tintColours, vehicleTint, type Rgb } from "./models";

export interface BipedTemplate {
  kind: "biped";
  root: THREE.Object3D;
  /** Feet to the top of the head, game units. */
  height: number;
  /** Whether the glb marks what takes the player's colour. */
  regions: boolean;
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
}

export interface WeaponInstance {
  object: THREE.Object3D;
  /** The barrel's end; null when the glb doesn't mark one. */
  muzzle: THREE.Object3D | null;
}

interface Loaded { scene: THREE.Object3D; asset?: { extras?: unknown } }
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
  return { kind: "biped", root, height, regions };
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
   *  for none: an AI's, or nobody's). */
  instantiateVehicle(t: VehicleTemplate, colour: string | null): THREE.Object3D {
    const object = t.root.clone(true);
    if (!colour) return object;
    const key = "vehicle|" + colour;
    let set = this.tinted.get(key);
    if (!set) { set = { geo: new Map(), mat: new Map() }; this.tinted.set(key, set); }
    const c = new THREE.Color(colour), tint = vehicleTint([c.r, c.g, c.b]), mats = set.mat;
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
    const object = t.root.clone(true);
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
      return { object, head: findNode(object, "head")!, hand: findNode(object, "weapon")!, height: t.height };
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
    return { object, head: findNode(object, "head")!, hand: findNode(object, "weapon")!, height: t.height };
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
