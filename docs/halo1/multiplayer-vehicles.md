# Halo 1 multiplayer: vehicles and what is not placed

Vehicles and the Halo 1 objects the converter leaves out, split from [multiplayer.md](multiplayer.md).

| Section |
|---|
| [Vehicles](#vehicles) |
| [What isn't placed, and why](#what-isnt-placed-and-why) |

Part of [multiplayer.md](multiplayer.md).

## Vehicles

Halo 2 spawns multiplayer vehicles from netgame equipment, not from the
scenario's vehicle block (which is empty on every retail map but
Headlong's seven placements with no palette). An entry names a vehicle
collection (`vehc`, `multiplayer\single_vehicles\<role>`: one entry, the
`vehi` and its model variant) and carries a type, the vehicle's class
(V, every retail and Map Pack map): 1 warthog and gauss warthog, 2 ghost,
3 wraith and scorpion, 4 banshee, 6 and 7 turrets. The spawner hands the
type to the game variant's vehicle options (`+0x209 + type - 1`, E
`0x0015a4d0`), which keep the vehicle, swap it for another of its class
or remove it; types 3 and 5 go to the heavy chooser (E `0x0015a250`:
wraith or scorpion, variant `multiplayer`). Retail maps also hold typed
entries **with no item**: a variant's vehicle option fills them. That is
how Coagulation gets a scorpion: it has the scorpion `vehi` and two empty
type-3 entries, but no scorpion collection.

**Every retail multiplayer map holds every vehicle tag** (banshee, ghost,
scorpion, spectre, warthog, wraith, both turrets). Only the collections
differ:

| Map | Vehicle collections |
|---|---|
| Coagulation | warthog, ghost, banshee |
| Waterworks | banshee, ghost, warthog, wraith, h_turret_ap |
| Headlong | warthog, warthog_gauss, ghost, h_turret_ap, banshee |
| Zanzibar | warthog_gauss, warthog, ghost, h_turret_ap |
| Burial Mounds | h_turret_ap, warthog, ghost |
| Ascension | banshee |
| Foundation | h_turret_ap |
| Beaver Creek, Lockout, Colossus, Ivory Tower (cyclotron), Midship | none |
| Map Pack: Containment | h_turret_ap, warthog_gauss, scorpion, banshee |
| Map Pack: Relic (dune) | warthog, ghost, h_turret_ap |
| Map Pack: Terminal (triplicate) | h_turret_ap, ghost, wraith, warthog_gauss |
| Map Pack: Sanctuary (deltatap) | h_turret_ap |
| Map Pack: Turf | warthog |
| Map Pack: Backwash, Elongation, Gemini, Warlock | none |

The Map Pack maps read with the same tools (`xiso` entries under
`/demos/dlc/0000000N/`); `h2 tags <map> vehi --disc` looks in `/maps`,
which the Map Pack disc doesn't have. Containment (snow, and the only map
with a scorpion collection) would suit Sidewinder or Ice Fields, but a
base must be on the player's disc, so the bases stay retail.

So a converted vehicle's collection is the base's when it has one, and
**otherwise a built one** for the base's own `vehi`, in the slot of a tag
the conversion frees (below): weight 1, the `vehi`, the variant
(`gauss` for the gauss warthog, `multiplayer` for the scorpion and wraith,
as retail collections name them; none otherwise), and the `+0x8` word of
one of the base's collections. Every netgame equipment entry gets
its class's type (weapons 0, grenades 8, powerups 9, vehicles above). The rocket
warthog becomes the gauss warthog (the user's decision). Blood Gulch and
Sidewinder place all 8 of their vehicles (2 warthogs, 4 ghosts, 2
scorpions; the Xbox maps have no rocket warthog).

**Built. Multiplayer vehicle placements.** In the Halo 2 sandbox, a vehicle
with no counterpart uses the Halo 1 object converter. Its netgame entry
names a built `vehc` whose sole weighted entry references the rebuilt
`vehi`. Repeated placements share that collection and retain their own
position and yaw. The source render-model depth raises the origin by the same amount
as in the Halo 1 sandbox. The H2 spawner checks for a collection at
`0x15ab12..0x15ab23`. Source controller types choose the H2 game-variant
slot for a rebuilt vehicle: tracked uses heavy, wheeled and hover use the
two light slots, aircraft use flying, and stationary uses primary turret.
The retail maps place eight Ghosts, four Warthogs and four Scorpions.
The disc regression renames the Blood Gulch Ghost in memory to check the
unmatched path through fresh-map assembly and read the resulting vehicle
and its placements back.

**Added tags.** A built vehicle collection is added after the base's
tags (`TagSet::add_tag`, which keeps the pinned index's tail); its datum
is known when the placements are written (slot `n`, salt `0xe174 + n`,
`convert::halo1::added_datum`) and checked when the map is built. No base
tag gives up its slot (`tests/disc_maps.rs` checks every built collection
and the index size). The converted lightmap still takes the base's own
lightmap bitmap's slot: it replaces that bitmap, whose pixels would
otherwise stay in the map unused.

## What isn't placed, and why

`h2 h1 convert` reports race flags and the Halo 2 sandbox's omitted ammo
pickups with their positions.

**Built in both sandboxes.** Halo 1's overshield, camouflage and health
pack collections are rebuilt from `itmc`. The spawn points, facing,
weights, game-type filters and respawn times come from Halo 1. Blood
Gulch's shield-invisibility collection can still roll either item.
Beaver Creek's `powerup_super_shield` and Zanzibar's
`powerup_invisibility` establish the Halo 2 layout (V). Each uses a
type 9 netgame equipment placement and a collection referencing an
`eqip`. The game reads those references at `0x0015ab12` and selects
by weight at `0x0019f1a0` (E). The normal import uses
`prepare_halo1` or `prepare_halo2`, both supplied with the object
converter. The direct base-only `prepare` reports missing collections.

The ignored test `bloodgulch_native_powerups_spawn_in_both_sandboxes`
reads both discs and checks all four placements and their rebuilt item
and collection data in the fresh maps.

The retail disc test checks each material against its rebuilt shader and
reads back the diffuse and lightmap coordinates. It checks the images
through the Halo 2 decoder against the Halo 1 source.

Lightmap pages (clusters): beavercreek 25
(29), bloodgulch 30 (30), boardingaction 41 (41), carousel 36 (36),
chillout 10 (10), damnation 12 (12), hangemhigh 1 (1), longest 1 (1),
prisoner 1 (1), putput 9 (9), ratrace 14 (16), sidewinder 32 (35),
wizard 18 (18); a cluster with nothing lit has no page. Per map (placed / Halo 1; items include
vehicles, which Halo 2 places as items):

| Map | Base | Triangles | Spawns | Flags | Items | Vehicles | Left out |
|---|---|---|---|---|---|---|---|
| beavercreek | beavercreek | 3317 | 70/70 | 88/97 | 28/35 | 0/0 | 11 race, 4 health, 3 camo |
| bloodgulch | coagulation | 5503 | 71/71 | 172/191 | 39/43 | 8/8 | 21 race, 2 health, 1 camo, 1 overshield |
| boardingaction | beavercreek | 7142 | 73/73 | 66/77 | 43/52 | 0/0 | 13 race, 7 health, 2 camo |
| carousel | beavercreek | 2940 | 63/63 | 71/79 | 33/37 | 0/0 | 10 race, 1 health, 3 camo |
| chillout | beavercreek | 3108 | 58/58 | 39/49 | 27/33 | 0/0 | 12 race, 4 health, 2 camo |
| damnation | beavercreek | 8618 | 67/67 | 77/79 | 41/46 | 0/0 | 4 race, 4 health, 1 camo |
| hangemhigh | beavercreek | 3218 | 67/67 | 83/92 | 41/45 | 0/0 | 11 race, 2 health, 2 camo |
| longest | beavercreek | 2319 | 71/71 | 48/51 | 35/40 | 0/0 | 5 race, 4 health, 1 camo |
| prisoner | beavercreek | 5234 | 58/58 | 40/47 | 45/51 | 0/0 | 9 race, 5 health, 1 camo |
| putput | beavercreek | 10494 | 59/59 | 127/134 | 26/40 | 0/0 | 9 race, 13 health, 1 camo |
| ratrace | beavercreek | 2692 | 48/48 | 41/43 | 27/30 | 0/0 | 4 race, 2 health, 1 camo |
| sidewinder | coagulation | 10036 | 76/76 | 147/166 | 50/55 | 8/8 | 21 race, 2 health, 1 camo, 2 overshield |
| wizard | beavercreek | 2972 | 48/48 | 57/59 | 36/44 | 0/0 | 4 race, 4 health, 4 camo |

Flag counts don't add up one for one: a Halo 1 flag can become several
Halo 2 flags (`placements::h2_flags`). Danger Canyon, Death Island,
Gephyrophobia, Ice Fields, Infinity and Timberland aren't on the Xbox
disc.

**Weather is not converted.** Halo 1's rain and snow (`rain` particle
systems in the BSP's weather palette) have a Halo 2 home: every `weat`
reference on the disc is at `+0x20` of a `0x88`-byte weather palette
element, in the structure at `sbsp +0x84` (50 references) or the
scenario at `+0x260` (12), and a cluster picks its entry at `+0x76`
(`weather_systems_are_named_by_the_weather_palettes`,
`h1convert/tests/h2_scenario.rs`; E for the structure fields, table
above). What stops it: Halo 2's `weat` is opaque
([lighting.md](../formats/lighting.md): its particle simulation and
render blocks are unreviewed), so a Halo 1 system can't be written into
one, and pointing the palette at a retail `weat` would be Halo 2's
weather, not Halo 1's. The structure side is also the render lane's
(`structure.rs`). The palette stays empty and each cluster `-1`.
