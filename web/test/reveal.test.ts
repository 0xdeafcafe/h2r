import { describe, expect, test } from "bun:test";
import { Color, Group, MeshStandardMaterial, PerspectiveCamera, Plane, Scene, ShaderLib, Texture, Vector3, Vector4 } from "three";
import { patchSeeThrough } from "../src/viewer/cutaway";
import { CutawayMaterials } from "../src/viewer/cutaway3d";
import { LevelLook } from "../src/viewer/level3d";
import { patchReveal, revealAt, revealBand, revealDuration } from "../src/viewer/reveal";
import { FOG_CLEAR } from "../src/viewer/sky3d";

describe("the opening sweep", () => {
  const reach = 60;
  const duration = 1640;
  const band = 3.6;

  test("rests at the origin for 200 ms, sweeps out, then switches off", () => {
    expect(revealAt(0, reach)).toEqual({ holo: 0, radius: 0, objectsRadius: 0, sky: 0, shadows: 0, done: false });
    expect(revealAt(200, reach)).toEqual({ holo: 2 / 3, radius: 0, objectsRadius: 0, sky: 0, shadows: 0, done: false });

    const mid = revealAt(200 + duration / 2, reach);
    expect(mid.holo).toBe(1);
    expect(mid.radius).toBeCloseTo((reach + band) * 0.875);
    expect(mid.sky).toBeCloseTo(5 / 12);
    expect(mid.shadows).toBe(0);
    expect(mid.done).toBe(false);

    const shellEnd = revealAt(200 + duration, reach);
    expect(shellEnd.radius).toBeCloseTo(reach + band);
    expect(shellEnd.sky).toBe(1);
    expect(shellEnd.shadows).toBe(1);
    expect(shellEnd.done).toBe(false);

    expect(revealAt(380 + duration, reach)).toEqual({ holo: 1, radius: -1, objectsRadius: -1, sky: 1, shadows: 1, done: true });
  });

  test("clamps the duration and gives the band a minimum width", () => {
    expect(revealDuration(2)).toBe(1300);
    expect(revealDuration(reach)).toBe(duration);
    expect(revealDuration(200)).toBe(2400);
    expect(revealBand(2)).toBe(1.5);
    expect(revealBand(reach)).toBeCloseTo(band);
  });

  test("the object sweep uses the same radius 180 ms later", () => {
    expect(revealAt(380, reach).objectsRadius).toBe(0);
    for (const ms of [200, 300, 500, 1300]) {
      expect(revealAt(ms + 180, reach).objectsRadius).toBe(revealAt(ms, reach).radius);
    }
  });
});

describe("the reveal shader", () => {
  test("patches standard, lambert, phong and basic shaders with or without the see-through", () => {
    for (const lib of [ShaderLib.standard, ShaderLib.lambert, ShaderLib.phong, ShaderLib.basic]) {
      const uncut = { vertex: lib.vertexShader, fragment: lib.fragmentShader };
      const cut = patchSeeThrough(uncut.vertex, uncut.fragment)!;
      for (const shaders of [uncut, cut]) {
        const patched = patchReveal(shaders.vertex, shaders.fragment)!;
        expect(patched).not.toBeNull();
        for (const source of [patched.vertex, patched.fragment]) {
          expect(source.match(/varying vec3 vSeeWorld;/g)).toHaveLength(1);
        }
        expect(patched.vertex.indexOf("#include <project_vertex>")).toBeLessThan(patched.vertex.indexOf("vSeeWorld = "));
        expect(patched.fragment).toContain("uniform vec4 uReveal;");
        expect(patched.fragment).toContain("smoothstep(uReveal.w - uRevealBand, uReveal.w, distance(vSeeWorld, uReveal.xyz))");
        expect(patched.fragment.indexOf("#include <clipping_planes_fragment>")).toBeLessThan(patched.fragment.indexOf("fwidth(revealQ)"));
        expect(patched.fragment.indexOf("fwidth(revealQ)")).toBeLessThan(patched.fragment.indexOf("discard;"));
        expect(patched.fragment.indexOf("gl_FragColor.rgb = mix")).toBeLessThan(patched.fragment.indexOf("#include <dithering_fragment>"));
        if (shaders === cut) expect(patched.fragment).toContain("uSeeCount");
      }
    }
  });

  test("also patches the normal pass with the same discard and no colour changes", () => {
    const { vertexShader, fragmentShader } = ShaderLib.normal;
    const cut = patchSeeThrough(vertexShader, fragmentShader)!;
    const patched = patchReveal(cut.vertex, cut.fragment, "normal")!;
    expect(patched).not.toBeNull();
    expect(patched.fragment).toContain("revealLine * uRevealHolo < 0.5) discard;");
    expect(patched.fragment).not.toContain("gl_FragColor.rgb = mix");
  });

  test("the shadow pass keeps only the solid geometry inside the band", () => {
    const { vertexShader, fragmentShader } = ShaderLib.depth;
    const patched = patchReveal(vertexShader, fragmentShader, "shadow")!;
    expect(patched.vertex).toContain("vSeeWorld = ");
    expect(patched.fragment).toContain("distance(vSeeWorld, uReveal.xyz) >= uReveal.w - uRevealBand) discard;");
    expect(patched.fragment).not.toContain("fwidth");
  });

  test("leaves shaders without the required hooks alone", () => {
    const { vertexShader, fragmentShader } = ShaderLib.standard;
    expect(patchReveal("void main() {}", fragmentShader)).toBeNull();
    expect(patchReveal(vertexShader, "void main() {}")).toBeNull();
    expect(patchReveal(vertexShader, fragmentShader.replace("#include <dithering_fragment>", ""))).toBeNull();
  });

  test("the solidity is one inside the band and falls to zero at the sweep edge", () => {
    // Mirror the shader's one-minus-smoothstep, including the off sentinel.
    function solidity(radius: number, band: number, distance: number) {
      if (radius < 0) return 1;
      const t = Math.min(1, Math.max(0, (distance - (radius - band)) / band));
      return 1 - t * t * (3 - 2 * t);
    }
    expect(solidity(20, 2, 17)).toBe(1);
    expect(solidity(20, 2, 18)).toBe(1);
    expect(solidity(20, 2, 19)).toBe(0.5);
    expect(solidity(20, 2, 20)).toBe(0);
    expect(solidity(20, 2, 21)).toBe(0);
    expect(solidity(-1, 2, 21)).toBe(1);
  });
});

describe("reveal in the scene's materials and atmosphere", () => {
  test("shell and vehicle placements keep distinct radii and a stable program key", () => {
    const cuts = new CutawayMaterials(new Plane(), new Vector3(), true);
    const shell = new MeshStandardMaterial(), vehicle = new MeshStandardMaterial();
    cuts.attach(shell);
    cuts.attach(vehicle, false, "objects");
    const renderShader = (material: MeshStandardMaterial) => {
      const shader = {
        vertexShader: ShaderLib.standard.vertexShader,
        fragmentShader: ShaderLib.standard.fragmentShader,
        uniforms: {} as Record<string, { value: unknown }>,
      };
      material.onBeforeCompile(shader as never, null as never);
      return shader;
    };
    const levelShader = renderShader(shell), vehicleShader = renderShader(vehicle);
    expect(levelShader.uniforms.uSeeCount).toBeDefined();
    expect(vehicleShader.uniforms.uSeeCount).toBeUndefined();
    const levelRadius = levelShader.uniforms.uReveal!.value as Vector4;
    const vehicleRadius = vehicleShader.uniforms.uReveal!.value as Vector4;
    const frame = revealAt(500, 20);
    const key = shell.customProgramCacheKey();
    expect(key).toEndWith("|see|reveal");
    expect(vehicle.customProgramCacheKey()).toEndWith("|reveal");
    cuts.setReveal(new Vector3(1, 2, 3), revealBand(20), 1, frame);
    expect(levelRadius.toArray()).toEqual([1, 2, 3, frame.radius]);
    expect(vehicleRadius.toArray()).toEqual([1, 2, 3, frame.objectsRadius]);
    cuts.clearReveal();
    expect([levelRadius.w, vehicleRadius.w]).toEqual([-1, -1]);
    expect(shell.customProgramCacheKey()).toBe(key);
    cuts.dispose();
  });

  test("the sky and fog can finish before the level layers return", () => {
    const scene = new Scene();
    const gradient = new Texture();
    const look = new LevelLook(scene, gradient, () => {});
    const root = new Group(), decals = new Group(), effects = new Group();
    decals.name = "decals_0";
    effects.name = "effects_0";
    root.add(decals, effects);
    const extras = { sky: { clear: [0.2, 0.3, 0.4], fog: { colour: [0.5, 0.6, 0.8], density: 0.5, start: 0, end: 40 } } };
    look.adopt(root, extras, "/map.glb");
    look.setReveal(0, false);
    expect((scene.background as Color).getHexString()).toBe("050d18");
    expect([decals.visible, effects.visible, scene.getObjectByName("weather")?.visible]).toEqual([false, false, false]);
    look.setReveal(0.5, false);
    look.beforeDraw(new PerspectiveCamera(), 0);
    expect(scene.backgroundIntensity).toBe(0.5);
    expect(FOG_CLEAR.value).toBe(0.75);
    look.setReveal(1, false);
    expect([decals.visible, effects.visible]).toEqual([false, false]);
    look.setEffects(false);
    look.setReveal(1, true);
    expect([decals.visible, effects.visible, scene.getObjectByName("weather")?.visible]).toEqual([true, false, true]);
    look.dispose();
    expect(scene.background).toBe(gradient);
  });
});
