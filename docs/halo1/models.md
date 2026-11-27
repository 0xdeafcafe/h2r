# Halo 1 models and animations as Halo 2 tags (`crates/tools/h1-assets`)

Status: **Built** for Halo 1 Xbox `mode` -> Halo 2 `mode` and
`antr` -> `jmad`, including compressed (keyframed) animations. It is proven
by decoding the converted tags through `crates/formats/tags` and comparing
them with the source; the converted maps load in the game
([campaign.md](campaign.md), [multiplayer.md](multiplayer.md)), and what
only a game run shows is under "Needs a game run".

This is the model and animation step of the Halo 1 campaign work
([campaign.md](campaign.md) §2). The Halo 1 cache container is in
[map-format.md](map-format.md), the Halo 2 targets in
[map-format.md](../formats/map-format.md) ("Render-model geometry"),
[structure-bsp.md](../formats/structure-bsp.md) and [animation.md](../formats/animation.md), and
the builder the output feeds in [map-builder.md](../formats/map-builder.md).

No map bytes, tag dumps or extracted assets are in the repository. The
tests build invented tags in memory. The `#[ignore]` tests read real maps at
run time and print only counts and error figures.

## Evidence labels

- **V1:** checked on the owner's Halo 1 Xbox multiplayer maps
  (`beavercreek.map`, `carousel.map`) by the ignored tests: every one of
  their 103 `mode` and 39 `antr` tags reads and converts, and the values
  read pass the stated checks.
- **V2:** checked on retail Halo 2 `mode` tags (`lockout.map`) by the
  ignored test `retail_h2_mode_layouts`.
- **V3:** checked against the `jmad` mode/class/type/action trees of every
  map on the owner's retail disc (30 maps, 2,268 graphs) by the ignored test
  `retail_h2_animation_names` in `crates/tools/h1-assets/src/models/names.rs`.
- **V4:** checked on every map of the owner's Halo 1 disc (24 maps, 628
  graphs, 186 compressed animations) by the ignored test
  `h1_disc_compressed_animations_decode`.
- **V5:** checked on every biped and first-person model of the disc's a10
  and Blood Gulch by the ignored tests in `crates/tools/h1-assets/src/models/skin_tests.rs`,
  which print only counts and error figures.
- **X:** read from the Halo 1 Xbox XBE's own code (disassembled from
  `default.xbe` on the disc; addresses are virtual addresses in that XBE).
- **S:** field order and meaning unverified, checked against the data
  where a V label says so.
- **Design:** an implementation choice.

## Halo 1 Xbox `mode` (render model)

Xbox maps use the `mode` group. There are no `mod2` (gbxmodel) tags on the
two maps. Xbox `mode` differs from PC `mod2` in its vertex form and in having
no local node table. Every block is the usual 12-byte block: count, address,
unused.

### Header (`0xe8`)

| Offset | Field | Evidence |
|---|---|---|
| `00` | u32 flags | S |
| `04` | u32 node list checksum (animations carry the same value) | V1 (equal on the cyborg's model and animations) |
| `08` | f32 x5 LOD cutoffs, super high to super low | V1 (5, 50, 120, 250, 400 on the cyborg) |
| `1c` | i16 x5 LOD node counts | S |
| `30` | f32 x2 base map u, v scale; texture coordinates are multiplied by it | S, V1 (finite) |
| `ac` | markers, stride `0x40` | V1 |
| `b8` | nodes, stride `0x9c` | V1 |
| `c4` | regions, stride `0x4c` | V1 |
| `d0` | geometries, stride `0x30` | V1 |
| `dc` | shaders, stride `0x20`: tag reference (`00`), i16 permutation (`10`): the per-sampler image index on a model material and the digit a numeric `schi` draws ([objects.md](objects.md), "Ammunition counter") | V1 (references name `soso`/`schi`/... tags); the assault rifle's first-person model names its `numbers` twice, permutations 1 and 0 |

The H1 draw uses a nonzero texture override from the object
(`obje +0x13e`, copied at `0x8febc`), otherwise the model shader's
permutation. Each bitmap takes the signed remainder by its own image count
at `0x8038b`. The converted b30 floor light fixtures bind the second image
of both their base and reflection mask. Their detail and cube have one
image each, shared with the wall and ceiling fixtures. The retail model
survey found six distinct `soso` materials with multi-image bases, including
four Grunt materials, and one `senv` (`levels\a50\shaders\covenant metal panel`). No nonzero actor-variant override at `actv +0x20c` targets a
model using one of these seven materials.

**Marker** (`0x40`): `char[32]` name, `34` block of instances (`0x20`):
i8 region, i8 permutation, i8 node, pad, f32x3 translation (`04`), f32x4
rotation (`10`). In a cache file the permutations' own marker blocks are
empty and every marker is here. V1.

**Node** (`0x9c`): `char[32]` name, i16 next sibling (`20`), first child
(`22`), parent (`24`), f32x3 default translation (`28`), f32x4 default
rotation `i, j, k, w` (`34`), f32 distance from parent (`44`). From `48`,
cache-computed values: 8 zero words, then at `68` the inverse of the
node's model-space rest frame (f32 scale 1, three f32x3 vectors that are
the inverse's **columns**, f32x3 position). The reader does not use them;
the skin test checks against them. V1: links in range, unit rotations;
V5: the inverse at `68`.

**Rotations are the conjugate of Halo 2's.** Every Halo 1 rotation (node
defaults, marker instances, animation frames) is the conjugate of the
quaternion Halo 2's rule (`mode::quat_matrix`, the rule the retail
`mode` inverse frames fix, V2) means. V5: composing the disc's bipeds'
nodes by Halo 2's rule puts their vertices about 3x farther from their own
node than the conjugate does (cyborg 0.267 against 0.084 units, grunt
0.275 against 0.090, elite 0.388 against 0.076), and only the conjugate
reproduces Halo 1's cached inverse frames at node `68` (to 7e-7 on every
biped). `h1::h2_rotation` converts; **the reader returns every rotation
already in Halo 2's convention**, so every consumer (the `mode` and
`jmad` writers, `hlmt` nodes, h1objects' physics and marker placement)
composes with Halo 2's rule. Copied unconverted, the rotations would leave
the rest pose matching (the inverse frames would be computed with the same
wrong rule) but put every animated pose's body parts in the wrong place, up
to 1.2 units off on the cyborg ([notes/models.md](notes/models.md)).

**Region** (`0x4c`): `char[32]` name, block of permutations at `40`.
**Permutation** (`0x58`): `char[32]` name, u32 flags (`20`), i16 x5
geometry index per LOD at `40`: super low, low, medium, high, super high.
V1: each is -1 or a geometry. On the cyborg the order runs 4, 3, 2, 1, 0,
so geometry 0 is the most detailed.

**Geometry** (`0x30`): u32 flags, block of parts at `24`.

**Part** (`0x68`):

| Offset | Field | Evidence |
|---|---|---|
| `00` | u32 flags | S |
| `04` | i16 shader index; `06`, `07` i8 previous/next filthy part | V1 (index in range) |
| `08` | i16 x2 centroid primary/secondary node, `0c` f32 x2 their weights | V1 (weights sum to 1) |
| `14` | f32x3 centroid | S |
| `20`, `2c`, `38` | blocks of uncompressed vertices (`0x44`), compressed vertices (`0x20`), strip rows (`6`). **Empty in cache files** | V1 |
| `44` | u32: 1 on every part (index buffer type) | V1, not interpreted |
| `48` | u32: strip length **less two** | V1: with this count every index is below the vertex count and the strip is followed by `ffff` padding |
| `4c` | u32: address of the strip's u16 indices | V1 |
| `50` | u32: address of this part's entry in the index table (index header `+0x1c`) | V1, not needed |
| `54` | u32 vertex type; 5 on every part (compressed) | V1 |
| `58` | u32 vertex count | V1 |
| `60` | u32 offset of the vertices in the model data | V1 (consecutive parts differ by count x 32), not needed |
| `64` | u32: address of this part's entry in the vertex table (index header `+0x14`); the entry's `+4` is the vertices' address | V1 |

The vertex and index tables have one 12-byte entry per part (index header
`+0x10` gives the count). Only the address at `+4` means anything; the
other words are uninitialised (`cacacaca` and similar). The reader still
accepts the in-tag vertex and strip blocks when they are populated, as in
source tags (not seen in cache files).

**Compressed vertex** (32 bytes; V1: unit normals, nodes in range, weights
in [0, 1]):

| Offset | Field |
|---|---|
| `00` | f32x3 position, model space at the rest pose |
| `0c`, `10`, `14` | normal, binormal, tangent: packed 11:11:10 (x: signed 11 bits at bit 0, y: 11 at bit 11, z: 10 at bit 22; divided by 1023, 1023, 511) |
| `18` | i16 x2 texture coordinate, divided by 32767 |
| `1c`, `1d` | i8 node 0, node 1, **stored times three**; negative means none |
| `1e` | i16 weight of node 0, divided by 32767; node 1 gets the rest |

**Strips.** Indices form one strip per part with degenerate joins. Halo 1
strips have the **opposite parity** to Halo 2's: the faces that agree with
their normals are `(a, c, b)` for even triangles and `(a, b, c)` for odd
ones. V1: with Halo 2's rule, 99% of triangles on the cyborg, assault rifle
and warthog face away from their own normals. With this rule, 52 of 5,692
(cyborg), 0 of 940 (assault rifle) and 38 of 7,597 (warthog) do.

## Halo 1 Xbox `antr` (animation graph)

Moved to [Halo 1 models: animation graphs and Halo 2 output](models-animation.md#halo-1-xbox-antr-animation-graph).

## Halo 2 output

Moved to [Halo 1 models: animation graphs and Halo 2 output](models-animation.md#halo-2-output).

## Campaign biped idles and talking (Built)

The ten Xbox campaign maps reference 25 distinct animation graphs from
their biped object layers. On a10, crew and armored marines share the marine
graph. Keyes and Cortana have their own. The pilot and cinematic Chief
reference graphs in the cinematics directory. The biped's animation
reference selects the conversion, including graphs shared by several models.

Halo 1 selects a resting pose from class slot 0 and directional cycles
from slots 8–11. Keyes's standing slot points to `stand unarmed idle%1`.
The marine's standing unarmed slot points to an alert pistol idle, and
its bridge-chair mode has its own seated idle. Splitting labels alone
would give a variant key without an `idle` key and merge standing and
alert labels. Halo 2 searches the actual action key and tries the
eight combinations of specific and wildcard mode, class and weapon type
(`0x1db170`, `0x2963f0`). The converted graph binds each source slot
to that key, including `patrol` for the alert pose and the rebuilt vehicle's
seat label for a passenger. Explicit script pose requests use the same mode.
When a rebuilt weapon carries the cyborg's class, the exact weapon type
still selects the owner's pose. A marine holds the Needler with its rifle
idle, and an Elite holds the plasma rifle with its pistol idle.

For a wildcard class, conversion uses the mode's unarmed idle when present,
then the first class with an authored idle. For unspecified modes it uses
the standing pose. The x30 pilot has an explicit standalone base animation
named `idle` and an empty standing slot. A cinematic actor without a walking
cycle keeps its authored base when asked to move. Conversion reports a
missing or ambiguous base. Base clips retain their loop and permutation
links. The graph uses the model's node order, enables the biped channels
and partitions the left arm from the remaining nodes. Xbox H1 can encode
the graph root as its own parent. H2 terminates it with -1, as does the H1
model. All other hierarchy links agree in the campaign biped/model pairs.

Halo 1 unit slot 10 supplies a weighted talk overlay. The converted
`talk` entry is in the weapon type's overlay block. Keyes's pose rotates
`frame jawbone`. Cortana's also translates her mouth corners. The shared
marine graph moves its own head and facial rig. Wounded marine graphs
without a talk slot retain their authored poses. Sound timing and native
playback are documented in [sounds.md](sounds.md#dialogue-mouth-curves-built).

The ignored `campaign_biped_slots_and_talk_frames` check follows every
campaign biped reference, checks the installed action and overlay lookups,
and decodes a10's talk frames against the original animated channels.

## Animation names

A Halo 1 name is split on spaces: `<unit> [<weapon class> [<weapon type>]]
<action>`, with a `%n` suffix for a random variant. The Halo 2 name is
`mode:class:type:action`, leaving out `any` segments. `-` becomes `_` and
names are lower-cased. A name that an earlier animation already produced
gets `:var1`, `:var2`, ... (Halo 1 repeats names for permutations: three on
the cyborg).

| Halo 1 | Halo 2 | Status |
|---|---|---|
| unit `stand` | mode `combat` | V3 (`combat:rifle:any:idle` and the movement actions on the retail masterchief/elite graphs) |
| unit `alert` | mode `combat` (no alert mode exists in retail) | record labels share the standing prefix; the cyborg binds the rifle override in `patrol` |
| unit `crouch` | `crouch` | V3 (`crouch:rifle:any:idle`) |
| `W-driver`, `W-gunner`, `W-passenger` | `warthog_d`, `warthog_g`, `warthog_p` | V3 (`warthog_g:fixed:any:idle` proves the gunner seat and the `fixed` class together) |
| `G-driver`, `B-driver`, `scorpion-driver` | `ghost_d`, `banshee_d`, `scorpion_d` | V3 (all three are modes of the retail biped graphs) |
| `CT-driver` | `c_turret_ap_d` | V3 for the name (`c_turret_ap` is the retail covenant turret, its jmad on every multiplayer map); the choice between it and the rarer `c_turret_mp_d` is S |
| `GT-gunner` | `guntower_d` | V3 for the name (03a elite graph); the choice is S (retail also has `h_turret_ap_d`/`h_turret_mp_d`) |
| `P-riderLF`, `P-riderRF` | `pelican_p_l01`, `pelican_p_r01` | V3 for the names; 01 = front row is S (retail numbers each side's seats 01..05) |
| `scorpionLF` | `scorpion_p_lf` | V3 (`scorpion_p_lf:any:any:enter`; retail also has `_p_lb`, `_p_rf`, `_p_rb`, which no Halo 1 name feeds) |
| weapon `unarmed`, `pistol`, `rifle`, `missile`, `fixed` | the same class | V3 (classes of the retail biped graphs; `fixed` under `warthog_g`) |
| third word before the action (`stand rifle sg reload-1`) | weapon type | V3 for the labels Halo 1 and Halo 2 share (`pr`, `sg`, `sr`, `hp`, `ne`, `pp`, `rl`, `f`); `ar` has no Halo 2 type (the assault rifle became several weapons) |
| action `reload` / `reload-1` | `reload_full` | record label; the cyborg binds the replacement under third-person overlay `reload_1` |
| action `melee` | `melee` (unchanged) | V3 (`combat:rifle:any:melee`, `combat:pistol:any:melee` on the retail elite; the cyborg binds its weapon melee under overlay `melee_strike_1`) |
| action `throw-grenade` | `throw_grenade` | V3 (`combat:sword:any:throw_grenade` on the masterchief) |
| vehicle actions `stand opening` / `stand closing` | `combat:opening` / `combat:closing` | V3 - the only actions any retail vehicle graph carries (banshee, scorpion, pelican, `c_turret_ap`, the phantom and wraith turrets) |
| `first-person <action>` (a weapon's first-person graph) | mode, class and type `any`, animation `first_person:<action>`; `melee` becomes `melee_strike_1`; the graph's `+0x54` the one pair (`any`, `any`) | V3 (every retail first-person graph on Coagulation, 34, has the one mode `any` and that pair; h1objects `first_person_graphs_take_retail_modes`, which lists the converted actions retail has and has not: 19 shared on Blood Gulch, and `ammunition`, `enter`, `firing`, `light_off`, `moving`, `overcharged_jitter`, `overheating_again`, `overlays`, `reload_full2`, `stealth_melee` it lacks; `ammunition`, `moving`, `overcharged_jitter` and `overlays` are overlays, not actions, and the converter follows the AR’s empty firing slot) |
| `%n` | `:var<n>` on the action | Design; retail variant suffixes (`:var0`, ...) sit on animation names, never on tree actions |

**No Halo 2 equivalent, or not mapped:**

- Unit damage outside the cyborg still has only record labels. The
  cyborg uses the directional damage tables described above.
- Vehicle overlays (`suspension left back`, `vehicle steering`,
  `vehicle ground-speed`, `firing`, `acc-front-back` and kin): V3 that
  retail vehicle graphs carry only `combat:opening`/`combat:closing` as
  actions, no `suspension` or `vehicle` mode. Halo 2 finds the rest as
  overlays, in the suspension and object overlay blocks and by blend
  screen; the converter puts them there (Vehicle graphs, above).
- Seats that keep their Halo 1 label: `P-riderLM`, `P-riderLB`,
  `P-riderRM`, `P-riderRB` (their retail row number among `pelican_p_l01..05`
  is not recoverable from the trees) and the dropship seats
  `CD-passengerL01`/`CD-passenger01` (retail phantom seats are columns
  `phantom_p_a01..c03`, with no left/right correspondence).
- Other biped graphs use named actions and borrowed slots. The cyborg,
  sentinel and vehicle graphs have dedicated slot bindings.
- Root movement follows both channel flag sets in the rebuilt
  resource. H2 `0x20aa70` reads one record per frame, with strides of 8,
  12 and 16 for XY, XY/yaw and XYZ/yaw. The source values retain their
  signs and order. The synthetic and disc round trips compare every byte.
- Overlay and replacement animations repeat each non-identity
  default rotation, translation and scale in their animated stream. H2
  reads the static stream only on base animations. An undriven identity
  channel keeps its mask clear. The round-trip tests sample each frame
  through the H2 decoder and check both channel presence and value.
- Playback timing for the cyborg and first-person weapons is described
  above. Other graphs omit loop starts and foot contacts.

## API

```rust
use h1_assets::models::{graph_from_map, model_from_map, mode::ModelOptions};

let map = h1map::H1Map::from_reader(file)?;
let tag = map.find(h1map::Group::new(b"mode"), "characters\\cyborg\\cyborg").unwrap();
let opts = ModelOptions { shaders: vec![/* H2 shad datum per H1 shader index */], ..Default::default() };
let (h1_model, converted) = model_from_map(&map, tag, &opts)?;
let mode_id = converted.install(&mut tag_set, "characters\\cyborg\\cyborg")?;

let antr = map.find(h1map::Group::new(b"antr"), "vehicles\\warthog\\warthog").unwrap();
let model = map.find(h1map::Group::new(b"mode"), "vehicles\\warthog\\warthog");
let (h1_graph, report, converted) = graph_from_map(&map, antr, model)?;
let jmad_id = converted.install(&mut tag_set, "vehicles\\warthog\\warthog")?;
```

A `Converted` is `{ group, root: map::build::Block, raw: Vec<(bytes,
align)> }`. Its raw fixups use `RawRef::Local(RawId(i))` with `i` indexing
its own `raw`, and `SELF_ID` stands for its own datum. `install` places the
raw resources in the set, rewrites both, declares the group, and adds the
tag. The lower-level pieces are `h1::Model::read`, `h1::Graph::read` (any
`h1::Memory`), `mode::convert_model` and `jmad::convert_graph`.
`jmad::GraphReport` lists each name mapping, the variants, and what was
dropped. `jmad::animation_names` gives each animation's Halo 2 name (and
whether it is a variant) without converting, so a script that names a
Halo 1 animation can name the converted one (h1campaign's cutscenes).

## Tests and measured error

`cargo xtask test -p h1-assets` runs the synthetic tests. The real-data
tests build a v8 map in memory with `map::build`, open it with `map::Map`,
decode through `tags::model::RenderModelGeometry` and
`tags::anim::AnimationGraph`, and compare with the Halo 1 source:

```sh
H1_MAPS=<dir with beavercreek.map, carousel.map> H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso [H2_MAP=lockout] cargo xtask test --disc -p h1-assets
```

The geometry test compares every vertex and triangle of every geometry at
all LODs: node names, hierarchy, default transforms, region and permutation
names, LOD sections, marker names and instances (exactly), per-triangle
indices, and the largest position, texture coordinate, normal and weight
errors, which are the i16 and u8 quantisation (about 1e-5 world units for
positions). The animation test compares every frame of every animation,
each channel the conversion carries; the rotation error is the i16
re-quantisation of normalised quaternions (6e-5 rad at most). The figures
are in [notes/models.md](notes/models.md).

**Retail control** (V5, `retail_h2_bipeds_pose_sanely`, 05a). The Halo 2
side of the round trip below is first held to retail data, so the round
trip cannot share an assumption with the converter: a retail biped's rest
frames times the inverse frames its `mode` stores, read as columns, are the
identity on every node (within 5e-7; read as rows, off by up to 2.0), and
its own retail animations, decoded by `crates/formats/tags` and composed
the same way, stretch the mesh's triangle edges little (median 1.0, 99th
percentile 1.5 to 2.4) and less than the same frames with conjugated
rotations (2.6 to 3.5), so the decoded quaternions are used as they are.

**Skinned round trip** (V5, `h1_disc_bipeds_skin`). Each model and its
graph are converted, built into a map and decoded through
`crates/formats/tags`; every vertex is skinned the Halo 2 way (decoded
position, node indices through the section node map, weights, the decoded
jmad frame composed by Halo 2's rule, times the inverse frame the `mode`
stores at node `+0x2c`, read as the control reads retail's) and compared
with the Halo 1 vertex skinned the Halo 1 way (its frame composed by Halo
1's rule, times Halo 1's own cached inverse), at the bind pose and at the
middle frame of about seven base animations per graph. Errors are the
largest vertex distance in world units (a biped is about 0.7 tall); the
vertices travel up to 0.93 in those frames. What is left is the i16
quantisation of positions and rotations: under 1e-5 at the bind pose and
under 4e-4 posed, on the cyborg, the armored marine, crewman, elite, grunt,
captain, pilot, Cortana and the cyborg's first-person arms. Every bind-pose
vertex stays within the model's Halo 1 bounding radius. The per-model
figures are in [notes/models.md](notes/models.md).

### First-person graphs

**The engine binds by name** (E). When a weapon's first-person state is
set up (`0x00167e86`), `0x001dd4c0` builds one table per model, the
arms (the player representation's `+0x0`, `0x00168083`) and the weapon's
first-person model (`weap +0x2a8` element `+0x0`, `0x001680eb`): for each
`mode` node (`+0x48`, stride `0x60`), the index of the `jmad` node
(`+0x0c`, stride `0x20`) whose string-ID name is equal, else -1; it
returns whether every node was found, which sets the state's flag `4`
(arms) or `2` (weapon). `0x0016774f` then seeds each bound graph node's
pose from the model node's default rotation and translation. Node order
does not matter: retail graphs list the arms' 37 nodes first, then the
weapon's and `camera_control` (V2, 14 graphs on 05a), but that is the
tool's order, not a rule the engine needs. Halo 1's graphs interleave
them (the assault rifle's `frame gun` is node 7) and are kept in their
order.

Halo 2's arms have nodes (`l_upperarm`, ...) that match no node of a Halo
1 graph (`frame l upperarm`, ...), so the Halo 1 sandbox and campaign use
Halo 1's arms (docs/halo1/multiplayer.md, "First-person arms"), whose 37
names all occur in every Halo 1 first-person graph.

V5 (`h1_disc_first_person_skin`): Halo 1's arms and five weapons'
first-person models on Blood Gulch, with each weapon's converted graph,
bound by name, posed at the middle frame of base animations and skinned
both ways as for the bipeds. Every node of both models finds its graph
node. Largest vertex error per weapon over 5 to 8 animations: assault
rifle 1.2e-4, pistol 1.5e-4, plasma rifle 5.4e-5, needler 6.0e-5,
rocket launcher 6.3e-5 (the i16 quantisation). The inverse frames are
stored as columns (Nodes, above); stored transposed, the arms and the
weapons' first-person models would be drawn as a large flat shape over the
view.

**The first-person body** (the representation's `+0x8`) stays Halo 2's,
and the engine skips it for a Halo 1 player (E). The first-person draw
list (`0x00165f16`) reads the representation's arms (`+0x4`) and body
(`+0xc`) datums (`0x00165fdb`, `0x00165fde`), adds the arms when the
state's flag `4` is set and each weapon when its flag `2` is, and adds the
body last, posed with the third-person unit's own node matrices
(`0x000b8c00`), only when the body's `mode` and the unit's have the same
node count (`+0x48`, `0x001661d8`) and the same `+0x44` (`0x001661e0`).
Halo 2's body has the Chief's 33 nodes and his `+0x44` (V5, 05a); a converted Halo 1 biped (the
cyborg, 19 nodes) does not, so the body is never drawn. Halo 1 has no
first-person body.

## Needs a game run

- Whether the engine draws a model whose PRT and other
  unread header blocks are empty, with header flags 0, and whether it needs the section blocks
  the decoder skips.
- Part kinds 2 and 4 for opaque and transparent, the part fields from
  `0e`, and the section record's `24`..`2c`.
- Marker group order: retail may expect it sorted by name.
- Marker rotations take the same conjugation as nodes and animations
  (V5 proves it only for nodes and frames).
- Overlay semantics: Halo 1 overlays are copied verbatim (frame 0 is not
  subtracted). The engine may expect a different reference.
- Every name in the table above.
- How the player emblem on the converted cyborg looks in the running game.

## Animation sounds

Halo 1 keeps a graph's sounds in `antr +0x54` (`0x14` each: a `snd!`
reference and 4 bytes of padding) and gives an animation at most one:
`+0x3c` i16 the reference index (-1 none), `+0x3e` i16 the frame. V
(`weapon_sounds_disc::halo1_animation_sounds`, every map): every element
names a `snd!` at `+0` and nothing else. The first-person reloads,
readies and the rest of a weapon's handling sounds are these.

Halo 2 holds a graph's sound references at `jmad +0x14` (`0xc`: group,
datum, u16 flags) and each animation's sound events at `+0x54` (8: i16
reference index, i16 frame, string ID). The engine's event walk
`0x001c7450` reads the frame events (`+0x4c`, `+0x50`) and the sound
events (`0x001c7545` `+0x54`, `0x001c7564` `+0x58`) of the animation
and plays those whose frame the update passed (E). The string ID is
written empty (S: `retail_animation_sound_events` prints retail's).

`h1_assets::models::sounds` reads Halo 1's and, after `jmad::convert_graph`,
writes them into the converted graph (`add_sound_events`): the sounds
`h1objects` converted, in order, and one event per animation that plays
one; a sound that did not convert takes its events with it. The
weapons' first-person graphs and the vehicle graphs the object
converter builds carry them (`Converter::graph_sounds`,
`crates/tools/h1objects/src/convert_arsenal.rs`).
