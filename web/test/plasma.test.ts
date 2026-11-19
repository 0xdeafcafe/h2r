import { expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { loadPlasma, plasmaOffset, plasmaSpec } from "../src/viewer/plasma3d";
import { deferTextures } from "../src/viewer/textures3d";

const motion = { from: 0.25, to: -0.75, period: 4 };
const transform = { scale: [2, 3], offset: [0.1, 0.2], scroll: [null, motion] };
const extras = {
  plasma: {
    uv: [transform, { ...transform, scroll: [motion, null] }, { ...transform, scroll: [null, null] }],
    colors: [[0.6, 0.1, 0.7], [0.2, 0.2, 0.5], [0, 0, 0]],
  },
  noise: [1, 2],
};

test("plasma UVs preserve scale and stationary axes, and wrap using the tag period", () => {
  const spec = plasmaSpec(extras)!;
  expect(spec.uv[0].scale).toEqual([2, 3]);
  expect(spec.uv[0].offset).toEqual([0.1, 0.2]);
  expect(spec.uv[0].scroll).toEqual([null, motion]);
  expect(plasmaOffset(motion, 0)).toBe(0.25);
  expect(plasmaOffset(motion, 1)).toBe(0);
  expect(plasmaOffset(motion, 3)).toBe(-0.5);
  expect(plasmaOffset(motion, 4)).toBe(0.25);
  expect(plasmaOffset(motion, 9)).toBe(0);
});

test("plasma rejects invalid motion and incomplete material metadata", () => {
  expect(plasmaSpec({ plasma: { ...extras.plasma, uv: [transform] } })).toBeNull();
  expect(plasmaSpec({ ...extras, noise: [1, -2] })).toBeNull();
  const bad = { ...transform, scroll: [null, { ...motion, period: 0 }] };
  expect(plasmaSpec({ ...extras, plasma: { ...extras.plasma, uv: [bad, transform, transform] } })).toBeNull();
  expect(plasmaSpec({ look: true })).toBeNull();
});

test("a plasma glTF material uses additive depth-tested rendering and streams all three pictures", async () => {
  const loader = new GLTFLoader();
  const pending = deferTextures(loader, "/art/game/map/test/map.glb?v=2");
  const gltf = await loader.parseAsync(JSON.stringify({
    asset: { version: "2.0" },
    scene: 0,
    scenes: [{}],
    extensionsUsed: ["KHR_materials_unlit"],
    materials: [{
      extensions: { KHR_materials_unlit: {} },
      pbrMetallicRoughness: { baseColorTexture: { index: 0 } },
      alphaMode: "BLEND",
      extras,
    }],
    images: ["mask", "a", "b"].map((name) => ({ uri: `tex/${name}.webp` })),
    textures: [0, 1, 2].map((source) => ({ source })),
  }), "/art/game/map/test/");
  const material = await gltf.parser.getDependency("material", 0) as THREE.MeshBasicMaterial;
  const mesh = new THREE.Mesh(new THREE.PlaneGeometry(), material);
  const solid = new THREE.Mesh(new THREE.PlaneGeometry(), new THREE.MeshBasicMaterial());
  gltf.scene.add(mesh, solid);
  const plasma = await loadPlasma(gltf);

  expect(plasma.has(material)).toBe(true);
  expect(plasma.has(solid.material)).toBe(false);
  expect(material.blending).toBe(THREE.CustomBlending);
  expect(material.blendEquation).toBe(THREE.AddEquation);
  expect(material.blendSrc).toBe(THREE.OneFactor);
  expect(material.blendDst).toBe(THREE.OneFactor);
  expect(material.depthTest).toBe(true);
  expect(material.depthWrite).toBe(false);
  expect(material.forceSinglePass).toBe(true);
  expect(plasma.animate(1)).toBe(false);

  expect(pending.map((p) => p.url)).toEqual(["mask", "a", "b"].map((name) => `/art/game/map/test/tex/${name}.webp?v=2`));
  for (const p of pending) p.texture.image = { width: 4, height: 4 };
  plasma.refresh();
  expect(material.visible).toBe(true);
  expect(plasma.animate(1)).toBe(true);
  const shader = {
    uniforms: {} as Record<string, THREE.IUniform>,
    vertexShader: THREE.ShaderLib.basic!.vertexShader,
    fragmentShader: THREE.ShaderLib.basic!.fragmentShader,
  };
  material.onBeforeCompile(shader as Parameters<typeof material.onBeforeCompile>[0], {} as THREE.WebGLRenderer);
  expect(shader.fragmentShader).toContain("texture2D(plasmaA");
  expect(shader.uniforms.plasmaUv!.value.map((v: THREE.Vector4) => v.toArray())).toEqual([
    [2, 3, 0.1, 0], [2, 3, 0, 0.2], [2, 3, 0.1, 0.2],
  ]);
  const a = shader.uniforms.plasmaA!.value as THREE.Texture;
  expect(a.colorSpace).toBe(THREE.NoColorSpace);
  plasma.setShown(false);
  expect(material.visible).toBe(false);
  expect(solid.material.visible).toBe(true);
  expect(plasma.animate(2)).toBe(false);
});
