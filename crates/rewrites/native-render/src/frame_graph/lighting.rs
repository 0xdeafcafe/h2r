//! Work issued by the lighting mode in a single graph draw. The guest's
//! own light and shadow commands are already counted in the graph totals.

use d3d::gfx_backend::TileWork;

use crate::light_choice::LightChoice;
use crate::sun_tiles::SunWork;

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
    /// Caster draws into the local lights' tiles.
    pub tiles: PassWork,
    /// Caster draws into the sun's tiles.
    pub sun_tile_draws: PassWork,
    /// Receivers of the sun's tiles.
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

    /// Sun tiles from objects: their caster draws are `e3.sun_tiles`,
    /// their receivers `e3.sun`, and a caster left without a tile keeps
    /// retail's object shadow and joins `tiles_refused`.
    pub fn add_sun(&mut self, work: SunWork) {
        self.sun_tile_draws.draws += work.tile_draws;
        self.sun.draws += work.receiver_draws;
        self.sun_tiles += work.sun_tiles;
        self.sun_tiles_redrawn += work.sun_tiles_redrawn;
        self.atlas_halved += work.atlas_halved;
        self.tiles_refused += work.refused;
    }

    /// The renderer's choice of lights (`crate::light_choice`): the
    /// lights it lights and the candidates its cap left unlit.
    pub fn add_choice(&mut self, choice: &LightChoice) {
        self.lights_lit += choice.lit();
        self.lights_over_cap += choice.over_cap();
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

    /// Sun casters are `e3.sun_tiles`, apart from the local lights'
    /// `e3.tiles`; their receivers are `e3.sun`.
    #[test]
    fn sun_work_fills_the_sun_row_and_its_counters() {
        let mut report = E3Report::default();
        report.add_sun(SunWork {
            sun_tiles: 3,
            sun_tiles_redrawn: 1,
            atlas_halved: 2,
            refused: 1,
            tile_draws: 1,
            receiver_draws: 7,
        });
        assert_eq!(report.sun_tile_draws.draws, 1);
        assert_eq!(report.tiles, PassWork::default());
        assert_eq!(report.sun.draws, 7);
        assert_eq!(
            (
                report.sun_tiles,
                report.sun_tiles_redrawn,
                report.atlas_halved
            ),
            (3, 1, 2)
        );
        assert_eq!(report.tiles_refused, 1);
        assert_eq!(report.marks, PassWork::default());
    }
}
