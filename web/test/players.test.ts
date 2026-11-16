import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import type { Frame, FramePlayer } from "../src/api/types";
import { aimOf, cleanAim, cleanId, cleanShots, interpolateFrames, nlerpAim, sampleFrames, yawAim, type SampledPlayer } from "../src/state/frames";
import { parseManifest } from "../src/lib/gameart";
import { FIRING_HOLD_MS, FLASH_MS, MAX_LIVE_SHOTS, MAX_SHOTS_PER_BURST, SHOT_FADE_MS, SHOT_GAP_MS, ShotQueue, flashAlpha, shotAlpha } from "../src/map/shots";
import { BODY_TINT, SECONDARY_SHADE, bipedFile, bodyTint, findNode, modelScale, modelUrl, parseModels, secondaryOf, tintAt, tintColours, tintWeights, weaponFile, weaponLabel } from "../src/viewer/models";
import { ModelLibrary, prepareBiped, tintGeometry } from "../src/viewer/players3d";
import { smoothPlayers, type SmoothPlayer } from "../src/viewer/sync";
import { MOCK_BIPEDS, MOCK_WEAPONS, encodeModelGlb, mockModels } from "../mock/models";
import { World } from "../mock/world";

const player = (extra: Partial<FramePlayer> = {}): FramePlayer => ({ index: 0, alive: true, pos: [0, 0, 0], yaw: 0, ...extra });
const frame = (tick: number, ...players: FramePlayer[]): Frame => ({ game_id: "g", tick, players });
const len = (v: readonly number[]) => Math.hypot(v[0]!, v[1]!, v[2]!);

describe("frame fields", () => {
  test("an old sender's frame samples exactly as before", () => {
    expect(sampleFrames([frame(0, player())], 0)).toEqual([{ index: 0, alive: true, x: 0, y: 0, z: 0, yaw: 0, vehicle: null }]);
  });

  test("species, weapon, aim and shots come through, the aim at unit length", () => {
    const [p] = sampleFrames([frame(30, player({ species: "elite", weapon: "battle_rifle", aim: [3, 4, 0], shots: 3 }))], 30);
    expect(p).toMatchObject({ species: "elite", weapon: "battle_rifle", shots: 3, shotTick: 30 });
    expect(p!.aim).toEqual([0.6, 0.8, 0]);
  });

  test("anything off the contract is dropped, not drawn", () => {
    expect(cleanId("battle_rifle")).toBe("battle_rifle");
    for (const bad of ["Elite", "", "a".repeat(33), "../x", 3, null, undefined]) expect(cleanId(bad)).toBeNull();
    for (const bad of [[0, 0, 0], [1, 2], [1, NaN, 0], [Infinity, 0, 0], "up", null, [1, "0", 0]]) expect(cleanAim(bad)).toBeNull();
    expect(cleanShots(3.9)).toBe(3);
    expect(cleanShots(1000)).toBe(64);
    for (const bad of [0, -2, NaN, "3", null, undefined]) expect(cleanShots(bad)).toBe(0);
    const [p] = sampleFrames([frame(0, player({ species: "Bad Species", weapon: "", aim: [0, 0, 0], shots: -1 }))], 0);
    expect(p).toEqual({ index: 0, alive: true, x: 0, y: 0, z: 0, yaw: 0, vehicle: null });
  });

  test("a player with no aim looks level along their yaw", () => {
    expect(aimOf({ yaw: Math.PI / 2 })[1]).toBeCloseTo(1, 9);
    expect(aimOf({ yaw: 0, aim: [0, 0, 1] })).toEqual([0, 0, 1]);
    expect(yawAim(0)).toEqual([1, 0, 0]);
  });
});

describe("aim between frames", () => {
  test("turns from one frame's aim to the next, staying unit length", () => {
    const a = frame(0, player({ aim: [1, 0, 0] })), b = frame(2, player({ aim: [0, 1, 0] }));
    const [mid] = interpolateFrames(a, b, 1);
    expect(mid!.aim![0]).toBeCloseTo(Math.SQRT1_2, 6);
    expect(mid!.aim![1]).toBeCloseTo(Math.SQRT1_2, 6);
    for (const t of [0, 0.3, 1.7, 2]) expect(len(interpolateFrames(a, b, t)[0]!.aim!)).toBeCloseTo(1, 9);
    expect(interpolateFrames(a, b, 0)[0]!.aim).toEqual([1, 0, 0]);
  });

  test("opposite aims have no middle: the nearer one is used", () => {
    expect(nlerpAim([1, 0, 0], [-1, 0, 0], 0.5)).toEqual([-1, 0, 0]);
    expect(nlerpAim([1, 0, 0], [-1, 0, 0], 0.49)[0]).toBeCloseTo(1, 9);
  });

  test("an aim in only one of the frames is used while that frame is nearer", () => {
    const a = frame(0, player()), b = frame(2, player({ aim: [0, 0, 1], weapon: "smg" }));
    expect(interpolateFrames(a, b, 0.5)[0]!.aim).toBeUndefined();
    expect(interpolateFrames(a, b, 1.5)[0]).toMatchObject({ aim: [0, 0, 1], weapon: "smg" });
  });

  test("shots belong to the frame behind the sampled tick, until the next frame", () => {
    const a = frame(10, player({ shots: 2 })), b = frame(12, player({ shots: 5 }));
    for (const t of [10, 11, 11.9]) expect(interpolateFrames(a, b, t)[0]).toMatchObject({ shots: 2, shotTick: 10 });
    expect(sampleFrames([a, b], 12)[0]).toMatchObject({ shots: 5, shotTick: 12 });
    // Before the first frame nothing has been fired yet.
    expect(sampleFrames([a, b], 3)[0]!.shots).toBeUndefined();
    // The dead don't shoot.
    expect(sampleFrames([frame(0, player({ alive: false, shots: 3 }))], 0)[0]!.shots).toBeUndefined();
  });

  test("the 3D view eases the aim and keeps species and weapon current", () => {
    const shown = new Map<number, SmoothPlayer>();
    const at = (extra: Partial<SampledPlayer>): SampledPlayer => ({ index: 0, alive: true, x: 0, y: 0, z: 0, yaw: 0, vehicle: null, ...extra });
    smoothPlayers(shown, [at({ aim: [1, 0, 0], species: "elite", weapon: "smg" })], 16);
    expect(shown.get(0)).toMatchObject({ aim: [1, 0, 0], species: "elite", weapon: "smg" });
    smoothPlayers(shown, [at({ aim: [0, 1, 0], species: "elite", weapon: "shotgun" })], 16);
    const s = shown.get(0)!;
    expect(s.weapon).toBe("shotgun");
    expect(s.aim[1]).toBeGreaterThan(0.05);
    expect(s.aim[0]).toBeGreaterThan(s.aim[1]); // part of the way, not all of it
    expect(len(s.aim)).toBeCloseTo(1, 9);
    for (let i = 0; i < 60; i++) smoothPlayers(shown, [at({ aim: [0, 1, 0] })], 16);
    expect(shown.get(0)!.aim[1]).toBeCloseTo(1, 4);
    expect(shown.get(0)!.species).toBeNull();
    // A seek jumps.
    smoothPlayers(shown, [at({ aim: [0, 0, -1] })], 16, { snap: true });
    expect(shown.get(0)!.aim).toEqual([0, 0, -1]);
    // No aim in the frame: level along the yaw.
    smoothPlayers(shown, [at({ yaw: Math.PI })], 16, { snap: true });
    expect(shown.get(0)!.aim[0]).toBeCloseTo(-1, 9);
  });
});

describe("shots", () => {
  const firing = (shots: number, shotTick: number, extra: Partial<SampledPlayer> = {}): SampledPlayer =>
    ({ index: 0, alive: true, x: 1, y: 2, z: 3, yaw: 0, vehicle: null, aim: [0, 1, 0], shots, shotTick, ...extra });

  test("a burst is drawn once, however often its frame is sampled", () => {
    const q = new ShotQueue();
    q.observe([firing(1, 10)], 1000);
    q.observe([firing(1, 10)], 1016);
    q.observe([firing(1, 10)], 1033);
    const live = q.live(1033);
    expect(live).toHaveLength(1);
    expect(live[0]).toMatchObject({ index: 0, x: 1, y: 2, z: 3, aim: [0, 1, 0], age: 33 });
    q.observe([firing(1, 12)], 1066);
    expect(q.live(1066)).toHaveLength(2);
  });

  test("a frame's shots come one after another, at most a few", () => {
    const q = new ShotQueue();
    q.observe([firing(40, 10)], 1000);
    expect(q.live(1000)).toHaveLength(1);
    expect(q.live(1000 + SHOT_GAP_MS)).toHaveLength(2);
    expect(q.live(1000 + SHOT_GAP_MS * (MAX_SHOTS_PER_BURST - 1))).toHaveLength(MAX_SHOTS_PER_BURST);
    expect(q.live(1000 + SHOT_GAP_MS * MAX_SHOTS_PER_BURST)).toHaveLength(MAX_SHOTS_PER_BURST);
  });

  test("a shot fades to nothing in about 150 ms and is then forgotten", () => {
    expect(SHOT_FADE_MS).toBe(150);
    expect(shotAlpha(0)).toBe(1);
    expect(shotAlpha(75)).toBeGreaterThan(0);
    expect(shotAlpha(75)).toBeLessThan(shotAlpha(20));
    expect(shotAlpha(SHOT_FADE_MS)).toBe(0);
    expect(shotAlpha(-5)).toBe(0);
    expect(flashAlpha(0)).toBe(1);
    expect(flashAlpha(FLASH_MS)).toBe(0);
    const q = new ShotQueue();
    q.observe([firing(1, 10)], 1000);
    expect(q.busy).toBe(true);
    expect(q.live(1000 + SHOT_FADE_MS - 1)).toHaveLength(1);
    expect(q.live(1000 + SHOT_FADE_MS)).toHaveLength(0);
    expect(q.busy).toBe(false);
  });

  test("never more than the cap alive at once; the oldest go first", () => {
    const q = new ShotQueue();
    const crowd = Array.from({ length: 40 }, (_, index) => firing(1, 10, { index }));
    q.observe(crowd, 1000);
    const live = q.live(1000);
    expect(live).toHaveLength(MAX_LIVE_SHOTS);
    expect(live[0]!.index).toBe(40 - MAX_LIVE_SHOTS);
  });

  test("a seek notes the bursts it lands on without drawing them", () => {
    const q = new ShotQueue();
    q.observe([firing(3, 300)], 1000, { quiet: true });
    expect(q.live(1000)).toHaveLength(0);
    q.observe([firing(3, 300)], 1016);
    expect(q.live(1016)).toHaveLength(0);
    q.observe([firing(1, 302)], 1033);
    expect(q.live(1033)).toHaveLength(1);
  });

  test("no shots, no tracers; and the dead don't fire", () => {
    const q = new ShotQueue();
    q.observe([{ index: 0, alive: true, x: 0, y: 0, z: 0, yaw: 0, vehicle: null }, firing(2, 10, { index: 1, alive: false })], 1000);
    expect(q.live(1000)).toHaveLength(0);
  });

  test("a shot without an aim goes level along the yaw", () => {
    const q = new ShotQueue();
    q.observe([firing(1, 10, { aim: undefined, yaw: Math.PI / 2 })], 1000);
    expect(q.live(1000)[0]!.aim[1]).toBeCloseTo(1, 9);
  });

  test("reduced motion: a steady firing state that holds, then ends", () => {
    const q = new ShotQueue();
    q.observe([firing(1, 10)], 1000);
    expect(q.firing(0, 1000)).toBe(true);
    expect(q.firing(1, 1000)).toBe(false);
    expect(q.firingNow(1000 + FIRING_HOLD_MS - 1)).toEqual(new Set([0]));
    expect(q.firing(0, 1000 + FIRING_HOLD_MS)).toBe(false);
    expect(q.firingNow(1000 + FIRING_HOLD_MS).size).toBe(0);
  });
});

describe("the manifest's models", () => {
  const raw = {
    version: 1,
    bipeds: { spartan: { glb: "models/biped/spartan.glb" }, elite: { glb: "models/biped/elite.glb" } },
    weapons: { battle_rifle: { glb: "models/weapon/battle_rifle.glb" } },
  };

  test("reads the table the contract describes", () => {
    expect(parseModels(raw)).toEqual({
      bipeds: { spartan: "models/biped/spartan.glb", elite: "models/biped/elite.glb" },
      weapons: { battle_rifle: "models/weapon/battle_rifle.glb" },
      vehicles: {},
    });
    expect(parseManifest({ version: 1, images: {}, models: raw })!.models).toEqual(parseModels(raw));
    expect(parseManifest({ version: 1, images: {} })!.models).toBeNull();
  });

  test("another version, nothing in it, or files outside the art are refused", () => {
    expect(parseModels({ ...raw, version: 2 })).toBeNull();
    expect(parseModels({ version: 1, bipeds: {}, weapons: {} })).toBeNull();
    expect(parseModels(null)).toBeNull();
    expect(parseModels("models")).toBeNull();
    expect(parseModels({ version: 1, bipeds: { spartan: { glb: "../secret.glb" }, elite: { glb: "https://x/y.glb" }, "Bad Id": { glb: "a.glb" }, brute: {} } })).toBeNull();
  });

  test("which model a player gets", () => {
    const m = parseModels(raw)!;
    expect(bipedFile(m, "elite")).toBe("models/biped/elite.glb");
    expect(bipedFile(m, "spartan")).toBe("models/biped/spartan.glb");
    // An unknown species, or none in the frame: a spartan.
    expect(bipedFile(m, "brute")).toBe("models/biped/spartan.glb");
    expect(bipedFile(m, null)).toBe("models/biped/spartan.glb");
    // No models, or no spartan to fall back on: the capsule.
    expect(bipedFile(null, "elite")).toBeNull();
    expect(bipedFile({ bipeds: { elite: "e.glb" }, weapons: {}, vehicles: {} }, "brute")).toBeNull();
    expect(bipedFile({ bipeds: { elite: "e.glb" }, weapons: {}, vehicles: {} }, "elite")).toBe("e.glb");
    expect(weaponFile(m, "battle_rifle")).toBe("models/weapon/battle_rifle.glb");
    expect(weaponFile(m, "smg")).toBeNull();
    expect(weaponFile(m, null)).toBeNull();
    expect(weaponFile(null, "battle_rifle")).toBeNull();
    expect(modelUrl("http://s/art/game/", "models/biped/a b.glb")).toBe("http://s/art/game/models/biped/a%20b.glb");
    expect(weaponLabel("battle_rifle")).toBe("battle rifle");
    expect(weaponLabel(null)).toBe("");
  });
});

describe("model size", () => {
  test("life size up close, growing with distance, never past the map's marker scale", () => {
    expect(modelScale(5, 0.72, 4)).toBe(1);
    expect(modelScale(48, 0.72, 4)).toBeCloseTo(2, 6);
    expect(modelScale(500, 0.72, 4)).toBe(4);
    // A small map keeps everyone life size from anywhere.
    expect(modelScale(500, 0.72, 1)).toBe(1);
    expect(modelScale(10, 0, 3)).toBe(3);
  });
});

describe("team colour", () => {
  const red: [number, number, number] = [0.8, 0.1, 0.05];

  test("weights share a vertex between the primary, the secondary and its own colour", () => {
    expect(tintWeights(0, 0)).toEqual([0, 0, 1]);
    expect(tintWeights(1, 0)).toEqual([1, 0, 0]);
    expect(tintWeights(0.25, 0.5)).toEqual([0.25, 0.5, 0.25]);
    // Out of range, or more than the whole: held to 0..1 and scaled together.
    expect(tintWeights(-1, 7)).toEqual([0, 1, 0]);
    expect(tintWeights(1, 1)).toEqual([0.5, 0.5, 0]);
  });

  test("no weight leaves the texture alone; full weight is the player's colour or its darker shade", () => {
    expect(tintAt(0, 0, red)).toEqual([1, 1, 1]);
    expect(tintAt(1, 0, red)).toEqual(red);
    expect(tintAt(0, 1, red)).toEqual(secondaryOf(red));
    expect(secondaryOf(red)).toEqual([0.8 * SECONDARY_SHADE, 0.1 * SECONDARY_SHADE, 0.05 * SECONDARY_SHADE]);
    const half = tintAt(0.5, 0, red);
    expect(half[0]).toBeCloseTo(0.9, 9);
    expect(half[1]).toBeCloseTo(0.55, 9);
    // The secondary is always darker than the primary.
    expect(tintAt(0, 1, red)[0]).toBeLessThan(tintAt(1, 0, red)[0]);
  });

  test("a whole mesh's colours, three floats a vertex", () => {
    const w: Array<[number, number]> = [[0, 0], [1, 0], [0, 1]];
    const out = tintColours(3, (i) => w[i]!, red);
    expect(out).toHaveLength(9);
    expect([...out.slice(0, 3)]).toEqual([1, 1, 1]);
    expect(out[3]).toBeCloseTo(0.8, 6);
    expect(out[6]).toBeCloseTo(0.8 * SECONDARY_SHADE, 6);
  });

  test("a model without regions takes mostly the player's colour all over", () => {
    const t = bodyTint(red);
    expect(t[0]).toBeCloseTo(1 - BODY_TINT + 0.8 * BODY_TINT, 9);
    expect(bodyTint([1, 1, 1])).toEqual([1, 1, 1]);
    expect(t[1]).toBeLessThan(t[0]);
  });
});

describe("the contract's nodes", () => {
  const node = (name: string, ...children: Array<{ name: string; children: never[] }>) => ({ name, children: children as never[] });

  test("found by name anywhere in the tree", () => {
    const tree = { name: "root", children: [node("body"), { name: "rig", children: [node("weapon"), node("head")] }] };
    expect(findNode(tree, "weapon")!.name).toBe("weapon");
    expect(findNode(tree, "head")!.name).toBe("head");
    expect(findNode(tree, "muzzle")).toBeNull();
  });

  test("an exact name wins over a loader's renamed duplicate or another case", () => {
    const tree = { name: "root", children: [node("weapon_1"), node("Head"), node("weapon"), node("weapons_rack")] };
    expect(findNode(tree, "weapon")!.name).toBe("weapon");
    expect(findNode(tree, "head")!.name).toBe("Head");
    expect(findNode({ name: "root", children: [node("weapons_rack"), node("muzzle_2")] }, "muzzle")!.name).toBe("muzzle_2");
    expect(findNode({ name: "root", children: [node("weapons_rack")] }, "weapon")).toBeNull();
  });
});

describe("model glbs", () => {
  const parse = (bytes: Uint8Array) => new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");
  const files: Record<string, Uint8Array> = {
    "spartan.glb": encodeModelGlb(MOCK_BIPEDS.spartan!()),
    "elite.glb": encodeModelGlb(MOCK_BIPEDS.elite!()),
    "rifle.glb": encodeModelGlb(MOCK_WEAPONS.battle_rifle!()),
  };
  const library = () => {
    const calls: string[] = [];
    let ready = 0;
    const lib = new ModelLibrary(() => { ready++; }, (url) => { calls.push(url); return files[url] ? parse(files[url]) : Promise.reject(new Error("404")); });
    return { lib, calls, ready: () => ready };
  };
  const settle = () => new Promise((r) => setTimeout(r, 20));
  const meshesOf = (o: THREE.Object3D) => { const out: THREE.Mesh[] = []; o.traverse((c) => { if ((c as THREE.Mesh).isMesh) out.push(c as THREE.Mesh); }); return out; };

  test("a synthetic biped loads with its weapon and head nodes where the glb put them", async () => {
    const g = await parse(files["spartan.glb"]!);
    const t = prepareBiped(g)!;
    expect(t.height).toBeCloseTo(0.72, 6);
    expect(t.regions).toBe(true);
    const head = findNode(t.root, "head")!, hand = findNode(t.root, "weapon")!;
    // The eye point is near the top and ahead of the feet; the hand is on the right (game -y is glTF +z).
    expect(head.position.y).toBeGreaterThan(0.6);
    expect(head.position.y).toBeLessThan(0.72);
    expect(hand.position.x).toBeGreaterThan(0.1);
    expect(hand.position.z).toBeGreaterThan(0);
    // It stands on the origin.
    expect(new THREE.Box3().setFromObject(t.root).min.y).toBeCloseTo(0, 6);
  });

  test("a biped without the nodes still gets a head and a hand", () => {
    const scene = new THREE.Group();
    scene.add(new THREE.Mesh(new THREE.BoxGeometry(0.4, 1, 0.4).translate(0, 0.5, 0), new THREE.MeshStandardMaterial()));
    const t = prepareBiped({ scene })!;
    expect(t.height).toBeCloseTo(1, 6);
    expect(t.regions).toBe(false);
    expect(findNode(t.root, "head")!.position.y).toBeCloseTo(0.91, 6);
    expect(findNode(t.root, "weapon")).not.toBeNull();
    expect(prepareBiped({ scene: new THREE.Group() })).toBeNull();
  });

  test("each glb is fetched once, and every player gets a clone of their own", async () => {
    const { lib, calls, ready } = library();
    expect(lib.biped("spartan.glb")).toBeNull(); // not yet: a capsule meanwhile
    expect(lib.biped("spartan.glb")).toBeNull();
    await settle();
    expect(ready()).toBe(1);
    const t = lib.biped("spartan.glb")!;
    expect(calls).toEqual(["spartan.glb"]);
    const a = lib.instantiateBiped(t, "#ff0000"), b = lib.instantiateBiped(t, "#ff0000"), c = lib.instantiateBiped(t, "#0000ff");
    expect(a.object).not.toBe(b.object);
    expect(a.head).not.toBe(b.head);
    expect(a.hand.parent).not.toBeNull();
    // Team mates share the coloured mesh; the other team has another; positions are shared by all.
    const ga = meshesOf(a.object)[0]!.geometry, gb = meshesOf(b.object)[0]!.geometry, gc = meshesOf(c.object)[0]!.geometry, g0 = meshesOf(t.root)[0]!.geometry;
    expect(ga).toBe(gb);
    expect(ga).not.toBe(gc);
    expect(ga).not.toBe(g0);
    expect(ga.getAttribute("position")).toBe(g0.getAttribute("position"));
    lib.dispose();
  });

  test("the weights in COLOR_0 become the player's colour and its darker shade", async () => {
    const { lib } = library();
    lib.biped("spartan.glb");
    await settle();
    const t = lib.biped("spartan.glb")!;
    const armour = meshesOf(t.root).find((m) => (m.material as THREE.Material).name === "armour")!;
    const w = armour.geometry.getAttribute("color");
    const tinted = tintGeometry(armour.geometry, [0, 0, 1]).getAttribute("color");
    expect(tinted.itemSize).toBe(3);
    const seen = new Set<string>();
    for (let i = 0; i < w.count; i++) {
      const key = `${w.getX(i)},${w.getY(i)}`;
      if (seen.has(key)) continue;
      seen.add(key);
      const want = tintAt(w.getX(i), w.getY(i), [0, 0, 1]);
      expect([tinted.getX(i), tinted.getY(i), tinted.getZ(i)]).toEqual([want[0], want[1], Math.fround(want[2])]);
    }
    // The stand-in has both a primary and a secondary region.
    expect(seen).toEqual(new Set(["0,1", "1,0"]));
    // A normalised byte encoding reads the same.
    const bytes = new THREE.BufferGeometry();
    bytes.setAttribute("position", new THREE.BufferAttribute(new Float32Array(6), 3));
    bytes.setAttribute("color", new THREE.BufferAttribute(new Uint8Array([255, 0, 0, 255, 0, 255, 0, 255]), 4, true));
    const fromBytes = tintGeometry(bytes, [0, 0, 1]).getAttribute("color");
    expect([fromBytes.getX(0), fromBytes.getY(0), fromBytes.getZ(0)]).toEqual([0, 0, 1]);
    expect(fromBytes.getZ(1)).toBeCloseTo(SECONDARY_SHADE, 6);
    lib.dispose();
  });

  test("a biped with no COLOR_0 is tinted all over, per colour", async () => {
    const { lib } = library();
    lib.biped("elite.glb");
    await settle();
    const t = lib.biped("elite.glb")!;
    expect(t.regions).toBe(false);
    const base = meshesOf(t.root).find((m) => (m.material as THREE.Material).name === "armour")!.material as THREE.MeshStandardMaterial;
    const blue = lib.instantiateBiped(t, "#0000ff"), blue2 = lib.instantiateBiped(t, "#0000ff");
    const mat = meshesOf(blue.object).find((m) => (m.material as THREE.Material).name === "armour")!.material as THREE.MeshStandardMaterial;
    expect(mat).not.toBe(base);
    expect(mat.color.b).toBeCloseTo(base.color.b, 6);
    expect(mat.color.r).toBeCloseTo(base.color.r * (1 - BODY_TINT), 6);
    expect(meshesOf(blue2.object).find((m) => (m.material as THREE.Material).name === "armour")!.material).toBe(mat);
    lib.dispose();
  });

  test("a weapon comes with its muzzle, ahead of the grip", async () => {
    const { lib, calls } = library();
    lib.weapon("rifle.glb");
    await settle();
    const w = lib.instantiateWeapon(lib.weapon("rifle.glb")!), w2 = lib.instantiateWeapon(lib.weapon("rifle.glb")!);
    expect(w.muzzle).not.toBeNull();
    expect(w.muzzle!.position.x).toBeGreaterThan(0.2);
    expect(w.muzzle).not.toBe(w2.muzzle);
    expect(calls).toEqual(["rifle.glb"]);
    // Under a hand, the muzzle's place in the world follows the player.
    const { lib: l2 } = library();
    l2.biped("spartan.glb");
    await settle();
    const body = l2.instantiateBiped(l2.biped("spartan.glb")!, "#fff");
    body.hand.add(w.object);
    body.object.position.set(10, 0, 0);
    body.object.updateWorldMatrix(true, true);
    const at = w.muzzle!.getWorldPosition(new THREE.Vector3());
    expect(at.x).toBeCloseTo(10 + body.hand.position.x + w.muzzle!.position.x, 5);
    lib.dispose();
    l2.dispose();
  });

  test("a glb that can't be loaded is asked for once and stays a capsule", async () => {
    const { lib, calls, ready } = library();
    expect(lib.biped("missing.glb")).toBeNull();
    await settle();
    expect(lib.biped("missing.glb")).toBeNull();
    expect(lib.failed("missing.glb")).toBe(true);
    expect(calls).toEqual(["missing.glb"]);
    expect(ready()).toBe(0);
  });

  test("the mock serves the models its manifest names, and nothing else", () => {
    const m = mockModels();
    const table = parseModels(m.models)!;
    for (const file of [...Object.values(table.bipeds), ...Object.values(table.weapons)]) {
      const f = m.file(file)!;
      expect(new TextDecoder().decode(f.bytes.slice(0, 4))).toBe("glTF");
    }
    expect(m.file("models/biped/hunter.glb")).toBeNull();
    expect(m.file("models/../x.glb")).toBeNull();
  });
});

describe("the mock's frames", () => {
  test("carry species, a unit aim that follows the facing, a weapon, and bursts of shots", () => {
    const frames: Frame[] = [];
    const world = new World();
    world.listeners.add({ onEvent() {}, onFrame: (f) => frames.push(f) });
    world.seed(20_000);
    world.live = true;
    for (let i = 0; i < 900; i++) { world.now += 1000 / 15; world.step(); }
    expect(frames.length).toBeGreaterThan(100);
    const players = frames.flatMap((f) => f.players);
    const species = new Set(players.map((p) => p.species));
    expect(species).toEqual(new Set(["spartan", "elite"]));
    let shots = 0;
    for (const p of players) {
      expect(cleanId(p.weapon)).toBe(p.weapon!);
      expect(len(p.aim!)).toBeCloseTo(1, 2);
      // Looking where they face, give or take the pitch.
      expect(Math.cos(p.yaw) * p.aim![0] + Math.sin(p.yaw) * p.aim![1]).toBeGreaterThan(0.9);
      if (p.shots !== undefined) { expect(p.alive).toBe(true); expect(cleanShots(p.shots)).toBe(p.shots!); shots++; }
    }
    expect(shots).toBeGreaterThan(0);
    expect(shots).toBeLessThan(players.length / 2);
    expect(players.some((p) => Math.abs(p.aim![2]) > 0.1)).toBe(true);
  });
});
