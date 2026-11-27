//! Light tiles (docs/graphics/native-light-tiles.md): a run of
//! consecutive lights' layers 12 and 13 drawn as one pass over screen
//! tiles, in place of one sequence per light.
//!
//! This half needs no GPU: each light's [`TileVolume`], the run planner
//! ([`plan`]), the counters and the CPU reference of the binning pass
//! ([`bin_cpu`]). The admission of a light's passes, the binning pass and
//! the tiled draws are `crate::light_tiles_gpu`.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use frame_scene::ViewSlot;

/// A tile's side in the target's texels.
pub const TILE: u32 = 16;
/// Lights in one run: a bit each in a tile's mask.
pub const MAX_RUN: usize = 32;

/// Why a light keeps its per-light pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileRefusal {
    /// Its steps draw layer 10 or 11, or a lit pass samples a shadow map.
    Shadow,
    /// Its steps draw layer 14.
    Opaque,
    /// Its gel is drawn into target 13 per light.
    GelTarget,
    /// A pass's blend is not an add of the source to the destination.
    Blend,
    /// A pass writes depth or compares other than less or equal.
    Depth,
    /// Its geometry's volume is not known.
    Bounds,
    /// A pass's module lacks the shape the instancing rewrite needs.
    Program,
}

impl TileRefusal {
    pub const ALL: [TileRefusal; 7] = [
        TileRefusal::Shadow,
        TileRefusal::Opaque,
        TileRefusal::GelTarget,
        TileRefusal::Blend,
        TileRefusal::Depth,
        TileRefusal::Bounds,
        TileRefusal::Program,
    ];

    /// `scene_light_tiles_refused_<cause>`, in [`Self::ALL`]'s order.
    pub const COUNTERS: [&'static str; 7] = [
        "scene_light_tiles_refused_shadow",
        "scene_light_tiles_refused_opaque",
        "scene_light_tiles_refused_gel_target",
        "scene_light_tiles_refused_blend",
        "scene_light_tiles_refused_depth",
        "scene_light_tiles_refused_bounds",
        "scene_light_tiles_refused_program",
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }

    pub fn name(self) -> &'static str {
        &self.counter()["scene_light_tiles_refused_".len()..]
    }

    pub fn counter(self) -> &'static str {
        Self::COUNTERS[self.index()]
    }
}

/// What a light's geometry covers: its pixels' rectangle `[x0, y0, x1,
/// y1]` in the target's texels and its nearest stored depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileVolume {
    pub rect: [f32; 4],
    pub near: f32,
}

impl TileVolume {
    /// The volume of `corners`, each `(x, y, w)`: window coordinates in
    /// guest pixels and the clip `w` the program writes, the stored depth
    /// being `w / z_scale` clamped to `[0, 1]` (`shaders::psh`
    /// `xps_w_depth`, no polygon offset). Scaled to `size` texels of a
    /// `guest` target. A corner at or behind the eye covers the whole
    /// target from depth 0. `None` for a non-finite corner.
    pub fn from_window(
        corners: &[[f32; 3]],
        guest: (u32, u32),
        size: (u32, u32),
        z_scale: f32,
    ) -> Option<Self> {
        if corners.is_empty() || corners.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        let whole = [0.0, 0.0, size.0 as f32, size.1 as f32];
        if corners.iter().any(|c| c[2] <= 0.0) {
            return Some(Self {
                rect: whole,
                near: 0.0,
            });
        }
        let sx = size.0 as f32 / guest.0.max(1) as f32;
        let sy = size.1 as f32 / guest.1.max(1) as f32;
        let mut rect = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut w = f32::MAX;
        for c in corners {
            rect[0] = rect[0].min(c[0] * sx);
            rect[1] = rect[1].min(c[1] * sy);
            rect[2] = rect[2].max(c[0] * sx);
            rect[3] = rect[3].max(c[1] * sy);
            w = w.min(c[2]);
        }
        // A texel's slack each side for the program's own rounding.
        let rect = [
            (rect[0] - 1.0).max(0.0),
            (rect[1] - 1.0).max(0.0),
            (rect[2] + 1.0).min(whole[2]),
            (rect[3] + 1.0).min(whole[3]),
        ];
        let near = (w.floor() / z_scale).clamp(0.0, 1.0);
        Some(Self { rect, near })
    }

    /// The volume of the world-space box `bounds` (`min`, `max`) as `view`'s
    /// world programs draw it (`ViewInputs::from_camera`): each corner's
    /// clip `x`, `y` and `w` from the view-projection rows `c[0]`, `c[1]`
    /// and `c[3]`, then over `w` through the viewport scale `c[10]` and
    /// offset `c[11]` to guest pixels ([`Self::from_window`]). The view's
    /// constants are the copy's (`ExternalPassRequest::view_constants`),
    /// so a widened display copy projects as it draws. `None` for an empty
    /// or non-finite box.
    pub fn from_bounds(
        view: &crate::program::ViewInputs,
        bounds: ([f32; 3], [f32; 3]),
    ) -> Option<Self> {
        let (min, max) = bounds;
        if (0..3).any(|k| min[k] > max[k]) {
            return None;
        }
        let c = &view.constants;
        let dot = |r: [f32; 4], p: [f32; 3]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
        let corners: Vec<[f32; 3]> = (0..8)
            .map(|n| {
                let p = std::array::from_fn(|k| if (n >> k) & 1 == 0 { min[k] } else { max[k] });
                let w = dot(c[3], p);
                if w <= 0.0 {
                    return [0.0, 0.0, w];
                }
                [
                    dot(c[0], p) / w * c[10][0] + c[11][0],
                    dot(c[1], p) / w * c[10][1] + c[11][1],
                    w,
                ]
            })
            .collect();
        Self::from_window(&corners, view.guest_size, view.size, view.z_scale)
    }

    /// Whether the volume touches tile `(tx, ty)` of a `size` target whose
    /// farthest stored depth there is `far`.
    pub fn touches(&self, tx: u32, ty: u32, size: (u32, u32), far: f32) -> bool {
        let lo = [(tx * TILE) as f32, (ty * TILE) as f32];
        let hi = [
            ((tx + 1) * TILE).min(size.0) as f32,
            ((ty + 1) * TILE).min(size.1) as f32,
        ];
        self.rect[0] < hi[0]
            && self.rect[2] > lo[0]
            && self.rect[1] < hi[1]
            && self.rect[3] > lo[1]
            && self.near <= far
    }
}

/// Tiles across and down a `size` target.
pub fn tiles(size: (u32, u32)) -> (u32, u32) {
    (size.0.div_ceil(TILE), size.1.div_ceil(TILE))
}

/// The binning pass on the CPU: per tile, row by row, a bit per volume
/// that touches it, from `depth` (`size.0 * size.1` stored depths).
pub fn bin_cpu(depth: &[f32], size: (u32, u32), volumes: &[TileVolume]) -> Vec<u32> {
    let (tx, ty) = tiles(size);
    let mut masks = Vec::with_capacity((tx * ty) as usize);
    for y in 0..ty {
        for x in 0..tx {
            let mut far = 0.0f32;
            for py in y * TILE..((y + 1) * TILE).min(size.1) {
                for px in x * TILE..((x + 1) * TILE).min(size.0) {
                    far = far.max(depth[(py * size.0 + px) as usize]);
                }
            }
            let mask = volumes
                .iter()
                .take(MAX_RUN)
                .enumerate()
                .filter(|(_, v)| v.touches(x, y, size, far))
                .fold(0u32, |m, (i, _)| m | 1 << i);
            masks.push(mask);
        }
    }
    masks
}

/// A piece of a view's lights, in the game's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Consecutive admitted lights of one slot, two to [`MAX_RUN`], whose
    /// layers 12 and 13 draw tiled.
    Tiled(Range<usize>),
    /// A light on its own per-light pass: refused, or a run of one.
    PerLight {
        light: usize,
        refused: Option<TileRefusal>,
    },
}

/// The segments of `lights` (each its view slot and its admission), in
/// order. A run breaks at a refused light, a change of slot and
/// [`MAX_RUN`].
pub fn plan(lights: &[(ViewSlot, Result<(), TileRefusal>)]) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut run: Option<(ViewSlot, usize)> = None;
    let close = |out: &mut Vec<Segment>, start: usize, end: usize| match end - start {
        0 => {}
        1 => out.push(Segment::PerLight {
            light: start,
            refused: None,
        }),
        _ => out.push(Segment::Tiled(start..end)),
    };
    for (i, (slot, admitted)) in lights.iter().enumerate() {
        match admitted {
            Err(cause) => {
                if let Some((_, start)) = run.take() {
                    close(&mut out, start, i);
                }
                out.push(Segment::PerLight {
                    light: i,
                    refused: Some(*cause),
                });
            }
            Ok(()) => match run {
                Some((s, start)) if s == *slot && i - start < MAX_RUN => {}
                Some((_, start)) => {
                    close(&mut out, start, i);
                    run = Some((*slot, i));
                }
                None => run = Some((*slot, i)),
            },
        }
    }
    if let Some((_, start)) = run {
        close(&mut out, start, lights.len());
    }
    out
}

static REFUSED: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];
static RUNS: AtomicU64 = AtomicU64::new(0);
static LIGHTS: AtomicU64 = AtomicU64::new(0);
static SINGLE: AtomicU64 = AtomicU64::new(0);

/// Counts what `segments` draw: each refusal by cause, each run and its
/// lights, each run of one.
pub fn count(segments: &[Segment]) {
    for s in segments {
        match s {
            Segment::Tiled(r) => {
                RUNS.fetch_add(1, Ordering::Relaxed);
                LIGHTS.fetch_add(r.len() as u64, Ordering::Relaxed);
            }
            Segment::PerLight {
                refused: Some(c), ..
            } => {
                REFUSED[c.index()].fetch_add(1, Ordering::Relaxed);
            }
            Segment::PerLight { refused: None, .. } => {
                SINGLE.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// The counters, named as `render.scene` lists them.
pub fn counters() -> Vec<(&'static str, u64)> {
    let mut out = vec![
        ("scene_light_tiles_runs", RUNS.load(Ordering::Relaxed)),
        ("scene_light_tiles_lights", LIGHTS.load(Ordering::Relaxed)),
        ("scene_light_tiles_single", SINGLE.load(Ordering::Relaxed)),
        (
            "scene_light_tiles_fallback",
            FALLBACK.load(Ordering::Relaxed),
        ),
        ("scene_light_tiles_lost", LOST.load(Ordering::Relaxed)),
    ];
    out.extend(ARM.counters());
    out.extend(
        TileRefusal::COUNTERS
            .iter()
            .zip(&REFUSED)
            .map(|(n, c)| (*n, c.load(Ordering::Relaxed))),
    );
    out
}

/// Matched compares before the arm draws a run uncompared.
pub const PROVEN: u64 = 4;
/// Once proven, one run in this many is still compared.
pub const COMPARE_EVERY: u64 = 64;

/// What a run of held lights draws (docs/graphics/native-light-tiles.md,
/// "The compare arm").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// A run compared since the map loaded differed: every light draws on
    /// its per-light pass.
    Off,
    /// The per-light passes draw the run, and the tiled pass draws it again
    /// into a copy of the target that is compared with theirs.
    Compare,
    /// The tiled pass alone.
    Tiled,
}

/// The tiled path's compare arm: every run compared until [`PROVEN`]
/// compares match, then one in [`COMPARE_EVERY`]; one difference turns
/// it off until the map is retired.
#[derive(Debug, Default)]
pub struct Arm {
    runs: AtomicU64,
    compared: AtomicU64,
    matched: AtomicU64,
    mismatched: AtomicU64,
    off: std::sync::atomic::AtomicBool,
}

impl Arm {
    pub const fn new() -> Self {
        Self {
            runs: AtomicU64::new(0),
            compared: AtomicU64::new(0),
            matched: AtomicU64::new(0),
            mismatched: AtomicU64::new(0),
            off: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Whether the arm holds lights at all.
    pub fn on(&self) -> bool {
        !self.off.load(Ordering::Relaxed)
    }

    /// What the next run draws.
    pub fn decide(&self) -> Decision {
        if !self.on() {
            return Decision::Off;
        }
        let n = self.runs.fetch_add(1, Ordering::Relaxed);
        if self.matched.load(Ordering::Relaxed) < PROVEN || n % COMPARE_EVERY == 0 {
            self.compared.fetch_add(1, Ordering::Relaxed);
            Decision::Compare
        } else {
            Decision::Tiled
        }
    }

    /// A compared run's result: a difference turns the arm off.
    pub fn report(&self, matched: bool) {
        if matched {
            self.matched.fetch_add(1, Ordering::Relaxed);
        } else {
            self.mismatched.fetch_add(1, Ordering::Relaxed);
            self.off.store(true, Ordering::Relaxed);
        }
    }

    /// A new map: unproven and on again.
    pub fn reset(&self) {
        self.runs.store(0, Ordering::Relaxed);
        self.matched.store(0, Ordering::Relaxed);
        self.off.store(false, Ordering::Relaxed);
    }

    pub fn counters(&self) -> [(&'static str, u64); 4] {
        [
            (
                "scene_light_tiles_compared",
                self.compared.load(Ordering::Relaxed),
            ),
            (
                "scene_light_tiles_matched",
                self.matched.load(Ordering::Relaxed),
            ),
            (
                "scene_light_tiles_mismatched",
                self.mismatched.load(Ordering::Relaxed),
            ),
            ("scene_light_tiles_arm_off", u64::from(!self.on())),
        ]
    }
}

/// The game's arm.
pub static ARM: Arm = Arm::new();

static FALLBACK: AtomicU64 = AtomicU64::new(0);
static LOST: AtomicU64 = AtomicU64::new(0);

/// A held run whose tiled sequence did not prepare, drawn per light.
pub fn fell_back() {
    FALLBACK.fetch_add(1, Ordering::Relaxed);
}

/// A held light whose per-light fallback also did not prepare: undrawn.
pub fn lost() {
    LOST.fetch_add(1, Ordering::Relaxed);
}

/// The largest per-channel difference two compared targets may show for
/// a run of `lights`: a unorm step per added light, and a little.
pub fn tolerance(lights: usize) -> f32 {
    (lights as f32 + 1.0) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter(name: &str) -> u64 {
        counters()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map_or(0, |(_, v)| v)
    }

    #[test]
    fn a_refused_light_draws_on_its_per_light_pass_and_is_counted() {
        let (world, fp) = (ViewSlot::LightAccumulation, ViewSlot::FirstPerson);
        let lights = [
            (world, Ok(())),
            (world, Ok(())),
            (world, Err(TileRefusal::Shadow)),
            (world, Ok(())),
            (world, Ok(())),
            (world, Ok(())),
            (fp, Ok(())),
            (fp, Err(TileRefusal::Opaque)),
            (fp, Ok(())),
            (fp, Ok(())),
        ];
        let segments = plan(&lights);
        assert_eq!(
            segments,
            [
                Segment::Tiled(0..2),
                Segment::PerLight {
                    light: 2,
                    refused: Some(TileRefusal::Shadow)
                },
                Segment::Tiled(3..6),
                Segment::PerLight {
                    light: 6,
                    refused: None
                },
                Segment::PerLight {
                    light: 7,
                    refused: Some(TileRefusal::Opaque)
                },
                Segment::Tiled(8..10),
            ],
            "a refused light keeps its place; runs never cross a slot"
        );
        let drawn: Vec<usize> = segments
            .iter()
            .flat_map(|s| match s {
                Segment::Tiled(r) => r.clone().collect::<Vec<_>>(),
                Segment::PerLight { light, .. } => vec![*light],
            })
            .collect();
        assert_eq!(
            drawn,
            (0..lights.len()).collect::<Vec<_>>(),
            "every light drawn once, in order"
        );

        let before: Vec<u64> = TileRefusal::COUNTERS.iter().map(|n| counter(n)).collect();
        let (runs, tiled) = (
            counter("scene_light_tiles_runs"),
            counter("scene_light_tiles_lights"),
        );
        count(&segments);
        let after: Vec<u64> = TileRefusal::COUNTERS.iter().map(|n| counter(n)).collect();
        assert!(
            after[0] > before[0] && after[1] > before[1],
            "shadow and opaque counted"
        );
        assert_eq!(
            TileRefusal::Shadow.counter(),
            "scene_light_tiles_refused_shadow"
        );
        assert_eq!(TileRefusal::GelTarget.name(), "gel_target");
        assert!(counter("scene_light_tiles_runs") >= runs + 3);
        assert!(counter("scene_light_tiles_lights") >= tiled + 7);
    }

    #[test]
    fn the_arm_compares_every_run_until_proven_then_one_in_64_and_a_difference_turns_it_off() {
        let arm = Arm::new();
        let first: Vec<Decision> = (0..PROVEN).map(|_| arm.decide()).collect();
        assert!(
            first.iter().all(|d| *d == Decision::Compare),
            "unproven: all compared"
        );
        for _ in 0..PROVEN {
            arm.report(true);
        }
        let later: Vec<Decision> = (0..2 * COMPARE_EVERY).map(|_| arm.decide()).collect();
        let compared = later.iter().filter(|d| **d == Decision::Compare).count();
        assert_eq!(compared, 2, "one in 64 once proven");
        arm.report(false);
        assert_eq!(arm.decide(), Decision::Off);
        assert!(!arm.on());
        arm.reset();
        assert_eq!(arm.decide(), Decision::Compare, "a new map compares again");
        assert_eq!(arm.counters()[2], ("scene_light_tiles_mismatched", 1));
    }

    #[test]
    fn a_run_stops_at_thirty_two_lights() {
        let lights = vec![(ViewSlot::LightAccumulation, Ok(())); MAX_RUN + 3];
        assert_eq!(
            plan(&lights),
            [
                Segment::Tiled(0..MAX_RUN),
                Segment::Tiled(MAX_RUN..MAX_RUN + 3)
            ]
        );
    }

    #[test]
    fn a_boxs_volume_is_its_corners_projected_through_the_views_rows() {
        // Clip x = x, y = y, w = z; the viewport maps x/w, y/w in [-1, 1]
        // to a 64 square, y down. The box spans x, y in [-1, 1] at
        // z 10..20: its near face projects to 32 +/- 3.2.
        let mut constants = [[0.0; 4]; crate::program::VIEW_CONSTANTS];
        constants[0] = [1.0, 0.0, 0.0, 0.0];
        constants[1] = [0.0, 1.0, 0.0, 0.0];
        constants[3] = [0.0, 0.0, 1.0, 0.0];
        constants[10] = [32.0, -32.0, 0.0, 0.0];
        constants[11] = [32.0, 32.0, 0.0, 0.0];
        let view = crate::program::ViewInputs {
            constants,
            guest_size: (64, 64),
            size: (128, 128),
            z_scale: 100.0,
            w_clip: None,
        };
        let v = TileVolume::from_bounds(&view, ([-1.0, -1.0, 10.0], [1.0, 1.0, 20.0])).unwrap();
        let want = [
            2.0 * 28.8 - 1.0,
            2.0 * 28.8 - 1.0,
            2.0 * 35.2 + 1.0,
            2.0 * 35.2 + 1.0,
        ];
        for k in 0..4 {
            assert!(
                (v.rect[k] - want[k]).abs() < 1e-3,
                "{:?} at the copy's scale",
                v.rect
            );
        }
        assert_eq!(v.near, 0.1, "the nearest corner's w over the W scale");
        assert!(
            !v.touches(0, 0, (128, 128), 1.0),
            "the corner tile is culled"
        );
        assert!(v.touches(3, 3, (128, 128), 1.0));
        let across =
            TileVolume::from_bounds(&view, ([-1.0, -1.0, -5.0], [1.0, 1.0, 20.0])).unwrap();
        assert_eq!(
            (across.rect, across.near),
            ([0.0, 0.0, 128.0, 128.0], 0.0),
            "a box across the eye"
        );
        assert!(
            TileVolume::from_bounds(&view, ([1.0; 3], [0.0; 3])).is_none(),
            "an empty box"
        );
    }

    #[test]
    fn a_volume_is_its_corners_rectangle_and_nearest_depth() {
        let v = TileVolume::from_window(
            &[[4.0, 2.0, 30.0], [20.0, 9.0, 50.0]],
            (32, 32),
            (64, 64),
            100.0,
        )
        .unwrap();
        assert_eq!(
            v.rect,
            [7.0, 3.0, 41.0, 19.0],
            "scaled to the copy, a texel's slack"
        );
        assert_eq!(v.near, 0.3);
        let behind =
            TileVolume::from_window(&[[4.0, 2.0, -1.0]], (32, 32), (32, 32), 100.0).unwrap();
        assert_eq!((behind.rect, behind.near), ([0.0, 0.0, 32.0, 32.0], 0.0));
        assert!(TileVolume::from_window(&[[f32::NAN, 0.0, 1.0]], (1, 1), (1, 1), 1.0).is_none());
        assert!(v.touches(0, 0, (64, 64), 0.3) && !v.touches(0, 0, (64, 64), 0.29));
        assert!(!v.touches(3, 0, (64, 64), 1.0), "x from 48 is past 41");
    }
}
