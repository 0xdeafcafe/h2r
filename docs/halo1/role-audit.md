# Halo 1 sources for rebuilt object tags

The converter starts with a Halo 2 tag definition and writes the source
object and its dependencies into it. A field without a source mapping
keeps the definition default. The following changes are **Built** in
the full-rebuild lane. The disc tests are written for the coordinator's
integration run. There is no new pass count or play result yet.

| Area | Source and evidence |
| --- | --- |
| Items and weapons | **Built**. The shared item layer carries flags, pickup order, collision sound, material effects, detonation effects, damage and delay bounds. Weapon ready and flashlight times, magazine flags, chamber time, ammunition objects and firing-effect shot ranges come from their own records. Each trigger supplies its own effect and damage references. Halo 2 caches ten inverse barrel times; the retail barrels on Coagulation use 1000 for instantaneous changes. Six times come from Halo 1. The second spread profile repeats the two Halo 1 error times. The two Halo 2 gun-climb times are instantaneous. See `arsenal.rs` and `weapon_rebuild_disc.rs`. |
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
- Biped models other than the Halo 1 player keep the native damage section array that `object_new`
  indexes at damage info `+0xbc`, and native shield/overshield shaders
  at `+0x7c/+0x84/+0xe8/+0xf0`. The per-model camouflage shader is
  H2-specific. Halo 1 implements camouflage through its globals.
- The optional Halo 2 side of the paired HUD, HUD drawing shaders,
  particle and material shader templates, default texture inputs, the
  scope effect's screen vertex shader and the player emblem are native
  rendering dependencies. The primary HUD and source bitmap slots use
  converted Halo 1 tags.
- **Known gap.** A Halo 1 model shader with no base map gets the base's
  stand-in model shader, a marine's head on 05a_deltaapproach. a10's
  `light siren` on the alert light names no map at all. The fix needs
  Halo 1's draw of a null base map.

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
