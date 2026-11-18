import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { LevelLook } from "../src/viewer/level3d";
import { skyOf } from "../src/viewer/sky3d";
import { WAVES, dressWater, fresnel, isWater, rippleSlope, waterUniforms } from "../src/viewer/water3d";
import { inside, weatherOf } from "../src/viewer/weather3d";

const unitBox = [
  [1, 0, 0, 1], [-1, 0, 0, 1], [0, 1, 0, 1], [0, -1, 0, 1], [0, 0, 1, 1], [0, 0, -1, 1],
];

describe("the level's weather", () => {
  const extras = {
    weather: [
      { name: "snow_falling", kind: "snow", amount: 1, boxes: [[-10, -10, -10, 10, 10, 10]], exclude: [unitBox] },
      { name: "odd", kind: "hail", amount: 1, boxes: [[0, 0, 0, 1, 1, 1]] },
      { name: "empty", kind: "rain", amount: 1, boxes: [] },
      { name: "bad box", kind: "rain", amount: 9, boxes: [[0, 0, 0], [0, 0, 0, 1, 1, 1]] },
    ],
  };

  test("only well-formed systems are kept, amounts clamped", () => {
    const w = weatherOf(extras);
    expect(w.map((s) => s.name)).toEqual(["snow_falling", "bad box"]);
    expect(w[1]!.boxes).toHaveLength(1);
    expect(w[1]!.amount).toBe(3);
    expect(weatherOf({})).toEqual([]);
    expect(weatherOf(null)).toEqual([]);
  });

  test("it falls in its boxes and not in its kept-out volumes", () => {
    const [snow] = weatherOf(extras);
    expect(inside(snow!, [5, 5, 5])).toBe(true);
    expect(inside(snow!, [0, 0, 0])).toBe(false);
    expect(inside(snow!, [0.5, -0.5, 0.9])).toBe(false);
    expect(inside(snow!, [11, 0, 0])).toBe(false);
  });

  test("the toolbar offers it only when the level has some, and it keeps the view moving", () => {
    const scene = new THREE.Scene();
    const look = new LevelLook(scene, new THREE.Texture(), () => {});
    look.adopt(new THREE.Group(), {}, "map.glb");
    expect(look.info().weather).toBe(false);
    expect(look.animate(1, new THREE.Vector3(), 10)).toBe(false);
    look.adopt(new THREE.Group(), extras, "map.glb");
    expect(look.info().weather).toBe(true);
    expect(look.animate(1, new THREE.Vector3(), 10)).toBe(true);
    look.setWeather(false);
    expect(look.animate(1, new THREE.Vector3(), 10)).toBe(false);
    expect(scene.getObjectByName("weather")!.visible).toBe(false);
    look.dispose();
  });
});

describe("the sky's glow", () => {
  test("is read when whole", () => {
    const glow = { colour: [0.9, 0.75, 0.6], density: 0.2, heading: 2.9, pitch: 0.6 };
    expect(skyOf({ sky: { name: "zanzibar", glow } })!.glow).toEqual(glow as never);
    expect(skyOf({ sky: { name: "x", glow: { ...glow, density: 0 } } })!.glow).toBeUndefined();
    expect(skyOf({ sky: { name: "x", glow: { ...glow, colour: [1, 1] } } })!.glow).toBeUndefined();
  });

  test("is not a fog: a level without fog gets the faint haze whatever its glow", () => {
    const scene = new THREE.Scene();
    scene.fog = new THREE.FogExp2(0x7f9fb6, 0.002);
    const look = new LevelLook(scene, new THREE.Texture(), () => {});
    look.setExtent(new THREE.Vector3(), 100);
    // A converted Halo 1 sky's light read as a glow: cyan and thick.
    look.adopt(new THREE.Group(), { sky: { name: "a30", glow: { colour: [0, 0.87, 0.86], density: 0.82, heading: 0, pitch: 0 } } }, "map.glb");
    expect(look.info().fog).toBe(false);
    look.beforeDraw(new THREE.PerspectiveCamera(), 10);
    const haze = scene.fog as THREE.FogExp2;
    expect(haze.color.getHex()).toBe(0x7f9fb6);
    // Faint: under 5 % at the level's far side (twice its radius).
    expect(1 - Math.exp(-((haze.density * 200) ** 2))).toBeLessThan(0.05);
    look.dispose();
  });

  test("a level's own fog is drawn as the game's, and offered", () => {
    const scene = new THREE.Scene();
    const look = new LevelLook(scene, new THREE.Texture(), () => {});
    look.setExtent(new THREE.Vector3(), 100);
    look.adopt(new THREE.Group(), { sky: { name: "coagulation", fog: { colour: [0.64, 0.51, 0.51], density: 0.4, start: 10, end: 150 } } }, "map.glb");
    expect(look.info().fog).toBe(true);
    look.beforeDraw(new THREE.PerspectiveCamera(), 0);
    const fog = scene.fog as THREE.Fog;
    expect([fog.near, fog.far]).toEqual([10, 150]);
    look.dispose();
  });
});

describe("the level's water", () => {
  test("is the glb's water material, made to ripple once", () => {
    const u = waterUniforms();
    const water = new THREE.MeshStandardMaterial({ name: "water", transparent: true, opacity: 0.75 });
    const wall = new THREE.MeshStandardMaterial({ name: "wall" });
    expect(isWater(water)).toBe(true);
    expect(dressWater(wall, u)).toBe(false);
    expect(dressWater(water, u)).toBe(true);
    expect(dressWater(water, u)).toBe(true);
    expect(water.depthWrite).toBe(false);
    expect(water.customProgramCacheKey()).toContain("h2water");
    const shader = { uniforms: {}, vertexShader: "#include <worldpos_vertex>", fragmentShader: "#include <normal_fragment_maps>\n#include <tonemapping_fragment>" };
    water.onBeforeCompile(shader as never, {} as never);
    expect(shader.uniforms).toHaveProperty("uWaterTime");
    expect(shader.fragmentShader).toContain("waterSlope");
    expect(shader.vertexShader).toContain("vWaterWorld =");
  });

  test("reflects more at grazing angles", () => {
    expect(fresnel(1)).toBeCloseTo(0.02, 5);
    expect(fresnel(0)).toBeCloseTo(1, 5);
    expect(fresnel(0.2)).toBeGreaterThan(fresnel(0.8));
  });

  test("its ripples move, and stay gentle", () => {
    const a = rippleSlope(1, 2, 0), b = rippleSlope(1, 2, 0.5);
    expect(a).not.toEqual(b);
    const most = WAVES.reduce((s, w) => s + (w.height * 2 * Math.PI) / w.length, 0);
    for (let i = 0; i < 50; i++) {
      const [gx, gz] = rippleSlope(i * 0.37, i * 1.1, i * 0.2);
      expect(Math.hypot(gx, gz)).toBeLessThanOrEqual(most + 1e-9);
    }
    expect(most).toBeLessThan(0.8);
  });
});
