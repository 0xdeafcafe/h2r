# Typed tag definitions (`tags::defs`)

Status: **Built** (2026-11-18) for 18 groups, each proven against every
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
  `antr`; the physics model built from Halo 1's collision (below).
- **Physics model**: `vehicle_new` (`0x000eeb80`) returns false when the
  model's `hlmt +0x20` physics model is null (`cmp [hlmt+0x24],-1` at
  `0x000eebcb`), and `object_new` then deletes the vehicle, so through
  version 37 a rebuilt vehicle was placed but never created. Version 38
  gave each a stand-in sphere; since version 43 the `phmo` is Halo 1's
  collision as Havok shapes (Physics models from Halo 1, below): one
  rigid body on the object (node -1) holding every collision node's
  solid cells, placed by the model's default pose, of Halo 1's `phys`
  mass. A vehicle with no solid collision (a10's fighterbomber and Halo
  ring have no `coll`) keeps the stand-in from `rebuild::sphere_physics`:
  one body (node -1), one 0.05 world unit sphere of 1000 kg on the
  origin, one material and one region.
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
  leaves at zero (the object layer's `+0x6c..+0x8c`), and `-1` halves
  where zero would name element 0. The unit layer's `+0xcc` was a role
  word through version 45; it is now named `camera_field_of_view` and
  written from Halo 1's `unit +0x1a0` (below). The table is
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
suspects if that goes wrong: the physics model (a dynamic body: a
vehicle whose shape is not on a floor may drop or drift); and
the rider's pose, since retail seat labels
(`warthog_d`) also read like the riders' animation mode names (D) and the
player's graph has no `CT-driver` mode.

## Bipeds from definitions (`h1objects::Options::rebuild_bipeds`)

Off by default: the template path stays what the game runs until a
rebuilt biped has played. With the option on (h1campaign's
`Options::rebuild_bipeds`), a Halo 1 biped with a role
(`h1objects::rebuild::BipedRole`) is written from the `bipd` definition
(`rebuild::biped`) instead of on a Halo 2 template:

- **Roles**: the Chief (`cyborg`), humans (marines, crew, the captain,
  pilots, Cortana, Johnson) and the Covenant (elites, grunts, jackals,
  hunters, engineers). The flood, sentinels and the monitor have no
  role and keep the template path.
- **Role words** (`rebuild::biped_role`): the turret rule over the base
  map's bipeds of the role, at conversion time, so no retail value is
  kept in the source. Every 4-byte unknown span of the root and its
  embedded structs where at least half the members hold one nonzero
  value takes it; each block whose elements are all such spans takes
  the element count most members hold (when at least half hold any),
  each element by the same rule. On 05a_deltaapproach the Chief has one
  member, humans four (marine, miranda, crewman, marine_female) and the
  Covenant five.
- **From Halo 1**: the object type (0), bounding sphere, model chain
  (`hlmt` from the definition, as for rebuilt scenery: no `phmo`) and
  predicted resources (the render model, then its bitmaps), the camera's
  field of view and the five `BIPED_WORDS`.
- **Everything else** is the definition's default; every reference is
  null (no HUD, weapons, character, sounds or effects), so the
  player biped has no health meter on this path.

Named on the way:

| Field | Evidence |
|---|---|
| `unit +0xcc` `camera_field_of_view` (`f32`, radians) | V: 70 degrees on every Halo 2 biped and turret with a Halo 1 counterpart, and equal to Halo 1's `unit +0x1a0` on five pairs (cyborg and masterchief, marine_armored and marine, elite, grunt, jackal). Name D: a field of view is the plain reading of 70 degrees on a unit; no reader found yet |

`rebuild::BIPED_WORDS` carries five more `bipd` words from Halo 1 as they
are: on the same five pairs each agrees within 1 % with one Halo 1 word
(`+0x1f4` with Halo 1's `+0x2f8`, `+0x220` with `+0x408`, `+0x26c` with
`+0x428`, `+0x2a0` and `+0x2a8` both with `+0x360`). They hold angles
in radians and small ratios; their meaning is not established, so they
stay unknown spans.

Proof (`crates/h1objects/tests/unit_rebuild_disc.rs`, and the lib test
`rebuilt_biped_is_the_role_with_halo_1s_header_and_words`):

- `biped_words_match_halo_2s_counterparts`: the camera and the five
  words, Halo 1 against Halo 2, on the five pairs.
- `bipeds_rebuilt_against_the_template_path`: all 31 Halo 1 bipeds with
  a role on the ten levels (11 without one keep the template), each
  built both ways. The rebuilt record reads back, writes the same
  again, pins nothing, and holds no template bytes; it has the template
  path's object type, bounding radius and model chain (`hlmt`, `mode`,
  `coll`, `jmad`), Halo 1's camera and words, and its role's words. The
  Chief's rebuilt root differs from the template path's in 0 of 116
  words (cinematic Chief: 2); the Covenant's in 19 to 44; humans' in 52
  to 56, since the template path puts humans on the Chief.

What differs from the template path, the suspects if a rebuilt biped
misbehaves: no `phmo` (the template path borrows the template's for 29
of the 31; grunts already run with none on both paths); no HUD, weapon,
character or sound references; and the role's majority shapes where a
retail biped is sized to its own model.

## Devices (`mach`, `ctrl`; `h1objects::rebuild::device`)

`crates/tags/src/defs/devi.txt`: `struct devi` (`0..0x11c`, the object
header and the device layer) and the `mach` (`0x134`) and `ctrl`
(`0x140`) roots. All 135 retail machines and 14 controls round-trip
byte-identical. The disc has no `lifi` tags, so there is no `lifi`
definition. Named fields and their evidence are in the file; the ones
the conversion writes:

| Field | Evidence |
|---|---|
| `devi +0xbc` `device_flags` | E: object_new reads it for device types; bit 0 set on the retail devices whose position wraps |
| `devi +0xc0..+0xd4` six rates: power, power acceleration, position, position acceleration, depowered position, depowered position acceleration | E `0x00115431`/`0x00115437`: the position step is `position_rate` and `depowered_position_rate` blended by the device's power, times the seconds per tick, so they are rates per second. V: no retail device holds 0 in an acceleration slot; 900 stands for "at once" |
| `devi +0x10c` `delay_time`, `+0x118` `activation_radius` | V: set on retail doors only, in seconds and world units, the values Halo 1's doors hold |
| `mach +0x11c` `machine_type` (door, platform, gear) | E `0x001153ee`: type 2 wraps the position at 1.0; `0x0011557e`: type 0 runs the door timer. V: retail doors 0, lifts and rails 1, flywheels and hinges 2 |
| `mach +0x11e` `machine_flags`, `+0x120` `door_open_time`, `+0x12c` `collision_response`, `+0x12e` `elevator_node` | E reads (objects.md); V: Halo 1's doors and lifts hold the same values at `mach +0x292`, `+0x294`, `+0x2e8`, `+0x2ea` |
| `ctrl +0x11c` `control_type`, `+0x11e` `control_trigger`, `+0x120` `control_value`, `+0x124` `action_string` | E reads (objects.md) |

Halo 1's devices (`h1objects::h1::Device`, V across the ten levels):
the device layer is `0x17c..0x290` (flags `+0x17c`; six times in seconds
`+0x180..+0x198`, in the order of Halo 2's rates; delay `+0x200`;
activation radius `+0x21c`), then the machine's type `+0x290`, flags
`+0x292`, door open time `+0x294`, collision response `+0x2e8` and
elevator node `+0x2ea`, or the control's type, trigger and value at
`+0x290..+0x298`. A time `t` becomes the rate `1/t`; a zero time is a
zero rate, or 900 in an acceleration slot. A light fixture (`lifi`) is
written as a machine of type platform (no behaviour of its own); its
light is not carried. Sounds and effects stay null (Halo 1's are `effe`
tags the converter does not build). Havok, which moves the player, sees a
device only through a physics model, and Halo 1's have none; through
version 42 a closed door showed, animated and stopped bullets (its
`coll`) but did not block the player. Since version 43 a device with
solid collision gets a `phmo` built from it (below): one rigid body per
collision node, on the model node of the same name, so each part moves
with its node as the door opens, as the disc's doors have (V:
`high_small_door`, a body on each of its three moving nodes). Counted
per level, 86 of the ten levels' 156 device tags get one; the rest have
no collision or none with solid space (most light fixtures).

## Physics models from Halo 1 (`h1objects::physics`, `rebuild::physics_model`)

A Halo 1 collision BSP's 3D tree ends in leaves (open space) and missing
children, which are solid (`h1objects::query`). Every root-to-missing
path is a set of half-spaces; clipped to the BSP's vertex box, their
intersection is a convex cell, and the cells together are exactly the
solid space. `physics::solid_cells` walks the tree and returns each
non-empty cell: its corners (the intersections of three of its planes
inside all of them), its face planes, volume, centroid and inertia.
`collision_disc.rs` (`halo1_solid_space_becomes_physics_pieces`) checks
this on every collision model of the ten levels (16,189 cells): just
outside a surface Halo 1's own tree finds open space 51,729 times and
solid 179, just inside solid 50,591 times and open 1,317 (parts thinner
than the probe step, open meshes); at 142,528 points through the BSP
boxes the tree and the cells agree on all but 117 (points on a face).

`rebuild::physics_model` writes the `phmo` from the definition: each
cell a polyhedron, a body with several a list of them, then the bodies,
one material, one region and permutation naming every body, and the
model's nodes (parent, next sibling, first child) when a body names one.
The layout follows the disc's polyhedron physics models (V:
`high_small_door`, `ghost`, `pelican`):

- a polyhedron of up to 12 corners keeps them inline at `+0x60` in up to
  three groups of four transposed points (capacity `0x80000003`); a
  bigger one takes its groups from the root `four_vectors` block, in
  polyhedron order (only those consume it), capacity `0x80000000 |
  groups`. Planes come from the root `plane_equations` block, in order,
  for every polyhedron; `+0xfc` is the plane count with bit 31 set.
- a list of up to four children keeps them inline (capacity
  `0x80000004`); a longer one takes them from `list_children`, capacity
  `0x80000000 | n`. Retail rigid bodies name a list directly 125 times,
  so no MOPP is written over it.
- shape header `+8` 1.0, `+0xc` 0.9, `+0x10` 0.6, `+0x14` the piece's
  volume, `+0x18` its mass; rigid body `+0x1a` 2, `+0x1e` 5 (retail
  bodies hold 1 to 6 there, 5 on most doors and vehicles; meaning not
  established), `+0x20` 1.0, `+0x28` 0.05; body mass, centre of mass and
  inertia are the pieces' at the body's density.
- the material is `hard_metal_thick_cov` with header `+6` 76, the pair
  the disc's `high_small_door` holds (D: `+6` reads as an index into a
  global material table that is not named here). Halo 1's collision
  materials are not carried.
- the shapes' Havok headers and pointers stay zero; the engine writes
  them at load (`0x001ea9c0`).
- no block of a `phmo` is shared with another tag
  (`tags::repo::build::IN_PLACE`). The loader turns each rigid body's and
  list child's `{type, index}` reference into a pointer in the block
  itself, so a block two tags share is resolved twice and the second
  pass reads a pointer as a reference: the resolver (`0x001eaf30`)
  returns null for it. A fresh map shares equal blocks between tags by
  content, and the first build of these models (a10, 42 on its branch)
  faulted at `0x002fc139` (the list's bounding box reading its first
  child), its cryotube vehicle and
  machine having equal list children; three a10 doors shared rigid
  bodies the same way. `physics_models_load_as_the_engine_reads_them`
  (h1campaign `disc.rs`) checks every converted map's physics models
  against the loader: no rewritten block shared, every reference in
  range, the list-children, four-vector and plane runs filling their
  blocks.

A device's mass is its cells' volume at 1000 kg per cubic metre
(`convert::DENSITY`, per cubic world unit). A body of more than 64 cells
(`physics::MAX_PIECES`; no built body on the ten levels reaches it, the
most is b30's falling box with 62) keeps one enclosing piece
per node instead: the intersection of the cells' own faces that hold
every corner and of 26 axis, edge and corner planes moved out to the
farthest corner. Not done: the `block_30` pairs retail doors hold
(adjacent bodies' nodes, D), constraints, and Halo 1's `phys` mass
points.

Placement (`h1campaign::scenario`): Halo 1's machines (`+0x294`, `0x40`
each), controls (`+0x2ac`, `0x40`) and light fixtures (`+0x2c4`, `0x58`)
and their palettes land in Halo 2's machine (`+0xa8`, `0x48`) and
control (`+0xb8`, `0x44`) blocks, fixtures after the machines. After the
object fields both games hold the power group, position group (`i16`
indices into the device groups, -1 none), the device flags and the
machine's or control's flags: Halo 1 at `+0x28`, Halo 2 at `+0x34`. The
device groups are `h1scripts::level`'s (`+0xa0`, in Halo 1's order); a
conversion that leaves out the level data (`Lean::LevelData`) writes
every group index as -1.

Proof:

```sh
H2_DISC=... H2_DEFS_GROUPS=mach,ctrl cargo test --release -p tags --test tag_defs -- --ignored --nocapture
H1_ISO=... H2_DISC=... cargo test --release -p h1campaign --test disc every_device_placement -- --ignored --nocapture
```

`every_device_placement_lands_with_its_device_groups`: on every level,
every Halo 1 machine, control and fixture placement lands (1,596
machines, 840 fixtures, 77 controls on the disc), at Halo 1's position and
rotation, with its palette entry a `mach` or `ctrl` that re-encodes
byte-identical, its power and position groups Halo 1's and below the
converted device group count, and its flags; all 496 Halo 1 names of
devices name their placement (a fixture's names its machine).

## Weapons, equipment, projectiles and damage (`h1objects::arsenal`)

With `Options::rebuild_items` every Halo 1 `weap`, `eqip` and `proj`, and
the `jpt!` its projectiles deal, is written from its definition with no
retail template bytes (`crates/h1objects/src/arsenal.rs`,
`convert_arsenal.rs`). **Off by default**: the template path stays what
the game runs, because the campaign sweep (h1campaign
`every_campaign_level_converts`, on a10 and b40) finds two things every
retail tag has and the rebuild lacks: material responses on every
rebuilt `proj` (`+0x19c` empty) and modes in the converted first-person
graphs (`jmad +0x54` empty). With the option off the sweep is clean. `proj.txt`
and `jpt_.txt` are new; all 47 retail projectiles and 135 damage effects
round-trip byte-identical (the proof above, now 18 groups). Building one
starts from the definition default, then:

- **Role words** (`arsenal::role`): the turret and biped rule over every
  tag of the group the base map holds, at conversion time. Each 4-byte
  unknown span where at least half hold one nonzero value takes it, and
  the same over all elements of the magazines, barrels and firing effects
  pooled.
- **Halo 1's values**: the named fields below, and by offset the words
  the template path already wrote (S, V2 in h1-objects.md):
  `WEAPON_WORDS` (heat; aim assist, E), `BARREL_WORDS` (rate of fire,
  magazine, rounds per shot, error, spread, first-person offset, heat per
  round), `PROJECTILE_WORDS` (radii, arming time, timer, gravity, damage
  ranges; speeds per tick times 30).
- **References**: converted Halo 1 tags for the model chain, the
  first-person model and animations (two elements, as retail's Chief and
  Arbiter), the projectiles and every damage slot of a projectile
  (impact, and detonation: Halo 1's attached detonation damage or the
  first `jpt!` its detonation effect names). The slots Halo 1 has nothing
  converted for take the **Halo 2 counterpart's reference** (the
  `TEMPLATES` row; references only): the HUD (`nhdt`; Halo 1's `wphi` has
  no converter), melee damage, effects and sounds (Halo 1's `effe` and
  most `snd!` are not converted), the barrels' firing effects. A
  projectile takes no `jpt!` from Halo 2.

Named on the way (V: `weapon_rebuild_disc`
`halo_1_and_halo_2_pairs_agree_on_the_named_fields`, Blood Gulch's
arsenal against Coagulation's counterparts):

| Field | Evidence |
|---|---|
| `weap` magazine `+0x6` `rounds_total_initial`, `+0x8` `rounds_total_maximum`, `+0xa` `rounds_loaded_maximum`, `+0x10` `reload_time`, `+0x14` `rounds_reloaded` | equal to Halo 1's magazine `+0x6`, `+0x8`, `+0xa`, `+0x14`, `+0x18` on 3, 3, 6, 5 and 6 of 8 pairs (no other offset matches) |
| `weap` barrel `+0x36` `firing_noise` | equal to Halo 1's trigger `+0x2e` on 8 of 10 |
| `weap +0x2a8` `first_person` (`model` `mode`, `animations` `jmad`), `+0x2b0` `hud` (`nhdt`) | the retail targets: `fp_<weapon>`, the Chief's and the Arbiter's first-person graphs, `ui\hud\<weapon>` |
| `eqip +0x12c` `powerup_type`, `+0x12e` `grenade_type`, `+0x130` `powerup_time` | equal to Halo 1's `+0x308`, `+0x30a`, `+0x30c` on all four pairs (over shield, active camouflage, both grenades) |
| `proj +0xc8` `collision_radius`, `+0xe0` `maximum_range` | equal to Halo 1's `+0x1a0` on 9 of 10, `+0x1c8` on 3 |
| `proj +0x100` `detonation_damage`, `+0x118` `super_detonation_damage`, `+0x12c` `attached_super_detonation_damage`, `+0x148` `impact_damage` (`jpt!`) | the retail targets' names (`rocket_launcher_explosion`, `needler_shard_*`, every bullet's damage at `+0x148`) |
| `jpt! +0x1c` `damage_lower_bound`, `+0x20` `damage_upper_bound` (from, to) | Halo 1's `+0x1d0`, `+0x1d4`: in both games lower <= from <= to on 6 of 8 pairs, all three equal on the sniper rifles |
| `obje` change colour `+0x8` `block_8` (`0x28`) | filled on the needler shard projectile only |

**Proof** (`every_weapon_equipment_and_projectile_rebuilds`): on the ten
campaign levels (on `05a_deltaapproach`) and the 13 multiplayer maps on
the disc (on their bases), every `weap`, `eqip` and `proj` converts:
316 weapons, 274 equipment, 363 projectiles and 378 damage effects
(counted per map). Each reads back through its definition and writes the
same again, has no raw and no pinned block, and every reference is a tag
of the tree or of the base of its group. Checked against Halo 1: object
type, bounding radius, magazines (counts, reload time), one barrel per
trigger with its firing noise and Halo 1's converted projectile,
first-person model (and animations, 211 weapons; a graph that does not
convert is left null and reported), powerup type, grenade type and time
(20 over shields and 18 camouflages built: they had no template before),
projectile radius, range, speed per second and Halo 1's damage, and every
`jpt!`'s bounds. The test prints the Halo 2 reference slots by group.

What only a game run can show, and the first suspects: firing,
reloading and the HUD (the role words are the majority of the base's
weapons, not the counterpart's; the barrel's unnamed words such as the
firing mode and the magazine's other words come from there); grenades
being thrown and bouncing (no material responses are written:
`proj +0x19c` is empty; the globals' grenade block still names Halo 2's
grenades); powerups (Halo 2's handling of over shield and camouflage
types with Halo 1's times); the first-person arms, since Halo 1's
first-person graph animates Halo 1's arm nodes.

## Which converted objects use the definitions

| Groups | Path | Why |
|---|---|---|
| `vehi` with no template row | definitions (default) | nothing else to build on |
| `mach`, `ctrl`, Halo 1's `lifi` (as `mach`) | definitions (only path) | no template path was ever kept |
| `phmo` of a rebuilt vehicle or device, or a templated vehicle with none | definitions (Halo 1's collision as polyhedra; a stand-in sphere with no solid collision) | `vehicle_new` refuses a model with none; Havok sees a device only through one |
| `scen` | template by default; definitions with `Options::rebuild` | the rebuilt scenery has not run in the game |
| `bipd` of a role (Chief, humans, Covenant) | template by default; definitions with `Options::rebuild_bipeds` | the rebuilt biped has not run in the game; its references are null |
| `weap`, `eqip`, `proj`, and the `jpt!` the projectiles deal | template by default; definitions with `Options::rebuild_items` | the rebuilt projectiles have no material responses and the first-person graphs no modes (the campaign sweep); not run in the game |
| other `bipd`, `vehi` with a template | template | their gameplay fields (speeds, seats, cameras, physics) are unknown spans; built from zero they would not play |

## Open

- **Unknown spans** are largest in the unit layer (`bipd`, `vehi`), the
  `weap` tuning, `mode`'s nested blocks, and the `bitm` and `snd!` roots.
  Each needs the engine's reader found to be named.
- **More groups:** `sky`, `effe` (the second tier asked for), and the
  other object groups (`bloc`, `garb`, `crea`, `ssce`), which
  `struct obje` already covers.
- **Physics shapes from Halo 1**: built from the collision BSP's solid
  cells since version 43. Open: whether a device's bodies need retail's
  `block_30` node pairs, the shape header's material index table, and
  the rigid body words at `+0x18`/`+0x1c`.
- **Rebuilt bipeds**: the HUD, weapons, character and sounds (null
  now), whether the character physics blocks need sizing to the Halo 1
  model, and what `BIPED_WORDS` mean.
- **More converter paths**: the other objects (templated vehicles),
  shaders (`shad` with its postprocess mirrors kept consistent),
  and the sky, each on its definition with its own comparison.
