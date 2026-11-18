// The objects a scenario places (scenery, crates, weapons and their
// multiplayer spawns, vehicles, controls, lights), from the map glb's
// `scenario` node: one child per placement, its extras naming it
// (`{kind, index, tag, pos, forward, up, scale, bsp?, item?}`,
// docs/live/live-art.md, "Scenario objects"). Placements of the same model are
// drawn as one instanced batch per kind; the placement nodes stay in the
// tree, hidden, so each can be handed out and moved on its own.
//
// For the frames' moving objects (`Frame.objects`, `{k, i, p, f, u, gone?}`):
// `find(kind, index)` returns a placement's node, taken out of its batch
// and shown, for the caller to move; `apply(list)` does that from frame
// entries; `restAll()` puts every object back where the scenario put it.
//
// `objectOf`, `objectKinds` and `KIND_LABELS` are pure; the class needs
// three.js, so this file is only ever loaded lazily.

import * as THREE from "three";
import { gameToGl } from "../map/views";
import { vehicleBasis } from "./models";
import type { TextureSwap } from "./textures3d";

type V3 = [number, number, number];

/** What a placement node's extras say, read defensively. */
export interface ObjectInfo { kind: string; index: number; tag: string; bsp: number | null; item: string | null }

export function objectOf(extras: unknown): ObjectInfo | null {
  const e = (extras ?? {}) as Record<string, unknown>;
  if (typeof e.kind !== "string" || !/^[a-z]{1,16}$/.test(e.kind)) return null;
  if (typeof e.index !== "number" || !Number.isInteger(e.index) || e.index < 0) return null;
  const bsp = typeof e.bsp === "number" && Number.isInteger(e.bsp) && e.bsp >= 0 ? e.bsp : null;
  return {
    kind: e.kind,
    index: e.index,
    tag: typeof e.tag === "string" ? e.tag : "",
    bsp,
    item: typeof e.item === "string" ? e.item : null,
  };
}

/** The Show menu's names for the kinds. */
export const KIND_LABELS: Record<string, string> = {
  scen: "Scenery",
  bloc: "Crates",
  netgame: "Weapon spawns",
  weap: "Weapons",
  eqip: "Equipment",
  vehi: "Vehicles",
  ctrl: "Controls",
  lifi: "Light fixtures",
  ssce: "Sound scenery",
};

/** The kinds present, in the menu's order (unknown kinds last, by name). */
export function objectKinds(kinds: Iterable<string>): string[] {
  const order = Object.keys(KIND_LABELS);
  const rank = (k: string) => { const i = order.indexOf(k); return i < 0 ? order.length : i; };
  return [...new Set(kinds)].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b));
}

/** A frame's moving object: kind, index, position, forward and up (game
 *  coordinates), or gone. */
export interface ObjectPose { k: string; i: number; p?: V3; f?: V3; u?: V3; gone?: boolean }

interface Placed {
  info: ObjectInfo;
  node: THREE.Object3D;
  /** Its batch and slot, while it is drawn there. */
  batch: Batch | null;
  slot: number;
  /** The node's own matrix as the glb placed it. */
  rest: THREE.Matrix4;
  /** Taken out of its batch: the caller moves it. */
  loose: boolean;
  gone: boolean;
}

interface Batch { kind: string; bsp: number | null; vehicle: boolean; meshes: THREE.InstancedMesh[]; slots: Placed[] }

/** A placement of a vehicle: a `vehi` one, or a multiplayer vehicle spawn. */
export const isVehicle = (info: ObjectInfo) => info.kind === "vehi" || (info.kind === "netgame" && info.item === "vehi");

const ZERO = new THREE.Matrix4().makeScale(0, 0, 0);
const tmp = new THREE.Matrix4();
const vx = new THREE.Vector3(), vy = new THREE.Vector3(), vz = new THREE.Vector3();
const pos = new THREE.Vector3(), quat = new THREE.Quaternion(), scl = new THREE.Vector3();

const key = (kind: string, index: number) => `${kind}:${index}`;

export class ScenarioObjects {
  private root: THREE.Object3D | null = null;
  private placed = new Map<string, Placed>();
  private batches: Batch[] = [];
  private zones = new Map<number, THREE.Object3D>();
  private on = true;
  private hiddenKinds = new Set<string>();
  /** The frames draw the vehicles: the level's own are hidden. */
  private vehiclesHidden = false;

  constructor(private changed: () => void = () => {}) {}

  /** Takes a loaded level's `scenario` node (after its materials are
   *  final): batches each model's placements and hides their nodes. */
  adopt(level: THREE.Object3D, textures?: TextureSwap) {
    this.clear();
    level.traverse((o) => {
      const m = /^bsp_?(\d+)$/i.exec(o.name);
      if (m) this.zones.set(Number(m[1]), o);
    });
    const root = level.getObjectByName("scenario");
    if (!root) return;
    this.root = root;
    root.updateWorldMatrix(true, true);
    const toRoot = root.matrixWorld.clone().invert();
    // Placements by (kind, zone, model): the model is its first mesh's geometry.
    const groups = new Map<string, Placed[]>();
    for (const node of [...root.children]) {
      const info = objectOf(node.userData);
      if (!info) continue;
      const meshes = meshesOf(node);
      if (!meshes.length) continue;
      const p: Placed = { info, node, batch: null, slot: -1, rest: node.matrix.clone(), loose: false, gone: false };
      this.placed.set(key(info.kind, info.index), p);
      const g = `${info.kind}|${info.item ?? ""}|${info.bsp ?? ""}|${meshes[0]!.geometry.uuid}`;
      let list = groups.get(g);
      if (!list) groups.set(g, (list = []));
      list.push(p);
    }
    for (const list of groups.values()) {
      const first = list[0]!;
      const batch: Batch = { kind: first.info.kind, bsp: first.info.bsp, vehicle: isVehicle(first.info), meshes: [], slots: list };
      for (const src of meshesOf(first.node)) {
        const im = new THREE.InstancedMesh(src.geometry, src.material, list.length);
        im.name = `${first.node.name}_batch`;
        im.castShadow = src.castShadow;
        im.receiveShadow = src.receiveShadow;
        im.renderOrder = src.renderOrder;
        // Each placement's matrix (and its primitive's, under it) in the
        // scenario node's space.
        list.forEach((p, slot) => {
          const own = meshesOf(p.node).find((m) => m.geometry === src.geometry) ?? src;
          im.setMatrixAt(slot, tmp.copy(toRoot).multiply(own.matrixWorld));
        });
        im.instanceMatrix.needsUpdate = true;
        im.computeBoundingSphere();
        im.userData.batchOf = first.info.kind;
        const pair = textures?.pairOf(src);
        if (pair) textures!.add(im, pair.textured, pair.flat);
        root.add(im);
        batch.meshes.push(im);
      }
      list.forEach((p, slot) => { p.batch = batch; p.slot = slot; p.node.visible = false; });
      this.batches.push(batch);
    }
    this.refresh();
  }

  clear() {
    for (const b of this.batches) for (const im of b.meshes) { im.removeFromParent(); im.dispose(); }
    this.batches = [];
    this.placed.clear();
    this.zones.clear();
    this.root = null;
  }

  /** The kinds the level has, in the menu's order. */
  kinds(): string[] { return objectKinds([...this.placed.values()].map((p) => p.info.kind)); }
  has(): boolean { return this.placed.size > 0; }

  /** Every placement where the level puts it, game coordinates, keyed
   *  `<kind>:<index>`, with the item a multiplayer spawn holds. */
  spots(): Map<string, { kind: string; item: string | null; at: V3 }> {
    const out = new Map<string, { kind: string; item: string | null; at: V3 }>();
    const v = new THREE.Vector3();
    for (const [k, p] of this.placed) {
      p.node.getWorldPosition(v);
      // gl (x, y, z) is game (x, -z, y).
      out.set(k, { kind: p.info.kind, item: p.info.item, at: [v.x, -v.z, v.y] });
    }
    return out;
  }

  /** All objects on or off. */
  setShown(on: boolean) { this.on = on; this.refresh(); }
  /** One kind on or off. */
  setKind(kind: string, on: boolean) {
    if (on) this.hiddenKinds.delete(kind); else this.hiddenKinds.add(kind);
    this.refresh();
  }

  /** The level's vehicles hidden (the frames draw every vehicle) or shown. */
  setVehiclesHidden(hidden: boolean) {
    if (hidden === this.vehiclesHidden) return;
    this.vehiclesHidden = hidden;
    this.refresh();
  }

  /** A placement's node, taken out of its batch and shown (unless the
   *  objects or its kind are hidden): from now on whoever asked moves it
   *  (`position`, `quaternion`; its `scale` is the placement's) and may
   *  hide it. Undefined when the level has no such placement. */
  find(kind: string, index: number): THREE.Object3D | undefined {
    const p = this.placed.get(key(kind, index));
    if (!p) return undefined;
    if (!p.loose) {
      p.loose = true;
      this.setSlot(p, ZERO);
      p.node.visible = this.shown(p);
    }
    return p.node;
  }

  /** Moves objects to where frame entries say (game coordinates), and
   *  hides the ones gone. Unknown placements are ignored. Returns whether
   *  anything changed. */
  apply(list: readonly ObjectPose[]): boolean {
    let moved = false;
    for (const e of list) {
      const node = this.find(e.k, e.i);
      if (!node) continue;
      const p = this.placed.get(key(e.k, e.i))!;
      if (e.gone) {
        if (!p.gone) { p.gone = true; node.visible = false; moved = true; }
        continue;
      }
      if (p.gone) { p.gone = false; node.visible = this.shown(p); moved = true; }
      if (!e.p) continue;
      const b = vehicleBasis(e.f ?? [1, 0, 0], e.u ?? [0, 0, 1]);
      p.rest.decompose(pos, quat, scl);
      tmp.makeBasis(vx.set(...b.x), vy.set(...b.y), vz.set(...b.z)).setPosition(...gameToGl(e.p[0], e.p[1], e.p[2]));
      tmp.decompose(pos, quat, vx);
      if (!node.position.equals(pos) || !node.quaternion.equals(quat)) {
        node.position.copy(pos);
        node.quaternion.copy(quat);
        node.scale.copy(scl);
        moved = true;
      }
    }
    if (moved) this.changed();
    return moved;
  }

  /** Every object back where the scenario placed it, in its batch. */
  restAll() {
    for (const p of this.placed.values()) {
      if (!p.loose) continue;
      p.loose = false;
      p.gone = false;
      p.rest.decompose(p.node.position, p.node.quaternion, p.node.scale);
      p.node.visible = false;
      this.root?.updateWorldMatrix(true, false);
      p.node.updateMatrixWorld(true);
      const own = meshesOf(p.node)[0];
      if (own && this.root) this.setSlot(p, tmp.copy(this.root.matrixWorld).invert().multiply(own.matrixWorld));
    }
    this.changed();
  }

  /** Before each draw: follows which zones are shown (objects belong to
   *  the zone their placement names). Cheap: a few dozen batches. */
  beforeDraw() { this.refresh(false); }

  private zoneShown(bsp: number | null): boolean {
    if (bsp === null) return true;
    const z = this.zones.get(bsp);
    return z ? z.visible : true;
  }

  private shown(p: Placed): boolean {
    return this.on && !this.hiddenKinds.has(p.info.kind) && !p.gone && !(this.vehiclesHidden && isVehicle(p.info)) && this.zoneShown(p.info.bsp);
  }

  private refresh(notify = true) {
    let changed = false;
    for (const b of this.batches) {
      const v = this.on && !this.hiddenKinds.has(b.kind) && !(this.vehiclesHidden && b.vehicle) && this.zoneShown(b.bsp);
      for (const im of b.meshes) if (im.visible !== v) { im.visible = v; changed = true; }
    }
    for (const p of this.placed.values()) {
      if (!p.loose) continue;
      const v = this.shown(p);
      if (p.node.visible !== v) { p.node.visible = v; changed = true; }
    }
    if (changed || notify) this.changed();
  }

  private setSlot(p: Placed, m: THREE.Matrix4) {
    if (!p.batch) return;
    for (const im of p.batch.meshes) {
      im.setMatrixAt(p.slot, m);
      im.instanceMatrix.needsUpdate = true;
    }
  }
}

/** The meshes drawn for a placement node: itself, or its primitives. */
function meshesOf(node: THREE.Object3D): THREE.Mesh[] {
  const out: THREE.Mesh[] = [];
  node.traverse((o) => { if ((o as THREE.Mesh).isMesh && !(o as THREE.InstancedMesh).isInstancedMesh) out.push(o as THREE.Mesh); });
  return out;
}
