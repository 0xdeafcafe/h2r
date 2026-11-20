# Halo 2 object tags (`obje` family, `hlmt`, `itmc`, `vehc`)

Implementation: `crates/tags/src/object/` (self-contained, table-driven
decoder; not in `schema.rs`). Command: `h2 object` in
`crates/cli/src/object.rs`. Evidence labels:

- **E**: engine code reads the field this way. The field note in
  `layouts.rs` gives the XBE address.
- **V**: the structural claim (offset, type, stride, reference class) holds on
  every instance on the retail disc.
- **D**: a name or interpretation chosen here.

No community tag definition was used for this family. Offsets, strides and
classes come from the disc and the XBE. Names without E evidence are
`unknown_<kind>_<offset>`, plus four D names argued below. No map bytes, tag
names or strings are committed. The tests are synthetic.

```sh
cargo run -p cli --release -- object <map> 'example\vehicles\buggy\buggy' --disc disc/halo2.iso
cargo run -p cli --release -- object <map> 'example\vehicles\buggy\buggy' hlmt --disc disc/halo2.iso
cargo run -p cli --release -- object --all all --disc disc/halo2.iso     # every map; exit 1 on any failure
cargo run -p cli --release -- object --all <map> --disc disc/halo2.iso
```

With no group argument, a name that matches an object and its same-named
`hlmt` selects the object. `h2 object` writes nothing.

## Type hierarchy (E, V)

The engine's object type table at `0x00468630` holds 13 descriptor pointers.
The `obje +0` value of a tag indexes this table: `object_new` (`0x000b7b40`)
does `table[*(i16*)def]`. Each descriptor has a name pointer at `+0`, a FourCC at
`+4`, and the runtime body size at `+8`. It also holds the scenario offsets of
the type's placement block (`+0xa`) and palette block (`+0xc`), the placement
record size (`+0xe`), callbacks, and a null-terminated ancestry list from `+0x84`.

| Index | Group | Body | Placement / palette / stride in `scnr` | Ancestry |
|---:|---|---:|---|---|
| 0 | `bipd` | `0x464` | `+0x60` / `+0x68` / `0x54` | obje, unit, bipd |
| 1 | `vehi` | `0x428` | `+0x70` / `+0x78` / `0x54` | obje, unit, vehi |
| 2 | `weap` | `0x25c` | `+0x90` / `+0x98` / `0x54` | obje, item, weap |
| 3 | `eqip` | `0x184` | `+0x80` / `+0x88` / `0x38` | obje, item, eqip |
| 4 | `garb` | `0x170` | none | obje, item, garb |
| 5 | `proj` | `0x1ac` | none | obje, proj |
| 6 | `scen` | `0x13c` | `+0x50` / `+0x58` / `0x5c` | obje, scen |
| 7 | `mach` | `0x1e0` | `+0xa8` / `+0xb0` / `0x48` | obje, devi, mach |
| 8 | `ctrl` | `0x1d4` | `+0xb8` / `+0xc0` / `0x44` | obje, devi, ctrl |
| 9 | `lifi` | `0x1e4` | `+0xc8` / `+0xd0` / `0x54` | obje, devi, lifi |
| 10 | `ssce` | `0x148` | `+0xd8` / `+0xe0` / `0x50` | obje, ssce |
| 11 | `bloc` | `0x130` | `+0x328` / `+0x330` / `0x4c` | obje, bloc |
| 12 | `crea` | `0x208` | `+0x368` / `+0x370` / `0x34` | obje, crea |

The abstract descriptors are object `0x004678e8`, unit `0x004679b0`, item
`0x00467c08` and device `0x00468248`, with body sizes `0x12c`, `0x348`, `0x16c`
and `0x1cc`. The body sizes are runtime object sizes, not tag sizes. The
placement columns tell a level viewer where each type's placements sit in
`scnr`. Placement records are not decoded here. The disc has no `lifi` tags, so
`lifi` has no reviewed layout.

The decoder's layer chain (for example `bipd` -> `unit` -> `obje`) is checked
against each map's group table, giving 9,010 of 9,010 matches. Every tag's
`obje +0` equals its group's table index; the decoder rejects a mismatch.

## Root sizes and layer boundaries (V, E)

**Root size.** On every disc instance, the first child block owned by a tag
starts exactly at the end of the root. The validator checks this on every tag:
8,841 tags have their own blocks, and 169 have none. Measured root sizes:

| `bipd` | `vehi` | `weap` | `eqip` | `garb` | `proj` | `scen` | `mach` | `ctrl` | `ssce` | `bloc` | `crea` | `hlmt` | `itmc` | `vehc` |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `0x314` | `0x300` | `0x31c` | `0x13c` | `0x1d4` | `0x1a4` | `0xc4` | `0x134` | `0x140` | `0xcc` | `0xc0` | `0x180` | `0xfc` | `0xc` | `0xc` |

**Layer boundaries:**

- **`obje` is `0xbc` bytes.** The shared fields below hold in all 12 groups:
  the decoder checks them on every tag. Beyond `0xbc` the groups differ. `object_new` reads a device flags word at
  `def+0xbc` for device types. `bloc` adds 4 bytes, which are zero on every disc
  instance.
- **`unit` is `0xbc..0x1ec`.** Below `0x1ec`, no offset holds conflicting
  classes in `bipd` and `vehi` (at most one of them is always zero there). At `0x1ec`, bipeds hold a float where vehicles hold a
  flags word: vehicle code tests bits there (`0x000efb8c`) and treats `0x1f0` as
  an enum compared with 5 and 6 (`0x000ef545`). The `biped_new` helper tests a
  bit at `0x1f0` (`0x000de5ca`).
- **`item` is `0xbc..0x12c`.** The three item groups agree up to `0x12c`. After
  that, equipment holds its own words plus a sound reference (`0x134`), while
  weapons and garbage differ.
- **`devi` is `0xbc..0x11c`.** Machine code and control code both start reading
  their own words at `0x11c`/`0x11e` (`0x001153ee`, `0x0011baa3`). From `0x124`
  on, a control holds a string ID where a machine holds a float.

## The shared `obje` header

| Offset | Field | Kind | Ev. | Evidence |
|---|---|---|---|---|
| `0x00` | `object_type` | u16 | E | indexes the type table (`0x000b7b40`) |
| `0x02` | `flags` | u16 | E | bit 0 tested by `object_new` |
| `0x04` | `bounding_radius` | f32 | E | multiplied by object scale (`0x000be075`) |
| `0x08` | `bounding_offset` | vec3 | E | transformed by the root node into the bounding-sphere centre (`0x000bdfa9`) |
| `0x30` | `default_variant` | string ID | E | `0x000ba571`: used when no variant is requested; passed to the variant lookup `0x0016d180` |
| `0x34` | `model` | ref `hlmt` | E | `object_new` resolves `+0x38` and reads hlmt fields through it |
| `0x3c` | `unknown_bloc_ref_3c` | ref `bloc` | V | |
| `0x44` | `unknown_shad_ref_44` | ref `shad` | V | |
| `0x4c` | `creation_effect` | ref `effe` | E | `object_new` passes `+0x50` to `0x00176780` once the object is created |
| `0x54` | `unknown_foot_ref_54` | ref `foot` | V | |
| `0x5c` | `unknown_block_5c` | block | V | at most one element; stride not established |
| `0x64` | `functions` | block, `0x20` | E | the loop at `0x000bac20` steps `0x20` and matches element `+8` against a requested ID |
| `0x94` | `attachments` | block, `0x18` | E | `0x000b8c82` indexes ×`0x18`; `0x000bed2a` switches on element `+0` class |
| `0x9c` | `widgets` | block, 8 | V | one reference per element; name D |
| `0xa4` | `unknown_block_a4` | block | V | always empty on the disc |
| `0xac` | `change_colors` | block, `0x10` | V | `object_new` reserves `0x18` bytes per element; name D (two RGB colours) |
| `0xb4` | `predicted_resources` | block, 8 | V | element `+4` is the full salted datum of an existing tag; name D |

**Element layouts:**

- **Attachments (E):** element `+0` is a reference. Its class picks the
  constructor: `ligh`, `lsnd`, `effe` and `cont` are handled; `lens`, `MGS2` and
  `tdtl` are skipped. `+8` is a marker string ID (accessor `0x000b8c40`, load at `0x000b8c8b`). `+0xc`
  is a u16 passed to the constructor. `+0x10` and `+0x14` are string IDs (V).
- **Functions:** `+4`, `+8` and `+0xc` are string IDs. `+8` is the output the
  engine matches. `+0x14` is a nonempty block. Its count looks like a byte
  length, but the element size is not reviewed.
- **Change colours:** two blocks whose element layouts are not reviewed.

## Subtype layers

The typed fields are the reviewed references, string IDs and blocks, plus the
E fields below. All other bytes are kept as opaque spans, so the records round
trip. Blocks marked "opaque" in `layouts.rs` record only count and address,
because their stride is either unproven or only an upper bound.

| Layer | E fields | V references and blocks |
|---|---|---|
| `unit` | none | camera field of view `0xcc` (f32 radians; tag-definitions.md); effe `0xc4`; char `0x134`; six `jpt!` at `0x164..0x18c`; opaque blocks `0xe8`, `0x198`, `0x1a0`, `0x1a8`, `0x1b8`, `0x1c0`, `0x1c8` (stride `0xb0` from layout; unit code reads `+0x1cc` 21 times) |
| `bipd` | `0x1f0` flags | effe `0x25c`; string IDs `0x278`, `0x27c`; blocks `0x284` (`0x80`), `0x28c` (`0x50`), `0x294` (`0x80`), `0x2f8` (4); char `0x300`, `0x308` |
| `vehi` | `0x1ec` flags, `0x1f0` vehicle type | blocks `0x268` (`0x44`), `0x2e8`/`0x2f0` (`0x4c`), `0x2f8` (one per vehicle); snd! `0x284`/`0x28c`; foot `0x294`; effe `0x29c`/`0x2a4` |
| `item` | none | ten string IDs `0xcc..0xf0`; foot `0xf4`; snd! `0xfc`; block `0x104` (8); jpt! `0x10c`; effe `0x11c`/`0x124` |
| `weap` | `0x208` `cone_angle`, `0x20c` `cone_range`, `0x210` `pull_angle`, `0x214` `pull_range`, `0x218` `bend_angle` (all f32; read by `0x001a50a0`, named for those reads; [aim-assist.md](../game/aim-assist.md)) | 28 references (effe, jpt!, snd!, nhdt, vehi, weap); blocks `0x2a8` (`0x10`), `0x2b8` (8), `0x2c0`, `0x2c8` (`0x40`), `0x2d0` (`0xec`) |
| `eqip` | none | u16 `0x12c`/`0x12e`; snd! `0x134` |
| `devi` | none | snd! `0xdc`/`0xe4`; effe or snd! `0xec`/`0xf4`/`0x110`; effe `0xfc`/`0x104` |
| `mach` | u16 `0x11c` (`0x001153ee`), `0x11e` (`0x001151c5`), f32 `0x120` (`0x0011596e`) | none |
| `ctrl` | u16 `0x11c` (`0x0011baa3`), `0x11e` (`0x0011b9fd`), f32 `0x120` (`0x0011bace`) | string ID `0x124`; three references `0x128`/`0x130`/`0x138`, null on the disc |
| `proj` | none | 14 references (effe, jpt!, snd!) `0xe8..0x15c`; block `0x19c` (`0x58`) |
| `crea` | none | blocks `0xf4` (`0x80`), `0xfc` (`0x50`), `0x104` (`0x80`), the same strides as `bipd +0x284/+0x28c/+0x294`; jpt! `0x168`, `0x170` |
| `scen`, `bloc`, `ssce`, `garb` | none | none; their extensions are opaque |

**Engine hints, not decoded:**

- The resource preparation at `0x0011c840` passes `bipd +0x264` and
  `crea +0xd4` to `0x001c2390`, so both offsets hold the same embedded structure.
  `0x001c2390` writes the Havok headers of that structure's shapes: its
  `+0x20` block (`bipd +0x284`, `0x80` each: a sphere at `+0x20`, a
  transform at `+0x30` whose child pointer `+0x3c` is the sphere), `+0x28`
  (`+0x28c`, `0x50` each: a pill at `+0x20`) and `+0x30` (`+0x294`, as
  `+0x20`). The sizes they are built from are in tag-definitions.md
  ("Bipeds from definitions").
- The same code passes `vehi +0x2ac` to `0x00205550`.

## `hlmt` object models

| Offset | Field | Kind | Ev. | Evidence |
|---|---|---|---|---|
| `0x00` | `render_model` | ref `mode` | V | |
| `0x08` | `collision_model` | ref `coll` | E | `object_new` sets an object flag when `+0xc` is not null |
| `0x10` | `animation` | ref `jmad` | E | `object_new` tests `+0x14` before its animation and physics setup |
| `0x18` | `unknown_phys_ref_18` | ref `phys` | V | |
| `0x20` | `physics_model` | ref `phmo` | V | |
| `0x50` | `variants` | block, `0x38` | E | `0x0016d1bb` steps `0x38` |
| `0x58` | `unknown_block_58` | block | V | stride `0x14` from layout |
| `0x60` | `unknown_block_60` | block | V | at most one element; `object_new` reads its `+0xbc` as a count of 8-byte entries |
| `0x68` | `unknown_block_68` | block | V | stride `0x1c` from layout |
| `0x70` | `regions` | block, `0x10` | E | `0x0016d205` steps `0x10`; `object_new` reserves 10 bytes per region |
| `0x78` | `nodes` | block, `0x5c` | V | `object_new` reserves `0x34` bytes per node (D: node matrices) |
| `0x84` | `unknown_block_84` | block | V | at most one element |
| `0x8c` | `unknown_udlg_ref_8c` | ref `udlg` | V | |
| `0x94`, `0xf0` | `unknown_shad_ref_*` | ref `shad` | V | |
| `0xf8` | `unknown_sid_f8` | string ID | V | |

**Variant (`0x38`):**

- `+0` name (E: `0x0016d1b6`).
- `+4` a 16-byte region map (E: `0x0016d764` reads the signed byte at
  `+4 + region`). It maps an hlmt region index to an index into the variant's
  own regions, with -1 for none.
- `+0x14` the variant's regions: stride `0x14` (E: `0x0016d78b`).
- `+0x1c` objects: stride `0x10`. Each holds two string IDs and an object
  reference.
- `+0x2c` a string ID and `+0x30` a `udlg` reference.

**Variant region (`0x14`):**

- `+0` region name.
- `+4` u8 hlmt region index.
- `+5` u8, unknown.
- `+8` permutations: stride `0x20`. Each holds a name string ID, a u32, a
  probability f32 (D) and an opaque block at `+0xc`.

**hlmt region (`0x10`):**

- `+0` name (E: `0x0016d200`).
- `+4` four bytes.
- `+8` permutations: stride 8, each a name plus four bytes.

The validator checks these claims on every `hlmt` (V):

- Each variant region names the hlmt region at its index.
- The variant's region map sends that region back to it.
- Every map entry is -1 or in range.

## Collections (`itmc`, `vehc`) (E)

Both collection classes have the same layout: a `0xc`-byte root with a block
at `+0`. The selector `0x0019f1a0` steps the block by `0x10`. For each entry it
subtracts the `+0` weight from a random roll, and the selected entry returns the
`+8` datum (part of the reference at `+4`) and the `+0xc` string ID. In `vehc`
that string ID is a variant name; in `itmc` it is zero on the disc.

## Resolver

`Context::resolve_object(id)` decodes the object and follows `model` to its
`hlmt`. From the hlmt it links the render, collision, physics and animation tags
by reference, without decoding them. Then it picks the variant the way the
engine does in `0x0016d180`:

- If `default_variant` is zero, the model's variant 0 is used, provided the
  model has any variants.
- Otherwise the first variant whose name matches the full string-ID value is
  used.
- If no variant matches, there is no variant. This matches the engine, which
  returns -1 and has no fallback.

For the chosen variant it reports each variant region: the region name, the
hlmt region index, and the permutations with their probabilities and their
index in the hlmt region's permutation list. The runtime choice among several
permutations goes through `0x0016dad0` and is not reproduced. A viewer gets the
whole weighted list.

## Aliasing and budgets

Cache blocks often point into another tag's data. Of 54,974 nonempty blocks
on the disc, 13,099 do. Block spans are therefore checked against the whole
ordinary metadata region and four-byte alignment, never against the owning
tag. Aliased blocks are flagged, not rejected.

Decoding one tag is capped by a `Budget`:

- 65,536 decoded elements in total, counted with repeats.
- 4,096 elements per block.
- A nesting depth of 6.

The largest real tags stay far below these limits: at most 72 nodes and about
100 predicted resources. Reference checks compare the full salted datum and
accept the referenced tag's class or an ancestor of it. A null reference may
carry any class, including classes absent from the cache (`antr`, `shdr` occur
on the disc). A live reference must use one of the field's classes.

## Validation on the retail disc (30 maps)

`h2 object --all all` decodes every object-family tag: 9,010 decoded, 0
failures.

| Group | Tags | Unique names | Group | Tags | Unique names |
|---|---:|---:|---|---:|---:|
| `bipd` | 242 | 43 | `ctrl` | 25 | 14 |
| `vehi` | 412 | 37 | `ssce` | 148 | 98 |
| `weap` | 822 | 63 | `bloc` | 1,347 | 262 |
| `eqip` | 254 | 13 | `crea` | 18 | 10 |
| `garb` | 30 | 1 | `hlmt` | 3,743 | 889 |
| `proj` | 725 | 43 | `itmc` | 153 | 20 |
| `scen` | 866 | 507 | `vehc` | 22 | 6 |
| `mach` | 203 | 135 | | | |

**Default variants of the 5,092 object tags.** 1,033 have no model. Of the
4,059 with one:

- 3,206 have a model with no variants.
- 442 have an empty default variant, so variant 0 is used.
- 406 name a variant that the model has.
- 5 name a variant the model lacks; the engine selects none for these.

**Model links.** Of the 4,059 object tags with a model, 4,046 link a `mode`, 3,079 a
`coll`, 2,433 a `phmo` and 1,457 a `jmad`.

**Variant permutation names.** Of 8,743 permutations across all variants,
7,030 name a permutation of their hlmt region. 1,685 have an empty (zero)
name, and 28 use a name the region lacks.

## Open items

- **Weapon report loudness.** `weap +0x270` (f32): Halo 1 multiplies it
  by three and adds the hearing unit's radius (`0x0014e379`) for the
  reach of a shot's alert report to the AI. The converter carries it in
  the perception table's weapon rows (docs/halo1/ai-control.md, "alert
  reports and forced combat levels").
- **Gameplay fields.** Most unit, biped, vehicle, weapon, projectile and device
  scalars are opaque (the weapon's aim-assist floats are the exception). Several blocks are kept opaque because their stride is only
  an upper bound (one element per tag) or their contents are unreviewed:
  - unit seats-like block `0x1c8`
  - weapon `0x2c0`
  - vehicle `0x2f8`
  - hlmt `0x58`, `0x60`, `0x68`, `0x84`
- **Empty permutation names.** What a zero-name variant permutation means is not
  known. The probable reading is "keep the region's default", but it is not
  established. 28 named permutations are also missing from their hlmt region.
- **Unknown model bytes.** The variant region `+5` byte, the four bytes at
  hlmt region `+4`, and region permutation `+4` are unknown.
- **No engine evidence yet.** The names `widgets`, `change_colors` and
  `predicted_resources`, and the hlmt `render_model` and `physics_model` fields,
  have no engine evidence yet. Their classes and strides are V.
- **`lifi`.** It has no disc instances, so its layout is unreviewed.
- **Placements.** Scenario placement records are located (table above) but not
  decoded.

## Method (reproducible, nothing committed)

Each step below is a measurement over every disc instance, or a reading of
XBE code with `h2 disasm` and Ghidra:

- **Field classes.** For each group and offset, the scan classifies the value as
  one of: zero, valid reference, block descriptor, string ID with matching
  length, plausible float, or other. A field is accepted only when every
  instance agrees.
- **Strides.** A block's stride is the minimum, over instances, of the distance
  to the next block in the tag divided by the element count. It counts as
  established when many multi-element instances hit that minimum exactly.
- **Engine reads.** A linear sweep tracks registers loaded through the
  tag-entry table (`0x004e3b44` entry `+8`) and reports the definition offsets
  read by each code range. It is paired with the type descriptors' callbacks.

The scratch tools stayed in session temporary storage.
