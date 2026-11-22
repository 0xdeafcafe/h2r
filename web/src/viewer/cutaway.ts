// Seeing into a busy level: which of a map glb's meshes are level geometry,
// the height cut (nothing above a chosen height is drawn), and the
// see-through (level geometry between the camera and the player in focus is
// cut away). The parts that need no three.js, so they can be tested; the
// shader text here is what cutaway3d.ts patches into the scene materials.

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

/** A player-sized window projected from the eye. World axes are y-up. */
export interface SeeThrough {
  eye: Vec3;
  target: Vec3;
  radius: number;
  halfHeight: number;
}

/** A fragment is cut when its pixel ray crosses the player's ellipsoid
 *  and the fragment lies on the near side of that window. In ellipsoid
 *  space the radii become one, so the ray must pass within one unit of
 *  the centre and its closest approach must be past the fragment. This
 *  leaves room for a slab flush against the player while preserving the
 *  surfaces beyond them. The camera must be outside the ellipsoid. */
export function seeThroughHides(p: Vec3, s: SeeThrough): boolean {
  if (s.radius <= 0 || s.halfHeight <= 0) return false;
  const [ex, ey, ez] = s.eye;
  const [tx, ty, tz] = s.target;
  const x = (tx - ex) / s.radius, y = (ty - ey) / s.halfHeight, z = (tz - ez) / s.radius;
  if (x * x + y * y + z * z <= 1) return false;
  const dx = (p[0] - ex) / s.radius, dy = (p[1] - ey) / s.halfHeight, dz = (p[2] - ez) / s.radius;
  const raySq = dx * dx + dy * dy + dz * dz;
  const along = dx * x + dy * y + dz * z;
  if (raySq >= along) return false;
  const cx = dy * z - dz * y, cy = dz * x - dx * z, cz = dx * y - dy * x;
  return cx * cx + cy * cy + cz * cz <= raySq;
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
export function seeThroughHidesAny(p: Vec3, list: readonly SeeThrough[]): boolean {
  return list.some((s) => seeThroughHides(p, s));
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
uniform vec3 uSeeEye;
uniform vec4 uSeeTargets[${SEE_MAX}];
uniform float uSeeHalfHeights[${SEE_MAX}];
uniform int uSeeCount;
`;
/** Same ray test as seeThroughHides. Discarding only the occluding
 *  fragments opens the window in the colour and depth buffers together. */
export const SEE_FRAGMENT_BODY = /* glsl */ `
for (int seeI = 0; seeI < ${SEE_MAX}; seeI++) {
  if (seeI >= uSeeCount) break;
  vec4 seeTarget = uSeeTargets[seeI];
  float seeHeight = uSeeHalfHeights[seeI];
  if (seeTarget.w <= 0.0 || seeHeight <= 0.0) continue;
  vec3 seeInvSize = 1.0 / vec3(seeTarget.w, seeHeight, seeTarget.w);
  vec3 seeCentre = (seeTarget.xyz - uSeeEye) * seeInvSize;
  if (dot(seeCentre, seeCentre) <= 1.0) continue;
  vec3 seeRay = (vSeeWorld - uSeeEye) * seeInvSize;
  float seeRaySq = dot(seeRay, seeRay);
  if (seeRaySq >= dot(seeRay, seeCentre)) continue;
  vec3 seeCross = cross(seeRay, seeCentre);
  if (dot(seeCross, seeCross) <= seeRaySq) discard;
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
