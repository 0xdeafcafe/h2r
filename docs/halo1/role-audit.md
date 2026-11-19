# Role audit: Halo 2 values in converted Halo 1 tags

Status (2026-11-19, converter v86): first run on Blood Gulch (on
Coagulation; v85, before the fixes below): 526 tags, 2,341 role values
noted, 3,169 leaks over 426 (group, field, kind) rows; 24 rows were
unclassified and are now in the rule lists. Since v86 the weapons'
overheated effect and melee damage, the projectiles' detonation effects
and flyby sound, and the items' and projectiles' material effects are
Halo 1's (below, "Fixed"). The v86 run, and every other multiplayer map
(`H1_MAPS=all`), are not run yet.

## Why

The converter rebuilds each tag from its Halo 2 definition
(`crates/h1objects`: `rebuild.rs`, `arsenal.rs`, `particles.rs`, the
garbage builder) and writes Halo 1's values into the fields it knows.
The fields it doesn't know are filled from a **role**: the base map's
retail Halo 2 tags of the same kind, as majority words per unknown span
(`rebuild::RoleWords`, `arsenal::Role`, `rebuild::VehicleRole`, ...), a
role's named values (light brightness, decal layer), retail values kept
in the source (`rebuild::TURRET_ROLE`, `TURRET_SEAT_ROLE`), whole blocks
copied from a Halo 2 counterpart (`rebuild::counterpart_fields`), and
references to the counterpart's tags (`arsenal::halo2_refs`).

Some of those fields have no Halo 1 equivalent, which is fine. Others do,
and then Halo 2 content leaks into a Halo 1 map with nothing saying so.
Found by playing before this audit: the weapons' firing sounds were the
SMG's, the Ghost's driving words were zeroed by a bad majority, the
Warthog's driver seat bit, the HUD widget thresholds. The audit lists
every such field, so each one is either allowed with a reason or a bug
with an owner.

## How it finds them

- **Provenance** (`crates/h1objects/src/provenance.rs`). Every builder
  notes each value it takes from a role, by its path in the tag's record
  (`barrels[0].unknown_4`), as it writes it. The converter collects the
  notes made while one tag's root is built (`provenance::capture`, which
  nests, so a tag built while another is being built keeps its own) and
  keeps them per tag in `convert::Tree::role_fields`. Nothing is noted
  outside a capture, and the notes change no output.
- **Value leaks.** The audit reads each built tag back through its
  definition (`tags::defs::from_built`) and reports a noted field only
  when it still holds the role's value: a Halo 1 value written later to
  the same field is not a leak. Zero, null and empty values are skipped
  (they are the definition's default too).
- **Copied blocks.** A block or struct taken whole from the counterpart
  (the biped's `unit.obje.functions`, the biped model's damage info) is
  reported as `copied` while it is still the counterpart's.
- **Reference leaks.** Every reference in a built tag to a tag of the base
  map (not one the conversion built, not a sound placeholder) is a retail
  Halo 2 tag in a slot. Found in the built tags themselves, so it needs no
  notes; groups with no definition are walked by fixup (paths are
  offsets, `+0x34`).

Each leak is keyed by (tag group, path with element indices as `[]`,
what: `value`, `copied` or the referenced tag's group) and matched against
two rule lists in `crates/h1objects/tests/role_audit_disc.rs`:

- `ALLOWED`: **(a)** Halo 1 has no equivalent. One line of reason each.
- `KNOWN`: **(b)** Halo 1 has an equivalent the converter does not
  translate yet, with its owner or what the fix needs; and
  **unidentified**: a Halo 2 unknown span, where whether Halo 1 has the
  equivalent cannot be told until the span is named. Unidentified spans
  list their suspects and count as bugs until shown otherwise.

A leak in neither list fails the test, and the failure prints it as a
row to paste into one of them: a new (b) leak cannot land silently. Rules
no leak meets any more (a lane fixed it) are printed as stale.

Run it (the coordinator; it converts in memory and writes nothing unless
asked):

```sh
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
  cargo test --release -p h1objects --test role_audit_disc -- --ignored --nocapture
H1_MAPS=all ...                       # every multiplayer map on its base
ROLE_AUDIT_OUT=/tmp/role-audit.md ... # the tables below, regenerated
```

## Classification

`ALLOWED` and `KNOWN` in `tests/role_audit_disc.rs` are the source;
this is them by group, with the evidence. "V" names come from the
retail tags the counterparts hold in the slot (Coagulation).

### Allowed (a): Halo 1 has no equivalent

| Group | Field | What | Why |
|---|---|---|---|
| any | words that are half of an unnamed reference holding none (a group word such as `jpt!`, `foot`, `effe`, or `-1`) | `none-word` | no value: weapon `+0x148`, `+0x174` .. `+0x198`, `+0x244` .. `+0x250`, magazines `+0x3c` .. `+0x50`, barrels `+0x94`, `+0x98`, item `+0xf4` .. `+0x120` |
| `hlmt` | `block_60` (damage info) and its shaders and effects | copied, references | Halo 2's damage and shield model wins ([campaign.md](campaign.md) decision 2) |
| `hlmt` | `shad_ref_94` (active camouflage) | `shad` | Halo 1 draws camouflage from its globals |
| `prt3` | `input_mask`, `vertex_shader`, `quad`, `uv_window`; the unit `color`, `alpha`, `size`, `rotation` constants; `shader_template`, `postprocess[].template` | values, copied, `stem` | Halo 2's particle shader inputs; Halo 1's values go on the emitter, which multiplies the constants |
| `shad` | templates, default and rasterizer bitmaps | `stem`, `bitm` | Halo 2's shader system; Halo 1's bitmaps fill the slots it has |
| `nhdt` | widget shaders; the Halo 2 HUD of the pair's bitmaps | `shad`, `bitm` | Halo 1's HUD has no shaders; the Halo 2 HUD is Halo 2's by design ([hud.md](hud.md)) |
| `bipd` | `unit.ref_16c`, `unit.ref_174` | `jpt!` | boarding melee and its response (V: `masterchief_boarding_melee`): Halo 1 has no boarding |
| `proj` | `unknown_178`, `unknown_e4` | values | Halo 1's own values (`PROJECTILE_WORDS`, the detonation noise), equal to the role's |
| `deca` | `unknown_18` (scale), `max_overlapping` | values | no Halo 1 decal field |

### Owned by other lanes (not touched here)

| Group | Field | What | Lane |
|---|---|---|---|
| `weap` | `barrels[].block_e4[]` | firing effects, dry-fire sounds, trigger damage | h1-weap-snd |
| `weap` | `ref_140` (ready effect, V `needler\ready`), `ref_264` (pickup sound), `ref_26c`, `ref_274` (zoom in, out) | `effe`, `snd!` | h1-weap-snd |
| `eqip` | `snd_ref_134` (pickup sound) | `snd!` | h1-weap-snd |
| `bipd` | `unit.obje.functions` | copied | h1-bipd-colour |
| `bipd` | `unit.obje.attachments` (shield-down effect, flashlight lights) | `effe`, `ligh` | a bug, after h1-bipd-colour: Halo 1's attachments name the functions that lane owns |
| `vehi` | seats, hover and friction points, gears, physics, root and unit words | values | h1-veh-phys |

### Fixed (v86)

| Group | Field | Was | Now (Halo 1) | Evidence |
|---|---|---|---|---|
| `weap` | `ref_16c` overheated effect | the plasma weapons' `overheated` | `weap +0x374`, converted (`Converter::effect`) | V: the slot holds `overheated` on both plasma weapons |
| `weap` | `ref_1bc` melee damage, `ref_1c4` melee response | `smash_melee`/`strike_melee` and responses | `weap +0x394`, `+0x3a4`, converted (`Converter::damage`) | V: 8 of 8 weapons |
| `proj` | `ref_e8` detonation started | the plasma grenade's `detonation_start` | `proj +0x1f4` | V: the one projectile with it |
| `proj` | `ref_f0`, `ref_f8` (airborne) detonation | `airborne_detonation`, `detonation`, `standard_detonation` | `proj +0x1ac`, Halo 1's one detonation effect, in both | V: 3 of 3 grenades and needle |
| `proj` | `ref_110` super detonation | the needle's `super_detonation` | `proj +0x18c` | V |
| `proj` | `ref_138` flyby sound | `bullet_flyby_*` | `proj +0x204`, converted (`Converter::sound`) | V: 8 bullets |
| `weap`, `eqip`, `proj` | `foot_ref_54` material effects | the counterpart's (`effects\materials\objects\weapons\*`) | the first `foot` Halo 1's root names (`Converter::h1_foot`); the counterpart's stays where it names none | as units since v48 (`unit_foot`) |

A slot Halo 1 leaves empty, or whose tag does not convert (noted in the
object's report), is now empty: Halo 1's content, not a retail stand-in.
These slots are named in `arsenal` (`WEAPON_OVERHEATED`,
`PROJECTILE_FLYBY_SOUND`, ...) rather than in the definitions, so the
lanes editing the same files keep their names. Proven by unit test
(`halo1_slots_replace_the_counterparts_references`) and, once run, by the
audit's v86 output; not seen in a game.

An object whose conversion reaches itself (a projectile whose detonation
effect creates it) is now refused at that point instead of recursed into
(`Converter::objects_open`); the effect part is left out and reported.

### Bugs (b) not fixed

| Group | Field | What leaks | Halo 1 source | What it needs |
|---|---|---|---|---|
| `eqip` | `item.ref_124` (effe), `item.ref_10c` (jpt!) | the grenades' `airborne_detonation`, `equipment_detonation`, `*_equipment_explosion` | the item's detonation effect (the explosion when a lying grenade is shot) | `h1::Equipment` does not read the item's detonation fields yet: read them (offsets by pairs), then as `proj` |
| `bipd` | `unit.block_e8[].ref_0` | `camera\biped_follow_camera` | the unit's camera track | read the unit's camera tracks; convert as the seats' (`Converter::camera_track`) |
| `bipd` | `unit.effe_ref_c4` | the Chief's `flashlight` effect | the unit's integrated light toggle | read it, then `Converter::effect` |
| `weap` | the detonation effect | none on Coagulation's weapons | `weap +0x384` | no retail weapon holds one, so its slot is not named yet |
| `deca` | `layer` | the base's majority (5) | the decal's layer (its own enumeration) | Halo 2's layer values named |

**Missing, not leaking.** `jpt!` leaks one word (`+0xa0`, 1.0): the base's
damage effects agree on almost nothing, so a converted damage effect is
Halo 1's bounds over **zeros**. Halo 1's radius, cutoff scale,
acceleration, camera shake and rumble are not translated and not taken
from anywhere either; the audit cannot see a field left at its default.
Naming Halo 2's `jpt!` fields by pairs is the next step there.

### Unidentified: Halo 2 spans not named yet

The role's words in unknown spans. Whether Halo 1 has their equivalent
is open until each is named; the suspects are listed.

| Group | Fields | Values | Suspects in Halo 1 |
|---|---|---|---|
| every object | `+0x6c` .. `+0x8c` of the header | 1, 2.5, 4.5, 0.2, 1.25, 2.5 (7 on the biped), 10, 0.2, 1.25 on every object of every group | none: Halo 1's `obje` has one acceleration scale; likely (a) |
| `bipd` | header `+0x14` .. `+0x1c`; `unit +0xbc`, `+0xc0`, `+0x1b0`, `+0x1b4`; root `+0x1ec` .. `+0x2c8`; the character shapes' (`block_284`, `block_28c`) words | speeds and angles (1.2, 8.49, 3.08, 0.6, 7, 12, cos 45 and 50 degrees, ...) | Halo 1's unit and biped speeds, jump velocity, stationary turning, ground angles; `rebuild::BIPED_WORDS` carries some |
| `bipd` | `ref_25c` | `blood_aoe_human_masterchief` | none: likely (a) |
| `weap` | root `+0x1b0` .. `+0x1b8`, `+0x27c`, `+0x2d8`, `+0x2dc`; barrels `+0x50`, `+0x54`, `+0x68`, `+0xc4` .. `+0xe0` (1000) | | ready time, zoom, autoaim, movement penalty, flags |
| `weap` | `ref_1cc` .. `ref_1ec` | further `strike`/`smash` melee damage | Halo 1 has one melee damage and response (now in `ref_1bc`, `ref_1c4`); likely (a) |
| `proj` | `+0x194` (1.0); `ref_140` (the rocket's `rocket_detonation` only) | | projectile flags |
| `eqip`, `weap`, `proj` | `foot_ref_54` where Halo 1's root names no `foot` | | whether Halo 1's object has material effects at all |
| `jpt!` | `+0xa0` (1.0) | | see "Missing, not leaking" |
| `ligh` | `+0x8`, `+0xc`, `+0x14`, `+0x80` .. `+0x8c`, `brightness` | | the light's intensity and falloff |
| `effe` | particle-system `fade`, `lod`, emitter `+0xb0`, `+0xb4` | | particle fade times |
| `scen` | header words only | | none past the header |

## What the audit does not see

- **Partly role values.** A field built from Halo 1's bits and the role's
  is noted with the role's value, and only reported when it still equals
  it: the seat flags (`rebuild::seat_flags`, Halo 1's low bits over the
  role's), the projectile's `unknown_e4` (the role's high half, Halo 1's
  detonation noise low) and `vehicle_flags` (the role's high half) are
  not reported. The seat flags and vehicle flags are h1-veh-phys's.
- **Retail values in the source that are not a role's**: the physics
  model's body words (`rebuild::DEVICE_BODY`, `BIPED_BODY`,
  `PIECE_MATERIAL`), the stand-in sphere (`convert::STAND_IN_SPHERE`),
  the decal fallback when the base has no decal. Havok's tuning has no
  Halo 1 source; the decal fallback only applies to a base without
  decals.
- **The Halo 2 HUD of a HUD pair** (`health_hud`): built from a retail
  `nhdt` on purpose ([hud.md](hud.md)); its references show up under
  `nhdt` and need rules on the first run.
- **Tags outside `h1objects`** (the structure, sky, shaders made by
  `h1convert`): not in the object tree.

## Results

First run, Blood Gulch on Coagulation, converter v85 (the tables
`ROLE_AUDIT_OUT` wrote; values abridged): 526 tags; leaks by group:
`vehi` 195 rows (all h1-veh-phys's), `weap` 76, `bipd` 71, `proj` 19,
`eqip` 19, `scen` 9, `prt3` 8, `ligh` 7, `hlmt` 7, `shad` 4, `effe` 4,
`deca` 3, `nhdt` 3, `jpt!` 1. Each row is classified above. The rows the
v86 fixes remove are under "Fixed"; the run after them, and the other
twelve multiplayer maps, are still to do (`H1_MAPS=all`).
