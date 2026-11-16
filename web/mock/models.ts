// Stand-in player and weapon models for the mock (H2_MOCK_MODELS=1), in the
// encoding the model contract describes, so the 3D view's model path can be
// run and tested with nothing from the game: block figures made of boxes.
//
// A biped stands on the origin facing game +X (glTF +X), has empty `weapon`
// and `head` nodes, and a COLOR_0 whose R is the primary change-colour
// weight and G the secondary. A weapon has an empty `muzzle` node. Vertices
// are written in glTF axes: game (x, y, z) → (x, z, -y).

type V3 = [number, number, number];

export interface ModelBox {
  /** Game coordinates. */
  min: V3; max: V3;
  material: number;
  /** [primary, secondary] change-colour weights; absent for none. */
  tint?: [number, number];
}

export interface ModelSpec {
  boxes: ModelBox[];
  materials: Array<{ name: string; colour: [number, number, number]; metallic: number; roughness: number }>;
  /** Empty nodes, by name, at a game position. */
  nodes: Record<string, V3>;
  /** Whether to write COLOR_0 (a model without it is tinted whole). */
  colours: boolean;
  extras: Record<string, unknown>;
}

const gl = (v: V3): V3 => [v[0], v[2], -v[1]];

/** A model as a binary glTF: one mesh node (a primitive per material) and
 *  the empty nodes. */
export function encodeModelGlb(spec: ModelSpec): Uint8Array {
  const chunks: Uint8Array[] = [];
  let offset = 0;
  const bufferViews: unknown[] = [], accessors: unknown[] = [], primitives: unknown[] = [];
  const push = (data: Float32Array | Uint32Array, target: number) => {
    const bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: bytes.length, target });
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
    accessors.push({ bufferView: push(idx, 34963), componentType: 5125, count: idx.length, type: "SCALAR" });
    primitives.push({ attributes, indices: accessors.length - 1, material: mi, mode: 4 });
  });
  const empties = Object.entries(spec.nodes).map(([name, at]) => ({ name, translation: gl(at) }));
  const json = {
    asset: { version: "2.0", generator: "h2 live mock", extras: { generator: "h2 live mock", models_version: 1, ...spec.extras } },
    scene: 0,
    scenes: [{ nodes: [0, ...empties.map((_, i) => i + 1)] }],
    nodes: [{ name: "body", mesh: 0 }, ...empties],
    meshes: [{ name: "body", primitives }],
    materials: spec.materials.map((m) => ({ name: m.name, pbrMetallicRoughness: { baseColorFactor: [...m.colour, 1], metallicFactor: m.metallic, roughnessFactor: m.roughness } })),
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

const box = (cx: number, cy: number, z0: number, sx: number, sy: number, sz: number, material: number, tint?: [number, number]): ModelBox =>
  ({ min: [cx - sx / 2, cy - sy / 2, z0], max: [cx + sx / 2, cy + sy / 2, z0 + sz], material, tint });

const ARMOUR = { name: "armour", colour: [0.78, 0.78, 0.78] as [number, number, number], metallic: 0.25, roughness: 0.55 };
const UNDER = { name: "under", colour: [0.09, 0.09, 0.1] as [number, number, number], metallic: 0.1, roughness: 0.8 };
const VISOR = { name: "visor", colour: [0.85, 0.6, 0.15] as [number, number, number], metallic: 0.9, roughness: 0.2 };

/** A block figure `h` tall facing +X, holding its right hand forward. With
 *  `lean` it stoops and its head juts forward, to tell the two apart. */
function figure(h: number, lean: number, colours: boolean): ModelSpec {
  const P: [number, number] = [1, 0], S: [number, number] = [0, 1];
  const u = h / 8;
  const boxes: ModelBox[] = [
    // Legs: shins in the secondary colour, thighs in the primary.
    box(0, 0.75 * u, 0, 1.1 * u, 1 * u, 1.9 * u, 0, S), box(0, -0.75 * u, 0, 1.1 * u, 1 * u, 1.9 * u, 0, S),
    box(0, 0.75 * u, 1.9 * u, 1 * u, 0.9 * u, 1.7 * u, 0, P), box(0, -0.75 * u, 1.9 * u, 1 * u, 0.9 * u, 1.7 * u, 0, P),
    // Hips (undersuit), chest, a backpack.
    box(lean * 0.3 * u, 0, 3.6 * u, 1.1 * u, 2.2 * u, 0.6 * u, 1),
    box(lean * 0.8 * u, 0, 4.2 * u, 1.5 * u, 2.6 * u, 2.2 * u, 0, P),
    box(lean * 0.8 * u - 0.95 * u, 0, 4.6 * u, 0.5 * u, 1.6 * u, 1.5 * u, 0, S),
    // Shoulders, and arms brought forward to hold a weapon.
    box(lean * 0.8 * u, 1.65 * u, 5.4 * u, 1.2 * u, 0.8 * u, 1 * u, 0, S), box(lean * 0.8 * u, -1.65 * u, 5.4 * u, 1.2 * u, 0.8 * u, 1 * u, 0, S),
    box(lean * 0.8 * u + 0.9 * u, 1.2 * u, 4.7 * u, 1.9 * u, 0.6 * u, 0.6 * u, 0, P), box(lean * 0.8 * u + 0.9 * u, -1.3 * u, 4.7 * u, 1.9 * u, 0.6 * u, 0.6 * u, 0, P),
    // Neck, head and visor.
    box(lean * 1.4 * u, 0, 6.4 * u, 0.7 * u, 0.8 * u, 0.3 * u, 1),
    box(lean * 2 * u, 0, 6.7 * u, (1.3 + lean * 0.7) * u, 1.2 * u, 1.3 * u, 0, P),
    box(lean * 2 * u + (0.7 + lean * 0.35) * u, 0, 7.1 * u, 0.12 * u, 0.9 * u, 0.5 * u, 2),
  ];
  return {
    boxes, colours, materials: [ARMOUR, UNDER, VISOR],
    nodes: {
      weapon: [lean * 0.8 * u + 1.9 * u, -1.3 * u, 5 * u],
      head: [lean * 2 * u + (0.7 + lean * 0.35) * u, 0, 7.35 * u],
    },
    extras: { height: h },
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
export const MOCK_BIPEDS: Record<string, () => ModelSpec> = {
  spartan: () => figure(0.72, 0, true),
  elite: () => figure(0.86, 1, false),
};

/** The stand-in weapons. One the mock's players hold is left out on
 *  purpose, so a weapon without a model is drawn too. */
export const MOCK_WEAPONS: Record<string, () => ModelSpec> = {
  battle_rifle: () => gun(0.42, 0.05),
  smg: () => gun(0.26, 0.05),
  shotgun: () => gun(0.4, 0.06),
  sniper_rifle: () => gun(0.62, 0.045),
  plasma_rifle: () => gun(0.3, 0.07),
};

export interface MockModels {
  /** The manifest's `models` table. */
  models: { version: 1; bipeds: Record<string, { glb: string }>; weapons: Record<string, { glb: string }> };
  file(name: string): { bytes: Uint8Array; type: string } | null;
}

export function mockModels(): MockModels {
  const table = (dir: string, set: Record<string, unknown>) => Object.fromEntries(Object.keys(set).map((k) => [k, { glb: `models/${dir}/${k}.glb` }]));
  const done = new Map<string, Uint8Array>();
  return {
    models: { version: 1, bipeds: table("biped", MOCK_BIPEDS), weapons: table("weapon", MOCK_WEAPONS) },
    file(name) {
      const m = /^models\/(biped|weapon)\/([a-z0-9_]+)\.glb$/.exec(name);
      const make = m ? (m[1] === "biped" ? MOCK_BIPEDS : MOCK_WEAPONS)[m[2]!] : undefined;
      if (!make) return null;
      let bytes = done.get(name);
      if (!bytes) { bytes = encodeModelGlb(make()); done.set(name, bytes); }
      return { bytes, type: "model/gltf-binary" };
    },
  };
}
