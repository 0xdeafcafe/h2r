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

- The vehicle controller still uses a base drivetrain of the same vehicle
  type, selected by nearest powered-point count and then tag path. Its
  compiled gear curves and their raw locations form a unit. Halo 1 has
  no Halo 2 gear curve resources. The controller selectors are at
  `+0x218` and `+0x244`, the coefficients at `+0x250..+0x264`,
  `+0x270..+0x280`, `+0x2b0..+0x2c4` and `+0x2d8..+0x2e4`.
  The same controller provides tire/tread coefficients by steering type
  and the remaining H2 hover coefficients by marker position. The
  drivetrain reads include `0x000eee7c`, `0x000efbd7`, `0x000f1612`
  and `0x000f2aab`. The tank control mode is 2. The seat entry test
  needs positive distance and angle limits (`0x000c9040`). H1 does
  not author these. Individual native controller coefficients still lack
  a verified physical interpretation. They are listed explicitly in
  the audit; no majority is taken over vehicles, seats, points or gears.
- Biped models keep the native damage section array that `object_new`
  indexes at damage info `+0xbc`, and native shield/overshield shaders
  at `+0x7c/+0x84/+0xe8/+0xf0`. The per-model camouflage shader is
  H2-specific. Halo 1 implements camouflage through its globals.
- The optional Halo 2 side of the paired HUD, HUD drawing shaders,
  particle and material shader templates, default texture inputs and
  the player emblem are native rendering dependencies. The primary HUD
  and source bitmap slots use converted Halo 1 tags.

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
