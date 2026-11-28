# Halo 1 objects: weapon and model details

Weapon class, model materials, change colours, zoom, ammunition and hit registration, split from [objects-build.md](objects-build.md).

| Section |
|---|
| [Weapon class and type (`weap +0x288`, `+0x28c`, **Built**)](#weapon-class-and-type-weap-0x288-0x28c-built) |
| [Model materials (`hlmt +0x58`, **Built**)](#model-materials-hlmt-0x58-built) |
| [Cut-out model shaders (**Built**)](#cut-out-model-shaders-built) |
| [Change colours (`obje +0xac`, **Built**)](#change-colours-obje-0xac-built) |
| [Weapon zoom](#weapon-zoom) |
| [Ammunition counter (**Built**)](#ammunition-counter-built) |
| [Tuning carried over](#tuning-carried-over) |
| [Blood Gulch hit registration (**Built**)](#blood-gulch-hit-registration-built) |

Part of [objects-build.md](objects-build.md).

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
Scope HUD conversion is described in [hud.md](hud-weapons.md#scope-mask).

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
