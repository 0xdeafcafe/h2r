//! Halo 1 campaign levels as Halo 2 v8 maps (docs/game/campaigns.md, step
//! 3 onward).
//!
//! A level is converted onto a **pinned retail Halo 2 campaign map** (the
//! base, [`BASE`]): its globals, HUD, shared tags and the Halo 2 characters
//! the AI stands in with stay the base's. On top of it:
//!
//! - [`structure`]: each Halo 1 structure BSP (collision, MOPP, render
//!   geometry, Halo 1's lightmap, generated AI pathfinding) in the base's
//!   BSP slots, through `h1convert`'s writer;
//! - [`scenario`]: the base's scenario rebuilt as a built root over its own
//!   pinned children, with Halo 1's sky, player starts, object placements,
//!   AI and scripts written over the base's;
//! - [`manifest`]: the level's place in the Halo 1 campaign
//!   (`content::campaign_list`).
//!
//! No game data is in this crate or its tests. The `#[ignore]` tests read
//! the owner's discs at run time.
#![forbid(unsafe_code)]

pub mod manifest;
pub mod scenario;
pub mod structure;

use std::io::{Read, Seek};

use h1map::H1Map;
use map::Map;
use std::collections::HashMap;

use map::build::{TagPayload, TagSet};

/// The retail Halo 2 level a converted campaign level is built on: Delta
/// Halo. Its sandbox is the closest to a30's (Covenant elites, grunts and
/// jackals against marines, outdoors on a ring), it holds every Halo 2
/// template `h1objects` needs for a30's weapons, equipment and vehicles
/// (warthog, ghost, banshee), and it has the most tag memory to spare of
/// the levels that do (0xaed000 of 0xc24000).
pub const BASE: &str = "05a_deltaapproach";

/// The base each Halo 1 campaign level is built on: [`BASE`] for all ten.
///
/// Measured per level (`every_campaign_level_converts`): with the base
/// scenario rebuilt from its relocated tree, built tags placed largest
/// first and Halo 1's repeated firing positions shared, every level fits
/// 05a's tag memory with its scenery and placed bipeds. b40 is the
/// tightest (0xc21000 of 0xc24000). 05a is also the only campaign base
/// with the pelican, phantom, wraith and Covenant turret templates next to
/// the warthog, ghost, scorpion and banshee; the alternatives with more
/// room each lack several of them (docs/game/campaigns.md §9). A level
/// moves only when a measured reason says so.
pub fn base_for(_level: &str) -> &'static str {
    BASE
}

/// Why a conversion failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    H1(#[from] h1map::Error),
    #[error(transparent)]
    Map(#[from] map::Error),
    #[error(transparent)]
    Convert(#[from] h1convert::Error),
    #[error(transparent)]
    Models(#[from] h1models::Error),
    #[error("{0}")]
    Invalid(String),
    /// The converted map needs more tag memory than the engine has.
    #[error(
        "{level}: tag memory {need:#x} over the engine's {max:#x}; largest groups: {groups}; scenario's largest fields: {fields}"
    )]
    TagMemory {
        level: String,
        need: u32,
        max: u32,
        /// The biggest groups, as `name built+pinned`.
        groups: String,
        /// The scenario's biggest root fields, as `offset bytes`.
        fields: String,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}

/// What the scripts layer needs besides the level: Halo 1's builtin table
/// (from its XBE) and the translation to Halo 2's.
pub struct Scripting {
    /// Halo 1's builtins and types.
    pub catalog: h1scripts::Catalog,
    /// Halo 2's.
    pub target: h1scripts::translate::Target,
    /// The mapping between them.
    pub table: h1scripts::translate::Table,
}

impl Scripting {
    /// From Halo 1's XBE, Halo 2's, and every Halo 2 scenario (for the
    /// builtin names and external globals Halo 2's table leaves unnamed).
    pub fn new(h1_xbe: &[u8], h2_xbe: &[u8], h2_scenarios: &[hs::Scenario]) -> Result<Self> {
        let parse = |b| xbe::Xbe::parse(b).map_err(|e| invalid(format!("xbe: {e}")));
        let catalog = h1scripts::Catalog::from_xbe(&parse(h1_xbe)?)
            .map_err(|e| invalid(format!("Halo 1 builtins: {e}")))?;
        let mut c = hs::Catalog::from_xbe(&parse(h2_xbe)?)
            .map_err(|e| invalid(format!("Halo 2 builtins: {e}")))?;
        for s in h2_scenarios {
            c.observe(s)
                .map_err(|e| invalid(format!("Halo 2 scripts: {e}")))?;
        }
        let mut target = h1scripts::translate::Target::from_catalog(&c);
        for s in h2_scenarios {
            target.observe_externals(s);
        }
        let table = h1scripts::translate::Table::build(&catalog, &target);
        Ok(Self {
            catalog,
            target,
            table,
        })
    }
}

/// Which layers to convert. Structure, sky and player starts are always
/// written.
pub struct Options<'a> {
    /// Weapons, equipment, vehicles and the player biped (h1objects).
    pub objects: bool,
    /// Squads, zones, orders (h1ai).
    pub ai: bool,
    /// Scripts (h1scripts), with hooks resolved to what was placed.
    pub scripts: Option<&'a Scripting>,
    /// Sounds the scripts name (h1sounds), up to this many tags.
    pub sounds: usize,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            objects: true,
            ai: true,
            scripts: None,
            sounds: 0,
        }
    }
}

/// What a conversion wrote and left out, layer by layer.
#[derive(Debug, Default, Clone)]
pub struct Report {
    /// Halo 1 level name.
    pub level: String,
    /// The Halo 2 map it was built on.
    pub base: String,
    /// Tags in the base, and in the converted map.
    pub tags_before: usize,
    pub tags_after: usize,
    /// Ordinary metadata of the converted map, and the engine's bound.
    pub metadata: u32,
    pub metadata_max: u32,
    /// Tag memory the map needs (index and metadata; bounded by
    /// `metadata_max`).
    pub memory: u32,
    /// Per tag group, largest first: (group, bytes of tags this conversion
    /// built, bytes of the base's pinned tags).
    pub groups: Vec<(String, u64, u64)>,
    /// Built tags that fit no hole in the base's metadata and so grew it,
    /// largest first: (group and name, bytes).
    pub appended: Vec<(String, u32)>,
    /// Per converted BSP.
    pub bsps: Vec<structure::BspReport>,
    /// The scenario's layers.
    pub scenario: scenario::ScenarioReport,
    /// Everything left out, one line each, with the reason.
    pub dropped: Vec<String>,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} on {}: {} tags ({} added), metadata {:#x}, tag memory {:#x} of {:#x}",
            self.level,
            self.base,
            self.tags_after,
            self.tags_after.saturating_sub(self.tags_before),
            self.metadata,
            self.memory,
            self.metadata_max
        )?;
        let grew: u32 = self.appended.iter().map(|a| a.1).sum();
        let top: Vec<String> = self
            .appended
            .iter()
            .take(4)
            .map(|(n, b)| format!("{n} {b:#x}"))
            .collect();
        writeln!(
            f,
            "  {} built tags ({grew:#x} bytes) past the base's metadata: {}",
            self.appended.len(),
            top.join(", ")
        )?;
        for b in &self.bsps {
            writeln!(f, "  {b}")?;
        }
        write!(f, "{}", self.scenario)?;
        for d in &self.dropped {
            writeln!(f, "  dropped: {d}")?;
        }
        Ok(())
    }
}

/// A converted level.
pub struct Converted {
    /// The map file (`<level>.map`).
    pub bytes: Vec<u8>,
    /// Counts and what was left out.
    pub report: Report,
    /// Each converted BSP's generated pathfinding, for checks.
    pub pathfinding: Vec<tags::pathfinding::PathfindingData>,
    /// The scenario path the map holds (`scenarios\solo\<level>\<level>`).
    pub scenario_path: String,
}

/// Converts the Halo 1 campaign level `h1` onto `base` (a retail Halo 2
/// campaign map, normally [`BASE`]).
pub fn convert<R: Read + Seek>(h1: &H1Map, base: &mut Map<R>, opts: &Options) -> Result<Converted> {
    let level = h1.header().name.to_ascii_lowercase();
    let mut report = Report {
        level: level.clone(),
        base: base.header().name.clone(),
        tags_before: base.tags().len(),
        ..Report::default()
    };
    let scnr = scenario::BaseScenario::read(base)?;
    let bsps = structure::prepare_all(h1, base, &scnr, &mut report)?;
    let objects_base = if opts.objects {
        Some(h1objects::base::Base::read(base).map_err(|e| invalid(format!("base: {e}")))?)
    } else {
        None
    };
    let mut set = TagSet::from_map(base)?;
    let scenario_path = manifest::scenario_path(&level);
    set.name = level.clone();
    set.scenario_path = scenario_path.clone();
    let slot = set.scenario.index();
    set.slots[slot].name = Some(scenario_path.clone());

    let mut sections = Vec::new();
    for (b, d) in bsps.iter().zip(&scnr.descriptors) {
        sections.push(b.write(&mut set, d.sbsp, d.ltmp)?);
    }
    let pathfinding = bsps.iter().map(|b| b.pathfinding.clone()).collect();
    scenario::write(
        h1,
        &mut set,
        &scnr,
        &sections,
        objects_base.as_ref(),
        opts,
        &mut report,
    )?;

    // Built without the bound, so an overflow can say by how much and
    // where the bytes went.
    set.check_tag_memory = false;
    // The rebuilt scenario and sound gestalt first, into their old ranges.
    set.largest_built_first = true;
    let (bytes, layout) = set.build()?;
    let max = map::writer::max_metadata_size(set.cache_type)?;
    report.memory = layout.memory_size;
    report.groups = memory_by_group(&set);
    if let Some(p) = &set.pinned {
        let end = p.metadata_base + p.metadata_size;
        let mut appended: Vec<(String, u32)> = set
            .slots
            .iter()
            .zip(&layout.tags)
            .filter(|(_, (a, n))| *n > 0 && *a >= end)
            .filter_map(|(s, (_, n))| {
                let t = s.tag.as_ref()?;
                Some((
                    format!("{} {}", t.group, s.name.as_deref().unwrap_or("")),
                    *n,
                ))
            })
            .collect();
        appended.sort_by_key(|a| std::cmp::Reverse(a.1));
        report.appended = appended;
    }
    if layout.memory_size > max {
        let groups = report.groups[..report.groups.len().min(8)]
            .iter()
            .map(|g| format!("{} {:#x}+{:#x}", g.0, g.1, g.2))
            .collect::<Vec<_>>()
            .join(", ");
        let fields = report
            .scenario
            .fields
            .iter()
            .map(|(at, n)| format!("{at:#x} {n:#x}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Error::TagMemory {
            level,
            need: layout.memory_size,
            max,
            groups,
            fields,
        });
    }
    let back = Map::from_reader(std::io::Cursor::new(bytes.as_slice()))?;
    report.tags_after = back.tags().len();
    report.metadata = back.header().meta_size;
    report.metadata_max = map::writer::max_metadata_size(back.header().cache_type)?;
    Ok(Converted {
        bytes,
        report,
        pathfinding,
        scenario_path,
    })
}

/// Bytes per tag group, largest first: (group, built, pinned).
fn memory_by_group(set: &TagSet) -> Vec<(String, u64, u64)> {
    let mut by: HashMap<map::FourCC, (u64, u64)> = HashMap::new();
    for t in set.slots.iter().filter_map(|s| s.tag.as_ref()) {
        let e = by.entry(t.group).or_default();
        match &t.payload {
            TagPayload::Built(b) => e.0 += b.tree_len(),
            TagPayload::Pinned { bytes, .. } => e.1 += bytes.len() as u64,
            _ => {}
        }
    }
    let mut v: Vec<_> = by
        .into_iter()
        .map(|(g, (b, p))| (g.to_string(), b, p))
        .collect();
    v.sort_by_key(|g| std::cmp::Reverse(g.1 + g.2));
    v
}

/// Whether `p` stands on one of `pf`'s sectors: inside a sector's outline
/// seen from above, with the sector's plane at most `below` under `p` and
/// at most `above` over it.
pub fn on_sector(
    pf: &tags::pathfinding::PathfindingData,
    p: [f32; 3],
    below: f32,
    above: f32,
) -> bool {
    use pathfind::geom::{contains_xy, newell};
    (0..pf.sectors.len()).any(|k| {
        let Some(ring) = pf.sector_polygon(k) else {
            return false;
        };
        let poly: Vec<[f32; 3]> = ring.into_iter().map(|i| pf.vertices[i]).collect();
        if poly.len() < 3 || !contains_xy(&poly, p[0], p[1]) {
            return false;
        }
        let n = newell(&poly);
        if n[2].abs() < 1e-6 {
            return false;
        }
        let d = n[0] * poly[0][0] + n[1] * poly[0][1] + n[2] * poly[0][2];
        let z = (d - n[0] * p[0] - n[1] * p[1]) / n[2];
        z <= p[2] + above && p[2] - z <= below
    })
}

#[cfg(test)]
mod tests;
