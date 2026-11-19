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
 *  `machines` node, and the scenario's placed objects; ordinary
 *  outward-facing models, left as the glb has them. `hidden`: collision
 *  geometry, never drawn. Both kinds that are drawn take the height cut and
 *  the see-through alike ([`takesSeeThrough`]). */
export type MeshRole = "level" | "machine" | "hidden";

/** A mesh's role from its own and its ancestors' names, outermost first. */
export function meshRole(names: readonly string[]): MeshRole {
  if (names.some((n) => /^collision/i.test(n))) return "hidden";
  // Placed objects are drawn whole like machines.
  if (names.some((n) => /^(machines?|scenario)(_\d+)?$/i.test(n))) return "machine";
  return "level";
}

/** Whether a drawn mesh of the map takes the see-through. The level, its
 *  machines (doors, lifts, platforms) and the scenario's placed objects
 *  are all cut the same way, in world space, so a lift or a crate in front
 *  of a player goes with the wall behind it instead of standing whole in
 *  the hole. A vehicle placement is the exception (`vehicle`): a player
 *  may be riding it, and the frames' own vehicles and the players are
 *  never cut either. */
export function takesSeeThrough(role: MeshRole, vehicle = false): boolean {
  return role !== "hidden" && !vehicle;
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

/** How far behind a surface (game units, along its normal) the point kept
 *  in view has to be before the surface counts as in the way. Keeps
 *  ground that the line of sight only grazes, and stops a surface nearly
 *  edge-on to the line from flickering as the camera moves. */
export const SEE_BEHIND = 0.15;

/** A surface whose normal is at least this close to straight up or down
 *  (|y|) is a floor, terrain or ceiling: it is taken as level, so a bump
 *  in the ground tilted towards the camera doesn't count as rising over
 *  the player just because its slope, carried on, would. */
export const SEE_LEVEL = 0.7;

/** Slack (game units) on how near the point the line of sight has to
 *  cross its surface, over the cone's radius there. */
export const SEE_NEAR = 0.5;
/** The shallowest the line of sight is taken to meet a surface at (the
 *  cosine to its normal) when sizing the cone's footprint on it. */
export const SEE_GRAZE = 0.15;

/** Whether the see-through removes a point (all in one y-up space, as the
 *  shader sees them). The point lies inside the cone from the eye to a disc
 *  of `radius` round the target, short of the target and above the floor;
 *  and its surface actually stands in the way: the line of sight passes
 *  through the surface's plane (the target more than [`SEE_BEHIND`] behind
 *  it), near the point. Ground under the line, the floor round a player
 *  and a wall running beside the line all have the target in front of
 *  them, so they stay. `normal` is the surface's, either way round (the
 *  face towards the eye when left out); one within [`SEE_LEVEL`] of
 *  vertical is taken as level. The cone ends in a clean edge: it used to
 *  thin out through a per-pixel dither, which speckled the rim, crawled as
 *  the camera moved and, read back from the depth, freckled the occlusion
 *  along it. The shader below is this, line for line. */
export function seeThroughHides(p: Vec3, s: SeeThrough, normal?: Vec3): boolean {
  if (!(s.radius > 0) || p[1] <= s.floor) return false;
  const ax: Vec3 = [s.target[0] - s.eye[0], s.target[1] - s.eye[1], s.target[2] - s.eye[2]];
  const len = Math.hypot(ax[0], ax[1], ax[2]);
  if (len < 1e-4) return false;
  const dir: Vec3 = [ax[0] / len, ax[1] / len, ax[2] / len];
  const rel: Vec3 = [p[0] - s.eye[0], p[1] - s.eye[1], p[2] - s.eye[2]];
  const t = rel[0] * dir[0] + rel[1] * dir[1] + rel[2] * dir[2];
  if (t <= 0 || t >= len - s.radius * 0.5) return false;
  const d = Math.hypot(rel[0] - dir[0] * t, rel[1] - dir[1] * t, rel[2] - dir[2] * t);
  if (!(d < s.radius * (t / len))) return false;
  // The surface's normal, turned to face the eye; level when near vertical.
  let n: Vec3 = normal ?? [-dir[0], -dir[1], -dir[2]];
  const nl = Math.hypot(n[0], n[1], n[2]);
  if (!(nl > 1e-9)) return false;
  n = [n[0] / nl, n[1] / nl, n[2] / nl];
  if (n[0] * -rel[0] + n[1] * -rel[1] + n[2] * -rel[2] < 0) n = [-n[0], -n[1], -n[2]];
  if (Math.abs(n[1]) > SEE_LEVEL) n = [0, Math.sign(n[1]), 0];
  // Which side of the surface the eye and the target are on; the line
  // crosses it at `u` of the way along, `x` from the point.
  const eyeSide = -(rel[0] * n[0] + rel[1] * n[1] + rel[2] * n[2]);
  const behind = (s.target[0] - p[0]) * n[0] + (s.target[1] - p[1]) * n[1] + (s.target[2] - p[2]) * n[2];
  if (!(eyeSide > 0) || !(behind < -SEE_BEHIND)) return false;
  const u = eyeSide / (eyeSide - behind);
  const x: Vec3 = [s.eye[0] + ax[0] * u - p[0], s.eye[1] + ax[1] * u - p[1], s.eye[2] + ax[2] * u - p[2]];
  // The cone's footprint on the surface: its radius there, stretched by
  // how slantwise the line meets it (capped for a line that only grazes).
  const slant = Math.max(SEE_GRAZE, Math.abs(dir[0] * n[0] + dir[1] * n[1] + dir[2] * n[2]));
  return Math.hypot(x[0], x[1], x[2]) < (s.radius * u) / slant + SEE_NEAR;
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
export function seeThroughHidesAny(p: Vec3, list: readonly SeeThrough[], normal?: Vec3): boolean {
  return list.some((s) => seeThroughHides(p, s, normal));
}

export const SEE_VERTEX_HEAD = /* glsl */ `varying vec3 vSeeWorld;\n`;
/** After `project_vertex`, where `transformed` is final: the point in
 *  world space as `project_vertex` places it, through an instanced
 *  batch's per-instance matrix (the scenario's placed objects are drawn
 *  that way) and a batched mesh's, so an instance is cut where it stands
 *  and not where its model sits at the origin. */
export const SEE_VERTEX_BODY = /* glsl */ `
vec4 seeLocal = vec4(transformed, 1.0);
#ifdef USE_BATCHING
seeLocal = batchingMatrix * seeLocal;
#endif
#ifdef USE_INSTANCING
seeLocal = instanceMatrix * seeLocal;
#endif
vSeeWorld = (modelMatrix * seeLocal).xyz;
`;
export const SEE_FRAGMENT_HEAD = /* glsl */ `
varying vec3 vSeeWorld;
uniform vec4 uSeeTargets[${SEE_MAX}];
uniform float uSeeFloors[${SEE_MAX}];
uniform int uSeeCount;
`;
/** After `clipping_planes_fragment`: [`seeThroughHides`] for each target
 *  (xyz the point kept in view, w the clear radius). The surface's normal
 *  is the triangle's own, from the screen-space change of its world
 *  position (taken before any branch, as derivatives need), turned to
 *  face the camera. */
export const SEE_FRAGMENT_BODY = /* glsl */ `
vec3 seeFace = cross(dFdx(vSeeWorld), dFdy(vSeeWorld));
if (uSeeCount > 0) {
  float seeFaceLen = length(seeFace);
  vec3 seeN = seeFaceLen > 1e-12 ? seeFace / seeFaceLen : vec3(0.0);
  if (dot(seeN, cameraPosition - vSeeWorld) < 0.0) seeN = -seeN;
  if (abs(seeN.y) > ${SEE_LEVEL.toFixed(2)}) seeN = vec3(0.0, sign(seeN.y), 0.0);
  float seeEyeSide = dot(cameraPosition - vSeeWorld, seeN);
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
    if (!(seeD < seeTarget.w * (seeT / seeLen))) continue;
    if (seeFaceLen <= 1e-12) continue;
    float seeBehind = dot(seeTarget.xyz - vSeeWorld, seeN);
    if (!(seeEyeSide > 0.0) || !(seeBehind < -${SEE_BEHIND.toFixed(2)})) continue;
    float seeU = seeEyeSide / (seeEyeSide - seeBehind);
    float seeSlant = max(${SEE_GRAZE.toFixed(2)}, abs(dot(seeDir, seeN)));
    if (length(cameraPosition + seeAxis * seeU - vSeeWorld) < seeTarget.w * seeU / seeSlant + ${SEE_NEAR.toFixed(2)}) discard;
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
