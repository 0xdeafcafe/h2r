// The level's water in the 3D view. The map glb draws every water surface
// with one class material, `water` (a see-through blue-green): here it
// gets moving ripples (procedural: a few travelling waves in the ground
// plane, no pictures), a glassy sheen, and the sky's colour reflected at
// grazing angles (Fresnel), where it also turns opaque, as water does.
// `isWater`, `fresnel` and `rippleSlope` are pure.
//
// This file pulls in three.js, so it is only ever loaded lazily.

import * as THREE from "three";

/** Whether a glb material is the level's water. */
export function isWater(m: { name?: string } | null | undefined): boolean {
  return m?.name === "water";
}

/** Schlick's Fresnel term for water (F0 0.02) at `cos` = n·v. */
export function fresnel(cos: number): number {
  const c = Math.min(1, Math.max(0, cos));
  return 0.02 + 0.98 * Math.pow(1 - c, 5);
}

/** The waves: direction (x, z in the ground plane), wavelength and
 *  speed in world units, and height. Kept in step with the shader. */
export const WAVES: ReadonlyArray<{ dir: [number, number]; length: number; speed: number; height: number }> = [
  { dir: [1, 0.3], length: 2.1, speed: 0.55, height: 0.03 },
  { dir: [-0.4, 1], length: 1.3, speed: 0.4, height: 0.02 },
  { dir: [0.7, -0.8], length: 0.7, speed: 0.3, height: 0.012 },
  { dir: [-1, -0.2], length: 0.45, speed: 0.22, height: 0.007 },
];

/** The slope (d height / dx, d height / dz) of the waves at a point and
 *  time: what the shader bends the normal by. */
export function rippleSlope(x: number, z: number, t: number): [number, number] {
  let gx = 0, gz = 0;
  for (const w of WAVES) {
    const l = Math.hypot(w.dir[0], w.dir[1]);
    const dx = w.dir[0] / l, dz = w.dir[1] / l;
    const k = (2 * Math.PI) / w.length;
    const c = Math.cos(k * (dx * x + dz * z) - k * w.speed * t) * w.height * k;
    gx += c * dx;
    gz += c * dz;
  }
  return [gx, gz];
}

const glslWaves = WAVES.map((w) => {
  const l = Math.hypot(w.dir[0], w.dir[1]);
  const f = (v: number) => v.toFixed(5);
  return `  waterWave(p, t, vec2(${f(w.dir[0] / l)}, ${f(w.dir[1] / l)}), ${f(w.length)}, ${f(w.speed)}, ${f(w.height)}, g);`;
}).join("\n");

const VERTEX_HEAD = /* glsl */ `
varying vec3 vWaterWorld;
`;
const VERTEX_BODY = /* glsl */ `
vWaterWorld = (modelMatrix * vec4(transformed, 1.0)).xyz;
`;
const FRAGMENT_HEAD = /* glsl */ `
varying vec3 vWaterWorld;
uniform float uWaterTime;
uniform vec3 uWaterSky;
void waterWave(vec2 p, float t, vec2 d, float len, float speed, float h, inout vec2 g) {
  float k = 6.2831853 / len;
  float c = cos(k * dot(d, p) - k * speed * t) * h * k;
  g += c * d;
}
vec2 waterSlope(vec2 p, float t) {
  vec2 g = vec2(0.0);
${glslWaves}
  return g;
}
`;
// After the normal is known (view space): bend it by the waves' slope,
// on faces turned up (a pool's surface, not a waterfall's).
const FRAGMENT_NORMAL = /* glsl */ `
{
  vec3 up = normalize((viewMatrix * vec4(0.0, 1.0, 0.0, 0.0)).xyz);
  float flat_ = smoothstep(0.5, 0.9, dot(normal, up));
  vec2 g = waterSlope(vWaterWorld.xz, uWaterTime) * flat_;
  vec3 bend = (viewMatrix * vec4(-g.x, 0.0, -g.y, 0.0)).xyz;
  normal = normalize(normal + bend);
}
`;
// Before tone mapping: the sky's colour at grazing angles, and opaque there.
const FRAGMENT_OUT = /* glsl */ `
{
  float cosv = clamp(dot(normal, normalize(vViewPosition)), 0.0, 1.0);
  float f = 0.02 + 0.98 * pow(1.0 - cosv, 5.0);
  gl_FragColor.rgb = mix(gl_FragColor.rgb, uWaterSky, f * 0.85);
  gl_FragColor.a = mix(gl_FragColor.a, 1.0, f);
}
`;

/** The uniforms every water material shares: the time (seconds) and the
 *  colour it reflects (linear). */
export interface WaterUniforms {
  uWaterTime: { value: number };
  uWaterSky: { value: THREE.Color };
}

export function waterUniforms(): WaterUniforms {
  return { uWaterTime: { value: 0 }, uWaterSky: { value: new THREE.Color(0.55, 0.68, 0.8) } };
}

/** Makes a glb's water material ripple and reflect; chains after whatever
 *  `onBeforeCompile` it has. Leaves other materials alone; says whether
 *  it was water. */
export function dressWater(m: THREE.Material, u: WaterUniforms): boolean {
  if (!isWater(m) || !(m instanceof THREE.MeshStandardMaterial)) return false;
  if (m.userData.h2Water) return true;
  m.userData.h2Water = true;
  m.transparent = true;
  m.depthWrite = false;
  m.side = THREE.DoubleSide;
  m.roughness = 0.08;
  m.metalness = 0;
  m.opacity = Math.min(m.opacity, 0.72);
  m.envMapIntensity = 0.6;
  const before = m.onBeforeCompile, key = m.customProgramCacheKey;
  m.onBeforeCompile = (shader, renderer) => {
    before.call(m, shader, renderer);
    Object.assign(shader.uniforms, u);
    shader.vertexShader = VERTEX_HEAD + shader.vertexShader.replace("#include <worldpos_vertex>", "#include <worldpos_vertex>\n" + VERTEX_BODY);
    shader.fragmentShader = FRAGMENT_HEAD + shader.fragmentShader
      .replace("#include <normal_fragment_maps>", "#include <normal_fragment_maps>\n" + FRAGMENT_NORMAL)
      .replace("#include <tonemapping_fragment>", FRAGMENT_OUT + "\n#include <tonemapping_fragment>");
  };
  m.customProgramCacheKey = () => key.call(m) + "|h2water";
  m.needsUpdate = true;
  return true;
}
