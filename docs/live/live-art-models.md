# Player and weapon models

The same extraction writes a posed model per player species and campaign
character, a model per multiplayer weapon and a model per vehicle, for the
3D viewer's players and AI (the players contract, sections 2 and 5). Like the maps, everything comes from the owner's disc at run
time; the tests build invented models.

Code: `crates/apps/cli/src/live_models.rs`. Tests:
`crates/apps/cli/src/live_models_tests.rs`.

## Files

```
models/biped/spartan.glb
models/biped/elite.glb
models/biped/<species>.glb    one per campaign character (grunt, jackal, ...)
models/weapon/<stem>.glb      one per multiplayer weapon
models/vehicle/<stem>.glb     one per vehicle tag
```

`manifest.json` gains a `models` object (absent when nothing was
extracted):

```json
"models": { "version": 2,
            "bipeds":  { "spartan": { "glb": "models/biped/spartan.glb",
                                      "clips": ["rifle:idle", "rifle:run", ...] },
                         "elite":   { "glb": "models/biped/elite.glb", "clips": [...] } },
            "weapons": { "battle_rifle": { "glb": "models/weapon/battle_rifle.glb",
                                           "class": "rifle",
                                           "ballistics": { "kind": "bullet", "speed": 400,
                                                           "final_speed": 400, "rate": 6.5 } } },
            "vehicles": { "warthog": { "glb": "models/vehicle/warthog.glb" } },
            "seats": { "warthog": [ { "label": "warthog_d", "node": "seat_driver" },
                                    { "label": "warthog_p", "node": "seat_passenger" },
                                    { "label": "warthog_g", "node": "seat_gunner" } ] } }
```

`vehicles` is absent when there are none. `clips` (a biped's animations,
in the glb's order) is absent when it has none; `class` (the weapon class a
weapon is held with, picking the clip set) is absent when the tag names
none. `ballistics` (what the weapon fires: `kind` one of `bullet`,
`plasma`, `needle`, `rocket`, `grenade`, `beam`, `melee`; `speed` and
`final_speed` in world units a second; `rate` sustained rounds a second;
each absent when unknown) comes from the weapon's barrel and its projectile
tag (`crates/apps/cli/src/live_weapons.rs` has the offsets and how they were
established) and drives the viewer's tracers, bolts and rockets. Version 1
had none of these and unskinned bipeds; a version 1 reader that skips the
version check still draws the version 2 files (below).

`seats` (extractor 22) lists, by vehicle stem, the vehicle's seats in the
order of its tag's seats block (`vehi +0x1c8`, stride `0xb0`;
crates/formats/tags/src/defs/unit.txt), the block the unit's seat index
(`unit +0x1fc`, telemetry's `seat`) indexes: element `i` is seat `i`.
`label` is the seat's label (string ID at `+4`), the rider's animation
mode, so `any:seat_<label>` is the biped clip that seats the rider (the
example's order is illustrative; the tag's order is what is written). `node` is the vehicle glb's `seat_` node for the seat's marker
(string ID at `+8`, named as in pipeline step 8), or `null` when the
marker is not a seat marker or the model has no such node (the rider is
then placed by the reader's own rule). A vehicle without seats is not
listed, and `seats` is absent when no vehicle has any. The vehicle's own
entry under `vehicles` is unchanged.

Art from an older extractor (the manifest's `extractor` below the running
`h2`'s) is stale: before version 2 its bipeds had no clips, so the viewer's
players couldn't animate. `h2 live serve` prints a warning naming the
command to extract again, `h2 live assets check [--out DIR]` exits 1 for
missing or stale art (a script can extract again then), `h2 play
--live-local` extracts again by itself (the extractor version is part of the
cache key), and the viewer says so when the models it is given can't
animate. The server sends the art with `Cache-Control: no-cache` and the
viewer asks for each model with the manifest's `source` as a query, so a
browser never keeps showing models from before an extraction.

A model that could not be built is listed under `skipped` as
`models/biped/<species>`, `models/weapon/<stem>` or
`models/vehicle/<stem>` and has no entry. Only keys matching
`[a-z0-9_]{1,32}` are written.

Keys (what telemetry sends):

- **Weapons and vehicles:** the tag's file name, lower case
  (`battle_rifle`, `energy_blade`, `brute_shot`, `flag`, ...; `warthog`,
  `ghost`, `banshee`, `scorpion`, `wraith`, `spectre`, `phantom`,
  `pelican`, and the turrets as their own tags: `chaingun`, `gauss`,
  `c_turret_ap`, ...).
- **Player bipeds:** `spartan` and `elite`.
- **Campaign characters:** the name of the biped tag's directory, lower
  case: `objects\characters\grunt\grunt` is `grunt`,
  `...\marine\marine_female\marine_female` is `marine_female`,
  `...\prophet\prophet_minor` is `prophet`. Where several biped tags share
  a directory, the one named like the directory is drawn, else the one on
  the most campaign maps, else the shortest name. The retail directories are
  not always the common names: drones are `bugger`, combat forms are
  `floodcombat_human` and `floodcombat_elite`, Tartarus is `tartarus`. The
  campaign `elite` directory is the player key `elite` and keeps the
  multiplayer model; the campaign chief is `masterchief`.

### glb

- Axes and units as `map.glb`: game `(x, y, z)` is glTF `(x, z, -y)`, game
  units kept.
- Nodes: `root`, whose children are the mesh node (`body` for a biped,
  `weapon` for a weapon) and empty nodes with `translation` and `rotation`
  only (on a biped, the skeleton's root joints too). Bipeds have `weapon`
  (the right-hand marker: a weapon glb placed under it sits in the hand)
  and `head` (the head marker), each the child of the joint it rides on
  (`joint_r_hand`, `joint_head`, ...; the root's child when it has none); weapons have
  `muzzle` (the `primary_trigger` marker, or the front of the model at mid
  height when it has none); vehicles (mesh node `vehicle`) have a
  `seat_<name>` node per seat marker (`seat_driver`, `seat_gunner`,
  `seat_passenger`, `seat_passenger_lf`, ...) where the model has them.
  Creatures without the usual markers still get both biped nodes: `weapon`
  at a node named with `hand` (right first), else the root; `head` at the
  top centre of the mesh.
- One primitive per material: `POSITION`, `NORMAL`, `TEXCOORD_0` (float),
  u32 indices. Biped primitives also have `COLOR_0` (float
  `VEC4`) when any of the model's materials has a change-colour map: red is
  the primary change-colour weight, green the secondary (0..1), sampled
  bilinearly from the material's change-colour map at the vertex. A
  material without a change-colour map has zero weights; weapons have no
  `COLOR_0`. These are weights, not colours: a reader must not multiply
  the base colour by them (the viewer turns vertex colours off).
- **Turrets.** A vehicle's turrets and guns are objects of their own that
  its model variant attaches (hlmt variants `+0x50`, 0x38 bytes: name
  string ID at `+0`, objects block at `+0x1c`, 0x10 bytes each: parent
  marker and child marker string IDs, then the child object reference).
  The variant is `multiplayer` when the model has one (the Scorpion's
  `cannon_mp`, the Wraith's `mortar_mp`, which has no side guns), else the
  object's default (`obje +0x30`), else the first (the Warthog's
  `default`: the chaingun; `gauss` and `troop` are other variants). Each
  child is built like a vehicle at rest, in its own colours, its child
  marker (or its origin, when it names none) on the parent's marker in the
  vehicle's pose, and its own attached objects in turn (two levels). An
  object attached to the vehicle itself is drawn as parts that aim (live
  art 18): a `turret_<n>` node at its attachment frame, extras
  `{"turret": "<tag stem>"}`, with a mesh of what turns with its yaw
  (everything but its guns, and what it carries in turn), and under it a
  `turret_<n>_pitch_<k>` node per gun, at that gun node's rest origin with
  the turret's axes, with a mesh of what hangs from it. The guns are the
  turret's nodes named `gun`, `cannon`, `gun_arm` or `turret_gun` with no
  such node above them (the Warthog's chaingun and gauss `gun`, the
  Scorpion's `cannon` and coaxial `gun`, the Wraith mortar's `gun_arm`,
  the Spectre's `turret_gun`); a triangle goes with its first vertex's
  heaviest node. A reader turns the yaw node about its own up (glTF +y)
  and each gun about its own side (glTF +z, raising it); at rest they
  are as the game models them. Parts are not skinned. Its materials join
  the vehicle's, and its seat nodes join the vehicle's (`seat_gunner` on
  the Warthog's chaingun) unless the vehicle has that name.
  `asset.extras.attached` lists the attached objects' tag names. On the disc the Ghost and Banshee attach nothing:
  their guns are in their hull models.
- **Vehicle colours.** Halo 2 paints a vehicle with its object's own
  change colours, not its driver's team (a red player's Banshee is as
  violet as anyone's). The `obje` change colours (`+0xac`, 0x10 bytes
  each: a block of initial permutations, then one of functions; a
  permutation is 0x20 bytes: weight, lower and upper bound RGB, variant
  string ID) give, per colour, the permutation of the object's default
  variant (`obje +0x30`), else the first, at the middle of its bounds. On
  the disc only the Banshee (violet and dark violet; the heretic variant's
  gold) and the Scorpion (grey, on no marked surface) have any. They are
  painted into the base maps as the game tints (per channel: one where
  the change-colour map is empty, the colour where it is full; red
  primary, green secondary), so a vehicle has no change-colour map and no
  `COLOR_0`.
- Materials (`material_look`): `baseColorTexture` (an embedded PNG, at most
  512 pixels a side) when the diffuse map decoded, else a grey
  `baseColorFactor`; metallic 0, roughness 0.8 unless:
  - the shader is a bump-env or shiny one (`tex_bump_env*`,
    `tex_bump_shiny`): the base map's alpha is Halo 2's specular mask, and
    becomes a `metallicRoughnessTexture` (green roughness 0.85 to 0.3,
    blue metalness up to 0.6 for env, 0.35 for shiny, red the mask
    itself); the base map is then stored opaque. The shader's reflection
    tint is `extras.h2.env` `{perpendicular, parallel}` (RGB 0..1): its
    last two colour parameters with any colour (compiled postprocess
    `+0x0c`, packed ARGB; those after carry only alpha). Facing the eye
    the reflection takes the first, at a grazing angle the second: the
    Covenant's sheen (Ghost orange to purple, Spectre pale yellow to blue,
    Wraith light to deep blue, Banshee blue to dark violet). The viewer
    adds the environment, clamped to 1 as the game's cube maps are low
    range, times the mask and the tint (`ENV_FRAGMENT`, at 0.5);
  - it has a self-illumination map (a bitmap named `*illum*`), or is a lit
    display (`illum`, `*meter*`, `*plasma*` templates): `emissiveTexture`
    (that map, or the base map) with `emissiveFactor` 1;
  - it is transparent (`shader_templates\transparent\...`): `*_add_*`
    templates are glows, `alphaMode` `BLEND`, black base, the base map as
    the emission and `extras.h2.blend` `"add"` (the viewer adds them);
    the others are glass, `BLEND` with a see-through blue-grey factor and
    no texture; `overlay` templates (decals) blend by their alpha;
  - it is an `overlay` decal on an opaque model whose pass blends by
    colour (render states `ALPHABLENDENABLE`, `SRCBLEND`, `DESTBLEND`;
    Xbox `D3DBLEND`): `DESTCOLOR`, `SRCCOLOR` doubles what is under it
    (`extras.h2.blend` `"double"`), `DESTCOLOR`, `ZERO` or `ZERO`,
    `SRCCOLOR` multiplies it (`"multiply"`). The material is
    `KHR_materials_unlit`, opaque, its picture as stored (mid-grey leaves
    the hull alone). Every vehicle overlay on the disc doubles (the
    Scorpion's `icon`, `number_decal`, `marine`, `no_step`, its cannon's
    `scorpion_decal`). The viewer blends it so, unlit, not tone mapped,
    without writing depth and pulled towards the camera (polygon offset);
  - it is any other `overlay` decal on an opaque model whose picture has
    no alpha and reads, as decal layers do
    (pipeline step 7), as doubling or multiplying: it is turned into one
    blended by alpha that looks about the same (`overlay_as_alpha`; by
    luminance l, double: white at l − 0.5 when l ≥ 0.5, else black at
    1 − 2l; multiply: black at 1 − l), not a grey slab;
  - it is alpha-tested (`*alpha_test*`): `alphaMode` `MASK` at 0.5, its
    alpha kept.
  The change-colour map (at most 256 pixels a side) is also stored, its
  texture index in `extras.h2.change_colour`, for readers that tint per
  pixel (the viewer does); `extras.h2.template` names the shader template.
  Each distinct picture is stored once. Bump maps (8-bit palettized
  normals; the palette is the engine's, not the tag's) and detail maps are
  not used.
- `asset.extras`: `generator`, `models_version` (2) and, for bipeds,
  `height` (game units, feet to the top of the head) and `lift` (game
  units, extractor 22): how far the model was raised to put its lowest
  vertex on the floor (pipeline step 7), positive when raised. The game
  draws a biped at its object origin, not lowest vertex on the floor, so
  a reader that lowers the model by `lift` puts the model's own origin at
  the object's position.
- A biped stands with its feet on the origin plane (lowest vertex at
  glTF y = 0) and faces glTF +x (game +x, yaw 0). It holds a rifle in its
  idle stance (or its own idle, below), and that posed shape is what the
  vertices store, so a reader that ignores the skin draws it as it was in
  version 1.
- Bipeds are skinned (weapons and vehicles are not). One node per
  render-model node, named `joint_<node>` (`joint_pelvis`, `joint_r_hand`,
  ...), in the model's tree under `root`, their `translation` and
  `rotation` the default (idle) pose. One skin over all of them; its
  inverse bind matrices are the inverse of each joint's default frame, so
  the default pose leaves every vertex where it is stored. Primitives add
  `JOINTS_0` (u16 `VEC4`) and `WEIGHTS_0` (float `VEC4`, summing to 1; at
  most four nodes per vertex, as the game's).
- Biped animations, one per clip, named `<weapon_class>:<state>`: classes
  `rifle`, `pistol`, `missile`, `support`, `sword`, `unarmed`, `flag`,
  `ball` (what multiplayer weapons are held with; `dual`, vehicle seats and
  AI-only classes are left out), states `idle`, `walk`, `run`, `crouch`,
  `airborne`, `land`, `land_hard`, `grenade`, `melee`, `reload`, `fire`,
  `switch`, then the movement states (extractor 22) `run_back`,
  `run_left`, `run_right`, `walk_back`, `walk_left`, `walk_right`,
  `crouch_run`, `crouch_back`, `crouch_left`, `crouch_right`, each only
  where the graph has it; and `any:die`, `any:dead` for every class
  (before extractor 13, `die` per class was the dead pose). After those,
  one seat pose per vehicle seat mode of the graph, `any:seat_<mode>`
  with `<mode>` the engine's animation mode as the seat names it
  (`any:seat_warthog_d`, `any:seat_warthog_g`, `any:seat_ghost_d`,
  `any:seat_scorpion_p_lf`, ...; extractor 22): the seated rider, one
  frame. Every joint has a `rotation`
  channel (normalised i16 `VEC4`, the core glTF option) and a
  `translation` channel (float `VEC3`), `LINEAR`, one key per frame at
  30 fps; a channel that never changes in the clip has a single key at
  time 0, so a seat pose has one key at time 0 on every channel. The clip
  keeps the floor of the default pose (no root motion: the
  game moves the biped, not the animation).
- A vehicle is in its neutral pose (below), origin at the vehicle's object
  origin, facing glTF +x. `asset.extras.pose` names the frame used
  (`combat:any:any:closing, last frame`, or `rest pose`).

On the retail disc: spartan 3,042 triangles, 80 clips and 2.6 MB (0.8 MB
of it mesh and textures), elite 2,744 triangles, 58 clips and 2.8 MB
(`ball`, `flag` and `unarmed` dropped for size; the dervish and heretics
likewise), 22 weapons of 430 to 1,400 triangles and 60 to
380 KB; 34 campaign characters of 330 to 4,800 triangles, 0 to 56 clips
and 0.2 to 2.8 MB; 38 vehicles of 26 to 4,750 triangles and 26 KB to 1.7 MB. The
whole step takes about 0.6 s on 10 cores. (Counted at extractor 21,
before the movement clips and seat poses; the disc test prints the
current counts.)

## Pipeline

1. **Bipeds.** On the multiplayer maps, the first `bipd` tag whose file name
   is `masterchief_mp` (else `masterchief`) is the spartan, `elite_mp` (else
   `elite`) the elite. Its object model (`obje +0x34`, docs/formats/objects.md) gives
   the render model (`hlmt +0x00`) and the animation graph (`hlmt +0x10`).
   Campaign characters: every `bipd` tag on the campaign maps, one per
   species key (above), built in parallel.
2. **Vehicles.** Every `vehi` tag of the shared, multiplayer and campaign
   maps, first one per stem. Posed like a biped from its animation graph:
   the last frame of `closing` (a vehicle's rest pose has its canopy or
   hatch open: a banshee's whole cab is tilted up), else the first frame of
   `opening`, else an `idle` (turrets, the enforcer); else the rest pose
   (the warthog, ghost, phantom and most turrets have no actions). Per region the permutation named `base` (else
   the first), the highest detail level under 40,000 triangles. Vehicle tags
   with fewer than 8 drawn triangles are placeholders and are skipped
   (two on the retail disc).
3. **Weapons.** The `weap` tags under `objects\weapons\` of the shared map
   and the multiplayer maps, except fixed emplacements (`\fixed\`), first
   one per stem. Vehicle guns live elsewhere and are not included.
4. **Geometry.** `tags::model::RenderModelGeometry`. Per region, the
   permutation named `base` (else the first); for bipeds the highest detail
   level whose drawn triangles fit 15,000 (the retail bipeds fit at their
   highest), for weapons the highest. Section-local node indices go through
   the section's node map. Parts of materials whose shader template name
   says `emblem` (the emblem decal) are left out.
5. **Skeleton.** Each render-model node's rest transform (translation at
   `+0xc`, quaternion at `+0x18`, relative to its parent) composed up the
   tree. The disc test checks the result against the inverse rest frames the
   tag stores (`+0x2c`: three axes and a position): they agree to 4e-7 on
   the spartan, which fixes the quaternion convention.
6. **Pose.** In the biped's graph: mode `combat`, class `rifle` (else
   `pistol`, `unarmed`), type `any`, action `idle`; else any `idle` action
   (the combat mode first); else any action with `idle` in its name; else
   the rest pose. An inherited action is read from its graph. Frame 0 is sampled (`tags::anim_sample`). Graph
   nodes are matched to model nodes by name; a channel the frame does not
   supply keeps the rest value. Sections whose vertices carry no node
   indices are rigid pieces of their section's first mapped node (a
   banshee's cab, a jackal's shield, a helmet), not of the root.
7. **Bake.** Linear blend skinning: each vertex is moved by the weighted sum
   of `posed_world × inverse(rest_world)` of its nodes (weights
   renormalised), normals likewise. Then the model is lifted so its lowest
   vertex is at height 0. The same weights become `JOINTS_0`/`WEIGHTS_0`,
   and the posed frames (lifted) the joints' defaults and bind matrices.
8. **Markers.** Render-model header `+0x58` is a block of marker groups
   (0xc bytes: string-ID name, block of markers); a marker is 0x24 bytes,
   node index at `+2`, translation `+4`, quaternion `+0x10`, relative to its
   node. On the retail models every name resolves and every node is in
   range. `weapon` is the posed `right_hand` marker (`right_hand_elite` on
   the elite; else a node named `r_hand`), `head` the `head` marker. A
   vehicle marker is a seat when its first word is `driver`, `gunner`,
   `passenger`, `seat`, `pilot` or `rider` and no word says it is something
   at the seat (`enter`, `camera`, `target`, `hand`, `foot`, `damage`, ...);
   the node is `seat_` plus the marker's words joined by `_`, first marker
   of a name. Phantoms, Pelicans and some turrets have no seat markers.
9. **Clips.** In the biped's graph and the graphs it inherits from (its
   own first). The locomotion states are actions of type `any`: `idle`,
   `walk_front` (walk), `move_front` (run), `airborne` (the whole leap:
   the game has no take-off clip), `land_soft` (land), `land_hard`
   (land_hard) in mode `combat`, and `idle` in mode `crouch` (crouch).
   The movement states are actions of the same kind, after a class's
   other states: `move_back`, `move_left`, `move_right` (`run_back`,
   `run_left`, `run_right`), `walk_back`, `walk_left`, `walk_right` (the
   same names) in mode `combat`, and `move_front`, `move_back`,
   `move_left`, `move_right` in mode `crouch` (`crouch_run`,
   `crouch_back`, `crouch_left`, `crouch_right`).
   Seat poses: a mode is a vehicle seat when it is not `combat` or
   `crouch` and a `_`-separated word after its first is `d`, `g` or `p`
   (driver, gunner, passenger: `warthog_d`, `spectre_g`, `scorpion_p_lf`,
   `c_turret_ap_d`, `pelican_p_l01`); generic, matched at run time. Each
   seat mode in graph order gives `any:seat_<mode>` from its `idle`
   action: class `rifle` (a passenger holding a weapon), else `any`, else
   the first class the mode has, type `any` before the others. Only its
   first frame is sampled. A seat mode without an `idle` (or whose idle
   is an aim grid) has no pose.
   Dying and lying dead are the same for every class, so they are written
   once as `any:die` and `any:dead`, first in the glb: `die` is the death
   animation of a soft kill from the front (`s_kill:front:gut`, its first
   variant; else `h_kill:front:gut`, else the dead pose), `dead` the
   action `landing_dead` of class `any` (the body on the ground; the viewer
   holds the death clip's last frame, and uses this pose for a body it
   never saw alive). These go
   through the action table, so `unarmed` plays what the graph
   maps it to (the spartan's sword animations). The weapon states are
   per weapon type in the game (their table is not decoded), so they are
   found by animation name, `combat:<class>[:<type>]:<action>[:varN]`:
   `throw_grenade` (grenade), `melee_strike_1` or `melee` (melee),
   `reload_1` (reload), `fire_1` (fire), `ready` (switch). The class's own
   animation comes first, then one of the types `br`, `hp`, `rl`, `bs`
   (battle rifle, magnum, rocket launcher, brute shot), then the first type
   by name. Aim grids (codec 8) are skipped. Every frame is sampled. A
   channel a frame lacks (an upper-body replacement's legs) keeps the
   default pose's value. An overlay (`fire_1`) adds its change since its
   first frame to the default pose: rotation `default × first⁻¹ × frame`,
   translation `default + frame − first`. Scale channels are dropped. A
   biped glb over 3.25 MiB is made to fit (`next_size_drop`): first the
   movement clips of every class but `rifle` go, a class at a time, last
   of the class list first (`ball`, `flag`, `unarmed`, `sword`,
   `support`, `missile`, `pistol`); then whole classes, last first
   (`ball`, `flag`, `unarmed`, ...; the rifle's movement goes only with
   the rifle class); only then the shared `any` clips (dying and the seat
   poses, which are one frame each and cost little). The elite keeps its
   sword clips; the report's animation note names what was dropped
   (`ball moves`, `flag`, ...).
10. **Weapon class.** The string ID at `weap +0x288` (D: every multiplayer
    weapon names a class of the player graphs there: `rifle` for the
    battle rifle, carbine, shotgun, SMG and the snipers, `pistol` for the
    magnum, needler and plasma pistol, `missile` for the rocket launcher and
    fuel rod, `support` for the brute shot, `sword`, `flag`, `ball`; vehicle
    guns say `fixed`).
11. **Textures.** The shader's bitmap parameters (`shad +0x20` postprocess,
   `+4` block, datum first; docs/formats/shader-instances.md). The compiler
   shares identical compiled data: a shader's postprocess may lie in
   another shader's payload (the Scorpion's right and spare treads point
   into its left tread's; the Wraith's boost glow and damage shaders do
   the same), so the blocks are read from the map's metadata wherever
   they are, not only the tag's own bytes. A bitmap's role comes
   from generic words in its tag name read at run time: engine defaults
   (`default_bitmaps`, `rasterizer`) are never used; a name ending in
   `_cc` or saying `change_color` (or `color_change`) is the change-colour
   map; one saying `illum` the self-illumination map; the first name
   that is not a bump, detail, reflection, cube, mask or engine map is the
   diffuse map. Decoded with `assets::decode_image` (DXT and the other
   formats the art already decodes; the largest stored level). The look
   follows from the template's name (above).

## Checking against your disc

```sh
cargo test --release -p cli --bin h2 models_from_the_disc -- --ignored --nocapture
```

writes every model, `report.txt` (sources, pose, triangle counts, sizes),
`models.json`, `seats.txt` (each vehicle's seat nodes),
`check-<species>.png`, `check-vehicle-<stem>.png`, `animation.txt` (each
biped's size and clips, each weapon's class, and the checks below) and
`strip-spartan.png` (six frames each of `rifle:run`, `rifle:grenade` and
`rifle:melee`, skinned from the glb as a viewer would) to
`$RUSH_KEEP/players-models` (a directory outside the repository). Each
picture shows the model from the front, the right side and three-quarter
with its diffuse texture, then the change-colour weights (red primary, blue
secondary over grey). Bipeds hold the battle rifle at their `weapon` node;
vehicles have the spartan standing at `seat_driver`.

It also checks the
spartan's file: the `gltf` crate reads its skin and clips, playing
`rifle:idle` frame 0 moves no vertex (the default pose is that frame), and
every `rifle:run` key at frames 0, 7, 13 and the last, back in game axes,
is the rotation `tags::anim_sample` gives the node (to 6e-8). It checks
true scale (the spartan 0.6 to 0.8 units tall, the elite taller, the
Warthog 1.5 to 3 long), the looks (change-colour maps on the spartan, elite
and banshee, the elite's glow, the Warthog's glass, the spartan's shine),
that the elite keeps its sword clips, that the spartan has `rifle:run_left`,
`rifle:run_back`, `rifle:crouch_run` and `any:seat_warthog_d` and the
Warthog's `seats` a `warthog_d` (it prints every vehicle's seats, the
spartan's seat poses and every biped's `lift`), and the battle rifle's, plasma rifle's
and rocket launcher's ballistics. `shaders_from_the_disc` (same flags)
writes `shaders.txt`: every player biped's and vehicle's materials with
their template, bitmaps (name, format, size, role) and instance colours and
vectors; `live_weapons::tests::weapons_from_the_disc` writes `weapons.txt`.
Do not commit anything they write.

## Limits

- The default pose is the rifle idle, whatever the player holds; a viewer
  that does not play the class's clips shows a pistol or sword in a rifle
  grip.
- Clips are whole-body and one at a time: the game layers aim, overlays
  and upper-body replacements over locomotion, a viewer playing these
  clips does not. `fire` is the overlay applied over the default pose;
  `reload`, `melee` and `fire` are one representative weapon type's for the
  whole class (the battle rifle's for `rifle`, and so on), and `any:die`
  is one death animation for every way of dying (multiplayer deaths are
  ragdolls in the game). Strafing, backwards and crouched movement are
  separate clips (extractor 22), not blended by speed or direction; a
  glb over the size cap may lose them for its less held classes. No
  turning clips, no aim. Characters without a rifle stance
  use their own idle (grunts the pistol one, hunters and flood forms the
  unarmed one, prophets and sentinels a combat idle); Lord Hood has no idle
  and stands in his rest pose.
- Vehicles are posed once: no wheel suspension. Turrets aim by their
  parts (above), at rest until a frame says where; the gun nodes are
  chosen by name, so a turret whose gun is named otherwise only turns in
  yaw, and pitch is about the gun node's rest origin, which may not be
  the game's exact hinge. A Warthog is always the chaingun one (a gauss
  Warthog is the same tag in another variant). A
  vehicle with no seat markers has no `seat_` nodes, and a seat node is the
  marker's frame; the rider's seated pose is the biped's
  `any:seat_<label>` clip, one still frame (no steering, aiming or
  entering), and the seat's mode is matched by its name's `_d`, `_g`,
  `_p` words.
- `COLOR_0` change-colour weights are per vertex, so their edges are as
  coarse as the mesh; the per-pixel map in `extras.h2.change_colour` is
  exact. Only the primary and secondary colours are tinted.
- No bump or detail maps; reflections are the reader's environment, not
  the shader's cube map, tinted by the shader's last two colours; other
  colour parameters are not read, so the chief's visor is the diffuse
  map's dark colour, not gold. The Ghost's red glyph is in its base map
  (no decal): it is drawn as the disc paints it. The Warthog's olive
  hubs are its tyre map's colour; its rubber and dirt detail maps would
  vary it but are not drawn.
- A vehicle is drawn in its default variant: the Banshee's indexed base
  map shows its first image (the heretic Banshee's is the second).
- Elite helmets and other permutations: only `base` (or the first).
