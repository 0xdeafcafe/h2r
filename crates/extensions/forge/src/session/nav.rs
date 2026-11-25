//! **Generate nav** in a session (docs/game/forge-nav.md).
//!
//! The bots' nav mesh is made from the structure BSP; a Forge map's solid
//! objects are not in it. **Generate nav** asks the game's
//! [`NavBuilder`] to build the document's play form and make the mesh
//! with the objects the document adds merged in, and keeps what it
//! reports: the counts, the problems (a start off the mesh, a teleporter
//! end off it, a solid left out) and the overlay's lines. Nothing is
//! generated while placing; an edit after a generation makes it stale.
//!
//! The report is kept beside the document, not in it, and a load starts
//! with none.
use std::collections::BTreeMap;

use forge_doc::{BlockKind, ForgeDoc, Sha256};

use super::{Session, reason};

/// Where the nav stands against the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavState {
    /// No generation in this session.
    NotBuilt,
    /// The last generation covers what the document is now.
    Current,
    /// The document changed since the last generation.
    Stale,
}

impl NavState {
    /// The state as `forge.nav` spells it.
    pub fn name(self) -> &'static str {
        match self {
            NavState::NotBuilt => "not_built",
            NavState::Current => "current",
            NavState::Stale => "stale",
        }
    }
}

/// Something the generated nav cannot do, and where: the overlay draws a
/// marker there.
#[derive(Clone, Debug, PartialEq)]
pub struct NavProblem {
    /// `start_off_mesh`, `teleporter_off_mesh`, `solid_left_out`.
    pub kind: String,
    /// What it is about: `start 3`, `crates 4 bsp 0`.
    pub what: String,
    pub position: [f32; 3],
    pub reason: String,
}

/// One line of the nav overlay, in world space.
#[derive(Clone, Debug, PartialEq)]
pub struct NavLine {
    /// `sector`, `out_of_map`, or a link's kind: `jump`, `teleporter`.
    pub kind: String,
    pub a: [f32; 3],
    pub b: [f32; 3],
}

/// What a generation made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavReport {
    /// The mesh's polygons.
    pub sectors: usize,
    /// How many links of each kind, by `forge.nav`'s name.
    pub links: BTreeMap<String, usize>,
    /// Polygons no player start reaches.
    pub out_of_map: usize,
    /// Solid objects merged into the level's collision.
    pub solids: usize,
    pub problems: Vec<NavProblem>,
    pub lines: Vec<NavLine>,
}

/// The generator, supplied by the game: build the play form, merge the
/// document's solid objects into the level and make the bots' mesh.
pub trait NavBuilder {
    /// Generate over `play`, the document's play form. `base_counts` are
    /// the loaded map's own entries per block: an index at or past its
    /// block's count is one the document adds. An `Err` is a sentence for
    /// the HUD.
    fn generate(
        &mut self,
        play: &ForgeDoc,
        base_counts: &BTreeMap<BlockKind, u32>,
    ) -> Result<NavReport, String>;
}

/// The last generation, kept beside the document.
#[derive(Clone, Debug, Default)]
pub(super) struct Track {
    built: Option<(Sha256, NavReport)>,
}

impl Session {
    fn nav_stamp(&self) -> Result<Sha256, String> {
        self.doc.id().map_err(reason)
    }

    pub fn nav_state(&self) -> NavState {
        match (&self.nav.built, self.nav_stamp()) {
            (None, _) => NavState::NotBuilt,
            (Some((stamp, _)), Ok(now)) if *stamp == now => NavState::Current,
            _ => NavState::Stale,
        }
    }

    /// What the last generation made.
    pub fn nav_report(&self) -> Option<&NavReport> {
        self.nav.built.as_ref().map(|(_, report)| report)
    }

    /// **Generate nav**: generate now, current or not. A failed generation
    /// leaves the last one and so the state as they were.
    pub fn generate_nav(
        &mut self,
        builder: &mut (impl NavBuilder + ?Sized),
    ) -> Result<&NavReport, String> {
        let stamp = self.nav_stamp()?;
        let play = forge_doc::play_form(&self.doc);
        let report = builder.generate(&play, &self.limits.base_counts)?;
        let (_, report) = self.nav.built.insert((stamp, report));
        Ok(report)
    }

    pub(super) fn restart_nav(&mut self) {
        self.nav = Track::default();
    }
}
