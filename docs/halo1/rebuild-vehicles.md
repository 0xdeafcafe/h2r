# Rebuilt Halo 1 objects: vehicles and bipeds

Vehicles and bipeds from definitions, split from [rebuild.md](rebuild.md).

| Section |
|---|
| [Every vehicle from definitions (`h1objects::rebuild::vehicle`)](#every-vehicle-from-definitions-h1objectsrebuildvehicle) |
| [Bipeds from definitions (`h1objects::rebuild::biped`)](#bipeds-from-definitions-h1objectsrebuildbiped) |

Part of [rebuild.md](rebuild.md).

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
  `0xf152c` and selects the next gear at `0xf18aa`. A converted vehicle
  holds its Halo 2 counterpart's gears. [Current mappings and XBE evidence](role-audit.md).
- **Contact points.** The converter writes friction and hover points in
  H1 mass-point order, each with the tuning of the counterpart's point
  ([Drive counterparts](role-audit.md#native-engine-fields)). H2 reads up
  to sixteen friction contacts (`0x20678d`). Each inline phantom shape holds
  eight spheres. The loader initializes the active shapes at `0x205550`.
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
  checks the ghost's markers and radii against Halo 1's `phys`, and
  `blood_gulch_vehicles_take_their_counterparts_drive_on_both_bases`
  that each vehicle's drive is its counterpart's.

What only a game run shows: driving (the friction points on Halo 1's
tire markers on the Halo 2 warthog's tuning, the scorpion's eight tread points
each on the tuning at the same fraction of Halo 2's twelve), hovering (the ghost's and wraith's points),
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

`rebuild::BIPED_WORDS` carries the biped's turning, landing time, camera
height, slope, flying and aim assist (`autoaim_width`, `bipd +0x458`) words
from Halo 1 as they are. Halo 2's gravity
stays, so Halo 1's jump apex and fall heights are kept by scaling speeds
with `rebuild::gravity_speed_factor` (sqrt(1 / `GRAVITY_RATIO`), 1.14):
`jump_velocity` (`BIPED_RATES`, per tick times 30 times the factor; the
cyborg's 0.07 becomes 2.394 world units per second, apex 0.687) and the
three landing velocities (`rebuild::BIPED_LANDING`, Halo 1's value times
the factor, no tick factor: the crewman's agree with its Halo 2
counterpart's as they are). The ones with a
Halo 2 pair agree within 1 % on the five pairs above; the crewman's
distinct cutoff and falloff angles on a30 and 05a_deltaapproach tell the
slope fields apart, and the c20 monitor and its 04b counterpart agree on
the three flying accelerations after conversion to seconds. Words that
hold angles in radians and small ratios whose meaning is not established
stay unknown spans carried raw.

Halo 1's biped flags (`bipd +0x2f4`) are carried by name
(`rebuild::BIPED_FLAG_BITS`, `BIPED_PHYSICS_BITS`): turns without
animating (bit 0), passes through other bipeds (5) and immune to falling
damage (7) set the Halo 2 `flags` bits of those names; physics centred at
the origin (3), spherical (4) and climbs any surface (6) set bits 0, 1 and
3 of the character physics word (`bipd +0x264`), beside the player bit and
the flying pair already written there. A converted sentinel keeps the
retail aggressor pair (`flags` `0xe00`, physics `0x13`) over these. Halo
1's other flag bits have no Halo 2 counterpart and set nothing.

The Halo 2 biped words Halo 1 has no word for are never left at the
definition's zero. `death_hard_landing_velocity` is Halo 1's `+0x3e8`, in
`BIPED_WORDS` beside the other landing velocities. The pelvis, head and
physics control node indices are the converted model's `bip01 pelvis` and
`bip01 head` nodes (`rebuild::BIPED_NODES`), -1 on a model without them.
`rebuild::BIPED_DECLARED` gives the rest a value and the reason for it:
the global material names null and their indices -1 (Halo 1 names a
material per collision region), headshot scale 1, stun duration 0 (Halo 1
bipeds are not stunned), no contact points, and zero acceleration range
and gaze limits (Halo 1 has none; a player's look follows the controller).
These values are chosen, not read from a Halo 2 biped.

Proof (`crates/tools/h1objects/tests/unit_rebuild_disc.rs`, and the lib tests
`biped_carries_each_source_word_and_leaves_unmapped_fields_at_default`,
`biped_flags_carry_halo1s_named_bits`,
`no_converted_biped_field_is_left_at_definition_zero` and
`rebuilt_biped_uses_source_shapes_tracks_weapons_and_hud`):

- `biped_words_match_halo_2s_counterparts`: the camera and the words,
  Halo 1 against Halo 2, on the five pairs.
- `jump_reaches_halo1s_apex_under_halo2_gravity` and
  `landing_thresholds_keep_halo1s_fall_heights` (lib tests), and
  `the_converted_cyborgs_physics_against_the_chiefs`: the jump apex and
  fall heights under Halo 2's gravity equal Halo 1's.
- `converted_bipeds_have_autoaim_width`: the cyborg, elite and grunt
  hold Halo 1's non-zero aim assist width.
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
