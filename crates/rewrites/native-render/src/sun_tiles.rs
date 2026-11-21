//! E3 lighting's sun shadows from objects: the tile plan
//! (docs/graphics/e3-lighting.md, "Sun shadows from objects").
//!
//! Each caster gets an orthographic depth tile fitted to its bounding
//! sphere along its own direction towards the light. The tile's edge is
//! the next power of two above 1.5 times the sphere's diameter in display
//! pixels, between [`MIN_EDGE`] and [`MAX_EDGE`], so a texel is never
//! larger than a pixel on the caster. Tiles are packed by size into one
//! square atlas ([`ATLAS_EDGE`]); when they do not fit, every tile of the
//! largest class halves and [`SunWork::atlas_halved`] counts it.
//!
//! The atlas persists between frames. A tile is drawn again only when its
//! caster's blended rows, its direction or its place in the atlas
//! changed, so a still caster costs nothing after its first frame
//! ([`SunWork::sun_tiles_redrawn`]).
//!
//! This module is pure. `sun_tiles_gpu` draws the plan.

use std::collections::HashMap;

/// The shared depth atlas's edge in texels.
pub const ATLAS_EDGE: u32 = 4096;
/// The smallest and largest tile edges.
pub const MIN_EDGE: u32 = 128;
pub const MAX_EDGE: u32 = 2048;

/// One object that casts a sun shadow this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunCaster {
    /// Stable across frames for the same object (its datum handle).
    pub id: u64,
    /// The bounding sphere, in world space.
    pub centre: [f32; 3],
    pub radius: f32,
    /// The sphere's diameter in display pixels ([`screen_diameter`]).
    pub screen_diameter: f32,
    /// Unit vector from the caster towards the light: the object lighting
    /// sample's direction (`0x0003dc90`, sample `+0x0c`).
    pub towards_light: [f32; 3],
    /// Retail's final strength: the sample's scalar times the object fade
    /// and the ranked entry's faded weight. In `[0, 1]`.
    pub strength: f32,
    /// [`rows_hash`] of the blended node rows the caster is drawn with.
    pub rows: u64,
}

/// What one frame's sun tiles did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SunWork {
    /// Tiles in the atlas this frame: one per admitted caster.
    pub sun_tiles: u32,
    /// Tiles cleared and drawn again this frame.
    pub sun_tiles_redrawn: u32,
    /// Times the largest tile class was halved to fit the atlas.
    pub atlas_halved: u32,
    /// Casters with no tile: they did not fit at [`MIN_EDGE`], or their
    /// sphere, direction or strength was not usable. They keep retail's
    /// object shadow.
    pub refused: u32,
    /// Caster draws into tiles.
    pub tile_draws: u32,
    /// Receiver draws comparing against a tile.
    pub receiver_draws: u32,
}

/// A caster's place in the atlas and its projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunTile {
    pub id: u64,
    pub edge: u32,
    /// `x, y, width, height` in atlas texels.
    pub rect: [u32; 4],
    /// World to tile space, as rows applied to `[x, y, z, 1]`: x and y in
    /// `[-1, 1]` across the sphere, z in `[0, 1]` from the sphere's
    /// light-facing side to its far side.
    pub rows: [[f32; 4]; 4],
    pub centre: [f32; 3],
    pub radius: f32,
    pub towards_light: [f32; 3],
    pub strength: f32,
    /// Cleared and drawn this frame.
    pub redraw: bool,
}

impl SunTile {
    /// World units per texel across the tile.
    pub fn texel(&self) -> f32 {
        2.0 * self.radius / self.edge as f32
    }

    /// Whether a sphere receives this tile: it meets the tile's box swept
    /// away from the light. The caster itself always does.
    pub fn meets(&self, centre: [f32; 3], radius: f32) -> bool {
        let p = [centre[0], centre[1], centre[2], 1.0];
        let x = dot4(self.rows[0], p) * self.radius;
        let y = dot4(self.rows[1], p) * self.radius;
        // Distance along the light's travel from the sphere's near plane.
        let along = dot4(self.rows[2], p) * 2.0 * self.radius;
        x.abs() <= self.radius + radius && y.abs() <= self.radius + radius && along >= -radius
    }
}

/// The frame's tiles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SunPlan {
    pub tiles: Vec<SunTile>,
    pub work: SunWork,
}

/// The tile edge for a sphere of `diameter` display pixels.
pub fn tile_edge(diameter: f32) -> u32 {
    let want = 1.5 * diameter.max(0.0);
    if !want.is_finite() || want >= MAX_EDGE as f32 {
        return MAX_EDGE;
    }
    let mut edge = MIN_EDGE;
    while (edge as f32) <= want && edge < MAX_EDGE {
        edge *= 2;
    }
    edge
}

/// A sphere's diameter in pixels of a view `height` pixels tall, under
/// clip `rows` (world to clip, applied to `[x, y, z, 1]`). Works for
/// orthographic and perspective rows.
pub fn screen_diameter(rows: &[[f32; 4]; 4], height: u32, centre: [f32; 3], radius: f32) -> f32 {
    let w = dot4(rows[3], [centre[0], centre[1], centre[2], 1.0]).max(1e-6);
    let sy = length([rows[1][0], rows[1][1], rows[1][2]]);
    2.0 * radius * sy / w * height as f32 * 0.5
}

/// A stable hash of a caster's blended node rows: equal rows give equal
/// hashes, so a still caster's tile is kept.
pub fn rows_hash(rows: &[[f32; 4]]) -> u64 {
    // FNV-1a over the bit patterns.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in rows.iter().flatten() {
        for b in v.to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// What a kept tile was drawn from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawn {
    rows: u64,
    towards_light: [u32; 3],
    rect: [u32; 4],
}

/// Plans each frame's tiles against the atlas's current contents. One
/// planner belongs to one atlas.
#[derive(Debug)]
pub struct SunTilePlanner {
    atlas_edge: u32,
    drawn: HashMap<u64, Drawn>,
}

impl SunTilePlanner {
    /// `atlas_edge` is a power of two no smaller than [`MIN_EDGE`].
    pub fn new(atlas_edge: u32) -> Self {
        debug_assert!(atlas_edge.is_power_of_two() && atlas_edge >= MIN_EDGE);
        Self {
            atlas_edge,
            drawn: HashMap::new(),
        }
    }

    pub fn atlas_edge(&self) -> u32 {
        self.atlas_edge
    }

    /// Forgets the atlas's contents: every tile is drawn on the next plan.
    pub fn invalidate(&mut self) {
        self.drawn.clear();
    }

    pub fn plan(&mut self, casters: &[SunCaster]) -> SunPlan {
        let mut work = SunWork::default();
        let mut admitted: Vec<(SunCaster, [f32; 3], u32)> = Vec::new();
        for c in casters {
            let Some(l) = normalize(c.towards_light) else {
                work.refused += 1;
                continue;
            };
            let usable = c.radius.is_finite()
                && c.radius > 0.0
                && c.centre.iter().all(|v| v.is_finite())
                && c.strength.is_finite()
                && c.strength > 0.0;
            if !usable {
                work.refused += 1;
                continue;
            }
            admitted.push((*c, l, tile_edge(c.screen_diameter)));
        }
        // Largest first, then by id, so the packing is the same every frame
        // for the same casters.
        admitted.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.id.cmp(&b.0.id)));
        let cells = (self.atlas_edge / MIN_EDGE).max(1);
        let capacity = u64::from(cells) * u64::from(cells);
        let area = |edges: &[(SunCaster, [f32; 3], u32)]| -> u64 {
            edges.iter().map(|e| u64::from(e.2 / MIN_EDGE).pow(2)).sum()
        };
        while area(&admitted) > capacity {
            let largest = admitted.first().map_or(MIN_EDGE, |e| e.2);
            if largest <= MIN_EDGE {
                // Every tile is at the smallest class: refuse the last ones.
                let keep = capacity as usize;
                work.refused += (admitted.len() - keep) as u32;
                admitted.truncate(keep);
                break;
            }
            for e in admitted.iter_mut().filter(|e| e.2 == largest) {
                e.2 = largest / 2;
            }
            work.atlas_halved += 1;
            admitted.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.id.cmp(&b.0.id)));
        }
        // Power-of-two squares placed largest first along a Morton curve of
        // MIN_EDGE cells are aligned and never overlap.
        let mut cursor: u64 = 0;
        let mut tiles = Vec::with_capacity(admitted.len());
        let mut drawn = HashMap::with_capacity(admitted.len());
        for (c, l, edge) in admitted {
            let span = u64::from(edge / MIN_EDGE).pow(2);
            let (cx, cy) = morton(cursor);
            cursor += span;
            let rect = [cx * MIN_EDGE, cy * MIN_EDGE, edge, edge];
            let now = Drawn {
                rows: c.rows,
                towards_light: l.map(f32::to_bits),
                rect,
            };
            let redraw = self.drawn.get(&c.id) != Some(&now);
            drawn.insert(c.id, now);
            work.sun_tiles += 1;
            work.sun_tiles_redrawn += u32::from(redraw);
            tiles.push(SunTile {
                id: c.id,
                edge,
                rect,
                rows: tile_rows(c.centre, c.radius, l),
                centre: c.centre,
                radius: c.radius,
                towards_light: l,
                strength: c.strength.min(1.0),
                redraw,
            });
        }
        self.drawn = drawn;
        SunPlan { tiles, work }
    }
}

/// World to tile space for a sphere seen along `l` (unit, towards the
/// light). Depth grows away from the light.
pub fn tile_rows(c: [f32; 3], r: f32, l: [f32; 3]) -> [[f32; 4]; 4] {
    let w = [-l[0], -l[1], -l[2]];
    let up = if w[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let u = normalize(cross(up, w)).unwrap_or([1.0, 0.0, 0.0]);
    let v = cross(w, u);
    let row = |a: [f32; 3], s: f32, off: f32| [a[0] * s, a[1] * s, a[2] * s, -dot3(a, c) * s + off];
    [
        row(u, 1.0 / r, 0.0),
        row(v, 1.0 / r, 0.0),
        row(w, 0.5 / r, 0.5),
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn morton(i: u64) -> (u32, u32) {
    let compact = |mut x: u64| {
        x &= 0x5555_5555_5555_5555;
        x = (x | (x >> 1)) & 0x3333_3333_3333_3333;
        x = (x | (x >> 2)) & 0x0f0f_0f0f_0f0f_0f0f;
        x = (x | (x >> 4)) & 0x00ff_00ff_00ff_00ff;
        x = (x | (x >> 8)) & 0x0000_ffff_0000_ffff;
        x = (x | (x >> 16)) & 0x0000_0000_ffff_ffff;
        x as u32
    };
    (compact(i), compact(i >> 1))
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dot4(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    dot3(a, a).sqrt()
}

fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let n = length(a);
    (n.is_finite() && n > 1e-6).then(|| [a[0] / n, a[1] / n, a[2] / n])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caster(id: u64, diameter: f32) -> SunCaster {
        SunCaster {
            id,
            centre: [id as f32 * 10.0, 0.0, 1.0],
            radius: 1.0,
            screen_diameter: diameter,
            towards_light: [1.0, 0.0, 1.0],
            strength: 1.0,
            rows: 7,
        }
    }

    #[test]
    fn the_edge_is_the_next_power_of_two_above_one_and_a_half_diameters() {
        assert_eq!(tile_edge(0.0), MIN_EDGE);
        assert_eq!(tile_edge(80.0), 128);
        // 1.5 * 86 = 129: above 128.
        assert_eq!(tile_edge(86.0), 256);
        assert_eq!(tile_edge(600.0), 1024);
        assert_eq!(tile_edge(5000.0), MAX_EDGE);
        assert_eq!(tile_edge(f32::INFINITY), MAX_EDGE);
    }

    #[test]
    fn the_sphere_spans_the_tile_and_depth_grows_away_from_the_light() {
        let l = normalize([1.0, 0.0, 1.0]).unwrap();
        let rows = tile_rows([0.0, 0.0, 1.0], 2.0, l);
        let at = |p: [f32; 3]| {
            let p = [p[0], p[1], p[2], 1.0];
            [dot4(rows[0], p), dot4(rows[1], p), dot4(rows[2], p)]
        };
        let c = at([0.0, 0.0, 1.0]);
        assert!(c[0].abs() < 1e-6 && c[1].abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6);
        let near = at([2.0 * l[0], 2.0 * l[1], 1.0 + 2.0 * l[2]]);
        assert!(near[2].abs() < 1e-5, "the light-facing side is depth 0");
        let far = at([-2.0 * l[0], -2.0 * l[1], 1.0 - 2.0 * l[2]]);
        assert!((far[2] - 1.0).abs() < 1e-5, "the far side is depth 1");
    }

    #[test]
    fn tiles_do_not_overlap_and_too_many_halve_the_largest_class() {
        let mut planner = SunTilePlanner::new(ATLAS_EDGE);
        // 32 casters at 2048 are eight atlases' worth.
        let casters: Vec<SunCaster> = (0..32).map(|i| caster(i, 4000.0)).collect();
        let planned = planner.plan(&casters);
        assert_eq!(planned.work.sun_tiles, 32);
        assert_eq!(planned.work.refused, 0);
        assert!(planned.work.atlas_halved >= 1);
        for (i, a) in planned.tiles.iter().enumerate() {
            assert!(a.rect[0] + a.rect[2] <= ATLAS_EDGE && a.rect[1] + a.rect[3] <= ATLAS_EDGE);
            for b in &planned.tiles[i + 1..] {
                let apart = a.rect[0] + a.rect[2] <= b.rect[0]
                    || b.rect[0] + b.rect[2] <= a.rect[0]
                    || a.rect[1] + a.rect[3] <= b.rect[1]
                    || b.rect[1] + b.rect[3] <= a.rect[1];
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    /// A still caster's tile is kept; a moved one, or one whose light
    /// turned, is drawn again.
    #[test]
    fn a_still_caster_is_not_redrawn_and_a_moved_one_is() {
        let mut planner = SunTilePlanner::new(ATLAS_EDGE);
        let still = caster(1, 50.0);
        let mut moving = caster(2, 50.0);
        let first = planner.plan(&[still, moving]);
        assert_eq!(first.work.sun_tiles_redrawn, 2);
        let second = planner.plan(&[still, moving]);
        assert_eq!(second.work.sun_tiles, 2);
        assert_eq!(second.work.sun_tiles_redrawn, 0);
        moving.rows = 8;
        let third = planner.plan(&[still, moving]);
        assert_eq!(third.work.sun_tiles_redrawn, 1);
        assert!(!third.tiles.iter().find(|t| t.id == 1).unwrap().redraw);
        assert!(third.tiles.iter().find(|t| t.id == 2).unwrap().redraw);
        moving.towards_light = [0.0, 1.0, 1.0];
        assert_eq!(planner.plan(&[still, moving]).work.sun_tiles_redrawn, 1);
        planner.invalidate();
        assert_eq!(planner.plan(&[still, moving]).work.sun_tiles_redrawn, 2);
    }

    #[test]
    fn unusable_casters_are_refused_and_counted() {
        let mut planner = SunTilePlanner::new(ATLAS_EDGE);
        let mut dark = caster(1, 50.0);
        dark.strength = 0.0;
        let mut flat = caster(2, 50.0);
        flat.towards_light = [0.0; 3];
        let mut nan = caster(3, 50.0);
        nan.radius = f32::NAN;
        let planned = planner.plan(&[dark, flat, nan, caster(4, 50.0)]);
        assert_eq!(planned.work.refused, 3);
        assert_eq!(planned.work.sun_tiles, 1);
    }

    #[test]
    fn receivers_are_the_spheres_in_the_swept_box() {
        let l = normalize([1.0, 0.0, 1.0]).unwrap();
        let tile = SunTile {
            id: 0,
            edge: 128,
            rect: [0, 0, 128, 128],
            rows: tile_rows([0.0, 0.0, 1.0], 1.0, l),
            centre: [0.0, 0.0, 1.0],
            radius: 1.0,
            towards_light: l,
            strength: 1.0,
            redraw: true,
        };
        assert!(tile.meets(tile.centre, tile.radius), "the caster itself");
        // Down-sun of the caster, on the ground.
        assert!(tile.meets([-3.0, 0.0, -1.0], 0.5));
        // Up-sun of the caster: between it and the light.
        assert!(!tile.meets([4.0, 0.0, 5.0], 0.5));
        // Off to the side.
        assert!(!tile.meets([0.0, 5.0, 1.0], 0.5));
    }

    #[test]
    fn screen_diameter_follows_the_projection() {
        let ortho = [
            [0.25, 0.0, 0.0, 0.0],
            [0.0, 0.25, 0.0, 0.0],
            [0.0, 0.0, -0.05, 0.5],
            [0.0, 0.0, 0.0, 1.0],
        ];
        // 8 pixels a unit on a 64-pixel view.
        assert!((screen_diameter(&ortho, 64, [0.0; 3], 1.0) - 16.0).abs() < 1e-4);
        assert_eq!(rows_hash(&ortho), rows_hash(&ortho));
        let mut moved = ortho;
        moved[0][3] = 1e-7;
        assert_ne!(rows_hash(&ortho), rows_hash(&moved));
    }
}
