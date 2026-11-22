import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { CUT_MARGIN, clampCut, cutHeight, cutRange, meshRole, patchSeeThrough, seeThroughHides, stepCut, takesSeeThrough, type SeeThrough } from "../src/viewer/cutaway";
import { buildArena, encodeGlb, enclose } from "../mock/arena";
import { CutawayMaterials } from "../src/viewer/cutaway3d";
import { FilmWorld3D } from "../src/viewer/film-world3d";

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

describe("which meshes the see-through cuts", () => {
  test("the level, its machines and its placed objects all take it; vehicles and collision don't", () => {
    expect(takesSeeThrough(meshRole(["structure"]))).toBe(true);
    expect(takesSeeThrough(meshRole(["bsp_3", "cluster_0"]))).toBe(true);
    expect(takesSeeThrough(meshRole(["machines", "lift_0"]))).toBe(true);
    expect(takesSeeThrough(meshRole(["scenario", "scen_4"]))).toBe(true);
    expect(takesSeeThrough(meshRole(["scenario", "vehi_0"]), true)).toBe(false);
    expect(takesSeeThrough(meshRole(["collision"]))).toBe(false);
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
  const s: SeeThrough = { eye: [0, 1, 0], target: [20, 1, 0], radius: 0.6, halfHeight: 1 };

  test("the window follows the projected body, growing toward the player", () => {
    expect(seeThroughHides([10, 1, 0], s)).toBe(true);
    expect(seeThroughHides([10, 1.45, 0], s)).toBe(true);
    expect(seeThroughHides([10, 1.6, 0], s)).toBe(false);
    expect(seeThroughHides([10, 1, 0.45], s)).toBe(false);
    expect(seeThroughHides([2, 1.2, 0], s)).toBe(false);
  });

  test("a close slab is still cut and surfaces behind the player or camera stay", () => {
    expect(seeThroughHides([19.95, 1, 0], s)).toBe(true);
    expect(seeThroughHides([20.05, 1, 0], s)).toBe(false);
    expect(seeThroughHides([-1, 1, 0], s)).toBe(false);
  });

  test("a floor above the player can occlude them and the floor underfoot stays from above", () => {
    const above: SeeThrough = { ...s, eye: [0, 3, 0], target: [10, 1, 0], halfHeight: 0.9 };
    expect(seeThroughHides([5, 2, 0], above)).toBe(true);
    expect(seeThroughHides([10, 0, 0], above)).toBe(false);
    expect(seeThroughHides([5, 0.05, 0], above)).toBe(false);
    expect(seeThroughHides([10, 0, 0], { ...above, eye: [10, 5, 0] })).toBe(false);
  });

  test("a disabled window or a camera inside the target cuts nothing", () => {
    expect(seeThroughHides([10, 1, 0], { ...s, radius: 0 })).toBe(false);
    expect(seeThroughHides([10, 1, 0], { ...s, eye: s.target })).toBe(false);
  });

  test("an instanced or batched mesh is cut where each copy stands, not where its model sits", () => {
    const out = patchSeeThrough(THREE.ShaderLib.standard.vertexShader, THREE.ShaderLib.standard.fragmentShader)!;
    const body = out.vertex.slice(out.vertex.indexOf("#include <project_vertex>"));
    // The same order project_vertex uses: batch, then instance, then model.
    const batch = body.indexOf("batchingMatrix * seeLocal"), inst = body.indexOf("instanceMatrix * seeLocal"), model = body.indexOf("vSeeWorld = (modelMatrix * seeLocal)");
    expect(batch).toBeGreaterThan(0);
    expect(inst).toBeGreaterThan(batch);
    expect(model).toBeGreaterThan(inst);
    expect(body).toContain("#ifdef USE_INSTANCING");
    expect(body).toContain("#ifdef USE_BATCHING");
  });

  test("fits the shaders three builds for the map's materials and the occlusion pass", () => {
    for (const lib of [THREE.ShaderLib.standard, THREE.ShaderLib.physical, THREE.ShaderLib.normal, THREE.ShaderLib.basic]) {
      const out = patchSeeThrough(lib.vertexShader, lib.fragmentShader)!;
      expect(out).not.toBeNull();
      expect(out.vertex).toContain("vSeeWorld = (modelMatrix * seeLocal)");
      expect(out.fragment).toContain("uniform vec4 uSeeTargets[");
      expect(out.fragment).toContain("uniform int uSeeCount;");
      expect(out.fragment.indexOf("#include <clipping_planes_fragment>")).toBeLessThan(out.fragment.indexOf("discard;"));
      expect(out.fragment).toContain("uniform vec3 uSeeEye;");
      expect(out.fragment).toContain("seeRaySq >= dot(seeRay, seeCentre)");
      expect(out.vertex.indexOf("#include <project_vertex>")).toBeLessThan(out.vertex.indexOf("vSeeWorld = "));
    }
    expect(patchSeeThrough("void main() {}", "void main() {}")).toBeNull();
  });

  test("the same window cuts a split BSP, instanced scenery and a loose machine", () => {
    const scene = new THREE.Scene();
    const cutaway = new CutawayMaterials(new THREE.Plane(new THREE.Vector3(0, -1, 0), 100), new THREE.Vector3(...s.eye));
    cutaway.uniforms.uSeeTargets.value[0]!.set(...s.target, s.radius);
    cutaway.uniforms.uSeeHalfHeights.value[0] = s.halfHeight;
    cutaway.uniforms.uSeeCount.value = 1;

    const geometry = new THREE.BoxGeometry(0.25, 2, 2);
    const levelMaterial = new THREE.MeshStandardMaterial();
    cutaway.attach(levelMaterial);
    const bsp = new THREE.Group();
    bsp.name = "bsp_3";
    const cluster = new THREE.Mesh(geometry, levelMaterial);
    cluster.name = "cluster_0";
    cluster.position.set(3, 1, 0);
    bsp.add(cluster);
    scene.add(bsp);

    const scenery = new THREE.InstancedMesh(geometry, levelMaterial, 2);
    scenery.name = "scenery";
    scenery.setMatrixAt(0, new THREE.Matrix4().makeTranslation(7, 1, 0));
    scenery.setMatrixAt(1, new THREE.Matrix4().makeTranslation(7, 1, 3));
    scene.add(scenery);

    const shared = new THREE.MeshStandardMaterial();
    const machine = new THREE.Mesh(geometry, shared);
    machine.name = "machine";
    machine.position.set(19.95, 1, 0);
    cutaway.attachModel(machine);
    scene.add(machine);
    const player = new THREE.Mesh(new THREE.SphereGeometry(s.radius), shared);
    player.position.set(...s.target);
    scene.add(player);
    const behind = new THREE.Mesh(geometry, levelMaterial);
    behind.name = "behind";
    behind.position.set(22, 1, 0);
    scene.add(behind);
    scene.updateMatrixWorld();

    const ray = new THREE.Raycaster(new THREE.Vector3(...s.eye), new THREE.Vector3(20, 0.02, 0.03).normalize());
    const hits = ray.intersectObjects(scene.children);
    expect(hits.some((hit) => hit.object === behind)).toBe(true);
    expect(hits.some((hit) => hit.object === player)).toBe(true);
    const cut = new Set(hits.flatMap(({ object, point: { x, y, z } }) =>
      object !== player && seeThroughHides([x, y, z], s) ? [object.name] : []));
    expect([...cut]).toEqual(["cluster_0", "scenery", "machine"]);
    ray.ray.direction.set(7, 0, 3).normalize();
    const side = ray.intersectObject(scenery).find((hit) => hit.instanceId === 1);
    expect(side).toBeDefined();
    const { x, y, z } = side!.point;
    expect(seeThroughHides([x, y, z], s)).toBe(false);
    expect(machine.material).not.toBe(shared);
    expect(shared.customProgramCacheKey()).not.toEndWith("|see");
    for (const material of [cluster.material, scenery.material, machine.material]) {
      expect(material.customProgramCacheKey()).toEndWith("|see");
      expect(material.transparent).toBe(false);
      expect(material.depthWrite).toBe(true);
    }
    cutaway.dispose();
  });

  test("a film's loose scenery joins the cutaway without changing the model template", () => {
    const cutaway = new CutawayMaterials(new THREE.Plane(), new THREE.Vector3());
    const world = new FilmWorld3D((object) => cutaway.attachModel(object));
    const template = new THREE.Mesh(new THREE.BoxGeometry(), new THREE.MeshStandardMaterial());
    world.update({ props: [{ id: "scenery", class: "scen", kind: "crate", pos: [2, 0, 0], forward: [1, 0, 0], up: [0, 0, 1] }], hill: null }, () => template);
    const placed = world.group.children.find((object) => object instanceof THREE.Mesh) as typeof template;
    expect(placed.material.customProgramCacheKey()).toEndWith("|see");
    expect(template.material.customProgramCacheKey()).not.toEndWith("|see");
    world.dispose();
    cutaway.dispose();
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
