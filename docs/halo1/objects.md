# Halo 1 sandbox objects as Halo 2 tags (`crates/tools/h1objects`)

Status: **Built** for reading every Halo 1 Xbox object tag on the 13
multiplayer maps, and for building Halo 1's objects (Blood Gulch's arsenal:
assault rifle, pistol, plasma rifle, needler, frag and plasma grenades,
health pack, cyborg, warthog, ghost; the campaign levels' objects) as Halo 2
tags. The built tags read back through `crates/formats/tags`, and the
converted maps load in the game; what only a game run shows is under "Needs
a game run".

This is the Halo 1 sandbox of [multiplayer.md](multiplayer.md)
and the object step of [campaign.md](campaign.md) §2.
Physics is Halo 2's (decision 1); Halo 1's health is kept (decision 2,
[health.md](health.md)). Models and animations come from
[models.md](models.md); the Halo 2 object family is in
[objects.md](../formats/objects.md), collision in [collision.md](../formats/collision.md).

No map bytes, tag dumps or extracted assets are in the repository. The
unit tests use invented data; the `#[ignore]` tests read the discs at run
time and print counts, names and values only.

## Evidence labels

- **V1:** checked on every object tag of the 13 Halo 1 Xbox multiplayer
  maps by `every_object_tag_on_every_multiplayer_map_reads`: 195 `weap`,
  208 `proj`, 182 `eqip`, 26 `bipd`, 39 `vehi`, 364 `coll`, 39 `phys`,
  and the references of 4,137 `effe`, 858 `jpt!`, 4,110 `snd!`, 104 `cont`
  and 391 `ligh`. Checks: the object type matches the group, model-chain
  references have their group, every collision BSP passes
  `CollisionBsp::validate`, node/sphere indices are in range, masses and
  radii are sane.
- **V2:** the value pattern on retail Halo 2 tags (Coagulation): the same
  field holds the expected value on the SMG, magnum, plasma rifle and the
  grenades (for example the magazine's loaded maximum and the
  barrel's heat per round are nonzero exactly where the weapon has a
  magazine or heat).
- **S:** field order and meaning unverified, placed by the data.
- **E:** from [objects.md](../formats/objects.md) (engine code).
- **Design:** a choice made here.

## Source widgets

The Xbox discs contain 51 antenna references, 175 light-volume
widget references, 15 lightning references, 13 cloth references, eight sword-glow
references and one additional volume attached to
the beam emitter on a10. The ignored `every_retail_widget_reaches_its_owner`
test follows each source occurrence through the public object converter and
checks the destination block, local tag, group and attachment marker.

- **Antenna.** The three vehicle antennas have matching vertex layouts on both
  discs. The H2 widget table at `0x467498` dispatches an `ant!` from object
  offset `0x9c`. Creation at `0x115da0` uses the bitmap sequence and cached
  joint displacements. The builder relocates the marker, bitmap, point physics
  and every source vertex. Its 20-segment limit leaves room for the terminal joint.
- **Light volume.** H1 `0x97d10..0x97e6a` applies spacing before sampling axial
  distance, radius, colour and opacity. H2 `0x41a24..0x41b91` resolves the
  owner's attachment marker and evaluates the primary scale before queuing
  `MGS2`. The builder writes the sample coordinates and curves into that
  definition. A null source image selects the glow bitmap in the source
  rasterizer globals, as in H1 `0x64495`.
- **Sword glow.** H1 `0x99420` orders five animated markers by forward
  alignment. `0x98ac0`, `0x98ce0` and `0x98340` assign sprites their
  radii, colours, travel directions and positions on the curve. Each sword
  glow becomes one H2 volume attachment naming its original marker group and
  sprite bitmap. H2 sorts indices into the marker queue at `0x1e392`.
  At the volume submission `0x4f792`, the context-only glow hook reads the
  consecutive marker frames and writes the orbiting sprites into the native
  sample array. The per-instance random sequence uses the owner and tag datums.
- **Lightning.** Each `elec` becomes a native `tdtl` widget with one layer per bolt
  and one attachment per source marker, in order. H2 queues the marker frames at
  `0x429a0` and submits the ribbon at `0x51648`. The marked displacement curve
  carries each endpoint's jitter, width, colour, break flag and subdivision depth.
  The renderer interpolates the endpoint styles over recursively displaced midpoints
  (H1 `0x96de0..0x97332`) and passes the samples to H2. The material setup applies
  the source double-multiply or additive blend equation from H1 `0x7f4e0`.
- **Multiplayer flag.** H2 pins vertex zero to a marker (`0x1171a0..0x1173ae`)
  and has space for 128 vertices per cloth datum. Two native `clwd` panels each
  hold 112 vertices, sharing the middle 16 to preserve the source 16 × 13
  grid, its UVs and both mast pins. H2 solves constraints and calculates normals at
  `0x1168a0`. The marked panels share their seam positions, velocities and normals.
  Their integration stage uses H1's point mass, air/liquid buoyancy and drag,
  plus its three-impact sweep (`0x9a56d`, `0x80970..0x80b5a`) against the H2
  structure. The marked panel stores friction at +0x24 and restitution at +0x2c.
  The material lookup at `0x116ac0` selects between the two converted H1 shaders
  using the objective's team (H1 `0x9a92e`).

## Reading Halo 1

Halo 1 references are 16 bytes (group, name address, name length, datum);
blocks are 12 (count, address, unused). The root size of each group is
where its first child block starts (V1).

**References without a layout (`refs::scan`).** A 16-byte run counts as a
reference when its datum names a tag of that group (or a child group) and
its name address is that tag's name, or when it is a null reference of a
group the map uses (no name). This is how `effe`, `jpt!`, `snd!`, `cont`
and `ligh` are read (references only, not their bodies). Note that many
null references in cache files have a zero group word, so they are not
found; the typed readers below read nulls by offset.

### `obje` (`0x17c`), shared

| Offset | Field | Evidence |
|---|---|---|
| `00` | u16 object type: bipd, vehi, weap, eqip, garb, proj, scen, mach, ctrl, lifi, plac, ssce | V1 (matches the group on every tag) |
| `02` | u16 flags | S |
| `04` | f32 bounding radius, `08` f32x3 bounding offset | S, V1 (finite) |
| `28` | `mode`, `38` `antr`, `70` `coll`, `80` `phys` | V1 |
| `90` | modifier shader, `a0` creation effect | S |
| `140` | attachments (`0x48`, reference at `+0`), `14c` widgets (`0x20`), `158` functions, `164` change colours, `170` predicted resources | S, V1 (blocks in range) |

### `item` (`0x17c..0x308`)

`248` material effects (`foot`), `258` collision sound, `2e8` detonating
effect, `2f8` detonation effect (V1: groups).

### `weap` (root `0x508`)

| Offset | Field | Evidence |
|---|---|---|
| `308` | u32 flags; `30c` label (`char[32]`): the weapon type the units' graphs animate it by (`stand rifle ar idle`): `ar`, `hp`, `sg`, `pr`, `rl`, `sr`, `pp`, `ne`, `ft`; `cg`, `cannon`, `gun` on vehicle guns | S; V1 (every Blood Gulch `weap`) |
| `330`..`336` | u16 A to D in sources, the weapon enum: 2 primary ammunition, 12 illumination, 14 integrated light ("Ammunition counter") | D (the assault rifle: 14, 2, 12, 0) |
| `338` | f32 ready time, `33c` ready effect | S, V1 |
| `34c`..`360` | heat recovery threshold, overheated threshold, detonation threshold, detonation fraction, heat loss per second, illumination | S; V1 (set on the plasma rifle, zero on the assault rifle) |
| `374`, `384` | overheated, detonation effects | S |
| `394`, `3a4` | melee damage, melee response (`jpt!`) | V1 |
| `3bc` | actor firing parameters (`actv`) | V1 |
| `3da` | i16 magnification levels, `3dc` f32x2 magnification range | V1 (nonzero only on zooming weapons) |
| `3e4`..`3f4` | `cone_angle`, `cone_range`, `pull_angle`, `pull_range`, `bend_angle` (the Halo 2 `weap +0x208..+0x218` order) | S, V1 (angles in radians, ranges in world units) |
| `418`, `41c`, `420`, `430` | light power on/off time and effects | S, V1 |
| `45c`, `46c` | first-person model, its animations | V1 |
| `480` | HUD interface (`wphi`); `490`, `4a0`, `4b0` pickup, zoom in, zoom out sounds | V1 |
| `4e2` | i16 weapon type | S |
| `4e4`, `4f0`, `4fc` | predicted resources, magazines (`0x70`), triggers (`0x114`) | V1 |

**Magazine (`0x70`):** `04` i16 rounds recharged, `06` total initial, `08`
total maximum, `0a` loaded maximum, `14` f32 reload time, `18` i16 rounds
reloaded, `1c` f32 chamber time, `38` reloading effect, `48` chambering
effect, `64` magazine objects (`0x1c`: i16 rounds, `+0xc` `eqip`). V1
(rounds reloaded equals the loaded maximum on the assault rifle).

**Trigger (`0x114`):** `04` f32x2 rounds per second, `0c`/`10`
acceleration and deceleration time, `20` i16 magazine, `22` rounds per
shot, `24` minimum rounds loaded, `26` rounds between tracers, `2e` firing
noise, `38`/`3c` error acceleration and deceleration time, `50` charging
effect, `6c` i16 distribution function, `6e` i16 projectiles per shot,
`70` distribution angle, `78` minimum error, `7c` f32x2 error angle, `84`
first-person offset, `94` projectile, `b8` heat per round, `108` firing
effects (`0x84`). V1 (`94` is a `proj` on every trigger with one), S
otherwise.

**Firing effect (`0x84`, `h1::FiringEffect`):** `00`/`02` i16 shot count
lower and upper, `24` firing, `34` misfire, `44` empty effect (`effe`,
or a bare `snd!`: the plasma rifle's empty effect on a10 is one), `54`
firing, `64` misfire, `74` empty damage (`jpt!`). V (every map,
`weapon_sounds_disc::halo1_firing_effect_layout`: no other reference in
an element), names S. A `snd!` converts as a sound and stays one in
Halo 2's slot, which takes either group (retail names both there). A shot plays the firing effect, whose `snd!` parts
are the weapon's fire sound; the converted weapon names Halo 1's
(docs/formats/tag-definitions.md, "What a shot and a reload play").

### Weapon effects and sounds

Every effect and sound a converted weapon plays is Halo 1's; a slot Halo 1
leaves empty stays empty (the engine
skips a null datum at each read below), never the Halo 2 counterpart's.
Halo 2 offsets: V from the groups and names on every retail weapon of
Coagulation (`h2 object coagulation <weapon>`), E where an engine read is
cited. Every root and item offset below is also typed in
`tags::object::layouts` (`ITEM`, `WEAP`) as a reference of that group,
checked on every map of the disc, so the S rows are S for the name only.
The tag schema (`tags::schema`) found only the slots some retail weapon
fills; the others (`+0x17c`, `+0x244`, `+0x24c`, the item's three, the
magazine's `+0x44`) are references there by `infer::ENGINE_REFS`, so a
converted weapon's tag in them is read back and moved as a tag, not as
bytes (docs/formats/tag-schema.md, step 10).

| Halo 2 | Halo 1 | Field | Evidence |
|---|---|---|---|
| `weap +0x140` | `weap +0x33c` | ready effect | V: `needler\ready` (needler, SMG), `energy_blade\blade_activate_fp` |
| `weap +0x16c` | `+0x374` | overheated effect | V: the plasma pistol's, plasma rifle's, ghost's `overheated`; E `0x000cf12c` reads `+0x170` when the weapon's overheated bit is set |
| `weap +0x17c` | `+0x384` | detonation effect | S: `effe` group words, then `jpt!` at `+0x184`; E `0x000cf19f` reads `+0x180` |
| `weap +0x244`, `+0x24c` | `+0x420`, `+0x430` | light power-on, power-off effect | S: `effe` group words, in Halo 1's order |
| `weap +0x264` | `+0x490` | pickup sound | V: `smg_ammo`, `magnum_ammo`, ... on 10 weapons; E `0x000a009a` plays `+0x268` (through `0x001896c0`) when the object picked up is a weapon |
| `weap +0x26c`, `+0x274` | `+0x4a0`, `+0x4b0` | zoom in, zoom out sound | V: `sniper_rifle\zoom_in`, `zoom_out`, the beam rifle's; E `0x000c5ba8` (`+0x270`) and `0x000c5ba0` (`+0x278`), by the weapon's magnification levels (`+0x1fe`); without a weapon definition the globals' zoom sounds play |
| `item +0xfc` | `item +0x258` | collision sound | S: `snd!` group words |
| `item +0x11c`, `+0x124` | `+0x2e8`, `+0x2f8` | detonating, detonation effect | S: `effe` group words |
| magazine `+0x34`, `+0x44` | magazine `+0x38`, `+0x48` | reloading, chambering effect | V (`+0x34`, the SMG's); S (`+0x44`, between two `jpt!` group words) |

A Halo 1 slot may name a `snd!` or an `effe`; each converts by the group
of the tag it names (`Converter::weapon_slot`). Keeping the counterpart's
would play the SMG's ready effect when the assault rifle is drawn and its
ammunition sound when it is picked up. The melee damage
(`+0x1bc..+0x1f4`), the firing damage and the material effects
(`item +0xf4`) are the counterpart's.

Halo 1's reload, ready and other first-person sounds are on the
animations, not the weapon: see [models.md](models.md), "Animation
sounds".

### `proj` (root `0x24c`)

| Offset | Field | Evidence |
|---|---|---|
| `17c` | u32 flags; `180` i16 detonation timer starts, `182` impact noise | S |
| `18c` | super detonation; `1ac` effect | V1 |
| `19c`..`1a8` | AI perception radius, collision radius, arming time, danger radius | S, V1 (set on grenades) |
| `1bc` | f32x2 timer | S |
| `1c4` | minimum velocity, `1c8` maximum range | S, V1 (equal to the Halo 2 template's on bullets) |
| `1cc` | air gravity scale, `1d0` air damage range, `1d8` water gravity scale, `1dc` water damage range | S, V1; both gravity scales are written times `GRAVITY_RATIO` (`arsenal::PROJECTILE_GRAVITY`), so the arc and a bullet's drop are Halo 1's under Halo 2's gravity (`projectile_gravity_keeps_halo1s_arc`) |
| `1e4`, `1e8`, `1ec` | initial velocity, final velocity, guided angular velocity: **per tick** | S; V1: bullet values are a thirtieth of a plausible speed per second |
| `1f0` | i16 detonation noise; `1f4` detonation started; `204` flyby sound; `214` attached detonation damage; `224` impact damage | V1 |
| `240` | material responses (`0xa0`, 33 on every projectile) | V1 |

### `eqip` (root `0x3b0`)

`308` i16 powerup type (0 none, 1 double speed, 2 overshield, 3 active
camouflage, 4 full-spectrum vision, **5 health**, 6 grenade), `30a` i16
grenade type, `30c` f32 powerup time, `310` pickup sound. V1 (the health
pack is type 5 with a positive time; the grenades are type 6).

### `effe` and attachments

Read field by field (`h1::Effect`,
`h1::attachments`): layout and evidence in
[tag-definitions.md](../formats/tag-definitions.md), "Effects from definitions".

### `part` (root `0x164`) as `prt3`

Blend 6 uses the RGB maximum of the destination and the tinted sprite.
Glass breakable samples its moving secondary texture at the sprite UVs.
Glass death projects that texture in screen coordinates. Glowingdrip goo
uses the primary sprite alone. The compiler writes the texture bindings,
vertex transforms, motion properties and colour combiner for each case.
Blend 7 ice uses the same moving screen-space mask as glass death. Its
compiled pass adds the tinted sprite to the scene, attenuating the scene
by one minus the product of sprite alpha, mask alpha, particle opacity and
fog visibility.

`h1objects::particles::H1Particle` and `Converter::particle`: the stem
by the framebuffer blend function at `+0xda` (0 alpha blend, 3 additive,
6 maximum, 7 alpha-multiply-add), the bitmap (`+0x4`) as
the stem's `alpha_map_texture` with the compiled shader built from the
stem ([tag-definitions.md](../formats/tag-definitions.md), "The compiled particle
shader"), Halo 1's sprite sequences (`+0x98` first, `+0x9a + +0x9c`
counts, one picked per particle), the collision and death effects
(`+0x48`, `+0x58`). The five properties and the input mask, vertex
shader, quad and UV window are those of the base's static particles
(white, opaque, unit size, unrotated, first sprite; the most common
set). Lifespan, fade, radius and frame animation, physics and the rest
are emitter-side in Halo 2.

### Effect particles (event `+0x38`, `0xe8`) as particle systems

Each Halo 1 effect particle whose `part` converts becomes a particle
system of its event (`particles::effect_particles`, `particle_system`,
`Converter::effect_particle_systems`), one emitter on the proven slots ([effects.md](../formats/effects.md),
"Emitter"). The record (V, `halo1_effect_particle_survey`): `+0x8`
location, `+0x54` the `part`, `+0x6c`/`+0x6e` count bounds, `+0x70`
distribution radius, `+0x84` velocity bounds, `+0x8c` velocity cone
(radians), `+0x90` angular velocity bounds, `+0xa0` radius bounds,
`+0xb0`/`+0xc0` tint bounds (alpha, red, green, blue), `+0xe0`/`+0xe4`
scale words.

- Count: the engine spawns `rate x 30 x dt` per step, `dt` the event's
  step in seconds (E, the tick length `[0x510c54]+4` cut to the event's
  remaining time), so the rate is Halo 1's count over 30 times the event's
  duration, floored at a tick; an event with particles shorter than a
  tick lasts one (retail's usual duration). Halo 1 creates the count at
  once; over a longer event Halo 2 spreads it.
- Lifespan (the `part`'s), speed (velocity), cone (degrees), size (twice
  the radius: unproven), tint (packed ARGB: unproven) and alpha: Halo 1's
  midpoints, as constants.
- The words no slot names (system flags, fade and LOD distances; emitter
  `+0x9c..+0xb8`; the constants' input words) are the base's most common
  (`EmitterRole`).
- Not carried: distribution radius, relative direction and offset,
  angular velocity, the `part`'s physics (`pmov` null), the scale words.
- A particle's collision or death effect that leads back to the effect
  being converted is left out.


### `pctl` (particle systems) as particle systems

An effect part naming a Halo 1 `pctl` becomes particle systems of its
event (`particles::particle_system_types`,
`Converter::particle_system_states`), after the event's own
particles. The layout (V, all 28 distinct tags,
`halo1_pctl_structure_survey`): a `0x68` root (`+0x38` point physics,
`+0x48` update physics, `+0x50` 4-byte constants, `+0x5c` particle types
of `0x80`); a type's `+0x0` name, `+0x24` initial count, `+0x2c` radius,
`+0x54` creation physics (0, 1, 2) and `+0x5c` its 4-byte constants,
`+0x68` system states (`0xc0`), `+0x74` particle states (`0x178`); a
particle state's `+0x0` name, `+0x20` duration and `+0x28` transition
bounds, `+0x30` bitmap, `+0x40` sequence index, `+0x48` radius bounds,
`+0x58` rotation rate bounds, `+0x60`/`+0x70` colour bounds, `+0x84`
point physics, and from `+0xdc` `part`'s shader words (`+0xe2` the blend
function: 298 fire states 3, 104 smoke states 0).

Each particle state that lasts becomes its own system, its `prt3` built
from it as from a `part`, all created with the part: the type's initial
count, the state's duration plus transition as lifespan, its radius and
colour. Halo 1 moves each particle through the states in order (fire
turning to smoke); here every state's particles appear at once. Not
carried: the state sequence, the creation physics and its constants (no
proven meaning), the system states, the rotation rate. The particles
start still at the part's location.

### Light fixture placements (Built)

The Halo 1 XBE's fixture descriptor at `0x1fca28` selects scenario
`+0x2c4` with stride `0x58`. Its constructor at `0x118e00` copies
placement RGB at `+0x30` and intensity at `+0x3c` into the live fixture.
The converter retains each fixture as a machine with its original
marker attachments. An authored tuple selects a rebuilt light variant
with that RGB at both colour endpoints and that intensity at both
brightness endpoints. Halo 2's `0xc0dc9` path reads those fields. A zero
tuple uses the fixture's own attached light.

There are 886 fixture placements across the retail maps. b30 has all 42
authored RGB and intensity tuples. Carousel, Damnation and Rat Race add
46 multiplayer placements. Both multiplayer asset variants install their
fixture machines and palette into the fresh scenario. The ignored
`both_sandboxes_rebuild_every_retail_map` check follows each placement and
its authored light values through the output.

### Object attachments

Every converted object kind uses Halo 1's `obje +0x140`
block for its marker attachments. Halo 2's attachment dispatcher
(`0x000bed2a`) creates lights, effects, contrails and looping sounds.
Their tags and marker names come from Halo 1, and the scales name the
converted object functions. Light fixtures use the same path on the
machines they become. The attachment reader also carries the
change-colour selector, translated to the same slots as the object's
colours. Halo 2's effect updater skips colour slot 0 (`0x00179610`).
A source function that lacks a supported curve or engine input stays
unconverted, with its reason in the report. Halo 1's additive flag
(H1 `0x8c06d..0x8c088` adds the previous output and wraps the sum by
one) is such a case, and so is a function that turns off with one that
did not convert (H1 `0x8c050..0x8c06b`). An attachment whose
primary or secondary scale names an output with no converted function,
or with no function at all, fails the conversion with the attachment,
marker and output (`functions::primary_scale`); an empty Halo 2 scale would
run the attachment at one where Halo 1 scaled it by that output.

Halo 2 does not dispatch `pctl` attachments directly. Each emitting
particle type becomes an attached `effe` with the source marker as its
location, an optional initial burst, and native emitter events. Halo 1
reads the emission-state rate in particles per second (`pctl` state
`+0x58`, E `0x0010f6a5`); Halo 2 reads particles per tick
(`0x00247100`). State durations, timed transitions and loop mode
come from Halo 1. Halo 2 evaluates the transition rates over native
event time. The existing particle-state conversion supplies the
textures, sizes, tints and lifetimes. Visual states still have separate
native emitters.

Attachment physics: the native emitter inherits the owner’s
motion. Its birth hook (`0x00249170`) uses the source’s radial scatter or
directional kick (`0x0010f2d0`, `0x0010f150` in Halo 1). Each particle
state’s `pphy` supplies the mass, collision radius scale, air and liquid
drag, buoyancy, friction and rebound. A marked native `pmov` stage keeps
Halo 2’s collision sweep and bounce (`0x002b9060`). The movement hook
applies Halo 1’s point forces before that sweep (`0x00080720` in Halo 1),
choosing the coefficients from the active BSP’s liquid plane where
present, and applies the source type’s death flags. Halo 2’s particle
initialisation and simulation own all mutable particle state.

Minimum-population clock: each native phase carries
the source minimum (including the transition endpoints and the
attachment-scale flag). After Halo 2 steps its emitters
(`0x00179020`), the clock shortens the remaining phase time to 30%
when the live count is below that threshold. Halo 1 does this at
`0x0010f928` before advancing the particle-state clock. To account
for parallel visual states, the hook counts the largest surviving
stream per phase and includes surviving streams from earlier phases.
The conversion continues to use separate visual layers and their
individual lifetimes, so the count can differ from Halo 1's sequential
particles. Particle forces use the active BSP’s physical liquid
volumes where available.

On a30 the entry pod's `burn` marker carries `burning big debris`,
starting at zero particles and emitting 30 per second (V).
`effects_disc::entry_lifepod_burns_at_its_halo1_marker_and_rate`
checks the model marker, the effect loop, the visible states, the
emitter rate, the minimum population and the native collision curves. `placed_object_builders_keep_halo1_attachments`
checks the attachment paths across object kinds.

### `cont` (root `0x144`) and contrail attachments

`h1objects::particles::H1Contrail`: Halo 2's
`cont` is Halo 1's compacted, span for span (V: the sniper, needler and
plasma contrails of both games word for word, `particles_disc.rs`
`contrail_survey`). `particles::CONT_SPANS` and `CONT_POINT_SPANS` list
the spans; the layout is in [tag-definitions.md](../formats/tag-definitions.md),
"Particles and contrails". Halo 2 effect parts never name a `cont` (the
part groups on the disc are `deca`, `snd!`, `obje`, `ligh`, `lens`,
`jpt!`, `MGS2`, `tdtl`): contrails are object attachments, so a Halo 1
attachment that names one converts it (`Converter::contrail`), on
vehicles with the other attachments and on rebuilt projectiles, whose
attachment block is replaced by Halo 1's when any of them converts. A
point with an authored physics reference names its rebuilt `pphy`. The definition carries the source flags, mass, buoyancy, density, drag, friction and restitution.

### `bipd`, `vehi`, `scen`

Read as the `obje` header (plus every reference, `refs::scan`, for the
units). Their tuning is not read yet (below).

### `coll` (root `0x298`)

| Offset | Field | Evidence |
|---|---|---|
| `08` | f32 maximum body vitality; `cc` maximum shield vitality | V1 (nonzero on the cyborg) |
| `234` | materials (`0x48`, name at `+0`, `u16` type at `+0x24`: an index into Halo 1's material types, `arsenal::MATERIALS` order) | V1 (Blood Gulch: boulders stone 2, trees wood 4, the cyborg cyborg armor 21, the warthog rubber 8, metal 7 and glass 9) |
| `240` | regions (`0x54`): name, `+0x38` destroyed effect, `+0x48` permutations (`0x20`, a name) | V1 |
| `24c` | modifiers (`0x34`), counted only | S |
| `280` | pathfinding spheres (`0x20`): i16 node (-1 none), `+0x10` centre, `+0x1c` radius | V1 |
| `28c` | nodes (`0x40`): name, `+0x20` i16 region, `+0x22` parent, `+0x24` next sibling, `+0x26` first child, `+0x34` BSPs (`0x60`, the structure's collision BSP form) | V1 (every BSP validates) |

A node's BSP `i` belongs to permutation `i` of its region (V1: never more
BSPs than permutations).

### `phys` (root `0x80`)

`08` mass, `0c` centre of mass, `18` density, `1c` gravity scale, `50`
moments, `5c` inertia matrices (2 x `0x24`), `68` powered mass points
(`0x80`), `74` mass points (`0x80`: name, `+0x20` powered mass point,
`+0x22` model node, `+0x2c` mass, `+0x34` density, `+0x38` position,
`+0x68` radius). V1 (strides; finite positions).

## Building Halo 2 tags (Design)

**Scenery has no template**: the `scen`, its
`hlmt` and its `mode`'s material list are written field by field from
typed definitions, with the base's scenery role words in the unknown
spans and Halo 1's model and bitmaps as predicted resources
([tag-definitions.md](../formats/tag-definitions.md), "Scenery from definitions").

**Weapons, equipment, projectiles and damage have no template**: they are built from their typed definitions,
with Halo 1's values and no template bytes ([tag-definitions.md](../formats/tag-definitions.md),
Weapons, equipment, projectiles and damage). The template rows below
for `weap`, `eqip` and `proj` only name the Halo 2 counterpart whose
references (HUD, effects, sounds, melee damage) fill the slots Halo 1
has nothing converted for.

**Vehicle drive.** Each vehicle comes from the `vehi` definition with
Halo 1's model, collision, seats, markers and powered mass points, on the
drive tuning of the matching Halo 2 vehicle: the warthog and rocket
warthog on the warthog, the ghost on the ghost, the scorpion on the
scorpion, the banshee on the banshee (`rebuild::DRIVE_COUNTERPARTS`,
declared per vehicle as `SWAPS` is). The counterpart's gears, engine,
steering, ground and hover laws and point tuning are copied from the base
map; the powered mass points name and place the points. A vehicle with no
counterpart is a named error, except a prop a cutscene or recorded
animation moves, a scripted set piece, a dropship and a turret, which keep
Halo 1's speeds and turn limits. Vehicles have no template row. [Drive
counterparts](role-audit.md#native-engine-fields).

**Vehicle seats.** A Halo 1 seat pairs with a counterpart seat by kind
(driver, gunner, passenger, read from the seat flags) and then order
within the kind; extra seats take the last of their kind, a kind the
counterpart lacks takes its first seat (`rebuild::PAIR_SEATS`,
`rebuild::counterpart_seat`). The seat takes the counterpart's entry
prompt, HUD interface, AI role and built-in gunner
(`rebuild::SEAT_PROMPT_FIELDS`), and its entry radius, facing angle and
relative-velocity limit (`rebuild::SEAT_ENTRY_FIELDS`). Halo 1 supplies
the turn rates, as the upper bounds of `yaw_rate_bounds` and
`pitch_rate_bounds` (`h1::Seat::rates`, `+0x7c` and `+0x80`, inferred
from the camera marker at `+0x84`). The entry cone stays pi on every seat
(Entry facing, below). Only a turret, a vehicle with no counterpart,
holds `rebuild::TURRET_SEAT_ROLE`; every seat holds no hidden region.

**Lifepod engine audio.** The a30 entry pod attaches the Halo 1
`sound\sfx\vehicles\lifeboat` loop. Its marker is empty and its primary
scale is zero. The rebuilt attachment names that level's converted loop,
whose track plays `lifeboat_entry`. The other lifepod variants have no
source engine attachment. The campaign disc test compares the ordered
loop attachments of converted vehicles with their source and checks a30's
entry pod explicitly. [Campaign audio evidence](sounds.md#campaign-loops-and-cache-slots-built).
### Native pickups and scripted props

`h1objects::convert_powerups` builds Halo 1's overshield, active
camouflage and health pack as fresh `eqip` and model chains.
The source provides the bounding sphere, pickup time, model, collision,
pickup sound and effect references. The paired retail `eqip` tags on
`shared` and `single_player_shared` establish the item sort word and
powerup fields (V). Overshield is type 2 and 60 seconds, camouflage type 3
and 45 seconds, and health type 5 with a positive time. Halo 2's pickup
routine reads those fields at `0x00151eff` and `0x00151f3e`, dispatches
shield at `0x00151f4b`, health at `0x00151f6a` and camouflage at
`0x00151f85` (E). The multiplayer conversion rebuilds the collections in
both settings ([multiplayer.md](multiplayer.md#what-isnt-placed-and-why)).

In `h1objects::scripted_props` and the campaign placement writer, b30's
seven boxes are dynamic `bloc` instances with the Halo 1
storage crate's model and collision. Halo 1's vehicle tag references a
`phys` for the mass and leaves the object's radius at zero. The rebuilt
crate takes its bounds from its model and its shape and inertia from
the collision hull. Halo 2's `crate_space` on 05a establishes a
dynamic `bloc` with one object-space body (V). a10's Halo ring is a
`scen`, as in Halo 2's 00a. The ring and boxes keep their scenario names,
transforms and script-controlled creation.

a10's seated props and aircraft: the cryotube and chairs
keep their source stationary vehicle type 6 and seats. a10's scripts call
`unit_enter_vehicle` or `vehicle_load_magic` on them (V).
Their collision bodies follow the model's nodes, including the cryotube
door. The fighter-bomber is a type 3 aircraft for `recording_play`. Its
`phys` has eighteen mass point spheres, which form one native compound
body. Halo 1 supplies mass, center of mass and inertia.

Lifepods: the flying pods use Halo 1's type 5, model,
collision, seats and flight rates. Where Halo 1 supplies `phys` (the
plain pod and a30 entry pod), the collision-derived rigid body keeps its
mass, center of mass and inertia tensor. The flight hook applies H1's positive-radius lifepod controller at
`0x37760` when the source supplies two powered groups.
The docked pod keeps its stationary body and source lights. a30's entry pod
attaches its own `lifeboat` loop, including the source track samples,
through `h1_assets::sounds::looping`, and its reentry fire through the native
`effe` attachment on `burn`. The plain and docked tags carry no engine
loop (V). Offline checks are in `vehicle_rebuild_disc` and
`h1campaign/tests/disc.rs`.

**Where a vehicle meets the ground.** Three
things the engine reads at a vehicle's base, proved in
`crates/tools/h1objects/tests/vehicle_ground_disc.rs`:

- **Bounding sphere.** Halo 1 centres every vehicle's sphere on its
  origin (`obje +0x08` zero on all of them), and its origin is where the
  wheels or hull touch the ground: every Blood Gulch vehicle placement
  is 1 mm above the floor. Halo 2 lights an object from that centre
  (E: `0x000d1850` passes `obj +0x30` to `0x000d2a50`, which casts from
  0.01 above it straight down 10 units, `0x004406e4` and `0x0045dc70`,
  and reads the lightmap where the ray meets the structure) and finds
  its cluster from it, then from the origin (`0x000b8600`: `obj +0x40`,
  then `obj +0x64`; with neither in a cluster the object is outside the
  map, `obj +4` bit 18, and the player's action search `0x00151110`,
  which walks the clusters round the player, never offers it). A centre
  a few millimetres under the ground leaves the vehicle unlit and,
  outside the map, unboardable. The retail warthog's centre is 0.29
  above its origin, inside the body; the ghost, banshee, scorpion and
  wraith keep theirs on the origin (the wraith's 3 cm under its model),
  so retail holds no general rule, and a centre on the contact point is
  a centimetre of sinking from dark. A rebuilt vehicle's sphere is its model's: the middle of the box
  round every vertex, reaching the farthest one
  (`rebuild::model_sphere`).
- **Entry facing.** `0x000c9040` admits a rider only when the entry
  marker's forward points back at the rider's eye within the seat's
  `+0x9c`. Retail entry markers face out of their door (the warthog's
  `driver_enter`: (0.15, 0.85, 0.5)); Halo 1's `driver enter` faces the
  vehicle's forward, so with the warthog role's 70 degrees a rider square
  beside the door was refused; only one ahead of it, looking back, got in. Rebuilt seats hold pi there
  (`rebuild::SEAT_ENTRY_FACING`), as the retail turrets do.
- **Spawn height.** The ghost's hull reaches 0.143 below its origin, so
  at Halo 1's placement it began 0.14 into the floor, where Halo 1's
  hover mass points lifted it and Halo 2 left it. The Halo 1 sandbox
  raises each vehicle spawn by how far its model reaches below its
  origin (`h1convert::convert::halo1::vehicle_depth`). The Halo 2
  sandbox raises each retail vehicle the same way, by the lowest z of
  its render model's position bounds
  (`h1convert::placements::retail_vehicle_depth`: the retail ghost
  0.15, the warthog 0.02, the wraith none).

**The Blood Gulch ghost.** The converted `mode` keeps the ghost's `__base`
and `~damaged` permutations in that order and its `hlmt` has no variants and
no damage sections (`hlmt +0x60` empty), so nothing selects the damaged
model: E, the region permutation update (`0x0016d660`) gathers candidates
only from the object's variant (`0x0016dad0`, through `hlmt +0x54`), and with
none the region keeps permutation 0, the intact one. At rest the ghost lies
on its hull, as in Halo 1: its `phmo` is Halo 1's collision with Halo 1's
`phys` mass, and neither physics shape hovers without a driver. Driven, a
hover point whose marker is on any node but the root lifts nothing, and every
friction and hover marker of every retail vehicle is on node 0, so every
powered point's marker is (tag-definitions.md, "Every vehicle from
definitions"; `crates/tools/h1objects/tests/vehicle_rebuild_disc.rs`,
`every_powered_point_rides_the_root_node`). The hover points' positions match
the retail ghost's within 0.06. The game-run figures are in
[notes/objects.md](notes/objects.md).

**Functions and what they scale (vehicles).** Halo 1 scales an
attachment (a looping engine sound, a headlight) by one of the object's
four outputs (attachment `+0x30`: 1 to 4, A to D); output *k* is the
object's function *k*, on every Halo 1 vehicle "one" scaled by an input
and bounded. An input is the vehicle's source where it names one
(`vehi +0x31c`), else the unit's (`unit +0x198`): the warthog's
headlights the unit's driver's seat power, its taillights the vehicle's
jump, its engine the vehicle's engine hack; the pelican's three sounds
engine hack, hover and thrust. Halo 2 names it instead: the attachment's
`+0x10` is evaluated (`0x000bab40`) as `one`/`zero`, then as an import
name of the object's type handlers (`0x00108d90`: vehicle `0x000f6d10`,
unit `0x000c6b90`, object `0x000bcc20`), then as a function's export
(`obje +0x64`, `+8`); a name nothing knows is 0, and a looping sound at 0
is off. So each Halo 1 function becomes a Halo 2 function
(`h1objects::functions`): its input's Halo 2 import name (Halo 2 keeps
Halo 1's names: the retail warthog's taillights scale by `jump`), export
`a_out` to `d_out`, Halo 1's invert and always-active flags, turn-off by
the export of the function Halo 1 names, and data an identity or linear
function whose range is Halo 1's bounds. Each attachment then names its
output's export. An output that doesn't convert leaves its attachments
unscaled (1), with a note; unscaled, a rebuilt vehicle's engine would sound
a constant hum and its lights be always on (tests:
`crates/tools/h1objects/tests/object_functions_disc.rs`).

Weapons, equipment, grenade projectiles and bipeds name the **retail
Halo 2 tag that plays the same role** (`convert::TEMPLATES`, first match;
`template_candidates` lists every match in table order and the converter
takes the first the base holds): assault rifle on the SMG, pistol on the
magnum, plasma rifle, needler, grenades on Halo 2's grenades, health pack
on the overshield, cyborg on `masterchief_mp`. For campaign bases
(05a_deltaapproach): placed elites, grunts, jackals and engineers on
Halo 2's elite, grunt, jackal and drone (`bugger`), every other placed biped
(marines, Keyes, the pilot) on `masterchief`. Vehicles (the pelican, Covenant
dropship, wraith, turrets, lifepods), scenery and devices have no template.
A lifepod is Halo 1's escape pod, a small flying craft, not Halo 2's
`insertion_pod`, the ODST drop pod: the scripts fly it with recordings, load
units into it, open, close, attach and animate it, and never hand it to the
player (`lifepod_use` in h1campaign's survey, below). Template child blocks
that are not rebuilt keep their retail addresses (aliasing, as retail does
13,099 times), so the tags must go into a `TagSet` made from that base map.

Lifepod use (**S**, `lifepod_use`): a30's `lifepod_entry`
(5 placements, the intro pods) is named in `recording_play` (5),
`recording_play_and_delete` (6), `unit_enter_vehicle` (2),
`object_teleport` (16), `objects_attach` (3) and `custom_animation` (1)
calls; a10's `lifepod_docked` and `lifepod` in `objects_attach`,
`custom_animation`, `unit_open`/`unit_close` and teleports; b30's and
a30's plain `lifepod` palette entries have no placement. No call drives
one with a player.

**Model chain.** `hlmt` = the template's `hlmt` root with: render model
-> converted `mode` (`h1_assets::models`, no shaders yet), collision -> converted
`coll`, animation -> converted `jmad` (`h1_assets::models`; null when the Halo 1
object has none, as on the weapons), **physics model: the template's
`phmo`, borrowed** (warthog's for the warthog, ghost's for the ghost,
`masterchief`'s for the cyborg; Coagulation's weapons and equipment have
none). The borrowed `phmo` is dropped (null) when its rigid bodies
(`phmo +0x38`, `0x90` each, node `i16` first) name a node past the
converted model's count: the physics model writes a matrix per rigid
body's node into the object's `hlmt`-sized array (Nodes, below). That is
why humans take the Chief, whose `phmo` stays inside a Halo 1 human's 20
nodes, and not the marine (node 36); grunts, monitors, flood forms and
small devices then have no physics model. A vehicle left with none gets one built
from its Halo 1 collision (tag-definitions.md, Physics models from Halo
1), since `vehicle_new` refuses a model without one. Variants are emptied (count 0); regions are rebuilt from the
converted model's region and permutation names. `obje default_variant`
is set to 0.

**Nodes** (`hlmt +0x78`, `0x5c` each) are the converted `mode`'s, one
for one (`h1_assets::models::mode::hlmt_nodes`): name, parent, first child, next
sibling, `+0xa` zero, default translation and rotation, scale 1, the
inverse model-space basis and position. V (05a: SMG, warthog, banshee,
Master Chief): a retail `hlmt` node is its `mode` node's first `0x5c`
bytes with `+0xa` zero. E: `object_new` reserves `0x34` bytes of node
matrices per `hlmt` node, at least one (`0x000b7ef3`, `0x000b800b`,
object `+0x114`); the model, graph and physics write one per *their*
node. With the nodes emptied, every multi-node object would write past its
body into the next object in the pool, faulting on the clobbered header of
the next item (`0x0010b96e`, `mov edx,[ecx+0BCh]` with the definition
pointer null).

**`coll`** is built from scratch: materials and names as string IDs
(lower case, spaces to underscores), each region permutation holding the
BSP of every node of that region for that permutation, each BSP the same
narrowing copy as the structure's (`h1convert::collision::convert`) with
the node index in the BSP's leading word (V: on retail objects the word
is a node index in range, `0, 2, 3` on a five-node weapon; the engine's
use is not established). Pathfinding spheres and nodes copy across.
Halo 1's and Halo 2's collision BSPs hold the same tree; the difference
is the strides (Halo 1 `12, 16, 8, 8, 20, 12, 24, 16` per element of
the eight blocks, Halo 2 `8, 16, 4, 4, 16, 8, 12, 16`: 32-bit indices
narrowed to 24-bit 3D children and 16-bit flagged indices), and Halo 2's
`0x44`-byte element puts a word before the eight blocks. Proof:
`campaign_collision_converts_and_answers_as_halo1` (below).

**Collision for Havok: BSP physics.** An object with a `coll` but neither
a `phmo` nor BSP physics slots is invisible to Havok, so the player would
walk through it (a10's round oxygen tanks, barricades and computer banks are
scenery with collision). Havok moves the player's biped, vehicles and loose
items, and it sees an object through the `hlmt`'s `phmo` or, with none,
through the `coll` permutation's BSP physics slots: one `0x70`-byte slot
per BSP, a Havok shape over the BSP inside a MOPP tree whose code keys
the BSP's surfaces by index ([collision.md](../formats/collision.md), "BSP
physics"). Retail writes the slots exactly for the `coll`s of scenery
with no `phmo` (V, every map on the disc: a Delta Halo statue, trees;
not weapons, equipment, devices, bipeds or crates). So:

- a scenery object (it has no `phmo`) gets its `coll` with one slot per
  BSP: the box of the BSP's vertices at `+0x10`/`+0x20`, the `coll`'s own
  datum at `+0x30`, and MOPP code from `crates/formats/mopp` over the BSP's
  surfaces, keyed `0..n` (`coll::bsp_mopp`), at `+0x64`;
- every other object gets its `coll` without slots, as retail. A Halo 1
  `coll` that both kinds use is built twice (the second named with
  `_bsp_physics` or `_phmo`).

**Shared blocks and b40.** A BSP's eight child blocks and a slot's MOPP
code are shared by content (`Block::shared`, a hash of the bytes): Halo 1
levels carry identical collision under several names (b40's three
granite boulders), and the slots alone add 37,280 bytes on b40 (38
slots; a10 13,152 for 15), which would put b40 0x8000 over the tag memory
bound even at the last lean step. With sharing b40 converts at lean 1
(the empty squads dropped) with tag memory `0xc22000` of `0xc24000`. A shared block can lie outside its tag's payload, so
`tags::collision::Collision::read` decodes against the whole metadata
when a block is out of the tag's range (`Collision::decode_at`).

**Vehicles meet bipeds through their `phmo`.** A vehicle's wheels and
treads are in its body above the hull's lowest point where its retail
role's physics models have wheel shapes (the scorpion's treads; not the
warthog's tires; tag-definitions.md, `convert::held_wheels`), so a walking
biped cannot enter the converted treads ([notes/objects.md](notes/objects.md)).

**Scenery has no body; placed scenery is structure.** The engine gives no
scenery a Havok body, from a `phmo` or otherwise (rigid bodies: only
bipeds, vehicles, machines, crates and creatures get them,
[collision.md](../formats/collision.md)), so neither the BSP slots nor a
built `phmo` would make a scenery object solid; the player put inside
`boulder_doublewide` or `boulder_crouch` drops to the ground through it
([tag-definitions.md](../formats/tag-definitions.md), "Scenery from
definitions"). Retail Blood Gulch's base keeps its rocks in the structure
and its one solid prop as a crate with a fixed body. So scenery has no
`phmo` and its `coll` keeps its BSP physics slots, as retail scenery with
no `phmo`, and every scenery placement there from the start whose object
has a collision model has that collision moved to its place and merged into
the level's collision BSP (`h1convert::scenery_collision`,
[multiplayer.md](multiplayer.md), "Collision"), the way retail builds its
rocks into the structure; the object still draws as scenery. So Havok
meets the rocks and trees through the structure's MOPP code, and the
engine's own BSP tests (bullets, projectiles, a biped's ground checks)
through the structure's 3D tree. Which objects is decided by the `coll`,
not the name. Proof: `crates/tools/h1objects/tests/scenery_collision_disc.rs`
(inside `boulder_doublewide` is solid in converted Blood Gulch, a ray from
above stops on its top).

**`hlmt` region bytes.** Each `hlmt` region (`0x10`) says which `coll`
region it uses (`i8` at `+4`) and which physics region (`+5`); each
permutation (`8`) its `coll` permutation (`+5`) and physics permutation
(`+6`); -1 is none (V, retail; [collision.md](../formats/collision.md)). They are
set by `convert::region_map`: the `coll`
region of the same name (string ID form), else the one at the same
index, else -1; the permutation likewise, then the region's first.
Physics bytes stay 0 (retail never holds -1 there on an object with no
`phmo`). A `coll` region no render region selects is listed in
the report.

**Physics.** A rebuilt vehicle's `phmo` is built from Halo 1's collision
and `phys` mass (`h1objects::physics`; [tag-definitions.md](../formats/tag-definitions.md),
"Every vehicle from definitions"). A biped borrows its template's `phmo`,
which binds rigid bodies to Halo 2's node layout, which the converted model
does not share (open question below).

### Halo 2 offsets written

| Tag | Offset | Field | Evidence |
|---|---|---|---|
| `obje` | `04`, `08` | bounding radius, offset | E |
| `obje` | `30`, `34` | default variant, model | E |
| `weap` | `150`..`164` | heat recovery, overheated, detonation threshold, detonation fraction, heat loss per second, illumination | S, V2 (set on the plasma rifle, zero on the SMG; the overheated effect follows at `16c`) |
| `weap` | `208`..`218` | `cone_angle`, `cone_range`, `pull_angle`, `pull_range`, `bend_angle` | E |
| `weap` | `2c0` | magazines, `0x5c` | V2 (stride: magazine objects start `0x5c` after) |
| magazine | `04`, `06`, `08`, `0a`, `10`, `14` | rounds recharged, total initial, total maximum, loaded maximum, reload time, rounds reloaded | S, V2 (the loaded maximum and rounds reloaded agree on every magazine) |
| magazine | `0c` | the most rounds held in reserve: total maximum less loaded maximum | E (`0x101a10` caps the reserve with it, [skulls.md](../game/skulls.md)), V (all seven retail magazines on Coagulation). Written 0 it would leave no reserve and the HUD count 0; written, the Blood Gulch assault rifle spawns with 180 in reserve, and emptying the magazine and reloading leaves 120 (Run) |
| `weap` | `2c8` | triggers of stride `0x40`. Input and behaviour at `04/06`, primary and secondary barrels at `08/0a`, charging action at `18`, charge times, action and illumination at `1c..2c`, charging effect at `30` | V on Coagulation and Blood Gulch. See “Firing a rebuilt weapon” below. |
| `weap` | `2d0` | barrels, `0xec` | E (objects.md) |
| barrel | `04`, `0c`, `10` | rounds per second (2), acceleration, deceleration time | S, V2 |
| barrel | `28`, `2a`, `2c`, `2e` | magazine, rounds per shot, minimum rounds loaded, rounds between tracers | S, V2 |
| barrel | `38`, `3c` | error acceleration, deceleration time | S |
| barrel | `68`, `6a`, `6c` | distribution function, projectiles per shot, distribution angle | S, V2 |
| barrel | `70`, `74` | minimum error, error angle (2) | S, V2 (radians) |
| barrel | `7c`, `8c`, `a4` | first-person offset, projectile, heat per round | S, V2 (`8c` is a `proj` reference) |
| `proj` | `c0`..`e4` | detonation timer starts, impact noise, AI perception radius, collision radius, arming time, danger radius, timer (2), minimum velocity, maximum range, detonation noise | S, V2 (grenade-only fields zero on bullets) |
| `proj` | `164`..`184` | air gravity scale, air damage range, water gravity scale, water damage range, initial, final, guided angular velocity | S, V2 (bullet speeds hundreds per second; grenade gravity set) |
| `eqip` | `12c`, `130` | powerup type, powerup time | [health.md](health.md) |

Halo 1 speeds are multiplied by 30 (per tick to per second).

### Firing a rebuilt weapon

The H1 disc has 316 weapon occurrences
across 24 maps, covering 23 distinct paths. Blood Gulch's shotgun and assault rifle both
have zero H1 trigger flags. The shotgun's weapon type is 1 at H1 `weap +4e2`,
also the type the two engines check for shell reloads (H1 `0xbf70f`, H2
`0x10271d`). The rebuild carries this to H2 `+292` and emits one shot per
press for its primary barrel. H1 flag 3 selects this mode on the sniper,
rocket launcher and Scorpion cannon. A charged trigger taps barrel 0 and
discharges the last barrel, including the single-barrel fuel rods. H2
checks its charging action at `0xfe955`. The charging effect comes from
H1 trigger `+5c`.

| Blood Gulch | AR | Shotgun |
|---|---|---|
| Input and cadence | Held, 15 rounds/s | One press, one shell/s |
| Ammunition and projectiles | 1 round, 1 bullet | 1 shell, 15 pellets |
| Spread | 2°–6.5°, rises in 0.6 s, recovers in 1 s | 10° at both error bounds |
| Magazine and reload | 60 rounds, 3.4 s | 12 shells, 0.4 s per shell |
| Projectile | 324 world units/s, 40 world units maximum | 140 to 100 world units/s, air slowdown from 1.5 to 3 world units |

H2 reads minimum and maximum shots per fire at barrel `+1c/1e`, recovery
at `+20`, and multiplayer prediction at `+34` (`0x10224f`, `0x103ec7`,
`0x10429b`). Coagulation's native shotgun has one shot per fire and ten
pellets. Its SMG and shotgun use continuous and discrete prediction,
respectively. The converted barrels keep H1's magazine cost and projectile
count independently of these fields.

H2 blends two spread profiles (`0xff474`, `0x104dd1`). Both carry H1's
bounds and acceleration/decay times and their reciprocal rates. The AR's
camera shake comes from its own firing damage effect, including the 0.05 s
duration and 0.5° rotational shake. H1 spin crossover, per-shot battery
cost and age-dependent cadence, cooling and misfire fields also survive
conversion. Barrel flags for partial ammunition, muzzle origins, zoom error,
aiming direction and shared projectile direction move from H1 bits
2/5/10/11/12 to H2 bits 2/3/5/6/7. The direct barrel damage reference at
`+94` has a null datum. H2 checks for `-1` after the projectile loop at
`0x1053f6`.

**Variable-speed H1 projectiles.** H1 computes constant acceleration
from the two speeds and the current medium's distance bounds at `0xc1a60`
and `0xc1ac0`. At impact it normalizes the measured speed between those
bounds at `0xc2de1`. H2 instead interpolates speed linearly with distance.
The converter retains the native air-distance bounds and their reciprocal
for effect inputs. It marks the zero guidance scalar at `proj +198` with
the negative-zero bit pattern. H2 compares it numerically at `0xf88df`. The 725 local projectile occurrences inspected on
the H2 disc contained no such marker. At the velocity-length call returning
to `0xf966a`, the simulation integrates the H1 curve over the collision
substep, supplies its mean velocity for the sweep and its end velocity for
the next step. At the damage initializer returning to `0xfad79`, it scales
direct impact damage by impact speed and retains material attenuation.
Both guided angular velocity bounds receive H1's single value.

`weapon_rebuild_disc` compares the firing fields, firing feedback and
projectile targets against each original weapon on the campaign and
multiplayer maps. Its Blood Gulch case also checks the retail Coagulation
trigger and barrel layouts.

### Weapon class and type (`weap +0x288`, `+0x28c`, **Built**)

A unit takes a weapon, picked up or given at spawn, only when its
animation graph holds it. `0x000cd6a0` (called by `unit_add_weapon`
`0x000cd0c0` at `0x000cd12e`, and on the pickup paths at `0x000e8a63` and
`0x000ea475`) passes the weapon's class (`+0x288`) and type (`+0x28c`)
string IDs, with mode `current` (sid `0x101`) or the seat's, to
`0x0010fcd0`; that asks `0x001cc3f0` for action `idle` (sid `0xc`) and
refuses the weapon when there is none (E). Retail: the SMG `rifle`/`smg`,
the magnum `pistol`/`hp`, the rocket launcher `missile`/`rl`, vehicle guns
`fixed`/`gun` (V, Coagulation). A rebuilt weapon with both null would let
the player pick nothing up, so it never fires.

**Player weapon classes.** Halo 1 stores the type on the weapon
(`weap +0x30c`) and lists it under a standing class in the cyborg graph.
The converted weapon and graph use that same class. Blood Gulch has
`rifle/ar`, `rifle/sg`, `rifle/sr`, `rifle/pr`, `missile/rl`,
`pistol/hp`, `pistol/pp` and `pistol/ne`. The needler has weapon clips
under pistol and an empty secondary listing under rifle. The flamethrower
is listed under `support/ft`, with no source idle for that class.
Objective weapons use `rifle/f` and `pistol/b`. Guns outside the cyborg's
tables use their unit graph. The ignored sandbox disc tests read the
class/type from each converted weapon and look up its source slots.

**Holding is finding.** The graph lookup
`0x001db170` finds the mode, class, type and action each by binary
search over signed string IDs (`0x001dd560`; the type falls back to
`any`), so a graph holds a weapon only if every level of its tree is in
string-ID order. Trees in name order hide names from the search: the
cyborg's graph would find `idle` and `throw_grenade` for none of the eight
Halo 1 weapons, so it would refuse every weapon (the Halo 1 sandbox's "no
starting weapon", "can't pick up weapons" and, with nothing held, no
first-person arms or weapon); every retail graph is in order. The trees are
laid out by string ID when the map is built
([models.md](models.md), `jmad`), and the disc tests look each weapon up
as the engine does: `idle` under `combat`, its class and its type, and
`idle` in its first-person graph, and `throw_grenade` under `combat`,
its class and its type. Halo 1 has no class fallback for the throw: it
finds a unit's animations through slot tables (per unit, weapon class
and type, a list of animation indices), and on the cyborg every class's
throw slot names `stand rifle throw-grenade` (V: Blood Gulch's cyborg
`antr`, every unit and class). The converter carries those slots over:
a slot naming another class's animation becomes an action of the
borrowing class naming the same animation (`jmad::player`), so the
pistol, plasma pistol, needler and rocket launcher throw with the rifle's
animation, as in Halo 1. The disc test fails on any weapon of any
multiplayer map whose path finds no throw.

### Model materials (`hlmt +0x58`, **Built**)

`hlmt +0x58` holds the model's materials, `0x14` each. The engine maps a
hit on an object to a global material through it: `0x000d8ae0` (ecx the
object, esi an index, eax the output) goes object tag `+0x38` (the `hlmt`)
to `+0x5c` (this block's address) and reads element `esi`'s `u16` at
`+0x10`, with no bound check (E). Its one caller, `0x001695fb` in
`0x00169510`, tests a ray against an object's model and passes the hit's
material index. An empty block faults there reading `[0 + esi*0x14 +
0x10]` (pc `0x000d8b33`, address `0x10`, a few seconds into play with the
player on the health pack).

Retail (V, `crates/tools/h1objects/tests/hlmt_materials_disc.rs` on coagulation, beavercreek and
05a, 100 `hlmt`s, 197 elements):

| Offset | Field | Evidence |
|---|---|---|
| `+0` | name: the `coll`'s material `k` (every `hlmt` with a `coll`: 80 of 80 in order, never fewer elements) | V |
| `+4` | two `i16`s; `0xffff0000` most often (67 of 197) | V |
| `+8` | low `i16` `k`, high `-1` (185 of 197) or 0 | V |
| `+0xc` | the global material's name (empty on some; then `+0` names it) | V |
| `+0x10` | `u16`: the index in the globals' materials (`matg +0x150`) of the `+0xc` name, else of `+0` (197 of 197); high half 0 | V |

A `phmo`'s shapes name material indices below the count on 60 of 62
(retail's exceptions: powerhaus, pelican_rear_gun, invisible_box, the
phantom).

The converter (`rebuild::model_materials`) writes one element per
converted `coll` material, in order: the material's name, `+4`
`0xffff0000`, `+8` `k | 0xffff0000`, the Halo 2 global material of its
Halo 1 type (`arsenal::MATERIALS`) and that material's index in the base's
globals. A `phmo` the converter builds (its shapes name material 0) adds
an element for each of its materials past the `coll`'s; a borrowed retail
`phmo` keeps the template's elements past the `coll`'s, each named as its
global material. `h1objects::verify::hlmt_materials` checks the rule on a
built map; `crates/tools/h1objects/tests/sandbox_disc.rs`
(`every_hlmt_holds_its_materials_in_both_sandboxes`) and
`crates/tools/h1campaign/tests/disc.rs` (`every_campaign_level_converts`) run it on
every converted map against what the retail base already breaks.

### Cut-out model shaders (**Built**)

Halo 1 alpha-tests a model shader (`soso`) on its base map's alpha
unless its flags (`+0x28`) set bit 2, which no Blood Gulch `soso` does
(`h1convert::shad::soso_alpha_tested`). The leaf cards of the trees and
plants are such shaders, each a DXT3 base three quarters clear:

| Object | `soso` | Base |
|---|---|---|
| `tree_desert_whitebark` | `whitebark_leaves` (flags 2, two-sided) | `whitebark_leaves`, 128², 72 % clear |
| `shrub_large` | `tree_leafy_leaves` (2) | `tree_leafy_leaves`, 256² |
| `plant fern` | `plant fern` (2) | `plant fern`, 256² |
| `plant_broadleaf_short` | `broadleaf top`, `broadleaf bottom` (0) | `plant_broadleaf_leaf`, 256² |

`tree_desert_dead` has no leaves: its one shader is its trunk (V, Blood
Gulch on the disc). The whitebark's model scales its texture coordinates
by (255.9, 119.1) (`mode +0x30`); its leaf cards span one tile each, u
104.9 to 106.0, v 95.0 to 96.0, so the whole leaf image lies on each card.

These draw on
`opaque\tex_bump_alpha_test_single_pass` with the leaf in slot 2, the
base. That stem cuts out in two places (V, `h2 shader` on coagulation's
`bush_vine` and `tree_cliff_leaf`, both on it; and
`crates/tools/h1objects/tests/model_shaders_disc.rs`): its colour pass (layer 3,
`lightmap_tex_detail_alpha_test`) alpha-tests with the base at stage 0,
and its layer 10 (`shadow\shadow_generate`, whose implementations 1 and 2
write depth only, the depth the light passes then test equal against)
samples **slot 0, the bump map**, at stage 0 with `D3DTSS_ALPHAKILL` on.
Retail's alpha-tested bump maps carry the cut-out: they are P8 bump with
palette index 255 exactly where the colour map is clear
(`tree_cliff_leaf_bump`: 24,367 of its 24,396 texels with colour alpha
below 128 are index 255, none above 159). A shader with
`default_vector` there is opaque, so each card's depth covers the whole
card: the whitebark's canopies drew as flat near-black diamonds, the clear
part showing the dark colour behind the depth, not the cliff; pointing the
leaf shader's slot 0 at the leaf bitmap itself cut the canopies into the
leaf's star shape ([notes/objects.md](notes/objects.md)).

The converter binds a bump map of its own in slot 0 when a cut-out
`soso` has none (`h1_assets::bitmaps::mask::cutout_bump`, tag `<base
name>_alpha_test_bump`, built once per base): DXT5 of the base's size and
mip chain, every texel the flat normal (129, 128, 255, a fixed colour
block within a step of `default_vector`'s 128, 128, 255), its alpha 0
where the base's level is below 128 and 255 elsewhere. A binary alpha
keeps the depth cut where the colour pass cuts; the threshold is Halo 1's
half and agrees with retail's leaf bump above. Its tiling and LOD offset
are the base's. Every image of a multi-image base is cut the same way.
A base that doesn't decode fails the conversion. Proof:
`h1_assets::bitmaps` `a_cut_out_base_gives_a_flat_bump_with_its_binary_alpha`;
`crates/tools/h1objects/tests/model_shaders_disc.rs`,
`bloodgulch_leaves_cut_their_depth_out_by_the_leaf_alpha` (retail's
slots on coagulation, then every converted shader on the cut-out stem
among the five plants and trees). Not covered: an alpha-tested `senv`
(structure) shader on the same stem still binds its own P8 bump or the
default, whether or not that bump's alpha cuts.

### Change colours (`obje +0xac`, **Built**)

Halo 2 colours a player by its object's change colours, not by its
shader. `object_new` keeps `0x18` bytes per change colour at object
`+0x124` (initial and current RGB). `0x000bacc0` sets one (esi the index,
edi the RGB; bound-checked against the definition's count, both halves
written), and `0x000bad50` reads it (E). `0x0014bfc0` writes the player's
four colours into slots 0 to 3 through `0x000bacc0`. The colours come from
`0x0015f330` and `0x0007f790`, which turn appearance bytes 0 to 3
(primary, secondary, tertiary, quaternary) into RGB through the palette at
`0x001a06a0`. In a team game `0x0007f790` makes slot 0 the team's colour
(`0x0007f720`) and slot 1 the player's own primary. When that fails, and
for every object that isn't a player, `0x000be240` resets the colours
from the definition: white (`[0x468710]`) unless the slot has initial
permutations. Then it picks one by weight among those whose variant
(`+0x1c`) is 0 or the object's, and a colour between its bounds
(`0x000be40e..0x000be44b`). The `obje` block at `+0xac` (`0x10` each)
holds those permutations (`initial_permutations`, `0x20` each: `+0` weight, `+4` lower
RGB, `+0x10` upper RGB, `+0x1c` variant string id) and the functions
(`functions`) (E). So the count must reach the slots the shaders read.

The shader reads them as constants. The Chief's stem
`opaque\tex_bump_env_four_change_color` draws base x detail x 2 x the
product over `k` of `lerp(1, colour k, change map channel k)`: red slot
0, green 1, blue 2, alpha 3 (V2, its colour pass on coagulation's three
instances; the layout and their values: `h1convert::shad`,
`TEX_BUMP_ENV_FOUR_CHANGE_COLOR`).

Halo 1 (V, every object tag on the disc's Blood Gulch, a10 and the
multiplayer maps): a `soso` names its change colour source at `+0x4c` (0
none, 1 to 4 = A to D) and masks it with its multipurpose map's blue (on
the cyborg it covers exactly the white armour plates; red is the
reflection mask, green near 0). An object's change colours are at
`obje +0x164` (`0x2c` each: `+0` darken by, `+2` scale by, `+4` flags,
`+8` and `+0x14` the function's bounds, `+0x20` permutations of `0x1c`:
weight, lower, upper). The cyborg's armour, visor and first-person hands
read C, which is Halo 1's player colour (the campaign cyborg's C is the
Chief's green; the multiplayer cyborg's A and B have only functions).
The scenery, the Scorpion's darkened hull (grey 0.45), the Covenant and
doors read A; marines' faces, the flood and the crewman's flesh B; the
cyborgs, the cryo tube and the cyborg helmet C. No object reads both A
and C.

The converter (**Design**):

- A `soso` with a change colour that isn't cut out is drawn with
  `tex_bump_env_four_change_color` (the black default cube when it has no
  reflection, as retail's `elite_fp_leather` is). Its change map is the
  multipurpose blue moved into the channel of the Halo 2 slot
  (`h1_assets::bitmaps::mask::channel_into`, DXT5, tag
  `<mask>_change_color_<slot>`); every other channel is 0, so the shader
  multiplies the rest by 1. A cut-out one (the trees' leaves) stays
  without: there is no alpha-tested change-colour stem in the base.
- Slots: A and C go to 0, B to 1, D to 2
  (`h1convert::shad::halo2_change_colour_slot`). C is the player's colour
  in both games, and no object reads both A and C, so one object's slot 0
  is C when its model's shaders read C and A otherwise
  (`h1::halo2_slots`).
- Every object with a Halo 1 change colour, or whose model reads C, gets
  four elements (`rebuild::object_change_colors`): the player writer
  needs slots 0 to 3. Each holds the Halo 1 permutations as initial
  permutations (weight, lower, upper; variant 0). Devices also carry the
  darken-by and scale-by imports or converted object outputs in the second
  block. On a10 the shield station's two colour ranges follow its two
  power-derived outputs. See [the model material audit](role-audit.md).
- A device colour bound to an output whose function did not
  convert fails the conversion. One bound to an output the device has no
  function for binds Halo 2's `zero`. E (Halo 1): the object allocator
  zeroes the whole object (`0x8b8d8..0x8b8e6`); only the function loop
  writes outputs (`0x8c092`), and only for the definition's functions
  (count at `+0x158`, `0x8c0d5`); the colour update reads that zero and
  applies it (`0x8c162` scale-by, `0x8c1be` darken-by). E (Halo 2): the
  colour update looks the name up with `0xbab40`, which clears the value
  before searching, and keeps zero for a name no function exports
  (`0xbe9bc..0xbe9c8`); dropping the colour's function would stop the
  colour following it.

So the cyborg's armour plates take the player's primary colour, or the
team's in a team game. Halo 1's cyborg has one mask, so the secondary
colour has no area to show on. Halo 2 does not team-colour vehicles; a
vehicle keeps its own colour (the Scorpion's grey). Proof:
`h1_assets::bitmaps` `a_multipurpose_blue_becomes_one_change_colour_channel`,
`h1convert` `halo1_change_colours_go_to_halo2_slots` and
`the_change_colour_layout_puts_the_mask_and_the_reflection_where_the_stem_reads_them`,
`h1objects` `change_colours_go_to_halo2_slots_with_c_first_when_read`,
`rebuilt_scenery_carries_its_change_colours`, and the disc tests
`crates/tools/h1objects/tests/model_shaders_disc.rs` (`bloodgulch_cyborg_and_scorpion_take_change_colours`)
and `crates/tools/h1objects/tests/rebuild_disc.rs`.

### Weapon zoom

`arsenal::weapon` transfers Halo 1's magnification count and range
from `weap +0x3da` and `+0x3dc` to Halo 2's `+0x1fe` (`i16`) and
`+0x200` (two `f32` values). The engine checks the count at
`0x001010cd` and reads the range at `0x00101103` and `0x0010111c`.
Blood Gulch's pistol keeps one level and the sniper rifle keeps two.
An unscoped weapon has zero levels and a zero range, like the retail
magnum and shotgun on Coagulation and Zanzibar.

A zero count lets Halo 2 use the player's binocular zoom. Converted unscoped weapons set `weap +0x12c` bit 16 to skip that
fallback (E `0x000c89c1..0x000c89df`). This keeps the assault rifle
and other unscoped Halo 1 weapons at the unzoomed field of view.
Scope HUD conversion is described in [hud.md](hud.md#scope-mask).

### Ammunition counter (**Built**)

The assault rifle's first-person model shows the rounds loaded on two
digits at its back. Halo 1 and Halo 2 draw such a counter differently.

**Halo 1** (E, Halo 1's XBE; D, Blood Gulch). The digits are a
`schi` with the numeric flag (flags `+0x29` bit 7): `weapons\assault
rifle\fp\shaders\numbers`, counter limit (`+0x28`) 60, one additive map,
`numbers_plate`, whose ten images are the digits 0 to 9. The model
(`weapons\assault rifle\fp\fp`) names that shader twice in its shader
block, with permutation (`mode +0xdc` element `+0x10`) 1 on the left digit
and 0 on the right. The model draw (`0x00067ad1`..`0x00067c32`) gives a
numeric `schi` image `(n / base^p) mod base` of its first map: `base` the
bitmap's image count, `p` the entry's permutation, and
`n = floor(limit × out + 0.5)` clamped to 0..limit, `out` the object's A
out (D out when the bitmap has eight images: the rifle's `compass`, limit
8). Extra flags (`+0x60`) bit 1 takes an engine number instead
(`[0x2e34a0]`, `0x00056aa0`). The rifle's A out is its function 0, "one"
scaled by B in, and its B in (`weap +0x332`) is 2, primary ammunition: the
rounds loaded over 60. So permutation 0 is the ones and 1 the tens of the
rounds loaded.

**Halo 2** (E, V). The battle rifle's digits are two `shad`s, `tens` and
`ones`, on `opaque\illum_detail` (additive on layer 7), bitmap 0 its own
`numbers_plate` of ten images. Each animates bitmap 0's frame (and an
identity texture transform) from one property whose input is a weapon
function: `primary_ammunition_tens` or `primary_ammunition_ones`, an
identity mapped onto 0 to 10. The weapon's function handler
(`0x000ff600`..`0x000ffc57`) answers those with the tens or ones digit of
magazine 0's rounds loaded (weapon `+0x22c`) times 0.1, so the frame is
the digit (`h1convert::shad::AMMUNITION_DIGITS`, `COUNTER_TREE`).

**The conversion.** When a weapon's A out is its primary ammunition as it
is (`h1::Weapon::rounds_counter`, the first magazine's loaded maximum),
its first-person model's numeric `schi` entries whose bitmap has ten
images, whose limit is that maximum and whose permutation is 0 or 1 become
Halo 2 counter digits (`Converter::counter_shader`): one `shad` per digit,
`<schi>_ones` or `<schi>_tens`, on `illum_detail` with retail's counter
tree, drawing Halo 1's digits (`h1convert::shad::counter_shader`). The
detail is the plain grey default and colour 0 white, so the digit draws
as Halo 1's additive `schi` drew it; Halo 1's digits carry their colour.
Any other numeric `schi`, and every model of a weapon whose counters
don't count its rounds, converts as a plain additive picture: both entries
would draw one `one_add_illum` copy of the digits' first image, so the
rifle would read 00 whatever it held.

Not converted: the rifle's `compass` (D out, the compass heading over
eight images). It draws its first image.

Proof: `h1convert` unit tests
`a_counter_digit_is_halo2s_ammunition_digit_only_when_it_counts_the_rounds`
and `a_counter_digit_draws_on_illum_detail_its_frame_from_the_weapons_digit`,
`tests/shad_rebuild.rs` `ammunition_counter_instances_share_one_tree`
(retail's counters against the tree and properties written), and
`crates/tools/h1objects/tests/model_shaders_disc.rs`
`bloodgulch_assault_rifle_counter_shows_the_rounds_loaded`. The digits
changing as the rifle fires is under "Needs a game run".

### Tuning carried over

Every field in the table above is written from Halo 1 for each object:
the weapon's heat and aim assist, each magazine's counts and reload time,
each trigger's rate of fire, error, spread and projectile (as a barrel),
each projectile's radii, timer, range, gravity, damage ranges and
velocities, the health pack's powerup type and amount, and every object's
bounding sphere. The values themselves are retail data and are not
committed: `bloodgulch_sandbox_converts_and_reads_back` prints, for every
object, each field with the Halo 1 value, the value written and the
template value it replaced (`ObjectReport::tuning`). On Blood Gulch the
rows that change play most are the assault rifle's reserve ammunition and
reload time, every weapon's error angles and cone ranges, the plasma
rifle's heat, the needler's magazine and rate of fire, and every bullet's
speed (Halo 1's are slower than the Halo 2 templates').

### Blood Gulch hit registration (**Built**)

The imported cyborg has one collision region and one permutation,
`cyborg/__base`, with 16 BSPs on the same numbered nodes as the render
skeleton. In the default pose the collision runs from about −0.002 to
0.705 world units vertically on both the Halo 1 disc and converted
Blood Gulch, against a render-model top of 0.704. The pills use the source standing height 0.7, crouching
height 0.5 and radius 0.2. Coagulation's Chief has four collision
regions and standing height 0.725, crouching height 0.5 and radius
0.175. The Halo 1 cyborg's pathfinding sphere, living character shapes
and rigid bodies serve other collision queries. A weapon's model ray
uses the node BSPs (`0x001695fb`).

- **Aim assist.** The weapon cones already reach the engine
  (`weap +0x208..0x218`, Halo 2 `0x001a5137..0x001a5199`). The
  converted cyborg had an empty `hlmt +0x68`. The target search
  (`0x001a5d18`) and shot-aim lookup (`0x001a6ca3`) read this block
  and resolve each entry's model marker. The converter writes the
  cyborg's authored `body` and `head` markers with the targeting
  radii, flags, angular limits and lock distance of Coagulation's
  player Chief. The marker positions and nodes come from Halo 1.
- **Headshots.** Halo 1 uses material `+0x20` bit 0
  (`0x00095089`) with damage flag bit 1 (`0x00095092`). Halo 2
  reads `hlmt` material `+6` for the damage section and checks its
  bit 7 and the damage flag (`0x000d8d50..0x000d8d79`). Imported
  cyborg materials would have section −1; they select rebuilt body or
  head sections by the source material flag. A pistol or sniper
  round can take the native headshot path once damage reaches the body.
- **Projectile travel.** Halo 1 Blood Gulch uses projectile tags for
  its bullets, pellets, bolts, needles, shells and rockets. The
  converter takes their collision radii and ranges and converts the
  speeds from world units per tick to per second. Halo 1 computes
  drag over its authored air interval (`0x000c1ac0` and `0x000c3f0e`).
  The converter also writes that interval into the Halo 2
  distance-based speed transition (`proj +0x18c..0x194`,
  `0x000f9527..0x000f95b4`). A plasma rifle bolt starts at 50
  world units per second and reaches 25 over 20–50 world units of
  travel. The flight hook uses Halo 1's acceleration law and selects
  the authored air or water interval for the current medium.
- **Damage.** The source model gives the cyborg 75 shield and 75 body,
  and converted `jpt!` tags carry the source damage bounds and reach.
  Halo 1 indexes the damage's material scales at `jpt! +0x200`
  (`0x00094a45` and `0x0009505c`, damage descriptor `+0x3c`).
  The cyborg armour and shield scales use native damage table
  rows (`matg +0xd0`, Halo 2 `0x000d5bc0`). For example, a plasma
  rifle bolt scales shield damage by 2 and body damage by 0.5.
  The grown rows go after the pinned intermediate's metadata, outside
  the globals' bytes, so the globals are recorded as grown there
  (`map::build::PinnedImage::grown`). The fresh build's unpinning and
  its from-scratch read take those arrays as the globals' own and leave
  the replaced rows out (`tags::schema::transplant::Unpinner::add_grown`,
  `h1convert::fresh::grown`). Without that, the globals are refused
  and dropped, and every Halo 1 sandbox build and Halo 1 assets level
  fails with "root tag missing or of the wrong class".
  The edit also records the stock rows and their value arrays
  as the arrays it replaced (`map::build::GrownTag::replaced`). The
  from-scratch read counts them as described
  (`tags::repo::MapSource::add_replaced`), so the globals are taken
  exactly; without that, every fresh build fails with "globals\globals.matg
  (3072 nonzero bytes in 1 ranges no array covers)". Test:
  `a_grown_tag_unpins_with_its_new_array_and_without_its_old_one`.
  The plasma grenade's free explosion and the needler's super
  explosion list an impulse-only `jpt!` before the vitality damage.
  The damaging part goes in the projectile's native damage slot.
  The companion impulse stays in the effect and runs through
  Halo 2's `jpt!` effect-part dispatch (`0x0017a97f`).
  Attached detonation damage uses its dedicated slot.

The Halo 2 sandbox uses its native player models and weapons.
`crates/tools/h1objects/tests/hitreg_disc.rs` checks the Halo 1 sandbox after writing and
reading the map. It compares head, torso and thigh rays to the disc
collision, checks authored marker positions and character shapes,
and follows each converted weapon's projectile and impact damage.
It also checks the plasma grenade and needler's separate damage and
impulse paths. A live hit and aiming check is under "Needs a game run".

## What has no Halo 2 home here

- **Vitality:** Halo 1 `coll` body and shield vitality
  and its damage modifiers; Halo 2 keeps damage in the `hlmt`, whose
  layout is not reviewed. The template's damage info stays.
- **Unit and vehicle tuning.** The rebuilt vehicle uses H1 seat data and
  mass point places, and its Halo 2 counterpart's drive. [Current mappings](role-audit.md#native-engine-fields).
- **Weapon:** ready time, chamber time and charging (plasma pistol). The
  [HUD conversion](hud.md#weapon-huds) covers the weapon panel and
  crosshairs. Scope masks need the screen effect block.
- **Projectile:** material responses: the template's. Contrails are
  Halo 1's (above).
- **Equipment:** non-health powerup and grenade type values (template's).
- **Grenade throwing** reads the projectile from the globals' grenade
  block (`matg`), which this crate does not touch; the converted grenade
  projectiles are built but only thrown once the globals point at them.

Stopgap references (every retail effect, sound, damage effect, HUD and
`phmo` kept from a template) are listed per object in
`ObjectReport::stopgaps`.

**Campaign bases** have no multiplayer biped or powerups. Each Halo 1
tag tries every matching row in table order (`template_candidates`) and
takes the first the base holds: the biped falls back to
`objects\characters\masterchief\masterchief`, the health pack to the frag
grenade (its powerup type and time are Halo 1's;
[campaign.md](campaign.md) §9 converts a30 onto Delta Halo
this way).

## API

```rust
use h1objects::{base::Base, convert::Converter};

let mut h2 = map::Map::from_reader(base_bytes)?;          // e.g. coagulation
let base = Base::read(&mut h2)?;
let mut conv = Converter::new(&h1_map, &base);
let weap = conv.object(h1_map.find(h1map::Group::new(b"weap"), "weapons\\assault rifle\\assault rifle").unwrap())?;
let tree = conv.tree;                                      // tags + per-object reports
let mut set = map::build::TagSet::from_map(&mut h2)?;
let ids = tree.install_over(&mut set, &donor_slots)?;      // or tree.install(&mut set) on an unpinned set
```

- `h1::{Weapon, Projectile, Equipment, Unit, Collision, Physics}::read(map,
  tag)`, `h1::H1Object::read` for any object tag, `refs::scan` for any tag.
- `coll::convert_collision(&Collision, bsp_physics) -> (Converted,
  CollisionReport)`; `coll::bsp_mopp`, `coll::bsp_box` for one BSP.
- `query::{locate, ray}`: the 3D tree's answer for a point (leaf or
  solid) and the first solid point along a segment, on either game's BSP
  (`query::Tree` for `tags::collision::Bsp` and `h1map::bsp::CollisionBsp`).
- `Converter::object(tag)` converts an object (and its projectiles, model,
  collision and animations, each once) into `Converter::tree`, a `Tree` of
  `(name, h1_assets::models::Converted)` whose cross references are placeholders
  (`convert::local(k)`).
- `Tree::install(set)` adds slots (fine on a `TagSet::new`).
  `Tree::install_over(set, donors)` reuses existing slots: **a pinned base
  cannot grow its tag index** (`TagSet::build` refuses), so on a base map
  the tags must take over slots the converted map no longer uses. A
  pinned donor's bytes stay in place as unowned metadata, so blocks other
  tags share with it remain valid. The proof uses Coagulation's 86
  `scenarios\` bitmaps for its 52 tags.

## Tests

```sh
cargo xtask test -p h1objects                   # synthetic: templates, coll, tree install
H1_ISO=<halo1 iso> H2_DISC=disc/halo2.iso cargo xtask test --disc -p h1objects
```

`crates/tools/h1objects/tests/collision_disc.rs` (`H2_MAPS=` limits the Halo 2 maps,
`H1_LEVELS=` the Halo 1 levels, default a10 and b40):

- `retail_static_scenery_carries_bsp_physics`: the slot rule, each
  slot's code keying `0..surfaces` and its box holding the BSP, on every
  object of every Halo 2 map; prints the `hlmt` region bytes seen.
- `campaign_collision_converts_and_answers_as_halo1`: every `coll` on the
  level converted with BSP physics, built, read back and validated; on
  every BSP, 64 rays and 64 points through its box (a quarter wider) give
  the same answer from the read-back Halo 2 tree as from Halo 1's, and
  every surface within a centimetre of a ray's entry point is a key the
  slot's code emits there. A crate's level walk through its middle stops.
- `level_objects_select_collision_with_bsp_physics` (`H1_LEVEL=`,
  default a10): the level's objects through the converter on 05a: slots
  on scenery `coll`s, region 0 of every `hlmt` with a `coll` selects a
  collision region; prints the bytes the slots add.

`bloodgulch_sandbox_converts_and_reads_back` converts the twelve tags of
the proof set (52 Halo 2 tags with their models, collisions, animations,
projectiles and `hlmt`s), installs them over donor slots in Coagulation,
builds the map, and reads it back: every object decodes through
`tags::object::Context::resolve_object` with its `hlmt` -> `mode` (all),
`coll` (all with a Halo 1 collision model), `jmad` (cyborg, warthog,
ghost) and `phmo` (cyborg, warthog, ghost); every converted `coll` passes
`tags::collision::Collision::validate`; and every reference word pair in
the new tags (136) names a tag of that group in the built map.

`crates/tools/h1objects/tests/particles_disc.rs`: `every_pctl_part_becomes_particle_systems`:
every `pctl` effect part (201 over the conversions) becomes one system
per lasting particle state (402), on that state's `prt3`, at the part's
location, the type's count over the event, the state's lifespan and size.
`every_effect_particle_becomes_a_particle_system`:
every Halo 1 effect particle whose `part` converts (42,266 over the
campaign and multiplayer conversions) is a particle system of its event
on the converted `prt3`, at its location, its rate times 30 times the
event's duration its count, its speed, cone and size Halo 1's.
`every_particle_converts`: every Halo 1
`part` of the campaign and multiplayer maps (2,301 conversions) reads
back and writes the same, names its blend function's stem, its bitmap as
parameter and compiled bitmap, Halo 1's sequences and converted effects.
`retail_particle_shaders_compile_from_their_stems`: the compiled shader
rule on 533 retail particles.

`every_contrail_converts`: every Halo 1
`cont` of the campaign and multiplayer maps (191) converts, reads back
through the definition with Halo 1's spans at Halo 2's offsets and its
bitmap converted; every rebuilt projectile and vehicle whose Halo 1
attachments name a contrail (236 attachments) names the converted one.

## Needs a game run

- Particle systems (`pctl`): every state at once instead of in
  sequence, and still particles (no creation physics): whether explosions
  read as explosions.
- Particles: the spawn count over an event, the size factor (twice the
  radius), the packed tint, null `pmov` (no gravity or drag), the base's
  role words on the systems and emitters.
- Contrails: Halo 1's spans as Halo 2's (the flags and the root's
  zeroed `+0x3c..+0x60`), and points with source `pphy` physics.

- That the engine accepts objects whose `hlmt` has no variants
  and regions renamed from Halo 1, and whose template damage info names
  regions the converted model lacks.
- The borrowed `phmo`s against Halo 1 skeletons (node counts and names
  differ: rigid bodies may attach to the wrong nodes or none).
- That the S offsets above mean what the values suggest (heat, magazine,
  barrel and projectile fields), and the per-tick to per-second factor.
- Halo 1's tuning on Halo 2's physics: maximum range 0 on the plasma bolt.
- The `coll` BSP leading word as a node index, and Halo 1 material names
  as Halo 2 materials (the global material table will not know them).
- That Havok bodies (grenades, vehicles) bounce off scenery, and that the
  player stops at converted boulders: a scenery placement's collision is
  merged into the level's collision ("Collision for Havok"), because
  the BSP physics slots alone did not put converted scenery in the Havok
  world ([notes/objects.md](notes/objects.md)). Which object code adds a
  slots-only scenery's shapes to the world is not found (the slot shapes
  are built by `0x00246c60` from the `coll` post-process `0x002122b0`; no
  reader of slot `+0x40` or `+0x50` was found in a sweep of `0x70`-stride
  loops).
- That a converted crate stops the player.
- The rebuilt Blood Gulch shotgun and assault rifle firing in play (automatic
  input, direct-damage datum zero).
- The assault rifle's ammunition digits changing as it fires.
- A live hit and aiming check on the Halo 1 sandbox's cyborg.
- Template seats, turrets (Halo 2's warthog gunner is a separate turret
  vehicle) and animation names against the converted `jmad`.
