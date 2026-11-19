# Typed tag definitions (`tags::defs`)

Status: **Built** (2026-11-18) for 18 groups, each proven against every
retail tag of the group on the disc. **Two converter paths use them**:
Halo 1 vehicles that no Halo 2 template stands in for (a10's cryotube,
chairs, fighterbomber and Halo ring), **on by default** since converter
version 33, and Halo 1 scenery, the only scenery path since converter
version 50.
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

- **Our own names and evidence only.** No outside tag definition is used
  for names or layouts. Unverified names elsewhere in the repo (the `S`
  rows of [objects.md](../halo1/objects.md)) were not taken over: those
  fields stay unknown or typed-only unless a disc check (V) or engine code
  (E) supports the meaning, and then the name is ours.
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
| `bipd` | 242 | 14.2 % | 16.1 % | 0 | 68.1 % | 1.6 % | 125 |
| `vehi` | 71 | 8.1 % | 22.7 % | 0 | 66.2 % | 3.0 % | 26 |
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
| `effe` | 805 | 37.5 % | 1.6 % | 0.4 % | 5.1 % | 55.4 % | 168 |
| `ligh` | 190 | 55.6 % | 10.0 % | 0 | 30.0 % | 4.3 % | 190 |
| `deca` | 157 | 44.2 % | 0 | 53.5 % | 2.3 % | 0 | 157 |

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
    barrel's `proj` reference, [objects.md](../halo1/objects.md) V2).
  - The `weap` aim-assist floats (`+0x208..+0x218`) stay unknown here.
    Their offsets are E (`0x1a50a0`); [aim-assist.md](../game/aim-assist.md)
    labels them by those reads (`cone_angle`, `cone_range`, `pull_angle`,
    `pull_range`, `bend_angle`).
    The heat, magazine and barrel scalars are `S` in docs/halo1/objects.md, so
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
  `0x0012c9fb`). The root scalars stay unknown: nothing of ours supports
  their meaning.
- **`jmad`**: [animation.md](animation.md), E fields of the animation
  record, events, modes, classes and actions (`0x001db170`), and static
  poses (`0x001dd8f0`).
- **`snd!`**: the sample rate, encoding, compression (E `0x0021e421`) and
  pitch ranges. Flags, class and playback index stay unknown (nothing of
  ours supports their meaning).
- **`lsnd`**: the Halo 2 layout the Halo 1 sound converter writes
  (`h1sounds::looping`), D.

### Editor-name groups (2026-11-18)

These files exist so the tag editor shows names. They have not been taken
through the disc proof above, only checked against the schema
(`builtin_definitions_agree_with_the_schema`). Each one is the schema
layout with typed-only names (`tagref_44`) and `unknown` spans. On top of
that come names for the offsets our own decoders read, each with its source
file in the meaning text:

- `bloc`, `ssce`, `crea`: the `obje` header (`crea` also has two `jpt!`
  references, from `tags::object` `CREA`).
- `itmc`, `vehc`: `tags::object` `COLLECTION` (E `0x0019f200..`).
- `stem`, `spas`, `vrtx`: `tags::shader` and
  [shader-instances.md](shader-instances.md); `vrtx` from
  `cli::extract_shaders`.
- `ugh!`: `tags::sound` and [sound.md](sound.md). The playback distances
  and the permutation length come from
  [halo1/sounds.md](../halo1/sounds.md) (E). The fields that doc marks S
  stay unnamed.
- `pmov`: `tags::effects::movement`, with the property layout from
  `effects::mem`.
- `MGS2`, `snde`, `fpch`, `fog `: `tags::lighting` and
  [lighting.md](lighting.md). The `snde` names are the reverb API's (I),
  and the reads are E.
- `sbsp`, `ltmp`: `tags::structure`, `geometry`, `collision`,
  `pathfinding` and `lighting`, with
  [structure-bsp.md](structure-bsp.md). The cluster, material and
  instanced-definition elements stay typed-only, because the schema has
  their element sizes smaller than the engine strides.
- `skin`, `wigl`, `wgtz`, `wgit`: `tags::ui` and [ui.md](ui.md).
- `unic`: `tags::localization`.
- `goof`, `sily`: `variants::schema` and
  [community-gametypes.md](../game/community-gametypes.md) (read from the
  XBE).
- `matg`:
  - aim-assist slowdown and follow ([aim-assist.md](../game/aim-assist.md));
  - grenades and player representations (`h1convert::globals`);
  - the UI globals references ([ui.md](ui.md));
  - start levels and level select (`content::campaign_list`);
  - the locale descriptors ([localization.md](localization.md)).

  Some level-select text slots and map ids stay unnamed, because the
  schema has a raw pointer or a raw size at those offsets.

## Scenery from definitions (`h1objects::rebuild::scenery`)

Since converter version 50 every Halo 1 scenery object becomes three tags
written from the definitions (`h1objects::rebuild`), with no retail
template read; the template path (Delta Halo's `statue_shard_large`) and
`Options::rebuild` are gone:

- **`scen`**: `default_record`, then the object type (6), Halo 1's
  bounding radius and offset, the model, the predicted resources (the
  converted `mode` as kind 2 index -1, then every converted bitmap its
  shaders draw as kind 0, as devices and vehicles), and the scenery role
  words (below) in the unknown spans.
- **`hlmt`**: the render, collision and animation models, no physics
  model, the nodes (`h1models::mode::hlmt_nodes`), the regions with the
  collision and physics region and permutation of each (`region_map`),
  and the scenery role words.
- **`mode`**: converted as before (`h1models::mode`, written from zero),
  its **material list** rewritten from the definition: one material per
  Halo 1 shader, the shader set or null, everything else at its default.

**Role words** (`rebuild::scenery_role`, the rule of the turret and biped
roles): per 4-byte unknown span of the `scen` and of the `hlmt` its model
is, the word at least half the base map's scenery holds there when it is
not zero, computed from the base at conversion time. V (2026-11-18): on
05a (56 scenery) the object header's `0x6c..0x8c` (nine floats, the same
words as the turret role's `0x6c..0x74`) and the `hlmt`'s `0x34..0x44`
(five decreasing floats); coagulation (12) and beavercreek (17) give the
same object words (beavercreek also `0x1c`) and no `hlmt` majority.

**Physics** (since converter version 85). Scenery whose Halo 1 collision
holds solid space gets a `phmo` built from it as a device's is (one rigid
body per collision node, on its model node, of the node's solid cells;
"Physics models from Halo 1"), whose bodies take the words the base's
scenery bodies hold (`rebuild::scenery_body`: flags `+0x18`, motion
`+0x1a`, `+0x1e`, the most common over every body of every scenery
`phmo` of the base), and its `coll` then has no BSP physics slots
(retail: nothing with a `phmo` has them). Other scenery with collision
(no solid cell, or a base whose scenery has no `phmo`) keeps the slots.
Through version 84 no scenery had a `phmo`, and in a game run on
converted Blood Gulch (2026-11-19) the player walked through a boulder
with its slots in place, and no Havok body belonged to any of the map's
50 scenery placements (docs/halo1/objects.md, open items). Retail holds
both forms (V: 05a 11 scenery with collision and no `phmo`, 5 with one;
beavercreek 6 and 3; coagulation 0 and 6), so the slots are a retail
form, but which engine path puts a slots-only object in the Havok world
was not found. E, the motion word: the body builder `0x001d5940` maps
`+0x1a` 0..5 to Havok motions 2..7 (jump table `0x001d5bd4`) and makes
the motion 7 whatever the word when its caller asks (`0x001d5a29`).
Proof: `tests/rebuild_disc.rs`, `every_scenery_rebuilds_from_its_definition`
(a `phmo` exactly where the collision is solid and the base has scenery
bodies, every body with their words, no slots beside it) and
`scenery_bodies_take_the_bases_scenery_words` (Coagulation's scenery
bodies hold one pattern, the one taken). Whether the player now stops
at the boulders is the game run's to show.

**Not carried:** Halo 1's attachments (on the ten levels and the
multiplayer maps: 58 `ligh`, 24 `effe`, 13 `lsnd`, 5 `pctl`; no light or
particle definition exists here yet, and effects are their own lane) and
widgets. The attachment block is empty and each object's report lists
what was left.

**Proof, V (2026-11-18)** (`tests/rebuild_disc.rs`):

```sh
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
  cargo test --release -p h1objects --test rebuild_disc -- --ignored --nocapture
```

- `every_scenery_rebuilds_from_its_definition`: 237 scenery objects (the
  ten levels on 05a, the multiplayer maps on coagulation and beavercreek)
  convert; `scen`, `hlmt` and `mode` read back through the definitions,
  write the same again, pin nothing, and name only converted tags (2
  materials take the base's model shader, the shader conversion's
  stand-in for a Halo 1 shader it cannot read); no report holds a
  template stopgap; the type, bounds, model chain (collision and
  animation exactly where Halo 1 has them), region count and role words
  are as above; every `coll` (109) has a BSP physics slot per BSP; every
  model is predicted first, 545 bitmaps after.
- `retail_scenery_has_bsp_physics_and_predicts_its_model`: the retail
  facts above (both physics forms, the model predicted first, kind words
  `0xffff0002` for `mode` and `0` for `bitm`; `hlmt` `block_58` and
  `block_84` empty on 26 and 29 of 05a's 56).

**Comparison with the template path** (before it was removed, commit
`c1a8c1fa`: 187 objects with the template on their base; the 50 on the
multiplayer bases had none, since neither carries the statue). Every
field agreed but these, all the statue's own:

| Field | Template path | Rebuilt |
|---|---|---|
| `scen.object.predicted_resources` | the statue's 8, pinned | the object's model and bitmaps |
| `hlmt.block_58`, `hlmt.block_84` | the statue's one element each, pinned | empty (retail-valid, above) |
| `scen.object.unknown_18`, `scen.unknown_bc`, `hlmt.unknown_28`, `unknown_2c`, `unknown_a4`, `unknown_e4` | the statue's values | zero (no retail majority) |
| null references (`bloc`, `shad`, `effe`, `foot`, `phys`, `udlg`) | group word kept, datum null | `0xffffffff` both |

The `coll` was the same tag on both paths. The role words matched the
statue's wherever they are set.

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
  (22 words in the root, 4 in the physics element). The seat table is
  over those vehicles' 8 driver seats (11 words, the entry distances and
  angles among them); through version 54 it was over every seat, where
  the entry words have no majority and stayed zero. Zero is
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

## Every vehicle from definitions (`h1objects::rebuild::vehicle`)

Since converter version 49 every Halo 1 vehicle is written by
`rebuild::vehicle` from the `vehi` definition. There is no vehicle
template path any more: the `vehi` rows of `convert::TEMPLATES`, the
lifepods' quiet stand-in (`QUIET_STAND_INS`) and the template branch of
`Converter::unit` are gone. a10's cryotube, chairs, fighterbomber and
Halo ring were already built this way.

- **From Halo 1**: the object header (type, bounds, model chain,
  predicted resources), the physics model from Halo 1's collision (as
  above; a stand-in sphere with none), the camera field of view, the
  **vehicle type** (Halo 1 `vehi +0x2f4`, `u16`), the **flags'** low half
  (Halo 1 `+0x2f0`), the seats, the unit's weapons and the powered mass
  points.
- **Seats**: one per Halo 1 seat (`rebuild::seat_names`). The label is
  the rider's animation mode, Halo 1's label as the converted biped
  graphs name it (`h1models::names::map_name`: `W-driver` is
  `warthog_d`, `GT-gunner` `guntower_d`, an unlisted label lower case
  with `-` as `_`), so the rider finds its seat animations. The marker
  (`+8`), entry marker (`+0xc`, `<marker> enter` when the model has
  one) and camera marker (`+0x60`) are the model's marker names as the
  converted `mode` keeps them (Halo 1's, spaces included). The scripts
  keep Halo 1's text: `h1scripts::seats` matches a label equal to the
  text's mode.
- **Seat flags and boarding** (`+0`; `rebuild::seat_flags`). The
  engine's enter-seat search (`0x000c8bb0`) skips a seat whose flags
  close it to the rider (bit 16 for a unit with a player, bit 17 for any
  other), one needing a driver (bit 9) while the vehicle has none, and
  treats bit 11 as a boarding seat (`0x000c92c0`: entered only to take
  the seat its `+0x3e` names). It then needs the entry marker (`+0xc`,
  `0x000c6fb0`) and the rider's marker (`+8`, `0x000c7160`) on the
  model (through `0x000b8d30`: an empty name is the object's origin, a
  missing one fails), the rider within the seat's distances and angles
  (`+0x98`, `+0x9c`, `+0xa0`, `+0xa4`, `0x000c9040`; zero admits no
  one), and the rider's graph to hold the label as a mode with `enter`
  or `enter_a`..`enter_d` (`0x0010f9b0`, the string IDs at
  `0x00467430`). Through version 54 a seat took the majority over every
  retail seat of its type, and most retail seats are boarding seats
  (the warthog has four beside two ridden ones), so every rebuilt seat
  was a boarding seat with no driver bit and no vehicle could be
  entered. Now a seat's words are the majority over the role's seats of
  its kind (driver bit 2, gunner bit 3, passenger; boarding seats left
  out), and its flags are Halo 1's bits 0..10 over the rest of that
  word less bits 11, 16 and 17. An entry word the role leaves zero
  (coagulation's hover drivers share no ghost angle) takes the turret
  drivers' value, and a marker the model lacks is written empty, the
  origin (a10's fighterbomber and the dropships name a `driver` their
  models do not have). V: on 05a bit 11 is set on exactly the
  retail seats labelled `_b_`; Halo 1 sets no bit above 10; on the eight
  pairs Halo 1's bits 0..10 equal the counterpart seat's on most seats
  and the driver bit on every one (`seat_flags_match_halo_2s_counterparts`).
  `every_seat_passes_the_engines_boarding_rule` applies the rule to
  every seat of every vehicle on the ten campaign levels and both
  sandboxes, for the converted Halo 1 player and, on the sandboxes, Halo
  2's `masterchief_mp` (on the seats Halo 1's player enters; Halo 2's
  graph has no mode for the scorpion's side seats, listed, not failed).
- **Friction and hover points** (`+0x2f0`, `+0x2e8`). Halo 1's
  counterparts are the `phys` mass points with a powered mass point:
  the warthog's four tires and the ghost's hull and two wings, the same
  counts as the Halo 2 warthog's four friction points and ghost's three
  hover points. Each becomes a marker of the converted model
  (`Converter::powered_points`, added before the `mode` is converted):
  named as the mass point (`tire_front_left`), at the mass point's
  position in its node's space. V: Halo 1's mass point positions are in
  object space (each warthog tire point lies within its radius of its
  tire node's default position). A point that grips the ground (Halo 1
  powered mass point flag bit 0: the warthog's tires, the scorpion's
  treads; the ghost's antigravity points hold bit 6) rides the root node
  (0); any other point rides its own node. No marker is rotated. V: the
  retail warthog's four tire markers are on node 0, 0.23 below it (the
  wheel centres, 0.19 above the origin, where Halo 1's tire points are),
  facing out along their axles; the scorpion's tread markers are on
  node 0 too, unrotated. Through converter version 75 the tire markers
  rode the tire nodes, which the spin and suspension animations move.
  The axle rotation is not taken: one rule would give the scorpion's
  gripping treads it too, and no game run has shown it helping. In a
  game run (2026-11-19, converted Blood Gulch) the markers, edited live
  to node 0 and facing out before the warthog spawned, left its path
  from the spawn unchanged to the hundredth; a phmo centre-of-mass edit
  did the same, so the engine likely reads both at load and the run
  decides neither.
  The marker has one instance per permutation of the model's first
  region (D).
  The Halo 2 point names that marker (`+0`) and the collision region of
  the collision node named as the model node (friction `+0x44`, hover
  `+0x34`: the warthog's `tire`; the ghost's collision has one region,
  `__unnamed`). Its other words are the type's role (below): friction
  points when the role's vehicles have them, else hover points, else
  none (fliers, dropships, turrets). The friction material (`+0x3c`)
  stays null. Friction points split by steering (flags `+4` bit 2, V: the
  retail warthog's front pair holds `0x26`, its back pair `0x32`): a
  point ahead of the powered points' middle takes the majority over the
  role's steering points, the rest the majority over the others. Through
  version 75 every point took one majority over all four, the back
  pair's word, so no converted wheel steered.
  Since converter version 77 the points are written in the order of
  the role's phantom spheres (`rebuild::role_spheres`: the physics
  element's first multisphere, count at `+0x6c`, centres from `+0x70`,
  `0x10` each): each sphere in turn takes the nearest point left
  (`convert::sphere_order`). E: the engine takes friction point `i`'s
  suspension compression from the phantom's sphere `i` (the per-point
  loop at `0x2067dd`; `0x207ab0` writes the point's radius less the
  sphere's ground distance, clamped to the radius; the spring is the
  role's `+0x2b4` and `+0x2b8`). Halo 1 lists the warthog's back tires
  first and the retail spheres run front first, so through version 76
  each front marker took a back sphere's compression: as the nose sank
  the back spheres rose and the front springs let go, and the converted
  warthog pitched onto its nose at spawn (forward z -0.93, game run
  2026-11-19). Paired by position both Blood Gulch warthogs stood level
  (forward z 0.08 and 0.14 on the slopes) and slept within 70 ticks.
  The friction point's region index (`+0x48`) is its region's own
  index among the `hlmt` regions, -1 for none (V: the retail warthog's
  4, 7, 3, 6, its four wheel regions; the scorpion's -1); through
  version 76 it was the role's word. E: `0x205fd1` turns a point off
  once that region's damage state reaches `+0x42`.
  The rigid body leaves the wheels and treads out (`convert::wheel_nodes`:
  the gripping points' nodes and every node under them) since version
  77: Halo 2 holds a vehicle up on its friction-point springs, and V
  the retail warthog's `phmo` has no wheel shape (a hull on node 0 whose
  lowest point is 0.18 above the origin, and a bumper box). With the
  tires in the body, their lowest point 0.018 below the origin sank up
  to 0.144 into the ground at spawn and the depenetration threw the
  warthog about 4.8 units (game run, 2026-11-19).
- **Control** (`+0x1f2`, the 16 bits after `vehicle_type`,
  `rebuild::VEHI_CONTROL`): the majority over the base's vehicles of the
  same `vehicle_type`. V, every `vehi` on every map of the retail disc:
  2 on the scorpion (the only type 0, tank) and on its two cannon
  turrets (type 6); 0 on every other vehicle: the warthog (1); the
  pelican, phantom, gravity throne and sentinel enforcer (3); the ghost,
  wraith, spectre and creep (4); the banshee (5); and the other 23
  turrets (6). So type 0 takes 2 and every other type 0
  (`tests/vehicle_rebuild_disc.rs`, `control_follows_vehicle_type`).
  Proved in a game run (converted Blood Gulch, 2026-11-19): written 0
  (the role took 4-byte spans only), the converted scorpion crept at
  0.05 units a second with the stick held forward; set to 2 live it
  drove at 2.5.
- **Engine gears** (`+0x268`, `0x44` each, raw curve pointers at `+0x14`,
  `+0x1c`, `+0x28`, `+0x2c`). Halo 1 has no counterpart: its `vehi` holds
  no curve or raw data, only scalar speeds, and those disagree with
  Halo 2's (below). So the gears are the type's role: the count most of
  the role's vehicles with gears hold, each field the majority word,
  raw pointers kept as words (the curves live in the shared map the
  base map uses, so the converted map built on that base reads the same
  ones). On 05a the jeep takes the warthog's four, the tank the
  scorpion's two; hover vehicles, fliers and turrets have none.
- **The type's role** (`rebuild::vehicle_role`): the turret rule over the
  base map's vehicles of the same `vehicle_type`, at conversion time:
  each 4-byte unknown span of the root, every seat, every friction and
  hover point, the plain-word blocks (the physics element `+0x2f8`, `unit
  +0x1a8`, `+0x1b8`), the gears and the flags' high half; the seats by
  kind (above). On 05a: jeep
  the warthog, tank the scorpion, hover the ghost and wraith, flier the
  banshee, dropship the pelican and phantom. The turret type keeps the
  disc-wide turret tables, as does a type the base has none of.
  **Since converter version 84 the role is taken over only the members
  whose friction and hover points together are nearest the Halo 1
  vehicle's powered mass points** (`rebuild::nearest_members`; every
  member on a tie). A majority over vehicles tuned apart leaves zero
  wherever no word has half the votes: Coagulation's hover type is the
  ghost (3 hover points), the spectre (4) and the wraith (6), which
  agree on few driving words, so through version 83 the converted Blood
  Gulch ghost held zero in 16 root words the retail ghost fills
  (`+0x1d0`..`+0x1dc`, `+0x1f4` 5.0, `+0x1f8` 2.5, `+0x204`, `+0x208`,
  `+0x218`, `+0x22c`, `+0x230`, `+0x238`, `+0x254`, `+0x2d8`, `+0x2dc`, `+0x2e4`; V, read
  live from the converted map against the base's three) and zero at
  `+0x8`, `+0x10` and `+0x14` of each hover point (the retail ghost's
  3.75 on its back point and 1.85 on its front pair, 0.5 and 0.45, 1.0),
  and did not move under its driver (user report, converted Blood Gulch,
  2026-11-19). Halo 1's ghost has three powered points, so its role is
  the retail ghost alone (`tests/vehicle_rebuild_disc.rs`,
  `ghost_takes_the_retail_ghosts_role`). The retail ghost's hover points
  are not alike (its back point holds strength 3.75 and height 0.5, its
  front pair 1.85 and 0.45), so with a one-member role whose hover
  points match Halo 1's in number each converted point takes one retail
  point's own words (`VehicleRole::hover_each`), paired by position:
  each retail point's marker is placed in the vehicle's space through
  its render model's rest pose (`rebuild::marker_positions`: the hover
  point's marker `+0` names a `mode` marker group, whose first marker's
  translation is carried through its node), and in the retail order each
  takes the nearest Halo 1 powered point left (`convert::sphere_order`,
  as friction points take phantom spheres). With a marker missing or
  counts that differ, every point takes the majority as before.
- **Seat flag bit 15** (`rebuild::SEAT_OPENS`). E: on entering a seat
  the engine tests bit 15 of the seat's flags (`0x000d12b0`, the test at
  `0x000d1302`; called from the seat change at `0x000ea979`, `0x000eab64`,
  `0x000eb1ff`) and, set, sends the vehicle unit action `0x17`; on
  leaving (`0x000d1360`, test at `0x000d13b2`; from `0x000ea7a5`,
  `0x000ea8ba`) action `0x18`. The action table at `0x004677c8` gives
  them `0x000e9370` and `0x000e9500`, which start the animation state
  `0x0700005c` or `0x0700005d` (string IDs of seven characters, D:
  `opening` and `closing` by length, not read off the string table) on
  the vehicle and the objects attached to it. No other read of bit 15 of
  a seat's flags was found (every `vehi +0x1cc` load followed by a bit 15
  test, over the whole `.text`). The retail ghost driver holds it
  (`0x801c`); Halo 1 has no such seat flag and its graphs no such state,
  so since converter version 84 a rebuilt seat never takes it from the
  role (the one-member ghost role would have carried it). It is not a
  grenade switch: the converted ghost's driver throwing grenades is not
  explained here.
- **References**: the unit's weapons are Halo 1's converted weapons
  where they convert (no Halo 1 vehicle weapon converts yet, each listed
  in the report). Since version 50 the attachments are Halo 1's looping
  sounds and effects and the creation effect Halo 1's (Effects from
  definitions, below). HUD (`unhi`) and damage stay null: no converted
  Halo 1 tag exists.
- **Camera tracks** (seat `+0x74`, `rebuild::SEAT_CAMERA_TRACKS`): each
  seat's first Halo 1 camera track (seat `+0xd0`, `0x1c` each; the
  warthog driver's loose one), converted to a `trak` with the same
  control points (`rebuild::camera_track`; `tags/src/defs/trak.txt`).
  The two games share the layout: Halo 2's
  `globals\default_unit_camera_track` holds Halo 1's default unit
  track's nine points value for value (`tests/vehicle_camera_disc.rs`).
  Through version 75 no seat named a track, so the engine used that
  default, whose top point is 3.9 above the camera marker where the
  warthog's own is 3.5 (Halo 1) or 2.05 (the retail warthog's).
  Since version 77 each point's orientation is written conjugated
  (`h1models::h1::h2_rotation`): Halo 1 stores rotations as the
  conjugate of Halo 2's. E: the following camera (`0x23cbb0`) bisects
  the track (`0x23c980`, sampling with `0x23c300`) for the point whose
  forward axis's elevation matches the look pitch, assuming elevation
  rises along the track. V: every retail track rises from -90 to +90
  degrees; Halo 1's, read unconjugated, fall, so the search ended on the
  first point, the top one, 3.5 above the warthog's camera marker
  whatever the pitch (`tests/vehicle_camera_disc.rs`,
  `retail_tracks_rise_halo_1s_fall`). Halo 2's default unit track is a
  byte copy of Halo 1's and falls too, so a seat Halo 1 gives no track
  takes the unit's (`unit +0x1f4`), else Halo 1's `globals\default unit
  camera track`, converted. Not yet checked by eye in a game run.

Named on the way:

| Field | Evidence |
|---|---|
| `vehi +0x1ec` `vehicle_flags` (`u32`) | E: objects.md (vehicle code tests bits here, `0x000efb8c`). V: the low half equals Halo 1's `vehi +0x2f0` on five of eight pairs (warthog, scorpion, wraith, banshee, Covenant gun turret); the ghost differs in bit 8, the pelican and dropship in bits Halo 2's fliers add |
| `unit +0x1c0` `weapons` (`weapon`, `tagref weap`) | V: every element of every vehicle on 05a and Coagulation that has one names a `weap` (the turrets' guns, the warthog's horn, the ghost's, banshee's and phantom's guns) |
| Halo 1 `vehi +0x2f4` vehicle type | V: equal to Halo 2's `vehicle_type` on all eight pairs; the games share the enumeration |

Halo 1's speeds do not carry: on the same pairs no Halo 1 word agrees
with a Halo 2 driving word on more than one pair (the warthog's Halo 1
`+0x2fc`, `+0x300`, `+0x304` times 30, 900 and 900 are Halo 2's `+0x1f8`,
`+0x1fc`, `+0x200` exactly; every other vehicle was retuned for Havok).

Proof (`crates/h1objects/tests/vehicle_rebuild_disc.rs`, lib tests
`rebuilt_vehicle_is_the_definition_with_seats_and_the_turret_role` and
`rebuilt_vehicle_takes_its_types_role_and_halo_1s_type`):

- `vehicle_type_and_flags_match_halo_2s_counterparts`: type 8 of 8
  pairs, flags low half 5 of 8, camera 8 of 8.
- `every_vehicle_rebuilds_from_its_definition`: all 29 distinct vehicles
  (23 on the ten campaign levels on 05a, 3 on each multiplayer base,
  Coagulation and Beaver Creek) rebuild; the `vehi`, `hlmt` and `phmo`
  read back, write the same again, pin nothing and hold no template
  bytes; type, flags, camera, bounds, seat labels and markers are Halo
  1's, the role words the type's. (Before the template path was removed
  the same test found the model chain equal to the template path's on
  all 23 vehicles with a template row.)
- `powered_mass_points_become_friction_and_hover_points`: the warthog
  has 4 friction points and gears, the ghost 3 hover points; each names
  a marker of the converted `mode` (instances on nodes in range) and a
  region of Halo 1's collision.

What only a game run shows: driving (the friction points on Halo 1's
tire markers with the warthog's gears, the scorpion's eight tread points
where Halo 2's has twelve), hovering (the ghost's and wraith's points),
flying for the fliers and dropships, whether a vehicle stands on its
Halo 1 collision hull, boarding (the seat labels as modes, the entry
markers) and the riders' poses. First suspects if driving fails: the
marker instances' region and permutation bytes, the null friction
material, and the gears' curves read through another vehicle's raw.

## Bipeds from definitions (`h1objects::Options::rebuild_bipeds`)

On by default since converter version 48 (h1objects' and h1campaign's
`Options::rebuild_bipeds`; off builds every biped on a Halo 2 template as
before). Every Halo 1 biped is written from the `bipd` definition
(`rebuild::biped`):

- **Roles** (`h1objects::rebuild::BipedRole`): the Chief (`cyborg`),
  humans (marines, crew, the captain, pilots, Cortana, Johnson) and the
  Covenant (elites, grunts, jackals, hunters, engineers). The flood,
  sentinels and the monitor have no role of their own and take their
  counterpart's (`Converter::biped_rebuild_role`): the Chief's, as the
  template path built them on the Chief; the flood combat elite the
  Covenant's, the flood captain humans'.
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
  (`hlmt` from the definition, as for rebuilt scenery) and predicted
  resources (the render model, then its bitmaps), the camera's field of
  view, the five `BIPED_WORDS`, the collision size and the character
  shapes sized to it (below), and a physics model built from the
  collision (below).
- **From the counterpart** (`rebuild::COUNTERPARTS`, the Halo 2 biped
  nearest the Halo 1 one that the base holds: masterchief, marine,
  crewman, elite, grunt, jackal, bugger; else the template path's
  template): every reference field of the `bipd` and of its `hlmt`
  (`rebuild::reference_fields`: each tag reference of the root, and each
  block whose elements hold one at any depth), as the counterpart holds
  it, less the model (`BIPED_OWN`) and the `hlmt`'s chain, nodes,
  regions and variants (`MODEL_OWN`); and the object functions, which
  attachments and change colours name. On the Chief that is the
  material effects (`foot`), the shield-down effect and flashlight
  attachments, the flashlight effect, the camera track, the melee and
  boarding damage, the HUD interfaces, the blood effect, and on the
  `hlmt` the damage block (`+0x60`: shield and body effects and shaders,
  damage sections), the dialogue and the camouflage shader. The base is
  read through the definitions (`Base::record`): string IDs keep their
  indices and references their datums, as the template path's copied
  bytes do.
- **The player** (`health_hud::is_player_biped`) takes the counterpart's
  HUD interfaces as health-meter copies (`*_h1_health`, docs/halo1/health.md),
  as the template path does.
- **Halo 1 references with no converter here** (`unhi` HUDs, `effe`, `jpt!`, `actv`, `snd!`) are not carried: those come
  from the counterpart. Halo 2's `bipd` holds no weapons (`unit +0x1c0`)
  or seats (`bipd +0x1c8`) on any base biped, and its two `char`
  references (`bipd +0x300`, `+0x308`) are null on all of them: an AI's
  character is the scenario's (h1ai), not the biped's.

Character shapes (E, V):

| Field | Evidence |
|---|---|
| `bipd +0x268`, `+0x26c`, `+0x270` (`rebuild::BIPD_SIZE`): standing and crouching collision height, collision radius | V: every retail biped of the base with shapes holds shapes derived from these three (below); Halo 1 holds the same three at `bipd +0x424`, `+0x428`, `+0x42c` (`h1::BIPED_SIZE`): the cyborg's 0.7, 0.5, 0.2 against the Chief's 0.725, 0.5, 0.175 |
| `bipd +0x284` spheres (`0x80`), `+0x28c` pills (`0x50`), `+0x294` spheres | E: the `bipd` post-process `0x001c2390` (from the tag load dispatch `0x0011c840`, on `bipd +0x264`) writes each sphere's Havok header at element `+0x20` and a transform's at `+0x30` with the sphere as its child (`+0x3c`), each pill's at `+0x20`; `0x001c2460` returns pill `i` (standing 0, crouching 1) as the character's shape, else sphere `i` of `+0x294`. V: a pill runs from (0, 0, radius) to (0, 0, height - radius), the radius in both ends' `w` and at `+0x2c`; the three `+0x284` spheres have radii 1, 1.35 and 1.7 times the collision radius, each resting on the ground (`+0x78` its radius); `+4` holds `0xffff` and `+0x1e` `0xffff` |

A rebuilt biped writes Halo 1's size and builds both blocks from it
(`rebuild::biped_shapes`, runtime words zero); one whose Halo 1 heights
are zero (seven of 42: the pilot, the engineer, the flood infection
forms, the monitor, the sentinel) keeps its role's.

Physics model: one rigid body per Halo 1 collision node with solid
cells, on the model node of that name (as devices,
`PhysicsKind::Biped`), with the rigid body words `+0x1a` 2 and `+0x1e` 1
every rigid body of every retail biped physics model on the base holds
(`rebuild::BIPED_BODY`, 80 bodies). Retail's are ragdolls: ten bodies
of pills and a box joined by node edges (`phmo +0x30`), four limited
hinges (`+0xe8`) and five ragdoll joints (`+0xb8`)
([collision.md](collision.md), "Constraints"). A built biped's bodies are
joined the same way (`h1objects::ragdoll`, `convert::biped_ragdoll`):
each body to the body of its nearest ancestor node, knees and elbows as
limited hinges, every other joint (hips, shoulders, head, spine, feet,
hands) a ragdoll joint, frames from Halo 1's rest pose (the child's frame
on its origin, the parent's the same frame carried by the child's rest
transform, as retail's), limits our own inside retail's range for each
joint, one edge per constraint with friction 0.25. A skeleton that does
not map (under three bodies, more than one body with no ancestor body, or
no hip and knee by name) keeps its unjoined bodies and the report says
so. A biped Halo 1 gives no collision (the a10 pilot, d40's
cinematic Johnson) gets none, as retail's cinematic characters (miranda,
regret_holo) have none; the template path borrowed the template's.

Named on the way:

| Field | Evidence |
|---|---|
| `unit +0xcc` `camera_field_of_view` (`f32`, radians) | V: 70 degrees on every Halo 2 biped and turret with a Halo 1 counterpart, and equal to Halo 1's `unit +0x1a0` on five pairs (cyborg and masterchief, marine_armored and marine, elite, grunt, jackal). Name D: a field of view is the plain reading of 70 degrees on a unit; no reader found yet |

`rebuild::BIPED_WORDS` carries five more `bipd` words from Halo 1 as they
are: on the same five pairs each agrees within 1 % with one Halo 1 word
(`+0x1f4` with Halo 1's `+0x2f8`, `+0x220` with `+0x408`, `+0x26c` with
`+0x428`, `+0x2a0` and `+0x2a8` both with `+0x360`). They hold angles
in radians and small ratios; their meaning is not established, so they
stay unknown spans (`+0x26c` is the crouching height above).

Proof (`crates/h1objects/tests/unit_rebuild_disc.rs`, and the lib tests
`rebuilt_biped_is_the_role_with_halo_1s_header_and_words` and
`rebuilt_biped_is_sized_to_halo_1_and_takes_its_counterparts_references`):

- `biped_words_match_halo_2s_counterparts`: the camera and the five
  words, Halo 1 against Halo 2, on the five pairs.
- `biped_shapes_follow_the_collision_size`: the shape rule on the 11
  retail bipeds of the base with shapes, and `BIPED_BODY` on their 80
  rigid bodies.
- `bipeds_rebuilt_against_the_template_path`: all 42 Halo 1 bipeds on
  the ten levels (3 player, 11 of no role of their own), each built both
  ways. The rebuilt record reads back, writes the same again, pins
  nothing, and holds no template bytes; it has the template path's
  object type, bounding radius and model chain, Halo 1's camera, words
  and size with shapes sized to it, and its role's words; every
  reference field of the `bipd` and `hlmt` is the counterpart's
  (19 or 20 fields), every reference resolves to a tag of its group, and
  none the template path held is lost where the counterpart has one; a
  physics model is built from Halo 1's collision for the 40 that have
  one, every biped the template path borrowed one for (33) but the two
  with no collision; the player's HUD interfaces are health-meter
  copies.
- h1campaign's `every_campaign_level_converts` builds all ten levels
  fresh with rebuilt bipeds (0 dangling references, tag memory at most
  0x923000 of 0xc24000 on b40), and `physics_models_load_as_the_engine_reads_them`
  walks a10's 88 physics models as the engine's post-process does.

What differs from the template path, the suspects if a rebuilt biped
misbehaves: the unknown words are the role's majority where the template
path kept one biped's; humans take the marine's references where the
template path put them on the Chief (so a marine has no HUD and the
marine's sounds and effects); the built physics models have no ragdoll
node pairs, so a dead body's pieces may not hold together; the character
shapes are Halo 1's size (a marine's radius 0.18 where Halo 2's is 0.12).

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

## Effects from definitions (`h1objects::rebuild::effect`)

`crates/tags/src/defs/effe.txt` is the Halo 2 `effe` from
[effects.md](effects.md) (strides V by tiling, the E readers there), with
`struct property` for the emitters' sixteen-byte properties. All 805
distinct retail effects on the disc (14,785 instances) round-trip
byte-identical with no pinned values. Named on the way:

| Field | Evidence |
|---|---|
| part `+8` `type_group` (`u32`) | E: `0x0017a8a0` dispatches on it (effects.md). V: the reference's group, `obje` for an object |
| particle system `+0x16` (unknown, was taken for padding) | V: nonzero on 347 instances |

**Halo 1's `effe`** (`h1objects::h1::Effect`, V on every effect of every
map of the Halo 1 disc, `effects_disc.rs` `halo1_effect_layout_survey`,
7,300 tags): root `0x40` (flags `+0`, loop start and stop events `+4`,
`+6`, locations `+0x28` of `0x20`: a marker name, events `+0x34` of
`0x44`); event: `+4` skip fraction, `+8` delay bounds, `+0x10` duration
bounds (`+0..4` and `+0x18..0x2c` zero everywhere), parts `+0x2c`
(`0x68`), particles `+0x38` (`0xe8`, a `part` reference at `+0x54`);
part: `+0` creation medium, `+2` violence mode, `+4` location, `+6`
flags, `+0x14` a runtime word, `+0x18` the reference, whose group word is
the part's kind on every part (`snd!` 5,031, `deca` 4,804, `jpt!` 342,
`ligh` 265, `pctl` 203, `garb` 131, `proj` 73), `+0x28..0x40` zero, then
from `+0x40` the same quantities in the same order as Halo 2's part from
`+0x14`: velocity bounds, cone angle (radians, 1 to 80 degrees), angular
velocity bounds, radius modifier bounds, and the two scale words at
`+0x60`, `+0x64` (`0x20`, bit 5, on the same seven parts in both words,
the bit Halo 2's engine tests). Halo 1's attachments (`obje +0x140`,
`0x48`): the attached tag at `+0`, its marker at `+0x10` (32
characters).

A Halo 1 effect (`Converter::effect`) is written from the definition with
no retail template read:

- **locations**: Halo 1's marker names as string IDs;
- **events**: Halo 1's skip fraction, delay and duration bounds;
- **parts**: the ones that convert here, each with Halo 1's creation
  medium (D: Halo 1's 1 is "air only", Halo 2's 1 skips the part
  underwater), location, flags, velocity, cone, angular velocity,
  radius modifier and scale words, the `type_group` and the reference:
  - sounds through `h1sounds` (`Converter::sound`, once per sound);
  - projectiles through the object converter (none convert yet on the
    disc: no template row for an effect's projectile);
  - lights and decals (since version 51, below);
  - damage (`jpt!`, the weapons lane's definition is not on master
    yet), particle systems (`pctl`) and debris (`garb`) are left out and
    counted in the effect's report.
- **particles** (Halo 1's `part`) are left out: Halo 2's are `prt3` on
  shader templates (`stem`), a separate job.
- Everything else is the definition's default (flags, the loop event,
  the root's unknown words zero; no beams, accelerations or particle
  systems).

The converted sounds and looping sounds need the map's sound gestalt
(`ugh!`), so a `Tree` holds them apart (`Tree::sounds`, `Tree::loops`)
and the tags name them by placeholders (`convert::sound(k)`,
`convert::looping(k)`). `Tree::install_audio` (called by
`Tree::install` after the tree's own tags, so those land in the slots the
Halo 1 sandbox's placements were promised; h1campaign calls it before
the objects) adds them all or none, reusing a sound or looping sound the set has by path; a refusal
(the gestalt's 16-bit indices) or `install_over` (no new slots) leaves
every placeholder null and a note.

**Attachments.** A rebuilt vehicle's attachments (`obje +0x94`: the
type and the marker; E `0x000bed2a` builds `ligh`, `lsnd`, `effe` and
`cont`) are Halo 1's looping sounds (`h1sounds::looping`) and effects at
Halo 1's markers, and its `creation_effect` Halo 1's (`obje +0xa0`).
Lights too since version 51 (below); contrails are left out (no
converter). On the ten levels: the warthog's, scorpion's, banshee's,
pelican's, dropship's and gun turret's engine loops and lights, and
a30's `lifepod_entry` its effect and loop. The plain lifepods carry only
two lights in Halo 1, which they now have; they stay quiet, as in Halo
1.

Proof (`crates/h1objects/tests/effects_disc.rs`; lib tests
`rebuilt_effect_keeps_halo_1s_events_and_the_parts_that_convert` and
`audio_placeholders_become_null_when_the_sounds_have_no_room`):

- `every_effect_rebuilds_from_its_definition`: every `effe` (7,271) and
  `vehi` of the ten campaign levels (on 05a_deltaapproach) and the
  multiplayer maps (on Coagulation and Beaver Creek) converts; every
  built effect reads back through the definition, writes the same again
  and pins nothing; its locations' markers, event count and skip
  fractions are Halo 1's; every part's reference resolves (a converted
  sound, or a tree tag of the part's group) with a matching
  `type_group` and a location in range; every vehicle attachment
  resolves to a converted looping sound or effect at a marker and of a
  group Halo 1's attachment has. 4,969 parts kept; left out: 4,801
  decals, 338 damage, 260 lights, 201 particle systems, 131 debris, 73
  projectiles, 46 null sounds.
- h1campaign's `every_campaign_level_converts` installs the objects'
  sounds with the gestalt and builds all ten levels fresh at lean
  `Everything` with 0 dangling references: 0 to 1 effect sounds and 0 to
  11 looping sounds per level (at most 0x1f8ad4 sample bytes, b40), no
  file size retry; tag memory at most 0x942000 of 0xc24000 (b40, was
  0x918000).

What only a game run shows: the effects' sounds playing at their
markers (the engine's part creation from the converted parts), the
vehicles' engine loops scaled by Halo 2 functions the rebuilt vehicles
do not have (Halo 1's attachments name no Halo 2 function: the loops
may play at a fixed level), and whether Halo 1's per-second part
velocities need a factor (no projectile or debris part is kept yet,
so nothing moves).

### Lights (`ligh`, version 51)

`crates/tags/src/defs/ligh.txt` is the Halo 2 `ligh` of
[effects.md](effects.md) (E readers in
[e3-lighting.md](../graphics/e3-lighting.md), "The light record"). All
190 distinct retail lights round-trip byte-identical.

Halo 1's `ligh` (`h1::Light`, `0x160`, no blocks; V on 304 lights of six
maps, `halo1_group_word_survey` with `H1_GROUP=ligh`): `+4` a radius
(0.05 to 15 world units), `+0x10` and `+0x14` two angles in radians (20
to 180 degrees, a half turn on most: a sphere), `+0x38` and `+0x48` two
colours as alpha, red, green, blue, `+0xac` a lens flare, `+0xf4` a
duration on 59 lights in multiples of three (3 to 180, read as ticks).
Names D.

`rebuild::light` writes, with no template: a sphere of Halo 1's radius
(radius and specular radius), or under a half-turn cutoff a frustum
(field of view the cutoff angle, falloff and cutoff distance the
radius, stretch 1); Halo 1's lower and upper colours as both the diffuse
and specular colours at the two intensity bounds; flag bit 3, no shadow
(Halo 1's dynamic lights cast none); the duration in seconds (ticks
over 30, D); and the base's lights of that kind as a role
(`rebuild::light_role`: the majority word of each unknown span and the
majority brightness bounds). Lens flares are not converted. Lights are
built for effect parts and object attachments; all 898 lights on the
disc's maps (95 cones) rebuild and read back.

### Decals (`deca`, version 51)

`crates/tags/src/defs/deca.txt`: all 157 distinct retail decals
round-trip byte-identical; the zero spans are pad. Halo 1's `deca`
(`h1::Decal`, `0x10c`; V on 270 decals of six maps) holds the same
fields in the same order as Halo 2's: flags (`+0`, the same bits, `8`,
`0x68`, `0x168`), kind (`+2`, 0 to 3), the next decal (`+8`), the radius
bounds (`+0x18`), two colours (`+0x34`, `+0x40`), the lifetime and decay
time bounds (`+0x78`, `+0x80`, seconds; Halo 2's retail 180 and 20 sit
there), the bitmap (`+0xd8`) and 16.0 at `+0xfc` (Halo 2's `+0xa4`).
`rebuild::decal` writes those from Halo 1, the bitmap converted by
`h1bitmaps`, the next decal converted in turn, and the layer, overlap
count and `+0x18` scale from the base's decals' majority
(`rebuild::decal_role`; Halo 1's layer, 0 to 3, is not Halo 2's 1 to 5).
All 916 decals on the disc's maps rebuild with their bitmaps.

Proof: `every_effect_rebuilds_from_its_definition` (above) also builds
every light and decal of every map and reads them back (kind, range,
colours; radius, lifetime, bitmap and next decal resolving in the tree):
10,030 effect parts kept. The campaign sweep stays at lean Everything
with 0 dangling references, b40 at tag memory 0x942000.

What only a game run shows: the light's brightness against Halo 1's
(the role's bounds with Halo 1's colours), the cone lights' look, the
duration factor, and the decals' layer and blending (the role's layer
for every decal).

**Halo 1 sandbox import** (`sandbox_disc.rs`
`every_map_builds_in_both_sandboxes_with_the_objects_sounds`): every
multiplayer map on the disc through `h2 import halo1`'s path in both
sandboxes (26 builds), every attachment of every converted vehicle (587)
naming a tag of its group in the built map. The placements name the
objects and the collections after them by datum, so the sounds the
objects play go in last: `ObjectTags::install` adds the tags alone
(`Tree::install_tags`) and `ObjectTags::after`, run once the collections
are in, adds the sounds and points the tags at them
(`Tree::install_audio_for`). The cli's
`every_multiplayer_job_builds_in_both_sandboxes` builds every map through
the import's own job function (`DiscConverter::multiplayer`).

### Dialogue (`udlg`)

`crates/tags/src/defs/udlg.txt`: all 42 distinct retail `udlg` round-trip
byte-identical. A Halo 2 `udlg` is a list of vocalizations, each a
string ID name and a `snd!`, plus an `adlg` reference (null on every tag)
and a per-tag string (a three-letter designator, `jon`, `grf`, `dop`;
null on the Chief's, the multiplayer elite's and the drone's). On 05a
its 17 tags use 330 names, Halo 2's AI vocabulary (`pain_mjr`,
`dth_fall`, `seefoe`, `chr_kllfoe`, ...).

Halo 1's `udlg` (`0x1010`; 102 distinct on the disc, `effects_disc.rs`
`halo1_dialogue_survey`) is a fixed table of `snd!` references, a line
per offset, the sound's own name saying which on every tag that sets it
(`+0x70` pain minor, `+0xf0` death quiet, `+0x740` grenade throwing,
`+0x360` killed enemy, ...).

Converted since version 53 (`h1objects::rebuild::DIALOGUE`,
`Converter::dialogue`): a table from Halo 1 offsets to Halo 2
vocalization names, 41 pairs. The Halo 1 slot names are V (the sound
names); the Halo 2 names are V (every one is used by a retail `udlg` on
05a; the disc test asserts it); the pairing is D by meaning, except pain
and death, where the two retail dialogue tags whose sounds are not named
after the vocalization (the Chief's and the multiplayer elite's) give
the bridge: their `death` plays for `dth`, `dth_mjr`, `dth_slnt`,
`dth_slw` and `dth_hdsht`, their falling death for `dth_fall`, their
`pain` for `pain_mjr`. Where two slots feed one vocalization the first
holding a sound wins. The hurt and killed lines by weapon and species,
the replies, the search and vehicle chatter have no Halo 2 name close
enough and are left out. The root's `adlg` word, null datum and `snd!`
group words are as on every retail tag; the designator string is null
(D: what reads it is not established).

A rebuilt biped's model (`hlmt +0x8c`, `DIALOGUE_FIELD`) names the
converted dialogue of the first `udlg` its Halo 1 unit names, in place
of the counterpart's. V (`every_dialogue_converts`, ten campaign levels
and the multiplayer maps): 102 tags, 6,502 slots holding a sound, 2,140
Halo 2 lines, all reading back with no pinned bytes and every sound a
converted one; 125 of 169 bipeds name dialogue, 21 more than one (only
the first is kept). The sounds are large (0xa0acae4 sample bytes over
all maps), so h1campaign's file size ladder has a dialogue step: after
the music, a level over the file limit gets a dialogue budget
(`Options::dialogue_budget`) that much smaller, down to none, before the
ambience is cut. A line whose sound would pass the budget is left out.
What only a real run can confirm: that the AI plays the lines (the
engine's reader of the vocalization list is not traced).

### Material effects (`foot`)

`crates/tags/src/defs/foot.txt`: all 95 distinct retail `foot` round-trip
byte-identical. A Halo 2 `foot` is a block of kinds by index, each with a
sounds block and an effects block of `0x18` elements: the tag (an effect
has a second, null on characters), a global material name and, at
`+0x14`, that name's index among the globals' materials (V: every
element's index names its string; `default_material` 0), then a word
that is 2 on most sound elements and 1 on every effect element (what
reads it is not established; written as the majority). On every Halo 2
character's `foot` the kinds 0, 1, 3 and 4 play walk, run, shuffle and
jump sounds and kind 2 none (`halo2_foot_survey`).

Halo 1's `foot` (`h1::Foot`, `halo1_foot_survey`) is the same idea: six
kinds (`0x1c`), each 32 materials (`0x30`: an effect at `+0`, a sound at
`+0x10`) in Halo 1's material order, the kinds named by their sounds
(walk, run, none, shuffle, jump up, landing). Converted since version 54
(`Converter::foot`, `rebuild::foot`): each kind at its index, each
material's sound (`h1sounds`) and effect (`rebuild::effect`) under the
Halo 2 name the weapons lane's `arsenal::MATERIALS` gives it, the first
Halo 1 material winning where two share a name, with the base's index of
that name. Retail Halo 2 feet also list `default_material` and the
coarse classes (`soft`, `tough`, `hard`); Halo 1 has no such entries,
so none is written. Every rebuilt biped and vehicle naming a Halo 1
`foot` names the converted one (`obje +0x54`, `FOOT_FIELD`) in place of
the counterpart's. V (`every_foot_converts`): 137 Halo 1 feet over the
campaign and multiplayer maps, 3,898 sound and 306 effect elements, every
material index naming its string on the base, every tag a converted one,
183 rebuilt units pointing at theirs. Only a real run shows whether the
steps play: the engine picks an element by the surface's global
material, and the converted BSPs' surfaces may name materials other than
these 32 (h1convert does not use this table), which then find nothing.

### Lens flares, debris and light fixtures (`lens`, `garb`, `lifi`)

**Lens flares.** `crates/tags/src/defs/lens.txt` is the Halo 2 `lens`
of [effects.md](effects.md); all 160 distinct retail lens flares (and the one `garb`) round-trip
byte-identical. Four engine paths submit a flare through
`0x0002dba0` (**E**, the calls found by a scan of the XBE): sky lights
(`0x0003f1e9`, `sky` light `+0x14`), lights (`0x000c34ac`, reading the
`ligh` datum at `+0x94`), object attachments (`0x00041b23`) and effect
parts in the effect render (`0x00179d23`, the part's datum at `+0x10`;
the part *creation* switch skips `lens` at `0x0017a8bd`). The
submission reads `+0x18`/`+0x1c` (no flare past the far distance; equal
values switch the test off), the flag bits at `+0x28` (2 or 3 drop it,
by the caller's kind) and `+0x2a`, and `+0x16` (a second record).

Halo 1's `lens` (`h1convert::lens::H1Lens`; V, all 641 lens tags of
the Halo 1 disc, `flares_disc.rs` `halo1_layout_survey`) is a `0xf0`
root with its reflections at `+0xc4` (`0x80` each). Its first `0x30`
bytes are Halo 2's in the same order (V by range: the two angles with
their cosines, a radius of 0.001 to 50, a `u16` of 0 to 2 at `+0x14`,
the two fade distances, 1 to 15 and 4 to 250, the bitmap at `+0x20`),
and Halo 1's two scales at `+0xa0`/`+0xa4` hold Halo 2's range at
`+0x34`/`+0x38` (0.1 to 8, 1 on most). A reflection pairs by value
sets (V, `halo1_layout_survey` with `H1_STRIDES=c4:80` against
`halo2_lens_survey`): Halo 1's `+0` flags (2, 4, `0xc`), `+4` bitmap
index (1 to 6), `+0x1c` position (the very same words, 0.15 to 0.61),
`+0x20` rotation offset (20), `+0x28` radius bounds, `+0x34`
brightness bounds and `+0x44` tint (red, green, blue after an alpha of
1) are Halo 2's `+0`, `+4`, `+8`, `+0xc`, `+0x10`, `+0x18` and `+0x24`.
`h1convert::lens::rebuild` writes those (the cosines recomputed) and
the bitmap converted by `h1bitmaps`. Not carried: Halo 1's flags (D:
its one set bit has no known Halo 2 meaning, and Halo 2's bits 2 and 3
hide the flare), the reflections' scaling sources (`+0x30`, `+0x3c`),
colour bounds (`+0x50`, `+0x60`) and animation (`+0x70`), and the
rotation function (`+0x80`, `+0x84`); the function blocks stay empty,
as on most retail flares (115 of 161 have none at `+0x4c`).

Halo 1 names lens flares from lights (`ligh +0xac`, 656), sky lights
(20), and two shader groups (`senv +0x30`, 17; `sotr +0x38`, 21), and
nothing else (V, `halo1_reference_survey`). Converted since version
55 (skies) and 56 (lights): a light's flare (`Converter::light` sets `ligh +0x90`),
a sky light's (`h1convert::sky::Sky::flares`, written to `sky` light
`+0x14` by `h1sky::convert::convert_sky_with_flares`). A shader's flare
has no Halo 2 home: no Halo 2 shader names a lens on the disc (V,
`halo2_reference_survey`: lens references come from `ligh`, `sky `,
`effe` parts and object tags only), and Halo 1 draws them only where
its lightmapper placed them in the structure, which the structure
converter does not read; they are left out.

**Debris.** Halo 1's effects throw `garb` objects (131 parts on the
disc; nothing else names a `garb`). Halo 2 keeps the group: the engine
type table has `garb` at index 4 with no scenario placement block (E
`0x00468630`), and an effect part of type `obje` creates any object
class its reference names (E `0x0017ab39`). Retail Halo 2 effects throw
`bloc` crates through `obje` parts (1,156 parts) and `proj` (4), and its
debris meshes are particle models (`PRTM`, 243 tags); its one `garb`
(the SMG's dropped magazine, 30 instances) is no effect's part (V,
`halo2_reference_survey`). `bloc` needs a Havok physics model and
`PRTM` the particle system lane's `prt3`, so Halo 1's garbage stays
garbage (D). `crates/tags/src/defs/garb.txt` is the item layer and
`0xa8` bytes zero on the disc's only `garb`, which round-trips.
`flares_conv::garbage` writes the object header (type 4, bounds, model,
predicted resources), the base's garbage role words and the attachments
from the definition; `Converter::effect` names it through an `obje`
part (version 56).

**Light fixtures.** Halo 1's 23 `lifi` tags each carry their light as
`ligh` attachments (25 attachments; V `halo1_reference_survey`), and
nine levels place 886 fixtures. Halo 2's scenario has its own light
placements (`scnr +0xe8`, `0x6c` each, palette `+0xf0`; V over 430
placements on 26 scenarios: the palette index and name first, the
position at `+8` and the facing at `+0x14` as other placements, then a
second point at `+0x4c`, two scales at `+0x58`/`+0x5c`, a cone angle at
`+0x60` and two distances at `+0x64`/`+0x68`, as the `ligh` cone's
fields). Halo 1 has no such placements: its fixture's light rides on
the fixture. So the converted fixture (a `mach`, as before) carries
Halo 1's lights as attachments at Halo 1's markers (version
56; E `0x000bed2a` builds `ligh` attachments), each with its
lens flare. The placement's own colour, intensity and cone angles
(Halo 1's light fixture placement) are not carried (D: what Halo 1
uses them for was not established).

Proof: `flares_disc.rs` `every_flare_debris_and_fixture_converts`
(ten campaign levels on 05a, the multiplayer maps on Coagulation and
Beaver Creek): 636 lens flares with 1,052 reflections (position and
radius bounds checked against Halo 1's bytes), 652 lights naming a
converted flare (every light whose Halo 1 light names one), 46 garbage
objects, 131 effect parts naming a converted `garb` through `obje`, 23
light fixtures carrying their 25 lights, 19 sky lights with a flare;
every tag reads back through its definition and pins nothing. Unit tests:
`h1convert::lens` (angles, distances, reflections, no bitmap),
`flares_conv` (garbage, a light's flare, a fixture's lights),
`h1sky` `converted_lights_name_the_flares_given`.

What only a run shows: the flares' look (no function blocks, Halo 2's
`+0x20` reflection word and root `+0x2c`/`+0x30` at zero: whether a
flare draws at all, and how bright), whether
garbage falls and settles as Halo 1's does (item physics), and the
fixtures' lights against Halo 1's.

## Weapons, equipment, projectiles and damage (`h1objects::arsenal`)

Every Halo 1 `weap`, `eqip` and `proj`, and the `jpt!` its projectiles
deal, is written from its definition with no retail template bytes
(`crates/h1objects/src/arsenal.rs`, `convert_arsenal.rs`). **The only
path since converter version 51**: the template path for these groups
was removed. `proj.txt` and `jpt_.txt` are new; all 47 retail projectiles
and 135 damage effects round-trip byte-identical (the proof above, now 18 groups). Building one
starts from the definition default, then:

- **Role words** (`arsenal::role`): the turret and biped rule over every
  tag of the group the base map holds, at conversion time. Each 4-byte
  unknown span where at least half hold one nonzero value takes it, and
  the same over all elements of the magazines, barrels and firing effects
  pooled.
- **Halo 1's values**: the named fields below, and by offset the words
  the template path already wrote (S, V2 in docs/halo1/objects.md):
  `WEAPON_WORDS` (heat; aim assist, E), `BARREL_WORDS` (rate of fire,
  magazine, rounds per shot, error, spread, first-person offset, heat per
  round), `PROJECTILE_WORDS` (radii, arming time, timer, gravity, damage
  ranges; speeds per tick times 30).
- **Material responses** (`proj +0x19c`, `arsenal::responses`): Halo
  1's 33 responses, one per Halo 1 material type, become the response
  most of them share, for `default_material`, then one element per Halo
  1 material whose response differs, for the Halo 2 global material of
  the same substance (`arsenal::MATERIALS`; the first Halo 1 material
  wins where two map to one name). The material index at `+0x10` is the
  name's position in the base's globals (`matg +0x150`). The response
  maps through `arsenal::response`; the effects are null (Halo 1's
  effects are not converted).
- **First-person graphs** (h1models): Halo 1's `first-person <action>`
  animations go under the one mode, class and type `any`, as every
  retail first-person graph has, with `jmad +0x54` the one pair
  (`any`, `any`).
- **Globals grenades** (`h1convert::globals::set_grenades`): each Halo 2
  grenade type of `matg +0x100` takes Halo 1's maximum and its converted
  equipment and projectile (Halo 1's `matg +0x128`), in the campaign
  and multiplayer converters both.
- **References**: converted Halo 1 tags for the model chain, the
  first-person model and animations (two elements, as retail's Chief and
  Arbiter), the projectiles and every damage slot of a projectile
  (impact, and detonation: Halo 1's attached detonation damage or the
  first `jpt!` its detonation effect names). The slots Halo 1 has nothing
  converted for take the **Halo 2 counterpart's reference** (the
  `TEMPLATES` row; references only): melee damage, effects and sounds (Halo 1's `effe` and
  most `snd!` are not converted), the barrels' firing effects. A
  projectile takes no `jpt!` from Halo 2. The HUD (`+0x2b0`) is Halo 1's
  `wphi` converted (`Converter::weapon_hud`, docs/halo1/hud.md); the
  counterpart's `nhdt` is the weapon's Halo 2 HUD in its HUD pair.

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
| `proj +0x19c` `material_responses` (`0x58`): `+0x2` `response`, `+0xc` `material_name`, `+0x10` `material`, `+0x14` `potential_response`, `+0x16` `potential_flags`, `+0x18` `chance`, `+0x1c` `angle_bounds`, `+0x24` `velocity_bounds`, `+0x34` `scale_effects_by`, `+0x38` `angular_noise`, `+0x3c` `velocity_noise`, `+0x48` `initial_friction`, `+0x4c` `maximum_distance`, `+0x50`, `+0x54` frictions | Halo 1's element (`0xa0`) in the same order without its padding; on two bases each of the 179 retail elements' `material` is its name's position in `matg +0x150` and its flags zero; Halo 1's most common response, mapped, is the retail default material's on all 8 Blood Gulch / Coagulation pairs (grenades bounce 4, needle attach 3, rocket and bullets impact 0), Halo 1's water overpenetration is retail's thin liquid 2, and the plasma grenade's attach chance (Halo 1 potential 4, flags 1, skip 0) is retail's (3, 1, chance 1) (`material_responses_pair_with_retail`) |
| `jmad` first-person modes | every retail first-person graph on Coagulation (34) has the one mode `any` and `+0x54` (`any`, `any`) (`first_person_graphs_take_retail_modes`) |
| `matg +0x100` grenades (`0x2c`: maximum `+0`, `eqip` `+0x1c`, `proj` `+0x24`); Halo 1's `matg +0x128` (`0x44`: `+0`, `+0x24`, `+0x34`) | element 0 the frag grenade, 1 the plasma grenade on three Halo 2 bases and all eleven Halo 1 maps, and each Halo 1 grenade's `eqip` grenade type its element (`grenade_blocks_pair_across_the_discs`) |

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

## Particles and contrails (`prt3`, `cont`)

`crates/tags/src/defs/prt3.txt`: all 2,021 distinct retail `prt3` (4,988
instances) round-trip byte-identical. The root's fields are named from
the engine's reader, the particle draw callback `0x00224550`
([effects.md](effects.md), "prt3 root"): the five properties are colour,
alpha, size, rotation and sprite frame; the flags, the sequence range,
the collision and death effects, the input mask, the quad and the UV
window. The nested particle systems are `effe`'s record (and use its
`struct property`, so the file follows `effe.txt`); the compiled shader
at `+0x8c` is `shad`'s postprocess layout but for two strides (V, the
schema: lod elements `0x8` and pass elements `0x4` where `shad`'s are
`0x6` and `0x2`). Two words the schema reads as raw in `prt3` are `raw`
here: an emitter's `+0x68` (the input pair of its seventh property) and an
implementation's `+0x4` (its stage-state and pixel-constant ranges).

**The compiled particle shader.** A particle's `postprocess` is what
the tag compiler makes of its `stem` and parameters; with no animated
parameter it follows one rule from the stem (V: 533 retail particles on
05a, Coagulation and Beaver Creek, the six with animated parameters
aside; `h1objects` `tests/particles_disc.rs`
`retail_particle_shaders_compile_from_their_stems`):

- one `bitmaps`, `colors` and `vectors` element per instance source the
  stem's bindings name, through each implementation's ranges;
- `lods`, `layers` and `passes` hold the stem's own layer, pass and
  implementation ranges (the lod's animated-layer mask zero), one empty
  `implementations` element per stem implementation;
- a bitmap's `lod_offset` is `log2(max(width, height))` of its first
  image times `ln(2)^2` (5 to 8 times `ln(2)^2` on 32 to 256 pixel
  bitmaps);
- `parameter_bitmaps` five entries (`0` then `-1`), `parameter_colors` two
  zero colours and `parameter_scalars` six zeros.

The colour and vector values are the parameters' compiled defaults; 502
of the 533 hold their stem's most common set. A lod's `+0x6` and a pass's
`+0x2` vary on retail and nothing reads them.
`h1objects::particles::particle_postprocess` writes this.

`crates/tags/src/defs/cont.txt`: all 34 distinct retail `cont`
round-trip (a `0xf0` root, the bitmap at `+0x30` and `+0x88`, `0x40`
points each with a `pphy` at `+0x10`). The spans are named by where they
sit in Halo 1's `cont` (`0x144`, points `0x68`), which Halo 2's compacts
(V, the same contrails of both games side by side): root `+0x4..+0x30`
is Halo 1's `+0x4..+0x30`, `+0x38` Halo 1's `+0x40`, `+0x3c..+0x60` zero
on every retail tag, `+0x60..+0x88` Halo 1's `+0xa8..+0xd0`, `+0x90..+0xe8`
Halo 1's `+0xe0..+0x138`; a point's `+0x0..+0x10` and `+0x18..+0x40` are
Halo 1's `+0x0..+0x10` and `+0x40..+0x68`. The fields' meanings are not
read from the engine yet.

## Which converted objects use the definitions

| Groups | Path | Why |
|---|---|---|
| `vehi` | definitions (only path, since version 49) | the vehicle template path was removed |
| `mach`, `ctrl`, Halo 1's `lifi` (as `mach`) | definitions (only path) | no template path was ever kept |
| `phmo` of a rebuilt vehicle or device, or a templated vehicle with none | definitions (Halo 1's collision as polyhedra; a stand-in sphere with no solid collision) | `vehicle_new` refuses a model with none; Havok sees a device only through one |
| `scen` | definitions (only path, since version 50) | the scenery template path was removed |
| `bipd` | definitions (default since version 48); template with `Options::rebuild_bipeds` off | references from the Halo 2 counterpart, size and physics from Halo 1 |
| `phmo` of a rebuilt biped | definitions (Halo 1's collision as polyhedra, a body per node) | the template path borrowed the template's ragdoll on other nodes |
| `weap`, `eqip`, `proj`, and the `jpt!` the projectiles deal | definitions (only path since version 51) | Halo 1's values on the role's words; references from the Halo 2 counterpart where Halo 1 has nothing converted; not run in the game |
| `effe` (vehicles' attachments and creation effects, since version 50) | definitions (only path) | no template path; sound parts, and lights and decals since 51 |
| `ligh`, `deca` (effect parts, attachments; since version 51) | definitions (only path) | no template path |
| `lens` (lights', sky lights'), `garb` (effect parts) | definitions (only path) | no template path |

## Open

- **Unknown spans** are largest in the unit layer (`bipd`, `vehi`), the
  `weap` tuning, `mode`'s nested blocks, and the `bitm` and `snd!` roots.
  Each needs the engine's reader found to be named.
- **More groups:** `sky` (the second tier asked for; `effe`, `ligh`,
  `deca`, `udlg`, `proj` and `jpt!` done; open: damage parts in effects,
  particle systems and Halo 1's particles as `prt3`, contrails; `lens`
  and `garb` done, their open fields in "Lens flares, debris and light
  fixtures"), and the other object groups (`bloc`, `crea`, `ssce`),
  which `struct obje` already covers.
- **Physics shapes from Halo 1**: built from the collision BSP's solid
  cells since version 43. Open: whether a device's bodies need retail's
  `block_30` node pairs, the shape header's material index table, and
  the rigid body words at `+0x18`/`+0x1c`.
- **Rebuilt bipeds**: Halo 1's body and shield vitality in the `hlmt` damage block (the
  counterpart's now), the ground-physics words after `bipd +0x29c` (the
  role's now), and what `BIPED_WORDS` mean.
- **More converter paths**: the other objects (templated vehicles), shaders (`shad` with its postprocess mirrors kept consistent),
  and the sky, each on its definition with its own comparison.
