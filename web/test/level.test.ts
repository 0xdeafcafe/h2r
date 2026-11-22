import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { skipsOcclusion } from "../src/viewer/aopass";
import { SEE_MAX, nextSee, seeTargets, seeThroughHidesAny, type SeeThrough } from "../src/viewer/cutaway";
import { dressLayer, layerLook, layerOf } from "../src/viewer/layers3d";
import { LevelLook } from "../src/viewer/level3d";
import { FOG_CLEAR, fogAmount, gameFog, skyColour, skyOf, skyUrl, useGameFog } from "../src/viewer/sky3d";
import { ZONE_DWELL_MS, ZoneFollower, isBackdrop, layerNode, zoneInPlay, zoneLabel, zoneList, zoneVisibility, zoneWanted, type ZoneBox } from "../src/viewer/zones";

describe("picking a level's zone", () => {
  test("zones are named for their BSP, or numbered from 1 when the name says nothing", () => {
    expect(zoneLabel(0, "earthcity_2")).toBe("Earthcity 2");
    expect(zoneLabel(3, "alpha-gasgiant.bsp")).toBe("Alpha gasgiant bsp");
    expect(zoneLabel(0, "bsp_01")).toBe("Zone 1");
    expect(zoneLabel(4, "")).toBe("Zone 5");
    expect(zoneLabel(1)).toBe("Zone 2");
  });

  test("the picker lists playable zones in order, cutscene ones only when asked, each once", () => {
    const nodes = [
      { name: "bsp_2", cinematic: false, label: "harbour" }, { name: "bsp_0", cinematic: false }, { name: "bsp_1", cinematic: true, label: "intro" },
      { name: "bsp_0", cinematic: false }, { name: "machines", cinematic: false }, { name: "decals_0", cinematic: false },
    ];
    expect(zoneList(nodes)).toEqual([{ bsp: 0, label: "Zone 1", cinematic: false }, { bsp: 2, label: "Harbour", cinematic: false }]);
    expect(zoneList(nodes, { cinematic: true }).map((z) => z.bsp)).toEqual([0, 1, 2]);
  });

  const nodes = [{ name: "bsp_0", cinematic: false }, { name: "bsp_1", cinematic: false }, { name: "bsp_2", cinematic: true }, { name: "bsp_1_backdrop", cinematic: false }];

  test("a picked zone is shown alone whatever the frames say; all shows every one", () => {
    expect(zoneVisibility(nodes, 0, { pick: 1 })).toEqual([false, true, false, false]);
    expect(zoneVisibility(nodes, 0, { pick: 2 })).toEqual([false, false, true, false]);
    // A zone the level hasn't got: as auto.
    expect(zoneVisibility(nodes, 0, { pick: 7 })).toEqual(zoneVisibility(nodes, null));
    expect(zoneVisibility(nodes, 0, { pick: "all" })).toEqual([true, true, false, false]);
    expect(zoneVisibility(nodes, 0, { pick: "all", cinematic: true, backdrop: true })).toEqual([true, true, true, true]);
  });

  test("the zone in play follows the frames", () => {
    expect(zoneVisibility(nodes, 1, { pick: "auto" })).toEqual([false, true, false, false]);
    const zones = zoneList(nodes);
    expect(zoneInPlay({ active: 1, zones })).toBe("Zone in play: Zone 2");
    expect(zoneInPlay({ active: null, zones })).toBe("Zone in play");
    expect(zoneInPlay(null)).toBe("Zone in play");
  });

  test("earlier extractions' backdrop names, and the layer nodes", () => {
    expect(isBackdrop("bsp_1_backdrop")).toBe(true);
    expect(isBackdrop("backdrop_3")).toBe(true);
    expect(isBackdrop("bsp_1")).toBe(false);
    expect(layerNode("decals_0")).toBe("decals");
    expect(layerNode("effects")).toBe("effects");
    expect(layerNode("decal_0")).toBeNull();
    expect(layerNode("bsp_0")).toBeNull();
  });
});

describe("a level's layers", () => {
  test("read from material extras; anything else is not a layer", () => {
    expect(layerOf({ layer: "decal", blend: "double" })).toEqual({ group: "decal", blend: "double" });
    expect(layerOf({ layer: "effect", blend: "additive" })).toEqual({ group: "effect", blend: "additive" });
    expect(layerOf({ layer: "effect", blend: "sideways" })).toEqual({ group: "effect", blend: "alpha" });
    expect(layerOf({ layer: "floor" })).toBeNull();
    expect(layerOf(undefined)).toBeNull();
  });

  test("decals sit on their surface; glows are light, unlit and unfogged; only cut-outs write depth", () => {
    const stain = layerLook({ group: "decal", blend: "double" });
    expect(stain).toMatchObject({ unlit: true, blending: "double", polygonOffset: true, doubleSided: false, depthWrite: false, fog: false });
    const sign = layerLook({ group: "decal", blend: "alpha" });
    expect(sign).toMatchObject({ unlit: false, blending: "normal", transparent: true, fog: true });
    const grate = layerLook({ group: "decal", blend: "test" });
    expect(grate).toMatchObject({ transparent: false, depthWrite: true, alphaTest: 0.5 });
    const glow = layerLook({ group: "effect", blend: "additive" });
    expect(glow).toMatchObject({ unlit: true, blending: "add", doubleSided: true, depthWrite: false, fog: false, polygonOffset: false });
    expect(glow.renderOrder).toBeGreaterThan(stain.renderOrder);
  });

  test("a loaded layer mesh gets its blend, keeps its picture, and is left out of shadows and occlusion", () => {
    const map = new THREE.Texture();
    const src = new THREE.MeshStandardMaterial({ map });
    src.userData = { layer: "decal", blend: "double" };
    const mesh = new THREE.Mesh(new THREE.BufferGeometry(), src);
    expect(dressLayer(mesh)).toEqual({ group: "decal", blend: "double" });
    const m = mesh.material as unknown as THREE.MeshBasicMaterial;
    expect(m).toBeInstanceOf(THREE.MeshBasicMaterial);
    expect(m.map).toBe(map);
    expect(m.blending).toBe(THREE.CustomBlending);
    expect([m.blendSrc, m.blendDst]).toEqual([THREE.DstColorFactor, THREE.SrcColorFactor]);
    expect(m.polygonOffset && m.polygonOffsetFactor < 0).toBe(true);
    expect(m.toneMapped).toBe(false);
    expect(mesh.castShadow).toBe(false);
    expect(skipsOcclusion(mesh)).toBe(true);

    const glow = new THREE.Mesh(new THREE.BufferGeometry(), Object.assign(new THREE.MeshStandardMaterial(), { userData: { layer: "effect", blend: "additive" } }));
    dressLayer(glow);
    const g = glow.material as unknown as THREE.MeshBasicMaterial;
    expect([g.blending, g.blendSrc, g.blendDst]).toEqual([THREE.CustomBlending, THREE.SrcAlphaFactor, THREE.OneFactor]);
    expect(g.side).toBe(THREE.DoubleSide);

    const wall = new THREE.Mesh(new THREE.BufferGeometry(), new THREE.MeshStandardMaterial());
    expect(dressLayer(wall)).toBeNull();
    expect(skipsOcclusion(wall)).toBe(false);
  });

  test("the occlusion pass leaves out what is see-through and writes no depth", () => {
    const tracer = new THREE.Mesh(new THREE.BufferGeometry(), new THREE.MeshBasicMaterial({ transparent: true, depthWrite: false }));
    expect(skipsOcclusion(tracer)).toBe(true);
    const glass = new THREE.Mesh(new THREE.BufferGeometry(), new THREE.MeshBasicMaterial({ transparent: true }));
    expect(skipsOcclusion(glass)).toBe(false);
    const marked = new THREE.Group();
    marked.userData.noAo = true;
    expect(skipsOcclusion(marked)).toBe(true);
    expect(skipsOcclusion(new THREE.Group())).toBe(false);
  });
});

describe("a level's sky and fog", () => {
  const extras = { sky: { name: "lockout", glb: "sky.glb", clear: [0.1, 0.2, 0.3], fog: { colour: [0.5, 0.6, 0.8], density: 0.5, start: 0, end: 40 }, scale: 1 } };

  test("read from the map glb's asset extras, and nothing odd let through", () => {
    expect(skyOf(extras)).toEqual({ name: "lockout", glb: "sky.glb", clear: [0.1, 0.2, 0.3], fog: { colour: [0.5, 0.6, 0.8], density: 0.5, start: 0, end: 40 } });
    expect(skyOf({})).toBeNull();
    expect(skyOf(null)).toBeNull();
    expect(skyOf({ sky: { name: "x", glb: "../../etc/sky.glb" } })!.glb).toBeUndefined();
    expect(skyOf({ sky: { name: "x", glb: "https://elsewhere/sky.glb" } })!.glb).toBeUndefined();
    expect(skyOf({ sky: { name: "x", clear: [1, 2] } })!.clear).toBeUndefined();
    // A fog that ends before it starts, or has no density, is no fog.
    expect(skyOf({ sky: { name: "x", fog: { colour: [1, 1, 1], density: 0.5, start: 10, end: 5 } } })!.fog).toBeUndefined();
    expect(skyOf({ sky: { name: "x", fog: { colour: [1, 1, 1], density: 0, start: 0, end: 5 } } })!.fog).toBeUndefined();
  });

  test("behind the sky: its clear colour, else its fog's, else the view's own", () => {
    expect(skyColour(skyOf(extras))).toEqual([0.1, 0.2, 0.3]);
    expect(skyColour({ name: "x", fog: { colour: [0.5, 0.5, 0.5], density: 1, start: 0, end: 1 } })).toEqual([0.5, 0.5, 0.5]);
    expect(skyColour({ name: "x" })).toBeNull();
    expect(skyColour(null)).toBeNull();
  });

  test("the sky model sits beside the map glb", () => {
    expect(skyUrl("/art/map/lockout/map.glb?v=10", "sky.glb")).toBe("/art/map/lockout/sky.glb?v=10");
    expect(skyUrl("map.glb", "sky.glb")).toBe("sky.glb");
  });

  test("the fog is measured from the point in focus, linear to the sky's density and no thicker", () => {
    const fog = { colour: [1, 1, 1] as [number, number, number], density: 0.5, start: 20, end: 60 };
    const f = gameFog(fog, 10);
    expect(f).toEqual({ near: 30, far: 70, cap: 0.5 });
    // Clear up to start past the focus.
    expect(fogAmount(0, f)).toBe(0);
    expect(fogAmount(30, f)).toBe(0);
    // Linear on the way (the game's, not three's smoothstep).
    expect(fogAmount(50, f)).toBeCloseTo(0.25, 9);
    // The sky's most at end past it, and never more: the far side of a
    // level seen from afar fades, it is not hidden.
    expect(fogAmount(70, f)).toBeCloseTo(0.5, 9);
    expect(fogAmount(10_000, f)).toBeCloseTo(0.5, 9);
  });

  test("odd fog numbers stay sane", () => {
    const f = gameFog({ colour: [1, 1, 1], density: 3, start: 5, end: 5 }, -4);
    expect(f.cap).toBe(1);
    expect(f.near).toBe(5);
    expect(f.far).toBeGreaterThan(f.near);
    expect(Number.isFinite(fogAmount(6, f))).toBe(true);
  });

  test("three's fog is patched to the game's, once, with a shared cap every material gets", () => {
    expect(useGameFog()).toBe(true);
    expect(useGameFog()).toBe(true);
    const chunks = THREE.ShaderChunk as unknown as Record<string, string>;
    expect(chunks.fog_fragment).toContain("fogClear");
    expect(chunks.fog_fragment).not.toContain("smoothstep( fogNear");
    expect(chunks.fog_pars_fragment?.match(/uniform float fogClear;/g)?.length).toBe(1);
    // The exponential haze is untouched.
    expect(chunks.fog_fragment).toContain("exp( - fogDensity * fogDensity");
    const shader = { uniforms: {} as Record<string, unknown>, vertexShader: "", fragmentShader: "" };
    new THREE.MeshStandardMaterial().onBeforeCompile(shader as never, null as never);
    expect(shader.uniforms.fogClear).toBe(FOG_CLEAR);
  });
});

describe("the level's look", () => {
  const level = () => {
    const root = new THREE.Group();
    for (const [name, extras] of [["bsp_0", { name: "earthcity_1" }], ["bsp_1", { name: "bsp_1", cinematic: true }], ["bsp_2", {}], ["backdrop_0", {}]] as const) {
      const o = new THREE.Group();
      o.name = name;
      o.userData = { ...extras };
      root.add(o);
    }
    const decals = new THREE.Group();
    decals.name = "decals_0";
    const effects = new THREE.Group();
    effects.name = "effects_2";
    root.children[0]!.add(decals);
    root.children[2]!.add(effects);
    return { root, decals, effects };
  };

  test("measures each zone's extent in game coordinates", () => {
    const look = new LevelLook(new THREE.Scene(), new THREE.Texture(), () => {});
    const root = new THREE.Group();
    const z = new THREE.Mesh(new THREE.BoxGeometry(2, 4, 6));
    z.name = "bsp_1";
    // gl (10, 20, 30) is game (10, -30, 20).
    z.position.set(10, 20, 30);
    root.add(z);
    look.adopt(root, {}, "/m/map.glb");
    expect(look.zoneBoxes()).toEqual([{ bsp: 1, cinematic: false, min: [9, -33, 18], max: [11, -27, 22] }]);
    expect(look.zoneBox(1)?.bsp).toBe(1);
    expect(look.setActive(1)).toBe(true);
    expect(look.setActive(1)).toBe(false);
    expect(look.following()).toBe(true);
    look.setPick(1);
    expect(look.following()).toBe(false);
  });

  test("finds the zones and layers, follows the zone in play, and flips each switch", () => {
    const scene = new THREE.Scene();
    const gradient = new THREE.Texture();
    let changes = 0;
    const look = new LevelLook(scene, gradient, () => changes++);
    const { root, decals, effects } = level();
    look.adopt(root, { sky: { name: "s", clear: [0.2, 0.3, 0.4], fog: { colour: [0.5, 0.5, 0.5], density: 0.5, start: 0, end: 40 } } }, "/m/map.glb");
    const info = look.info();
    expect(info.zones.map((z) => z.label)).toEqual(["Earthcity 1", "Zone 3"]);
    expect([info.zoneCount, info.cinematicCount]).toEqual([3, 1]);
    expect(info).toMatchObject({ backdrop: true, decals: true, effects: true, sky: true, fog: true });
    const shown = () => root.children.map((c) => c.visible);
    expect(shown()).toEqual([true, false, true, false]);
    look.setActive(2);
    expect(shown()).toEqual([false, false, true, false]);
    look.setPick(0);
    look.setBackdrop(true);
    expect(shown()).toEqual([true, false, false, true]);
    look.setDecals(false);
    expect([decals.visible, effects.visible]).toEqual([false, true]);
    // The sky's clear colour is the background; with the sky off, the view's own.
    expect(scene.background).toBeInstanceOf(THREE.Color);
    look.setSky(false);
    expect(scene.background).toBe(gradient);
    // The sky's fog, measured from the focus; off is off.
    const camera = new THREE.PerspectiveCamera();
    look.beforeDraw(camera, 10);
    expect(scene.fog).toBeInstanceOf(THREE.Fog);
    expect((scene.fog as THREE.Fog).near).toBe(10);
    look.setFog(false);
    look.beforeDraw(camera, 10);
    expect(scene.fog).toBeNull();
    expect(changes).toBeGreaterThan(5);
    look.clear();
    expect(look.info().zones).toEqual([]);
  });

  test("without a sky the view keeps its own background and haze", () => {
    const scene = new THREE.Scene();
    const haze = new THREE.FogExp2(0xffffff, 0.01);
    scene.fog = haze;
    const gradient = new THREE.Texture();
    const look = new LevelLook(scene, gradient, () => {});
    look.adopt(level().root, {}, "/m/map.glb");
    expect(look.info()).toMatchObject({ sky: false, fog: false });
    expect(scene.background).toBe(gradient);
    look.setExtent(new THREE.Vector3(), 100);
    look.beforeDraw(new THREE.PerspectiveCamera(), 10);
    expect(scene.fog).toBe(haze);
    expect(haze.density).toBeCloseTo(0.0011);
    expect(look.skyShown()).toBe(false);
  });
});

describe("cutting through to the players", () => {
  test("off cuts to nobody; auto to the player in focus, else everyone; always to everyone, focus first", () => {
    expect(seeTargets("off", 2, [1, 2, 3])).toEqual([]);
    expect(seeTargets("auto", 2, [1, 2, 3])).toEqual([2]);
    expect(seeTargets("auto", null, [3, 1])).toEqual([3, 1]);
    expect(seeTargets("always", 2, [1, 2, 3])).toEqual([2, 1, 3]);
    const many = Array.from({ length: 20 }, (_, i) => i);
    expect(seeTargets("always", null, many)).toHaveLength(SEE_MAX);
  });

  test("a wall is cut when it stands between the camera and any of them", () => {
    const a: SeeThrough = { eye: [0, 1, 0], target: [20, 1, 0], radius: 0.6, halfHeight: 0.9 };
    const b: SeeThrough = { eye: [0, 1, 0], target: [0, 1, 20], radius: 0.6, halfHeight: 0.9 };
    expect(seeThroughHidesAny([10, 1, 0], [a, b])).toBe(true);
    expect(seeThroughHidesAny([0, 1, 10], [a, b])).toBe(true);
    expect(seeThroughHidesAny([10, 1, 10], [a, b])).toBe(false);
    expect(seeThroughHidesAny([10, 1, 0], [])).toBe(false);
  });

  test("the toolbar's key steps off, auto, always", () => {
    expect(nextSee("off")).toBe("auto");
    expect(nextSee("auto")).toBe("always");
    expect(nextSee("always")).toBe("off");
  });
});

describe("the zone in play", () => {
  const box = (bsp: number, x0: number, x1: number, cinematic = false): ZoneBox => ({ bsp, min: [x0, -10, -5], max: [x1, 10, 5], cinematic });
  // Zones 0 and 1 share the corridor from 90 to 100; zone 2 is a cutscene's.
  const boxes = [box(0, 0, 100), box(1, 90, 200), box(2, 1000, 1100, true)];
  const at = (x: number) => ({ x, y: 0, z: 0 });

  test("the game's loaded zone, where the players are in it", () => {
    expect(zoneWanted(boxes, { reported: 1, focus: null, players: [at(95)] }, 0)).toBe(1);
    expect(zoneWanted(boxes, { reported: 0, focus: null, players: [at(150)] }, 0)).toBe(1);
  });

  test("a cutscene's zone, a missing report or nobody inside: where the players stand", () => {
    expect(zoneWanted(boxes, { reported: 2, focus: null, players: [at(50)] }, 1)).toBe(0);
    expect(zoneWanted(boxes, { reported: 2, focus: null, players: [at(50)] }, 1, true)).toBe(0);
    expect(zoneWanted(boxes, { reported: null, focus: null, players: [at(150), at(160), at(20)] }, null)).toBe(1);
    // In the shared corridor the zone shown stays.
    expect(zoneWanted(boxes, { reported: null, focus: null, players: [at(95)] }, 0)).toBe(0);
    expect(zoneWanted(boxes, { reported: null, focus: null, players: [at(95)] }, 1)).toBe(1);
    // The followed player decides over the rest.
    expect(zoneWanted(boxes, { reported: null, focus: at(20), players: [at(150), at(160), at(20)] }, 1)).toBe(0);
    // No players: the game's word, else what is shown.
    expect(zoneWanted(boxes, { reported: 1, focus: null, players: [] }, 0)).toBe(1);
    expect(zoneWanted(boxes, { reported: 2, focus: null, players: [] }, 0)).toBe(0);
    expect(zoneWanted(boxes, { reported: null, focus: null, players: [at(5000)] }, 1)).toBe(1);
    expect(zoneWanted([], { reported: 1, focus: null, players: [] }, null)).toBeNull();
  });

  test("a new zone has to stay wanted a moment, except the first and on a seek", () => {
    const z = new ZoneFollower();
    const m = (x: number) => ({ reported: null, focus: null, players: [at(x)] });
    expect(z.update(0, boxes, m(50))).toBe(0);
    expect(z.update(100, boxes, m(150))).toBe(0);
    // Back over the line before the dwell is up: no flick.
    expect(z.update(200, boxes, m(50))).toBe(0);
    expect(z.update(300, boxes, m(150))).toBe(0);
    expect(z.update(300 + ZONE_DWELL_MS - 1, boxes, m(150))).toBe(0);
    expect(z.update(300 + ZONE_DWELL_MS, boxes, m(150))).toBe(1);
    expect(z.update(2000, boxes, m(20), { snap: true })).toBe(0);
    z.reset();
    expect(z.current).toBeNull();
  });
});
