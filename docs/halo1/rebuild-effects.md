# Rebuilt Halo 1 objects: effects, weapons and particles

Effects, weapons, equipment, projectiles, damage, particles and contrails from definitions, split from [rebuild.md](rebuild.md).

| Section |
|---|
| [Effects from definitions (`h1objects::rebuild::effect`)](#effects-from-definitions-h1objectsrebuildeffect) |
| [Weapons, equipment, projectiles and damage (`h1objects::arsenal`)](#weapons-equipment-projectiles-and-damage-h1objectsarsenal) |
| [Particles and contrails (`prt3`, `cont`)](#particles-and-contrails-prt3-cont) |

Part of [rebuild.md](rebuild.md).

## Effects from definitions (`h1objects::rebuild::effect`)

`crates/formats/tags/src/defs/effe.txt` is the Halo 2 `effe` from
[effects.md](../formats/effects.md) (strides V by tiling, the E readers there), with
`struct property` for the emitters' sixteen-byte properties. All 805
distinct retail effects on the disc (14,785 instances) round-trip
byte-identical with no pinned values. Named on the way:

| Field | Evidence |
|---|---|
| part `+8` `type_group` (`u32`) | E: `0x0017a8a0` dispatches on it (effects.md). V: the reference's group, `obje` for an object |
| particle system `+0x16` (unknown, was taken for padding) | V: nonzero on 347 instances |

**Halo 1's `effe`** (`h1objects::h1::Effect`, V on every effect of every
map of the Halo 1 disc, `effects_disc.rs` `halo1_effect_layout_survey`,
7,300 tags): root `0x40` (flags `+0`, loop start and stop events `+4`,
`+6`, locations `+0x28` of `0x20`: a marker name, events `+0x34` of
`0x44`); event: `+4` skip fraction, `+8` delay bounds, `+0x10` duration
bounds (`+0..4` and `+0x18..0x2c` zero everywhere), parts `+0x2c`
(`0x68`), particles `+0x38` (`0xe8`, a `part` reference at `+0x54`);
part: `+0` creation medium, `+2` violence mode, `+4` location, `+6`
flags, `+0x14` a runtime word, `+0x18` the reference, whose group word is
the part's kind on every part (`snd!` 5,031, `deca` 4,804, `jpt!` 342,
`ligh` 265, `pctl` 203, `garb` 131, `proj` 73), `+0x28..0x40` zero, then
from `+0x40` the same quantities in the same order as Halo 2's part from
`+0x14`: velocity bounds, cone angle (radians, 1 to 80 degrees), angular
velocity bounds, radius modifier bounds, and the two scale words at
`+0x60`, `+0x64` (`0x20`, bit 5, on the same seven parts in both words,
the bit Halo 2's engine tests). Halo 1's attachments (`obje +0x140`,
`0x48`): the attached tag at `+0`, its marker at `+0x10` (32
characters).

A Halo 1 effect (`Converter::effect`) is written from the definition with
no retail template read:

- **locations**: Halo 1's marker names as string IDs;
- **events**: Halo 1's skip fraction, delay and duration bounds;
- **parts**: each with Halo 1's creation
  medium (D: Halo 1's 1 is "air only", Halo 2's 1 skips the part
  underwater), location, flags, velocity, cone, angular velocity,
  radius modifier and scale words, the `type_group` and the reference:
  - sounds through `h1sounds` (`Converter::sound`, once per sound);
  - projectiles and debris (`garb`) through the object converter;
  - lights and decals (below);
  - damage (`jpt!`) through `Converter::damage`; a projectile's own
    primary damage part is left to the projectile
    (`Converter::effect_after_detonation`);
  - particle systems (`pctl`) are written into the event's particle
    systems (`effect_particle_systems`, [Particles and
    contrails](#particles-and-contrails-prt3-cont));
  - any other group is a named conversion error.
- Everything else is the definition's default (flags, the loop event,
  the root's unknown words zero; no beams or accelerations).

The converted sounds and looping sounds need the map's sound gestalt
(`ugh!`), so a `Tree` holds them apart (`Tree::sounds`, `Tree::loops`)
and the tags name them by placeholders (`convert::sound(k)`,
`convert::looping(k)`). `Tree::install_audio` (called by
`Tree::install` after the tree's own tags, so those land in the slots the
Halo 1 sandbox's placements were promised; h1campaign calls it before
the objects) adds them all or none, reusing a sound or looping sound the set has by path; a refusal
(the gestalt's 16-bit indices) is an error. `install_over` (no new slots)
refuses a tree that holds source audio, and a placeholder left in a tag
with no sound to point at is a conversion error naming the map and the
tag, never a null reference.

**Attachments.** A rebuilt vehicle's attachments (`obje +0x94`: the
type and the marker; E `0x000bed2a` builds `ligh`, `lsnd`, `effe` and
`cont`) are Halo 1's looping sounds (`h1sounds::looping`) and effects at
Halo 1's markers, and its `creation_effect` Halo 1's (`obje +0xa0`).
Lights (below) and contrails (`Converter::contrail`) are attached too.
On the ten levels: the warthog's, scorpion's, banshee's,
pelican's, dropship's and gun turret's engine loops and lights, and
a30's `lifepod_entry` its effect and loop. The plain lifepods carry only
two lights in Halo 1 and stay quiet, as in Halo 1.

Proof (`crates/tools/h1objects/tests/effects_disc.rs`; lib tests
`rebuilt_effect_keeps_halo_1s_events_and_the_parts_that_convert` and
`audio_placeholders_without_their_sounds_are_a_named_error_over_donor_slots`):

- `every_effect_rebuilds_from_its_definition`: every `effe` (7,271) and
  `vehi` of the ten campaign levels (on 05a_deltaapproach) and the
  multiplayer maps (on Coagulation and Beaver Creek) converts; every
  built effect reads back through the definition, writes the same again
  and pins nothing; its locations' markers, event count and skip
  fractions are Halo 1's; every part's reference resolves (a converted
  sound, or a tree tag of the part's group) with a matching
  `type_group` and a location in range; every vehicle attachment
  resolves to a converted tag at the source marker. Attached Halo 1
  particle systems resolve to native `effe` tags.
- h1campaign's `every_campaign_level_converts` installs the objects'
  sounds with the gestalt and builds all ten levels fresh at lean
  `Everything` with 0 dangling references: 0 to 1 effect sounds and 0 to
  11 looping sounds per level (at most 0x1f8ad4 sample bytes, b40), no
  file size retry; tag memory at most 0x942000 of 0xc24000 (b40).

What only a game run shows: the effects' sounds playing at their
markers (the engine's part creation from the converted parts), the
vehicles' engine loops scaled by Halo 2 functions the rebuilt vehicles
do not have (Halo 1's attachments name no Halo 2 function: the loops
may play at a fixed level), and whether Halo 1's per-second part
velocities need a factor.

### Lights (`ligh`)

`crates/formats/tags/src/defs/ligh.txt` is the Halo 2 `ligh` of
[effects.md](../formats/effects.md) (E readers in
[e3-lighting.md](../graphics/e3-lighting.md), "The light record"). All
190 distinct retail lights round-trip byte-identical.

Halo 1's `ligh` (`h1::Light`, `0x160`, no blocks; V on 304 lights of six
maps, `halo1_group_word_survey` with `H1_GROUP=ligh`): `+4` a radius
(0.05 to 15 world units), `+0x10` and `+0x14` two angles in radians (20
to 180 degrees, a half turn on most: a sphere), `+0x38` and `+0x48` two
colours as alpha, red, green, blue, `+0xac` a lens flare, `+0xf4` a
duration on 59 lights in multiples of three (3 to 180, read as ticks).
Names D.

`rebuild::light` writes, with no template: a sphere of Halo 1's radius
(radius and specular radius), or under a half-turn cutoff a frustum
(field of view the cutoff angle, falloff and cutoff distance the
radius, stretch 1); Halo 1's lower and upper colours as both the diffuse
and specular colours at the two intensity bounds; flag bit 3, no shadow
(Halo 1's dynamic lights cast none); the duration in seconds (ticks
over 30, D); and a unity brightness multiplier (E `0x000c0dc9`
interpolates the colour and a separate brightness; Halo 1 gives the
colour directly). A light's lens flare is set by
`convert_flares::light_with_flare`. Lights are
built for effect parts and object attachments; all 898 lights on the
disc's maps (95 cones) rebuild and read back.

### Decals (`deca`)

`crates/formats/tags/src/defs/deca.txt`: all 157 distinct retail decals
round-trip byte-identical; the zero spans are pad. Halo 1's `deca`
(`h1::Decal`, `0x10c`; V on 270 decals of six maps) holds the same
fields in the same order as Halo 2's: flags (`+0`, the same bits, `8`,
`0x68`, `0x168`), kind (`+2`, 0 to 3), the next decal (`+8`), the radius
bounds (`+0x18`), two colours (`+0x34`, `+0x40`), the lifetime and decay
time bounds (`+0x78`, `+0x80`, seconds; Halo 2's retail 180 and 20 sit
there), the bitmap (`+0xd8`) and 16.0 at `+0xfc` (Halo 2's `+0xa4`).
`rebuild::decal` writes those from Halo 1, the bitmap converted by
`h1bitmaps`, the next decal converted in turn, and the layer (Halo 1's
0 to 3 select Halo 2's 2, 4, 5 and 1; [role-audit.md](role-audit.md)); the
overlap limit and optional scale take their definition defaults.
All 916 decals on the disc's maps rebuild with their bitmaps.

Proof: `every_effect_rebuilds_from_its_definition` (above) also builds
every light and decal of every map and reads them back (kind, range,
colours; radius, lifetime, bitmap and next decal resolving in the tree):
10,030 effect parts kept. The campaign sweep stays at lean Everything
with 0 dangling references, b40 at tag memory 0x942000.

What only a game run shows: the light's brightness against Halo 1's,
the cone lights' look, the duration factor, and the decals' layer and
blending.

**Halo 1 sandbox import** (`sandbox_disc.rs`
`every_map_builds_in_both_sandboxes_with_the_objects_sounds`): every
multiplayer map on the disc through `h2 import halo1`'s path in both
sandboxes (26 builds), every attachment of every converted vehicle (587)
naming a tag of its group in the built map. The placements name the
objects and the collections after them by datum, so the sounds the
objects play go in last: `ObjectTags::install` adds the tags alone
(`Tree::install_tags`) and `ObjectTags::after`, run once the collections
are in, adds the sounds and points the tags at them
(`Tree::install_audio_for`). The cli's
`every_multiplayer_job_builds_in_both_sandboxes` builds every map through
the import's own job function (`DiscConverter::multiplayer`).

### Dialogue (`udlg`)

`crates/formats/tags/src/defs/udlg.txt`: all 42 distinct retail `udlg` round-trip
byte-identical. A Halo 2 `udlg` is a list of vocalisations, each a
string ID name and a `snd!`, plus an `adlg` reference (null on every tag)
and a per-tag string (a three-letter designator, `jon`, `grf`, `dop`;
null on the Chief's, the multiplayer elite's and the drone's). On 05a
its 17 tags use 330 names, Halo 2's AI vocabulary (`pain_mjr`,
`dth_fall`, `seefoe`, `chr_kllfoe`, ...).

Halo 1's `udlg` (`0x1010`; 102 distinct on the disc, `effects_disc.rs`
`halo1_dialogue_survey`) is a fixed table of `snd!` references, a line
per offset, the sound's own name saying which on every tag that sets it
(`+0x70` pain minor, `+0xf0` death quiet, `+0x740` grenade throwing,
`+0x360` killed enemy, ...).

**Built.** The rebuilt `udlg` starts with one row per entry of the
base's `adlg +0x00` vocabulary. Halo 2 accesses it by numeric index
(`0x1144f4`). Native rows carry the converted Halo 1 sounds for
`h1objects::rebuild::DIALOGUE`'s verified event pairs. Explicit source
slots follow those rows. Each source row has the exact sound and the
slot index in the low half of its marker at `+0x00`.

A rebuilt unit with one ordinary voice uses `hlmt +0x8c`. Multiple
voices use separate variant references at `hlmt.variants[].+0x30`,
with their source selectors. H2 caches the selected dialogue datum
per unit during speech setup. See [AI dialogue](ai.md) for
the runtime lookup and remaining automatic events.


### Material effects (`foot`)

`crates/formats/tags/src/defs/foot.txt`: all 95 distinct retail `foot` round-trip
byte-identical. A Halo 2 `foot` is a block of kinds by index, each with a
sounds block and an effects block of `0x18` elements: the tag (an effect
has a second, null on characters), a global material name and, at
`+0x14`, that name's index among the globals' materials (V: every
element's index names its string; `default_material` 0), then a word
that is 2 on most sound elements and 1 on every effect element (what
reads it is not established; written as the majority). On every Halo 2
character's `foot` the kinds 0, 1, 3 and 4 play walk, run, shuffle and
jump sounds and kind 2 none (`halo2_foot_survey`).

Halo 1's `foot` (`h1::Foot`, `halo1_foot_survey`) is the same idea: six
kinds (`0x1c`), each 32 materials (`0x30`: an effect at `+0`, a sound at
`+0x10`) in Halo 1's material order, the kinds named by their sounds
(walk, run, none, shuffle, jump up, landing). `Converter::foot` and
`rebuild::foot` write each kind at its index, each
material's sound (`h1sounds`) and effect (`rebuild::effect`) under the
Halo 2 name `arsenal::MATERIALS` gives it, the first
Halo 1 material winning where two share a name, with the base's index of
that name. Retail Halo 2 feet also list `default_material` and the
coarse classes (`soft`, `tough`, `hard`); Halo 1 has no such entries,
so none is written. Every rebuilt biped and vehicle naming a Halo 1
`foot` names the converted one (`obje +0x54`, `FOOT_FIELD`) in place of
the counterpart's. V (`every_foot_converts`): 137 Halo 1 feet over the
campaign and multiplayer maps, 3,898 sound and 306 effect elements, every
material index naming its string on the base, every tag a converted one,
183 rebuilt units pointing at theirs. Only a real run shows whether the
steps play: the engine picks an element by the surface's global
material, and the converted BSPs' surfaces may name materials other than
these 32 (h1convert does not use this table), which then find nothing.

### Lens flares, debris and light fixtures (`lens`, `garb`, `lifi`)

**Lens flares.** `crates/formats/tags/src/defs/lens.txt` is the Halo 2 `lens`
of [effects.md](../formats/effects.md); all 160 distinct retail lens flares (and the one `garb`) round-trip
byte-identical. Four engine paths submit a flare through
`0x0002dba0` (**E**, the calls found by a scan of the XBE): sky lights
(`0x0003f1e9`, `sky` light `+0x14`), lights (`0x000c34ac`, reading the
`ligh` datum at `+0x94`), object attachments (`0x00041b23`) and effect
parts in the effect render (`0x00179d23`, the part's datum at `+0x10`;
the part *creation* switch skips `lens` at `0x0017a8bd`). The
submission reads `+0x18`/`+0x1c` (no flare past the far distance; equal
values switch the test off), the flag bits at `+0x28` (2 or 3 drop it,
by the caller's kind) and `+0x2a`, and `+0x16` (a second record).

Halo 1's `lens` (`h1convert::lens::H1Lens`; V, all 641 lens tags of
the Halo 1 disc, `flares_disc.rs` `halo1_layout_survey`) is a `0xf0`
root with its reflections at `+0xc4` (`0x80` each). Its first `0x30`
bytes are Halo 2's in the same order (V by range: the two angles with
their cosines, a radius of 0.001 to 50, a `u16` of 0 to 2 at `+0x14`,
the two fade distances, 1 to 15 and 4 to 250, the bitmap at `+0x20`),
and Halo 1's two scales at `+0xa0`/`+0xa4` hold Halo 2's range at
`+0x34`/`+0x38` (0.1 to 8, 1 on most). A reflection pairs by value
sets (V, `halo1_layout_survey` with `H1_STRIDES=c4:80` against
`halo2_lens_survey`): Halo 1's `+0` flags (2, 4, `0xc`), `+4` bitmap
index (1 to 6), `+0x1c` position (the very same words, 0.15 to 0.61),
`+0x20` rotation offset (20), `+0x28` radius bounds, `+0x34`
brightness bounds and `+0x44` tint (red, green, blue after an alpha of
1) are Halo 2's `+0`, `+4`, `+8`, `+0xc`, `+0x10`, `+0x18` and `+0x24`.
`h1convert::lens::rebuild` writes those (the cosines recomputed) and
the bitmap converted by `h1bitmaps`. Not carried: Halo 1's flags (D:
its one set bit has no known Halo 2 meaning, and Halo 2's bits 2 and 3
hide the flare), the reflections' scaling sources (`+0x30`, `+0x3c`),
colour bounds (`+0x50`, `+0x60`) and animation (`+0x70`), and the
rotation function (`+0x80`, `+0x84`); the function blocks stay empty,
as on most retail flares (115 of 161 have none at `+0x4c`).

Halo 1 names lens flares from lights (`ligh +0xac`, 656), sky lights
(20), and two shader groups (`senv +0x30`, 17; `sotr +0x38`, 21), and
nothing else (V, `halo1_reference_survey`). A light's flare (`Converter::light` sets `ligh +0x90`),
a sky light's (`h1convert::sky::Sky::flares`, written to `sky` light
`+0x14` by `h1sky::convert::convert_sky_with_flares`). A shader's flare
has no Halo 2 home: no Halo 2 shader names a lens on the disc (V,
`halo2_reference_survey`: lens references come from `ligh`, `sky `,
`effe` parts and object tags only), and Halo 1 draws them only where
its lightmapper placed them in the structure, which the structure
converter does not read; they are left out.

**Debris.** Halo 1's effects throw `garb` objects (131 parts on the
disc; nothing else names a `garb`). Halo 2 keeps the group: the engine
type table has `garb` at index 4 with no scenario placement block (E
`0x00468630`), and an effect part of type `obje` creates any object
class its reference names (E `0x0017ab39`). Retail Halo 2 effects throw
`bloc` crates through `obje` parts (1,156 parts) and `proj` (4), and its
debris meshes are particle models (`PRTM`, 243 tags); its one `garb`
(the SMG's dropped magazine, 30 instances) is no effect's part (V,
`halo2_reference_survey`). `bloc` needs a Havok physics model and
`PRTM` a `prt3`, so Halo 1's garbage stays garbage (D). `crates/formats/tags/src/defs/garb.txt` is the item layer and
`0xa8` bytes zero on the disc's only `garb`, which round-trips.
`convert_flares::garbage` writes the object header (type 4, bounds, model,
predicted resources), the item words and the attachments
from the definition; `Converter::effect` names it through an `obje`
part.

**Light fixtures.** Halo 1's 23 `lifi` tags each carry their light as
`ligh` attachments (25 attachments; V `halo1_reference_survey`), and
nine levels place 886 fixtures. Halo 2's scenario has its own light
placements (`scnr +0xe8`, `0x6c` each, palette `+0xf0`; V over 430
placements on 26 scenarios: the palette index and name first, the
position at `+8` and the facing at `+0x14` as other placements, then a
second point at `+0x4c`, two scales at `+0x58`/`+0x5c`, a cone angle at
`+0x60` and two distances at `+0x64`/`+0x68`, as the `ligh` cone's
fields). Halo 1 has no such placements: its fixture's light rides on
the fixture. So the converted fixture (a `mach`) carries
Halo 1's lights as attachments at Halo 1's markers
(E `0x000bed2a` builds `ligh` attachments), each with its
lens flare. The placement's own colour, intensity and cone angles
(Halo 1's light fixture placement) are not carried (D: what Halo 1
uses them for was not established).

Proof: `flares_disc.rs` `every_flare_debris_and_fixture_converts`
(ten campaign levels on 05a, the multiplayer maps on Coagulation and
Beaver Creek): 636 lens flares with 1,052 reflections (position and
radius bounds checked against Halo 1's bytes), 652 lights naming a
converted flare (every light whose Halo 1 light names one), 46 garbage
objects, 131 effect parts naming a converted `garb` through `obje`, 23
light fixtures carrying their 25 lights, 19 sky lights with a flare;
every tag reads back through its definition and pins nothing. Unit tests:
`h1convert::lens` (angles, distances, reflections, no bitmap),
`convert_flares` (garbage, a light's flare, a fixture's lights),
`h1sky` `converted_lights_name_the_flares_given`.

What only a run shows: the flares' look (no function blocks, Halo 2's
`+0x20` reflection word and root `+0x2c`/`+0x30` at zero: whether a
flare draws at all, and how bright), whether
garbage falls and settles as Halo 1's does (item physics), and the
fixtures' lights against Halo 1's.

## Weapons, equipment, projectiles and damage (`h1objects::arsenal`)

Every Halo 1 `weap`, `eqip` and `proj`, and the `jpt!` its projectiles
deal, is written from its definition with no retail template bytes
(`crates/tools/h1objects/src/arsenal.rs`, `convert_arsenal.rs`); there
is no template path for these groups. `proj.txt` and `jpt_.txt` are the
definitions; all 47 retail projectiles and 135 damage effects
round-trip byte-identical. Building one starts from the definition
default, then:

- **Halo 1's values**: the named fields below, and by offset the words
  carried raw (S, V2 in docs/halo1/objects.md):
  `WEAPON_WORDS` (heat; aim assist, E), `BARREL_WORDS` (rate of fire,
  magazine, rounds per shot, error, spread, first-person offset, heat per
  round), `PROJECTILE_WORDS` (radii, arming time, timer, gravity, damage
  ranges; speeds per tick times 30).
- **Material responses** (`proj +0x19c`, `arsenal::responses`): Halo
  1's 33 responses, one per Halo 1 material type, become the response
  most of them share, for `default_material`, then one element per Halo
  1 material whose response differs, for the Halo 2 global material of
  the same substance (`arsenal::MATERIALS`; the first Halo 1 material
  wins where two map to one name). The material index at `+0x10` is the
  name's position in the base's globals (`matg +0x150`). The response
  maps through `arsenal::response`; a response Halo 2 has no
  counterpart for is a conversion error.
- **First-person graphs** (h1models): Halo 1's `first-person <action>`
  animations go under the one mode, class and type `any`, as every
  retail first-person graph has, with `jmad +0x54` the one pair
  (`any`, `any`).
- **Globals grenades** (`h1convert::globals::set_grenades`): each Halo 2
  grenade type of `matg +0x100` takes Halo 1's maximum and its converted
  equipment and projectile (Halo 1's `matg +0x128`), in the campaign
  and multiplayer converters both.
- **References**: converted Halo 1 tags for the model chain, the
  first-person model and animations (two elements, as retail's Chief and
  Arbiter), the projectiles and every damage slot of a projectile
  (impact, and detonation: Halo 1's attached detonation damage or the
  first `jpt!` its detonation effect names). A slot Halo 1 has nothing
  converted for stays null; the counterpart (`Converter::counterpart`,
  the first `TEMPLATES` row the base holds) supplies only the Halo 2 side
  of the weapon's HUD pair. The slots
  named by the retail tags they hold (`arsenal::WEAPON_OVERHEATED`, which
  is the weapon's `overheated_effect` below, `WEAPON_MELEE_DAMAGE`,
  `WEAPON_MELEE_RESPONSE`, `PROJECTILE_DETONATION_STARTED`,
  `PROJECTILE_AIRBORNE_DETONATION`, `PROJECTILE_DETONATION`,
  `PROJECTILE_SUPER_DETONATION`, `PROJECTILE_FLYBY_SOUND`) and the
  material effects take Halo 1's converted tags, or none
  ([role-audit.md](role-audit.md)).
- **What a shot and a reload play**: each barrel's
  firing effects (`barrels.firing_effects`, `0x34`) are its Halo 1 trigger's
  (`+0x108`, `0x84`: `h1::FiringEffect`), one element per Halo 1
  element, the firing, misfire and empty effects (Halo 1 `+0x24`,
  `+0x34`, `+0x44`) converted with `Converter::effect` (their `snd!`
  parts with `h1sounds`) into `firing_effect`, `misfire_effect`, `empty_effect`
  (`arsenal::FIRING_EFFECT_REFS`); one Halo 1 names as a bare `snd!`
  (the plasma rifle's empty effect) is converted as a sound and named
  as one, which retail's slots also hold. Each magazine's reloading effect
  (Halo 1 `+0x38`) is Halo 1's at magazine `+0x34`. A weapon with no
  Halo 1 effect there plays none. V
  (`weapon_sounds_disc`): Halo 1 firing effects hold `effe` or `snd!`
  only at `+0x24/+0x34/+0x44` and `jpt!` only at `+0x54/+0x64/+0x74` on every
  map; retail's `firing_effect`, `misfire_effect` and `empty_effect` hold
  `effe` or `snd!` and the three damage slots `jpt!`; on Blood Gulch every weapon's built firing
  and reloading effects are Halo 1's by path and play Halo 1's sounds.
  The weapon's own effects and sounds (ready, overheated, detonation,
  light, pickup, zoom, collision) and the chambering effect are Halo 1's
  too, or empty (docs/halo1/objects.md, "Weapon effects and sounds"),
  and the first-person graphs carry Halo 1's animation sounds
  (docs/halo1/models.md, "Animation sounds"). The shot counts (`+0` of
  both elements) are not carried: every retail element holds i16 1, 1
  ([notes/rebuild.md](notes/rebuild.md), "Weapon firing effects"). The HUD (`+0x2b0`) is Halo 1's
  `wphi` converted (`Converter::weapon_hud`, docs/halo1/hud.md); the
  counterpart's `nhdt` is the weapon's Halo 2 HUD in its HUD pair.

Named on the way (V: `weapon_rebuild_disc`
`halo_1_and_halo_2_pairs_agree_on_the_named_fields`, Blood Gulch's
arsenal against Coagulation's counterparts):

| Field | Evidence |
|---|---|
| `weap` magazine `+0x6` `rounds_total_initial`, `+0x8` `rounds_total_maximum`, `+0xa` `rounds_loaded_maximum`, `+0x10` `reload_time`, `+0x14` `rounds_reloaded` | equal to Halo 1's magazine `+0x6`, `+0x8`, `+0xa`, `+0x14`, `+0x18` on 3, 3, 6, 5 and 6 of 8 pairs (no other offset matches) |
| `weap` barrel `+0x36` `firing_noise` | equal to Halo 1's trigger `+0x2e` on 8 of 10 |
| `weap +0x2a8` `first_person` (`model` `mode`, `animations` `jmad`), `+0x2b0` `hud` (`nhdt`) | the retail targets: `fp_<weapon>`, the Chief's and the Arbiter's first-person graphs, `ui\hud\<weapon>` |
| `eqip +0x12c` `powerup_type`, `+0x12e` `grenade_type`, `+0x130` `powerup_time` | equal to Halo 1's `+0x308`, `+0x30a`, `+0x30c` on all four pairs (over shield, active camouflage, both grenades) |
| `proj +0xc8` `collision_radius`, `+0xe0` `maximum_range` | equal to Halo 1's `+0x1a0` on 9 of 10, `+0x1c8` on 3 |
| `proj +0x100` `detonation_damage`, `+0x118` `super_detonation_damage`, `+0x12c` `attached_super_detonation_damage`, `+0x148` `impact_damage` (`jpt!`) | the retail targets' names (`rocket_launcher_explosion`, `needler_shard_*`, every bullet's damage at `+0x148`) |
| `jpt! +0x1c` `damage_lower_bound`, `+0x20` `damage_upper_bound` (from, to) | Halo 1's `+0x1d0`, `+0x1d4`: in both games lower <= from <= to on 6 of 8 pairs, all three equal on the sniper rifles |
| `obje` change colour `+0x8` `functions` (`0x28`) | filled on the needler shard projectile only |
| `proj +0x19c` `material_responses` (`0x58`): `+0x2` `response`, `+0xc` `material_name`, `+0x10` `material`, `+0x14` `potential_response`, `+0x16` `potential_flags`, `+0x18` `chance`, `+0x1c` `angle_bounds`, `+0x24` `velocity_bounds`, `+0x34` `scale_effects_by`, `+0x38` `angular_noise`, `+0x3c` `velocity_noise`, `+0x48` `initial_friction`, `+0x4c` `maximum_distance`, `+0x50`, `+0x54` frictions | Halo 1's element (`0xa0`) in the same order without its padding; on two bases each of the 179 retail elements' `material` is its name's position in `matg +0x150` and its flags zero; Halo 1's most common response, mapped, is the retail default material's on all 8 Blood Gulch / Coagulation pairs (grenades bounce 4, needle attach 3, rocket and bullets impact 0), Halo 1's water overpenetration is retail's thin liquid 2, and the plasma grenade's attach chance (Halo 1 potential 4, flags 1, skip 0) is retail's (3, 1, chance 1) (`material_responses_pair_with_retail`) |
| `jmad` first-person modes | every retail first-person graph on Coagulation (34) has the one mode `any` and `+0x54` (`any`, `any`) (`first_person_graphs_take_retail_modes`) |
| `matg +0x100` grenades (`0x2c`: maximum `+0`, `eqip` `+0x1c`, `proj` `+0x24`); Halo 1's `matg +0x128` (`0x44`: `+0`, `+0x24`, `+0x34`) | element 0 the frag grenade, 1 the plasma grenade on three Halo 2 bases and all eleven Halo 1 maps, and each Halo 1 grenade's `eqip` grenade type its element (`grenade_blocks_pair_across_the_discs`) |

**Proof** (`every_weapon_equipment_and_projectile_rebuilds`): on the ten
campaign levels (on `05a_deltaapproach`) and the 13 multiplayer maps on
the disc (on their bases), every `weap`, `eqip` and `proj` converts:
316 weapons, 274 equipment, 363 projectiles and 378 damage effects
(counted per map). Each reads back through its definition and writes the
same again, has no raw and no pinned block, and every reference is a tag
of the tree or of the base of its group. Checked against Halo 1: object
type, bounding radius, magazines (counts, reload time), one barrel per
trigger with its firing noise and Halo 1's converted projectile,
first-person model (and animations, 211 weapons; a graph that does not
convert is left null and reported), powerup type, grenade type and time
(over shields and camouflages),
projectile radius, range, speed per second and Halo 1's damage, and every
`jpt!`'s bounds. The test prints the Halo 2 reference slots by group.

What only a game run can show, and the first suspects: firing,
reloading and the HUD (the barrel's unnamed words such as the firing
mode and the magazine's other words stay at the definition's default);
grenades being thrown and bouncing (the material responses and the
globals' grenade block); powerups (Halo 2's handling of over shield and camouflage
types with Halo 1's times); the first-person arms, since Halo 1's
first-person graph animates Halo 1's arm nodes.

## Particles and contrails (`prt3`, `cont`)

`crates/formats/tags/src/defs/prt3.txt`: all 2,021 distinct retail `prt3` (4,988
instances) round-trip byte-identical. The root's fields are named from
the engine's reader, the particle draw callback `0x00224550`
([effects.md](../formats/effects.md), "prt3 root"): the five properties are colour,
alpha, size, rotation and sprite frame; the flags, the sequence range,
the collision and death effects, the input mask, the quad and the UV
window. The nested particle systems are `effe`'s record (and use its
`struct property`, so the file follows `effe.txt`); the compiled shader
at `+0x8c` is `shad`'s postprocess layout but for two strides (V, the
schema: lod elements `0x8` and pass elements `0x4` where `shad`'s are
`0x6` and `0x2`). Two words the schema reads as raw in `prt3` are `raw`
here: an emitter's `+0x68` (the input pair of its seventh property) and an
implementation's `+0x4` (its stage-state and pixel-constant ranges).

**The compiled particle shader.** A particle's `postprocess` is what
the tag compiler makes of its `stem` and parameters; with no animated
parameter it follows one rule from the stem (V: 533 retail particles on
05a, Coagulation and Beaver Creek, the six with animated parameters
aside; `h1objects` `tests/particles_disc.rs`
`retail_particle_shaders_compile_from_their_stems`):

- one `bitmaps`, `colors` and `vectors` element per instance source the
  stem's bindings name, through each implementation's ranges;
- `lods`, `layers` and `passes` hold the stem's own layer, pass and
  implementation ranges (the lod's animated-layer mask zero), one empty
  `implementations` element per stem implementation;
- a bitmap's `lod_offset` is `log2(max(width, height))` of its first
  image times `ln(2)^2` (5 to 8 times `ln(2)^2` on 32 to 256 pixel
  bitmaps);
- `parameter_bitmaps` five entries (`0` then `-1`), `parameter_colors` two
  zero colours and `parameter_scalars` six zeros.

The colour and vector values are the parameters' compiled defaults; 502
of the 533 hold their stem's most common set. A lod's `+0x6` and a pass's
`+0x2` vary on retail and nothing reads them.
`h1objects::particles::particle_postprocess` writes this.

`crates/formats/tags/src/defs/cont.txt`: all 34 distinct retail `cont`
round-trip (a `0xf0` root, the bitmap at `+0x30` and `+0x88`, `0x40`
points each with a `pphy` at `+0x10`). The spans are named by where they
sit in Halo 1's `cont` (`0x144`, points `0x68`), which Halo 2's compacts
(V, the same contrails of both games side by side): root `+0x4..+0x30`
is Halo 1's `+0x4..+0x30`, `+0x38` Halo 1's `+0x40`, `+0x3c..+0x60` zero
on every retail tag, `+0x60..+0x88` Halo 1's `+0xa8..+0xd0`, `+0x90..+0xe8`
Halo 1's `+0xe0..+0x138`; a point's `+0x0..+0x10` and `+0x18..+0x40` are
Halo 1's `+0x0..+0x10` and `+0x40..+0x68`. The fields' meanings are not
read from the engine.
