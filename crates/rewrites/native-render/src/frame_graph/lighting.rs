//! Work issued by the lighting mode in a single graph draw. The guest's
//! own light and shadow commands are already counted in the graph totals.

use d3d::gfx_backend::TileWork;

/// Submitted work for one E3 stage. An empty stage has no GPU interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassWork {
    pub draws: u32,
    pub gpu_ns: Option<u64>,
}

/// Per-draw accounting for E3's own lighting passes. Selection and atlas
/// counters describe decisions of this renderer, independently of the
/// guest's selections retained in the sealed scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct E3Report {
    pub tiles: PassWork,
    pub sun: PassWork,
    pub marks: PassWork,
    pub lights: PassWork,
    pub lights_lit: u32,
    pub lights_shadowed: u32,
    pub lights_over_cap: u32,
    pub sun_tiles: u32,
    pub sun_tiles_redrawn: u32,
    pub atlas_halved: u32,
    /// Shadow maps the mode left at the guest's size with retail's
    /// comparison (`TileWork::refused`).
    pub tiles_refused: u32,
}

impl E3Report {
    /// Local-light tiles: their casters are `e3.tiles`, their Sharp
    /// receivers `e3.marks`, and each tile is one shadowed light.
    pub(super) fn add_tiles(&mut self, work: TileWork) {
        self.tiles.draws += work.tile_draws;
        self.marks.draws += work.marks;
        self.lights_shadowed += work.tiles;
        self.tiles_refused += work.refused;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Casters into tiles are `e3.tiles`, Sharp receivers `e3.marks`,
    /// and every tile is one shadowed light.
    #[test]
    fn tile_work_fills_the_tile_and_mark_rows() {
        let mut report = E3Report::default();
        report.add_tiles(TileWork {
            tiles: 2,
            tile_draws: 9,
            marks: 5,
            refused: 1,
        });
        assert_eq!(
            report,
            E3Report {
                tiles: PassWork {
                    draws: 9,
                    gpu_ns: None
                },
                marks: PassWork {
                    draws: 5,
                    gpu_ns: None
                },
                lights_shadowed: 2,
                tiles_refused: 1,
                ..E3Report::default()
            }
        );
    }
}
