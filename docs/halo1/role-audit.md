# Role audit: Halo 2 values in converted Halo 1 tags

Status (2026-11-19, converter v85): the audit tool and its
classification are written; **not run yet**. The tables under "Results"
fill in from the first run (`ROLE_AUDIT_OUT`, below), and anything the
rules do not cover fails the test until it is classified. No converter
output changed, and no leak is fixed yet: every bug found so far sits in a
Halo 2 field that is still unnamed, so it has to be named first (see
"What the bugs need").

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

### Allowed (a)

| Group | Field | What | Why there is no Halo 1 source |
|---|---|---|---|
| `hlmt` | `block_60` (damage info) | copied, references | Halo 2's damage and shield model wins ([campaign.md](campaign.md) decision 2, [health.md](health.md)) |
| `hlmt` | `shad_ref_94`, `shad_ref_f0` | `shad` references | the active camouflage and overshield shaders: Halo 1 draws those from its globals, not per model |
| `prt3` | the static particle encoding (`input_mask`, `vertex_shader`, `quad`, `uv_window`, the unit constants) | values | Halo 2's particle shader inputs; Halo 1's particle values go on the emitter ([../formats/effects.md](../formats/effects.md)) |
| `prt3` | `shader_template` | `stem` reference | Halo 2's shader system; the stem is picked by Halo 1's blend mode (`particles::particle_stem`) |
| `deca` | `unknown_18` (scale), `max_overlapping` | values | no Halo 1 decal field (`rebuild::DecalRole`) |

### Owned by other lanes (not touched here)

| Group | Field | What | Lane |
|---|---|---|---|
| `weap` | `barrels[].block_e4[]` | firing effects and their sounds | h1-weap-snd |
| `weap` | any `snd!`, `lsnd` reference | ready, pickup and zoom sounds | h1-weap-snd |
| `eqip` | any `snd!` reference | the item pickup sound | h1-weap-snd |
| `bipd` | `unit.obje.functions`, `unit.obje.change_colors` | copied with the change colours | h1-bipd-colour |
| `vehi` | `block_1c8[]` (seats), `block_2e8[]` (hover), `block_2f0[]` (friction), `block_268[]` (gears), `block_2f8[]` (physics) | values (the driver seat bit, the Ghost's words) | h1-veh-phys |
| `vehi` | the root and `unit.` words | values | h1-veh-phys |

Sky (sky-depth) and the teleporter and plasma shaders (h1-shader2) are
built outside these builders and are not audited here.

### Bugs (b): Halo 1 has the equivalent

| Group | Field | What leaks | Halo 1 source | Status |
|---|---|---|---|---|
| `jpt!` | every root word past `damage_lower_bound` / `damage_upper_bound` | the majority of the base's damage effects | `jpt!` radius (from, to), cutoff scale, flags, category, instantaneous acceleration, camera shaking and rumble | not fixed: Halo 2's `jpt!` is unnamed past the damage bounds (`crates/tags/src/defs/jpt_.txt`); name the fields by Halo 1 / Halo 2 pairs, as the bounds were, then translate |
| `weap` | the `jpt!` references (melee damage, melee response) | the counterpart's (the SMG's on Blood Gulch) | `weap` melee damage and melee response | not fixed: convert with `Converter::damage` once the slot is named |
| `weap` | the `effe` references outside the barrels | the counterpart's | `weap` overheated, detonation, light power on and off effects (the ready effect is h1-weap-snd's) | not fixed: Halo 1 `effe` converts (`Converter::effect`); the unnamed Halo 2 slots (`item` `ref_10c`, `ref_124`; `weap` `ref_140`, `ref_16c`, `ref_1bc` .. `ref_1f4`, `ref_264` .. `ref_274`, `ref_2f0` .. `ref_300`) need pairing |
| `proj` | the `effe` references | the counterpart's | `proj` detonation started, detonation, super detonation and attached effects | not fixed: pair `ref_e8` .. `ref_15c` with Halo 1's, then convert |
| `proj` | the `snd!` reference | the counterpart's flyby sound | `proj` flyby sound | not fixed: pair the slot, then `Converter::sound` |
| `deca` | `layer` | the base's majority layer | `deca` layer (Halo 1's own enumeration) | not fixed: Halo 2's layer values need naming to map Halo 1's |

### Unidentified: Halo 2 spans not named yet

Each is the role's word in an unknown span. Until the span is named,
whether Halo 1 has its equivalent is open; the suspects are where Halo 1
holds something of the same kind.

| Group | Fields | Suspects in Halo 1 |
|---|---|---|
| any | the object header's words (`object.`, `item.obje.`, `unit.obje.`) | `obje` flags (does not cast shadow, transparent self-occlusion, not a pathfinding obstacle), acceleration scale, render bounding radius |
| `weap` | root, magazine and barrel words past `arsenal::WEAPON_WORDS`, `BARREL_WORDS` and the magazine fields | ready time, zoom levels and magnification range, autoaim, movement penalty, weapon and trigger flags |
| `proj` | words past `arsenal::PROJECTILE_WORDS` | projectile flags, detonation timer start, guided angular velocity |
| `eqip` | item and equipment words | item flags, pickup text index |
| `bipd` | words past `rebuild::BIPED_WORDS` | unit speeds, jump velocity, seat acceleration, unit and biped flags |
| `hlmt` | scenery models' words | `mod2` LOD cutoff distances |
| `scen` | words past the object header | none: Halo 1's `scen` holds nothing past its `obje` (likely (a)) |
| `garb` | item words | none past the item header (likely (a)) |
| `ligh` | words and `brightness` | the light's intensity and falloff function |
| `effe` | particle-system and emitter words (`unknown_c` .. `unknown_16`, `fade`, `lod`, `unknown_9c`, `unknown_b0`, `unknown_b4`) | particle fade times |

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

Not run yet. The first run's tables (`ROLE_AUDIT_OUT`) go here, with the
unclassified rows it prints moved into `ALLOWED` or `KNOWN`.
