// Seeing into a busy level: which of a map glb's meshes are level geometry,
// the height cut (nothing above a chosen height is drawn), and the
// see-through (level geometry between the camera and the player in focus is
// cut away). The parts that need no three.js, so they can be tested; the
// shader text here is what viewer/scene.ts patches into the map's materials.

import type { Vec3 } from "../map/views";

// ---- what a mesh is ----

/** `level`: the structure (one node, or several `bsp_<n>`), single-sided
 *  and facing into the playable space, so its back faces are culled and it
 *  can be seen into from outside. `machine`: placed device machines under a
 *  `machines` node; ordinary outward-facing models, left as the glb has
 *  them. `hidden`: collision geometry, never drawn. Both kinds that are
 *  drawn take the height cut; only the level takes the see-through
 *  (machines, like players and vehicles, are always drawn whole). */
export type MeshRole = "level" | "machine" | "hidden";

/** A mesh's role from its own and its ancestors' names, outermost first. */
export function meshRole(names: readonly string[]): MeshRole {
  if (names.some((n) => /^collision/i.test(n))) return "hidden";
  // Placed objects are drawn whole like machines.
  if (names.some((n) => /^(machines?|scenario)(_\d+)?$/i.test(n))) return "machine";
  return "level";
}

// ---- the height cut ----

/** Clear space kept above a followed player's feet (game units): their
 *  height and some headroom, so the room they are in keeps its walls. */
export const CUT_MARGIN = 1.35;

/** The heights a cut can be put at, for a map's z range: from a little
 *  above its lowest floor to its top (where nothing is cut). */
export function cutRange(minZ: number, maxZ: number): [number, number] {
  const lo = Math.min(minZ, maxZ), hi = Math.max(minZ, maxZ);
  return [lo + Math.min(0.5, (hi - lo) / 2), hi];
}

/** A cut height held inside the range. */
export function clampCut(h: number, range: readonly [number, number]): number {
  return Math.min(range[1], Math.max(range[0], h));
}

/** The next height one key press up (+1) or down (-1): a twentieth of the range. */
export function stepCut(h: number, dir: number, range: readonly [number, number]): number {
  return clampCut(h + (Math.sign(dir) * (range[1] - range[0])) / 20, range);
}

/** Where the cut is: null when off; the chosen height; or, following a
 *  player, their feet plus the margin (so it rises and falls with them). */
export function cutHeight(cut: { on: boolean; height: number }, followZ: number | null): number | null {
  if (!cut.on) return null;
  return followZ === null ? cut.height : followZ + CUT_MARGIN;
}

// ---- the see-through ----

/** How far round the player in focus is kept clear (game units), before
 *  the marker scale. */
export const SEE_RADIUS = 2.4;

export interface SeeThrough {
  eye: Vec3;
  /** The point kept in view: the player's chest. */
  target: Vec3;
  /** The clear radius at the target; 0 turns it off. */
  radius: number;
  /** Nothing at or below this height is cut: the floor they stand on stays. */
  floor: number;
}

/** Whether the see-through removes a point (all in one y-up space, as the
 *  shader sees them): it lies inside the cone from the eye to a disc of
 *  `radius` round the target, short of the target, above the floor.
 *  `rim` (0..1) is the dither value; the outer fifth of the cone thins out
 *  by it instead of ending in a hard edge. The shader below is this, line
 *  for line. */
export function seeThroughHides(p: Vec3, s: SeeThrough, rim = 1): boolean {
  if (!(s.radius > 0) || p[1] <= s.floor) return false;
  const ax: Vec3 = [s.target[0] - s.eye[0], s.target[1] - s.eye[1], s.target[2] - s.eye[2]];
  const len = Math.hypot(ax[0], ax[1], ax[2]);
  if (len < 1e-4) return false;
  const dir: Vec3 = [ax[0] / len, ax[1] / len, ax[2] / len];
  const rel: Vec3 = [p[0] - s.eye[0], p[1] - s.eye[1], p[2] - s.eye[2]];
  const t = rel[0] * dir[0] + rel[1] * dir[1] + rel[2] * dir[2];
  if (t <= 0 || t >= len - s.radius * 0.5) return false;
  const d = Math.hypot(rel[0] - dir[0] * t, rel[1] - dir[1] * t, rel[2] - dir[2] * t);
  return d < s.radius * (t / len) * (0.8 + 0.2 * rim);
}

/** At most this many players are seen through at once. */
export const SEE_MAX = 8;

/** Who the see-through cuts towards: nobody (`off`); the player in focus,
 *  else every player (`auto`); every player and the one in focus
 *  (`always`). */
export type SeeMode = "off" | "auto" | "always";

/** The players (by index) to cut towards, at most [`SEE_MAX`], the one in
 *  focus first. `players` are the alive ones in view, nearest the camera
 *  first. */
export function seeTargets(mode: SeeMode, focus: number | null, players: readonly number[]): number[] {
  if (mode === "off") return [];
  if (mode === "auto" && focus !== null) return [focus];
  const out = focus !== null ? [focus] : [];
  for (const p of players) if (out.length < SEE_MAX && !out.includes(p)) out.push(p);
  return out;
}

/** The mode the toolbar's key goes to next: off, auto, always, off. */
export function nextSee(m: SeeMode): SeeMode {
  return m === "off" ? "auto" : m === "auto" ? "always" : "off";
}

/** Whether any of several see-throughs removes a point. */
export function seeThroughHidesAny(p: Vec3, list: readonly SeeThrough[], rim = 1): boolean {
  return list.some((s) => seeThroughHides(p, s, rim));
}

export const SEE_VERTEX_HEAD = /* glsl */ `varying vec3 vSeeWorld;\n`;
/** After `project_vertex`, where `transformed` is final. */
export const SEE_VERTEX_BODY = /* glsl */ `
vSeeWorld = (modelMatrix * vec4(transformed, 1.0)).xyz;
`;
export const SEE_FRAGMENT_HEAD = /* glsl */ `
varying vec3 vSeeWorld;
uniform vec4 uSeeTargets[${SEE_MAX}];
uniform float uSeeFloors[${SEE_MAX}];
uniform int uSeeCount;
`;
/** After `clipping_planes_fragment`: [`seeThroughHides`] for each target
 *  (xyz the point kept in view, w the clear radius). */
export const SEE_FRAGMENT_BODY = /* glsl */ `
if (uSeeCount > 0) {
  float seeRim = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
  for (int seeI = 0; seeI < ${SEE_MAX}; seeI++) {
    if (seeI >= uSeeCount) break;
    vec4 seeTarget = uSeeTargets[seeI];
    if (seeTarget.w <= 0.0 || vSeeWorld.y <= uSeeFloors[seeI]) continue;
    vec3 seeAxis = seeTarget.xyz - cameraPosition;
    float seeLen = length(seeAxis);
    if (seeLen <= 1e-4) continue;
    vec3 seeDir = seeAxis / seeLen;
    vec3 seeRel = vSeeWorld - cameraPosition;
    float seeT = dot(seeRel, seeDir);
    if (seeT <= 0.0 || seeT >= seeLen - seeTarget.w * 0.5) continue;
    float seeD = length(seeRel - seeDir * seeT);
    if (seeD < seeTarget.w * (seeT / seeLen) * (0.8 + 0.2 * seeRim)) discard;
  }
}
`;

/** Adds the see-through to a built-in material's shaders. Returns null when
 *  the shader lacks the places it hooks into (then it is left alone). */
export function patchSeeThrough(vertex: string, fragment: string): { vertex: string; fragment: string } | null {
  const V = "#include <project_vertex>", F = "#include <clipping_planes_fragment>";
  if (!vertex.includes(V) || !fragment.includes(F)) return null;
  return {
    vertex: SEE_VERTEX_HEAD + vertex.replace(V, V + SEE_VERTEX_BODY),
    fragment: SEE_FRAGMENT_HEAD + fragment.replace(F, F + SEE_FRAGMENT_BODY),
  };
}
