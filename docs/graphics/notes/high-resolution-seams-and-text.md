# High resolution: seam and text evidence

Evidence for [high-resolution.md](../high-resolution.md) ("Subpixel snapping and cracks" and "Text"). Newest first. Both entries were carried over from the design page at f7f5604cd; rerun the named test to refresh them.

## 2026-11-27: glyph filter against bilinear and point

Commit f7f5604cd. Method: `crates/platform/d3d/tests/glyph_text.rs` on a synthetic glyph cache (a ring, a letter, a thick and a thin slanted stroke, a dot), drawn 1:1 on the 64x64 corner of an 80x80 target (clear of the target's edge, where scaled positions are clamped). Along the thick stroke's edge: the largest distance between where alpha crosses one half and the true edge, and the edge's width (rows between 0.1 and 0.9).

| Scale | Bilinear | Point | Glyph filter |
|---|---|---|---|
| 2x | 0.28 px, 3.0 rows | 0.88 px, 1.9 rows | 0.19 px, 1.6 rows |
| 3x | 0.49 px, 4.5 rows | 1.58 px, 2.8 rows | 0.28 px, 1.7 rows |
| 4x | 0.65 px, 6.0 rows | 2.22 px, 3.8 rows | 0.39 px, 1.6 rows |

The area drawn wrong against the true outline falls to between a quarter (4x) and a half (2x) of either filter's. At 1x and in the reference at every scale the image is byte-identical with the filter on and off.

## 2026-11-27: seam pixels by vertex fixup

Commit f7f5604cd. Method: `crates/platform/d3d/tests/seams_gpu.rs`: a 64x64 frame, a strip meeting a long edge at 47 T-junction vertices, and a seam whose two draws hold an end one ulp apart. Clear-colour pixels inside the mesh, as T-junctions + ulp seam.

| Fixup | 2x | 3x | 4x | 6x |
|---|---|---|---|---|
| guest grid | 8 + 0 | 14 + 22 | 29 + 0 | 56 + 0 |
| 1/16 host pixel | 3 + 0 | 6 + 0 | 5 + 0 | 8 + 0 |
| none (used) | 0 | 0 | 0 | 0 |

## 2026-11-27: BSP crack survey

Commit f7f5604cd. Method: `bsp_cracks` (a tool since removed from `viewer`) welded a BSP's drawn cluster triangles by their exact position bits and classified every edge one triangle uses. Reported symptom: a thin see-through line along a terrain seam on Burial Mounds at about 4K.

| Level | Result |
|---|---|
| Burial Mounds (9,666 triangles, 27 clusters) | no T-junction and no near-coincident unwelded edge at a 0.001 or 0.05 world-unit tolerance; 58 open edges, all the level's border; 1,964 shared edges between triangles of different draws (1,083 between clusters, 882 between parts with other materials), every one with bit-identical ends |
| Zanzibar, Ascension | no T-junctions, no unwelded seams |
