//! Halo 1 campaign levels as Halo 2 v8 maps (docs/halo1/campaign.md, step
//! 3 onward).
//!
//! A level is converted onto a **pinned retail Halo 2 campaign map** (the
//! base, [`BASE`]): its globals, HUD, shared tags and the Halo 2 characters
//! the AI stands in with stay the base's. On top of it:
//!
//! - [`structure`]: every Halo 1 structure BSP (collision, MOPP, render
//!   geometry, Halo 1's lightmap, generated AI pathfinding) in the base's
//!   BSP slots and new `sbsp`/`ltmp` tags past them, through `h1convert`'s
//!   writer;
//! - [`donor`]: the characters the base lacks, copied whole
//!   from other retail maps, sounds included;
//! - [`scenario`]: the base's scenario rebuilt as a built root over its own
//!   pinned children, with Halo 1's sky, player starts, object placements,
//!   AI and scripts written over the base's;
//! - [`manifest`]: the level's place in the Halo 1 campaign
//!   (`content::campaign_list`).
//!
//! Then, unless [`Options::pinned`], the level is built from scratch
//! ([`scratch`]): its own scenario ([`scenario::fresh_root`]), every tag
//! read by path and laid out anew, and only what the scenario, the globals
//! and [`KEEP`] reach. A level that cannot be built so is an error
//! ([`Error::Scratch`]), never a map with the base left in.
//!
//! No game data is in this crate or its tests. The `#[ignore]` tests read
//! the owner's discs at run time.
#![forbid(unsafe_code)]

pub mod cutscenes;
mod actor_control;
mod ai_control_characters;
mod ai_control_roster;
pub mod donor;
pub mod h2assets;
pub mod manifest;
pub mod profiles;
pub mod recordings;
mod roster;
pub mod scenario;
pub mod scratch;
mod squad_roster;
pub mod structure;

use std::io::{Read, Seek};

use h1map::H1Map;
use h1map::phase;
use map::{Map, TagId};
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
/// 05a's tag memory with every BSP, its scenery and placed bipeds, and the
/// characters donors bring. b40 is the tightest (0xc24000 of 0xc24000).
/// 05a is also the only campaign base with the pelican, phantom, wraith
/// and Covenant turret templates next to the warthog, ghost, scorpion and
/// banshee; the alternatives with more room each lack several of them (docs/halo1/campaign.md §9). A level
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
    #[error("{level}: map file {need:#x} bytes, over the native campaign slot's {max:#x}")]
    FileSize { level: String, need: u64, max: u64 },
    /// The level could not be built from scratch ([`scratch`]): a tag the
    /// repository cannot take exactly, or one the build cannot lay out.
    #[error("{level}: from scratch: {reason}")]
    Scratch { level: String, reason: String },
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
    /// Retail maps to copy the characters the base lacks from
    /// ([`donor`]); none copies nothing.
    pub donors: Option<&'a donor::Donors>,
    /// Halo 1's cinematics and the a10 tutorial: played or skipped. `None`
    /// (the default) plays all ten levels as authored
    /// ([`cutscenes::default_for`]; docs/halo1/campaign.md, Cutscenes).
    pub cutscenes: Option<h1scripts::compile::Cutscenes>,
    /// The animation graphs and recorded animations the scripts name
    /// ([`cutscenes`]); off measures what they cost.
    pub cutscene_data: bool,
    /// The animation graphs alone (their animation data is raw in the map
    /// file, which the campaign cache bounds).
    pub cutscene_graphs: bool,
    /// Keep the base's file and metadata (`--pinned`). By default the
    /// level is built from scratch ([`scratch`]); a level that cannot be is
    /// an error ([`Error::Scratch`]).
    pub pinned: bool,
    /// Which of Halo 1's devices are placed. `convert` places fewer when a
    /// level is still over a bound with everything else left out (pinned
    /// b40 and d40).
    pub devices: Devices,
    /// Halo 1's characters, weapons and vehicles, or Halo 2's nearest
    /// ([`h2assets`], docs/halo1/campaign.md §5).
    pub assets: h2assets::Assets,
    /// What conversions of this level onto this base share (its two
    /// asset variants): each BSP prepared once. `None` prepares its own.
    pub shared: Option<&'a Shared>,
    /// The base's whole file, when the caller holds it: read in place
    /// rather than copied out of the base map.
    pub base_file: Option<&'a [u8]>,
}

/// What a level's conversions onto one base share ([`Options::shared`]):
/// its BSPs, prepared once (`structure::prepare_all` reads only the Halo 1
/// map, the base and the level's palettes, never the assets or devices),
/// with the same palettes for both asset variants.
#[derive(Default)]
pub struct Shared {
    bsps: std::sync::Mutex<Option<std::sync::Arc<PreparedBsps>>>,
}

struct PreparedBsps {
    bsps: Vec<structure::Prepared>,
    reports: Vec<structure::BspReport>,
}

impl Shared {
    fn bsps(
        &self,
        prepare: impl FnOnce() -> Result<PreparedBsps>,
    ) -> Result<std::sync::Arc<PreparedBsps>> {
        let mut slot = self.bsps.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = slot.as_ref() {
            return Ok(p.clone());
        }
        let prepared = std::sync::Arc::new(prepare()?);
        *slot = Some(prepared.clone());
        Ok(prepared)
    }
}

/// Which of Halo 1's devices a conversion places.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Devices {
    /// Machines, controls and light fixtures (as machines).
    #[default]
    All,
    /// Machines and controls: no light fixtures (Halo 2 has no `lifi`, so
    /// they are lamp models that give no light).
    NoLightFixtures,
    /// None.
    None,
}

impl Devices {
    fn fewer(self) -> Option<Self> {
        match self {
            Self::All => Some(Self::NoLightFixtures),
            Self::NoLightFixtures => Some(Self::None),
            Self::None => None,
        }
    }
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            objects: true,
            ai: true,
            scripts: None,
            sounds: 0,
            donors: None,
            cutscenes: None,
            cutscene_data: true,
            cutscene_graphs: true,
            pinned: false,
            devices: Devices::All,
            assets: h2assets::Assets::Halo1,
            shared: None,
            base_file: None,
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
    /// Tag trees copied from donor maps.
    pub brought: Vec<donor::Brought>,
    /// The scenario's layers.
    pub scenario: scenario::ScenarioReport,
    /// Everything left out, one line each, with the reason.
    pub dropped: Vec<String>,
    /// Whether the map was built from scratch (nothing of the base
    /// pinned); false only for [`Options::pinned`].
    pub fresh: bool,
    /// What the from-scratch build did; `None` only for [`Options::pinned`].
    pub scratch: Option<scratch::ScratchReport>,
    /// How lean the conversion had to be to fit (`convert`).
    pub lean: Lean,
    /// The map file's size, bytes.
    pub file_size: u64,
    /// Each conversion retried before this one, and why (`convert`).
    pub retries: Vec<String>,
    /// All Halo 1 looping definitions in the level and their local sample bytes.
    pub looping_sounds: usize,
    pub looping_bytes: u64,
}

/// The lean ladder: what a conversion of a level over the engine's tag
/// memory bound leaves out, one more step each time (`convert`). Each step
/// leaves out its own extra and every step before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lean {
    /// Everything.
    #[default]
    Everything,
    /// The seat-mapping table.
    SeatTable,
    /// The cutscenes' animation graphs. Gameplay recordings stay installed
    /// ([`cutscenes`]).
    CutsceneData,
    /// The level data the scripts name (cutscene flags, camera points,
    /// titles, device groups): the calls naming it are stubbed
    /// (`h1scripts::compile::INDEXED`), their other hooks stay `none`.
    LevelData,
}

impl Lean {
    /// The steps in order.
    pub const LADDER: [Lean; 4] = [
        Lean::Everything,
        Lean::SeatTable,
        Lean::CutsceneData,
        Lean::LevelData,
    ];

    /// The next step down, if any.
    pub fn next(self) -> Option<Lean> {
        let i = Self::LADDER.iter().position(|&l| l == self)?;
        Self::LADDER.get(i + 1).copied()
    }

    /// Whether this conversion leaves out `step`'s extra.
    pub fn leaves_out(self, step: Lean) -> bool {
        step != Lean::Everything && self >= step
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} on {} ({}): {} tags ({} added), metadata {:#x}, tag memory {:#x} of {:#x}, file {:#x} ({} looping sounds, {:#x} sample bytes), lean {:?}",
            self.level,
            self.base,
            if self.fresh { "fresh" } else { "pinned" },
            self.tags_after,
            self.tags_after.saturating_sub(self.tags_before),
            self.metadata,
            self.memory,
            self.metadata_max,
            self.file_size,
            self.looping_sounds,
            self.looping_bytes,
            self.lean
        )?;
        if let Some(s) = &self.scratch {
            writeln!(
                f,
                "  from scratch: {} tags ({} retail, {} converted, {} scenario and BSPs; {} datum words rewritten), {} dangling",
                s.tags,
                s.from_base,
                s.from_level,
                s.built_here,
                s.words,
                s.dangling.len()
            )?;
            match (&s.gestalt, &s.gestalt_whole) {
                (Some(g), _) => writeln!(
                    f,
                    "  gestalt cut to {} sounds: {} -> {} bytes, {} -> {} chunks; {} raw resources ({} bytes) dropped",
                    g.sounds,
                    g.bytes.0,
                    g.bytes.1,
                    g.chunks.0,
                    g.chunks.1,
                    g.raw_dropped.0,
                    g.raw_dropped.1
                )?,
                (None, Some(why)) => writeln!(f, "  gestalt kept whole: {why}")?,
                _ => {}
            }
        }
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
        for b in &self.brought {
            writeln!(f, "  brought: {b}")?;
        }
        write!(f, "{}", self.scenario)?;
        for d in &self.dropped {
            writeln!(f, "  dropped: {d}")?;
        }
        for r in &self.retries {
            writeln!(f, "  retried: {r}")?;
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
    // The tag memory retries affect scenario layers. Audio stays in every attempt.
    let mut lean = Lean::Everything;
    let preserve_content = !opts.pinned
        && opts.scripts.is_some()
        && opts.cutscene_data
        && opts.cutscene_graphs
        && opts.cutscenes.unwrap_or_default() == h1scripts::compile::Cutscenes::Play;
    let mut opts = Options { ..*opts };
    // Each retry and why, for the report.
    let mut retries = Vec::new();
    loop {
        match convert_with(h1, base, &opts, lean) {
            // A normal fresh scripted campaign promises the whole level. Report
            // a capacity regression with the original diagnostics before a retry
            // could remove a seat, a device, a recording dependency or a graph.
            Err(e @ (Error::TagMemory { .. } | Error::FileSize { .. })) if preserve_content => {
                return Err(e);
            }
            Err(Error::TagMemory { need, max, .. }) if lean.next().is_some() => {
                lean = lean.next().expect("checked");
                retries.push(format!("tag memory {need:#x} over {max:#x}: lean {lean:?}"));
            }
            Err(Error::TagMemory { need, max, .. }) if opts.devices.fewer().is_some() => {
                opts.devices = opts.devices.fewer().expect("checked");
                retries.push(format!(
                    "tag memory {need:#x} over {max:#x} at the end of the ladder: devices {:?}",
                    opts.devices
                ));
            }
            Ok(mut done) => {
                done.report.retries = retries;
                return Ok(done);
            }
            r => return r,
        }
    }
}

fn convert_with<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    opts: &Options,
    lean: Lean,
) -> Result<Converted> {
    let level = h1.header().name.to_ascii_lowercase();
    let mut report = Report {
        lean,
        level: level.clone(),
        base: base.header().name.clone(),
        tags_before: base.tags().len(),
        ..Report::default()
    };
    let scnr = scenario::BaseScenario::read(base)?;
    let ambience = h1convert::ambience::read_campaign(h1)?;
    let prepare = |base: &mut Map<R>| -> Result<PreparedBsps> {
        let mut r = Report::default();
        let bsps = phase::time("bsp", || {
            structure::prepare_all(h1, base, &ambience, &mut r)
        })?;
        Ok(PreparedBsps {
            bsps,
            reports: r.bsps,
        })
    };
    let prepared = match opts.shared {
        Some(s) => s.bsps(|| prepare(base))?,
        None => std::sync::Arc::new(prepare(base)?),
    };
    report.bsps.extend(prepared.reports.iter().cloned());
    let bsps = &prepared.bsps;
    let objects_base = if opts.objects || opts.ai {
        let b = phase::time("base shader programs", || h1objects::base::Base::read(base))
            .map_err(|e| invalid(format!("base: {e}")))?;
        Some(b)
    } else {
        None
    };
    let mut set = phase::time("tag set", || TagSet::from_map(base))?;
    let scenario_path = manifest::scenario_path(&level);
    set.name = level.clone();
    set.scenario_path = scenario_path.clone();
    let slot = set.scenario.index();
    set.slots[slot].name = Some(scenario_path.clone());

    // The level's background sounds and sound environments, which the
    // clusters and the scenario's palettes name.
    let raw_start = set.raw.len();
    let loops = phase::time("looping sounds", || h1sounds::looping::install_level_loops(h1, &mut set))
        .map_err(|e| invalid(format!("{}: looping sounds: {e}", level)))?;
    report.looping_sounds = loops.len();
    report.looping_bytes = set.raw[raw_start..].iter().map(|r| r.bytes.len() as u64).sum();
    let sounds = phase::time("ambience", || ambience.install(&mut set))?.bounded();

    // Halo 1's indoor fog, if the level has one, is the scenario's
    // atmosphere element 0, which every skyless cluster names
    // (`scenario::indoor_fog`).
    let indoor = scenario::indoor_fog(h1)?.map(|_| 0u8);

    // Every Halo 1 BSP: the base's slots first, then new sbsp/ltmp tags
    // under Halo 1's names.
    let mut slots = Vec::new();
    for (i, b) in bsps.iter().enumerate() {
        let (sbsp, ltmp) = match scnr.descriptors.get(i) {
            Some(d) => (d.sbsp, d.ltmp),
            None => (
                set.add_tag(map::FourCC(*b"sbsp"), &b.name, TagPayload::Empty)?,
                set.add_tag(map::FourCC(*b"ltmp"), &b.name, TagPayload::Empty)?,
            ),
        };
        let section = phase::time("bsp", || b.write(&mut set, sbsp, ltmp, &sounds, indoor))?;
        if !opts.pinned {
            // A fresh level's BSPs are Halo 1's, named so in every slot.
            for id in [sbsp, ltmp] {
                set.slots[id.index()].name = Some(b.name.clone());
            }
        }
        slots.push(scenario::BspSlot {
            section,
            sbsp,
            ltmp,
            checksum: b.checksum,
            centroids: b.centroids(),
            sounds: b.sound_clusters.clone(),
        });
    }
    let pathfinding: Vec<_> = bsps.iter().map(|b| b.pathfinding.clone()).collect();
    if let Some(d) = opts.donors.filter(|_| opts.objects || opts.ai) {
        let mut wanted = if opts.assets == h2assets::Assets::Halo2 {
            donor::wanted(h1)?
        } else {
            // The native Sentinel behaviour style has no Halo 1 tag equivalent.
            let ai = h1ai::h1::H1Ai::read(h1).map_err(|e| invalid(format!("ai: {e}")))?;
            let sentinel = ai.actors.iter().any(|(_, actor)| actor.kind.0 == 13);
            sentinel
                .then(|| (map::FourCC(*b"styl"), roster::SENTINEL_STYLE.to_owned()))
                .into_iter()
                .collect()
        };
        if opts.assets == h2assets::Assets::Halo2
            && let Some(scripts) = opts.scripts
        {
            wanted.extend(actor_control::wanted(h1, &scripts.catalog)?);
        }
        // The retail weapons, vehicles and player the Halo 2-assets
        // variant places that the base lacks.
        if opts.objects && opts.assets == h2assets::Assets::Halo2 {
            wanted.extend(h2assets::wanted(h1, &set, d)?);
        }
        phase::time("donors", || donor::bring(d, &mut set, &wanted, &mut report))?;
    }
    let mut locales = h1text::convert::Locales::from_map(base)
        .map_err(|e| invalid(format!("base text: {e}")))?;
    let base_rows = locales.len();
    let level_text = h1text::scenario::LevelText::install(h1, &mut set, &mut locales)
        .map_err(|e| invalid(format!("level text: {e}")))?;
    phase::time("scenario", || {
        scenario::write(
            h1,
            &mut set,
            &scnr,
            &slots,
            &pathfinding,
            bsps,
            &sounds,
            objects_base.as_ref(),
            opts,
            lean,
            &level_text,
            &mut report,
        )
    })?;

    level_text.write_object_prompts(&mut set);
    if locales.len() != base_rows {
        locales.install_pinned(&mut set)
            .map_err(|e| invalid(format!("installing level text: {e}")))?;
    }

    // Built without the bound, so an overflow can say by how much and
    // where the bytes went.
    set.check_tag_memory = false;
    set.check_file_size = false;
    // The rebuilt scenario and sound gestalt first, into their old ranges.
    set.largest_built_first = true;
    let built_map = phase::time("build map", || set.build());
    let (mut bytes, mut layout) = built_map?;
    let _build = phase::start("build map");
    if !opts.pinned {
        // From scratch ([`scratch`]). A failure is the conversion's error:
        // there is no other way to write the level without the base.
        let copied;
        let file = match opts.base_file {
            Some(f) => {
                debug_assert_eq!(f.len() as u64, base.storage_length());
                f
            }
            None => {
                copied = scratch::file_bytes(base)?;
                &copied[..]
            }
        };
        // Each Halo 2 HUD is named by nothing but the HUD pairs: kept for
        // `boot::hud` (docs/halo1/hud.md).
        let mut keep = loops;
        keep.extend(report.scenario.hud_swaps.iter().map(|h| h[2]));
        let (out, r) =
            scratch::build(file, &set, &bytes, &keep).map_err(|e| Error::Scratch {
                level: level.clone(),
                reason: e.to_string(),
            })?;
        report.fresh = true;
        report.scratch = Some(r);
        // Over the tag memory or the native campaign slot, the checks
        // below name the bound: no pinned map is written in its place.
        let built = out.build()?;
        report.scenario.hud_swaps = remap_hud(&set, &out, &report.scenario.hud_swaps)
            .ok_or_else(|| invalid("a HUD pair's tag left out of the fresh map"))?;
        (bytes, layout) = built;
        set = out;
    }
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
    let limit = u64::from(map::writer::NATIVE_CAMPAIGN_FILE_LIMIT);
    if bytes.len() as u64 > limit {
        return Err(Error::FileSize {
            level,
            need: bytes.len() as u64,
            max: limit,
        });
    }
    report.file_size = bytes.len() as u64;
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

/// The roots a fresh campaign level keeps besides what its scenario and
/// globals reach (`h1convert::scratch::build`). No scenario palette is
/// emptied: the conversion already replaced every one it places from.
pub const KEEP: h1convert::fresh::Keep<'static> = h1convert::fresh::Keep {
    palettes: &[],
    groups: &h1convert::fresh::KEEP_GROUPS,
    prefixes: &h1convert::fresh::KEEP_PREFIXES,
};

/// The HUD pairs `hud` (datums of `from`) as datums of `to`, by group and
/// path; `None` when one is not in `to`.
pub fn remap_hud(from: &TagSet, to: &TagSet, hud: &[[TagId; 3]]) -> Option<Vec<[TagId; 3]>> {
    let key = |d: TagId| {
        let slot = from.slots.get(d.index())?;
        let t = slot.tag.as_ref().filter(|t| t.id == d)?;
        Some((t.group, slot.name.clone()))
    };
    let by_key: std::collections::HashMap<_, TagId> = to
        .slots
        .iter()
        .filter_map(|s| s.tag.as_ref().map(|t| ((t.group, s.name.clone()), t.id)))
        .collect();
    let new = |d: TagId| key(d).and_then(|k| by_key.get(&k).copied());
    hud.iter()
        .map(|h| Some([new(h[0])?, new(h[1])?, new(h[2])?]))
        .collect()
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
