import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { CUT_MARGIN, SEE_BEHIND, SEE_RADIUS, clampCut, cutHeight, cutRange, meshRole, patchSeeThrough, seeThroughHides, stepCut, type SeeThrough } from "../src/viewer/cutaway";
import { buildArena, encodeGlb, enclose } from "../mock/arena";

describe("what a map glb's meshes are", () => {
  test("the structure, however its nodes are named, is level geometry", () => {
    expect(meshRole(["structure"])).toBe("level");
    expect(meshRole(["bsp_0"])).toBe("level");
    expect(meshRole(["Scene", "bsp_12", "cluster_3"])).toBe("level");
    expect(meshRole([])).toBe("level");
  });

  test("anything under `machines` is a machine, and collision is never drawn", () => {
    expect(meshRole(["machines", "wheel"])).toBe("machine");
    expect(meshRole(["Scene", "machines", "relay_dish", "part"])).toBe("machine");
    expect(meshRole(["machines_1", "wheel"])).toBe("machine");
    expect(meshRole(["machinery_hall"])).toBe("level");
    expect(meshRole(["collision"])).toBe("hidden");
    expect(meshRole(["machines", "collision_wheel"])).toBe("hidden");
  });
});

describe("the height cut", () => {
  test("off, at the chosen height, or just above a followed player", () => {
    expect(cutHeight({ on: false, height: 4 }, 2)).toBeNull();
    expect(cutHeight({ on: true, height: 4 }, null)).toBe(4);
    expect(cutHeight({ on: true, height: 4 }, 2)).toBe(2 + CUT_MARGIN);
    // The margin clears a player and some headroom.
    expect(CUT_MARGIN).toBeGreaterThan(0.9);
  });

  test("the range runs from just above the lowest floor to the top", () => {
    expect(cutRange(0, 10)).toEqual([0.5, 10]);
    expect(cutRange(10, 0)).toEqual([0.5, 10]);
    expect(cutRange(2, 2.4)).toEqual([2.2, 2.4]);
    expect(clampCut(99, [0.5, 10])).toBe(10);
    expect(clampCut(-3, [0.5, 10])).toBe(0.5);
  });

  test("keys move it a twentieth of the range and stop at the ends", () => {
    const r: [number, number] = [0, 20];
    expect(stepCut(10, 1, r)).toBe(11);
    expect(stepCut(10, -1, r)).toBe(9);
    expect(stepCut(19.5, 1, r)).toBe(20);
    expect(stepCut(0.2, -1, r)).toBe(0);
  });
});

describe("the see-through", () => {
  // The camera 20 units from a player, looking level along +x. y is up.
  const s: SeeThrough = { eye: [0, 1, 0], target: [20, 1, 0], radius: 2, floor: 0.1 };

  test("a wall on the line of sight is cut; one beside it is not", () => {
    expect(seeThroughHides([10, 1, 0], s)).toBe(true);
    // The cone is half as wide halfway there: radius 1.
    expect(seeThroughHides([10, 1.7, 0], s)).toBe(true);
    expect(seeThroughHides([10, 2.2, 0], s)).toBe(false);
    expect(seeThroughHides([10, 1, 1.5], s)).toBe(false);
    // Close to the camera it is narrow: what's beside the view is kept.
    expect(seeThroughHides([2, 1.5, 0], s)).toBe(false);
    expect(seeThroughHides([2, 1.1, 0], s)).toBe(true);
  });

  test("nothing behind the camera, at the player, or past them is cut", () => {
    expect(seeThroughHides([-1, 1, 0], s)).toBe(false);
    expect(seeThroughHides([19.5, 1, 0], s)).toBe(false);
    expect(seeThroughHides([25, 1, 0], s)).toBe(false);
  });

  test("the floor they stand on stays, a floor above them goes", () => {
    const above: SeeThrough = { eye: [0, 21, 0], target: [20, 1, 0], radius: 2, floor: 0.1 };
    // Ground just in front of the player, inside the cone but at floor height.
    expect(seeThroughHides([18.6, 0.05, 0], above)).toBe(false);
    // An upper floor on the line of sight.
    expect(seeThroughHides([15, 6, 0], above)).toBe(true);
  });

  // The old see-through cut everything in the cone; `inCone` is that test
  // (a surface squarely facing the camera), to show each case below is one
  // the cone reaches.
  const inCone = (p: [number, number, number], at: SeeThrough) => seeThroughHides(p, at, 1);

  test("open terrain: ground under and beside the line of sight stays, a crest in the way goes", () => {
    // Behind and a little above a player on sandy ground, as the 3D view
    // follows: the eye 8 back and 2 up, the chest 0.6 above their feet.
    const follow: SeeThrough = { eye: [-8, 2, 0], target: [0, 0.6, 0], radius: SEE_RADIUS, floor: 0.06 };
    const bump: [number, number, number] = [-3, 0.4, 0];
    const beside: [number, number, number] = [-3.5, 0.3, 0.9];
    expect(inCone(bump, follow)).toBe(true);
    expect(inCone(beside, follow)).toBe(true);
    // A bump in the sand between them, its slope tilted up towards the camera.
    expect(seeThroughHides(bump, follow, 1, [-0.3, 1, 0])).toBe(false);
    // Flat ground beside the line.
    expect(seeThroughHides(beside, follow, 1, [0, 1, 0])).toBe(false);
    // A dune crest the line of sight goes into: higher than the line there.
    const crest: [number, number, number] = [-4, 1.6, 0];
    expect(inCone(crest, follow)).toBe(true);
    expect(seeThroughHides(crest, follow, 1, [-0.4, 1, 0])).toBe(true);
    // The crest's steep face towards the camera; the normal either way round.
    expect(seeThroughHides([-4.5, 1.5, 0], follow, 1, [-1, 0.3, 0])).toBe(true);
    expect(seeThroughHides([-4.5, 1.5, 0], follow, 1, [1, -0.3, 0])).toBe(true);
  });

  test("indoors: a partition or an upper floor in the way goes; a wall beside the line and the floor underfoot stay", () => {
    const room: SeeThrough = { eye: [0, 6, 0], target: [10, 1, 0], radius: 2, floor: 0.1 };
    // A partition across the line, facing the camera.
    expect(seeThroughHides([5, 3.5, 0], room, 1, [-1, 0, 0])).toBe(true);
    // The top of an upper floor between them.
    expect(seeThroughHides([5, 3.4, 0.3], room, 1, [0, 1, 0])).toBe(true);
    // A corridor wall running along the line, beside it: the player is in front of it.
    expect(inCone([5, 3.5, 0.7], room)).toBe(true);
    expect(seeThroughHides([5, 3.5, 0.7], room, 1, [0, 0, -1])).toBe(false);
    // A step up near the player, inside the cone, below their chest.
    expect(inCone([7, 1.1, 0], room)).toBe(true);
    expect(seeThroughHides([7, 1.1, 0], room, 1, [0, 1, 0])).toBe(false);
  });

  test("a surface nearly edge-on to the player is kept, so it doesn't flicker as the camera moves", () => {
    // A wall plane passing just behind the chest: within the margin, kept
    // whichever side the camera's small moves put it.
    for (const dx of [-0.05, 0, 0.05]) {
      const near: SeeThrough = { eye: [0, 1, 0], target: [10 + SEE_BEHIND * 0.5 + dx, 1, 0], radius: 2, floor: 0.1 };
      expect(seeThroughHides([10, 1.3, 0], near, 1, [-1, 0, 0])).toBe(false);
    }
    // A degenerate face (no normal) is never cut.
    expect(seeThroughHides([10, 1, 0], s, 1, [0, 0, 0])).toBe(false);
  });

  test("off, or with no distance to look along, it cuts nothing", () => {
    expect(seeThroughHides([10, 1, 0], { ...s, radius: 0 })).toBe(false);
    expect(seeThroughHides([0, 1, 0], { ...s, target: s.eye })).toBe(false);
  });

  test("the rim thins out: the outer fifth depends on the dither", () => {
    // Halfway, the cone's radius is 1: at 0.9 from the axis only a high dither value cuts.
    expect(seeThroughHides([10, 1.9, 0], s, 1)).toBe(true);
    expect(seeThroughHides([10, 1.9, 0], s, 0)).toBe(false);
    expect(seeThroughHides([10, 1.7, 0], s, 0)).toBe(true);
    expect(SEE_RADIUS).toBeGreaterThan(1);
  });

  test("fits the shaders three builds for the map's materials and the occlusion pass", () => {
    for (const lib of [THREE.ShaderLib.standard, THREE.ShaderLib.physical, THREE.ShaderLib.normal, THREE.ShaderLib.basic]) {
      const out = patchSeeThrough(lib.vertexShader, lib.fragmentShader)!;
      expect(out).not.toBeNull();
      expect(out.vertex).toContain("vSeeWorld = (modelMatrix");
      expect(out.fragment).toContain("uniform vec4 uSeeTargets[");
      expect(out.fragment).toContain("uniform int uSeeCount;");
      expect(out.fragment.indexOf("#include <clipping_planes_fragment>")).toBeLessThan(out.fragment.indexOf("discard;"));
      // The surface normal is taken outside every branch (derivatives need it) and tested before the discard.
      expect(out.fragment.indexOf("dFdx(vSeeWorld)")).toBeLessThan(out.fragment.indexOf("if (uSeeCount > 0)"));
      expect(out.fragment.indexOf("seeTarget.xyz - vSeeWorld, seeN")).toBeLessThan(out.fragment.indexOf("discard;"));
      expect(out.vertex.indexOf("#include <project_vertex>")).toBeLessThan(out.vertex.indexOf("vSeeWorld = "));
    }
    expect(patchSeeThrough("void main() {}", "void main() {}")).toBeNull();
  });
});

describe("a level in several nodes", () => {
  test("the mock's indoor level has bsp nodes and machines, found whatever they are called", async () => {
    const arena = enclose(buildArena("orchard")!);
    const bytes = encodeGlb(arena);
    const g = await new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");
    const roles = new Map<string, string>();
    let shell: THREE.Material | null = null;
    const walk = (o: THREE.Object3D, names: string[]) => {
      const path = o.name ? [...names, o.name] : names;
      const mesh = o as THREE.Mesh;
      if (mesh.isMesh) {
        roles.set(path.filter((n) => /^(structure|bsp_\d+|backdrop|machines|wheel|dish)$/.test(n)).join("/"), meshRole(path));
        for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) if (m.name === "shell") shell = m;
      }
      for (const c of o.children) walk(c, path);
    };
    walk(g.scene, []);
    expect(Object.fromEntries(roles)).toEqual({
      structure: "level", bsp_0: "level", bsp_1: "level", bsp_2: "level", backdrop: "level", "machines/wheel": "machine", "machines/dish": "machine",
    });
    // The shell is written double-sided: only the viewer's culling opens it.
    expect(shell!.side).toBe(THREE.DoubleSide);
    // The pre-drawn views are still made from the open arena.
    expect(arena.tris.length).toBe(buildArena("orchard")!.tris.length);
    expect(arena.extra!.length).toBeGreaterThan(500);
  });

  test("an ordinary arena is still one structure node", async () => {
    const bytes = encodeGlb(buildArena("teacup")!);
    const g = await new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");
    expect(g.scene.children.map((c) => c.name)).toEqual(["structure"]);
  });
});
