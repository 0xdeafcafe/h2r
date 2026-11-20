# Halo 1 sources for rebuilt object tags

The converter starts with a Halo 2 tag definition and writes the source
object and its dependencies into it. A field without a source mapping
keeps the definition default. The following changes are **Built** in
the full-rebuild lane. The disc tests are written for the coordinator's
integration run. There is no new pass count or play result yet.

| Area | Source and evidence |
| --- | --- |
| Items and weapons | **Built**. The shared item layer carries flags, pickup order, collision sound, material effects, detonation effects, damage and delay bounds. Weapon ready and flashlight times, magazine flags, chamber time, ammunition objects and firing-effect shot ranges come from their own records. Each trigger supplies its own effect and damage references. Halo 2 caches ten inverse barrel times; the retail barrels on Coagulation use 1000 for instantaneous changes. Six of the times have Halo 1 fields. The remaining four are instantaneous. See `arsenal.rs` and `weapon_rebuild_disc.rs`. |
| Projectiles and damage | **Built**. Projectiles carry their flags, own impact and attached damage, and each damage part in its original effect event. A damage tag carries radius, cutoff, damage bounds, impulse, camera shake, sound, screen flash and motor fades. Each motor has its own duration and transition; the Halo 2 response selectors share Halo 1's authored feedback. Blood Gulch's needle and plasma grenade distinguish the normal and attached damage slots. |
| Object functions and attachments | **Built**. Scenery, bipeds, vehicles, projectiles, weapons, equipment and debris resolve the source creation effect, attachment and marker, and bind primary scales to the output names of the converted object functions. The object acceleration scale moves from H1 `+0x20` to H2 `+0x14`. The ball carries 2 in both formats. The schema includes the creation-effect reference and inherited function and attachment blocks even in groups where retail leaves them empty. |
| Bipeds and models | **Built**. Camera tracks, HUD interfaces, weapon lists, integrated light, melee damage, dialogue, blood effect and material effects follow the original unit and collision references. Unit camera and acceleration parameters, landing and slope limits, jump and crouch rates, and flying motion are written from Halo 1. The crewman's angles on a30 distinguish cutoff and falloff. The monitor and sentinel on c20 and 04b distinguish flying physics. Standing and crouching use the source collision dimensions or a pair of spheres for sphere-only characters. The alternate three sphere sizes remain available to the engine. `0x001c2460` and `0x001d6dd6` select those shapes. |
| Vitality and shields | **Built**. The model's damage record uses H1 collision body and shield vitality, body and shield global material, shield stun and recharge times, the reciprocal recharge rate, and shield damaged/depleted/recharging effects. The cyborg and elite's `coll +0x198` name their depletion effects; their H2 models use `+0xac` in the damage element. The ordinary shield uses the source modifier shader. The native overshield slots and damage section array are described below. |
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
  H1 gravity is 0.0035651792 units/tick² at `0x1fb648`, or 3.20866
  units/s². H2 initializes gravity to 4.17126 at `0x1eb584`. The ratio
  scales hover strength and inversely scales hover damping and ground
  spring depth, preserving each force at the same height and velocity.
  The converted vehicle's gravity step uses its source physics multiplier
  even with empty seats (H1 `0x8399d`, H2 `0x1ced8d`).
- **Built. Tire drag and materials.** H1 projects relative point velocity and
  commanded wheel speed into the contact plane at `0x83e30..0x83f9f`.
  `0x80f80` applies separate drag factors along and across the friction axis.
  Point mass, ground friction, slope and wheel alignment scale the result.
  The Warthog's rear tires use 0.45/0.65 and its front tires use 0.75/0.45.
  H1 globals supply five surface modifiers. The first applies to masses up to
  7,500 at `0x83d26`, so ice scales the Warthog's drag by 0.125 and leaves
  the Scorpion's unchanged. The H2 wheel gains are zero. The engine adds
  H1 force and moment after `0x206090` computes the contact frame and spring
  and before `0x205757` consumes them. Converted rigid bodies carry the H1
  center of mass and inertia tensor.
- **Built. Controls and impulses.** H1 samples crouch in one tick at
  `0x394be`. H2 reads a rate at `+0x25c` and multiplies by the tick
  duration at `0xf045b`, giving 30/s at H1's 30 Hz. Driver and gunner
  power rise and fall separately using the two times in each H1 powered-seat
  record (`0x45880`, `0x4592e`). H1 clears attached vehicles' velocities
  at `0x394a1`. Both extra rider impulse gains at H2 `0xe538c` are zero.
- **Built. Vehicle priority.** H2's AI ram decision compares two vehicle
  ranks at `0x1ab48a`. The H1 driving decisions at `0x16f954` and
  `0x16fbc1` read distances from the acting vehicle. The converter gives
  every vehicle equal rank. This is a translation of the priority comparison.
  The H2 AI still chooses when to ram.
- **Built. Aircraft.** The source type, physics radius and powered-group
  count select the motor. H1 `0x38ac0` drives dropship collective and
  forward thrust, `0x37760` drives the positive-radius lifepod, and
  `0x37ab0` banks the Banshee and uses the acceleration bound at
  `0x35fb0`. A scripted dropship hover suspends the motor and preserves
  the rotor setting (H1 `0x38b14`, H2 hold flag at `0xf0ffe`).
  A source vehicle without physics skips the dispatch at
  `0x398ea`. Converted aircraft supply the resulting forces and moments
  before the H2 contact solve. H2 supplies the contact geometry and rigid-body
  integration.
- The seat entry test needs positive distance and angle limits (`0xc9040`).
  `+0x258` and `+0x2e4` keep their definition defaults. Neither has an
  identified read in the ground or hover drive equations.
- Biped models keep the native damage section array that `object_new`
  indexes at damage info `+0xbc`, and native overshield shaders at `+0xe8/+0xf0`. The source modifier supplies both ordinary shield views at `+0x7c/+0x84`. The per-model camouflage shader is
  H2-specific. Halo 1 implements camouflage through its globals.
- The optional Halo 2 side of the paired HUD, HUD drawing shaders,
  particle and material shader templates, default texture inputs, the
  scope effect's screen vertex shader and the player emblem are native
  rendering dependencies. The primary HUD and source bitmap slots use
  converted Halo 1 tags.

### Retail vehicle scan

**Built.** An offline scan of both Xbox discs found no inputs that trigger
the vehicle-drive conversion errors on the ten campaign or thirteen
multiplayer maps.
It read every `vehi` entry, including vehicles with no placements. Counts
are per map, so a vehicle present on two maps counts twice.

| Source maps | Maps | Vehicle tags | Drive entries | Friction points | Hover points | Powered-seat records |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Campaign | 10 | 57 | 55 | 66 | 51 | 58 |
| Multiplayer | 13 | 39 | 39 | 156 | 39 | 39 |

The two redirected tags are a10’s Halo ring (scenery) and b30’s falling
box (crate). c20 has no vehicle tags. Of the 94 vehicle entries, 90 have
a positive-volume solid collision cell. The other four supply 18 or 19
positive-radius mass points each. All 89 vehicle physics tags have inertia
data. The five vehicles without a physics tag have collision hulls.

All 222 friction points use the supported forward/up frame. Their axis is
0 on 146 points and 1 on 76. Powered groups and normalized marker names
are consistent on every vehicle. The maximum is eight friction contacts
per vehicle (limit 16) and 136 bytes of wheel and surface data (limit 264).
Every vehicle fits the two seat-power channels.

Each source map has 32 material responses. The scan applied the source
mass threshold and resolved the responses for all 41 ground-vehicle
entries against each of the three destination globals tables. Each table
contains 182 materials, with ice at index 101. No response was missing
or conflicted with another response at the same destination index.

The ignored `retail_vehicle_drive_laws_round_trip` regression reads the
map directory, selects each map’s conversion base, and rebuilds the drive
record separately from its attachments.

## Retail fallback inventory

The survey read the ten campaign and thirteen multiplayer maps on the owner's
Halo 1 Xbox disc. It also read the tag tables, structure palettes and sky
models on 05a_deltaapproach, Coagulation and Beaver Creek on the Halo 2
disc. Counts below are distinct map, parent and source-use combinations.
A tag being present on the disc establishes a candidate, not a runtime draw.
The accompanying TSV has 35,510 rows. Each gives the map, group, source path, parent or BSP index, destination base, historical candidate or disposition and code site.

| Former fallback and code site | Retail source and former target | Current handling |
| --- | --- | --- |
| BSP render shaders, `textures::plan_bsp` | 1,981 BSP/shader rows. The old word matcher selected the closest base shader or the first opaque one. | **Built.** Material records name the rebuilt source shader. Its recipe must be available and its source images must convert. The illumination-only `levels\a10\shaders\monitor glow` on a10, d40 and putput uses its own illumination texture at `+0x254` and UV scale at `+0x250`. |
| BSP collision, `textures::Retexture::install` and `Structure::write` | 1,295 BSP/shader/material rows. Delta Approach supplied `tower_metal_dirty`, `tower_metal_trim_g` or `shaders\invalid`. Coagulation supplied `coag_rocks` and Beaver Creek supplied `bc_forerunner_lightgray`. | **Built.** Each source palette entry retains its shader and maps its substance at `+0x22` to the destination globals material index. Collision-only shaders enter the conversion graph. Merged scenery surfaces use their object's source substance. H1 object collision materials have no shader reference. |
| Model materials, `Converter::model_shader` | The first eligible opaque object shader was `objects\characters\marine\marine_female\shaders\head_female` on Delta Approach and `objects\multi\2way_teleporter\shaders\2way_teleporter` on both multiplayer bases. Seven distinct diffuse-null `soso` paths account for 17 map/model/shader rows. | **Built.** A missing recipe, map or program reports the source shader and map. The null-base, Chicago and volume plasma programs are implemented on the separate `h1-next-nullshader` branch. |
| Sky materials, `Sky::from_map/install` | 135 sky/material rows. Delta Approach used `deltatemple\shaders\ring_afternoon`, both multiplayer bases used `coagulation\shaders\coag_ring`. | **Built.** Source material references and bitmaps are required. Missing recipes and flares fail conversion. The scenario records the actual number of source skies. |
| Bitmap, sound and flare reuse, `TagSet::converted` | Six `bitm` paths collide on every map, including `rasterizer\glow` and five `ui\hud\bitmaps` images. No `snd!`, `lsnd`, `snde` or `lens` namesakes were found on these three bases. | **Built.** Reuse requires an installed conversion with the same group, name and live datum. The existing slot may receive rebuilt data. The name alone cannot certify its contents. Loop tracks, details, sound environments and authored sounds propagate conversion failures. |
| Objects, units, campaign placements and AI, `Converter::model`, `roster::prepare/install`, `ai_control_characters::resolve` | Previously an object could inherit a native unit/model root or species character. Examples include H2 Marine, Grunt and Elite characters. The disc has 265 map/unit rows and 217 map/actor-palette entries. Nine additional map/variant pairs occur in explicit script references. | **Built.** The H1 sandbox builds the object and model definitions, actor variant unit, weapon and major variant links. A vehicle lacking a solid hull uses its own mass points or fails. The H2 sandbox can explicitly select an H2 asset. |
| Script atoms and cinematic animation, `h1campaign::scenario` and `cutscenes` | 910 explicit map/reference pairs across eleven source groups, including sounds, loops, animation graphs, effects, damage, actor variants and five object groups. Formerly a path could resolve to a native namesake or disappear. | **Built.** The scenario resolves typed converted datums and explicit H2 asset aliases. Script-only effect, damage and object roots are rebuilt. First- and third-person animation sound events both resolve their authored sounds. |
| Effects, projectiles and devices, `effect_uncached`, `arsenal::responses`, `device` | 10,102 effect-part rows, including 338 damage rows. 7,115 projectile material/effect rows and 137 device transitions, of which 127 are effects and ten are sounds. | **Built.** Every event keeps its original damage parts. H2 dispatches damage directly at `0x17a8a0`. Each material response resolves its normal, potential and detonation effects. Device transitions use the source reference's group. Cyclic dependencies resolve to the eventual converted datum. |
| Contrails, `Converter::contrail` | 265 map/contrail/physics rows formerly emitted a null `pphy`. Cold smoke, steam, vacuum and the Warthog antenna have corresponding point-physics records with differing coefficients on the two discs. | **Built.** The typed H2 definition receives source flags, mass, air/liquid buoyancy, density, drag, friction and restitution. Each contrail point names its own converted physics tag. |
| Multiplayer starting equipment, `halo1::plan_starting_equipment` | Both H2 bases used one `multiplayer\\single_weapons\\smg` collection. Blood Gulch's CTF row instead names the assault rifle and pistol. Wizard has three distinct game-type rows. | **Built.** The H1 sandbox rebuilds its source rows, selector words, collections, weapons and weights. A native variant selector with no H1 weapon or vehicle is left unassigned. |
| Ordinary biped shields, `biped_rebuilt` | 55 biped/modifier rows name three `spla` tags. The H2 Chief uses `masterchief_plasma_shield_change_color_fp` and `masterchief_plasma_shield_change_color`; the Elite uses `elite_plasma_shield_fp` and `elite_plasma_shield`. | **Built.** Both ordinary shield views bind the source modifier through the model-material converter. All three source shaders use the Elite shield noise volume twice. Integration needs the sibling volume-plasma rebuild described above. |

### Dependencies kept from Halo 2

The base map supplies engine globals, global material indices, the sound
gestalt and native scenario infrastructure. The H1 conversion writes source
entries into these containers. Material and HUD rendering require native
shader programs, vertex shaders and default inputs. The H1 Xbox engine has
no equivalent tags for those programs. The HUD's optional H2 view and the
multiplayer player emblem are explicit features. The generic AI behavior
parent and Sentinel behavior style supply H2 engine behavior. H1 actor
variants provide the concrete character. H2 asset mode retains its selected
native units, weapons and vehicles, including assets from donor maps.

`h1map`, `h1bitmaps`, `h1models` and `h1sky` read source data or write
caller-supplied datums. `h1text` extends the destination locale table
to keep IDs already referenced by engine tags. The HUD converter lives
in `h1objects::h1hud`. It builds its primary `nhdt` and binds destination
drawing programs. `h1scripts` emits relocation requests. Campaign resolves
those against the selected sandbox before installing the scenario.

### Retail gaps that stop conversion

| Source | Implementation still needed |
| --- | --- |
| `levels\a10\shaders\burnt flourescent off` on d40 and `levels\a50\shaders\covenant glass floor` on a50/d20 | A verified textureless environment and transparent-glass draw. The source base slot is null. A name-matched BSP material gives the wrong appearance. |
| `effects\particles\solid\glass breakable`, `effects\particles\solid\glass death` and d20's `scenery\captain_implant\glowingdrip goo` | Blend 6 needs a verified framebuffer equation and a matching compiled particle program. The former additive choice affected 47 map/tag pairs. |
| 27 distinct widget paths in 262 owner/map rows, and a10's `levels\a30\devices\beam emitter\beam` attachment | Source widget conversion and dispatcher bindings for `mgs2` volumetric lights, `ant!` antennas, `elec` lightning, `flag` cloth and `glw!` glow. Examples include the Warthog antenna, the Marine flashlight, the multiplayer flag and the Monitor tendrils. An object owning one fails with the exact dependency. |
| A sky cloud layer on a base without the two-input sky program | Supply the shared two-input program and preserve the authored mask and tile. A single-sampler transparent material omits the mask. |
| Authored `udlg` slots without a corresponding H2 vocalization and bipeds with multiple dialogue variants | Native event routing for the unmapped lines and per-unit voice selection. H2 has one dialogue reference per model. The converter identifies the source sound or additional voice that cannot be represented. |

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

```sh
H1_ISO="$PWD/disc/halo1.iso" H2_DISC="$PWD/disc/halo2.iso" H1_MAPS=all cargo test --release -p h1objects --test role_audit_disc -- --ignored --nocapture
```
