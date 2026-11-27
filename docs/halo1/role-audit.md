# Halo 1 sources for rebuilt object tags

The converter starts with a Halo 2 tag definition and writes the source
object and its dependencies into it. A field without a source mapping
keeps the definition default. Everything below is **Built**. The disc
tests are ignored tests that read both discs ("Running the audit"); the
page records what they check, not a pass count or a play result.

| Area | Source and evidence |
| --- | --- |
| Items and weapons | **Built**. The shared item layer carries flags, pickup order, collision sound, material effects, detonation effects, damage and delay bounds. Weapon ready and flashlight times, magazine flags, chamber time, ammunition objects and firing-effect shot ranges come from their own records. Each trigger supplies its own effect and damage references. Halo 2 caches ten inverse barrel times; the retail barrels on Coagulation use 1000 for instantaneous changes. Six times come from Halo 1. The second spread profile repeats the two Halo 1 error times. The two Halo 2 gun-climb times are instantaneous. See `arsenal.rs` and `weapon_rebuild_disc.rs`. |
| Weapon HUD | **Built.** The converter places inherited `wphi` entries into the native `nhdt` bitmap and text definitions. Primary and secondary magazine meters retain their byte transfer. Sniper readouts use the acquired target, and trigger, grenade, battery and reload warnings use the owner's thresholds and per-user clock. H1 `0xda70a`, `0xdbd15` and `0xe213a` supply the behaviours. The read-only census counted 1,497 authored entries across 360 interfaces on 23 maps. The ignored placement test checks the serialized records. See [Weapon HUDs](hud.md#weapon-huds). |
| Projectiles and damage | **Built**. Projectiles carry their flags, own impact and attached damage, and the primary damage part of the normal or super-detonation event; that event's other parts stay in its effect so each fires once. A damage tag carries radius, cutoff, damage bounds, impulse, camera shake, sound, screen flash and motor fades. Each motor has its own duration and transition; the Halo 2 response selectors share Halo 1's authored feedback. Blood Gulch's needle and plasma grenade distinguish the normal and attached damage slots. |
| Object functions and attachments | **Built**. Scenery, bipeds, vehicles, projectiles, weapons, equipment and debris resolve the source creation effect, attachment and marker, and bind primary scales to the output names of the converted object functions. The object acceleration scale moves from H1 `+0x20` to H2 `+0x14`. The ball carries 2 in both formats. The schema includes the creation-effect reference and inherited function and attachment blocks even in groups where retail leaves them empty. |
| Bipeds and models | **Built**. Camera tracks, HUD interfaces, weapon lists, integrated light, melee damage, dialogue, blood effect and material effects follow the original unit and collision references. Unit camera and acceleration parameters, landing and slope limits, jump and crouch rates, and flying motion are written from Halo 1. The crewman's angles on a30 distinguish cutoff and falloff. The monitor and sentinel on c20 and 04b distinguish flying physics. Standing and crouching use the source collision dimensions or a pair of spheres for sphere-only characters. The alternate three sphere sizes remain available to the engine. `0x001c2460` and `0x001d6dd6` select those shapes. |
| Vitality and shields | **Built**. The model's damage record uses H1 collision body and shield vitality, body and shield global material, shield stun and recharge times, the reciprocal recharge rate, and shield damaged/depleted/recharging effects. The cyborg and elite's `coll +0x198` name their depletion effects; their H2 models use `+0xac` in the damage element. The ordinary shield uses the source modifier shader. The native overshield slots and damage section array are described below. |
| Vehicles | **Built**. A Halo 1 ground vehicle or flier keeps its model, collision, seats, camera tracks, sounds and effects, and the marker, region and radius of each powered mass point. Its drive is the matching Halo 2 vehicle's: gears, engine, steering, ground and hover laws, and the tuning of its friction or hover points (`rebuild::DRIVE_COUNTERPARTS`, see "Native engine fields"). A prop a cutscene or a recorded animation moves, a scripted set piece, a dropship and a turret take no counterpart and keep Halo 1's speeds and turn limits. |
| Lights, decals and particles | **Built**. Lights use H1's radius, cone, duration and colours and a unity brightness multiplier (`0x000c0dc9`). H1 decal layers 0, 1, 2 and 3 select H2 layers 2, 4, 5 and 1. The overlap limit and optional scale use their definition defaults; the placement branches are recorded in `docs/formats/effects.md`. Particle properties use functions with neutral multipliers and the emitter's H1 values. Shader constants use identity UV scale and zero offset; the second vertex vector is the homogeneous origin. The compiled stem supplies the bindings (`0x00017000` and `0x00017170`). The single-bitmap particle instances on Coagulation supply the comparison. |

## Native engine fields

- **Built. Drive counterparts.** Each Halo 1 vehicle takes its drive from a
  Halo 2 vehicle (`rebuild::DRIVE_COUNTERPARTS`): the warthog and rocket
  warthog from the warthog, the ghost from the ghost, the scorpion from the
  scorpion and the banshee from the banshee. `rebuild::copy_drive` copies
  the counterpart's control, speed and turn envelope, steering and slide
  laws, size class, damping, engine, flight, seat-acceleration, drag and
  thrust scales, ground and anti-gravity laws (`rebuild::DRIVE_SPANS`) and
  gears. The vehicle type must match. `rebuild::copy_point_tuning` gives
  each Halo 1 powered mass point the counterpart's point tuning, its own
  when the counts match and else the one at the same fraction of the list;
  the marker, region, region index and radius stay Halo 1's, and the mass
  is shared equally when the counts differ. A counterpart with no points
  of the kind (the banshee) takes none of Halo 1's. A vehicle with no
  counterpart is a named conversion error unless it is a prop an animation
  moves (`rebuild::DRIVE_EXEMPT`), a scripted set piece, a dropship or a
  turret.
- **Built. Gear format.** At H2 `0xf1540` throttle blends two inline
  torque profiles. Each profile gives torque at rest, peak torque, the
  fraction of redline at the peak, the falloff exponent, and torques at
  redline and twice redline (`0xf15d0`). The remaining five floats are
  the upshift delay and threshold, speed ratio, and downshift delay and
  threshold (`0xf152c`, `0xf18aa`). Engine inertia divides net torque
  before integration at `0xf1c02`. The counterpart's gears supply the
  torque, inertia, redline and shift schedule. A zero redline bypasses
  native gear selection and torque integration at `0xf17b1` and `0xf1981`.
  The native steering curve has a reference angle and exponent at
  `+0x250/+0x254`. `0xeee7c` supplies one for either zero. An exponent
  of one gives a linear angle.
- **Built. Impulses.** Both extra rider impulse gains at H2 `0xe538c` come
  from the counterpart (`seat_entrance_acceleration_scale`,
  `seat_exit_acceleration_scale`).
- **Built. Vehicle priority.** H2's AI ram decision compares two vehicle
  ranks at `0x1ab48a`. The H1 driving decisions at `0x16f954` and
  `0x16fbc1` read distances from the acting vehicle. The converter gives
  every vehicle equal rank. This is a translation of the priority comparison.
  The H2 AI still chooses when to ram.
- **Built. Aircraft.** The banshee flies on the Halo 2 banshee's flier
  controller and tuning. A scripted flier (a lifepod, the fighter-bomber)
  keeps Halo 1's speeds and turn limits as the Halo 2 flier's limits, and
  H2 supplies the contact geometry and rigid-body integration.
- The seat entry test needs positive distance and angle limits (`0xc9040`).
  `+0x258` and `+0x2e4` keep their definition defaults. Neither has an
  identified read in the ground or hover drive equations.
- Biped models other than the Halo 1 player keep the native damage section array that `object_new`
  indexes at damage info `+0xbc`, and native overshield shaders at `+0xe8/+0xf0`. The source modifier supplies both ordinary shield views at `+0x7c/+0x84`. The per-model camouflage shader is
  H2-specific. Halo 1 implements camouflage through its globals.
- The optional Halo 2 side of the paired HUD, HUD drawing shaders,
  particle and material shader templates, default texture inputs, the
  scope effect's screen vertex shader and the player emblem's draw program
  and atlas are native rendering dependencies. The primary HUD and source
  bitmap slots use converted Halo 1 tags.

## Models that used the marine head shader

**Built.** A model with an H1 `soso` whose base, detail and reflection
datums are null gets its own H2 `shad`. On the owner's campaign
discs, 15 of the 21 model records that referenced
`objects\characters\marine\marine_female\shaders\head_female` on
05a_deltaapproach meet this condition. Fourteen have all four bitmaps
null. The shield charger has a multipurpose bitmap.

The H1 Xbox model setup at `0x70433..0x70561` binds these images. The
null-datum branch at `0x8036a` selects the bitmap for the texture kind
from the rasterizer globals at `0x803c3..0x8041e`. Its image records
are 0x30 bytes apart. Reading the named images on a10 gives these values.

| Slot | Reference / datum | Rasterizer bitmap | Image | RGB |
| --- | --- | --- | ---: | --- |
| Base | `+0xa4 / +0xb0` | `default 2d` | 1 | white |
| Detail | `+0xdc / +0xe8` | `default 2d` | 2 | 128, 128, 128 |
| Multipurpose | `+0xbc / +0xc8` | `default 2d` | 1 | white |
| Reflection | `+0x164 / +0x170` | `default cube map` | 0 | black |

At `0x705dc..0x7064a` the XBE interpolates the RGB at `+0x78`
and `+0x84` using the waveform at `+0x72` and time divided by the
period at `+0x74`. Bit 0 at `+0x6c` synchronizes the phase. Otherwise
the draw uses an object-seeded phase. Function 0 returns one
(`0x173f20`). All affected shaders either select a constant endpoint
or have identical endpoints, so their emission is independent of time
and phase. In particular, the rocks have a nonconstant waveform but
both endpoints are black. A custom map with two different endpoints
and a varying wave returns a conversion error.

H1 multiplies the resulting emission by the object colour at `+0x70`
and independently selects diffuse object colour at `+0x4c`
(`0x7064f..0x7068e`). In the model combiner at `0x6f4df..0x6f58f`,
multipurpose green masks the addition of emission to the lit surface.
Blue interpolates from white to the diffuse object colour and multiplies
the saturated result. The null cube contributes black. The charger uses
green from its actual `levels\a10\bitmaps\light` bitmap.

`h1convert::shad::program::model_defaults` builds the instance, template
and additional passes through the H2 definitions. The opaque lighting
inputs are a white base, flat normal and neutral detail. H2 scene layer
7 adds emission. Layer 2 multiplies the destination after the dynamic
lights (`0x2c034..0x2c05b`). The native four-change-colour pass supplies
the object vector selectors. The alert light emits fixed red, the door
lock uses object colour A for emission and diffuse, and the signal light
uses A for emission and B for diffuse. The waterfall emitter and rocks
require only the white lit surface.

Each level in a row below contains a separate affected `mode` tag. Slots
are zero-based. The inventory compares the material datums of all ten
converted campaign maps against the shader references in
the matching H1 disc maps.

| Levels | Render model tag | Slot | Source shader |
| --- | --- | ---: | --- |
| a10, d40 | `levels\a10\devices\alert_light\alert_light` | 1 | `soso light siren` |
| a10, d40 | `levels\a10\devices\doors\door blast closed\door blast closed` | 2 | `soso obj-light lock` |
| a10, d40 | `levels\a10\devices\doors\door blast half\door blast half` | 2 | `soso obj-light lock` |
| a10, d40 | `levels\a10\devices\doors\door blast\door blast` | 6 | `soso obj-light lock` |
| a10, d40 | `levels\a10\devices\doors\door jeff tube\door jeff tube` | 3 | `soso obj-light lock` |
| a10 | `levels\a10\devices\shield charge\shield charge` | 4 | `soso obj-shield charge` |
| d40 | `levels\a10\devices\signal light\signal light` | 0 | `soso light signal` |
| a30 | `scenery\waterfalls\waterfall_emitter\waterfall_emitter` | 0 | `soso invisible` |
| b30, d20 | `scenery\debris\rocks_smoldering\rocks_smoldering` | 0 | `soso rocks_smoldering` |
| a50, d20 | `levels\a50\devices\small door\small door` | 1 | `schi door lock symbols` |
| c10, c20, c40, d40 | `characters\sentinel\sentinel` | 1 | `spla sentinel_shield` |

**a10 shield charging station.** The machine has two luminous materials. Slot 4
(`obj-shield charge`, above) uses the green channel of its 16×16 DXT1
`light` bitmap. The illumination waveform is constant one and selects the
upper RGB (255, 255, 183). Its UV functions are constant, and neither
the diffuse nor the emission term selects an object colour.

Slot 5 is `shield charge force`, an additive generic transparent shader.
Its four maps all reference the same 128×128 DXT3 bitmap. For each draw
the H1 XBE at `0x56ff0..0x57057` adds a phase in seconds before dividing
by the signed period. The entries below give the offsets in texture repeats.

| Sample | U offset | V offset |
| --- | --- | --- |
| 0 | fixed | (t + 1) / 3 |
| 1 | fixed | (t + 1) / −4.78 |
| 2 | t / −0.5 | (t + 1) / 3 |
| 3 | t / −1 | (t + 1) / −4.78 |

The stage decoder (`0x62d14`, input table `0x1e0b68`) builds the first
sum from samples 0 and 1 weighted by alpha from samples 2 and 3. Stage
1 adds the product of samples 0 and 1. The next stages multiply the RGB
by object change colours A and B (`0x6796e..0x679d2`). The model vertex
program at `0x1e5ac0` uses the absolute normal–camera-forward dot product
and fog visibility. The final combiner scales RGB by that interpolated
factor. The draw adds RGB on both faces, with depth writes disabled.

The H2 rebuild defines its own vertex program, four-stage pixel program,
pass, template and instance. Each texture stage gets its own UV pair and
the original bitmap. The H2 scroll is continuous between repeats. H1
interpolates byte-valued slide samples (`0x173f20`), so the intermediate
coordinates can differ slightly. Fog visibility and device power follow
H2's current scene and machine state.

Both colour inputs come from the machine. H1 output A is power times ten,
clamped to one. Output B switches on strictly above half power. Output C
is power itself, used by audio. The machine interpolates black to white
by A and white to (1, 0.443137, 0.443137) by B. H1 applies the threshold
and gain at `0x8bea4` and `0x8bfad`, and updates colours at `0x8c142`.
H2 reads the corresponding named colour functions at `0xbe9a7` and
`0xbea15`. The disc regression
`a10_shield_charger_keeps_its_lamp_four_scrolls_and_power_colours`
follows the two material slots, texture payloads, scrolls and power curves.

**Built, small door.** Both caches name the same `schi` in material 1.
Neither uses `scex`. The block at `+0x54` has one 0xdc-byte stage. Its
colour and alpha functions both select the current map (0), its scale is
(1, 1), its U, V and rotation functions are constant (0), and its bitmap
datum is null. There are no extra layers. The framebuffer blend is
additive (3). At `0x67c43..0x67cd8` the draw requests a 2D texture and
image zero. The null branch at `0x803c3` reads the rasterizer bitmap at
`+0xb8`. In both maps that is `globals\globals` block `+0x134`, reference
`+0xac`, naming `rasterizer\default 2d`. Image zero is 4×4 opaque black.
The instance binds that converted H1 bitmap to H2's additive
`one_add_illum` template. The stage adds zero RGB and leaves depth intact. The adjacent floor-symbol
material in d20 has four source images. Its cutout bump preserves the
alpha mask for every image and mip.

**Built, Sentinel.** All four caches bind the same H1 bitmap to both noise
inputs at `+0xd4` and `+0x11c`. The image is a swizzled 32×32×32 A8Y8
volume with five lower mip levels. H1 binds both stages with linear
filtering and U/V/W wrap at `0x6d546..0x6d628`. The following rows give
the transform of the position before each lookup. Time is in seconds.

| Stage | Scale | Translation |
| --- | ---: | --- |
| 0 | 2 | (2, 0, −0.3) × time / 2.1 |
| 1 | 1 | (0, 2, 0.5) × time / 3.2 |

The XBE writes the rows at `0x6d73f..0x6d867`. The H2 instance writes
their XYZ coefficients and animates the W components at the same rates,
modulo one texture repeat. The H1 combiner at `0x6d8ed..0x6d99f`
takes two differences biased by one half, Y0−Y1 and A1−A0. The first
selects which difference to square and multiply by four. Calling that
saturated result `s`, the bright core is `max(2s−1, 0)²`. The output RGB
is `s² × saturate(s × tint + core/2)`. Source alpha is fog visibility
times the angle-interpolated brightness times object output A. H1 sets
source-alpha/one blend, RGB writes and read-only depth at `0x6d633..0x6d6e5`.

The authored perpendicular RGBA is approximately (0.17647, 1, 0.98039, 1)
and the parallel RGBA is (0.06275, 0, 0.31765, 1). At `0x6d867..0x6d8da`
the XBE uploads perpendicular minus parallel in c[−83] and parallel
in c[−82]. Instructions 37–38 of the plasma vertex program compute
`c[−83] + dot(normal, eye) × c[−82]`. The effective facing endpoint
is perpendicular. The effective grazing endpoint is approximately
(0.11373, 1, 0.66274, **0**). H2's two vertex attributes receive those
effective endpoints. Displacement is zero.
The Sentinel's `shield glow source` maps recent shield damage through
a fourth root (H1 `0x8bf9e`, table 2 at `0x173c65`). The converted biped
exports the same curve as `a_out` from H2's `current_shield_damage`.

The layer-15 `stem`, `spas` and `shad` are authored from the definitions.
H2's retail plasma shield on 05a_deltaapproach establishes the two native
3D stages and `transparent_generic_offset` vertex program. The rebuilt
pass uses the H1 noise channels, colour equation and blend. The H2 vertex
program supplies the model vertex format, skinning, view direction and fog.
H1 projects the volume coordinates from the input position before skinning.
H2's rigid and skinned model variants project the posed position. Animation
can therefore move the noise over a deforming surface. H2 also clamps
negative normal–eye dot products to zero. The H1 plasma vertex
program at `0x1e2c70` normalizes the normal and uses the signed dot before
output clamping. Back-facing normals can therefore produce different tints.
Fog and the underlying recent-damage value follow the H2 engine.

**Built, conversion errors.** Model materials require a rebuilt shader.
An unsupported source fails with its map, group and tag path. A missing
datum identifies the referring object, material slot and datum. The
object tree keeps its first material error so a caller that collects
placement errors cannot install an incomplete map. The campaign lean
ladder keeps model materials at every step. The audit accepts the native
player emblem added by the converter only in an appended slot. Each original slot must name its
converted H1 shader or a permutation-specific numeric shader in the same tree.

## Retail fallback inventory

The survey read the ten campaign and thirteen multiplayer maps on the owner's
Halo 1 Xbox disc. It also read the tag tables, structure palettes and sky
models on 05a_deltaapproach, Coagulation and Beaver Creek on the Halo 2
disc. Counts below are distinct map, parent and source-use combinations.
A tag being present on the disc establishes a candidate, not a runtime draw.
The survey has 35,510 rows. Each gives the map, group, source path, parent or BSP index, destination base, the fallback candidate or disposition and code site.

| Fallback and code site | Retail source and the fallback target | Handling |
| --- | --- | --- |
| BSP render shaders, `textures::plan_bsp` | 1,981 BSP/shader rows. A word matcher would select the closest base shader or the first opaque one. | **Built.** Material records name the rebuilt source shader. Its recipe must be available and its source images must convert. The illumination-only `levels\a10\shaders\monitor glow` on a10, d40 and putput uses its own illumination texture at `+0x254` and UV scale at `+0x250`. |
| BSP collision, `textures::Retexture::install` and `Structure::write` | 1,295 BSP/shader/material rows. Delta Approach would supply `tower_metal_dirty`, `tower_metal_trim_g` or `shaders\invalid`. Coagulation would supply `coag_rocks` and Beaver Creek `bc_forerunner_lightgray`. | **Built.** Each source palette entry retains its shader and maps its substance at `+0x22` to the destination globals material index. Collision-only shaders enter the conversion graph. Merged scenery surfaces use their object's source substance. H1 object collision materials have no shader reference. |
| Model materials, `Converter::model_shader` | The first eligible opaque object shader would be `objects\characters\marine\marine_female\shaders\head_female` on Delta Approach and `objects\multi\2way_teleporter\shaders\2way_teleporter` on both multiplayer bases. Seven distinct diffuse-null `soso` paths account for 17 map/model/shader rows. | **Built.** A missing recipe, map or program reports the source shader and map. The null-base, Chicago and volume plasma programs are built ("Models that used the marine head shader" above). |
| Sky materials, `Sky::from_map/install` | 135 sky/material rows. Delta Approach would use `deltatemple\shaders\ring_afternoon`, both multiplayer bases would use `coagulation\shaders\coag_ring`. | **Built.** Source material references and bitmaps are required. Missing recipes and flares fail conversion. The scenario records the actual number of source skies. |
| Bitmap, sound and flare reuse, `TagSet::converted` | Six `bitm` paths collide on every map, including `rasterizer\glow` and five `ui\hud\bitmaps` images. No `snd!`, `lsnd`, `snde` or `lens` namesakes were found on these three bases. | **Built.** Reuse requires an installed conversion with the same group, name and live datum. The existing slot may receive rebuilt data. The name alone cannot certify its contents. Loop tracks, details, sound environments and authored sounds propagate conversion failures. |
| Objects, units, campaign placements and AI, `Converter::model`, `roster::prepare/install`, `ai_control_characters::resolve` | An object could otherwise inherit a native unit/model root or species character. Examples include H2 Marine, Grunt and Elite characters. The disc has 265 map/unit rows and 217 map/actor-palette entries. Nine additional map/variant pairs occur in explicit script references. | **Built.** The H1 sandbox builds the object and model definitions, actor variant unit, weapon and major variant links. A vehicle lacking a solid hull uses its own mass points or fails. The H2 sandbox can explicitly select an H2 asset. |
| Script atoms and cinematic animation, `h1campaign::scenario` and `cutscenes` | 910 explicit map/reference pairs across eleven source groups, including sounds, loops, animation graphs, effects, damage, actor variants and five object groups. A path could otherwise resolve to a native namesake or disappear. | **Built.** The scenario resolves typed converted datums and explicit H2 asset aliases. Script-only effect, damage and object roots are rebuilt. First- and third-person animation sound events both resolve their authored sounds. |
| Effects, projectiles and devices, `effect_uncached`, `arsenal::responses`, `device` | 10,102 effect-part rows, including 338 damage rows. 7,115 projectile material/effect rows and 137 device transitions, of which 127 are effects and ten are sounds. | **Built.** Every event keeps its original damage parts, except that a projectile's detonation effect hands its primary damage part to the projectile. H2 dispatches damage directly at `0x17a8a0`. Each material response resolves its normal, potential and detonation effects. Device transitions use the source reference's group. Cyclic dependencies resolve to the eventual converted datum. |
| Source widgets, `Converter::native_widgets` and `object_attachments` | 27 retail paths in 262 owner/map rows, plus the a10 beam attachment. | **Built.** The converter binds antennas, light volumes, lightning, cloth and glow to their owners. The ignored disc test checks each binding through the public owner converter. See [objects.md](objects.md#source-widgets) for native dispatch, physics and renderer evidence. |
| Contrails, `Converter::contrail` | 265 map/contrail/physics rows would emit a null `pphy` without it. Cold smoke, steam, vacuum and the Warthog antenna have corresponding point-physics records with differing coefficients on the two discs. | **Built.** The typed H2 definition receives source flags, mass, air/liquid buoyancy, density, drag, friction and restitution. Each contrail point names its own converted physics tag. |
| Multiplayer starting equipment, `halo1::plan_starting_equipment` | Both H2 bases carry one `multiplayer\\single_weapons\\smg` collection. Blood Gulch's CTF row instead names the assault rifle and pistol. Wizard has three distinct game-type rows. | **Built.** The H1 sandbox rebuilds its source rows, selector words, collections, weapons and weights. A native variant selector with no H1 weapon or vehicle is left unassigned. |
| Ordinary biped shields, `biped_rebuilt` | 55 biped/modifier rows name three `spla` tags. The H2 Chief uses `masterchief_plasma_shield_change_color_fp` and `masterchief_plasma_shield_change_color`; the Elite uses `elite_plasma_shield_fp` and `elite_plasma_shield`. | **Built.** Both ordinary shield views bind the source modifier through the model-material converter. All three source shaders use the Elite shield noise volume twice. The volume-plasma rebuild above draws them. |

### Sky clouds and unit voices

- **Built.** The two-input sky pass uses the retail cloud equation on bases
  without the stem. Its mask and tiled texture are separate authored
  inputs. Coagulation and Beaver Creek establish the pass, and the
  05a_deltaapproach vertex shader supplies the same two transforms.
  See [sky.md](sky.md#the-models-shaders).
- **Built.** One unit can select among all of its authored voices. Halo 1
  `0x468d0` defines selector matching and alternative selection. Halo 2
  `0x114ec0` binds a model variant's dialogue to the individual unit.
  The rebuilt table uses the native vocalisation indices, and explicit
  vocalize commands resolve source slots on that unit.
  See [ai.md](ai.md#command-execution).
- **Built.** Every authored `udlg` slot has an automatic event. The
  authored biped voice inventory across all 23 Xbox retail maps has zero
  unbound rows. Source-only automatic lines use Halo 2's native
  notifications or boot hooks for their original conditions. Three
  authored slots that the Halo 1 retail scheduler never selects remain
  available by their exact source index. A slot with no rule fails
  conversion naming the sound, voice and unit.

### Sounds, fixtures and the emblem

**Built. Script sounds and player emblem.** Every named script sound is
installed without a converter count budget. The runtime datum lookup in Halo 1
(`0xf2e53`, `0x2b9f6`) and the 570 authored map/sound pairs are documented
in [campaign.md](campaign.md#script-sound-capacity). The converter builds a
player-emblem instance from the Halo 2 `shad` definition. Halo 1's part
shader selection (`0xa2449..0xa246d`) uses the model's authored slots, and
the cyborg lacks an emblem slot. The appended part uses Halo 2's named
emblem inputs at `0x22ace7`. See [models.md](models.md#the-emblem-built).

**Built. Impulse promotion and fixture lighting.** Halo 1's `0x24ea0`
compares decaying mixer credit against the authored count and duration.
The converted sound appends that rule and references the rebuilt target
by datum. The boot hook routes the impulse or applies the source rate
limit. The fixture constructor at `0x118e00` copies the placement's RGB
and intensity. Campaign and multiplayer retain the fixture machines and
marker lights. Each of b30's 42 authored placements binds a light with
that tuple. See [sounds.md](sounds.md#impulse-promotion-built) and
[objects.md](objects.md#light-fixture-placements-built).

**Built. Native donor promotions.** Halo 2's `0x218f10` returns the
rule's pitch range within the donor sound. `0x219070` adds its relocated
first range. Donor transplantation and gestalt compaction share the merge
that carries those rules and timers with the rebuilt range order. Invalid
rules and incompatible reuse identify the source sound and map.
See [sounds.md](sounds.md#merging-a-moved-sound-h1_assetssoundsmerge_sound).

### Dependencies kept from Halo 2

The base map supplies engine globals, global material indices, the sound
gestalt and native scenario infrastructure. The H1 conversion writes source
entries into these containers. Material and HUD rendering require native
shader programs, vertex shaders and default inputs. The H1 Xbox engine has
no equivalent tags for those programs. The HUD's optional H2 view and the
multiplayer player emblem are explicit features. The generic AI behaviour
parent and Sentinel behaviour style supply H2 engine behaviour. H1 actor
variants provide the concrete character. H2 asset mode retains its selected
native units, weapons and vehicles, including assets from donor maps.

`h1map` and the `h1-assets` modules `bitmaps`, `models` and `sky` read
source data or write caller-supplied datums. `h1_assets::text` extends the
destination locale table to keep IDs already referenced by engine tags.
The HUD converter lives in `h1objects::h1hud`. It builds its primary
`nhdt` and binds destination drawing programs. `h1campaign::scripts` emits
relocation requests. Campaign resolves
those against the selected sandbox before installing the scenario.

### Rebuilt surface and particle draws

- **Built. Textureless environment and glass.** On d40, `burnt flourescent off` is an opaque, lightmapped `senv`. H1 selects white for the absent base and neutral grey for detail at `0x774a2` and `0x77500`. The H2 opaque program uses its white, neutral detail and flat normal bitmaps. On a50 and d20, `covenant glass floor` is an `sgla` with a white background tint and zero reflection. H1 draws it with ZERO/SRC_COLOR/ADD at `0x68383..0x683db`. The rebuilt layer-15 pass multiplies destination RGB by white and preserves depth. Its only native program dependency is the H2 transparent mesh vertex declaration and transform.
- **Built. Maximum-blend particles.** H1 `part +0xda = 6` selects ONE/ONE/MAX at `0x7f4e0`. The tables start at `0x1e9c0c`, `0x1e9c30` and `0x1e9c54`. At `0x66e45..0x67060` the source RGB is the sprite times vertex tint, optional secondary-map RGB, particle opacity and fog visibility. Only RGB is written. Across 47 source map/tag pairs, glass breakable uses the sprite UVs for its moving mask, glass death uses clip XY/W and glowingdrip goo has one sampler. H1 clamps both goo sequence phases to the bitmap’s only sprite at `0x10e217`. The authored vertex programs use H2 billboard declarations 7 and 8. Glass breakable aligns to velocity as in H1 `0x59291`. The H1 mirror flags at `0x10e9f9` and `0x10ea10` select the corresponding H2 random U/V mirrors. The single-sampler variant uses H2's existing particle vertex program for sprite placement, tint and fog. Mask motion follows H1 `0x56ff0` through the H2 periodic property evaluator.
- **Built. Premultiplied ice particles.** All 23 retail maps contain `effects\particles\solid\ice`. The template `particle_alpha_multiply_add` is absent from Beaver Creek (11 source maps) and Coagulation (2). H1 XBE blend table entry 7 at `0x1e9c0c`, `0x1e9c30` and `0x1e9c54` selects ONE, ONE_MINUS_SRC_ALPHA and ADD. Its pixel-program branch at `0x67039` scales both RGB and alpha by particle opacity and fog visibility. The rebuilt pass samples the source sprite and moving screen-space mask on campaign and multiplayer. It adds source RGB to destination RGB scaled by one minus source alpha, preserving destination alpha.
- **Built. Model bitmap permutations.** At `0xa2449..0xa246d` the geometry part selects its model shader entry, including the signed permutation at entry `+0x10`. A nonzero object texture override replaces it at `0xa25d1..0xa25ef`. The bitmap binder at `0x8038b` takes the remainder separately for each sampler. On b30 the five wall and ceiling light fixtures draw image zero and both floor fixtures draw image one of the same frame material. Each derived base includes the red reflection mask from the corresponding multipurpose image.

The offline draw survey covered all ten campaign and thirteen multiplayer maps. The 2,301 particle tags contain 47 maximum-blend draws and 23 premultiplied draws. The 629 states in 199 particle systems contain neither blend. Glass breakable uses sprite coordinates with waveform 2 and a four-second period. Glass death and ice use screen coordinates with waveform 6 and a one-second period. Each has sprite sequences 0–3. The single unmasked goo uses sequence 0. All 12 fully textureless environment or glass shaders match the rebuilt layouts. Of 3,618 model material entries referencing environment or model shaders, 92 select multi-image bases across seven materials. All selected images satisfy the draw guards.

### Retail gaps that stop conversion

No retail gap is open: the sky cloud and `udlg` rows
([Sky clouds and unit voices](#sky-clouds-and-unit-voices)), the
textureless and glass draws, maximum-blend particles and source widgets
above are all built.

The ignored `both_sandboxes_rebuild_every_retail_map` test attempts all 46
conversions, collects individual map/sandbox errors and asserts that the
failure list is empty. It also checks that the disc contained ten campaign
and thirteen multiplayer maps. This is an integration acceptance test.
The disc survey above reads input data and does not establish a pass.

## Running the audit

`role_audit_disc.rs` walks built records and their references. A value
taken from the base is recorded by field path and reported when it is
still present in the output. Unclassified entries fail the test.
`H1_MAPS=multiplayer` selects the multiplayer maps, `campaign` selects
all ten campaign levels from a10 to d40, and `all` selects both.
Comma-separated names select individual maps. The campaign scan includes
all supported object roots and their rebuilt dependencies on
05a_deltaapproach, including objects reachable only by script.
`ROLE_AUDIT_OUT` optionally names a Markdown output file.

The test is `role_audit_disc` in `crates/tools/h1objects/tests/it.rs`,
with `H1_ISO` and `H2_DISC` set to the two discs and `H1_MAPS=all`.

The shader regressions cover the combiner equations, named conversion errors,
all 21 affected material slots, and the a10 charger's scrolls and power
colours: the `weapon_counter` and `model_shaders_disc` modules of the same
`it` test binary (`model_shaders_disc` is ignored and needs the discs), and
the `h1-assets` and `h1convert` library tests. Run the library tests with
`cargo xtask test -p h1-assets -p h1convert -p h1objects`.
