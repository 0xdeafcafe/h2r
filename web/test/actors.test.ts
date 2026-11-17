import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import type { Frame, FrameActor, FramePlayer, FrameVehicle } from "../src/api/types";
import { MAX_ACTORS, actorIndex, sampleActors, sampleBsp, sampleFrames, sampleVehicles, type SampledPlayer, type SampledVehicle } from "../src/state/frames";
import { ShotQueue } from "../src/map/shots";
import { vehicleColour } from "../src/map/render";
import { EMPTY_VEHICLE_SHADE, actorColour, bipedFile, parseModels, vehicleBasis, vehicleBox, vehicleFile } from "../src/viewer/models";
import { ModelLibrary } from "../src/viewer/players3d";
import { smoothPlayers, smoothVehicles, type SmoothPlayer } from "../src/viewer/sync";
import { MOCK_ACTORS, ZONE_TICKS, campaignHook } from "../mock/campaign";
import { MAPS } from "../mock/maps";
import { MOCK_ACTOR_BIPEDS, MOCK_BIPEDS, MOCK_VEHICLES, encodeModelGlb, mockModels } from "../mock/models";

const actor = (id: string, x: number, extra: Partial<FrameActor> = {}): FrameActor => ({ id, species: "grunt", pos: [x, 0, 0], yaw: 0, team: "covenant", ...extra });
const vehicle = (id: string, x: number, extra: Partial<FrameVehicle> = {}): FrameVehicle => ({ id, kind: "warthog", pos: [x, 0, 0], forward: [1, 0, 0], up: [0, 0, 1], ...extra });
const frame = (tick: number, extra: Partial<Frame> = {}): Frame => ({ game_id: "g", tick, players: [], ...extra });
const len = (v: readonly number[]) => Math.hypot(v[0]!, v[1]!, v[2]!);

describe("actors in frames", () => {
  test("an actor's index is negative and the same in every frame", () => {
    expect(actorIndex("e1000")).toBeLessThan(0);
    expect(actorIndex("e1000")).toBe(actorIndex("e1000"));
    expect(actorIndex("e1000")).not.toBe(actorIndex("e1001"));
    expect(actorIndex("")).toBe(-1);
  });

  test("they are sampled like players, with their id and side beside them", () => {
    const frames = [frame(0, { actors: [actor("a1", 0, { aim: [1, 0, 0], weapon: "plasma_pistol" })] }), frame(2, { actors: [actor("a1", 4, { aim: [0, 1, 0] })] })];
    const [a] = sampleActors(frames, 1);
    expect(a).toMatchObject({ index: actorIndex("a1"), alive: true, x: 2, species: "grunt", actor: { id: "a1", team: "covenant" } });
    expect(a!.aim![0]).toBeCloseTo(Math.SQRT1_2, 6);
    expect(sampleActors(frames, 0)[0]!.weapon).toBe("plasma_pistol");
    // Players are untouched by a frame's actors.
    expect(sampleFrames(frames, 1)).toEqual([]);
  });

  test("only the living are sent: one missing from the next frame goes when that frame is nearer", () => {
    const frames = [frame(0, { actors: [actor("a1", 0), actor("a2", 5)] }), frame(2, { actors: [actor("a2", 5), actor("a3", 9)] })];
    expect(sampleActors(frames, 0.5).map((a) => a.actor!.id)).toEqual(["a1", "a2"]);
    expect(sampleActors(frames, 1.5).map((a) => a.actor!.id)).toEqual(["a2", "a3"]);
  });

  test("a frame with none, an old sender, and anything off the contract", () => {
    expect(sampleActors([frame(0)], 0)).toEqual([]);
    expect(sampleActors([], 5)).toEqual([]);
    const bad = [
      actor("Bad Id", 0), actor("a1", NaN), { ...actor("a2", 0), yaw: Infinity }, actor("ok", 1, { team: "Not A Team", species: "??" }), actor("ok", 2),
    ] as FrameActor[];
    const out = sampleActors([frame(0, { actors: bad })], 0);
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ x: 1, actor: { id: "ok", team: null } });
    expect(out[0]!.species).toBeUndefined();
  });

  test("no more than the cap are drawn from one frame", () => {
    const many = Array.from({ length: 200 }, (_, i) => actor("a" + i, i));
    expect(sampleActors([frame(0, { actors: many })], 0)).toHaveLength(MAX_ACTORS);
  });

  test("their shots draw like players', and never collide with a player's", () => {
    const q = new ShotQueue();
    const frames = [frame(10, { actors: [actor("a1", 0, { shots: 1 })], players: [{ index: 0, alive: true, pos: [0, 0, 0], yaw: 0, shots: 1 } as FramePlayer] })];
    q.observe(sampleFrames(frames, 10), 1000);
    q.observe(sampleActors(frames, 10), 1000);
    expect(q.live(1000).map((s) => s.index)).toEqual([0, actorIndex("a1")]);
  });

  test("the 3D view eases them with the players and knows who is aboard what", () => {
    const shown = new Map<number, SmoothPlayer>();
    const p: SampledPlayer = { index: 0, alive: true, x: 0, y: 0, z: 0, yaw: 0, vehicle: "f2000" };
    const [a] = sampleActors([frame(0, { actors: [actor("a1", 3, { vehicle: "f2000" })] })], 0);
    smoothPlayers(shown, [p, a!], 16);
    expect(shown.get(0)).toMatchObject({ vehicle: "f2000", actor: null });
    expect(shown.get(actorIndex("a1"))).toMatchObject({ vehicle: "f2000", actor: { id: "a1", team: "covenant" }, species: "grunt" });
    smoothPlayers(shown, [{ ...p, vehicle: null }], 16);
    expect(shown.get(0)!.vehicle).toBeNull();
    expect(shown.has(actorIndex("a1"))).toBe(false);
  });
});

describe("vehicles in frames", () => {
  test("glide between frames, axes kept unit length", () => {
    const frames = [frame(0, { vehicles: [vehicle("v1", 0)] }), frame(2, { vehicles: [vehicle("v1", 4, { forward: [0, 1, 0] })] })];
    const [v] = sampleVehicles(frames, 1);
    expect(v).toMatchObject({ id: "v1", kind: "warthog", x: 2 });
    expect(len(v!.forward)).toBeCloseTo(1, 9);
    expect(v!.forward[0]).toBeCloseTo(Math.SQRT1_2, 6);
    expect(v!.up).toEqual([0, 0, 1]);
  });

  test("one is there while someone is in it: gone, or new, at the nearer frame", () => {
    const frames = [frame(0, { vehicles: [vehicle("v1", 0)] }), frame(2, { vehicles: [vehicle("v2", 9)] })];
    expect(sampleVehicles(frames, 0.5).map((v) => v.id)).toEqual(["v1"]);
    expect(sampleVehicles(frames, 1.5).map((v) => v.id)).toEqual(["v2"]);
    expect(sampleVehicles([frame(0)], 0)).toEqual([]);
  });

  test("a far jump snaps, and bad vectors get level defaults", () => {
    const frames = [frame(0, { vehicles: [vehicle("v1", 0)] }), frame(2, { vehicles: [vehicle("v1", 100)] })];
    expect(sampleVehicles(frames, 0.9)[0]!.x).toBe(0);
    expect(sampleVehicles(frames, 1.1)[0]!.x).toBe(100);
    const odd = [vehicle("v1", 0, { forward: [0, 0, 0], up: [NaN, 0, 0], kind: "Bad Kind" }), vehicle("no good", 0), { ...vehicle("v3", 0), pos: [0, 0] }] as FrameVehicle[];
    expect(sampleVehicles([frame(0, { vehicles: odd })], 0)).toEqual([{ id: "v1", kind: "vehicle", x: 0, y: 0, z: 0, forward: [1, 0, 0], up: [0, 0, 1], occupied: true }]);
  });

  test("the 3D view eases them and drops one nobody is in", () => {
    const shown = new Map<string, SampledVehicle>();
    const at = (x: number, forward: [number, number, number] = [1, 0, 0]): SampledVehicle => ({ id: "v1", kind: "ghost", x, y: 0, z: 0, forward, up: [0, 0, 1], occupied: true });
    smoothVehicles(shown, [at(0)], 16);
    smoothVehicles(shown, [at(1, [0, 1, 0])], 16);
    const v = shown.get("v1")!;
    expect(v.x).toBeGreaterThan(0);
    expect(v.x).toBeLessThan(1);
    expect(len(v.forward)).toBeCloseTo(1, 9);
    smoothVehicles(shown, [at(50)], 16);
    expect(shown.get("v1")!.x).toBe(50);
    smoothVehicles(shown, [at(51)], 16, { snap: true });
    expect(shown.get("v1")!.x).toBe(51);
    smoothVehicles(shown, [], 16);
    expect(shown.size).toBe(0);
  });

  test("how one is turned: forward, up and right as the scene's axes", () => {
    // Level, facing game +x: the model's own frame.
    const level = vehicleBasis([1, 0, 0], [0, 0, 1]);
    [[level.x, [1, 0, 0]], [level.y, [0, 1, 0]], [level.z, [0, 0, 1]]].forEach(([got, want]) => got!.forEach((n, i) => expect(n).toBeCloseTo(want![i]!, 9)));
    // Facing game +y (north) is glTF -z; its right is glTF +x.
    const north = vehicleBasis([0, 1, 0], [0, 0, 1]);
    expect(north.x[2]).toBeCloseTo(-1, 9);
    expect(north.z[0]).toBeCloseTo(1, 9);
    // A banking flyer, with an up that isn't quite square to its forward.
    const b = vehicleBasis([0.8, 0, 0.6], [0.1, 0.5, 0.86]);
    for (const v of [b.x, b.y, b.z]) expect(len(v)).toBeCloseTo(1, 9);
    const dot = (p: readonly number[], q: readonly number[]) => p[0]! * q[0]! + p[1]! * q[1]! + p[2]! * q[2]!;
    expect(dot(b.x, b.y)).toBeCloseTo(0, 9);
    expect(dot(b.x, b.z)).toBeCloseTo(0, 9);
    expect(dot(b.y, b.z)).toBeCloseTo(0, 9);
    // An up along the forward says nothing: the world's up is used.
    const flat = vehicleBasis([1, 0, 0], [1, 0, 0]);
    expect(flat.y[1]).toBeCloseTo(1, 9);
    expect(vehicleBasis([0, 0, 0], [0, 0, 0]).x).toEqual([1, 0, 0]);
  });

  test("a box about the size of the kind, when there is no model", () => {
    const [warthog, ghost, scorpion] = [vehicleBox("warthog"), vehicleBox("ghost"), vehicleBox("scorpion")];
    expect(ghost[0]).toBeLessThan(warthog[0]);
    expect(scorpion[0]).toBeGreaterThan(warthog[0]);
    expect(vehicleBox("something_new")).toEqual(warthog);
  });

  test("the outline on the 2D map takes its player's colour, else its AI's side", () => {
    const roster = new Map([[0, { name: "A", colour: "#ff0000" }]]);
    const p: SampledPlayer = { index: 0, alive: true, x: 0, y: 0, z: 0, yaw: 0, vehicle: "v1" };
    const [a] = sampleActors([frame(0, { actors: [actor("a1", 0, { vehicle: "v2", team: "human" })] })], 0);
    expect(vehicleColour("v1", [p], [a!], roster)).toBe("#ff0000");
    expect(vehicleColour("v2", [p], [a!], roster)).toBe(actorColour("human"));
    expect(vehicleColour("v3", [p], [a!], roster)).toBe("#9aa3ab");
  });
});

describe("models for vehicles and AI", () => {
  const raw = {
    version: 1,
    bipeds: { spartan: { glb: "models/biped/spartan.glb" }, grunt: { glb: "models/biped/grunt.glb" } },
    weapons: {},
    vehicles: { warthog: { glb: "models/vehicle/warthog.glb" }, bad: { glb: "../x.glb" } },
  };

  test("the manifest's vehicles table, and a manifest with only vehicles", () => {
    const m = parseModels(raw)!;
    expect(m.vehicles).toEqual({ warthog: "models/vehicle/warthog.glb" });
    expect(vehicleFile(m, "warthog")).toBe("models/vehicle/warthog.glb");
    expect(vehicleFile(m, "ghost")).toBeNull();
    expect(vehicleFile(null, "warthog")).toBeNull();
    expect(parseModels({ version: 1, vehicles: raw.vehicles })).toEqual({ bipeds: {}, weapons: {}, vehicles: { warthog: "models/vehicle/warthog.glb" }, objects: {}, classes: {}, version: 1, animated: false });
  });

  test("an AI gets its species' model, else the spartan's, else a capsule", () => {
    const m = parseModels(raw)!;
    expect(bipedFile(m, "grunt")).toBe("models/biped/grunt.glb");
    expect(bipedFile(m, "hunter")).toBe("models/biped/spartan.glb");
    expect(bipedFile({ ...m, bipeds: {} }, "hunter")).toBeNull();
  });

  test("sides have their colours; an unknown one is neutral", () => {
    expect(actorColour("covenant")).toBe("#a66be0");
    expect(actorColour("human")).toBe("#58b368");
    expect(actorColour("flood")).toBe("#c9a66b");
    expect(actorColour("sentinel")).toBe("#9aa3ab");
    expect(new Set(["covenant", "human", "flood", "sentinel"].map(actorColour)).size).toBe(4);
    expect(actorColour("7")).toBe(actorColour(null));
  });

  const parse = (bytes: Uint8Array) => new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");
  const files: Record<string, Uint8Array> = {
    "warthog.glb": encodeModelGlb(MOCK_VEHICLES.warthog!()),
    "spartan.glb": encodeModelGlb(MOCK_BIPEDS.spartan!()),
    "grunt.glb": encodeModelGlb(MOCK_ACTOR_BIPEDS.grunt!()),
  };
  const settle = () => new Promise((r) => setTimeout(r, 20));
  const mats = (o: THREE.Object3D) => { const out: THREE.MeshStandardMaterial[] = []; o.traverse((c) => { const m = c as THREE.Mesh; if (m.isMesh) out.push(m.material as THREE.MeshStandardMaterial); }); return out; };

  test("a vehicle glb is fetched once; it keeps its own colours whoever drives it, and darkens empty", async () => {
    const calls: string[] = [];
    const lib = new ModelLibrary(() => {}, (url) => { calls.push(url); return parse(files[url]!); });
    expect(lib.vehicle("warthog.glb")).toBeNull();
    await settle();
    const t = lib.vehicle("warthog.glb")!;
    expect(t.height).toBeCloseTo(0.9, 5);
    expect(t.root.getObjectByName("seat_driver")).toBeDefined();
    const plain = lib.instantiateVehicle(t, null), red = lib.instantiateVehicle(t, "#ff0000");
    const empty = lib.instantiateVehicle(t, null, true), empty2 = lib.instantiateVehicle(t, "#0000ff", true);
    const hullOf = (o: THREE.Object3D) => mats(o).find((m) => m.name === "hull")!;
    const base = hullOf(t.root);
    expect(hullOf(plain)).toBe(base);
    // Halo 2 does not repaint a vehicle for its driver's team.
    expect(hullOf(red)).toBe(base);
    expect(hullOf(empty)).not.toBe(base);
    expect(hullOf(empty).color.g).toBeCloseTo(base.color.g * EMPTY_VEHICLE_SHADE, 6);
    expect(hullOf(empty2)).toBe(hullOf(empty));
    expect(calls).toEqual(["warthog.glb"]);
    lib.dispose();
  });

  test("an AI wearing the spartan's model is not tinted, and its weights are not taken for colours", async () => {
    const lib = new ModelLibrary(() => {}, (url) => parse(files[url]!));
    lib.biped("spartan.glb");
    lib.biped("grunt.glb");
    await settle();
    const t = lib.biped("spartan.glb")!;
    const ai = lib.instantiateBiped(t, null), player = lib.instantiateBiped(t, "#0000ff");
    const armour = (o: THREE.Object3D) => mats(o).find((m) => m.name === "armour")!;
    expect(armour(ai.object).vertexColors).toBe(false);
    expect(armour(player.object).vertexColors).toBe(true);
    expect(armour(ai.object).color.getHex()).toBe(armour(t.root).color.getHex());
    expect(ai.head).toBeDefined();
    // A species with a model of its own, shorter than a spartan.
    expect(lib.biped("grunt.glb")!.height).toBeLessThan(t.height);
    lib.dispose();
  });

  test("the mock's manifest names its vehicles and AI bipeds, and serves them", () => {
    const m = mockModels();
    const table = parseModels(m.models)!;
    expect(Object.keys(table.vehicles)).toEqual(["warthog"]);
    expect(table.bipeds.grunt).toBeDefined();
    expect(m.file(table.vehicles.warthog!)).not.toBeNull();
    expect(m.file(table.bipeds.grunt!)).not.toBeNull();
  });
});

describe("the mock's campaign", () => {
  const map = MAPS.find((m) => m.waypoints.length > 10)!;
  const players = (): FramePlayer[] => [{ index: 0, alive: true, pos: [1, 2, 3], yaw: 0.5 }, { index: 1, alive: true, pos: [4, 5, 6], yaw: 0 }];
  const run = (hook: ReturnType<typeof campaignHook>, mode: string | undefined, ticks: number, id = "game1") => {
    const out: Frame[] = [];
    for (let tick = 2; tick <= ticks; tick += 2) {
      const f: Frame = { game_id: id, tick, players: players() };
      hook(f, { id, map, tick, mode });
      out.push(f);
    }
    return out;
  };

  test("a campaign game's frames carry about thirty actors and two vehicles, on the contract", () => {
    const frames = run(campaignHook(), "campaign", 600);
    const last = frames[frames.length - 1]!;
    expect(last.actors).toHaveLength(MOCK_ACTORS);
    expect(MOCK_ACTORS).toBeGreaterThanOrEqual(28);
    // Two in use, listed first, and three parked with nobody in them.
    expect(last.vehicles!.map((v) => [v.kind, v.occupied])).toEqual([["ghost", true], ["warthog", true], ["warthog", false], ["ghost", false], ["banshee", false]]);
    // Everything survives the client's own checks.
    expect(sampleActors([last], last.tick)).toHaveLength(MOCK_ACTORS);
    expect(sampleVehicles([last], last.tick).map((v) => v.occupied)).toEqual([true, true, false, false, false]);
    expect(new Set(last.actors!.map((a) => a.id)).size).toBe(MOCK_ACTORS);
    const species = new Set(last.actors!.map((a) => a.species));
    for (const s of ["grunt", "jackal", "elite", "hunter", "marine"]) expect(species.has(s)).toBe(true);
    expect(new Set(last.actors!.map((a) => a.team))).toEqual(new Set(["covenant", "human"]));
    // The first player drives one, with an AI aboard; an AI drives the other.
    const warthog = last.vehicles!.find((v) => v.kind === "warthog" && v.occupied)!, ghost = last.vehicles!.find((v) => v.kind === "ghost" && v.occupied)!;
    expect(last.players[0]!.vehicle).toBe(warthog.id);
    expect(warthog.pos).toEqual([1, 2, 3]);
    expect(last.players[1]!.vehicle).toBeUndefined();
    expect(last.actors!.filter((a) => a.vehicle === warthog.id)).toHaveLength(1);
    expect(last.actors!.filter((a) => a.vehicle === ghost.id)).toHaveLength(1);
    // They move, and some of them fire.
    const first = frames[0]!;
    const moved = last.actors!.filter((a, i) => Math.hypot(a.pos[0] - first.actors![i]!.pos[0], a.pos[1] - first.actors![i]!.pos[1]) > 1);
    expect(moved.length).toBeGreaterThan(MOCK_ACTORS / 2);
    expect(frames.some((f) => f.actors!.some((a) => a.shots))).toBe(true);
  });

  test("only campaign games, unless asked for everywhere; and the same game plays the same way", () => {
    expect(run(campaignHook(), "multiplayer", 60)[0]!.actors).toBeUndefined();
    expect(run(campaignHook(), undefined, 60)[0]!.vehicles).toBeUndefined();
    expect(run(campaignHook(() => true), undefined, 60)[0]!.actors).toHaveLength(MOCK_ACTORS);
    expect(run(campaignHook(), "campaign", 120)).toEqual(run(campaignHook(), "campaign", 120));
    expect(run(campaignHook(), "campaign", 120, "other")[5]!.actors![0]!.pos).not.toEqual(run(campaignHook(), "campaign", 120)[5]!.actors![0]!.pos);
  });

  test("the driver on foot while they are down", () => {
    const hook = campaignHook();
    const f: Frame = { game_id: "g", tick: 2, players: [{ index: 0, alive: false, pos: [0, 0, 0], yaw: 0 }] };
    hook(f, { id: "g", map, tick: 2, mode: "campaign" });
    expect(f.players[0]!.vehicle).toBeUndefined();
    expect(f.vehicles!.filter((v) => v.occupied).map((v) => v.kind)).toEqual(["ghost"]);
    // Having driven, the vehicle stays where it stopped, empty, while they are down.
    const up: Frame = { game_id: "g", tick: 3, players: [{ index: 0, alive: true, pos: [4, 5, 6], yaw: 0 }] };
    hook(up, { id: "g", map, tick: 3, mode: "campaign" });
    expect(up.players[0]!.vehicle).toBe("f2000");
    const down: Frame = { game_id: "g", tick: 4, players: [{ index: 0, alive: false, pos: [9, 9, 9], yaw: 0 }] };
    hook(down, { id: "g", map, tick: 4, mode: "campaign" });
    expect(down.vehicles!.find((v) => v.id === "f2000")).toMatchObject({ kind: "warthog", pos: [4, 5, 6], occupied: false });
  });

  test("the frames name the zone in play, and it changes", () => {
    const frames = run(campaignHook(), "campaign", ZONE_TICKS + 60);
    expect(frames[0]!.bsp).toBe(0);
    expect(frames[frames.length - 1]!.bsp).toBe(1);
    expect(sampleBsp(frames, frames[frames.length - 1]!.tick)).toBe(1);
  });
});
