import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { KIND_LABELS, ScenarioObjects, objectKinds, objectOf } from "../src/viewer/scenario3d";
import { TEXTURES_KEY, TextureSwap, flatOf, loadFlats, savePref, texturesPref } from "../src/viewer/textures3d";
import { meshRole } from "../src/viewer/cutaway";

class MemoryStore {
  map = new Map<string, string>();
  getItem(k: string) { return this.map.get(k) ?? null; }
  setItem(k: string, v: string) { this.map.set(k, v); }
}

/** A level as the loader gives it: two zones and a `scenario` node whose
 *  placements share models (one mesh each, or two primitives in a group). */
function level() {
  const root = new THREE.Group();
  for (const n of [0, 1]) { const z = new THREE.Group(); z.name = `bsp_${n}`; root.add(z); }
  const scenario = new THREE.Group();
  scenario.name = "scenario";
  root.add(scenario);
  const crate = new THREE.BoxGeometry(1, 1, 1);
  const metal = new THREE.MeshStandardMaterial({ name: "crate_pic" });
  const place = (name: string, extras: Record<string, unknown>, x: number) => {
    const m = new THREE.Mesh(crate, metal);
    m.name = name;
    m.userData = extras;
    m.position.set(x, 0, 0);
    scenario.add(m);
    return m;
  };
  const a = place("bloc_0", { kind: "bloc", index: 0, tag: "crate", bsp: 0 }, 1);
  const b = place("bloc_1", { kind: "bloc", index: 1, tag: "crate", bsp: 0 }, 2);
  const c = place("bloc_2", { kind: "bloc", index: 2, tag: "crate", bsp: 1 }, 3);
  // A weapon spawn: a group of two primitives.
  const rifle = new THREE.Group();
  rifle.name = "netgame_4";
  rifle.userData = { kind: "netgame", index: 4, tag: "battle_rifle", item: "weap" };
  rifle.position.set(0, 0, -5);
  const g1 = new THREE.BufferGeometry(), g2 = new THREE.BufferGeometry();
  rifle.add(new THREE.Mesh(g1, metal), new THREE.Mesh(g2, metal));
  scenario.add(rifle);
  // Not a placement: ignored.
  const stray = new THREE.Mesh(crate, metal);
  stray.name = "junk";
  scenario.add(stray);
  root.updateMatrixWorld(true);
  return { root, scenario, a, b, c, rifle, metal };
}

const batches = (s: THREE.Object3D) => s.children.filter((o) => (o as THREE.InstancedMesh).isInstancedMesh) as THREE.InstancedMesh[];
const slot = (im: THREE.InstancedMesh, i: number) => { const m = new THREE.Matrix4(); im.getMatrixAt(i, m); return new THREE.Vector3().setFromMatrixPosition(m); };

describe("scenario objects", () => {
  test("a placement's extras are read to the contract", () => {
    expect(objectOf({ kind: "weap", index: 3, tag: "smg", bsp: 2, pos: [0, 0, 0] })).toEqual({ kind: "weap", index: 3, tag: "smg", bsp: 2, item: null });
    expect(objectOf({ kind: "netgame", index: 0, tag: "ghost", item: "vehi" })).toEqual({ kind: "netgame", index: 0, tag: "ghost", bsp: null, item: "vehi" });
    expect(objectOf({ kind: "weap", index: -1 })).toBeNull();
    expect(objectOf({ kind: "weap", index: 1.5 })).toBeNull();
    expect(objectOf({ kind: "../x", index: 1 })).toBeNull();
    expect(objectOf({ index: 1 })).toBeNull();
    expect(objectOf(null)).toBeNull();
    expect(objectOf({ kind: "scen", index: 0, bsp: -1 })!.bsp).toBeNull();
  });

  test("kinds are listed in the menu's order, unknown ones last", () => {
    expect(objectKinds(["weap", "zzzz", "scen", "netgame", "bloc", "scen"])).toEqual(["scen", "bloc", "netgame", "weap", "zzzz"]);
    expect(KIND_LABELS.netgame).toBe("Weapon spawns");
  });

  test("placed objects are drawn like machines: whole, cut by height only", () => {
    expect(meshRole(["root", "scenario", "bloc_0"])).toBe("machine");
    expect(meshRole(["root", "structure"])).toBe("level");
  });

  test("placements of one model are one instanced batch per kind and zone; their nodes hide", () => {
    const { root, scenario, a, b, c, rifle } = level();
    const objects = new ScenarioObjects();
    objects.adopt(root);
    expect(objects.has()).toBe(true);
    expect(objects.kinds()).toEqual(["bloc", "netgame"]);
    const ims = batches(scenario);
    // bloc zone 0 (two), bloc zone 1 (one), the rifle's two primitives.
    expect(ims.map((m) => m.count).sort()).toEqual([1, 1, 1, 2]);
    expect([a, b, c, rifle].every((n) => !n.visible)).toBe(true);
    const pair = ims.find((m) => m.count === 2)!;
    expect(slot(pair, 0).x).toBe(1);
    expect(slot(pair, 1).x).toBe(2);
    const rifles = ims.filter((m) => m.geometry !== a.geometry);
    expect(rifles.every((m) => slot(m, 0).z === -5)).toBe(true);
    objects.clear();
    expect(batches(scenario)).toHaveLength(0);
  });

  test("kinds, the whole layer and hidden zones hide batches", () => {
    const { root, scenario, a } = level();
    const objects = new ScenarioObjects();
    objects.adopt(root);
    const crates = batches(scenario).filter((m) => m.geometry === a.geometry);
    objects.setKind("bloc", false);
    expect(crates.every((m) => !m.visible)).toBe(true);
    objects.setKind("bloc", true);
    expect(crates.every((m) => m.visible)).toBe(true);
    root.getObjectByName("bsp_1")!.visible = false;
    objects.beforeDraw();
    expect(crates.filter((m) => m.visible).map((m) => m.count)).toEqual([2]);
    objects.setShown(false);
    expect(batches(scenario).every((m) => !m.visible)).toBe(true);
  });

  test("the level's vehicles (placed, and multiplayer vehicle spawns) hide while the frames draw vehicles", () => {
    const { root, scenario, a } = level();
    const hog = new THREE.Mesh(new THREE.BoxGeometry(2, 1, 1), new THREE.MeshStandardMaterial());
    hog.name = "vehi_0";
    hog.userData = { kind: "vehi", index: 0, tag: "warthog" };
    const ghost = new THREE.Mesh(new THREE.BoxGeometry(1, 1, 2), new THREE.MeshStandardMaterial());
    ghost.name = "netgame_7";
    ghost.userData = { kind: "netgame", index: 7, tag: "ghost", item: "vehi" };
    scenario.add(hog, ghost);
    root.updateMatrixWorld(true);
    const objects = new ScenarioObjects();
    objects.adopt(root);
    const shown = () => batches(scenario).filter((m) => m.visible).map((m) => m.geometry);
    expect(shown()).toContain(hog.geometry);
    expect(shown()).toContain(ghost.geometry);
    objects.setVehiclesHidden(true);
    expect(shown()).not.toContain(hog.geometry);
    expect(shown()).not.toContain(ghost.geometry);
    // The weapon spawn and the crates stay; a vehicle handed out stays hidden too.
    expect(shown()).toContain(a.geometry);
    expect(shown().length).toBe(4);
    expect(objects.find("netgame", 7)!.visible).toBe(false);
    objects.setVehiclesHidden(false);
    expect(shown()).toContain(hog.geometry);
    expect(objects.find("netgame", 7)!.visible).toBe(true);
  });

  test("find hands out a placement's node, out of its batch; apply moves and hides from frames", () => {
    const { root, scenario, b } = level();
    let changes = 0;
    const objects = new ScenarioObjects(() => changes++);
    objects.adopt(root);
    expect(objects.find("bloc", 99)).toBeUndefined();
    expect(objects.find("vehi", 1)).toBeUndefined();
    const node = objects.find("bloc", 1)!;
    expect(node).toBe(b);
    expect(node.visible).toBe(true);
    const pair = batches(scenario).find((m) => m.count === 2)!;
    const m = new THREE.Matrix4();
    pair.getMatrixAt(1, m);
    expect(m.determinant()).toBe(0);
    expect(slot(pair, 0).x).toBe(1);

    // Frame entries: game (x, y, z) is gl (x, z, -y); facing +y turns it a quarter.
    changes = 0;
    expect(objects.apply([{ k: "bloc", i: 1, p: [5, 6, 7], f: [0, 1, 0], u: [0, 0, 1] }])).toBe(true);
    expect(node.position.toArray()).toEqual([5, 7, -6]);
    const fwd = new THREE.Vector3(1, 0, 0).applyQuaternion(node.quaternion);
    expect(fwd.x).toBeCloseTo(0);
    expect(fwd.z).toBeCloseTo(-1);
    expect(changes).toBe(1);
    expect(objects.apply([{ k: "bloc", i: 1, p: [5, 6, 7], f: [0, 1, 0], u: [0, 0, 1] }])).toBe(false);
    // Gone, and back.
    objects.apply([{ k: "bloc", i: 1, gone: true }]);
    expect(node.visible).toBe(false);
    objects.setKind("bloc", true);
    expect(node.visible).toBe(false);
    objects.apply([{ k: "bloc", i: 1, p: [5, 6, 7] }]);
    expect(node.visible).toBe(true);
    // A crate of another zone, picked out, still follows its zone.
    const far = objects.find("bloc", 2)!;
    root.getObjectByName("bsp_1")!.visible = false;
    objects.beforeDraw();
    expect(far.visible).toBe(false);
    // Unknown entries are ignored.
    expect(objects.apply([{ k: "scen", i: 0, p: [0, 0, 0] }])).toBe(false);

    // Back to rest: in the batch where the glb placed it.
    objects.restAll();
    expect(node.visible).toBe(false);
    expect(node.position.toArray()).toEqual([2, 0, 0]);
    expect(slot(pair, 1).x).toBe(2);
  });
});

describe("textures", () => {
  test("the switch is remembered, on unless turned off", () => {
    const s = new MemoryStore();
    expect(texturesPref(s)).toBe(true);
    savePref(false, s);
    expect(s.getItem(TEXTURES_KEY)).toBe("off");
    expect(texturesPref(s)).toBe(false);
    savePref(true, s);
    expect(texturesPref(s)).toBe(true);
    expect(texturesPref(null)).toBe(true);
    const broken = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); } };
    expect(texturesPref(broken)).toBe(true);
    expect(() => savePref(false, broken)).not.toThrow();
  });

  test("a textured primitive names its class material", () => {
    expect(flatOf({ class: "floor", flat: 3 })).toBe(3);
    expect(flatOf({ flat: -1 })).toBeNull();
    expect(flatOf({ flat: "3" })).toBeNull();
    expect(flatOf(undefined)).toBeNull();
  });

  test("both looks are kept per mesh, and batches join their source's pair", () => {
    const { root, scenario, a, b, c, metal } = level();
    const flat = new THREE.MeshStandardMaterial({ name: "metal" });
    const swap = new TextureSwap();
    expect(swap.has()).toBe(false);
    swap.adopt([a, b, c].map((mesh) => ({ mesh, textured: metal, flat })));
    expect(swap.has()).toBe(true);
    const objects = new ScenarioObjects();
    objects.adopt(root, swap);
    const crates = batches(scenario).filter((m) => m.geometry === a.geometry);
    expect(crates.every((m) => m.material === metal)).toBe(true);
    expect(swap.set(false)).toBe(true);
    expect(a.material).toBe(flat);
    expect(crates.every((m) => m.material === flat)).toBe(true);
    expect(swap.set(false)).toBe(false);
    swap.set(true);
    expect(crates.every((m) => m.material === metal)).toBe(true);
    // A mesh added while off starts flat.
    swap.set(false);
    const late = new THREE.Mesh(a.geometry, metal);
    swap.add(late, metal, flat);
    expect(late.material).toBe(flat);
  });

  test("the class materials are loaded from the glb by the index its primitives name", async () => {
    const glb = texturedGlb();
    const gltf = await new GLTFLoader().parseAsync(glb, "");
    const flats = await loadFlats(gltf);
    const meshes: THREE.Mesh[] = [];
    gltf.scene.traverse((o) => { if ((o as THREE.Mesh).isMesh) meshes.push(o as THREE.Mesh); });
    expect(meshes).toHaveLength(2);
    const textured = meshes.find((m) => (m.material as THREE.Material).name === "wall_plate")!;
    expect(textured.geometry.userData).toEqual({ class: "wall", flat: 0 });
    expect(flats.get(textured)!.name).toBe("wall");
    const plain = meshes.find((m) => m !== textured)!;
    expect(flats.has(plain)).toBe(false);
  });
});

/** A glb of two triangles: one with the textured look (its material
 *  `wall_plate`, extras naming class material 0), one plain `wall`. */
function texturedGlb(): ArrayBuffer {
  const pos = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]);
  const nrm = new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1]);
  const uv = new Float32Array([0, 0, 1, 0, 0, 1]);
  const idx = new Uint16Array([0, 1, 2, 0]);
  const parts = [pos, nrm, uv, idx];
  const views: unknown[] = [];
  let off = 0;
  for (const p of parts) { views.push({ buffer: 0, byteOffset: off, byteLength: p.byteLength }); off += p.byteLength; }
  const doc = {
    asset: { version: "2.0" },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ name: "structure", mesh: 0 }],
    meshes: [{ name: "structure", primitives: [
      { attributes: { POSITION: 0, NORMAL: 1, TEXCOORD_0: 2 }, indices: 3, material: 1, extras: { class: "wall", flat: 0 } },
      { attributes: { POSITION: 0, NORMAL: 1 }, indices: 3, material: 0 },
    ] }],
    materials: [
      { name: "wall", pbrMetallicRoughness: { baseColorFactor: [0.5, 0.5, 0.5, 1] } },
      { name: "wall_plate", pbrMetallicRoughness: { metallicFactor: 0 }, extras: { look: true } },
    ],
    accessors: [
      { bufferView: 0, componentType: 5126, count: 3, type: "VEC3", min: [0, 0, 0], max: [1, 1, 0] },
      { bufferView: 1, componentType: 5126, count: 3, type: "VEC3" },
      { bufferView: 2, componentType: 5126, count: 3, type: "VEC2" },
      { bufferView: 3, componentType: 5123, count: 3, type: "SCALAR" },
    ],
    bufferViews: views,
    buffers: [{ byteLength: off }],
  };
  let json = new TextEncoder().encode(JSON.stringify(doc));
  const pad = (4 - (json.length % 4)) % 4;
  json = new Uint8Array([...json, ...new Array(pad).fill(0x20)]);
  const total = 12 + 8 + json.length + 8 + off;
  const out = new Uint8Array(total);
  const dv = new DataView(out.buffer);
  dv.setUint32(0, 0x46546c67, true); dv.setUint32(4, 2, true); dv.setUint32(8, total, true);
  dv.setUint32(12, json.length, true); dv.setUint32(16, 0x4e4f534a, true);
  out.set(json, 20);
  let at = 20 + json.length;
  dv.setUint32(at, off, true); dv.setUint32(at + 4, 0x004e4942, true);
  at += 8;
  for (const p of parts) { out.set(new Uint8Array(p.buffer, p.byteOffset, p.byteLength), at); at += p.byteLength; }
  return out.buffer;
}
