# Halo 1 multiplayer: sandbox and converted structure

The Halo 1 sandbox and the converted structure BSP of the multiplayer maps, split from [multiplayer.md](multiplayer.md).

| Section |
|---|
| [The converted structure BSP (**Built** as `h1convert::structure`)](#the-converted-structure-bsp-built-as-h1convertstructure) |
| [Halo 1 sandbox (**Built**: `convert::halo1`, `h1objects::sandbox`)](#halo-1-sandbox-built-converthalo1-h1objectssandbox) |

Part of [multiplayer.md](multiplayer.md).

## The converted structure BSP (**Built** as `h1convert::structure`)

Moved to [Halo 1 multiplayer: the converted structure BSP](multiplayer-structure.md#the-converted-structure-bsp-built-as-h1convertstructure).

## Halo 1 sandbox (**Built**: `convert::halo1`, `h1objects::sandbox`)

`h2 h1 convert --sandbox h1` builds the same structure, spawns and flags
as the Halo 2 variant, and places every Halo 1 item and vehicle spawn as
the Halo 1 object itself: `h1objects` converts each `weap`, `eqip` and
`vehi` (and what they reference: `proj`, models, collision, animations)
onto a Halo 2 template ([objects.md](objects.md)), and
the conversion adds the tags to the pinned base after its own slots.
Nothing is left out but race flags. Slot `n` of a set made from a map gets
salt `0xe174 + n` (`TagSet::from_map`), so the spawns name each new tag's
datum before it exists (`convert::halo1::added_datum`), and the build
checks the datums came out so.

**Each netgame equipment spawn names a collection built from Halo 1's.**
The spawner `0x0015aad0` reads the entry's item
(`+0x58` group, `+0x5c` datum) and rolls it with `0x0019f1a0` only when
the group is `itmc` or `vehc` (E `0x0015ab12`..`0x0015ab23`); any other
group leaves the item -1, and `0x0015ab8b` returns without placing.
A spawn naming a `weap`, `eqip` or `vehi` directly would spawn nothing,
so **no weapon, grenade, powerup or vehicle would ever spawn** in the
Halo 1 sandbox. `convert::halo1` builds one `itmc` per Halo 1 item collection a spawn
uses (each converted item at Halo 1's weight; Halo 1's respawn time as
the root `i16` at `+8`, which the spawner reads when the spawn's own
`+0xe` is 0, E `0x0015ac83`..`0x0015acb0`) and one `vehc` per Halo 1
vehicle, added right after the objects so the spawns name their datums.
`0x0019f1a0` returns a one-entry collection's item whatever its weight
(E `0x0019f227`), and rolls the weights otherwise. Types: weapons 0;
warthog 1, ghost 2, scorpion 3; grenades 8; camouflage, overshield and
the health pack 9. Type 0 keeps a weapon the globals' weapon list
doesn't know (E `0x0015a640`: `0x0015eb80` returns -1 and the item is
kept); vehicle types 1 to 7 and the powerup types keep the item while
the variant leaves the map's default (E `0x0015a4d0`, `0x0015a5b0`).

`h1objects/tests/sandbox_disc.rs` converts Blood Gulch onto Coagulation
and reads every netgame equipment spawn of the built map back: 43 of 43
(35 items, 8 vehicles), each naming an `itmc` or `vehc` this conversion
added, whose entries name added objects of the collection's kind, each
with a render model (`tags::object::Context`). 68 tags are added:
8 weapons with their projectiles, camouflage, overshield, the health pack
(each rebuilt from its Halo 1 equipment), both grenades, warthog, ghost and scorpion,
and the cyborg (`characters\cyborg_mp\cyborg_mp`, on `masterchief_mp`):
73 in all.

**A fresh map rehomes the templates' blocks.** A converted object is its
template's root bytes with Halo 1's values written over, so every block
field it doesn't rebuild still holds the template's address in the
pinned map. A fresh map lays the base's tags out anew, so those
addresses hold other bytes there. In a vehicle that meant `+0x2f8`
(elements of `0x2a0`), whose element `+0x38` count the load-time loop
`0x205550` (from `0x11c8f3`, resource preparation `0x11c840`) runs over,
writing `0x90`-byte entries from element `+0x60` with room for 4. The
junk counts write past the element (pc `0x205574`). The
cyborg (10 fields), its model (4) and every
weapon, projectile and powerup are stale too. `build_fresh` walks
each built tag with the schema and copies every block field that still
holds a pinned metadata address as a tree of its own
(`Unpinner::rehome`, `FreshReport::rehomed`; refusals in
`FreshReport::stale`). `sandbox_disc.rs` checks every multiplayer map on
the disc in both sandboxes: every vehicle's `+0x2f8` counts are at most
4 and nothing is stale. It also checks that every starting location is
in open space with the converted floor within 2 units below.

**The player is the cyborg (Built, `h1convert::globals`).** The engine
picks a player's unit in `0x0014f040` (E): the player's representation
byte (player `+0x88`), clamped to the globals' player representation
block (`matg +0x138`, elements of `0xbc`), and in multiplayer (game mode
2, `[0x4e6948]+8`) 0 becomes element 2 and 1 element 3; the unit is the
element's tag reference at `+0xb0` (datum `+0xb4`), with a word at `+0xb8`.
On the disc every multiplayer map's elements are none, none,
`masterchief_mp`, `elite_mp`; campaign maps fill element 0
(`masterchief`) or 1 (`dervish`) (`tests/h2_globals.rs`, both checks).
`globals::set_player_unit(set, biped, Players::Multiplayer)` points
elements 2 and 3 at the converted cyborg and clears the word;
`Players::Campaign` does elements 0 and 1, for `h1campaign`. The
sandbox's disc test checks both elements name the added cyborg, which
decodes with a render model.

**First-person arms.** The element's `+0x0` is the
first-person arms model (`+0x8` the first-person body). The Halo 1
sandbox and the Halo 1 campaign put Halo 1's arms there
(`globals::set_player_hands`; the arms Halo 1's globals name at
`matg +0x17c`, element `+0x0`, `characters\cyborg\fp\fp` on every map,
converted on their own by `h1objects`). The engine binds the arms' nodes
and the weapon's first-person model's nodes to the weapon's first-person
graph **by name** (E `0x001dd4c0`, called from `0x001680eb` for the
weapon model and `0x00168083` for the arms): Halo 2's arms name no node
of a converted Halo 1 graph (`l_upperarm` against `frame l upperarm`), so
with Halo 2's arms every converted first-person graph would drive the arms
with no node bound. Halo 2's arms stay in the Halo 2 sandbox. The first-person
body stays Halo 2's (Halo 1 has none; its nodes bind to no node of a
Halo 1 graph either).

**The player's camera and hands.** A rebuilt
biped names its model's Halo 1 markers in the unit's marker fields
(`h1objects::rebuild::UNIT_MARKERS`): the camera at `unit +0xd4`
(`head`), and the weapon markers at `+0x158`/`+0x15c` (`right hand`,
`left hand`), which `0x000d0870` passes to the attach `0x0010cec0` when
a unit takes a weapon. Halo 1's units hold no marker names (its engine
uses fixed ones), so left empty the cyborg's
camera would sit at its origin (the feet in view, the view low) and its
weapons would have no marker to attach to. The disc test checks the cyborg's three fields name
markers of its render model.

**The health model per variant (Built, `boot::health`).** At each game
start the loaded map's name (`0x547898`) picks the model
(`health::model_for_map`): `h1_<map>` is Halo 1's when the Halo 1
sandbox is chosen for the game (`boot::h1_variants::set_mp_sandbox`
keeps `health::set_halo1_sandbox` in step), Halo 2's in the
Halo 2 sandbox; Halo 1's campaign level names (`a10` to `d40`) are Halo
1's. `H2_HEALTH_MODEL` overrides. The HUD meter is in the converted HUD
([health.md](health.md), "The health meter in the HUD").

**Halo 1's scenery, in both sandboxes (Built:
`convert::halo1::plan_scenery`, `fresh::put_scenery`).** The conversion
empties the base's placements and places Halo 1's trees, rocks and
shrubs instead: each Halo 1 scenery palette entry (`scnr +0x21c`, `0x30`) is converted
through `h1objects` (the `scen` from its definition) and every placement
(`+0x210`, `0x48`) on an entry that converted is placed: Halo 2's
palette entry, no name, Halo 1's "not placed automatically" bit, Halo
1's position and rotation, and the unique ID, type and source bytes
`h1campaign` writes. The palette (`scnr +0x58`, `0x28`) and placements
(`+0x50`, `0x5c`) go into the **fresh** build's scenario, which is a
built tree there and takes any size; the pinned build
can't grow the base's palette and places none. The Halo 2 sandbox
converts through `h1objects` too (`halo1::prepare_halo2`): its scenery
tags are added after the vehicle collections, which are added first.
`every_map_places_halo1_s_scenery_in_both_sandboxes`
(`h1objects/tests/sandbox_disc.rs`) checks every palette entry names a
`scen` with a render model and every placement keeps Halo 1's place:
Blood Gulch 50 placements on 19 entries, Battle Creek 30 on 9, none left
out.

**Placed scenery is solid as structure (Built:
`h1convert::scenery_collision`, `convert::halo1::merge_scenery_collision`).**
The engine gives scenery no Havok body, so a placed rock or tree drawn as
scenery is walked through ([objects.md](objects.md), "Collision for
Havok"). Retail Halo 2 builds its rocks into the structure, and so does the
conversion: in both sandboxes, every scenery placement there from the
start whose object has a collision model (decided by the `coll`, not the
name: Blood Gulch's boulders and trees among them) has that collision
merged into the level's collision BSP, and the object still draws as
scenery. "Collision" (above) has how.

**Object and scenery textures (Built, `textures::ModelTemplate`).**
Each Halo 1 model shader gets a Halo 2 `shad` copy: the base's first
`objects\` shader on `shaders\shader_templates\opaque\tex_bump`
(by name, so the pick is stable), its base map swapped for the Halo 1
shader's base map, its detail for the Halo 1 detail map (else the base's
`default_detail`) and its bump for `default_vector` (Halo 1 model
shaders have no bump). The maps come from `textures::h1_maps_of`, which
reads `soso` (base `+0xa4`, detail `+0xdc`), `senv`, `swat`, `smet`,
`schi`, `sotr` and `sgla`. `h1objects` converts each map once
(`h1bitmaps`) and names the copies in the model's materials;
campaign scenery goes through `h1objects` too, so it gets them as well.
On Blood Gulch's sandbox 17 models are converted and 81 materials draw
Halo 1 textures. A model shader that is not
`senv` or `soso` (glass, chicago, generic, meter, plasma, water) is
rebuilt first as the structure's shaders are (`shad::read_h1`, its own
blended, additive or self-illuminated stem and maps); the `tex_bump`
copy is what's left when that finds no stem or map (a10's cryo tube
glass, a `sotr` with no map at the offset `h1_maps_of` reads, would
otherwise draw as an opaque grey pane over the view from inside the tube;
`tests/model_shaders_disc.rs`). A transparent shader's map that is
a cube is its reflection, in `one_alpha_env`'s cube slot, and the least
tiled 2D map is its picture (`shad::transparent_maps`): the cryo glass
lists its reflection cube first. A `soso`
with a reflection (a cube map at `+0x164` and a brightness at `+0x144`
or `+0x154`, `shad::soso_reflection`) is rebuilt the same way on
`tex_bump_env` (the commonest retail object stem, 27 of Coagulation's
weapon, vehicle and character shaders) with its cube map and Halo 1's
tints facing the eye and grazing. Halo 1's weapons take their metal from
that reflection over dark base maps (the sniper rifle's averages 35 of
255); the tex_bump copy would draw them near black, most visibly in first
person. **The reflection mask.** Halo 1 masks a
`soso`'s reflection with its multipurpose map (`+0xbc`): every weapon
`soso` on Blood Gulch has one, and on this (Xbox) disc the mask is its red
channel (V: the sniper rifle's, DXT1 512x512, is red only, mostly dark:
the metal reflects in places). `tex_bump_env` has no mask map of its own:
the reflection is masked by the colour pass's alpha, the base map's times
the detail's (`shad::TEX_BUMP_ENV`). Halo 1's weapon bases are DXT1 with
no alpha, so without the mask the converted weapons would reflect everywhere
at full strength. A `soso` drawn on `tex_bump_env` takes a copy of its
base whose alpha is the multipurpose map's red
(`h1bitmaps::mask::red_into_alpha`, `Converter::masked_bitmap`), named
`<base>_reflection_<multipurpose leaf>` and built once per pair: DXT5, as
retail's weapon bases are (V: the battle rifle's and sniper rifle's on
Coagulation, 0x203), the base's colours re-encoded from its decoded
blocks (exact where a DXT1 block's endpoints are among its texels) and
its whole mip chain kept, the mask sampled at each level's size (nearest).
Every image of a multi-image base is masked, by the mask's image of the
same index, or by a single-image mask's only image
(`h1bitmaps::mask::convert_reflection_mask`; D: b30's `light fixtures`
and the grunt's maps come in two-image pairs with their `multi` maps). A
base or mask that does not decode, or images that do not pair, fail the
conversion. Proof: h1bitmaps
`a_base_takes_its_masks_red_as_alpha` and the block round trips;
`tests/model_shaders_disc.rs`, `bloodgulch_sniper_metal_reflects_its_cube_map`
(the sniper's converted base against its multipurpose red). **Detail
tiling and cut-outs.** A `soso` tiles its detail
map by its own scale (`+0xd8`, v times `+0xec`; `textures::scales_from`):
16 on the Warthog's hull and chain gun and the Scorpion's hull, 8 on the
Ghost's, 10 on the cyborg's armour. Tiling it once over the base would
lay the corroded metal over each hull as a few large dark smears (Run: the
Ghost's hull detail vector set to 8 and the Warthog's to 16 each turned
the flat panels into Halo 1's fine corroded metal). And Halo 1
alpha-tests a model shader's base unless its flags (`+0x28`) set bit 2,
"not alpha-tested", which no Blood Gulch `soso` does
(`shad::soso_alpha_tested`). Where the base stores an alpha (DXT3 and the
other alpha formats, `shad::h1_has_alpha`), the shader is rebuilt on
`tex_bump_alpha_test_single_pass`, its reflection dropped if it has one
(retail's `tex_bump_env_alpha_test` binds seven maps, which has no layout
here; retail's ammunition bags are alpha-tested without a reflection
too): the plants' and trees' leaves (DXT3, three quarters clear), the
ammunition bags and the weapon and ammunition decals, which would draw as
solid cards on the opaque copy. The stem alone leaves the cards whole: its
depth and shadow passes cut by the bump map's alpha, so the bump slot
holds a flat bump cut out where the base is
(objects.md "Cut-out model shaders"). Proof:
`a_model_shader_tiles_its_detail_by_its_scale`,
`a_model_shader_cuts_out_where_its_base_is_clear`, and
`tests/model_shaders_disc.rs`,
`bloodgulch_leaves_cut_out_and_details_tile_by_their_scale`. Not converted:
a `soso`'s self-illumination (multipurpose green times its animated
colour, `+0x70` to `+0x8c`), its detail mask (`+0xd6`) and the reflection of a
cut-out one. A `soso`'s change colour (`+0x4c`, multipurpose
blue) draws with Halo 2's change-colour stem
(objects.md "Change colours"). A player biped also carries the player's
emblem (models.md "The emblem").

**Additive chicago chains and the view-angle fade.** Blood Gulch's teleporter receivers (the three
`scenery\teleporter_base\teleporter_base` placements; the senders are
in the bases' structure) draw a light cone,
`scenery\teleporter_base\shaders\teleporter_cone` (`schi`, additive,
two-sided, framebuffer fade mode 2, "fade when parallel"): three maps,
each map's colour function (`+0x2c`, 2 multiply) multiplying it into the
next, the grey noise `base volumetric dust` twice, each scrolling at its
own rate (u 6 s, v −20 s; u −4 s, v 15 s), times `teleporter_dust`, a
32×32 green ramp black at the top (v ×1.05). Drawn as its least tiled map
alone (the first dust), the cone would be a static grey speckled shell at
full brightness, back faces culled, its silhouette hard and none of
Halo 1's green. An additive `schi` or
`scex` whose maps all multiply (every map but the last on function 2 or
3, `shad::multiply_chain`) is rebuilt on `two_add_clouds` (Coagulation,
Beaver Creek, 05a; `rgb = t1 × t2 × 2 × v0.rgb × c0.a × v0.a`, blend
one/one, two-sided): its first scrolling map on bitmap 0, which the
clouds tree scrolls (`shad::CLOUDS_TREE`, as retail's `cr_stream`), its
first still map on bitmap 1, and every other map folded into the tint
as its mean colour (a scrolling layer averages to it), halved for the
stem's doubling (`shad::Chain`). Without the stem, `one_add_illum` draws
the still map with the rest folded in. Every additive transparent shader
also fades as Halo 1's fade mode says (`+0x2e`; `shad::Tint`): the
stems' vertex shader (`transparent_generic_reflection`) blends the
perpendicular colour and alpha (`v[10]`, the surface facing the eye)
into the parallel ones (`v[11]`, grazing), and the pass multiplies its
colour by both; retail's sun volumes and headlights use it the same
way. Fade when parallel is alpha 1 facing the eye and 0 grazing, so a
cone's silhouette dissolves; fade when perpendicular the reverse. D
(every model shader on the disc): 61 `schi` and 125 `sotr`; the chains
are seven, all fade when parallel: the teleporter cone, a50's
`volumetric light cone small` and `volumetric light cone ground`, b40's
`b40_foglamp`, the fighterbomber's `afterburners` and the pelican's
`hover thrusters` and `jet thrusters`; 8 `schi` and 16 `sotr` fade when
parallel, one `sotr` when perpendicular. Proof: `shad` unit tests
(`a_multiply_chain_draws_its_first_scrolling_map_and_first_still_one`,
`the_tint_and_scroll_land_in_the_stems_vectors_and_tree`,
`the_fade_mode_sets_the_facing_and_grazing_alphas`) and
`tests/shad_rebuild.rs`,
`additive_chicago_chains_draw_two_maps_scrolling_and_faded`. **Plasma
`sotr`.** The receiver also draws
`scenery\teleporter_shield\shaders\teleporter_shield` (`sotr`,
additive, fade when parallel): seven combiner stages over four maps,
`energy plasma` twice (scrolling u 10 s backwards and v 6 s; u 11 s and
v 7 s backwards), a still energy mask and the still shield picture,
whose alpha is a rounded square. It is one of Halo 1's plasma programs:
the difference of two scrolling noise samples, squared and sharpened
into thin moving bands, coloured and masked by the picture, the same
idea as Halo 2's `plasma_alpha` (retail's own teleporter and needler
shaders: `r0 = t1 − t2`, squared ×4 and `bx2` squared, coloured between
colour 1 and colour 0, times the mask's alpha and the fade). Drawn as its
least tiled map alone on `one_add_illum`, it would be `energy plasma`,
white, added over the base as a blown-out white square. A `sotr` is a plasma
(`shad::is_plasma`) when it is additive with seven stages over four 2D
maps, the first two the same bitmap and both scrolling, the last two
still; it is rebuilt on `plasma_alpha` (`shad::plasma_shader`): the first
noise on bitmap 0, the second on bitmap 1 scrolling as Halo 1's map 1
(the clouds tree, as retail's `needler_ammo_glow` animates it), the
picture on bitmap 2 as the mask, both band colours the picture's mean
colour, colour 2 black and colour 3's alpha 0 as on every retail
instance, and Halo 1's fade in vectors 7 (`v[10]`, facing) and 6
(`v[11]`, grazing): retail's teleporter plasma fades when parallel
too. D (every `sotr` on the disc): thirteen match, on four stage
programs: the teleporter shield, the jackal's and the field generator's
shields, the overshield and active camouflage powerups, the dropship's
tractor beam, the needler's core and ammo glow, the gun turret's and
c20 index platform's beam tendrils, a50's prison energy door and shuttle
bay doors, and b40's structure `b40_shaftenergy`. Of the 125 `sotr`
models draw, 12 are plasmas; none of the other 113 matches (D, by
framebuffer blend and stage count): additive with 0 stages 18, 1 stage
24, 2 stages 15, 3 stages 12, 4 stages 4 (chillout's hilltop, b40's holo
halo ring, a10's shield charge, Cortana); alpha-blended (blend 0) with
0 to 3 stages 22, 4, 5 and 1; blends 1, 6 and 7 with 0 to 2 stages 1,
6 and 1. The a10 shield charger's four-stage rebuild and its device colours
are described in [the role audit](role-audit.md). The other non-plasma
programs in this inventory keep their single-map rebuild. Lost on a plasma: the energy mask
(map 2), Halo 1's stage constants (the teleporter's teal base glow,
added under the bands), and the first noise's own scroll (retail's
teleporter animates both pairs; that tree isn't verified here). Proof:
`a_plasma_is_seven_stages_over_one_noise_twice_then_two_still_maps`,
`a_plasma_scrolls_its_second_noise_and_fades_through_vectors_7_and_6`,
and `tests/shad_rebuild.rs`,
`plasma_sotrs_draw_on_plasma_alpha_and_no_other_sotr_does` (prints what
every `sotr` becomes). Which instance colour reaches which combiner
constant is read from the retail colour values, not the pass's
bindings (colours 2 and 3 are written the same, alpha 0, which makes
the order of those two moot: one is stage 5's, which reads only alpha,
the other stage 6's, which reads only colour).
Retail `plasma_alpha` scrolls either noise: `needler_ammo_glow` the
second (the clouds tree, bitmap target 1, `c[22]` and `c[23]`),
04a's `glass_gravunit_glow` the first (bitmap target 0, binding words
`0x01d2` and `0x01f3`: `c[20]` and `c[21]`).

The conversion's face-on brightness against Halo 1's, per channel, is checked by
`teleporter_receiver_brightness_against_halo1`
([notes/multiplayer.md](notes/multiplayer.md)).

The shield's last stage (`teleporter_shield` stage 6, and every plasma
but the jackal's and the generator's shields) adds `picture.rgb × (band
+ c0)`. The band is 1 except along thin dark lines (the noises' squared
difference `s`, expanded and squared, then `x(2 − x)`); `c0` runs from
its lower to its upper bound by the object's A output (stage flag bit
2, `shad::STAGE_A_OUT_COLOR0`: set on the plasma stages alone, whose
bounds differ), and `teleporter_base` has no functions, so the glow is
the lower bound, (0, 0.329, 0.424). `plasma_alpha` adds `mask.a ×
(colour 0|1 × s + colour 2|3 × q)`, `q` the stem's own band (`s`
expanded and squared three times: 1 but for thin dark lines), so the
conversion (`shad::PlasmaLight`) binds the picture with its brightness
as alpha (`h1bitmaps::mask::brightest_into_alpha`: DXT5, alpha `gain ×
max(r, g, b)`, `<picture>_brightness_<gain × 1000>`), colours 2 and 3
the lit band `hue × (1 + glow) / gain` and 0 and 1 the glow `hue × glow
/ gain`, the gain `1 + glow` in the hue's brightest channel (1.329 for
the teleporter's green), so the stem's 8-bit colours keep Halo 1's
headroom and its clamp meets Halo 1's. The red and blue peaks differ from Halo 1's because the stem has one hue
where Halo 1 colours each texel its own. Lost: a
glow an object's A output animates (the jackal's shield) holds at its
lower bound; a plasma whose last stage is another sum (the jackal's and
the generator's shields add `v1 × (1 − picture.a)`) gets no glow.

A model material with a null base uses the matching H1 shader rebuild
(role-audit.md). An unsupported material fails conversion with the map
and source shader name. The sky model's
materials name copies of the base's sky shaders drawing Halo 1's sky
bitmaps at Halo 1's scale (`h1convert::sky_shaders`, sky.md "The
model's shaders"), or the base sky's own shader where one doesn't convert
(`tests/sandbox_disc.rs`).

A material never names no shader (NONE). The shader draw begin
`0x1bbf0` does not skip a NONE shader: at `0x1bcb9` it only clears its
cached words `0x485a64..0x485a70`. It still calls the draw
`0x16b90(0x485b48, NONE, ...)`, which reads tag slot `0xffff`'s entry (past
the table) as a `shad` and dereferences its compiled postprocess
(`0x16bc7`, `0x16bde`). A converted sky whose materials name NONE is the
likely cause of a null read on Blood Gulch's first frame (pc `0x16bde`,
ebp `0x485b48`, the state cache).

### Starting weapons (**Built**)

A game variant's starting weapons are bytes (primary `+0xd6`, secondary
`+0xd7` of the variant; `variants` settings 89 and 94). `0x0019eda0`
turns one into a weapon: it jumps on the byte minus 1 through the table
at `0x0019eee8` to a read of one entry of the multiplayer globals' weapon
list (globals `+0x168` names the `mulg`; its `+8` block's first element,
`+0x68`, 8-byte tag references). Byte 1 gives none, 2 a random byte in
3..=18, and bytes 3..=20 each one entry (E; the table is
`h1convert::globals::STARTING_WEAPON_ENTRY`):

| Byte | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Entry | 3 | 0 | 1 | 6 | 7 | 5 | 2 | 4 | 12 | 9 | 10 | 13 | 11 | 15 | 14 | 8 | 16 | 17 |

The list is Halo 2's (Coagulation: magnum, SMG, shotgun, battle rifle,
sniper rifle, rocket launcher, plasma pistol, plasma rifle, brute plasma
rifle, needler, carbine, none, brute shot, beam rifle, energy sword,
rocket launcher, sentinel beam, none). The Halo 1 cyborg's graph has no
Halo 2 weapon type, so the default variants' SMG is refused
(docs/halo1/objects.md, "Weapon class and type") and players would spawn
unarmed. In the Halo 1 sandbox every entry naming a weapon names a
Halo 1 one (`sandbox::HALO1_WEAPON_LIST`, `convert::halo1`), the first of
its choices the map has:

| Halo 2 weapon | Halo 1 weapon | Why |
|---|---|---|
| magnum | pistol | the sandbox table reversed |
| SMG | assault rifle | reversed |
| battle rifle | assault rifle | the precision row reversed |
| shotgun, sniper rifle, rocket launcher, plasma pistol, plasma rifle, needler | the same | reversed |
| carbine | pistol | **D**: Halo 1's mid-range precision weapon |
| beam rifle | sniper rifle | **D**: long-range precision |
| brute shot | rocket launcher | **D**: explosive launcher |
| brute plasma rifle | plasma rifle | **D**: automatic Covenant plasma |
| energy sword | plasma rifle | **D**: Halo 1 has no melee weapon; nearest close-range Covenant one |
| sentinel beam | plasma rifle | **D**: continuous Covenant beam |

**Byte 0, the map's default** (64 of the 78 starting weapon bytes of
mainmenu's built-in variants, Slayer among them): `0x0019ef40` reads the
scenario's starting equipment (`scnr +0x128`, `0x9c` each; entries whose
game type `0x0015a090` accepts) and rolls its five `itmc` references
(`+0x3c`, 8 apart; `0x0019efef` -> `0x0019f1a0`) for the default (E). The
Halo 1 sandbox reads the original scenario's starting equipment at
`+0x390`, including the flags, four game-type selectors, collection order
and permutation weights. Blood Gulch has an assault rifle and pistol in
its CTF row and a plasma pistol in its other row. Both H2 bases have a
single SMG collection, which does not supply H1 starting weapons.

A weapon the map lacks falls to the next choice (the needler to the
plasma pistol, the plasma rifle, then the assault rifle, which every Halo 1
multiplayer map has). V: `tests/sandbox_disc.rs`
(`every_starting_weapon_is_held_by_the_halo1_player_graph`): on every
map, every byte the table maps, each built-in variant's primary and
secondary, and every weapon of the starting equipment's collections, is a
weapon the converted player biped's graph holds in mode `combat`.

### Design notes

The Halo 1 sandbox's `weap`, `proj`, `bipd`, `vehi`, `eqip` and their
`mod2`/`coll`/`phys`, `antr` and effects are converted per
[campaign.md](campaign.md) §2, with Halo 2's physics. The health subsystem of campaign.md §6 is shared with
the campaign: no body regeneration, health packs refill on pickup, a
health meter under the shield meter, on for Halo 1 maps. Health packs
are placed in both asset settings.
Hooks (docs/halo1/health.md): the game start sets the model from the map's
name; the converted cyborg's HUD (`nhdt`) has a widget on the body
(input 17), built by `h1objects::health_hud`. A health pack
is an `eqip` with `+0x12c` = 5 (i16) and `+0x130` > 0; retail Halo 2
already handles that powerup type, so no hook is needed.

### Pickup HUD text (**Built**)

The Halo 1 sandbox's weapons show their source name and silhouette in
the pickup and swap prompts. Halo 2 sandbox weapons use the retail
HUD strings. In both sandboxes, automatic Halo 1 power-up pickups show
their source pickup notification. The scenario's `hmt ` ties the
rebuilt items to the existing HUD lookup. See [the disc and XBE
evidence](text.md#multiplayer-pickup-text).
