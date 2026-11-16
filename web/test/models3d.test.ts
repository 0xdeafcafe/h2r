// The models' surfaces (change colour per pixel, glows, reflections), their
// aim, and the parts of the stale-art story the viewer owns.
import { describe, expect, test } from "bun:test";
import * as THREE from "three";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { MAX_TWIST, aimAngles, legTwist, wrapAngle } from "../src/viewer/anim";
import { secondaryOf } from "../src/viewer/models";
import { CC_FRAGMENT, ENV_FRAGMENT, MODEL_ENV_INTENSITY, ModelLibrary, applyLook, changeColourMaterial, cloneMaterial, resolveLooks } from "../src/viewer/players3d";
import { MOCK_BIPEDS, encodeModelGlb } from "../mock/models";

const parse = (bytes: Uint8Array) => new GLTFLoader().parseAsync(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer, "");
const settle = () => new Promise((r) => setTimeout(r, 20));

describe("change colour per pixel", () => {
  const base = () => { const m = new THREE.MeshStandardMaterial(); m.map = new THREE.Texture(); m.vertexColors = true; return m; };

  test("the copy tints its base colour by the map's red and green, in its own program", () => {
    const cc = new THREE.Texture();
    const out = changeColourMaterial(base(), cc, [1, 0, 0]) as THREE.MeshStandardMaterial;
    expect(out.vertexColors).toBe(false);
    expect(out.userData.cc).toEqual({ primary: [1, 0, 0], secondary: secondaryOf([1, 0, 0]) });
    expect(out.customProgramCacheKey()).toBe("h2-change-colour");
    const shader = { uniforms: {} as Record<string, { value: unknown }>, fragmentShader: "#include <common>\nvoid main() {\n#include <map_fragment>\n}", vertexShader: "" };
    out.onBeforeCompile(shader as never, null as never);
    expect(shader.uniforms.ccMap!.value).toBe(cc);
    expect((shader.uniforms.ccPrimary!.value as THREE.Color).toArray()).toEqual([1, 0, 0]);
    expect(shader.fragmentShader).toContain("uniform sampler2D ccMap;");
    // After the base map is read, so it tints the texture.
    expect(shader.fragmentShader.indexOf(CC_FRAGMENT)).toBeGreaterThan(shader.fragmentShader.indexOf("#include <map_fragment>"));
  });

  test("an environment-mapped look tints its reflection, and copies keep it", () => {
    const compile = (m: THREE.Material) => {
      const shader = { uniforms: {} as Record<string, THREE.IUniform>, fragmentShader: "#include <common>\n#include <map_fragment>\n#include <lights_fragment_end>", vertexShader: "" };
      m.onBeforeCompile(shader as unknown as THREE.WebGLProgramParametersWithUniforms, null as unknown as THREE.WebGLRenderer);
      return shader;
    };
    const covenant = new THREE.MeshStandardMaterial();
    covenant.userData = { h2: { env: { perpendicular: [0.9, 0.6, 0.3], parallel: [0.6, 0, 0.6] } } };
    applyLook(covenant, new THREE.Texture());
    applyLook(covenant, new THREE.Texture());
    const shader = compile(covenant);
    expect((shader.uniforms.envParallel!.value as THREE.Color).toArray()).toEqual([0.6, 0, 0.6]);
    // Once, before the lights are summed.
    expect(shader.fragmentShader.split(ENV_FRAGMENT).length).toBe(2);
    expect(shader.fragmentShader.indexOf(ENV_FRAGMENT)).toBeLessThan(shader.fragmentShader.indexOf("#include <lights_fragment_end>"));
    // A copy (empty vehicle, a player's colours) keeps the tint, and adds its own.
    expect(compile(cloneMaterial(covenant)).fragmentShader).toContain(ENV_FRAGMENT);
    const tinted = changeColourMaterial(covenant, new THREE.Texture(), [1, 0, 0]);
    const both = compile(tinted);
    expect(both.fragmentShader).toContain(ENV_FRAGMENT);
    expect(both.fragmentShader).toContain(CC_FRAGMENT);
    expect(tinted.customProgramCacheKey()).toBe("h2-env|h2-change-colour");
    // A plain look has no tint.
    const plain = new THREE.MeshStandardMaterial();
    applyLook(plain, null);
    expect(compile(plain).fragmentShader).not.toContain(ENV_FRAGMENT);
  });

  test("glows add, glass doesn't hide what's behind, everything reflects the environment", () => {
    const env = new THREE.Texture();
    const glow = new THREE.MeshStandardMaterial();
    glow.userData = { h2: { blend: "add" } };
    applyLook(glow, env);
    expect(glow.blending).toBe(THREE.AdditiveBlending);
    expect(glow.depthWrite).toBe(false);
    expect(glow.envMap).toBe(env);
    expect(glow.envMapIntensity).toBe(MODEL_ENV_INTENSITY);
    const glass = new THREE.MeshStandardMaterial({ transparent: true });
    applyLook(glass, null);
    expect(glass.depthWrite).toBe(false);
    expect(glass.blending).toBe(THREE.NormalBlending);
  });

  test("a glb whose material names a change-colour map: copies take the player's colour per pixel", async () => {
    const g = await parse(encodeModelGlb(MOCK_BIPEDS.spartan!()));
    // As the extractor writes it: the map's texture index in extras.h2.
    let armour: THREE.MeshStandardMaterial | null = null;
    g.scene.traverse((o) => { const m = o as THREE.Mesh; if (m.isMesh && !armour) { armour = m.material as THREE.MeshStandardMaterial; } });
    armour!.userData = { h2: { change_colour: 0 } };
    armour!.map = new THREE.Texture();
    const ccTex = new THREE.Texture();
    const loaded = { ...g, parser: { getDependency: async (_: "texture", i: number) => { expect(i).toBe(0); return ccTex; } } };
    const lib = new ModelLibrary(() => {}, async () => loaded);
    lib.biped("a");
    await settle();
    const t = lib.biped("a")!;
    expect(armour!.userData.ccMap).toBe(ccTex);
    expect(ccTex.colorSpace).toBe(THREE.NoColorSpace);
    const red = lib.instantiateBiped(t, "#ff0000"), blue = lib.instantiateBiped(t, "#0000ff");
    const mats = (o: THREE.Object3D) => { const out: THREE.Material[] = []; o.traverse((c) => { const m = c as THREE.Mesh; if (m.isMesh) out.push(m.material as THREE.Material); }); return out; };
    const tinted = mats(red.object).find((m) => m.userData.cc)!;
    expect(tinted.userData.cc.primary).toEqual([1, 0, 0]);
    expect(mats(blue.object).find((m) => m.userData.cc)!.userData.cc.primary).toEqual([0, 0, 1]);
    // Team mates share one copy.
    expect(mats(lib.instantiateBiped(t, "#ff0000").object).find((m) => m.userData.cc)).toBe(tinted);
  });

  test("a change-colour map that can't be read leaves the vertex weights to colour it", async () => {
    const g = await parse(encodeModelGlb(MOCK_BIPEDS.spartan!()));
    g.scene.traverse((o) => { const m = o as THREE.Mesh; if (m.isMesh) (m.material as THREE.Material).userData = { h2: { change_colour: 3 } }; });
    await resolveLooks({ ...g, parser: { getDependency: () => Promise.reject(new Error("no image")) } });
    let any = false;
    g.scene.traverse((o) => { const m = o as THREE.Mesh; if (m.isMesh && (m.material as THREE.Material).userData.ccMap) any = true; });
    expect(any).toBe(false);
  });
});

describe("looking where the player looks", () => {
  test("aim angles and how far the legs turn to follow the movement", () => {
    expect(aimAngles([1, 0, 0])).toEqual({ yaw: 0, pitch: 0 });
    const up = aimAngles([0, 1, 1]);
    expect(up.yaw).toBeCloseTo(Math.PI / 2, 9);
    expect(up.pitch).toBeCloseTo(Math.PI / 4, 9);
    expect(wrapAngle(3 * Math.PI / 2)).toBeCloseTo(-Math.PI / 2, 9);
    // Standing: legs face the aim. Strafing a little: legs follow.
    expect(legTwist(0, null)).toBe(0);
    expect(legTwist(0, 0.4)).toBeCloseTo(0.4, 9);
    // Strafing hard: held to the reach. Backing up: the legs face the aim, not behind.
    expect(legTwist(0, Math.PI / 2 - 0.01)).toBe(MAX_TWIST);
    expect(legTwist(0, Math.PI)).toBeCloseTo(0, 9);
    expect(legTwist(0, Math.PI - 0.3)).toBeCloseTo(-0.3, 9);
  });

  const headForward = (object: THREE.Object3D, head: THREE.Object3D) => {
    object.updateWorldMatrix(true, true);
    return new THREE.Vector3(1, 0, 0).applyQuaternion(head.getWorldQuaternion(new THREE.Quaternion()));
  };

  test("aiming up pitches the head and spine up; the clip's own pose comes back when it stops", async () => {
    const bytes = encodeModelGlb(MOCK_BIPEDS.spartan!());
    const lib = new ModelLibrary(() => {}, () => parse(bytes));
    lib.biped("a");
    await settle();
    const t = lib.biped("a")!;
    const inst = lib.instantiateBiped(t, "#3a7bd5");
    const an = inst.animator!;
    an.setLoop("rifle:idle");
    an.update(16);
    const level = headForward(inst.object, inst.head).y;
    an.aim(0.6, 0);
    an.update(16);
    const up = headForward(inst.object, inst.head).y;
    // The whole pitch reaches the head (it rides on the spine).
    expect(Math.asin(up) - Math.asin(level)).toBeCloseTo(0.6, 2);
    // Down is down.
    an.aim(-0.6, 0);
    an.update(16);
    expect(headForward(inst.object, inst.head).y).toBeLessThan(level - 0.3);
    // Many frames of the same aim don't pile up (the idle clip itself
    // sways the head a little meanwhile).
    an.aim(0.6, 0);
    for (let i = 0; i < 30; i++) an.update(16);
    expect(headForward(inst.object, inst.head).y).toBeCloseTo(up, 1);
    an.aim(0, 0);
    an.update(16);
    expect(headForward(inst.object, inst.head).y).toBeCloseTo(level, 1);
  });

  test("a twist turns the upper body back to the aim", async () => {
    const bytes = encodeModelGlb(MOCK_BIPEDS.spartan!());
    const lib = new ModelLibrary(() => {}, () => parse(bytes));
    lib.biped("a");
    await settle();
    const inst = lib.instantiateBiped(lib.biped("a")!, "#3a7bd5");
    const an = inst.animator!;
    an.setLoop("rifle:idle");
    an.update(16);
    const before = headForward(inst.object, inst.head);
    an.aim(0, 0.8);
    an.update(16);
    const after = headForward(inst.object, inst.head);
    const turned = Math.atan2(-after.z, after.x) - Math.atan2(-before.z, before.x);
    expect(turned).toBeCloseTo(0.8, 2);
  });
});
