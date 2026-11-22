// The opening sweep's timeline and shader hooks (docs/live/viewer-overlay.md).
// The scene advances the shared uniforms without changing material programs.

import { SEE_VERTEX_BODY, SEE_VERTEX_HEAD } from "./cutaway";

const SHELL_START_MS = 200;
const OBJECTS_START_MS = 380;
const HOLO_MS = 300;

export function revealDuration(reach: number): number {
  return Math.min(2400, Math.max(1300, 1100 + 9 * reach));
}

export function revealBand(reach: number): number {
  return Math.max(1.5, 0.06 * reach);
}

function progress(t: number): number {
  return Math.min(1, Math.max(0, t));
}

export function revealAt(ms: number, reach: number) {
  const duration = revealDuration(reach);
  const shell = progress((ms - SHELL_START_MS) / duration);
  const objects = progress((ms - OBJECTS_START_MS) / duration);
  const extent = reach + revealBand(reach);
  const done = objects === 1;
  return {
    holo: progress(ms / HOLO_MS),
    radius: done ? -1 : extent * (1 - (1 - shell) ** 3),
    objectsRadius: done ? -1 : extent * (1 - (1 - objects) ** 3),
    sky: progress((shell - 0.25) / (0.85 - 0.25)),
    shadows: progress((shell - 0.6) / (1 - 0.6)),
    done,
  };
}

const REVEAL_HEAD = /* glsl */ `
uniform vec4 uReveal;
uniform float uRevealBand;
uniform float uRevealCell;
uniform float uRevealHolo;
const vec3 REVEAL_TINT = vec3(0.56, 0.83, 1.0);
`;

// The radius is uniform across the draw, so derivatives in this branch
// are defined. Put it before the see-through's per-fragment discard.
const REVEAL_BODY = /* glsl */ `
float revealK = 1.0;
float revealLine = 0.0;
if (uReveal.w >= 0.0) {
  vec3 revealQ = vSeeWorld / uRevealCell;
  vec3 revealW = max(fwidth(revealQ), vec3(1e-5));
  vec3 revealN = normalize(cross(dFdx(vSeeWorld), dFdy(vSeeWorld)));
  vec3 revealG = abs(fract(revealQ - 0.5) - 0.5) / revealW + step(0.9, abs(revealN)) * 1e3;
  revealLine = 1.0 - clamp(min(min(revealG.x, revealG.y), revealG.z) - 0.5, 0.0, 1.0);
  revealK = 1.0 - smoothstep(uReveal.w - uRevealBand, uReveal.w, distance(vSeeWorld, uReveal.xyz));
  if (revealK <= 0.0 && revealLine * uRevealHolo < 0.5) discard;
}
`;

const REVEAL_COLOUR = /* glsl */ `
if (uReveal.w >= 0.0) {
  vec3 revealHolo = REVEAL_TINT * (0.35 + 0.65 * revealLine) * uRevealHolo;
  float revealEdge = revealK * (1.0 - revealK) * 4.0;
  gl_FragColor.rgb = mix(revealHolo, gl_FragColor.rgb, revealK) + REVEAL_TINT * revealEdge * 0.6;
}
`;

/** Add the sweep to built-in shaders, reusing the cutaway's world position.
 *  A normal pass needs the lattice's discard. A shadow pass needs only
 *  the solid part inside the band. */
export function patchReveal(vertex: string, fragment: string, pass: "colour" | "normal" | "shadow" = "colour"): { vertex: string; fragment: string } | null {
  const project = "#include <project_vertex>";
  const clipping = "#include <clipping_planes_fragment>";
  const dithering = "#include <dithering_fragment>";
  if (!vertex.includes(project) || !fragment.includes(clipping) || (pass === "colour" && !fragment.includes(dithering))) return null;
  const varying = SEE_VERTEX_HEAD.trim();
  if (!vertex.includes(varying)) vertex = SEE_VERTEX_HEAD + vertex.replace(project, project + SEE_VERTEX_BODY);
  if (!fragment.includes(varying)) fragment = SEE_VERTEX_HEAD + fragment;
  const body = pass === "shadow"
    ? "\nif (uReveal.w >= 0.0 && distance(vSeeWorld, uReveal.xyz) >= uReveal.w - uRevealBand) discard;\n"
    : REVEAL_BODY;
  fragment = REVEAL_HEAD + fragment.replace(clipping, clipping + body);
  if (pass === "colour") fragment = fragment.replace(dithering, REVEAL_COLOUR + dithering);
  return { vertex, fragment };
}
