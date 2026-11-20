# Halo 1 sources for rebuilt object tags

The converter starts with a Halo 2 tag definition and writes the source
object and its dependencies into it. A field without a source mapping
keeps the definition default. The following changes are **Built** in
the full-rebuild lane. The disc tests are written for the coordinator's
integration run. There is no new pass count or play result yet.

| Area | Source and evidence |
| --- | --- |
| Items and weapons | **Built**. The shared item layer carries flags, pickup order, collision sound, material effects, detonation effects, damage and delay bounds. Weapon ready and flashlight times, magazine flags, chamber time, ammunition objects and firing-effect shot ranges come from their own records. Each trigger supplies its own effect and damage references. Halo 2 caches ten inverse barrel times; the retail barrels on Coagulation use 1000 for instantaneous changes. Six of the times have Halo 1 fields. The remaining four are instantaneous. See `arsenal.rs` and `weapon_rebuild_disc.rs`. |
| Projectiles and damage | **Built**. Projectiles carry their flags, own impact and attached damage, and damage from the relevant normal or super-detonation event's parts. A damage tag carries radius, cutoff, damage bounds, impulse, camera shake, sound, screen flash and motor fades. Each motor has its own duration and transition; the Halo 2 response selectors share Halo 1's authored feedback. Blood Gulch's needle and plasma grenade distinguish the normal and attached damage slots. |
| Object functions and attachments | **Built**. Scenery, bipeds, vehicles, projectiles, weapons, equipment and debris resolve the source creation effect, attachment and marker, and bind primary scales to the output names of the converted object functions. The object acceleration scale moves from H1 `+0x20` to H2 `+0x14`. The ball carries 2 in both formats. The schema includes the creation-effect reference and inherited function and attachment blocks even in groups where retail leaves them empty. |
| Bipeds and models | **Built**. Camera tracks, HUD interfaces, weapon lists, integrated light, melee damage, dialogue, blood effect and material effects follow the original unit and collision references. Unit camera and acceleration parameters, landing and slope limits, jump and crouch rates, and flying motion are written from Halo 1. The crewman's angles on a30 distinguish cutoff and falloff. The monitor and sentinel on c20 and 04b distinguish flying physics. Standing and crouching use the source collision dimensions or a pair of spheres for sphere-only characters. The alternate three sphere sizes remain available to the engine. `0x001c2460` and `0x001d6dd6` select those shapes. |
| Vitality and shields | **Built**. The model's damage record uses H1 collision body and shield vitality, body and shield global material, shield stun and recharge times, the reciprocal recharge rate, and shield damaged/depleted/recharging effects. The cyborg and elite's `coll +0x198` name their depletion effects; their H2 models use `+0xac` in the damage element. First- and third-person shield shaders and the native damage section array are the exceptions below. |
| Vehicles | **Built**. Speeds scale by 30 and accelerations by 900 when converting per-tick cache values. Seat flags, camera/pitch/yaw limits and tracks, vehicle sounds, material effects and antigravity effects come from H1. Powered mass points supply lift strength, offset, height and damping, wheel radii and marker positions. The friction points and their phantom spheres use the same point order. The Warthog's four tires and the Scorpion's treads show the two layouts. The loader at `0x00205550` constructs the Havok headers and pointers from the number of shapes at `+0x38` and the inline arrays at `+0x60` (stride `0x90`, eight spheres each). `0x002067dd` pairs each friction point with the same-numbered contact. |
| Lights, decals and particles | **Built**. Lights use H1's radius, cone, duration and colours and a unity brightness multiplier (`0x000c0dc9`). H1 decal layers 0, 1, 2 and 3 select H2 layers 2, 4, 5 and 1. The overlap limit and optional scale use their definition defaults; the placement branches are recorded in `docs/formats/effects.md`. Particle properties use functions with neutral multipliers and the emitter's H1 values. Shader constants use identity UV scale and zero offset; the second vertex vector is the homogeneous origin. The compiled stem supplies the bindings (`0x00017000` and `0x00017170`). The single-bitmap particle instances on Coagulation supply the comparison. |

## Native engine fields

- **Built. Ground drive.** Halo 1 advances contact speed directly at
  `0x39785` and `0x80c10`. The two Warthog groups receive the same speed
  and opposite steering angles at `0x3925c`. The Scorpion gets forward
  speed minus and plus turn speed at `0x39301` and `0x39325`.
  The converted definitions select that velocity controller in the engine.
  Its two direction records have speed ratios −1 and +1. The solver
  receives the track assigned by the source powered group for every contact.
- **Built. Gear format.** At H2 `0xf1540` throttle blends two inline
  torque profiles. Each profile gives torque at rest, peak torque, the
  fraction of redline at the peak, the falloff exponent, and torques at
  redline and twice redline (`0xf15d0`). The remaining five floats are
  the upshift delay and threshold, speed ratio, and downshift delay and
  threshold (`0xf152c`, `0xf18aa`). Engine inertia divides net torque
  before integration at `0xf1c02`. Halo 1's velocity law supplies no unique
  torque, inertia, redline, or shift schedule. A zero redline bypasses
  native gear selection and torque integration at `0xf17b1` and `0xf1981`.
  The native steering curve has a reference angle and exponent at
  `+0x250/+0x254`. `0xeee7c` supplies one for either zero. An exponent
  of one gives a linear angle. The ground controller applies H1's signed
  steering limits and slew directly at the contacts.
- **Built. Ghost and contacts.** The H1 hover drive uses one horizontal
  speed cap and one acceleration magnitude at `0x38152..0x38232`. The
  Ghost gets 6.75 world units/s and 4.5 units/s² on both axes and signs.
  H1 `0x84376..0x84485` and H2 `0x20631d..0x20656e` give the lift
  height, point radius, damping and two slope cosines. H1 fixed lift
  height gives zero H2 banking-height modulation. The ground spring on
  both engines uses gravity times penetration divided by depth, minus
  damping times normal velocity (`0x83df4`, `0x2069b6`). Converting the
  velocities and force units multiplies the damping coefficient by 30.
- The remaining native root values are the vehicle priority compared at
  `0x1ab48a` (`+0x244`), the unit input activation rate at `0xf045b`
  (`+0x25c`), the two impulse gains selected at `0xe538c`
  (`+0x274/+0x278`), and the contact friction coefficients
  (`+0x2b0/+0x2bc`). H2 switches tire friction laws at a per-point slip
  threshold, using a square-root coefficient and normal load at
  `0x207017` and `0x2070e8`. H1 applies longitudinal and transverse drag
  at `0x80f80` and `0x83e30`. The speed ramps do not determine those
  gains or the native tire slip and material response. For example, the
  Coagulation Warthog's first tire has gain 14 at `+0x08` and a slip
  threshold of 7 at `+0x18`. The Scorpion's are 40 and 200. Blood
  Gulch's Warthog instead has two directional drag factors per point,
  0.45/0.65 at the rear and 0.75/0.45 at the front. The Scorpion has
  1/1 on its tracks. Aircraft retain
  their subtype and flight gains (`+0x218/+0x270/+0x27c/+0x280`).
  The dispatch table at `0xf069c` selects their controllers, which use
  those terms for flight at `0xf23b9`, `0xf2875`, `0xf2aab` and
  `0xf5a89`. The seat entry test needs positive distance and angle
  limits (`0xc9040`). `+0x258` and `+0x2e4` keep their definition defaults.
  Neither has an identified read in the ground or hover drive equations.
- Biped models keep the native damage section array that `object_new`
  indexes at damage info `+0xbc`, and native shield/overshield shaders
  at `+0x7c/+0x84/+0xe8/+0xf0`. The per-model camouflage shader is
  H2-specific. Halo 1 implements camouflage through its globals.
- The optional Halo 2 side of the paired HUD, HUD drawing shaders,
  particle and material shader templates, default texture inputs, the
  scope effect's screen vertex shader and the player emblem are native
  rendering dependencies. The primary HUD and source bitmap slots use
  converted Halo 1 tags.

## Models that used the marine head shader

**Built.** A model with an H1 `soso` whose base, detail and reflection
datums are null now gets its own H2 `shad`. On the owner's campaign
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
and a varying wave currently returns a conversion error.

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
previously converted campaign maps against the shader references in
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
The new instance binds that converted H1 bitmap to H2's additive
`one_add_illum` template. The stage adds zero RGB and leaves depth intact. The adjacent floor-symbol
material in d20 has four source images. Its cutout bump now preserves the
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

```sh
H1_ISO="$PWD/disc/halo1.iso" H2_DISC="$PWD/disc/halo2.iso" H1_MAPS=all cargo test --release -p h1objects --test role_audit_disc -- --ignored --nocapture
```

The shader regressions cover the combiner equations, named conversion errors,
all 21 affected material slots, and the a10 charger's scrolls and power colours. Run from the
integrated checkout with `disc/` present.

```sh
cargo test --release -p h1bitmaps -p h1convert -p h1objects --lib
cargo test --release -p h1objects --test weapon_counter
H1_ISO="$PWD/disc/halo1.iso" H2_DISC="$PWD/disc/halo2.iso" cargo test --release -p h1objects --test model_shaders_disc -- --ignored
```
