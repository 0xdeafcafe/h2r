import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { skipsOcclusion } from "../src/viewer/aopass";
import { SEE_MAX, nextSee, seeTargets, seeThroughHidesAny, type SeeThrough } from "../src/viewer/cutaway";
import { dressLayer, layerLook, layerOf } from "../src/viewer/layers3d";
import { LevelLook } from "../src/viewer/level3d";
import { fogRange, skyColour, skyOf, skyUrl } from "../src/viewer/sky3d";
import { isBackdrop, layerNode, zoneInPlay, zoneLabel, zoneList, zoneVisibility } from "../src/viewer/zones";

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

  test("the fog is measured from the point in focus, and never hides the far side of the level", () => {
    const fog = { colour: [1, 1, 1] as [number, number, number], density: 0.5, start: 20, end: 60 };
    // Clear up to start past the focus; as thick as the game's (a half) at end past it.
    const r = fogRange(fog, 10);
    expect(r.near).toBe(30);
    const at = (d: number, x: { near: number; far: number }) => (d - x.near) / (x.far - x.near);
    expect(at(70, r)).toBeCloseTo(0.5);
    // Seen from afar, the level's far side (800 away) is no thicker than the game's most.
    const wide = fogRange(fog, 300, 800);
    expect(at(800, wide)).toBeCloseTo(0.5);
    expect(at(300 + 60, wide)).toBeLessThan(0.5);
    // A level that ends before the fog is thick changes nothing.
    expect(fogRange(fog, 10, 40)).toEqual(r);
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
    const a: SeeThrough = { eye: [0, 1, 0], target: [20, 1, 0], radius: 2, floor: 0.1 };
    const b: SeeThrough = { eye: [0, 1, 0], target: [0, 1, 20], radius: 2, floor: 0.1 };
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
