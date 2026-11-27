# Typed tag definitions (`tags::defs`)

Status: Built. The disc checks under “Proof” cover the groups in the results table.
Code: `crates/formats/tags/src/defs.rs` and `crates/formats/tags/src/defs/*.txt`.
Evidence labels are those of [map-format.md](map-format.md): **E** engine
code, **V** checked on the owner's disc, **D** a naming or interpretation
choice made here.

The Halo 1 converter writes scenery, vehicles, bipeds, devices, physics
models, effects, weapons and particles from these definitions. That
behaviour, with its per-section evidence, is in
[../halo1/rebuild.md](../halo1/rebuild.md).

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

- Names and descriptions are written here, in our words. A name carries
  no source label. A field's type and extent must agree with the local
  layout. The names' reference, permission and the disagreements kept
  are in [the field-name note](../editor/notes/tag-field-names.md).
- **A name needs a known role.** A field whose type is known but not its
  role (a reference, a string ID, a block found by the schema) gets a name
  ending in `_<offset in hex>`: `string_1c`, `phys_ref_18`, `block_5c`.
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

`crates/formats/tags/src/defs/*.txt`, parsed in the order of `defs::FILES` (a
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
An optional `doc="..."` before the evidence gives the editor a single-line
description. Use `\"` for a quotation mark and `\\` for a backslash.

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

## Editor attributes

Attributes the field panel reads. Each has a closed word list or a text
check, and the parser refuses a word outside it with the line number
(`defs/attrs.rs`). A unit or a timing written without evidence fails
review, as a name does: the evidence goes in the line's meaning text.

| Attribute | On | Value |
|---|---|---|
| `label="..."` | a field line | The label when the name derived from the field's name is wrong. |
| `unit=<word>` | an integer or float field | One of `seconds`, `ticks`, `world_units`, `world_units_per_second`, `degrees`, `degrees_per_second`, `fraction`, `percent`, `pixels`, `count`, `index`. |
| `axes=a\|b\|...` | an integer or float array | One word per component, as many as the array has. |
| `title="..."` | a `group` or `struct` line | The kind in words (`Weapon`). |
| `title=<field>` | a `block` line | The element's field whose value titles each element. It names a field with a value: not a block, data reference, struct, `unknown` or `pad`. |
| `view=<word>` | a field line | How a rich editor shows the field: `rgb`, `hdr_rgb` (`f32[3]`), `argb` (`f32[4]`), `rgb8`, `argb8`, `xrgb8` (a `u32`) or `function` (a `data` reference). A word on another type, or any other word, fails the parse ([rich-editors.md](../editor/rich-editors.md)). Needs evidence of the stored form in the meaning text. |
| `applies=frame\|use\|creation` | a field, `block`, `struct`, `group` line | When a live write shows. Inherited, below. |

```text
struct shared 0x8 title="Shared kind" applies=use
  0x0 f32 rate unit=world_units_per_second label="Rate (fast)"
group test 0x1c title="Test thing" applies=frame
  0x0 struct inner shared
  0x8 f32[3] offset axes=x|y|z unit=world_units
  0x14 block parts 0x8 align 4 title=name applies=use
    0x0 string name
    0x4 f32 weight applies=creation
```

A `label` and a `title` are single-line text in our own words: not empty,
no space at either end, no double space, no control character. They use
the same `\"` and `\\` escapes as `doc=`. A group or struct line takes
only `title=` and `applies=`. `label` is refused on `unknown`, `pad` and
embedded `struct` lines; `unit` and `axes` on any other type; `title` on
a field line that is not a `block`.

`degrees` and `degrees_per_second` mean a value stored in radians and shown
in degrees. A field with any `unit` is shown by it: the registry takes
an angle from `degrees` and `degrees_per_second` only, not from a name
that contains `angle`; a field with no `unit` keeps the name match
(`registry::hint`).

**`applies` is inherited.** `registry::Field::applies` is the field's own
`applies=`, else the nearest one declared above it, nearest first:

1. the `struct` or `group` line of the element the field is in;
2. the `block` or `struct` line that embeds that element;
3. the lines that enclose that line, by the same rule.

A field's own value always wins. A field with no `applies` anywhere above
it has `None`, which the editor reads as "not known". The evidence for an
`applies` on a `group`, `struct` or `block` line goes in that line's
meaning text, naming the reader that reads every field below it.

In the registry (`tags::registry`), `Field` carries `label`, `unit`,
`axes` and `view`; `Layout::title` holds a group's or a block's `title`; and
`Layout::section_titles` maps the path of each embedded struct
(`Field::section`) to its `title="..."`.

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
- **Raw bytes into typed fields.** `Record::set` with `Val::Bytes` also
  writes over typed fields: from the field the path names, the bytes fill
  as many consecutive plain fields (integers, floats, enums, flags,
  unknown spans) as they cover, and each field reads them back as its own
  type, bit for bit. The path's last part is a field name, a name and a
  byte offset into it (`pitch_range+0x4`), or a byte offset in the
  element (`@0x38`). Bytes that cover a reference, string or block, or
  run past the element, are an error. `Record::bytes(path, len)` reads
  the same span back. The Halo 1 converter writes its raw words this way,
  so naming a field it writes does not change its output. Records from
  `decode`, `from_built` and `default_record` carry the field offsets for
  this; a `Record::new` has none.
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

Unit tests (`cargo test -p tags defs`, and `registry` for inheritance
and the merged fields) cover:

- decode/encode on an invented tag;
- a fixup inside a plain field refused;
- defaults, shared arrays, diff paths and builder trees;
- gaps refused and the text form round-tripping, header attributes
  included (`title`, `applies` on a `group` or `struct` line);
- each editor attribute parsed, printed back byte for byte, and refused
  with a wrong word or on the wrong type of field;
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
  - The other object-family names (the `weap` heat, aim-assist, magazine
    and barrel scalars, the `vehi` seats, gears and physics points, the
    `bipd` movement and shapes, `proj`, `jpt!`) match the layouts the
    [field-name note](../editor/notes/tag-field-names.md) records. The
    `weap` aim-assist offsets are E (`0x1a50a0`);
    [aim-assist.md](../game/aim-assist.md) describes them by those reads.
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
- **`snd!`**: the sample rate, encoding, compression (E `0x0021e421`),
  pitch ranges and the extra-info index (E the engine requests the
  sound's extra-info resource for lip sync, V the index names an `ugh!`
  extra info or is `ffff`; [sounds.md](../halo1/sounds.md)). Flags, class
  and playback index stay unknown (nothing of ours supports their
  meaning).
- **`lsnd`**: the Halo 2 layout the Halo 1 sound converter writes
  (`h1sounds::looping`), D.

### Editor-name groups

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
  [halo1/sounds.md](../halo1/sounds.md) (E). The extra-info names - the
  permutation's descriptor index, the block, its resource, its locators
  and the owner datum - come from the lip-sync read there ("Dialogue
  mouth curves", E and V). The fields that doc marks S
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
  - the locale descriptors ([localisation.md](localization.md)).

  Some level-select text slots and map ids stay unnamed, because the
  schema has a raw pointer or a raw size at those offsets.

## Open

- **Unknown spans** are largest in the unit layer (`bipd`, `vehi`), the
  `weap` tuning, `mode`'s nested blocks, and the `bitm` and `snd!` roots.
  Each needs the engine's reader found to be named.
- **More groups:** `sky` (the second tier asked for; `effe`, `ligh`,
  `deca`, `udlg`, `proj` and `jpt!` done; open: damage parts in effects,
  particle systems and Halo 1's particles as `prt3`, contrails; `lens`
  and `garb` done, their open fields in
  [rebuild.md](../halo1/rebuild.md), "Lens flares, debris and light
  fixtures"), and the other object groups (`bloc`, `crea`, `ssce`),
  which `struct obje` already covers.
- **Physics shapes from Halo 1**: built from the collision BSP's solid
  cells ([rebuild.md](../halo1/rebuild.md), "Physics models from Halo 1"). Open: whether a device's bodies need retail's
  `block_30` node pairs, the shape header's material index table, and
  the rigid body words at `+0x18`/`+0x1c`.
- **Rebuilt bipeds**: Halo 1's body and shield vitality in the `hlmt` damage block (the
  counterpart's now), the ground-physics words after `bipd +0x29c` (the
  role's now), and what `BIPED_WORDS` mean.
- **More converter paths**: the other objects (templated vehicles), shaders (`shad` with its postprocess mirrors kept consistent),
  and the sky, each on its definition with its own comparison.
