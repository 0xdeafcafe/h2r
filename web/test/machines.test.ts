import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import type { Frame, FrameMachine } from "../src/api/types";
import { FrameBuffer, MAX_MACHINES, fromFrameMachine, sampleMachines } from "../src/state/frames";
import { fetchFeatures, glbExtras, glbJsonEnd, liftEnd, linkArc, linkEnds, parseFeatures } from "../src/map/features";
import { buildFeatures, RING } from "../src/viewer/features3d";
import { isIdentity, machineMatrix, machineNode, placeMachine } from "../src/viewer/machines3d";
import { parseModels } from "../src/viewer/models";
import { buildArena, enclose, encodeGlb, furnish, mockMapGeometry } from "../mock/arena";
import { machineHooks, mockFixtures, mockMachines, platformPosition } from "../mock/fixtures";
import { MAPS } from "../mock/maps";
import { mockModels } from "../mock/models";

const machine = (over: Partial<FrameMachine> = {}): FrameMachine => ({ node: "platform_0", id: "d1", kind: "platform", pos: [0, 0, 0], forward: [1, 0, 0], up: [0, 0, 1], ...over });
const frame = (tick: number, machines?: FrameMachine[]): Frame => ({ game_id: "g", tick, players: [], ...(machines ? { machines } : {}) });
const parse = (bytes: Uint8Array) => new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");

describe("machines in the frames", () => {
  test("a machine is read to the contract, and anything off it is dropped or mended", () => {
    expect(fromFrameMachine(machine({ pos: [1, 2, 3], position: 0.25 }))).toEqual({ id: "d1", node: "platform_0", kind: "platform", x: 1, y: 2, z: 3, forward: [1, 0, 0], up: [0, 0, 1], position: 0.25 });
    expect(fromFrameMachine(machine({ node: undefined }))!.node).toBeNull();
    expect(fromFrameMachine(machine({ node: "../../etc" }))!.node).toBeNull();
    expect(fromFrameMachine(machine({ position: 7 }))!.position).toBe(1);
    expect(fromFrameMachine(machine({ position: NaN }))!.position).toBeNull();
    expect(fromFrameMachine(machine({ forward: [0, 0, 0] }))!.forward).toEqual([1, 0, 0]);
    expect(fromFrameMachine(machine({ pos: [NaN, 0, 0] }))).toBeNull();
    expect(fromFrameMachine(machine({ id: "" }))).toBeNull();
  });

  test("recorded frames carry every machine: they glide from one second to the next", () => {
    const frames = [frame(30, [machine({ pos: [0, 0, 0], position: 0 })]), frame(60, [machine({ pos: [0, 0, 4], position: 1 })])];
    expect(sampleMachines(frames, 29)).toEqual([]);
    expect(sampleMachines(frames, 30)[0]).toMatchObject({ z: 0, position: 0 });
    expect(sampleMachines(frames, 45)[0]).toMatchObject({ z: 2, position: 0.5 });
    expect(sampleMachines(frames, 60)[0]).toMatchObject({ z: 4, position: 1 });
    expect(sampleMachines(frames, 600)[0]!.z).toBe(4);
    expect(sampleMachines([frame(30, Array.from({ length: 300 }, (_, i) => machine({ id: "d" + i })))], 30)).toHaveLength(MAX_MACHINES);
  });

  test("live frames carry only what moved: a machine stays where it was last sent", () => {
    const frames = [frame(10, [machine({ pos: [0, 0, 1] }), machine({ id: "d2", node: null, kind: "walker", pos: [5, 5, 0] })]), frame(11), frame(12, [machine({ pos: [0, 0, 2] })]), frame(13)];
    // At tick 11.5 the platform is on its way to where frame 12 has it; the walker hasn't been heard of since frame 10.
    const at = sampleMachines(frames, 11.5);
    expect(at.find((m) => m.id === "d1")!.z).toBeCloseTo(1.5, 6);
    expect(at.find((m) => m.id === "d2")).toMatchObject({ x: 5, node: null, kind: "walker" });
    expect(sampleMachines(frames, 13).find((m) => m.id === "d1")!.z).toBe(2);
    // A machine quiet for longer than the frames kept is remembered by the live buffer.
    const buf = new FrameBuffer(4, 0);
    buf.push(frames[0]!, 0);
    for (let t = 11; t < 40; t++) buf.push(frame(t, t === 20 ? [machine({ pos: [0, 0, 3] })] : undefined), (t - 10) * 33.3);
    const live = buf.sampleMachines(30 * 33.3);
    expect(live.map((m) => [m.id, m.z]).sort()).toEqual([["d1", 3], ["d2", 0]]);
    expect(new FrameBuffer().sampleMachines(0)).toEqual([]);
  });
});

describe("moving a glb's machine nodes", () => {
  const at = (x: number, y: number, z: number, forward: [number, number, number] = [1, 0, 0]) => machineMatrix({ x, y, z, forward, up: [0, 0, 1] });

  test("a machine's place as a matrix: game z is up, forward is the model's +x", () => {
    const m = at(1, 2, 3, [0, 1, 0]);
    expect(new THREE.Vector3().setFromMatrixPosition(m).toArray()).toEqual([1, 3, -2]);
    // The model's nose (+x) points along game +y, which is the scene's -z.
    const nose = new THREE.Vector3(1, 0, 0).transformDirection(m);
    expect(nose.x).toBeCloseTo(0, 6); expect(nose.z).toBeCloseTo(-1, 6);
    expect(isIdentity(at(0, 0, 0))).toBe(true);
    expect(isIdentity(m)).toBe(false);
  });

  test("a mesh baked in world space moves by how far the machine has come since first seen", () => {
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(1, 1, 1).translate(10, 0.5, -20), new THREE.MeshBasicMaterial());
    const n = machineNode(mesh);
    expect(n.baked).toBe(true);
    // First seen where the scenario put it: nothing moves.
    expect(placeMachine(n, at(10, 20, 0))).toBe(false);
    expect(placeMachine(n, at(10, 20, 3))).toBe(true);
    mesh.updateMatrixWorld(true);
    const box = new THREE.Box3().setFromObject(mesh);
    expect(box.min.y).toBeCloseTo(3, 5);
    expect(box.getCenter(new THREE.Vector3()).x).toBeCloseTo(10, 5);
    // Turned a quarter about its own place, it stays over that place.
    placeMachine(n, at(10, 20, 3, [0, 1, 0]));
    mesh.updateMatrixWorld(true);
    const c = new THREE.Box3().setFromObject(mesh).getCenter(new THREE.Vector3());
    expect(c.x).toBeCloseTo(10, 5); expect(c.z).toBeCloseTo(-20, 5);
    expect(placeMachine(n, at(10, 20, 3, [0, 1, 0]))).toBe(false);
  });

  test("a node with a transform of its own is put where the frame says, keeping its scale", () => {
    const mesh = new THREE.Mesh(new THREE.BoxGeometry(1, 1, 1), new THREE.MeshBasicMaterial());
    mesh.position.set(4, 0, -4);
    mesh.scale.setScalar(2);
    const n = machineNode(mesh);
    expect(n.baked).toBe(false);
    // Wherever it was first seen, it goes to the frame's place.
    placeMachine(n, at(7, 8, 9));
    const p = new THREE.Vector3(), q = new THREE.Quaternion(), s = new THREE.Vector3();
    mesh.matrix.decompose(p, q, s);
    expect(p.toArray()).toEqual([7, 9, -8]);
    expect(s.x).toBeCloseTo(2, 6);
    expect(mesh.matrixAutoUpdate).toBe(false);
  });
});

describe("a map's teleporters and lifts", () => {
  const raw = {
    teleporters: [
      { id: "t0", pos: [0, 0, 0], facing: 1.5, role: "sender", channel: 0 }, { id: "t1", pos: [10, 0, 2], facing: 0, role: "receiver", channel: 0 },
      { id: "t2", pos: [0, 9, 0], role: "two_way" }, { id: "t3", pos: [9, 9, 0], role: "weird" },
      { id: "t0", pos: [5, 5, 5] }, { id: "bad", pos: [1, 2] }, null,
    ],
    links: [{ from: "t0", to: "t1" }, { from: "t2", to: "t3" }, { from: "t3", to: "t2" }, { from: "t0", to: "nowhere" }, { from: "t1", to: "t1" }, { from: "t0", to: "t1" }],
    lifts: [{ id: "l0", pos: [1, 1, 0], dir: [0, 0, 2], length: 6, radius: 1.2, source: "x" }, { id: "l1", pos: [2, 2, 0] }, { id: "l2", pos: "no" }],
  };

  test("the extras are read to the contract; what is off it is dropped or given a default", () => {
    const f = parseFeatures({ features: raw })!;
    expect(f.teleporters.map((t) => [t.id, t.role, t.channel])).toEqual([["t0", "sender", 0], ["t1", "receiver", 0], ["t2", "two_way", null], ["t3", "two_way", null]]);
    expect(f.teleporters[0]).toMatchObject({ x: 0, y: 0, z: 0, facing: 1.5 });
    expect(f.links).toEqual([{ from: "t0", to: "t1" }, { from: "t2", to: "t3" }, { from: "t3", to: "t2" }]);
    expect(f.lifts).toEqual([
      { id: "l0", x: 1, y: 1, z: 0, dir: [0, 0, 1], length: 6, radius: 1.2 },
      { id: "l1", x: 2, y: 2, z: 0, dir: [0, 0, 1], length: 4, radius: 0.8 },
    ]);
    expect(liftEnd(f.lifts[0]!)).toEqual([1, 1, 6]);
    // The features object itself reads too; nothing to draw is null.
    expect(parseFeatures(raw)!.lifts).toHaveLength(2);
    expect(parseFeatures({ features: { teleporters: [], lifts: [] } })).toBeNull();
    expect(parseFeatures({ generator: "x" })).toBeNull();
    expect(parseFeatures(null)).toBeNull();
  });

  test("links pair teleporters up, a there-and-back pair as one two-way link", () => {
    const ends = linkEnds(parseFeatures(raw)!);
    expect(ends.map((l) => [l.from.id, l.to.id, l.both])).toEqual([["t0", "t1", false], ["t2", "t3", true]]);
  });

  test("a link arcs through the air between its ends", () => {
    const arc = linkArc({ x: 0, y: 0, z: 0 }, { x: 10, y: 0, z: 2 }, 11);
    expect(arc[0]).toEqual([0, 0, 0]);
    expect(arc[10]).toEqual([10, 0, 2]);
    expect(arc[5]![2]).toBeCloseTo(1 + Math.hypot(10, 2) * 0.2, 6);
    // Close together it still rises enough to see.
    expect(linkArc({ x: 0, y: 0, z: 0 }, { x: 1, y: 0, z: 0 }, 3)[1]![2]).toBe(1);
  });

  test("the 3D view builds a ring per teleporter, an arc per link and a column per lift", () => {
    const g = buildFeatures(parseFeatures(raw)!);
    const names = g.children.map((c) => c.name).filter(Boolean);
    expect(names).toEqual(["teleporter:t0", "teleporter:t1", "teleporter:t2", "teleporter:t3", "link:t0:t1", "link:t2:t3", "lift:l0", "lift:l1"]);
    // One arrowhead for a one-way link, two for a two-way.
    expect(g.children.filter((c) => !c.name)).toHaveLength(3);
    // t1 stands at game (10, 0, 2): scene (10, 2, 0), its ring a doorway above the ground.
    const t1 = g.getObjectByName("teleporter:t1")!;
    expect(t1.position.toArray()).toEqual([10, 2, -0]);
    g.updateMatrixWorld(true);
    const box = new THREE.Box3().setFromObject(t1);
    expect(box.max.y - 2).toBeGreaterThan(RING * 2);
    // The lift's column stands the lift's length up from its base.
    const lift = new THREE.Box3().setFromObject(g.getObjectByName("lift:l0")!);
    expect(lift.min.y).toBeCloseTo(-0.06, 1);
    expect(lift.max.y).toBeGreaterThan(6);
    expect(lift.max.x - lift.min.x).toBeCloseTo(2.4 + 0.12, 1);
  });

  test("the 2D map reads the features from the head of the glb, without the meshes", async () => {
    const map = MAPS.find((m) => m.name === "orchard")!;
    const bytes = encodeGlb(furnish(buildArena("orchard")!, mockFixtures(map)));
    const end = glbJsonEnd(bytes)!;
    expect(end).toBeLessThan(bytes.length / 4);
    expect(glbJsonEnd(new Uint8Array(40))).toBeNull();
    expect(glbExtras(bytes.subarray(0, end - 1))).toBeNull();
    const f = parseFeatures(glbExtras(bytes.subarray(0, end)))!;
    expect(f.teleporters).toHaveLength(4);
    expect(f.lifts).toHaveLength(1);
    // Fetched in pieces, it stops reading once the JSON is in.
    let read = 0, cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      pull(c) { const n = Math.min(4096, bytes.length - read); if (!n) { c.close(); return; } c.enqueue(bytes.slice(read, read + n)); read += n; },
      cancel() { cancelled = true; },
    });
    const got = await fetchFeatures("x", (async () => new Response(body)) as unknown as typeof fetch);
    expect(got).toEqual(f);
    expect(cancelled).toBe(true);
    expect(read).toBeLessThan(bytes.length / 2);
    expect(await fetchFeatures("x", (async () => new Response("no", { status: 404 })) as unknown as typeof fetch)).toBeNull();
    expect(await fetchFeatures("x", (async () => new Response("not a glb at all, just some text")) as unknown as typeof fetch)).toBeNull();
    expect(await fetchFeatures("x", (async () => { throw new Error("offline"); }) as unknown as typeof fetch)).toBeNull();
  });
});

describe("the mock's furniture", () => {
  const map = MAPS.find((m) => m.name === "teacup")!;
  const fx = mockFixtures(map)!;

  test("the glb has the platform as a machines child and the features in its extras, indoors too", async () => {
    for (const arena of [furnish(buildArena("teacup")!, fx), enclose(furnish(buildArena("teacup")!, fx))]) {
      const g = await parse(encodeGlb(arena));
      const machines = g.scene.getObjectByName("machines")!;
      expect(machines.children.map((c) => c.name)).toContain(fx.platform.node);
      expect(isIdentity(machines.getObjectByName(fx.platform.node)!.matrix)).toBe(true);
      const f = parseFeatures((g.asset as { extras?: unknown }).extras)!;
      expect(f.teleporters.map((t) => t.role)).toEqual(["sender", "receiver", "two_way", "two_way"]);
      expect(linkEnds(f).map((l) => l.both)).toEqual([false, true]);
    }
    // The plain arena, as the tests elsewhere build it, has neither.
    expect((await parse(encodeGlb(buildArena("teacup")!))).scene.getObjectByName("machines")).toBeUndefined();
    expect(mockMapGeometry({ furniture: false }).file("map/teacup/map.glb")!.bytes.length).toBeLessThan(mockMapGeometry().file("map/teacup/map.glb")!.bytes.length);
    expect(mockFixtures({ name: "tiny", outline: null, waypoints: [[0, 0, 0]] })).toBeNull();
  });

  test("the platform rides up and down and rests at each end; the walker has no node", () => {
    expect(platformPosition(0)).toBe(0);
    expect(platformPosition(30 * 5)).toBe(1);
    expect(platformPosition(30 * 2)).toBeCloseTo(0.5, 6);
    expect(platformPosition(30 * 11)).toBe(0);
    const [platform, walker] = mockMachines(fx, 30 * 5);
    expect(platform).toMatchObject({ node: "platform_0", kind: "platform", position: 1 });
    expect(platform!.pos[2]).toBe(fx.platform.base[2] + fx.platform.travel);
    expect(walker!.node).toBeUndefined();
    expect(walker!.kind).toBe("walker");
    // The server's models have something to draw the walker with.
    expect(parseModels(mockModels().models)!.objects).toEqual({ walker: "models/object/walker.glb" });
    expect(mockModels().file("models/object/walker.glb")).not.toBeNull();
  });

  test("live frames carry only the machines that moved, recorded ones all of them", () => {
    const hooks = machineHooks();
    const live: Frame[] = [], kept: Frame[] = [];
    for (let tick = 1; tick <= 30 * 12; tick++) {
      const f = frame(tick), game = { id: "g", map, tick, mode: "multiplayer" };
      hooks.frame(f, game);
      live.push(f);
      if (tick % 30 === 0) { const k = { ...f }; hooks.record(k, game); kept.push(k); }
    }
    expect(live[0]!.machines).toHaveLength(2);
    // Rising: the platform every frame. Resting at the top: not at all.
    expect(live[60]!.machines!.some((m) => m.id === fx.platform.id)).toBe(true);
    expect(live.slice(30 * 4 + 5, 30 * 6 - 5).every((f) => !f.machines?.some((m) => m.id === fx.platform.id))).toBe(true);
    expect(kept.every((k) => k.machines!.length === 2)).toBe(true);
    // The client reads them back, and a replay finds the platform at the top half way through.
    expect(sampleMachines(kept, 30 * 5).find((m) => m.id === fx.platform.id)!.position).toBe(1);
    const buf = new FrameBuffer(48, 0);
    live.forEach((f, i) => buf.push(f, i * 33.33));
    expect(buf.sampleMachines(live.length * 33.33).map((m) => m.id).sort()).toEqual([fx.platform.id, fx.walker.id]);
    // A map too small for furniture gets none.
    const f = frame(1);
    hooks.frame(f, { id: "h", map: { name: "tiny", outline: null, waypoints: [] }, tick: 1 });
    expect(f.machines).toBeUndefined();
  });
});
