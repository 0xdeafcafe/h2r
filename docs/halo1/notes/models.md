# Halo 1 model and animation conversion: measurements

Evidence for [../models.md](../models.md).

## 2026-11-18: skinned round trip of the Halo 1 bipeds

Commits: `19e863ad4` (inverse frames stored as retail stores them, columns,
converter version 59) and `374d881bc` (rotations read as the conjugate of
Halo 2's, converter version 54). Method: `h1_disc_bipeds_skin` in
`crates/tools/h1-assets/src/models/skin_tests.rs`
(`H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo xtask test --disc -p h1-assets`).
Each model and its graph are converted, built into a map, decoded, and every
vertex is skinned the Halo 2 way and the Halo 1 way, at the bind pose and at
the middle frame of about seven base animations per graph. Errors are the
largest vertex distance in world units (a biped is about 0.7 tall).

| Model | Vertices | Our inverse frames vs Halo 1's | Bind pose | Posed, version 53 | Posed, version 54 |
|---|---|---|---|---|---|
| cyborg | 6,025 | 4.2e-7 (was 1.2) | 5.9e-6 | 0.64 to 1.16 | 1.4e-4 to 2.8e-4 |
| marine (armored) | 13,666 | 4.8e-7 (was 1.7) | 7.0e-6 | 0.53 to 0.85 | 7.3e-5 to 1.8e-4 |
| crewman | 1,578 | 4.8e-7 | 6.9e-6 | 0.53 to 0.85 | 7.4e-5 to 1.8e-4 |
| elite | 10,818 | 7.2e-7 (was 1.9) | 7.1e-6 | 0.54 to 1.00 | 1.4e-4 to 3.5e-4 |
| grunt | 8,065 | 3.5e-7 (was 1.9) | 6.8e-6 | 0.41 to 1.15 | 9.0e-5 to 2.8e-4 |
| captain, pilot, Cortana | 1,036 to 1,747 | under 7e-7 | under 7e-6 | 0.15 to 0.78 | under 2e-4 |
| cyborg first-person arms | 1,440 | 8.9e-8 | 2.9e-6 | (no graph of its own) | |

"Was" is the error with the inverse frames written as rows. Version 53
copied the rotations unconverted; from version 54 they are read as the
conjugate. The vertices travel up to 0.93 in those frames.

The retail control (`retail_h2_bipeds_pose_sanely`, 05a): a retail biped's
rest frames times its stored inverse frames, read as columns, are the
identity within 5e-7 on every node; read as rows, off by up to 2.0.

First-person models (`h1_disc_first_person_skin`): Halo 1's arms and five
weapons' first-person models on Blood Gulch, graph bound by name, posed at
the middle frame of base animations. Largest vertex error per weapon over 5
to 8 animations: assault rifle 1.2e-4, pistol 1.5e-4, plasma rifle 5.4e-5,
needler 6.0e-5, rocket launcher 6.3e-5.

## 2026-11-16: geometry and animation round trip on beavercreek

Commit: `5c220c8d6`. Method: the ignored tests in
`crates/tools/h1-assets/src/models/tests.rs`
(`H1_MAPS=<dir with beavercreek.map, carousel.map>`) convert each model,
build a v8 map in memory, decode through `tags::model::RenderModelGeometry`
and `tags::anim::AnimationGraph`, and compare with the Halo 1 source. Every
vertex and triangle of every geometry, all LODs:

| Model | Nodes | Markers | Geometries | Vertices | Triangles | Max position error | Texcoord | Normal | Weight |
|---|---|---|---|---|---|---|---|---|---|
| cyborg | 19 | 8 | 5 | 6,025 | 5,692 | 5.4e-6 | 7.6e-6 | 0 | 2.0e-3 (u8) |
| assault rifle | 3 | 9 | 3 | 1,075 | 940 | 2.2e-6 | 7.6e-6 | 0 | 0 |
| warthog | 18 | 26 | 20 | 10,396 | 7,597 | 1.5e-5 | 1.5e-5 | 0 | 2.0e-3 (u8) |

Node names, hierarchy, default transforms, region and permutation names,
LOD sections, marker names and instances (exact), and per-triangle indices
all match.

Animations, every frame of every animation, each channel the conversion
carries:

| Graph | Animations | Frames | Max rotation error | Translation | Scale |
|---|---|---|---|---|---|
| cyborg | 195 | 5,779 | 6.0e-5 rad | 0 | 0 |
| warthog | 11 | 43 | 2.1e-5 rad | 0 | 0 |

The rotation error is the i16 re-quantisation of normalised quaternions.
