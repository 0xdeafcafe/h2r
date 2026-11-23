# Models and animations: measurements

Evidence for [../models.md](../models.md).

All numbers come from the ignored tests named in the entries, run against
the owner's discs. The tests print counts and error figures only.

## 2026-11-20: campaign biped graphs

Commit: `fe15711b8`. Method: `campaign_biped_slots_and_talk_frames`, a
read-only walk of the ten Xbox campaign maps.

The maps reference 25 distinct animation graphs from their biped object
layers.

## 2026-11-19: the emblem placement on Blood Gulch's cyborg

Commit: `2bc3e05c5`. Method: `h1objects/tests/model_shaders_disc.rs`
`bloodgulch_cyborg_wears_the_chiefs_emblem_on_its_left_shoulder`, reading
the converted model.

The anchor joint (`bip01 l upperarm`) is at (-0.008, 0.09, 0.569) and the
arm's x axis is (0.01, 0.19, -0.98).

## 2026-11-18: first-person skinning and graph binding

Commits: `680d3f6e8` (first-person), `ffa90c4eb` (first-person actions on
Blood Gulch). Method: `h1_disc_first_person_skin`, `h1objects`
`first_person_graphs_take_retail_modes`.

- Halo 1's arms and five weapons' first-person models on Blood Gulch,
  with each weapon's converted graph bound by name, posed at the middle
  frame of base animations and skinned both ways. Every node of both
  models finds its graph node. Largest vertex error per weapon over 5 to 8
  animations: assault rifle 1.2e-4, pistol 1.5e-4, plasma rifle 5.4e-5,
  needler 6.0e-5, rocket launcher 6.3e-5 (the i16 quantisation).
- Retail Halo 2's first-person graphs on 05a: 14 list the arms' 37 nodes
  first, then the weapon's and `camera_control`.
- Every retail first-person graph on Coagulation (34) has the one mode
  `any` and the one pair (`any`, `any`). The converted actions retail has
  and has not: 19 shared on Blood Gulch, and `ammunition`, `enter`,
  `firing`, `light_off`, `moving`, `overcharged_jitter`,
  `overheating_again`, `overlays`, `reload_full2` and `stealth_melee` it
  lacks (the test's wording).

## 2026-11-18: skinned round trip of the bipeds

Commits: `19e863ad4` (retail control, inverse frames as columns, converter
version 59), `374d881bc` (rotations as the conjugate, converter version 54).
Method: `retail_h2_bipeds_pose_sanely` (05a) and `h1_disc_bipeds_skin`
(Blood Gulch and a10), reproduced with the commands in
[../models.md](../models.md), "Tests and measured error".

Retail control: a retail biped's rest frames times its stored inverse
frames, read as columns, are the identity on every node (within 5e-7); read
as rows, as the converter wrote them before version 59, they are off by up
to 2.0. Its own retail animations, decoded and composed the same way,
stretch the mesh's triangle edges little (median 1.0, 99th percentile 1.5
to 2.4) and less than the same frames with conjugated rotations (2.6 to
3.5).

Errors are the largest vertex distance in world units (a biped is about 0.7
tall); the vertices travel up to 0.93 in those frames. Bind pose, and the
middle frame of about seven base animations per graph:

| Model | Vertices | Our inverse frames vs Halo 1's | Bind pose | Posed, version 53 | Posed, version 54 |
|---|---|---|---|---|---|
| cyborg | 6,025 | 4.2e-7 (was 1.2) | 5.9e-6 | 0.64 to 1.16 | 1.4e-4 to 2.8e-4 |
| marine (armored) | 13,666 | 4.8e-7 (was 1.7) | 7.0e-6 | 0.53 to 0.85 | 7.3e-5 to 1.8e-4 |
| crewman | 1,578 | 4.8e-7 | 6.9e-6 | 0.53 to 0.85 | 7.4e-5 to 1.8e-4 |
| elite | 10,818 | 7.2e-7 (was 1.9) | 7.1e-6 | 0.54 to 1.00 | 1.4e-4 to 3.5e-4 |
| grunt | 8,065 | 3.5e-7 (was 1.9) | 6.8e-6 | 0.41 to 1.15 | 9.0e-5 to 2.8e-4 |
| captain, pilot, Cortana | 1,036 to 1,747 | under 7e-7 | under 7e-6 | 0.15 to 0.78 | under 2e-4 |
| cyborg first-person arms | 1,440 | 8.9e-8 | 2.9e-6 | (no graph of its own) | |

Before converter version 54 the rotations were copied unconverted: the
rest pose matched, but every animated pose put each body part up to 1.2
units off on the cyborg (the "exploded" bipeds and first-person arms).
Before version 59 the arms and the weapons' first-person models carried the
transposed inverse frames, so the engine drew them as a large flat shape
over the view, and a rotated node's vertices were skinned by R twice.

## 2026-11-17: compressed animations and the section group

Commits: `6671d5c89` (compressed animations), `bf8589cea` (section group).
Method: `h1_disc_compressed_animations_decode` on every map of the Halo 1
disc; `retail_h2_mode_layouts` on 05a.

Compressed animations: 186 on 24 maps' cinematic graphs (628 graphs in
all), the pilot, captain, chief, Cortana, marines and others on a10, a30,
a50, b40, c40, d20 and d40; none on the multiplayer maps. `[+88]` is 0 on
all 186. 3,031 channels, 17 with no keys.

| Check | Result |
|---|---|
| Animations, frames, keys | 186, 31,596, 202,404 |
| a50's chief `B30HoloMapShort` (compressed) against a30's copy on the b30 graph (uncompressed, same 130 frames and channels) | max 0.0026 rad, translation 1.3e-4 |
| a10's chief x30 `stand rifle idle` (compressed) against the cyborg's (uncompressed, same 59 frames) | max 0.0028 rad, translation 0 |
| Converted to `jmad` and decoded by `crates/formats/tags` | max 6.1e-5 rad, translation and scale 0 |

The two uncompressed copies agree to under 0.003 rad, the keyframe
reduction's own loss. Undriven channels sit at the model's rest pose (same
node list) for 1,426 of 2,007 undriven rotation channels (within 0.01 rad).
The pilot's `aye_aye_sir` has 21 rotation statics, 10 rotation channels and
no scale statics.

Section group: all 181 of 05a's `mode`s have the one section group. Retail
sets header flag `0x8` on all 181 and `0x4` on 167 (on Coagulation: `0x4`
on 103 of 121 `mode`s, 17 of the 22 first-person ones; `0x8` on all 121).

V3 and V4 coverage: V3 reads the `jmad` trees of 30 maps, 2,268 graphs. V4
reads 24 maps, 628 graphs, 186 compressed animations. The disc's vehicle
graphs (86 on the campaign and multiplayer maps) hold animation types 0 and
1 only.

## 2026-11-16: the first round trips

Commit: `5c220c8d6`. Machine: the owner's beavercreek and carousel maps;
lockout for the Halo 2 side. Method: the ignored tests in
[../models.md](../models.md), "Tests and measured error".

V1 coverage: 103 `mode` and 39 `antr` tags on the two maps all read and
convert.

Every vertex and triangle of every geometry, all LODs (beavercreek):

| Model | Nodes | Markers | Geometries | Vertices | Triangles | Max position error | Texcoord | Normal | Weight |
|---|---|---|---|---|---|---|---|---|---|
| cyborg | 19 | 8 | 5 | 6,025 | 5,692 | 5.4e-6 | 7.6e-6 | 0 | 2.0e-3 (u8) |
| assault rifle | 3 | 9 | 3 | 1,075 | 940 | 2.2e-6 | 7.6e-6 | 0 | 0 |
| warthog | 18 | 26 | 20 | 10,396 | 7,597 | 1.5e-5 | 1.5e-5 | 0 | 2.0e-3 (u8) |

Animations, every frame of every animation, each channel the conversion
carries:

| Graph | Animations | Frames | Max rotation error | Translation | Scale |
|---|---|---|---|---|---|
| cyborg | 195 | 5,779 | 6.0e-5 rad | 0 | 0 |
| warthog | 11 | 43 | 2.1e-5 rad | 0 | 0 |

Lockout's markers: all 922 sit in named groups at `+0x58`; the f32 at `+0x20`
is 0 on 842 and nonzero on 80.
