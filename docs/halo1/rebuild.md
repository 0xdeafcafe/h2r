# Halo 1 objects rebuilt from definitions (`h1objects::rebuild`)

Status: Built, not run in the game. How the Halo 1 converter writes scenery,
vehicles, bipeds, devices, physics models, effects, weapons and particles
from the typed definitions of [tag-definitions.md](../formats/tag-definitions.md)
instead of patching a Halo 2 template. The sections below keep their
evidence labels: **E** engine code, **V** checked on the owner's disc, **D**
a naming or interpretation choice (labels of
[map-format.md](../formats/map-format.md)). Dated game runs and the
disc measurements behind them are in [notes/rebuild.md](notes/rebuild.md).

## Scenery from definitions (`h1objects::rebuild::scenery`)

Every Halo 1 scenery object becomes three tags written from the
definitions (`rebuild::scenery`, `Converter::scenery`), with no retail
template read:

- **`scen`**: `default_record`, then the object type (6), Halo 1's
  bounding radius and offset, the model, the predicted resources (the
  converted `mode` as kind 2 index -1, then every converted bitmap its
  shaders draw as kind 0, as devices and vehicles), the change colours,
  and the attachments and creation effect
  ([Object attachments](objects.md#object-attachments)).
- **`hlmt`**: the render, collision and animation models, no physics
  model, the nodes (`h1models::mode::hlmt_nodes`) and the regions with the
  collision and physics region and permutation of each (`region_map`).
- **`mode`**: converted by `h1models::mode`, written from zero, its
  **material list** rewritten from the definition: one material per
  Halo 1 shader, the shader set or null, everything else at its default.

**Physics.** Converted scenery has no `phmo`; its `coll` keeps one BSP
physics slot per BSP, as retail scenery with no `phmo` does, and each
placement's collision is merged into the level's instead
(`h1convert::scenery_collision`, docs/halo1/multiplayer.md, "Collision").
Retail holds both forms (V: 05a 11 scenery with collision and no `phmo`,
5 with one; beavercreek 6 and 3; coagulation 0 and 6). The engine never
gives scenery a Havok body, so a `phmo` on scenery would be inert. E:
the only place an object gets a physics component and rigid bodies from
its `phmo` is `0x001c35f0` (it calls `0x001cf0b0` for the component at
`0x001c3667`, then `0x001cf120`), and it returns at `0x001c361c` unless
the object's type (object table entry `+3`) is in the mask `0x1883`:
biped 0, vehicle 1, machine 7, crate 11, creature 12. Scenery (6) never
passes. `0x001cf120` itself switches on the type (byte table
`0x001cf20c`, jump table `0x001cf1f4`) and has cases for exactly those
five; every other type's index is 5, whose jump-table entry is zero. The
other callers of `0x001cf120` (`0x000c54d1` for units, `0x000f1053`,
`0x001c37d2`, `0x001d3ffb`) rebuild a component the object already has.
Rigid bodies are built in two places only, the level's at `0x001c3c32`
and an object component's at `0x001d72c2`. Run: a converted Blood Gulch
lists no Havok body for any scenery placement and a player put at a
boulder's middle drops through it ([notes/rebuild.md](notes/rebuild.md),
"Scenery has no Havok body").

Retail does not rely on scenery for solid props either. V, coagulation
(Blood Gulch's base): no tag on the map names a rock or a boulder, so
its rocks are structure; its scenery are the flag and teleporter bases,
bushes and trees; and its one solid placed prop, the `powerhaus`, is a
crate (`bloc`) whose `phmo` body holds motion `5` (Havok motion 7,
fixed), flags 0, on node 0. A converted Halo 1 scenery object with solid
collision is therefore solid to bipeds and vehicles as structure
(instanced geometry merged into the level's collision); a crate with a
fixed body is not built.

**Proof, V** (`tests/rebuild_disc.rs`):

```sh
H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso \
  cargo test --release -p h1objects --test it -- rebuild_disc:: --skip unit_rebuild_disc:: --ignored --nocapture
```

- `every_scenery_rebuilds_from_its_definition`: every scenery object
  (the ten levels on 05a, the multiplayer maps on coagulation and
  beavercreek) converts; `scen`, `hlmt` and `mode` read back through the
  definitions, write the same again, pin nothing, and name only converted
  tags. Every scenery shader belongs to the tree; no report holds a
  template stopgap; the type, bounds, model chain (collision and
  animation exactly where Halo 1 has them) and region count are Halo 1's;
  every `coll` has a BSP physics slot per BSP; every model is predicted
  first, its bitmaps after.
- `retail_scenery_has_bsp_physics_and_predicts_its_model`: the retail
  facts above (both physics forms, the model predicted first, kind words
  `0xffff0002` for `mode` and `0` for `bitm`).

## Vehicles with no template (`h1objects::rebuild::vehicle`)

A Halo 1 vehicle is written from its `vehi` definition whether or not a
Halo 2 vehicle resembles it. a10 has five with no Halo 2 counterpart: the
cryotube, the pilot and pod chairs, the fighterbomber and the Halo ring.
The scripts seat the player in the cryotube and open it as the opening
cinematic ends (`unit_open cryotube_1`); a name left unplaced would fault
there, which `h1scripts::compile::NONE_UNSAFE` guards against for any
unplaced name.

- **Object header**: object type 1, Halo 1's bounding radius and offset,
  the model, and the predicted resources: the converted `mode` (`+0` kind
  2, `+2` -1), then every converted bitmap its shaders draw (kind 0, index
  0). V: every retail vehicle on the disc lists some, its `mode` once in
  that form and first, its bitmaps in that form.
- **`hlmt` and `mode` materials**: as rebuilt scenery (above). Collision is
  Halo 1's converted `coll`; the animation graph Halo 1's converted
  `antr`; the physics model built from Halo 1's collision (below).
- **Physics model**: `vehicle_new` (`0x000eeb80`) returns false when the
  model's `hlmt +0x20` physics model is null (`cmp [hlmt+0x24],-1` at
  `0x000eebcb`), and `object_new` then deletes the vehicle, so every
  vehicle has one. It is Halo 1's collision as Havok shapes (Physics
  models from Halo 1, below): one rigid body on the object (node -1)
  holding every collision node's solid cells, placed by the model's
  default pose, of Halo 1's `phys` mass. A vehicle with no solid collision
  (a10's fighterbomber and Halo ring have no `coll`) takes the stand-in
  from `rebuild::sphere_physics`: one body (node -1), one 0.05 world unit
  sphere of 1000 kg on the origin, one material and one region.
- **Vehicle type** (`vehi +0x1f0`, named below): Halo 1's own type, read
  from its `vehi`. `ROLE_VEHICLE_TYPE` (turret, Halo 2's stationary
  vehicle) is the type written for a unit with no Halo 1 vehicle type of
  its own. Turrets hold no wheels, treads, hover points or engine raw
  (`+0x268`, `+0x2e8`, `+0x2f0` empty on every one), and their one
  physics element (`+0x2f8`) is zero but for four `-1` halves
  (`TURRET_PHYSICS_ROLE`, written when a vehicle has no powered mass
  points).
- **Seats** (`+0x1c8`): one per Halo 1 seat (Halo 1's `unit +0x2e4`,
  `0x11c` each, V on a10, b30 and c10: label `+4`, marker `+0x24`, camera
  marker `+0x84`, each 32 characters). The Halo 2 seat takes Halo 1's
  `label` (so a10's `CT-driver` is the text its scripts name; the seat
  table matches a label equal to the script's text), the marker at `+8`
  and the camera marker at `+0x60` (D: retail's values there are marker
  names, `driver`, `gunner`, `camera_rider`).
- **Entry words** (`TURRET_SEAT_ROLE`): a Halo 1 seat has no entry
  distance or angle, and the engine admits no rider at zero
  (`0x000c9040`), so every seat takes the one set of entry radius, cone
  angle, facing angle and maximum relative velocity that retail's turret
  seats hold, and `-1` for the invisible seat region.
  `tests/unit_rebuild_disc.rs`, `turret_role_is_the_retail_turrets_majority`,
  recomputes the table from every distinct retail turret.
- **Everything else** is the definition's default: zero, null, empty.
  Blocks left empty that some retail vehicles fill: the unit's camera
  tracks and HUD, the seats' HUD, damage. A Halo 1 vehicle with no seats
  (a10's Halo ring, b30's falling box) has none in Halo 2 either; three
  retail vehicles on 06a_sentinelwalls have none, so
  `every_campaign_level_converts`' sweep accepts an empty `vehi +0x1c8`.
  Halo 1's seat tuning that has no established Halo 2 home is not carried
  (listed in the object report).

Fields named for this (V, the disc proof below round-trips all 71 retail
vehicles byte-identical with no pinned values):

| Field | Evidence |
|---|---|
| `vehi +0x1f0` `vehicle_type` (`enum16`: tank, jeep, type_2, dropship, hover, flier, turret) | E: objects.md (vehicle type). V: the names by the retail vehicles holding each value: 0 scorpion, 1 warthog, 3 pelican, phantom and gravity throne, 4 ghost, wraith, spectre and creep, 5 banshee, 6 the turrets; none holds 2 |
| `vehi +0x1c8` seat `label` | V: the scenario's seat-mapping masks select seats by it (`h1scripts::seats`, on 05a) |
| `vehi +0x284` `suspension_sound`, `+0x294` `foot_ref_294` | V: objects.md lists snd! and foot here; the group word is `snd!`/none and `foot` and the datum null on every retail vehicle, so the schema could not see them. As unknown spans a fresh tag held zero, which names tag 0 |

Proof:

```sh
cargo test --release -p h1objects --test it unit_rebuild_disc:: -- --ignored --nocapture
cargo test --release -p h1campaign --test disc a10_cryotubes -- --ignored --nocapture
```

- `a10_vehicles_without_templates_are_rebuilt`: all five a10 vehicles
  convert; each reads back through the definition and writes the same
  tree; nothing is pinned; the predicted resources are the tree's `mode`
  then its bitmaps; the model chain (`hlmt`, `mode`, `coll`,
  `jmad`) is in the tree; the `hlmt` has the model's nodes; one seat per
  Halo 1 seat with its label and marker; the one physics element; the two
  null references null.
- `a10_cryotubes_are_placed_and_built_from_their_definition` (pinned and
  fresh): `cryotube_1` and `cryotube_2` name vehicle placements whose
  palette entry is the cryotube; read back out of the converted map it
  decodes and encodes byte-identical, its `hlmt`, `mode`, `coll` and
  `jmad` are tags in the map, and it has one seat, `CT-driver`, at marker
  `driver`.

What to watch in the game: the player seated in the cryotube during
a10's opening, and getting out when the scripts open it. The first
suspects if that goes wrong: the physics model (a dynamic body: a
vehicle whose shape is not on a floor may drop or drift); and
the rider's pose, since retail seat labels
(`warthog_d`) also read like the riders' animation mode names (D) and the
player's graph has no `CT-driver` mode.

## Every vehicle from definitions (`h1objects::rebuild::vehicle`)

Every Halo 1 vehicle is written by `rebuild::vehicle` from the `vehi`
definition. There is no vehicle template path: `convert::TEMPLATES` has no
`vehi` row.

- **From Halo 1**: the object header (type, bounds, model chain,
  predicted resources), the physics model from Halo 1's collision (as
  above; a stand-in sphere with none), the camera field of view, the
  **vehicle type** (Halo 1 `vehi +0x2f4`, `u16`), the **flags'** low half
  (Halo 1 `+0x2f0`), the seats, the unit's weapons, the powered mass
  points, the drive speeds and accelerations (cache values per tick
  scaled by 30 and 900 into seconds, `rebuild::vehicle_with_tuning`), and
  the ground contact, gravity and slope words of Halo 1's `phys`.
- **Seats**: one per Halo 1 seat (`rebuild::seat_names`). The label is
  the rider's animation mode, Halo 1's label as the converted biped
  graphs name it (`h1models::names::mode_name`: `W-driver` is
  `warthog_d`, `GT-gunner` `guntower_d`, an unlisted label lower case
  with `-` as `_`), so the rider finds its seat animations. The marker
  (`+8`), entry marker (`+0xc`, `<marker> enter` when the model has
  one) and camera marker (`+0x60`) are the model's marker names as the
  converted `mode` keeps them (Halo 1's, spaces included). The scripts
  keep Halo 1's text: `h1scripts::seats` matches a label equal to the
  text's mode.
- **Seat flags and boarding** (`+0`; `rebuild::seat_flags`). The
  engine's enter-seat search (`0x000c8bb0`) skips a seat whose flags
  close it to the rider (bit 16 for a unit with a player, bit 17 for any
  other), one needing a driver (bit 9) while the vehicle has none, and
  treats bit 11 as a boarding seat (`0x000c92c0`: entered only to take
  the seat its `+0x3e` names). It then needs the entry marker (`+0xc`,
  `0x000c6fb0`) and the rider's marker (`+8`, `0x000c7160`) on the
  model (through `0x000b8d30`: an empty name is the object's origin, a
  missing one fails), the rider within the seat's distances and angles
  (`+0x98`, `+0x9c`, `+0xa0`, `+0xa4`, `0x000c9040`; zero admits no
  one), and the rider's graph to hold the label as a mode with `enter`
  or `enter_a`..`enter_d` (`0x0010f9b0`, the string IDs at
  `0x00467430`). A seat's flags are Halo 1's bits 0..10 and nothing
  else: no boarding bit, no bit 16 or 17, no bit 15 and no player-gunner
  bit. A marker the model lacks is written empty, the origin (a10's
  fighterbomber and the dropships name a `driver` their models do not
  have). V: on 05a bit 11 is set on exactly the retail seats labelled
  `_b_`; Halo 1 sets no bit above 10; on the eight pairs Halo 1's bits
  0..10 equal the counterpart seat's on most seats and the driver bit on
  every one (`seat_flags_match_halo_2s_counterparts`).
  `every_seat_passes_the_engines_boarding_rule` applies the rule to
  every seat of every vehicle on the ten campaign levels and both
  sandboxes, for the converted Halo 1 player and, on the sandboxes, Halo
  2's `masterchief_mp` (on the seats Halo 1's player enters; Halo 2's
  graph has no mode for the scorpion's side seats, listed, not failed).
  **Who fires**: `0x000cc810` makes the seat with bit 2 the vehicle's
  driver (`+0x248`) and the seat with bit 3 its gunner (`+0x24c`), or one
  with bit 18 (`SEAT_PLAYER_GUNNER`) when its rider has a player
  (`+0x13c` != -1). The retail warthog's driver seat is `0x48014`: bit 18
  makes a player driver its gunner, and the retail warthog's own weapon
  is its horn (its gun is a turret vehicle with a seat of its own). A
  rebuilt seat never takes bit 18, so only Halo 1's gunner bit makes a
  gunner (`a_rebuilt_seat_s_gunner_is_halo_1_s_alone`,
  `the_warthog_gun_is_held_and_only_its_gunner_fires`).
- **Friction and hover points** (`+0x2f0`, `+0x2e8`). Halo 1's
  counterparts are the `phys` mass points with a powered mass point:
  the warthog's four tires and the ghost's hull and two wings, the same
  counts as the Halo 2 warthog's four friction points and ghost's three
  hover points. Each becomes a marker of the converted model
  (`Converter::powered_points`, added before the `mode` is converted):
  named as the mass point (`tire_front_left`), at the mass point's
  position in its node's space. V: Halo 1's mass point positions are in
  object space (each warthog tire point lies within its radius of its
  tire node's default position). Every point rides the root node (0),
  at its Halo 1 position. A wheel's or tread's own node turns and rises
  with the spin and suspension animations, and a hover point on a node
  other than 0 gives no lift: the engine's lift reads the marker's node
  ([notes/rebuild.md](notes/rebuild.md), "Hover points on another
  node"). V: every friction and hover marker of every vehicle on
  coagulation, beavercreek and 05a_deltaapproach is on node 0
  (`every_powered_point_rides_the_root_node`). The point's region: a
  gripping point's is the root node's collision region, any other's its
  own node's. No marker is rotated. V: the retail warthog's four tire
  markers are on node 0, 0.23 below it (the wheel centres, 0.19 above the
  origin, where Halo 1's tire points are), facing out along their axles;
  the scorpion's tread markers are on node 0 too, unrotated. The axle
  rotation is not taken: one rule would give the scorpion's gripping
  treads it too, and no run has shown it helping (the same note).
  The marker has one instance per permutation of the model's first
  region (D).
  The Halo 2 point names that marker (`+0`) and the collision region of
  the collision node named as the model node (friction `+0x44`, hover
  `+0x34`: the warthog's `tire`; the ghost's collision has one region,
  `__unnamed`). The friction material (`+0x3c`) stays null. Friction
  points split by steering (flags `+4` bit 2, V: the retail warthog's
  front pair holds `0x26`, its back pair `0x32`): Halo 1's first powered
  group steers, the second takes bit 3 on a jeep.
  The points are written in Halo 1's mass point order, and the phantom's
  spheres (the physics element's first multisphere, count at `+0x6c`,
  centres from `+0x70`, `0x10` each) in the same order
  (`write_vehicle_phantom`). E: the engine takes friction point `i`'s
  suspension compression from the phantom's sphere `i` (the per-point
  loop at `0x2067dd`; `0x207ab0` writes the point's radius less the
  sphere's ground distance, clamped to the radius), so point and sphere
  must pair by index. The friction point's region index (`+0x48`) is its
  region's own index among the `hlmt` regions, -1 for none (V: the
  retail warthog's 4, 7, 3, 6, its four wheel regions; the scorpion's
  -1). E: `0x205fd1` turns a point off once that region's damage state
  reaches `+0x42`.
  The wheels and treads (`convert::wheel_nodes`: the gripping points'
  nodes and every node under them) follow the source vehicle type: a
  wheeled vehicle (`vehicle_type == 0`, `PhysicsKind::Vehicle { wheels }`)
  has its body hold them above the hull's lowest point, each wheel node's
  cells cut there and enclosed in one piece (`convert::held_wheels`);
  otherwise they stay out. Halo 2 holds a vehicle up on its
  friction-point springs and keeps its body clear of the ground either
  way. V, coagulation:
  the retail scorpion's `phmo` has two tread shapes reaching 1.13 either
  side where its hull reaches 0.53, from 0.22 above the origin where the
  hull's lowest point is 0.25; the retail warthog's has no wheel shape
  (a hull on node 0 whose lowest point is 0.18 above the origin, and a
  bumper box). The warthog's four friction points name its wheel
  regions (`lf_wheel` ...; hlmt regions 4, 7, 3, 6), which its `phmo`
  (regions `bumper`, `hull`; bodies on nodes `hull` and `bumper`) does
  not have; the scorpion's twelve name none, its `phmo` one region,
  `hull`, two bodies on no node. Every friction marker of both hangs
  from node 0, so the markers' nodes cannot tell wheels from hull. The
  converted scorpion therefore holds its treads and the converted
  warthog leaves its tires out, as retail; a biped meets a vehicle
  through its `phmo` (inferred from a run where a player walked through
  scorpion treads the `coll` carried; the engine path was not traced,
  [notes/rebuild.md](notes/rebuild.md), "Vehicle runs on converted Blood
  Gulch"). Tires whole in the body would sink into the ground at spawn and
  the depenetration would throw the warthog about (the same note).
  Proof: `tests/vehicle_rebuild_disc.rs`,
  `wheels_and_treads_are_solid_above_the_hull` (the retail scorpion's
  reach; the base's scorpion has wheel shapes, its warthog none; the
  converted scorpion reaches its treads'
  sides, the warthog no further than its hull, neither under the hull's
  lowest point; the ghost has no wheel nodes), and
  `held_wheels_keep_what_is_above_the_floor`.
- **Control** (`+0x1f2`, the 16 bits after `vehicle_type`,
  `rebuild::VEHI_CONTROL`). H2 selects the differential controller for
  mode 2 at `0xef747`. On the retail disc the field holds
  2 on the scorpion (the only type 0, tank) and on its two cannon
  turrets (type 6); 0 on every other vehicle: the warthog (1); the
  pelican, phantom, gravity throne and sentinel enforcer (3); the ghost,
  wraith, spectre and creep (4); the banshee (5); and the other 23
  turrets (6). So type 0 takes 2 and every other type 0
  (`tests/vehicle_rebuild_disc.rs`, `control_follows_vehicle_type`). A
  scorpion written with 0 creeps at 0.05 units a second with the stick
  held forward and drives at 2.5 with 2 (the run note).
- **Engine gears** (`+0x268`, stride `0x44`). The two torque
  curves are inline six-float records. The XBE interpolates them at
  `0xf15d0`, blends them at `0xf1540`, reads the speed ratio at
  `0xf152c` and selects the next gear at `0xf18aa`. Converted H1 wheels
  and tracks drive the contacts directly. [Current mappings and XBE evidence](role-audit.md).
- **Contact and flight data.** The converter writes friction and hover
  points in H1 mass-point order. H2 reads up to sixteen friction contacts
  (`0x20678d`). Each inline phantom shape holds eight spheres. The loader
  initializes the active shapes at `0x205550`. On converted vehicles the
  unused tail stores the H1 wheel or aircraft controller at `+0x180` and
  the powered-seat header at `+0x288`. Its count word at `+0x28c` holds
  the channel count in the low byte and the source-gravity flag in bit 8.
  Excess or conflicting source data reports a conversion error. [Drive equations and disc evidence](role-audit.md#native-engine-fields).
- **Seat flag bit 15** (`rebuild::SEAT_OPENS`). E: on entering a seat
  the engine tests bit 15 of the seat's flags (`0x000d12b0`, the test at
  `0x000d1302`; called from the seat change at `0x000ea979`, `0x000eab64`,
  `0x000eb1ff`) and, set, sends the vehicle unit action `0x17`; on
  leaving (`0x000d1360`, test at `0x000d13b2`; from `0x000ea7a5`,
  `0x000ea8ba`) action `0x18`. The action table at `0x004677c8` gives
  them `0x000e9370` and `0x000e9500`, which start the animation state
  `0x0700005c` or `0x0700005d` (string IDs of seven characters, D:
  `opening` and `closing` by length, not read off the string table) on
  the vehicle and the objects attached to it. No other read of bit 15 of
  a seat's flags was found (every `vehi +0x1cc` load followed by a bit 15
  test, over the whole `.text`). The retail ghost driver holds it
  (`0x801c`); Halo 1 has no such seat flag and its graphs no such state,
  so a rebuilt seat never takes it. It is not a grenade switch: the
  converted ghost's driver throwing grenades is not explained here.
- **References**: the unit's weapons are Halo 1's converted weapons
  where they convert. The attachments and creation effect are Halo 1's
  ([Object attachments](objects.md#object-attachments)). HUD (`unhi`) and damage stay null: no converted
  Halo 1 tag exists.
- **Camera tracks** (seat `+0x74`, `rebuild::SEAT_CAMERA_TRACKS`): each
  seat's first Halo 1 camera track (seat `+0xd0`, `0x1c` each; the
  warthog driver's loose one), converted to a `trak` with the same
  control points (`rebuild::camera_track`; `tags/src/defs/trak.txt`).
  The two games share the layout: Halo 2's
  `globals\default_unit_camera_track` holds Halo 1's default unit
  track's nine points value for value (`tests/vehicle_camera_disc.rs`).
  Each point's orientation is written conjugated
  (`h1models::h1::h2_rotation`): Halo 1 stores rotations as the
  conjugate of Halo 2's. E: the following camera (`0x23cbb0`) bisects
  the track (`0x23c980`, sampling with `0x23c300`) for the point whose
  forward axis's elevation matches the look pitch, assuming elevation
  rises along the track. V: every retail track rises from -90 to +90
  degrees; Halo 1's, read unconjugated, fall, so the search would end on
  the first point, the top one, 3.5 above the warthog's camera marker
  whatever the pitch (`tests/vehicle_camera_disc.rs`,
  `retail_tracks_rise_halo_1s_fall`). Halo 2's default unit track is a
  byte copy of Halo 1's and falls too, so a seat Halo 1 gives no track
  takes the unit's (`unit +0x1f4`), else Halo 1's `globals\default unit
  camera track`, converted. Not checked by eye in a game run.

Named on the way:

| Field | Evidence |
|---|---|
| `vehi +0x1ec` `vehicle_flags` (`u32`) | E: objects.md (vehicle code tests bits here, `0x000efb8c`). V: the low half equals Halo 1's `vehi +0x2f0` on five of eight pairs (warthog, scorpion, wraith, banshee, Covenant gun turret); the ghost differs in bit 8, the pelican and dropship in bits Halo 2's fliers add |
| `unit +0x1c0` `weapons` (`weapon`, `tagref weap`) | V: every element of every vehicle on 05a and Coagulation that has one names a `weap` (the turrets' guns, the warthog's horn, the ghost's, banshee's and phantom's guns) |
| Halo 1 `vehi +0x2f4` vehicle type | V: equal to Halo 2's `vehicle_type` on all eight pairs; the games share the enumeration |

Halo 1's speeds carry as seconds-based values (speeds times 30,
accelerations times 900). On the eight pairs no Halo 1 word agrees with a
Halo 2 driving word on more than one pair (the warthog's Halo 1 `+0x2fc`,
`+0x300`, `+0x304` times 30, 900 and 900 are Halo 2's `+0x1f8`, `+0x1fc`,
`+0x200` exactly; every other vehicle was retuned for Havok).

Proof (`crates/tools/h1objects/tests/vehicle_rebuild_disc.rs`, lib test
`rebuilt_vehicle_is_the_definition_with_seats_and_the_turret_role`):

- `vehicle_type_and_flags_match_halo_2s_counterparts`: type 8 of 8
  pairs, flags low half 5 of 8, camera 8 of 8.
- `every_vehicle_rebuilds_from_its_definition`: every distinct vehicle
  of the ten campaign levels on 05a and of each multiplayer base,
  Coagulation and Beaver Creek, rebuilds; the `vehi`, `hlmt` and `phmo`
  read back, write the same again, pin nothing and hold no template
  bytes; type, flags, camera, bounds, seat labels and markers are Halo
  1's.
- `powered_mass_points_become_friction_and_hover_points`: the warthog
  has 4 friction points and gears, the ghost 3 hover points; each names
  a marker of the converted `mode` (instances on nodes in range) and a
  region of Halo 1's collision. `ghost_keeps_its_source_hover_points`
  checks the ghost's against Halo 1's `phys`.

What only a game run shows: driving (the friction points on Halo 1's
tire markers with the source velocity controller, the scorpion's eight tread points
where Halo 2's has twelve), hovering (the ghost's and wraith's points),
flying for the fliers and dropships, whether a vehicle stands on its
Halo 1 collision hull, boarding (the seat labels as modes, the entry
markers) and the riders' poses. First suspects if driving fails: the
marker instances' region and permutation bytes and the friction material.

## Bipeds from definitions (`h1objects::rebuild::biped`)

Every Halo 1 biped is written from the `bipd` definition
(`rebuild::biped`, `Converter::biped_rebuilt`), with no Halo 2 template
read:

- **From Halo 1**: the object type (0), bounding sphere, model chain
  (`hlmt` from the definition, as for rebuilt scenery) and predicted
  resources (the render model, then its bitmaps), the change colours, the
  camera's field of view, the unit and biped words (`UNIT_WORDS`,
  `BIPED_WORDS` as they are, `BIPED_RATES` per tick to per second), the
  camera tracks, weapons, integrated light, melee damage, blood effect,
  attachments and functions, the collision size and the character shapes
  sized to it (below), and a physics model built from the collision
  (below). The unit's marker names (`UNIT_MARKERS`) are written when the
  converted model has them. Every field without a Halo 1 source keeps the
  definition's default.
- **From the counterpart** (`rebuild::COUNTERPARTS`, `Converter::biped_counterpart`:
  the Halo 2 biped nearest the Halo 1 one that the base holds:
  masterchief, elite, grunt, jackal, bugger, hunter, marine, crewman),
  read through the definitions (`Base::record`): the model's shield
  shader (`shad_ref_94`), the damage block's native overshield shaders
  and damage sections (`tagref_e8`, `tagref_f0`, `block_bc`; the
  player takes no damage sections from it), and the player's HUD
  interfaces for the switchable Halo 2 view. Those are Halo 2 engine
  dependencies with no Halo 1 source; each copied value is recorded by
  `Tree::note_roles` and audited in [role-audit.md](role-audit.md).
- **The player** (`health_hud::is_player_biped`) takes the converted
  Halo 1 unit HUD for every interface the counterpart has, with the
  counterpart's as the paired Halo 2 view (docs/halo1/health.md).
- **Halo 1 references with no converter here** (`unhi` HUDs, `actv`) are
  not carried. Halo 2's `bipd` holds no seats (`bipd +0x1c8`) on any base
  biped, and its two `char` references (`bipd +0x300`, `+0x308`) are null
  on all of them: an AI's character is the scenario's (h1ai), not the
  biped's.

Character shapes (E, V):

| Field | Evidence |
|---|---|
| `bipd +0x268`, `+0x26c`, `+0x270` (`rebuild::BIPD_SIZE`): standing and crouching collision height, collision radius | V: every retail biped of the base with shapes holds shapes derived from these three (below); Halo 1 holds the same three at `bipd +0x424`, `+0x428`, `+0x42c` (`h1::BIPED_SIZE`): the cyborg's 0.7, 0.5, 0.2 against the Chief's 0.725, 0.5, 0.175 |
| `bipd +0x284` spheres (`0x80`), `+0x28c` pills (`0x50`), `+0x294` spheres | E: the `bipd` post-process `0x001c2390` (from the tag load dispatch `0x0011c840`, on `bipd +0x264`) writes each sphere's Havok header at element `+0x20` and a transform's at `+0x30` with the sphere as its child (`+0x3c`), each pill's at `+0x20`; `0x001c2460` returns pill `i` (standing 0, crouching 1) as the character's shape, else sphere `i` of `+0x294`. V: a pill runs from (0, 0, radius) to (0, 0, height - radius), the radius in both ends' `w` and at `+0x2c`; the three `+0x284` spheres have radii 1, 1.35 and 1.7 times the collision radius, each resting on the ground (`+0x78` its radius); `+4` holds `0xffff` and `+0x1e` `0xffff` |

A rebuilt biped writes Halo 1's size and builds both blocks from it
(`rebuild::biped_shapes`, runtime words zero). One whose Halo 1 heights
are zero (the pilot, the engineer, the flood infection forms, the
monitor, the sentinel) has no pills and takes two character spheres
(`usable_size` false) sized from its radius or bounding radius, and the
three `+0x284` probe spheres.

Physics model: one rigid body per Halo 1 collision node with solid
cells, on the model node of that name (as devices,
`PhysicsKind::Biped`), with the rigid body words `+0x1a` 2 and `+0x1e` 1
every rigid body of every retail biped physics model on the base holds
(`rebuild::BIPED_BODY`, 80 bodies). Retail's are ragdolls: ten bodies
of pills and a box joined by node edges (`phmo +0x30`), four limited
hinges (`+0xe8`) and five ragdoll joints (`+0xb8`)
([collision.md](../formats/collision.md), "Constraints"). A built biped's bodies are
joined the same way (`h1objects::ragdoll`, `convert::biped_ragdoll`):
each body to the body of its nearest ancestor node, knees and elbows as
limited hinges, every other joint (hips, shoulders, head, spine, feet,
hands) a ragdoll joint, frames from Halo 1's rest pose (the child's frame
on its origin, the parent's the same frame carried by the child's rest
transform, as retail's), limits our own inside retail's range for each
joint, one edge per constraint with friction 0.25. A skeleton that does
not map (under three bodies, more than one body with no ancestor body, or
no hip and knee by name) keeps its unjoined bodies and the report says
so. A biped Halo 1 gives no collision (the a10 pilot, d40's
cinematic Johnson) gets none, as retail's cinematic characters (miranda,
regret_holo) have none.

Named on the way:

| Field | Evidence |
|---|---|
| `unit +0xcc` `camera_field_of_view` (`f32`, radians) | V: 70 degrees on every Halo 2 biped and turret with a Halo 1 counterpart, and equal to Halo 1's `unit +0x1a0` on five pairs (cyborg and masterchief, marine_armored and marine, elite, grunt, jackal). Name D: a field of view is the plain reading of 70 degrees on a unit; no reader found yet |

`rebuild::BIPED_WORDS` carries the biped's turning, landing, camera
height, slope and flying words from Halo 1 as they are. The ones with a
Halo 2 pair agree within 1 % on the five pairs above; the crewman's
distinct cutoff and falloff angles on a30 and 05a_deltaapproach tell the
slope fields apart, and the c20 monitor and its 04b counterpart agree on
the three flying accelerations after conversion to seconds. Words that
hold angles in radians and small ratios whose meaning is not established
stay unknown spans carried raw.

Proof (`crates/tools/h1objects/tests/unit_rebuild_disc.rs`, and the lib tests
`biped_carries_each_source_word_and_leaves_unmapped_fields_at_default` and
`rebuilt_biped_uses_source_shapes_tracks_weapons_and_hud`):

- `biped_words_match_halo_2s_counterparts`: the camera and the words,
  Halo 1 against Halo 2, on the five pairs.
- `biped_shapes_follow_the_collision_size`: the shape rule on the
  retail bipeds of the base with shapes, and `BIPED_BODY` on their 80
  rigid bodies.
- `bipeds_rebuilt_from_the_source_unit`: every Halo 1 biped on the ten
  levels is rebuilt; the record reads back, writes the same again, pins
  nothing, and holds no template bytes; its object type, bounding radius
  and model chain, camera, words, size and shapes are Halo 1's, its
  camera tracks match Halo 1's, every reference resolves to a tag of its
  group, a physics model is built from Halo 1's collision wherever it has
  one, and the player's HUD interfaces are the converted unit HUD.
- h1campaign's `every_campaign_level_converts` builds all ten levels
  fresh (0 dangling references, tag memory within the bound), and
  `physics_models_load_as_the_engine_reads_them` walks a10's physics
  models as the engine's post-process does.

Suspects if a rebuilt biped misbehaves: the character shapes are Halo 1's
size (a marine's radius 0.18 where Halo 2's is 0.12), and a human takes
the marine's shield shader and damage sections.

## Devices (`mach`, `ctrl`; `h1objects::rebuild::device`)

`crates/formats/tags/src/defs/devi.txt`: `struct devi` (`0..0x11c`, the object
header and the device layer) and the `mach` (`0x134`) and `ctrl`
(`0x140`) roots. All 135 retail machines and 14 controls round-trip
byte-identical. The disc has no `lifi` tags, so there is no `lifi`
definition. Named fields and their evidence are in the file; the ones
the conversion writes:

| Field | Evidence |
|---|---|
| `devi +0xbc` `device_flags` | E: object_new reads it for device types; bit 0 set on the retail devices whose position wraps |
| `devi +0xc0..+0xd4` six rates: power, power acceleration, position, position acceleration, depowered position, depowered position acceleration | E `0x00115431`/`0x00115437`: the position step is `position_rate` and `depowered_position_rate` blended by the device's power, times the seconds per tick, so they are rates per second. V: no retail device holds 0 in an acceleration slot; 900 stands for "at once" |
| `devi +0x10c` `delay_time`, `+0x118` `activation_radius` | V: set on retail doors only, in seconds and world units, the values Halo 1's doors hold |
| `mach +0x11c` `machine_type` (door, platform, gear) | E `0x001153ee`: type 2 wraps the position at 1.0; `0x0011557e`: type 0 runs the door timer. V: retail doors 0, lifts and rails 1, flywheels and hinges 2 |
| `mach +0x11e` `machine_flags`, `+0x120` `door_open_time`, `+0x12c` `collision_response`, `+0x12e` `elevator_node` | E reads (objects.md); V: Halo 1's doors and lifts hold the same values at `mach +0x292`, `+0x294`, `+0x2e8`, `+0x2ea` |
| `ctrl +0x11c` `control_type`, `+0x11e` `control_trigger`, `+0x120` `control_value`, `+0x124` `action_string` | E reads (objects.md) |

Halo 1's devices (`h1objects::h1::Device`, V across the ten levels):
the device layer is `0x17c..0x290` (flags `+0x17c`; six times in seconds
`+0x180..+0x198`, in the order of Halo 2's rates; delay `+0x200`;
activation radius `+0x21c`), then the machine's type `+0x290`, flags
`+0x292`, door open time `+0x294`, collision response `+0x2e8` and
elevator node `+0x2ea`, or the control's type, trigger and value at
`+0x290..+0x298`. A time `t` becomes the rate `1/t`; a zero time is a
zero rate, or 900 in an acceleration slot. A light fixture (`lifi`) is
written as a machine of type platform (no behaviour of its own); its
light is not carried. Sounds and effects stay null (Halo 1's are `effe`
tags the converter does not build). Havok, which moves the player, sees a
device only through a physics model, and Halo 1's have none; without
one a closed door would show, animate and stop bullets (its `coll`) but
not block the player. A device with
solid collision therefore gets a `phmo` built from it (below): one rigid body per
collision node, on the model node of the same name, so each part moves
with its node as the door opens, as the disc's doors have (V:
`high_small_door`, a body on each of its three moving nodes). Counted
per level, 86 of the ten levels' 156 device tags get one; the rest have
no collision or none with solid space (most light fixtures).

## Physics models from Halo 1 (`h1objects::physics`, `rebuild::physics_model`)

A Halo 1 collision BSP's 3D tree ends in leaves (open space) and missing
children, which are solid (`h1objects::query`). Every root-to-missing
path is a set of half-spaces; clipped to the BSP's vertex box, their
intersection is a convex cell, and the cells together are exactly the
solid space. `physics::solid_cells` walks the tree and returns each
non-empty cell: its corners (the intersections of three of its planes
inside all of them), its face planes, volume, centroid and inertia.
`collision_disc.rs` (`halo1_solid_space_becomes_physics_pieces`) checks
this on every collision model of the ten levels (16,189 cells): just
outside a surface Halo 1's own tree finds open space 51,729 times and
solid 179, just inside solid 50,591 times and open 1,317 (parts thinner
than the probe step, open meshes); at 142,528 points through the BSP
boxes the tree and the cells agree on all but 117 (points on a face).

`rebuild::physics_model` writes the `phmo` from the definition: each
cell a polyhedron, a body with several a list of them, then the bodies,
one material, one region and permutation naming every body, and the
model's nodes (parent, next sibling, first child) when a body names one.
The layout follows the disc's polyhedron physics models (V:
`high_small_door`, `ghost`, `pelican`):

- a polyhedron of up to 12 corners keeps them inline at `+0x60` in up to
  three groups of four transposed points (capacity `0x80000003`); a
  bigger one takes its groups from the root `four_vectors` block, in
  polyhedron order (only those consume it), capacity `0x80000000 |
  groups`. Planes come from the root `plane_equations` block, in order,
  for every polyhedron; `+0xfc` is the plane count with bit 31 set.
- a list of up to four children keeps them inline (capacity
  `0x80000004`); a longer one takes them from `list_children`, capacity
  `0x80000000 | n`. Retail rigid bodies name a list directly 125 times,
  so no MOPP is written over it.
- shape header `+8` 1.0, `+0xc` 0.9, `+0x10` 0.6, `+0x14` the piece's
  volume, `+0x18` its mass; rigid body `+0x1a` 2, `+0x1e` 5 (retail
  bodies hold 1 to 6 there, 5 on most doors and vehicles; meaning not
  established), `+0x20` 1.0, `+0x28` 0.05; body mass, centre of mass and
  inertia are the pieces' at the body's density.
- the material is `hard_metal_thick_cov` with header `+6` 76, the pair
  the disc's `high_small_door` holds (D: `+6` reads as an index into a
  global material table that is not named here). Halo 1's collision
  materials are not carried.
- the shapes' Havok headers and pointers stay zero; the engine writes
  them at load (`0x001ea9c0`).
- no block of a `phmo` is shared with another tag
  (`tags::repo::build::IN_PLACE`). The loader turns each rigid body's and
  list child's `{type, index}` reference into a pointer in the block
  itself, so a block two tags share is resolved twice and the second
  pass reads a pointer as a reference: the resolver (`0x001eaf30`)
  returns null for it, and the list's bounding box faults reading its
  first child (`0x002fc139`). A fresh map shares equal blocks between
  tags by content, so tags with equal list children (a10's cryotube
  vehicle and machine) or equal rigid bodies (three a10 doors) would
  share. `physics_models_load_as_the_engine_reads_them`
  (h1campaign `disc.rs`) checks every converted map's physics models
  against the loader: no rewritten block shared, every reference in
  range, the list-children, four-vector and plane runs filling their
  blocks.

A device's mass is its cells' volume at 1000 kg per cubic metre
(`convert::DENSITY`, per cubic world unit). A body of more than 64 cells
(`physics::MAX_PIECES`; no built body on the ten levels reaches it, the
most is b30's falling box with 62) keeps one enclosing piece
per node instead: the intersection of the cells' own faces that hold
every corner and of 26 axis, edge and corner planes moved out to the
farthest corner. Not done: the `block_30` pairs retail doors hold
(adjacent bodies' nodes, D), constraints, and Halo 1's `phys` mass
points.

Placement (`h1campaign::scenario`): Halo 1's machines (`+0x294`, `0x40`
each), controls (`+0x2ac`, `0x40`) and light fixtures (`+0x2c4`, `0x58`)
and their palettes land in Halo 2's machine (`+0xa8`, `0x48`) and
control (`+0xb8`, `0x44`) blocks, fixtures after the machines. After the
object fields both games hold the power group, position group (`i16`
indices into the device groups, -1 none), the device flags and the
machine's or control's flags: Halo 1 at `+0x28`, Halo 2 at `+0x34`. The
device groups are `h1scripts::level`'s (`+0xa0`, in Halo 1's order); a
conversion that leaves out the level data (`Lean::LevelData`) writes
every group index as -1.

Proof:

```sh
H2_DISC=... H2_DEFS_GROUPS=mach,ctrl cargo test --release -p tags --test tag_defs -- --ignored --nocapture
H1_ISO=... H2_DISC=... cargo test --release -p h1campaign --test disc every_device_placement -- --ignored --nocapture
```

`every_device_placement_lands_with_its_device_groups`: on every level,
every Halo 1 machine, control and fixture placement lands (1,596
machines, 840 fixtures, 77 controls on the disc), at Halo 1's position and
rotation, with its palette entry a `mach` or `ctrl` that re-encodes
byte-identical, its power and position groups Halo 1's and below the
converted device group count, and its flags; all 496 Halo 1 names of
devices name their placement (a fixture's names its machine).

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
byte-identical. A Halo 2 `udlg` is a list of vocalizations, each a
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

## Which converted objects use the definitions

| Groups | Path | Why |
|---|---|---|
| `vehi` | definitions (only path) | no vehicle template path |
| `mach`, `ctrl`, Halo 1's `lifi` (as `mach`) | definitions (only path) | no template path |
| `phmo` of a rebuilt vehicle or device | definitions (Halo 1's collision as polyhedra; a stand-in sphere with no solid collision) | `vehicle_new` refuses a model with none; Havok sees a device only through one |
| `scen` | definitions (only path) | no scenery template path |
| `bipd` | definitions (only path) | Halo 1's values; the shield shader, damage sections and Halo 2 HUD view from the Halo 2 counterpart |
| `phmo` of a rebuilt biped | definitions (Halo 1's collision as polyhedra, a body per node, joined as a ragdoll) | Halo 1 gives no ragdoll |
| `weap`, `eqip`, `proj`, and the `jpt!` the projectiles deal | definitions (only path) | Halo 1's values; not run in the game |
| `effe` (effect parts, vehicles' attachments and creation effects) | definitions (only path) | no template path; sound, light, decal, damage, object and particle-system parts |
| `ligh`, `deca` (effect parts, attachments) | definitions (only path) | no template path |
| `lens` (lights', sky lights'), `garb` (effect parts) | definitions (only path) | no template path |
