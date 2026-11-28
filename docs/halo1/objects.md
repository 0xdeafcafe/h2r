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
did not convert (H1 `0x8c050..0x8c06b`).

An attachment whose
primary or secondary scale names an output with no converted function,
or with no function at all, fails the conversion with the attachment,
marker, output and the function's reason (`functions::primary_scale`):
an empty Halo 2 scale would run the attachment at one where Halo 1
scales it by that output.

An input with no Halo 2 import names its
object type and Halo 1 source (`input 3 (unit source 7) has no Halo 2
import`, `functions::input_source`). A device change colour bound to
such an output fails the same way, with the function's reason.

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

Moved to [Halo 1 objects: building Halo 2 tags](objects-build.md#building-halo-2-tags-design).

### Firing a rebuilt weapon

Moved to [objects-build.md](objects-build.md#firing-a-rebuilt-weapon).

### Weapon class and type

Moved to [objects-build-weapons.md](objects-build-weapons.md#weapon-class-and-type-weap-0x288-0x28c-built).

### Change colours

Moved to [objects-build-weapons.md](objects-build-weapons.md#change-colours-obje-0xac-built).

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

Moved to [Halo 1 objects: tests](objects-tests.md#tests).

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
