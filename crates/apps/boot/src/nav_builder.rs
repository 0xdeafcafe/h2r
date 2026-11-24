//! The nav generator Forge's **Generate nav** row runs
//! (docs/game/forge-nav.md): `forge_build::build` over the document's play
//! form, over the maps of this machine's disc, then `forge_nav` over the
//! built map with the placements the document adds as solids. `game` may
//! not use `tools`, so the composition root supplies this, through
//! `forge_glue::nav::install`.
//!
//! The generation runs on the worker the editor gives it
//! (`forge_glue::nav`), never on the game thread. It reports; it does not
//! load the mesh into a running game.
use std::collections::BTreeMap;
use std::path::PathBuf;

use forge::{NavBuilder, NavLine, NavProblem, NavReport};
use forge_doc::{BlockKind, ForgeDoc};
use scenario_edit::MapSource;

/// The maps the build reads, by name.
pub type Maps = Box<dyn FnMut() -> Result<Vec<(String, MapSource)>, String> + Send>;

pub struct AppNav {
    maps: Maps,
}

impl AppNav {
    pub fn new(maps: Maps) -> Self {
        Self { maps }
    }

    /// The maps of the disc image at `disc`.
    pub fn disc(disc: PathBuf) -> Self {
        Self::new(Box::new(move || {
            MapSource::disc_maps(&disc).map_err(|e| format!("The disc's maps do not read ({e})."))
        }))
    }
}

impl NavBuilder for AppNav {
    fn generate(
        &mut self,
        play: &ForgeDoc,
        base_counts: &BTreeMap<BlockKind, u32>,
    ) -> Result<NavReport, String> {
        let maps = (self.maps)()?;
        let built = forge_build::build(play, |name| {
            maps.iter()
                .find(|(map, _)| map.eq_ignore_ascii_case(name))
                .map(|(_, source)| source.clone())
        })
        .map_err(|e| e.to_string())?;
        let input = forge_nav::read::read(built.map_bytes(), |block, index| {
            added(base_counts, block, index)
        })?;
        let generated = forge_nav::generate(&input)?;
        Ok(report_of(&generated))
    }
}

/// Whether entry `index` of `block` is one the document adds: at or past
/// the loaded map's own count. A block the map has no count for is all
/// the document's.
pub fn added(base_counts: &BTreeMap<BlockKind, u32>, block: BlockKind, index: usize) -> bool {
    base_counts
        .get(&block)
        .is_none_or(|&count| index >= count as usize)
}

/// The session's report of a generation.
pub fn report_of(g: &forge_nav::Generated) -> NavReport {
    let mut links = BTreeMap::new();
    for e in &g.data.nav.edges {
        *links
            .entry(forge_nav::link_name(e.kind).to_string())
            .or_insert(0) += 1;
    }
    NavReport {
        sectors: g.sectors(),
        links,
        out_of_map: g.out_of_map(),
        solids: g.solids,
        problems: g
            .problems
            .iter()
            .map(|p| NavProblem {
                kind: p.kind.name().into(),
                what: p.what.clone(),
                position: p.position,
                reason: p.reason.clone(),
            })
            .collect(),
        lines: g
            .overlay()
            .into_iter()
            .map(|l| NavLine {
                kind: l.kind.name().into(),
                a: l.a,
                b: l.b,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use content::Sha256;
    use forge_doc::{BuilderVersions, FORMAT, MapRef};

    fn document() -> ForgeDoc {
        ForgeDoc {
            format: FORMAT,
            builder: BuilderVersions::CURRENT,
            base: MapRef {
                map: "court".into(),
                sha256: Sha256::of(b"an invented court"),
            },
            sources: vec![],
            title: "Ramps".into(),
            author: "Builder".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![],
            expect: None,
        }
    }

    #[test]
    fn only_the_entries_the_document_adds_are_solids() {
        let counts = BTreeMap::from([(BlockKind::Scenery, 4), (BlockKind::Crates, 0)]);
        assert!(!added(&counts, BlockKind::Scenery, 3));
        assert!(added(&counts, BlockKind::Scenery, 4));
        assert!(added(&counts, BlockKind::Crates, 0));
        assert!(added(&counts, BlockKind::Machines, 0));
    }

    #[test]
    fn a_map_the_disc_lacks_or_cannot_read_fails_the_generation_with_the_reason() {
        let mut none = AppNav::new(Box::new(|| Ok(vec![])));
        let error = none.generate(&document(), &BTreeMap::new()).unwrap_err();
        assert!(error.contains("court.map is missing"), "{error}");

        let mut broken = AppNav::new(Box::new(|| Err("The disc is not there.".into())));
        assert_eq!(
            broken.generate(&document(), &BTreeMap::new()).unwrap_err(),
            "The disc is not there."
        );
    }
}
