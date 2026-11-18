# Typed tag definitions (`tags::defs`)

Status: **Built** (2026-11-18) for 14 groups, each proven against every
retail tag of the group on the disc. **Two converter paths use them**:
Halo 1 vehicles that no Halo 2 template stands in for (a10's cryotube,
chairs, fighterbomber and Halo ring), **on by default** since converter
version 33, and Halo 1 scenery, opt-in (`h1objects::Options::rebuild`).
Neither has run in the game yet.
Evidence labels are those of [map-format.md](map-format.md): **E**
engine code, **V** checked on the owner's disc, **D** a naming or
interpretation choice made here.

[tag-schema.md](tag-schema.md) knows where a group's pointers, references,
string IDs and raw pointers are, and nothing else. A converter that knows
no more has to copy a retail tag (a template) and patch the fields it
knows. Template leftovers caused real bugs:

- lifepods kept the pelican's engine loops;
- a sky model kept NONE materials and crashed the game;
- scenery had zeroed region bytes.

A definition lists **every** field of a group. Each field has an offset, a
type, our own name, and what it means with the evidence. Bytes nobody
understands yet are explicit `unknown` spans, never implicit. A tag
written from a definition has a defined value in every byte.

## Rules

- **Our own names and evidence only.** No community definition (Invader,
  Assembly plugins, H2EK/Guerilla, OpenH2, ...) is used for names or
  layouts. Two places in the repo carry such names already: `schema.rs`
  (OpenH2/Assembly provenance) and the `S` rows of
  [h1-objects.md](h1-objects.md) ("recalled from the editing kits").
  Neither was taken over: those fields stay unknown or typed-only unless a
  disc check (V) or engine code (E) supports the meaning, and then the
  name is ours.
- **A name needs evidence.** A field whose type is known but not its role
  (a reference, a string ID, a block found by the schema) gets a name
  ending in `_<offset in hex>`: `string_1c`, `foot_ref_54`, `block_5c`.
  The coverage report counts those as **typed**, not **named**.
- **No retail values at length.** Files record offsets, types and names.
  `seen=` summarises an unknown span's retail values as `zero`, `const`,
  a count of distinct values, or `many`. A `default=` holds a small
  constant at most.
- **Agreement with the schema** (unit test
  `builtin_definitions_agree_with_the_schema`): every schema field (block
  with its element size, tagref, datum, string, raw, rawsize, word, bsp)
  is in the definition at the same offset with the same kind.
  - A definition may have pointer fields the schema lacks. Those are null
    or empty in every retail instance of the group, so inference could not
    see them. The disc test checks they hold no retail datum, string index
    or address.
  - Alignment can differ, because the schema keeps the weakest alignment
    seen per group.

## The files

`crates/tags/src/defs/*.txt`, parsed in the order of `defs::FILES` (a
struct before the files that embed it):

```text
# comment
struct obje 0xbc                      # a reusable struct
  0x0 enum16 object_type values=bipd|vehi|weap|... -- E 0x000b7b40: ...
  0x4 f32 bounding_radius -- E ...
  0x14 unknown 0x4 seen=many
  0x34 tagref model groups=hlmt -- E ...
  0x64 block functions 0x20 align 4 -- E 0x000bac20 ...
    0x8 string output -- E ...
    0x14 data data_14 align 4
group scen 0xc4                       # a group's root
  0x0 struct object obje -- the object header
  0xbc unknown 0x4
```

A field line is `<offset> <type> <name> [key=value ...] [-- meaning]`.
Fields must tile the element exactly, with no gap and no overlap. The
parser refuses a gap ("every byte needs a field or an unknown span").

| Type | Bytes | Value |
|---|---|---|
| `u8 i8 u16 i16 u32 i32 f32`, `[n]` for several | 1–4 each | integers, floats (a point is `f32[3]`) |
| `enum8/16/32 values=a\|b\|...` | 1/2/4 | enumeration, value `i` named `values[i]` |
| `flags8/16/32 bits=a\|_\|b` | 1/2/4 | flag bits, bit 0 first |
| `string` | 4 | string ID |
| `tagref groups=a\|b` | 8 | group word, then datum |
| `datum groups=...` | 4 | bare datum |
| `raw`, `rawsize of=<offset>` | 4 | raw-resource pointer, and its byte length |
| `word` | 4 | packed data that can look like an address |
| `bsp` | 12 | scenario BSP descriptor |
| `block <name> <element size> align <n>` | 8 | count and address; the element's fields follow, indented |
| `data <name> align <n>` | 8 | data reference (bytes) |
| `struct <name> <struct>` | the struct's | an embedded struct |
| `unknown <len> seen=...` | len | not understood; named `unknown_<offset>` |
| `pad <len>` | len | zero in every retail instance and read by nothing found (the disc test checks the zero) |

`default=<value>` gives a fresh tag's value: an integer, floats
`a,b,c`, an enum value name, flag names joined by `|`, or hex bytes for
an unknown span. Without it the value is zero, null or empty.

**Shared layers.** The object header is `struct obje` (`0..0xbc`,
[objects.md](objects.md)). `struct unit` (`0..0x1c8`) and `struct item`
(`0..0x12c`) embed it, and the groups embed those. The unit layer stops
at `0x1c8`, before the seat block: the schema infers its element as
`0xb4` in `bipd` and `0xb0` in `vehi`. One of the two is an inference
artefact (objects.md has `0xb0` from the layout). A shared struct can't
hold both, so each group has its own copy from `0x1c8`.

## The API

```rust
use tags::defs::{self, Defs, Val};

let def = Defs::builtin().group(FourCC(*b"scen")).unwrap();
let rec = defs::decode(def, &loose_tag)?;                 // LooseTag -> Record
let back = defs::encode(def, key, &rec, Region::Metadata)?; // Record -> LooseTag
let fresh = defs::default_record(def);                    // every field at its default
let changes = defs::diff(def, &a, &b);                    // field by field, with paths
let block = defs::to_built(def, &rec, &mut resolve)?;     // Record -> map::build::Block
let rec = defs::from_built(def, &block)?;                 // and back
rec.at("object.functions[0].output"); rec.set("object.bounding_radius", Val::Float(1.0))?;
```

- **Values.** A `Record` is the element's `(name, Val)` pairs in
  definition order. A `Val` is an integer, float or list of them, an enum,
  flags, a string ID (`Str`), a reference or datum (`Target`: null, by
  `TagKey`, or by datum in a tag set), a raw pointer, a block (its
  records), data bytes, an embedded struct, or the bytes of an unknown
  span.
- **Unresolved.** A block or data reference with no child is
  `Val::Unresolved`. Retail has some: a count with a null address. A
  builder tree can also keep a pinned map's pointer this way.
- **Sharing.** Retail shares arrays: one node reached twice. Decoding
  records the source node on a block's first element (`Record::share`).
  Encoding shares exactly those arrays, and shares fresh records with
  equal content. `share` is layout, not value: equality and `diff`
  ignore it.
- **Builder trees.** `from_built` reads a reference fixup's group word as
  the builder will write it: null as `0xffffffff`, a target's group as 0.
  The builder fills in the target's group itself, so a template's stale
  group word under a fixup is not a difference. A converter's 8-byte
  reference at `hlmt +0x10` (the definition has a group word and the
  `+0x14` datum) reads into those two fields.

## Proof

```sh
H2_DISC=disc/halo2.iso cargo test --release -p tags --test tag_defs -- --ignored --nocapture
```

The test reads every map on the disc, shared caches first. It takes every
tag of the defined groups through extraction (`MapSource::extract`, the
schema walk) and keeps each distinct content once. For every tag it
checks four things:

- it decodes with its group's definition;
- it holds no datum, string index or address outside a definition field
  the schema also knows;
- it re-encodes **byte-identical** (`defs::canonical`: every array
  written out where it is reached, every element's bytes and field
  values; alignment, which is placement, is left out);
- every `pad` span is zero.

The test also counts tags whose node tree re-encodes exactly, sharing
included (the `nodes` column).

Options:

- `H2_DEFS_GROUPS` and `H2_DEFS_MAPS` pick groups and maps.
- `H2_DEFS_SKELETON=1` prints a starting definition from the schema: every
  free word an `unknown` with `seen=`, zero runs merged. That is how each
  group started.
- `H2_DEFS_ANNOTATE=1` refreshes `seen=`.
- `H2_DEFS_EXTRA=file.txt` checks a draft outside the source tree.

Unit tests (`cargo test -p tags defs`) cover:

- decode/encode on an invented tag;
- a fixup inside a plain field refused;
- defaults, shared arrays, diff paths and builder trees;
- gaps refused and the text form round-tripping;
- agreement with the schema;
- every built-in definition's default record round-tripping.

**Result, V (2026-11-18), all 30 disc maps:** 122,992 tag instances,
92,538 distinct, 10 aliases (not trees), none refused. **Every distinct
tag of every group decodes, re-encodes byte-identical, and holds no
pinned values**, and no padding is nonzero.

Coverage is weighted by retail bytes: every element of every block
counted, and data-reference contents counted as data.

| Group | Tags | Named | Typed only | Pad | Unknown | Data | Exact node tree |
|---|---|---|---|---|---|---|---|
| `scen` | 505 | 42.9 % | 13.1 % | 0 | 43.7 % | 0.3 % | 505 |
| `bipd` | 242 | 13.5 % | 16.5 % | 0 | 68.3 % | 1.6 % | 125 |
| `vehi` | 71 | 6.8 % | 23.2 % | 0 | 67.0 % | 3.0 % | 26 |
| `weap` | 122 | 10.7 % | 31.0 % | 0 | 57.6 % | 0.7 % | 122 |
| `eqip` | 14 | 29.5 % | 27.3 % | 0 | 43.2 % | 0 | 14 |
| `hlmt` | 927 | 54.9 % | 14.9 % | 14.0 % | 16.3 % | 0 | 927 |
| `mode` | 937 | 42.4 % | 3.1 % | 2.9 % | 51.6 % | 0 | 937 |
| `coll` | 678 | 97.8 % | 0 | 0.2 % | 0.3 % | 1.7 % | 675 |
| `phmo` | 2,243 | 65.2 % | 3.6 % | 1.7 % | 28.8 % | 0.7 % | 2,243 |
| `shad` | 2,288 | 83.8 % | 4.8 % | 3.2 % | 1.8 % | 6.4 % | 2,288 |
| `bitm` | 21,158 | 34.7 % | 2.9 % | 19.1 % | 43.2 % | 0 | 21,158 |
| `jmad` | 858 | 56.2 % | 19.7 % | 0 | 24.1 % | 0 | 857 |
| `snd!` | 62,012 | 50.0 % | 0 | 0 | 50.0 % | 0 | 62,012 |
| `lsnd` | 483 | 86.3 % | 4.6 % | 0 | 9.1 % | 0 | 483 |

"Exact node tree" below the tag count means some arrays that retail lays
out twice with equal content (data references, which carry no sharing
record) encode as one. The bytes are identical either way.

## Where the names come from

Every named field's meaning text in the files gives its evidence. In
summary:

- **Object family** (`obje`, `scen`, `unit`, `item`, `bipd`, `vehi`,
  `weap`, `eqip`): [objects.md](objects.md).
  - E: object type (indexes the type table at `0x00468630`, `0x000b7b40`),
    bounding sphere (`0x000be075`, `0x000bdfa9`), default variant
    (`0x000ba571`), model, creation effect, functions (`0x000bac20`, with
    `output` the matched ID), attachments (`0x000b8c82`, `0x000bed2a`; the
    marker `0x000b8c40`).
  - V: widgets, change colours, predicted resources (each element's `+4`
    an existing tag), the `weap` magazines and barrels (strides and the
    barrel's `proj` reference, [h1-objects.md](h1-objects.md) V2).
  - The `weap` aim-assist floats (`+0x208..+0x218`) stay unknown here.
    Their offsets are E (`0x1a50a0`), but the names in
    [aim-assist.md](../game/aim-assist.md) came from a community file.
    The heat, magazine and barrel scalars are `S` in h1-objects.md, so
    they stay unknown too.
- **`hlmt`**:
  - E: the model chain (`object_new`), variants (`0x0016d1bb`, region
    map `0x0016d764`), regions (`0x0016d205`).
  - V: the region and permutation collision and physics bytes
    ([collision.md](collision.md), and what the converter writes); nodes
    as the `mode` node's first `0x5c` bytes.
- **`mode`**:
  - V, data checks: compression bounds that contain every decoded
    position, LOD section indices in range, section counts equal to the
    raw block's, node tree links.
  - E: the section raw block info ([structure-bsp.md](structure-bsp.md)),
    section groups and their per-LOD indices (`0x00137650`).
  - D: materials, with the shader at `+8`.
- **`coll`, `phmo`**: [collision.md](collision.md) and
  `tags::collision`: the BSP blocks (E `0x001de6d0`, `0x001ddea0`,
  `0x001de2c0`, ...), the BSP physics (E `0x00246c60`), and the shape
  blocks and rigid bodies (E `0x001ea9c0`, `0x001eaf30`, `0x001ef070`).
- **`shad`**: [shader-instances.md](shader-instances.md) and
  `tags::shader`.
  - E: the template, the postprocess and everything the draw path reads
    (`0x00016bc7` on).
  - V: the root parameters block, a mirror: each of its bitmaps, colours
    and scalars is copied into the postprocess (checked on all 2,288).
    The engine never reads it.
- **`bitm`**: image fields from the repo's own pixel decoder (size,
  format, flags, levels of detail, sizes; the cache handles, E
  `0x0012c9fb`). The root scalars stay unknown: their only source is a
  community plugin.
- **`jmad`**: [animation.md](animation.md), E fields of the animation
  record, events, modes, classes and actions (`0x001db170`), and static
  poses (`0x001dd8f0`).
- **`snd!`**: the sample rate, encoding, compression (E `0x0021e421`) and
  pitch ranges. Flags, class and playback index stay unknown (community
  source only).
- **`lsnd`**: the Halo 2 layout the Halo 1 sound converter writes
  (`h1sounds::looping`), D.

## Scenery rebuilt from definitions (`h1objects::Options::rebuild`)

With `Converter::options.rebuild` set, a Halo 1 scenery object becomes
three tags written from the definitions (`h1objects::rebuild`), with no
retail template read:

- **`scen`**: `default_record`, then the object type (6), Halo 1's
  bounding radius and offset, and the model.
- **`hlmt`**: the render, collision and animation models (no physics
  model), the nodes (`h1models::mode::hlmt_nodes`), and the regions with
  the collision and physics region and permutation of each (the same
  `region_map` as the template path).
- **`mode`**: converted as before (`h1models::mode`, already written
  from zero), its **material list** rewritten from the definition: one
  material per Halo 1 shader, the shader set or null, everything else at
  its default.

The template path stays the default until the rebuilt scenery has run in
the game.

**Comparison, V (2026-11-18)** (`rebuild_disc`, a10, a30, b30, c10 and
Blood Gulch on `05a_deltaapproach`, 158 scenery objects):

```sh
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso H1_LEVELS=a10,a30,b30,c10,bloodgulch \
  cargo test --release -p h1objects --test rebuild_disc -- --ignored --nocapture
```

Both paths' `scen`, `hlmt` and `mode` are read back through the
definitions and diffed. **Every field the conversion writes agrees**:

- object type, bounds, default variant and model;
- the `hlmt` model chain, nodes, regions and permutations (names and
  collision/physics bytes), and variants;
- the whole `mode`, materials included.

The rebuilt tags contain no pinned block. What differs is exactly what
the template contributed:

| Field | Template path | Rebuilt | Why |
|---|---|---|---|
| `scen.object.predicted_resources` | the statue's block, pinned at its retail address | empty | the statue's resources, not the converted object's |
| `hlmt.block_58`, `hlmt.block_84` | the statue's one-element blocks, pinned | empty | typed only; unknown role |
| `scen.object.unknown_18`, `unknown_6c..unknown_8c` (10 words), `scen.unknown_bc` | the statue's values | zero | unknown spans |
| `hlmt.unknown_28`..`unknown_44` (7 words), `unknown_a4`, `unknown_e4` | the statue's values | zero | unknown spans |
| null references: `scen.object` `bloc_ref_3c`, `shad_ref_44`, `creation_effect`, `foot_ref_54`; `hlmt` `phys_ref_18`, `udlg_ref_8c`, `shad_ref_94`, `shad_ref_f0` | group word kept, datum null | `0xffffffff` both | both null |

The unknown spans are the risk to check in game. In retail these
words are floats on most scenery (the `seen=` of `scen` `0x6c..0x8c`
and `hlmt` `0x28..0x44`), and zero may not be a neutral value for all
of them. If the rebuilt scenery misbehaves (fades out, never draws at a
distance, or is lit differently), those words are the first suspects:
name them from the engine's readers, then give them defaults.

## Vehicles with no template (`h1objects::rebuild::vehicle`)

A Halo 1 vehicle that no row of `h1objects::convert::TEMPLATES` names
used to be dropped, its palette entry null and its object names pointing
at placement -1. a10 has five: the cryotube, the pilot and pod chairs, the
fighterbomber and the Halo ring. The scripts seat the player in the
cryotube and open it as the opening cinematic ends (`unit_open
cryotube_1`), which faulted until the name was stubbed
(`h1scripts::compile::NONE_UNSAFE`, kept as the safety net for any other
unplaced name). Now such a vehicle is written from the `vehi` definition,
with no retail template read:

- **Object header**: object type 1, Halo 1's bounding radius and offset,
  the model, and the predicted resources: the converted `mode` (`+0` kind
  2, `+2` -1), then every converted bitmap its shaders draw (kind 0, index
  0). V: every retail vehicle on the disc lists some, its `mode` once in
  that form and first, its bitmaps in that form.
- **`hlmt` and `mode` materials**: as rebuilt scenery (above). Collision is
  Halo 1's converted `coll`; the animation graph Halo 1's converted
  `antr`; no physics model (Halo 1's cryotube has no `phys`).
- **Vehicle type** (`vehi +0x1f0`, named below): turret, Halo 2's
  stationary vehicle. Turrets hold no wheels, treads, hover points or
  engine raw (`+0x268`, `+0x2e8`, `+0x2f0` empty on every one), and their
  one physics element (`+0x2f8`) is zero but for four `-1` halves.
- **Seats** (`+0x1c8`): one per Halo 1 seat (Halo 1's `unit +0x2e4`,
  `0x11c` each, V on a10, b30 and c10: label `+4`, marker `+0x24`, camera
  marker `+0x84`, each 32 characters). The Halo 2 seat takes Halo 1's
  `label` (so a10's `CT-driver` is the text its scripts name; the seat
  table matches a label equal to the script's text before any role rule),
  the marker at `+8` and the camera marker at `+0x60` (D: retail's values
  there are marker names, `driver`, `gunner`, `camera_rider`).
- **The turret role** (`TURRET_ROLE`, `TURRET_SEAT_ROLE`,
  `TURRET_PHYSICS_ROLE`): every 4-byte unknown span of the root, the
  seats and the physics element where at least half of Halo 2's 25
  distinct turret-type vehicles hold one nonzero value takes that value
  (22 words in the root, 5 per seat, 4 in the physics element). Zero is
  not neutral there: they are scales, distances and angles retail never
  leaves at zero (the object layer's `+0x6c..+0x8c`, the unit layer's
  `+0xcc`), and `-1` halves where zero would name element 0. The table is
  in code, not in the definition files, because the shared `obje` and
  `unit` structs also build scenery; the disc test recomputes it.
- **Everything else** is the definition's default: zero, null, empty.
  Blocks left empty that some retail vehicles fill: the unit's weapons,
  camera tracks and HUD, the seats' camera tracks and HUD, attachments,
  functions. A Halo 1 vehicle with no seats (a10's Halo ring, b30's
  falling box) has none in Halo 2 either; three retail vehicles on
  06a_sentinelwalls have none, so `every_campaign_level_converts`' sweep
  accepts an empty `vehi +0x1c8`. Halo 1's seat flags, camera angles and
  other unit tuning have no established Halo 2 home and are not carried
  (listed in the object report).

Fields named for this (V, the disc proof above still round-trips all 71
retail vehicles byte-identical with no pinned values):

| Field | Evidence |
|---|---|
| `vehi +0x1f0` `vehicle_type` (`enum16`: tank, jeep, type_2, dropship, hover, flier, turret) | E: objects.md (vehicle type). V: the names by the retail vehicles holding each value: 0 scorpion, 1 warthog, 3 pelican, phantom and gravity throne, 4 ghost, wraith, spectre and creep, 5 banshee, 6 the turrets; none holds 2 |
| `vehi +0x1c8` seat `label` | V: the scenario's seat-mapping masks select seats by it (`h1scripts::seats`, on 05a) |
| `vehi +0x284` `snd_ref_284`, `+0x294` `foot_ref_294` (were unknown) | V: objects.md lists snd! and foot here; the group word is `snd!`/none and `foot` and the datum null on every retail vehicle, so the schema could not see them. As unknown spans a fresh tag held zero, which names tag 0 |

Proof (`H1_ISO=... H2_DISC=...`):

```sh
cargo test --release -p h1objects --test unit_rebuild_disc -- --ignored --nocapture
cargo test --release -p h1campaign --test disc a10_cryotubes -- --ignored --nocapture
```

- `turret_role_is_the_retail_turrets_majority`: recomputes the role table
  from every distinct retail vehicle of type turret and fails if it
  differs from the code's.
- `a10_vehicles_without_templates_are_rebuilt`: all five a10 vehicles
  convert; each reads back through the definition and writes the same
  tree; nothing is pinned; the predicted resources are the tree's `mode`
  then its bitmaps; the model chain (`hlmt`, `mode`, `coll`,
  `jmad`) is in the tree; the `hlmt` has the model's nodes; one seat per
  Halo 1 seat with its label and marker; the one physics element; the two
  null references null.
- `a10_cryotubes_are_placed_and_built_from_their_definition` (pinned and
  fresh): `cryotube_1` and `cryotube_2` name vehicle placements whose
  palette entry is the cryotube; read back out of the converted map it
  decodes and encodes byte-identical, its `hlmt`, `mode`, `coll` and
  `jmad` are tags in the map, and it has one seat, `CT-driver`, at marker
  `driver`.

What to watch in the game: the player seated in the cryotube during
a10's opening, and getting out when the scripts open it. The first
suspects if that goes wrong: the vehicle has no physics model, which no
retail turret lacks; and the rider's pose, since retail seat labels
(`warthog_d`) also read like the riders' animation mode names (D) and the
player's graph has no `CT-driver` mode.

## Which converted objects use the definitions

| Groups | Path | Why |
|---|---|---|
| `vehi` with no template row | definitions (default) | nothing else to build on |
| `scen` | template by default; definitions with `Options::rebuild` | the rebuilt scenery has not run in the game |
| `bipd`, `vehi` with a template, `weap`, `eqip`, `proj` | template | their gameplay fields (speeds, seats, cameras, physics, weapon tuning) are unknown spans; built from zero they would not play |

## Open

- **Unknown spans** are largest in the unit layer (`bipd`, `vehi`), the
  `weap` tuning, `mode`'s nested blocks, and the `bitm` and `snd!` roots.
  Each needs the engine's reader found to be named.
- **More groups:** `sky`, `effe`, `proj`, `jpt!` (the second tier asked
  for), and the device and other object groups (`bloc`, `mach`, `ctrl`,
  `garb`, `proj`, `crea`, `ssce`), which `struct obje` already covers.
- **More converter paths**: the other objects (templated vehicles,
  bipeds, weapons, equipment, projectiles), shaders (`shad` with its postprocess mirrors kept consistent),
  and the sky, each on its definition with its own comparison.
