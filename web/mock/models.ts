// Stand-in player and weapon models for the mock (H2_MOCK_MODELS=1), in the
// encoding the model contract describes, so the 3D view's model path can be
// run and tested with nothing from the game: block figures made of boxes.
//
// A biped stands on the origin facing game +X (glTF +X), has empty `weapon`
// and `head` nodes, and a COLOR_0 whose R is the primary change-colour
// weight and G the secondary. A weapon has an empty `muzzle` node. Vertices
// are written in glTF axes: game (x, y, z) → (x, z, -y).
//
// A biped is skinned (models version 2): each box hangs rigidly off one
// joint of a small skeleton, and the glb carries clips named
// `<weapon_class>:<state>` that swing those joints, so the 3D view's
// animation can be run and tested too. With H2_MOCK_MODELS=static the
// bipeds are the plain version 1 kind instead.

type V3 = [number, number, number];

export interface ModelBox {
  /** Game coordinates. */
  min: V3; max: V3;
  material: number;
  /** [primary, secondary] change-colour weights; absent for none. */
  tint?: [number, number];
  /** The joint it moves with, when the model has a rig. */
  joint?: string;
}

type Quat = [number, number, number, number];

export interface ModelRig {
  /** Joints, parents before children, at game positions in the bind pose. */
  joints: Array<{ name: string; parent: string | null; at: V3 }>;
  /** Which joint each empty node (`weapon`, `head`) rides on. */
  attach: Record<string, string>;
  clips: Array<{
    name: string;
    /** Key times in seconds, shared by every track. */
    times: number[];
    /** Per joint: a rotation (glTF axes) per key, and/or an offset from the bind position (game axes) per key. */
    tracks: Array<{ joint: string; rotations?: Quat[]; offsets?: V3[] }>;
  }>;
}

export interface ModelSpec {
  boxes: ModelBox[];
  materials: Array<{ name: string; colour: [number, number, number]; metallic: number; roughness: number }>;
  /** Empty nodes, by name, at a game position. */
  nodes: Record<string, V3>;
  /** Whether to write COLOR_0 (a model without it is tinted whole). */
  colours: boolean;
  extras: Record<string, unknown>;
  rig?: ModelRig;
}

const gl = (v: V3): V3 => [v[0], v[2], -v[1]];

/** A model as a binary glTF: one mesh node (a primitive per material) and
 *  the empty nodes. */
export function encodeModelGlb(spec: ModelSpec): Uint8Array {
  const chunks: Uint8Array[] = [];
  let offset = 0;
  const bufferViews: unknown[] = [], accessors: unknown[] = [], primitives: unknown[] = [];
  const rig = spec.rig;
  const jointIndex = new Map((rig?.joints ?? []).map((j, i) => [j.name, i]));
  const push = (data: Float32Array | Uint32Array | Uint16Array, target?: number) => {
    const bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: bytes.length, ...(target ? { target } : {}) });
    chunks.push(bytes);
    offset += bytes.length;
    return bufferViews.length - 1;
  };
  // A box's six faces: normal, then four corners counter-clockwise seen from outside (game axes).
  const FACES: Array<[V3, [number, number, number][]]> = [
    [[1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]],
    [[-1, 0, 0], [[0, 1, 0], [0, 0, 0], [0, 0, 1], [0, 1, 1]]],
    [[0, 1, 0], [[1, 1, 0], [0, 1, 0], [0, 1, 1], [1, 1, 1]]],
    [[0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]],
    [[0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]],
    [[0, 0, -1], [[0, 1, 0], [1, 1, 0], [1, 0, 0], [0, 0, 0]]],
  ];
  spec.materials.forEach((_, mi) => {
    const boxes = spec.boxes.filter((b) => b.material === mi);
    if (!boxes.length) return;
    const n = boxes.length * 24;
    const pos = new Float32Array(n * 3), nor = new Float32Array(n * 3), col = new Float32Array(n * 4), idx = new Uint32Array(boxes.length * 36);
    const bone = new Uint16Array(n * 4), weight = new Float32Array(n * 4);
    const min: V3 = [Infinity, Infinity, Infinity], max: V3 = [-Infinity, -Infinity, -Infinity];
    let v = 0, i = 0;
    for (const b of boxes) {
      for (const [normal, corners] of FACES) {
        const nn = gl(normal);
        for (const c of corners) {
          const p = gl([c[0] ? b.max[0] : b.min[0], c[1] ? b.max[1] : b.min[1], c[2] ? b.max[2] : b.min[2]]);
          for (let k = 0; k < 3; k++) {
            pos[v * 3 + k] = p[k]!; nor[v * 3 + k] = nn[k]!;
            if (p[k]! < min[k]!) min[k] = p[k]!;
            if (p[k]! > max[k]!) max[k] = p[k]!;
          }
          bone[v * 4] = jointIndex.get(b.joint ?? "") ?? 0; weight[v * 4] = 1;
          col[v * 4] = b.tint?.[0] ?? 0; col[v * 4 + 1] = b.tint?.[1] ?? 0; col[v * 4 + 2] = 0; col[v * 4 + 3] = 1;
          v++;
        }
        idx.set([v - 4, v - 3, v - 2, v - 4, v - 2, v - 1], i);
        i += 6;
      }
    }
    const first = accessors.length;
    accessors.push(
      { bufferView: push(pos, 34962), componentType: 5126, count: n, type: "VEC3", min: Array.from(new Float32Array(min)), max: Array.from(new Float32Array(max)) },
      { bufferView: push(nor, 34962), componentType: 5126, count: n, type: "VEC3" },
    );
    const attributes: Record<string, number> = { POSITION: first, NORMAL: first + 1 };
    if (spec.colours) {
      attributes.COLOR_0 = accessors.length;
      accessors.push({ bufferView: push(col, 34962), componentType: 5126, count: n, type: "VEC4" });
    }
    if (rig) {
      attributes.JOINTS_0 = accessors.length;
      accessors.push({ bufferView: push(bone, 34962), componentType: 5123, count: n, type: "VEC4" });
      attributes.WEIGHTS_0 = accessors.length;
      accessors.push({ bufferView: push(weight, 34962), componentType: 5126, count: n, type: "VEC4" });
    }
    accessors.push({ bufferView: push(idx, 34963), componentType: 5125, count: idx.length, type: "SCALAR" });
    primitives.push({ attributes, indices: accessors.length - 1, material: mi, mode: 4 });
  });
  // Nodes: the mesh, then the joints (each placed relative to its parent),
  // then the empties, each under the joint it rides on or at the root.
  const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
  const joints = rig?.joints ?? [];
  const jointAt = (name: string | null): V3 => joints.find((j) => j.name === name)?.at ?? [0, 0, 0];
  type Node = { name: string; mesh?: number; skin?: number; translation?: V3; children?: number[] };
  const nodes: Node[] = [{ name: "body", mesh: 0, ...(rig ? { skin: 0 } : {}) }];
  const roots = [0];
  const child = (parent: string | null | undefined, index: number) => {
    const p = parent ? jointIndex.get(parent) : undefined;
    if (p === undefined) roots.push(index);
    else (nodes[1 + p]!.children ??= []).push(index);
  };
  for (const j of joints) { nodes.push({ name: j.name, translation: gl(sub(j.at, jointAt(j.parent))) }); child(j.parent, nodes.length - 1); }
  for (const [name, where] of Object.entries(spec.nodes)) {
    const on = rig?.attach[name];
    nodes.push({ name, translation: gl(on ? sub(where, jointAt(on)) : where) });
    child(on, nodes.length - 1);
  }
  let skins: unknown[] | undefined, animations: unknown[] | undefined;
  if (rig) {
    // No joint is turned in the bind pose, so each inverse bind matrix only moves back to the joint.
    const inv = new Float32Array(joints.length * 16);
    joints.forEach((j, i) => { const p = gl(j.at); inv.set([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, -p[0], -p[1], -p[2], 1], i * 16); });
    accessors.push({ bufferView: push(inv), componentType: 5126, count: joints.length, type: "MAT4" });
    skins = [{ joints: joints.map((_, i) => 1 + i), inverseBindMatrices: accessors.length - 1 }];
    animations = rig.clips.map((clip) => {
      const times = new Float32Array(clip.times);
      accessors.push({ bufferView: push(times), componentType: 5126, count: times.length, type: "SCALAR", min: [Math.min(...times)], max: [Math.max(...times)] });
      const input = accessors.length - 1;
      const samplers: unknown[] = [], channels: unknown[] = [];
      const track = (joint: string, path: string, type: string, data: number[]) => {
        const node = jointIndex.get(joint);
        if (node === undefined) return;
        accessors.push({ bufferView: push(new Float32Array(data)), componentType: 5126, count: times.length, type });
        samplers.push({ input, output: accessors.length - 1, interpolation: "LINEAR" });
        channels.push({ sampler: samplers.length - 1, target: { node: 1 + node, path } });
      };
      for (const t of clip.tracks) {
        if (t.rotations) track(t.joint, "rotation", "VEC4", t.rotations.flat());
        if (t.offsets) {
          const j = joints.find((q) => q.name === t.joint);
          if (j) { const rest = sub(j.at, jointAt(j.parent)); track(t.joint, "translation", "VEC3", t.offsets.flatMap((o) => gl([rest[0] + o[0], rest[1] + o[1], rest[2] + o[2]]))); }
        }
      }
      return { name: clip.name, samplers, channels };
    });
  }
  const json = {
    asset: { version: "2.0", generator: "h2 live mock", extras: { generator: "h2 live mock", models_version: rig ? 2 : 1, ...spec.extras } },
    scene: 0,
    scenes: [{ nodes: roots }],
    nodes,
    meshes: [{ name: "body", primitives }],
    materials: spec.materials.map((m) => ({ name: m.name, pbrMetallicRoughness: { baseColorFactor: [...m.colour, 1], metallicFactor: m.metallic, roughnessFactor: m.roughness } })),
    ...(skins ? { skins, animations } : {}),
    buffers: [{ byteLength: offset }],
    bufferViews, accessors,
  };
  let text = new TextEncoder().encode(JSON.stringify(json));
  if (text.length % 4) { const padded = new Uint8Array(text.length + 4 - (text.length % 4)).fill(0x20); padded.set(text); text = padded; }
  const total = 12 + 8 + text.length + 8 + offset;
  const out = new Uint8Array(total);
  const dv = new DataView(out.buffer);
  dv.setUint32(0, 0x46546c67, true); dv.setUint32(4, 2, true); dv.setUint32(8, total, true);
  dv.setUint32(12, text.length, true); dv.setUint32(16, 0x4e4f534a, true);
  out.set(text, 20);
  let at = 20 + text.length;
  dv.setUint32(at, offset, true); dv.setUint32(at + 4, 0x004e4942, true);
  at += 8;
  for (const c of chunks) { out.set(c, at); at += c.length; }
  return out;
}

const box = (cx: number, cy: number, z0: number, sx: number, sy: number, sz: number, material: number, tint?: [number, number], joint?: string): ModelBox =>
  ({ min: [cx - sx / 2, cy - sy / 2, z0], max: [cx + sx / 2, cy + sy / 2, z0 + sz], material, tint, joint });

/** A turn of `angle` radians about a game axis, as a glTF quaternion. */
const turn = (axis: V3, angle: number): Quat => {
  const a = gl(axis), s = Math.sin(angle / 2);
  return [a[0] * s, a[1] * s, a[2] * s, Math.cos(angle / 2)];
};
// Game axes: Y is to the figure's left, so a turn about it swings a limb
// forward (negative) or back (positive); Z is up, so a turn about it twists.
const pitch = (...angles: number[]) => angles.map((a) => turn([0, 1, 0], a));
const twist = (...angles: number[]) => angles.map((a) => turn([0, 0, 1], a));
const even = (seconds: number, keys: number) => Array.from({ length: keys }, (_, i) => +((seconds * i) / (keys - 1)).toFixed(4));

/** The stand-in figure's clips: enough of each state to tell them apart at
 *  a glance. `u` is an eighth of its height. */
function figureClips(u: number): ModelRig["clips"] {
  const stride = (cls: string, state: string, seconds: number, swing: number, knee: number, lean: number): ModelRig["clips"][number] => ({
    name: `${cls}:${state}`, times: even(seconds, 5),
    tracks: [
      { joint: "thigh_l", rotations: pitch(-swing, 0, swing, 0, -swing) },
      { joint: "thigh_r", rotations: pitch(swing, 0, -swing, 0, swing) },
      { joint: "shin_l", rotations: pitch(0, 0, knee, knee * 1.4, 0) },
      { joint: "shin_r", rotations: pitch(knee, knee * 1.4, 0, 0, knee) },
      { joint: "spine", rotations: pitch(-lean, -lean * 1.2, -lean, -lean * 1.2, -lean) },
      { joint: "pelvis", offsets: [[0, 0, 0], [0, 0, 0.12 * u], [0, 0, 0], [0, 0, 0.12 * u], [0, 0, 0]] },
    ],
  });
  const still = (name: string, seconds: number, tracks: ModelRig["clips"][number]["tracks"]): ModelRig["clips"][number] => ({ name, times: even(seconds, 3), tracks });
  const rifle: ModelRig["clips"] = [
    still("rifle:idle", 2.4, [{ joint: "spine", rotations: pitch(0, -0.04, 0) }, { joint: "neck", rotations: twist(0, 0.12, 0) }]),
    stride("rifle", "walk", 1, 0.42, 0.45, 0.04),
    stride("rifle", "run", 0.62, 0.85, 0.9, 0.2),
    still("rifle:crouch", 2, [
      { joint: "pelvis", offsets: [[0, 0, -1.1 * u], [0, 0, -1.15 * u], [0, 0, -1.1 * u]] },
      { joint: "thigh_l", rotations: pitch(-1, -1, -1) }, { joint: "thigh_r", rotations: pitch(-1.1, -1.1, -1.1) },
      { joint: "shin_l", rotations: pitch(1.6, 1.6, 1.6) }, { joint: "shin_r", rotations: pitch(1.7, 1.7, 1.7) },
      { joint: "spine", rotations: pitch(-0.22, -0.25, -0.22) },
    ]),
    still("rifle:airborne", 1, [
      { joint: "thigh_l", rotations: pitch(-0.7, -0.6, -0.7) }, { joint: "thigh_r", rotations: pitch(-0.25, -0.35, -0.25) },
      { joint: "shin_l", rotations: pitch(1.1, 1, 1.1) }, { joint: "shin_r", rotations: pitch(0.7, 0.8, 0.7) },
      { joint: "arm_l", rotations: pitch(-0.5, -0.6, -0.5) },
    ]),
    still("rifle:land", 0.3, [
      { joint: "pelvis", offsets: [[0, 0, 0], [0, 0, -0.7 * u], [0, 0, 0]] },
      { joint: "thigh_l", rotations: pitch(0, -0.7, 0) }, { joint: "thigh_r", rotations: pitch(0, -0.7, 0) },
      { joint: "shin_l", rotations: pitch(0, 1.1, 0) }, { joint: "shin_r", rotations: pitch(0, 1.1, 0) },
    ]),
    { name: "rifle:grenade", times: [0, 0.3, 0.5, 0.9], tracks: [
      // The left arm goes up and back, then whips forward.
      { joint: "arm_l", rotations: pitch(0, -2.6, -0.5, 0) },
      { joint: "spine", rotations: twist(0, 0.35, -0.3, 0) },
    ] },
    { name: "rifle:melee", times: [0, 0.15, 0.3, 0.6], tracks: [
      { joint: "arm_r", rotations: twist(0, -0.9, 0.8, 0) },
      { joint: "spine", rotations: twist(0, -0.5, 0.6, 0) },
    ] },
    { name: "rifle:reload", times: [0, 0.3, 1.2, 1.5], tracks: [
      { joint: "arm_r", rotations: pitch(0, 0.5, 0.5, 0) },
      { joint: "arm_l", rotations: twist(0, -0.5, -0.6, 0) },
      { joint: "neck", rotations: pitch(0, 0.3, 0.3, 0) },
    ] },
    { name: "rifle:fire", times: [0, 0.05, 0.18], tracks: [{ joint: "arm_r", rotations: pitch(0, -0.22, 0) }, { joint: "arm_l", rotations: pitch(0, -0.18, 0) }] },
    { name: "rifle:switch", times: [0, 0.25, 0.6], tracks: [{ joint: "arm_r", rotations: pitch(0, 1.3, 0) }] },
    { name: "rifle:die", times: [0, 0.5, 1.1], tracks: [
      { joint: "pelvis", rotations: pitch(0, 0.9, 1.5), offsets: [[0, 0, 0], [-1 * u, 0, -1.4 * u], [-2 * u, 0, -3 * u]] },
      { joint: "arm_l", rotations: pitch(0, -1, -1.5) }, { joint: "arm_r", rotations: pitch(0, -0.8, -1.2) },
    ] },
  ];
  // One-handed: the left arm hangs. Only what differs from the rifle's; the rest falls back to it.
  const hang = { joint: "arm_l", rotations: pitch(1.35, 1.3, 1.35) };
  return [
    ...rifle,
    still("pistol:idle", 2.4, [hang, { joint: "neck", rotations: twist(0, 0.12, 0) }]),
    { ...stride("pistol", "walk", 1, 0.42, 0.45, 0.04), tracks: [...stride("pistol", "walk", 1, 0.42, 0.45, 0.04).tracks, { joint: "arm_l", rotations: pitch(1.6, 1.35, 1.1, 1.35, 1.6) }] },
    still("unarmed:idle", 2.4, [hang, { joint: "arm_r", rotations: pitch(1.35, 1.3, 1.35) }]),
  ];
}

const ARMOUR = { name: "armour", colour: [0.78, 0.78, 0.78] as [number, number, number], metallic: 0.25, roughness: 0.55 };
const UNDER = { name: "under", colour: [0.09, 0.09, 0.1] as [number, number, number], metallic: 0.1, roughness: 0.8 };
const VISOR = { name: "visor", colour: [0.85, 0.6, 0.15] as [number, number, number], metallic: 0.9, roughness: 0.2 };

/** A block figure `h` tall facing +X, holding its right hand forward. With
 *  `lean` it stoops and its head juts forward, to tell the two apart. */
function figure(h: number, lean: number, colours: boolean, rigged = true): ModelSpec {
  const P: [number, number] = [1, 0], S: [number, number] = [0, 1];
  const u = h / 8;
  const boxes: ModelBox[] = [
    // Legs: shins in the secondary colour, thighs in the primary.
    box(0, 0.75 * u, 0, 1.1 * u, 1 * u, 1.9 * u, 0, S, "shin_l"), box(0, -0.75 * u, 0, 1.1 * u, 1 * u, 1.9 * u, 0, S, "shin_r"),
    box(0, 0.75 * u, 1.9 * u, 1 * u, 0.9 * u, 1.7 * u, 0, P, "thigh_l"), box(0, -0.75 * u, 1.9 * u, 1 * u, 0.9 * u, 1.7 * u, 0, P, "thigh_r"),
    // Hips (undersuit), chest, a backpack.
    box(lean * 0.3 * u, 0, 3.6 * u, 1.1 * u, 2.2 * u, 0.6 * u, 1, undefined, "pelvis"),
    box(lean * 0.8 * u, 0, 4.2 * u, 1.5 * u, 2.6 * u, 2.2 * u, 0, P, "spine"),
    box(lean * 0.8 * u - 0.95 * u, 0, 4.6 * u, 0.5 * u, 1.6 * u, 1.5 * u, 0, S, "spine"),
    // Shoulders, and arms brought forward to hold a weapon.
    box(lean * 0.8 * u, 1.65 * u, 5.4 * u, 1.2 * u, 0.8 * u, 1 * u, 0, S, "arm_l"), box(lean * 0.8 * u, -1.65 * u, 5.4 * u, 1.2 * u, 0.8 * u, 1 * u, 0, S, "arm_r"),
    box(lean * 0.8 * u + 0.9 * u, 1.2 * u, 4.7 * u, 1.9 * u, 0.6 * u, 0.6 * u, 0, P, "arm_l"), box(lean * 0.8 * u + 0.9 * u, -1.3 * u, 4.7 * u, 1.9 * u, 0.6 * u, 0.6 * u, 0, P, "arm_r"),
    // Neck, head and visor.
    box(lean * 1.4 * u, 0, 6.4 * u, 0.7 * u, 0.8 * u, 0.3 * u, 1, undefined, "neck"),
    box(lean * 2 * u, 0, 6.7 * u, (1.3 + lean * 0.7) * u, 1.2 * u, 1.3 * u, 0, P, "neck"),
    box(lean * 2 * u + (0.7 + lean * 0.35) * u, 0, 7.1 * u, 0.12 * u, 0.9 * u, 0.5 * u, 2, undefined, "neck"),
  ];
  const rig: ModelRig = {
    joints: [
      { name: "pelvis", parent: null, at: [0, 0, 3.6 * u] },
      { name: "spine", parent: "pelvis", at: [lean * 0.4 * u, 0, 4.2 * u] },
      { name: "neck", parent: "spine", at: [lean * 1.4 * u, 0, 6.4 * u] },
      { name: "arm_l", parent: "spine", at: [lean * 0.8 * u, 1.5 * u, 5.9 * u] },
      { name: "arm_r", parent: "spine", at: [lean * 0.8 * u, -1.5 * u, 5.9 * u] },
      { name: "thigh_l", parent: "pelvis", at: [0, 0.75 * u, 3.6 * u] },
      { name: "shin_l", parent: "thigh_l", at: [0, 0.75 * u, 1.9 * u] },
      { name: "thigh_r", parent: "pelvis", at: [0, -0.75 * u, 3.6 * u] },
      { name: "shin_r", parent: "thigh_r", at: [0, -0.75 * u, 1.9 * u] },
    ],
    attach: { weapon: "arm_r", head: "neck" },
    clips: figureClips(u),
  };
  return {
    boxes, colours, materials: [ARMOUR, UNDER, VISOR],
    nodes: {
      weapon: [lean * 0.8 * u + 1.9 * u, -1.3 * u, 5 * u],
      head: [lean * 2 * u + (0.7 + lean * 0.35) * u, 0, 7.35 * u],
    },
    extras: { height: h },
    ...(rigged ? { rig } : {}),
  };
}

/** A long box of a gun, `len` long, its grip at the origin and a `muzzle`
 *  node at the end of the barrel. */
function gun(len: number, thick: number): ModelSpec {
  return {
    boxes: [
      { min: [-len * 0.3, -thick / 2, -thick * 0.2], max: [len * 0.45, thick / 2, thick * 1.1], material: 0 },
      { min: [len * 0.45, -thick * 0.25, thick * 0.4], max: [len * 0.7, thick * 0.25, thick * 0.9], material: 1 },
      { min: [-thick * 0.3, -thick * 0.3, -thick * 1.4], max: [thick * 0.4, thick * 0.3, -thick * 0.2], material: 1 },
    ],
    colours: false,
    materials: [{ name: "body", colour: [0.2, 0.22, 0.25], metallic: 0.6, roughness: 0.45 }, { name: "barrel", colour: [0.07, 0.07, 0.08], metallic: 0.8, roughness: 0.35 }],
    nodes: { muzzle: [len * 0.7, 0, thick * 0.65] },
    extras: {},
  };
}

/** The stand-in bipeds: the first has tint regions, the second none (so
 *  the whole-body tint runs too). */
export const MOCK_BIPEDS: Record<string, (rigged?: boolean) => ModelSpec> = {
  spartan: (rigged) => figure(0.72, 0, true, rigged),
  elite: (rigged) => figure(0.86, 1, false, rigged),
};

/** The class of clips each stand-in weapon is held with; one is left
 *  unclassed, so the rifle fallback runs. */
export const MOCK_WEAPON_CLASSES: Record<string, string> = { battle_rifle: "rifle", smg: "pistol", sniper_rifle: "rifle", plasma_rifle: "pistol" };

/** The stand-in weapons. One the mock's players hold is left out on
 *  purpose, so a weapon without a model is drawn too. */
export const MOCK_WEAPONS: Record<string, () => ModelSpec> = {
  battle_rifle: () => gun(0.42, 0.05),
  smg: () => gun(0.26, 0.05),
  shotgun: () => gun(0.4, 0.06),
  sniper_rifle: () => gun(0.62, 0.045),
  plasma_rifle: () => gun(0.3, 0.07),
};

/** A boxy vehicle `len` long facing +X, its origin on the ground under its
 *  middle, with seat nodes. */
function car(len: number, wid: number, tall: number, seats: Record<string, [number, number, number]>): ModelSpec {
  const wheel = tall * 0.45;
  const boxes: ModelBox[] = [
    { min: [-len / 2, -wid / 2, wheel * 0.6], max: [len / 2, wid / 2, tall * 0.75], material: 0 },
    { min: [-len * 0.15, -wid * 0.4, tall * 0.75], max: [len * 0.2, wid * 0.4, tall], material: 1 },
  ];
  for (const fx of [-0.32, 0.32]) for (const fy of [-0.5, 0.5]) {
    boxes.push({ min: [fx * len - wheel / 2, fy * wid - wheel * 0.2, 0], max: [fx * len + wheel / 2, fy * wid + wheel * 0.2, wheel], material: 2 });
  }
  return {
    boxes, colours: false,
    materials: [
      { name: "hull", colour: [0.55, 0.58, 0.5], metallic: 0.3, roughness: 0.6 },
      { name: "cab", colour: [0.2, 0.22, 0.24], metallic: 0.5, roughness: 0.4 },
      { name: "wheel", colour: [0.06, 0.06, 0.06], metallic: 0, roughness: 0.9 },
    ],
    nodes: Object.fromEntries(Object.entries(seats).map(([k, v]) => ["seat_" + k, v])),
    extras: {},
  };
}

/** The stand-in vehicles. */
export const MOCK_VEHICLES: Record<string, () => ModelSpec> = {
  warthog: () => car(2.3, 1.3, 0.9, { driver: [0.1, 0.3, 0.6], passenger: [0.1, -0.3, 0.6], gunner: [-0.7, 0, 0.9] }),
};

/** Stand-in AI bipeds: a short one and a plain one. The other species the
 *  mock's campaign sends have none, so the fallbacks run. */
export const MOCK_ACTOR_BIPEDS: Record<string, (rigged?: boolean) => ModelSpec> = {
  grunt: (rigged) => figure(0.5, 1, false, rigged),
  marine: (rigged) => figure(0.68, 0, false, rigged),
};

type Entry = { glb: string; clips?: string[]; class?: string };

export interface MockModels {
  /** The manifest's `models` table. */
  models: { version: 1 | 2; bipeds: Record<string, Entry>; weapons: Record<string, Entry>; vehicles: Record<string, Entry> };
  file(name: string): { bytes: Uint8Array; type: string } | null;
}

/** The mock's models: skinned bipeds with clips (version 2), or with
 *  `rigged` false the static version 1 kind. */
export function mockModels(rigged = true): MockModels {
  const bipedSet = { ...MOCK_BIPEDS, ...MOCK_ACTOR_BIPEDS };
  const table = (dir: string, set: Record<string, unknown>, more: (k: string) => Partial<Entry> = () => ({})): Record<string, Entry> =>
    Object.fromEntries(Object.keys(set).map((k) => [k, { glb: `models/${dir}/${k}.glb`, ...more(k) }]));
  const done = new Map<string, Uint8Array>();
  return {
    models: {
      version: rigged ? 2 : 1,
      bipeds: table("biped", bipedSet, (k) => (rigged ? { clips: bipedSet[k]!(true).rig!.clips.map((c) => c.name) } : {})),
      weapons: table("weapon", MOCK_WEAPONS, (k) => (rigged && MOCK_WEAPON_CLASSES[k] ? { class: MOCK_WEAPON_CLASSES[k] } : {})),
      vehicles: table("vehicle", MOCK_VEHICLES),
    },
    file(name) {
      const m = /^models\/(biped|weapon|vehicle)\/([a-z0-9_]+)\.glb$/.exec(name);
      const sets: Record<string, Record<string, (rigged?: boolean) => ModelSpec>> = { biped: bipedSet, weapon: MOCK_WEAPONS, vehicle: MOCK_VEHICLES };
      const make = m ? sets[m[1]!]![m[2]!] : undefined;
      if (!make) return null;
      let bytes = done.get(name);
      if (!bytes) { bytes = encodeModelGlb(make(rigged)); done.set(name, bytes); }
      return { bytes, type: "model/gltf-binary" };
    },
  };
}
