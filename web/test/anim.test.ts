import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import type { Frame } from "../src/api/types";
import { FrameBuffer, sampleBsp, sampleFrames, sampleHits, sampleVehicles, type SampledHit, type SampledPlayer } from "../src/state/frames";
import { BipedAnim, ONE_SHOT_MS, RUN_SPEED, SpeedMeter, WALK_SPEED, actionShot, clipFor, locomotion, paceScale, weaponClass } from "../src/viewer/anim";
import { ActionWatch, EventFeed, GRENADE_MS, GrenadeQueue, HIT_FOLD_MS, HIT_LINE_MS, HIT_MARK_MS, HitQueue, KILL_MARK_MS, entityIndex, grenadeAt, grenadeTrail, hitColour, hitText, locator, mergeHits } from "../src/map/events";
import { bspNumber, isBackdrop, isCinematic, zoneVisibility } from "../src/viewer/zones";
import { parseModels } from "../src/viewer/models";
import { BipedAnimator, ModelLibrary, prepareBiped } from "../src/viewer/players3d";
import { smoothPlayers, type SmoothPlayer } from "../src/viewer/sync";
import { actorIndex } from "../src/state/frames";
import { actionHooks } from "../mock/action";
import { campaignHook } from "../mock/campaign";
import { buildArena, enclose, encodeGlb } from "../mock/arena";
import { MAPS } from "../mock/maps";
import { MOCK_BIPEDS, encodeModelGlb, mockModels } from "../mock/models";

const parse = (bytes: Uint8Array) => new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");
const player = (over: Partial<SampledPlayer> = {}): SampledPlayer => ({ index: 0, x: 0, y: 0, z: 0, yaw: 0, alive: true, vehicle: null, ...over });

describe("which clip a biped plays", () => {
  test("the looping state follows stance first, then speed", () => {
    expect(locomotion({ alive: true })).toBe("idle");
    expect(locomotion({ alive: true, stance: "stand", speed: WALK_SPEED - 0.01 })).toBe("idle");
    expect(locomotion({ alive: true, stance: "stand", speed: WALK_SPEED })).toBe("walk");
    expect(locomotion({ alive: true, stance: "stand", speed: RUN_SPEED })).toBe("run");
    expect(locomotion({ alive: true, speed: 3 })).toBe("run");
    expect(locomotion({ alive: true, stance: "crouch", speed: 3 })).toBe("crouch");
    expect(locomotion({ alive: true, stance: "airborne", speed: 0 })).toBe("airborne");
    expect(locomotion({ alive: true, stance: "vehicle", speed: 9 })).toBe("idle");
    expect(locomotion({ alive: true, stance: "dead" })).toBe("die");
    expect(locomotion({ alive: false, stance: "stand", speed: 3 })).toBe("die");
  });

  test("the weapon's class picks the clip set, with fallbacks down to anything that has the state", () => {
    const classes = { battle_rifle: "rifle", magnum: "pistol", energy_blade: "sword" };
    expect(weaponClass(classes, "magnum")).toBe("pistol");
    expect(weaponClass(classes, "never_heard_of_it")).toBe("rifle");
    expect(weaponClass(classes, null)).toBe("unarmed");
    expect(weaponClass(undefined, "magnum")).toBe("rifle");
    const clips = ["rifle:idle", "rifle:walk", "rifle:run", "rifle:fire", "pistol:idle", "pistol:fire", "sword:melee", "unarmed:idle"];
    expect(clipFor(clips, "pistol", "idle")).toBe("pistol:idle");
    // The pistol has no walk of its own: the rifle's.
    expect(clipFor(clips, "pistol", "walk")).toBe("rifle:walk");
    expect(clipFor(clips, "sword", "idle")).toBe("rifle:idle");
    // A state only one class has is better than none.
    expect(clipFor(clips, "rifle", "melee")).toBe("sword:melee");
    // A loop the glb lacks falls to a nearer one; a one-shot it lacks isn't played.
    expect(clipFor(clips, "rifle", "crouch")).toBe("rifle:idle");
    expect(clipFor(clips, "rifle", "airborne")).toBe("rifle:idle");
    expect(clipFor(["rifle:walk"], "rifle", "run")).toBe("rifle:walk");
    expect(clipFor(["rifle:run"], "rifle", "walk")).toBe("rifle:run");
    expect(clipFor(clips, "rifle", "reload")).toBeNull();
    expect(clipFor(clips, "rifle", "die")).toBeNull();
    expect(clipFor([], "rifle", "idle")).toBeNull();
  });

  test("walk and run clips keep pace with the ground, within limits", () => {
    expect(paceScale("idle", 5)).toBe(1);
    expect(paceScale("walk", null)).toBe(1);
    expect(paceScale("walk", 0.9)).toBeCloseTo(1, 6);
    expect(paceScale("run", 4.5)).toBe(2);
    expect(paceScale("run", 0.1)).toBe(0.5);
  });

  test("one-shots lie over the loop, the more important interrupting the lesser", () => {
    const a = new BipedAnim();
    expect(a.update(0, { alive: true, stance: "stand", speed: 2 })).toEqual({ loop: "run", shot: null, shotId: 0 });
    expect(a.trigger("fire", 100)).toBe(true);
    expect(a.update(110, { alive: true, speed: 2 })).toMatchObject({ loop: "run", shot: "fire", shotId: 1 });
    // A throw interrupts firing; firing doesn't interrupt the throw.
    expect(a.trigger("grenade", 120, 900)).toBe(true);
    expect(a.trigger("fire", 200)).toBe(false);
    expect(a.update(500, { alive: true, speed: 0 })).toMatchObject({ loop: "idle", shot: "grenade", shotId: 2 });
    // Once it has run its length the loop is all there is, and firing can start again.
    expect(a.update(1021, { alive: true, speed: 0 }).shot).toBeNull();
    expect(a.trigger("fire", 1030)).toBe(true);
    expect(a.update(1030 + ONE_SHOT_MS.fire, { alive: true }).shot).toBeNull();
  });

  test("coming down from the air lands; dying ends everything", () => {
    const a = new BipedAnim();
    a.update(0, { alive: true, stance: "airborne" });
    expect(a.update(600, { alive: true, stance: "stand", speed: 1 })).toMatchObject({ loop: "walk", shot: "land" });
    a.trigger("reload", 700);
    expect(a.update(800, { alive: false })).toMatchObject({ loop: "die", shot: null });
    // Dying in the air is not a landing.
    const b = new BipedAnim();
    b.update(0, { alive: true, stance: "airborne" });
    expect(b.update(100, { alive: true, stance: "dead" }).shot).toBeNull();
  });

  test("actions that have a clip, and those that don't", () => {
    expect(["grenade", "melee", "reload", "switch"].map((x) => actionShot(x as never))).toEqual(["grenade", "melee", "reload", "switch"]);
    expect(actionShot("jump")).toBeNull();
    expect(actionShot("pickup")).toBeNull();
  });

  test("a sender without speeds: it is worked out from how far they moved", () => {
    const m = new SpeedMeter();
    expect(m.update(0, 0, 16)).toBe(0);
    let v = 0;
    for (let i = 1; i <= 60; i++) v = m.update(i * 0.05, 0, 16.67); // 3 units a second
    expect(v).toBeGreaterThan(2.7);
    expect(v).toBeLessThan(3.3);
    // A respawn across the map is not a sprint.
    expect(m.update(500, 0, 16.67)).toBe(0);
  });
});

describe("stances, actions, hits and the zone in sampled frames", () => {
  const frame = (tick: number, over: Partial<Frame> = {}, p: Partial<Frame["players"][number]> = {}): Frame =>
    ({ game_id: "g", tick, players: [{ index: 0, alive: true, pos: [tick / 30, 0, 0], yaw: 0, ...p }], ...over });

  test("a player's stance, speed, actions and grenade come through, cleaned", () => {
    const frames = [
      frame(30, {}, { stance: "crouch", speed: 1, actions: ["grenade", "nonsense", "jump"], grenade: "plasma" }),
      frame(60, {}, { stance: "airborne", speed: 3 }),
    ];
    const at = sampleFrames(frames, 30)[0]!;
    expect(at).toMatchObject({ stance: "crouch", speed: 1, actions: ["grenade", "jump"], actionTick: 30, grenade: "plasma" });
    // Between frames the speed blends, the stance is the nearer frame's, and the actions are the frame behind's.
    const mid = sampleFrames(frames, 45)[0]!;
    expect(mid.speed).toBeCloseTo(2, 6);
    expect(sampleFrames(frames, 40)[0]!.stance).toBe("crouch");
    expect(sampleFrames(frames, 50)[0]!.stance).toBe("airborne");
    expect(mid.actionTick).toBe(30);
    expect(sampleFrames(frames, 50)[0]!.actions).toEqual(["grenade", "jump"]);
    // An old sender's frame has none of it; a bad stance is dropped.
    expect(sampleFrames([frame(30)], 30)[0]!.stance).toBeUndefined();
    expect(sampleFrames([frame(30, {}, { stance: "moonwalk" as never, speed: -4, actions: [] })], 30)[0]).not.toHaveProperty("actions");
    expect(sampleFrames([frame(30, {}, { stance: "moonwalk" as never })], 30)[0]!.stance).toBeUndefined();
    // At most eight actions a frame.
    expect(sampleFrames([frame(30, {}, { actions: Array(20).fill("melee") })], 30)[0]!.actions).toHaveLength(8);
  });

  test("hits are the frame behind's, capped and cleaned", () => {
    const hit = { from: "p0", to: "e1000", weapon: "smg", pos: [1, 2, 3] as [number, number, number], amount: 12, kind: "bullet" };
    const frames = [frame(30, { hits: [hit, { pos: [NaN, 0, 0] } as never, { to: "p1", pos: [4, 5, 6] }] }), frame(60)];
    expect(sampleHits(frames, 44)).toEqual({ tick: 30, hits: [
      { from: "p0", to: "e1000", weapon: "smg", x: 1, y: 2, z: 3, amount: 12, kind: "bullet" },
      { from: null, to: "p1", weapon: null, x: 4, y: 5, z: 6, amount: null, kind: null },
    ] });
    expect(sampleHits(frames, 60)).toBeNull();
    expect(sampleHits([frame(30)], 30)).toBeNull();
    expect(sampleHits([frame(30, { hits: Array(200).fill(hit) })], 30)!.hits).toHaveLength(64);
    const buf = new FrameBuffer();
    buf.push(frames[0]!, 0);
    expect(buf.sampleHits(1000)!.tick).toBe(30);
  });

  test("the zone, and every vehicle with whether anyone is in it", () => {
    const v = { id: "v1", kind: "warthog", pos: [0, 0, 0] as [number, number, number], forward: [1, 0, 0] as [number, number, number], up: [0, 0, 1] as [number, number, number] };
    const frames = [frame(30, { bsp: 2, vehicles: [{ ...v, occupied: true }, { ...v, id: "v2", occupied: false }, { ...v, id: "v3" }] }), frame(60, { bsp: 3 }), frame(90)];
    expect(sampleBsp(frames, 30)).toBe(2);
    expect(sampleBsp(frames, 59)).toBe(2);
    expect(sampleBsp(frames, 60)).toBe(3);
    expect(sampleBsp(frames, 95)).toBeNull();
    expect(sampleBsp([frame(30, { bsp: -1 })], 30)).toBeNull();
    // A sender from before `occupied` only sent vehicles in use.
    expect(sampleVehicles(frames, 30).map((x) => [x.id, x.occupied])).toEqual([["v1", true], ["v2", false], ["v3", true]]);
  });

  test("the 3D view's eased players carry the stance and speed", () => {
    const shown = new Map<number, SmoothPlayer>();
    smoothPlayers(shown, [player({ stance: "crouch", speed: 1.5 })], 16);
    expect(shown.get(0)).toMatchObject({ stance: "crouch", speed: 1.5 });
    smoothPlayers(shown, [player()], 16);
    expect(shown.get(0)).toMatchObject({ stance: null, speed: null });
  });
});

describe("actions and grenades as a view sees them", () => {
  test("each frame's actions are reported once, and a seek notes them silently", () => {
    const w = new ActionWatch();
    const p = player({ actions: ["grenade", "melee"], actionTick: 30 });
    expect(w.observe([p]).map((e) => [e.index, e.action])).toEqual([[0, "grenade"], [0, "melee"]]);
    expect(w.observe([p])).toEqual([]);
    expect(w.observe([player({ actions: ["reload"], actionTick: 60 })], { quiet: true })).toEqual([]);
    expect(w.observe([player({ actions: ["reload"], actionTick: 60 })])).toEqual([]);
    expect(w.observe([player({ actions: ["reload"], actionTick: 90 })])).toHaveLength(1);
    expect(w.observe([player()])).toEqual([]);
  });

  test("a grenade flies an arc from the hand along the aim and comes to rest", () => {
    const g = { x: 10, y: 5, z: 2, aim: [1, 0, 0] as [number, number, number] };
    const start = grenadeAt(g, 0);
    expect(start[0]).toBe(10);
    expect(start[2]).toBeGreaterThan(2.3);
    const mid = grenadeAt(g, 250), end = grenadeAt(g, GRENADE_MS), after = grenadeAt(g, GRENADE_MS * 3);
    // Up and out, then down to the thrower's footing, and it stays there.
    expect(mid[2]).toBeGreaterThan(start[2]);
    expect(mid[0]).toBeGreaterThan(11);
    expect(end[2]).toBeCloseTo(2, 5);
    expect(after).toEqual(end);
    expect(end[1]).toBe(5);
    // Thrown upwards it goes higher and lands later.
    expect(grenadeAt({ ...g, aim: [0.7, 0, 0.7] }, 400)[2]).toBeGreaterThan(grenadeAt(g, 400)[2]);
    const trail = grenadeTrail(g, 600, 7);
    expect(trail).toHaveLength(7);
    expect(trail[0]).toEqual(start);
    expect(trail[6]).toEqual(grenadeAt(g, 600));
  });

  test("the queue holds a throw for its flight, in the thrower's grenade type", () => {
    const q = new GrenadeQueue();
    q.add(player({ x: 1, aim: [0, 1, 0], grenade: "plasma" }), 1000);
    q.add(player({ index: 3, yaw: Math.PI / 2 }), 1100);
    const live = q.live(1200);
    expect(live.map((g) => [g.index, g.type, g.age])).toEqual([[0, "plasma", 200], [3, null, 100]]);
    // No aim in the frame: level along the yaw.
    expect(live[1]!.aim[1]).toBeCloseTo(1, 6);
    expect(live[0]!.alpha).toBe(1);
    expect(q.live(1000 + GRENADE_MS - 50)[0]!.alpha).toBeLessThan(0.3);
    expect(q.live(1100 + GRENADE_MS)).toEqual([]);
    expect(q.busy).toBe(false);
  });
});

describe("damage as a view draws it", () => {
  const hit = (over: Partial<SampledHit> = {}): SampledHit => ({ from: "p0", to: "p1", weapon: "smg", x: 5, y: 0, z: 0, amount: 10, kind: "bullet", ...over });
  const people = (victimAlive = true) => locator(
    [player({ index: 0, x: 0 }), player({ index: 1, x: 5, alive: victimAlive })],
    [player({ index: actorIndex("e1000"), x: 9, actor: { id: "e1000", team: "covenant" } })],
    [{ id: "f2000", kind: "ghost", x: 20, y: 0, z: 0, forward: [1, 0, 0], up: [0, 0, 1], occupied: true }],
  );

  test("who a hit names: a player, an actor or a vehicle", () => {
    expect(entityIndex("p3")).toBe(3);
    expect(entityIndex("e1000")).toBe(actorIndex("e1000"));
    const where = people();
    expect(where("p1")).toEqual({ x: 5, y: 0, z: 0, alive: true });
    expect(where("e1000")!.x).toBe(9);
    expect(where("f2000")!.x).toBe(20);
    expect(where("p7")).toBeNull();
    expect(where("nobody")).toBeNull();
    expect(locator([player({ index: 1, stance: "dead" })], undefined, undefined)("p1")!.alive).toBe(false);
  });

  test("a frame's pellets on one victim are one hit, their damage added up", () => {
    const merged = mergeHits([hit({ amount: 4 }), hit({ amount: 5 }), hit({ to: "e1000", amount: 1 }), hit({ amount: null }), hit({ kind: "melee", amount: 70 }), hit({ to: null }), hit({ to: null })]);
    expect(merged.map((h) => [h.to, h.kind, h.amount])).toEqual([["p1", "bullet", 9], ["e1000", "bullet", 1], ["p1", "melee", 70], [null, "bullet", 10], [null, "bullet", 10]]);
  });

  test("a hit draws a line from attacker to victim that fades fast, and a marker with the number that lingers", () => {
    const q = new HitQueue(), where = people();
    q.observe({ tick: 30, hits: [hit()] }, 1000, where);
    // The same frame sampled again adds nothing.
    q.observe({ tick: 30, hits: [hit()] }, 1016, where);
    let live = q.live(1000, where);
    expect(live).toHaveLength(1);
    expect(live[0]).toMatchObject({ fromId: "p0", toId: "p1", from: { x: 0, y: 0, z: 0 }, x: 5, kill: false, line: 1, mark: 1, rise: 0, text: "10" });
    live = q.live(1000 + HIT_LINE_MS / 2, where);
    expect(live[0]!.line).toBeCloseTo(0.25, 6);
    live = q.live(1000 + HIT_LINE_MS, where);
    expect(live[0]!.line).toBe(0);
    expect(live[0]!.mark).toBe(1);
    expect(q.live(1000 + HIT_MARK_MS - 20, where)[0]!.mark).toBeLessThan(0.1);
    expect(q.live(1000 + HIT_MARK_MS, where)).toEqual([]);
    expect(q.busy).toBe(false);
  });

  test("sustained fire on one victim is one marker with a rising number; a new one after a pause", () => {
    const q = new HitQueue(), where = people();
    q.observe({ tick: 30, hits: [hit({ amount: 6 })] }, 1000, where);
    q.observe({ tick: 31, hits: [hit({ amount: 7, x: 6 })] }, 1033, where);
    q.observe({ tick: 32, hits: [hit({ amount: 5, to: "e1000" })] }, 1066, where);
    expect(q.live(1070, where).map((h) => [h.toId, h.text, h.x])).toEqual([["p1", "13", 6], ["e1000", "5", 5]]);
    q.observe({ tick: 50, hits: [hit({ amount: 1.26 })] }, 1000 + HIT_FOLD_MS + 1, where);
    expect(q.live(1000 + HIT_FOLD_MS + 2, where).map((h) => h.text)).toEqual(["13", "5", "1.3"]);
  });

  test("a kill is marked as one, when the hit lands or when the victim is found dead after, and stays longer", () => {
    const q = new HitQueue();
    q.observe({ tick: 30, hits: [hit()] }, 1000, people(false));
    expect(q.live(1001)[0]!.kill).toBe(true);
    expect(q.live(1000 + HIT_MARK_MS + 100)).toHaveLength(1);
    expect(q.live(1000 + KILL_MARK_MS)).toEqual([]);
    // Alive when hit, down a moment later.
    q.observe({ tick: 60, hits: [hit()] }, 5000, people());
    expect(q.live(5100, people())[0]!.kill).toBe(false);
    expect(q.live(5300, people(false))[0]!.kill).toBe(true);
    expect(hitColour({ kill: true, kind: "bullet" })).not.toBe(hitColour({ kill: false, kind: "bullet" }));
    // Three people hit them as they went down: one kill, the last hit's.
    const three = new HitQueue();
    three.observe({ tick: 30, hits: [hit({ from: "p0" }), hit({ from: "p2" })] }, 1000, people());
    three.observe({ tick: 31, hits: [hit({ from: "p3" })] }, 1033, people(false));
    expect(three.live(1040, people(false)).map((h) => [h.fromId, h.kill])).toEqual([["p0", false], ["p2", false], ["p3", true]]);
    // Hits on the body afterwards aren't more kills.
    three.observe({ tick: 32, hits: [hit({ from: "p0", kind: "explosion" })] }, 1066, people(false));
    expect(three.live(1070, people(false)).filter((h) => h.kill)).toHaveLength(1);
  });

  test("hits with no attacker, one the frame doesn't have, or the victim's own, draw no line; a seek draws nothing", () => {
    const q = new HitQueue(), where = people();
    q.observe({ tick: 30, hits: [hit({ from: null, kind: "fall" }), hit({ from: "p9", to: "e1000" }), hit({ from: "p1", to: "p1", kind: "explosion" })] }, 1000, where);
    expect(q.live(1001, where).map((h) => h.from)).toEqual([null, null, null]);
    q.observe({ tick: 60, hits: [hit()] }, 2000, where, { quiet: true });
    expect(q.live(2001, where)).toEqual([]);
    q.observe({ tick: 60, hits: [hit()] }, 2016, where);
    expect(q.live(2017, where)).toEqual([]);
    q.observe(null, 2100, where);
    expect(hitText(null)).toBe("");
    expect(hitText(0.54)).toBe("0.5");
    expect(hitText(45.6)).toBe("46");
  });

  test("one feed for a view: actions throw grenades, hits find their people in the frame", () => {
    const feed = new EventFeed();
    const thrower = player({ index: 0, actions: ["grenade", "reload"], actionTick: 30, grenade: "frag" });
    const f = { players: [thrower, player({ index: 1, x: 5 })], hits: { tick: 30, hits: [hit()] } };
    expect(feed.observe(f, 1000).map((e) => e.action)).toEqual(["grenade", "reload"]);
    expect(feed.observe(f, 1016)).toEqual([]);
    expect(feed.liveGrenades(1100).map((g) => g.type)).toEqual(["frag"]);
    expect(feed.liveHits(1100)[0]).toMatchObject({ from: { x: 0 }, x: 5 });
    expect(feed.busy).toBe(true);
    // The dead throw nothing.
    const other = new EventFeed();
    other.observe({ players: [player({ alive: false, actions: ["grenade"], actionTick: 30 })] }, 0);
    expect(other.liveGrenades(10)).toEqual([]);
  });
});

describe("a level's zones", () => {
  test("names: zones, backdrop, anything else", () => {
    expect(bspNumber("bsp_0")).toBe(0);
    expect(bspNumber("bsp_12")).toBe(12);
    expect(bspNumber("structure")).toBeNull();
    expect(bspNumber("bsp_x")).toBeNull();
    expect(isBackdrop("backdrop")).toBe(true);
    expect(isBackdrop("skybox")).toBe(true);
    expect(isBackdrop("bsp_0")).toBe(false);
    expect(isCinematic({ cinematic: true })).toBe(true);
    expect(isCinematic({ cinematic_only: true })).toBe(true);
    expect(isCinematic({})).toBe(false);
    expect(isCinematic(undefined, "bsp_2", { cinematic_bsps: [2, 5] })).toBe(true);
    expect(isCinematic(undefined, "bsp_1", { cinematic_bsps: [2, 5] })).toBe(false);
  });

  const nodes = [
    { name: "bsp_0", cinematic: false }, { name: "bsp_1", cinematic: false }, { name: "bsp_2", cinematic: true },
    { name: "machines", cinematic: false }, { name: "backdrop", cinematic: false },
  ];

  test("only the zone the frames name is shown; machines always; the backdrop when asked", () => {
    expect(zoneVisibility(nodes, 1)).toEqual([false, true, false, true, false]);
    expect(zoneVisibility(nodes, 0, { backdrop: true })).toEqual([true, false, false, true, true]);
    // The game can say a cutscene's zone is loaded: then that is what there is to see.
    expect(zoneVisibility(nodes, 2)).toEqual([false, false, true, true, false]);
  });

  test("no zone named, or one the glb hasn't got: every playable zone, never the cutscene ones", () => {
    expect(zoneVisibility(nodes, null)).toEqual([true, true, false, true, false]);
    expect(zoneVisibility(nodes, undefined)).toEqual([true, true, false, true, false]);
    expect(zoneVisibility(nodes, 9)).toEqual([true, true, false, true, false]);
  });

  test("never an empty level: zones all marked cinematic are all shown; one structure is left alone", () => {
    expect(zoneVisibility([{ name: "bsp_0", cinematic: true }, { name: "bsp_1", cinematic: true }], null)).toEqual([true, true]);
    expect(zoneVisibility([{ name: "structure", cinematic: false }, { name: "machines", cinematic: false }], 3)).toEqual([true, true]);
    expect(zoneVisibility([], 0)).toEqual([]);
  });

  test("the mock's indoor level: two zones, a cutscene one marked in its extras, and a backdrop", async () => {
    const g = await parse(encodeGlb(enclose(buildArena("orchard")!)));
    const found: Array<{ name: string; cinematic: boolean }> = [];
    g.scene.traverse((o) => { if (bspNumber(o.name) !== null || isBackdrop(o.name)) found.push({ name: o.name, cinematic: isCinematic(o.userData, o.name) }); });
    expect(found.sort((a, b) => a.name.localeCompare(b.name))).toEqual([
      { name: "backdrop", cinematic: false }, { name: "bsp_0", cinematic: false }, { name: "bsp_1", cinematic: false }, { name: "bsp_2", cinematic: true },
    ]);
  });
});

describe("skinned models and their clips", () => {
  test("the manifest's version 2: clips per biped and a class per weapon; version 1 still reads", () => {
    const v2 = parseModels(mockModels().models)!;
    expect(v2.classes).toEqual({ battle_rifle: "rifle", smg: "pistol", sniper_rifle: "rifle", plasma_rifle: "pistol" });
    expect(v2.bipeds.spartan).toBe("models/biped/spartan.glb");
    const v1 = parseModels(mockModels(false).models)!;
    expect(v1.classes).toEqual({});
    expect(v1.bipeds).toEqual(v2.bipeds);
    expect(mockModels().models.version).toBe(2);
    expect(mockModels().models.bipeds.spartan!.clips).toContain("rifle:grenade");
    expect(mockModels(false).models.bipeds.spartan!.clips).toBeUndefined();
  });

  test("the mock's biped is skinned, with a clip for every state", async () => {
    const g = await parse(encodeModelGlb(MOCK_BIPEDS.spartan!()));
    const t = prepareBiped(g)!;
    expect(t.skinned).toBe(true);
    for (const state of ["idle", "walk", "run", "crouch", "airborne", "land", "grenade", "melee", "reload", "fire", "switch", "die"]) expect(t.clips.has("rifle:" + state)).toBe(true);
    expect(t.clips.has("pistol:idle")).toBe(true);
    expect(t.clips.get("rifle:fire")!.duration).toBeCloseTo(0.18, 5);
    // The static kind has neither, and reads as before.
    const flat = prepareBiped(await parse(encodeModelGlb(MOCK_BIPEDS.spartan!(false))))!;
    expect(flat.skinned).toBe(false);
    expect(flat.clips.size).toBe(0);
    expect(flat.height).toBe(t.height);
  });

  const instance = async (rigged = true) => {
    const bytes = encodeModelGlb(MOCK_BIPEDS.spartan!(rigged));
    const lib = new ModelLibrary(() => {}, () => parse(bytes));
    lib.biped("a");
    await new Promise((r) => setTimeout(r, 20));
    const t = lib.biped("a")!;
    return { lib, t, a: lib.instantiateBiped(t, "#3a7bd5"), b: lib.instantiateBiped(t, "#d53a3a") };
  };
  const handAt = (o: { object: THREE.Object3D; hand: THREE.Object3D }) => { o.object.updateWorldMatrix(true, true); return o.hand.getWorldPosition(new THREE.Vector3()); };

  test("each copy has its own bones and animator; a static model has none", async () => {
    const { a, b } = await instance();
    expect(a.animator).toBeInstanceOf(BipedAnimator);
    expect(a.animator).not.toBe(b.animator);
    const bones = (o: THREE.Object3D) => { let s: THREE.Skeleton | null = null; o.traverse((c) => { if ((c as THREE.SkinnedMesh).isSkinnedMesh) s = (c as THREE.SkinnedMesh).skeleton; }); return s!; };
    expect(bones(a.object)).not.toBe(bones(b.object));
    expect(bones(a.object).bones[0]).not.toBe(bones(b.object).bones[0]);
    // The markers ride on this copy's bones.
    let under = false;
    a.hand.traverseAncestors((p) => { if (bones(a.object).bones.includes(p as THREE.Bone)) under = true; });
    expect(under).toBe(true);
    expect((await instance(false)).a.animator).toBeNull();
  });

  test("a looping clip moves this copy only; a one-shot plays over it and ends", async () => {
    const { a, b } = await instance();
    const an = a.animator!, rest = handAt(a).clone();
    expect(an.duration("rifle:switch")).toBeCloseTo(600, 3);
    expect(an.duration("rifle:nothing")).toBeNull();
    an.setLoop("rifle:crouch");
    an.update(500);
    // Crouching, the hand is lower; the other copy hasn't moved.
    const low = handAt(a).clone();
    expect(low.y).toBeLessThan(rest.y - 0.05);
    expect(handAt(b).y).toBeCloseTo(rest.y, 6);
    expect(an.looping).toBe("rifle:crouch");
    // The swap drops the weapon hand further while it plays, then it is back.
    expect(an.playOnce("rifle:switch")).toBe(true);
    an.update(250);
    const swapping = handAt(a).clone();
    expect(swapping.distanceTo(low)).toBeGreaterThan(0.05);
    for (let i = 0; i < 30; i++) an.update(33);
    expect(handAt(a).distanceTo(low)).toBeLessThan(0.02);
    expect(an.playOnce("rifle:nothing")).toBe(false);
    expect(an.playOnce(null)).toBe(false);
  });

  test("changing the loop cross-fades; dying holds its last frame", async () => {
    const { a } = await instance();
    const an = a.animator!;
    an.setLoop("rifle:idle");
    an.update(100);
    const standing = handAt(a).clone();
    an.setLoop("rifle:crouch");
    an.update(70);
    const part = handAt(a).clone();
    for (let i = 0; i < 10; i++) an.update(33);
    const full = handAt(a).clone();
    // Half way through the fade it is between the two.
    expect(part.y).toBeLessThan(standing.y - 0.005);
    expect(part.y).toBeGreaterThan(full.y + 0.005);
    an.setLoop("rifle:die", 1, true);
    for (let i = 0; i < 80; i++) an.update(33);
    const down = handAt(a).clone();
    for (let i = 0; i < 40; i++) an.update(33);
    expect(handAt(a).distanceTo(down)).toBeLessThan(1e-6);
    expect(down.y).toBeLessThan(full.y);
    expect(BipedAnimator.envelope(0, 1)).toBe(0);
    expect(BipedAnimator.envelope(0.5, 1)).toBe(1);
    expect(BipedAnimator.envelope(1, 1)).toBe(0);
    an.dispose();
  });
});

describe("the mock's action", () => {
  const map = MAPS.find((m) => m.waypoints.length > 4)!;
  const run = (ticks: number, withActors = false) => {
    const hooks = actionHooks(), campaign = campaignHook(() => true);
    const live: Frame[] = [], kept: Frame[] = [];
    for (let tick = 1; tick <= ticks; tick++) {
      const f: Frame = { game_id: "g1", tick, players: [0, 1, 2, 3].map((index) => ({
        index, alive: !(index === 3 && tick > 300 && tick < 400), pos: [index * 1.2 + tick * 0.05, index, 0] as [number, number, number], yaw: 0,
        weapon: tick > 500 && index === 0 ? "smg" : "battle_rifle", ...(tick % 40 < 6 ? { shots: 2 } : {}),
      })) };
      const game = { id: "g1", map, tick, mode: "multiplayer" };
      if (withActors) campaign(f, game);
      hooks.frame(f, game);
      live.push(f);
      if (tick % 30 === 0) { const k = { ...f, players: f.players.map((p) => ({ ...p })) }; hooks.record(k, game); kept.push(k); }
    }
    return { live, kept };
  };

  test("every player has a stance and a speed; they jump, crouch, throw, swing, reload and swap", () => {
    const { live } = run(3000);
    const all = live.flatMap((f) => f.players);
    expect(all.every((p) => typeof p.speed === "number" && typeof p.stance === "string")).toBe(true);
    expect(new Set(all.map((p) => p.stance))).toEqual(new Set(["stand", "crouch", "airborne", "dead"]));
    // 0.05 a tick is 1.5 units a second.
    expect(live[100]!.players[0]!.speed).toBeCloseTo(1.5, 1);
    const actions = new Set(all.flatMap((p) => p.actions ?? []));
    for (const a of ["jump", "grenade", "melee", "reload", "switch"]) expect(actions.has(a)).toBe(true);
    // A jump leaves the ground for a moment, and says so.
    const jump = live.findIndex((f) => f.players[1]!.actions?.includes("jump"));
    const air = live.slice(jump, jump + 20).map((f) => f.players[1]!);
    expect(air.every((p) => p.stance === "airborne")).toBe(true);
    expect(Math.max(...air.map((p) => p.pos[2]))).toBeGreaterThan(0.6);
    expect(live[jump + 21]!.players[1]!.stance).not.toBe("airborne");
    expect(live[jump + 21]!.players[1]!.pos[2]).toBe(0);
    expect(new Set(all.map((p) => p.grenade))).toEqual(new Set(["frag", "plasma"]));
    // The same game plays the same way.
    expect(run(300).live).toEqual(run(300).live);
  });

  test("firing turns on someone near and some of it lands; a death comes with its hit; all on the contract", () => {
    const { live } = run(3000);
    const hits = live.flatMap((f) => f.hits ?? []);
    expect(hits.length).toBeGreaterThan(20);
    for (const kind of ["bullet", "melee", "explosion"]) expect(hits.some((h) => h.kind === kind)).toBe(true);
    expect(live.every((f) => (f.hits?.length ?? 0) <= 64)).toBe(true);
    const shot = live.find((f) => f.hits?.some((h) => h.kind === "bullet" && h.from === "p0"))!;
    const h = shot.hits!.find((x) => x.kind === "bullet" && x.from === "p0")!, shooter = shot.players[0]!, victim = shot.players[Number(h.to!.slice(1))]!;
    // The shooter is looking at who they hit.
    const d = Math.hypot(victim.pos[0] - shooter.pos[0], victim.pos[1] - shooter.pos[1], victim.pos[2] - shooter.pos[2]);
    expect(shooter.aim![0]).toBeCloseTo((victim.pos[0] - shooter.pos[0]) / d, 2);
    expect(h.pos).toEqual(victim.pos);
    expect(h.amount).toBeGreaterThan(0);
    const death = live[300]!;
    expect(death.players[3]!.alive).toBe(false);
    expect(death.hits!.some((x) => x.to === "p3")).toBe(true);
    // The client reads every one of them.
    for (const f of live) if (f.hits) expect(sampleHits([f], f.tick)!.hits).toHaveLength(f.hits.length);
  });

  test("the frame recorded each second carries the second's actions and hits", () => {
    const { live, kept } = run(900);
    for (const k of kept) {
      const second = live.filter((f) => f.tick > k.tick - 30 && f.tick <= k.tick);
      for (const p of k.players) expect(p.actions ?? []).toEqual(second.flatMap((f) => f.players[p.index]!.actions ?? []).slice(0, 8));
      expect(k.hits ?? []).toEqual(second.flatMap((f) => f.hits ?? []).slice(-64));
    }
    expect(kept.some((k) => k.hits?.length)).toBe(true);
    expect(kept.some((k) => k.players.some((p) => p.actions?.length))).toBe(true);
    // The live frame of that tick is left as it was.
    expect(live[29]!.players).not.toBe(kept[0]!.players);
  });

  test("the AI gets stances too, and is hit and hits back", () => {
    const { live } = run(1500, true);
    const actors = live.flatMap((f) => f.actors ?? []);
    expect(actors.every((a) => typeof a.speed === "number" && !!a.stance)).toBe(true);
    expect(actors.some((a) => a.stance === "vehicle")).toBe(true);
    const hits = live.flatMap((f) => f.hits ?? []);
    expect(hits.some((h) => h.to && !h.to.startsWith("p"))).toBe(true);
    expect(hits.some((h) => h.from && !h.from.startsWith("p") && h.to?.startsWith("p"))).toBe(true);
    // Players in a vehicle are in the vehicle stance.
    expect(live[100]!.players[0]!.stance).toBe("vehicle");
  });
});
