//! Build a Firefight package from the owner's multiplayer cache.
//! The kit arms the map's two bases (`bases`): drop zones keep off them,
//! one zone and one Hunt order per base, and each starting location takes
//! its nearest base's team. Stage one authors one character; stage two
//! (`transplant`) copies more classes from donor maps. Scenario and BSP
//! roots keep their addresses and point at new block trees in resident
//! tag memory.
use std::{fmt, io::Cursor, path::Path};

use anyhow::{Context, Result, ensure};
use content::{Manifest, MapId, Sha256, manifest::Layer};
use h1campaign::ai::h2::layout;
use map::{
    CacheType, FourCC, Map, TagId,
    build::{Block, TagPayload, TagSet},
    writer::max_metadata_size,
};
use firefight::ShipKind;
use serde::Serialize;
use tags::pathfinding::SBSP_PATHFINDING_FIELD;

mod ai;
mod bases;
pub mod builtin;
pub mod ground;
mod nav;
pub mod retail;
pub mod ship;
pub mod sky;
mod transplant;
/// An invented map the kit builds on, for tests here and in other crates.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use firefight::Faction;
pub use bases::{Bases, Config, Source, Team};
pub use ship::{Dropship, ShipReport};
pub use sky::{Fall, PodReport};
pub use retail::RetailClass;
pub use ground::{Ground, Labels, Problem, Receipt, Recipe};
pub use transplant::{ClassCost, ClassSpec, Classes, Donor, copy_unit};

/// The kit's version: the `firefight` crate's, which session data names.
pub const KIT_VERSION: u32 = firefight::KIT_VERSION;

/// The disc map a build without a class donor reads the campaign Elite's
/// behaviour rows from (`ai::character::DONOR_ELITE`; every campaign map
/// carries it, and this one is a ship donor too).
pub const ELITE_DONOR_MAP: &str = "03a_oldmombasa";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Kit,
    Probe,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Costs {
    /// The pathfinding of every structure BSP: sectors, links, vertices,
    /// one ref per collision surface, the element.
    pub sectors: usize,
    /// Sectors generated from the walkable collision surfaces, and of
    /// them the ones kept: what the AI can use (`nav::usable_sectors`).
    pub sectors_generated: usize,
    pub sectors_kept: usize,
    /// The kept sectors once coplanar neighbours are merged
    /// ([`Fit::MergeSectors`]); `sectors_kept` when the build does not
    /// merge.
    pub sectors_merged: usize,
    pub zones: usize,
    pub squads: usize,
    pub characters: usize,
    /// The transplanted classes' tags and index entries
    /// (`ClassCost::bytes` summed): an estimate inside `tag_growth`.
    pub classes: usize,
    /// The dropship: its transplanted tags (when a donor gave them), its
    /// parked placement, palette row, object name and point sets.
    pub ship: usize,
    /// The drop pods: the copied tags, the pools, the falls and their
    /// names (`sky`).
    pub pods: usize,
    pub scripts: usize,
    /// The starting locations, when their rewritten rows cannot stay in
    /// place (0 when they do).
    pub starts: usize,
    /// The part of the tag index the new character's entry pushes into
    /// the ordinary metadata (`map::build::BuildLayout::index_in_metadata`):
    /// the group table, or the whole entry table when the groups' room is
    /// not enough.
    pub index: usize,
    /// Alignment between the parts and the rounding of the metadata to a
    /// page: the rest of the growth.
    pub padding: usize,
    /// Actual increase in header +0x1c: every part above. File tables and
    /// patch compression do not count.
    pub tag_growth: u32,
    pub available: u32,
}

impl Costs {
    /// The parts, largest first.
    fn parts(&self) -> [(&'static str, usize); 11] {
        let mut parts = [
            ("sectors", self.sectors),
            ("zones", self.zones),
            ("squads", self.squads),
            ("characters and palettes", self.characters),
            ("transplanted classes", self.classes),
            ("dropship", self.ship),
            ("drop pods", self.pods),
            ("scripts", self.scripts),
            ("starting locations", self.starts),
            ("tag index", self.index),
            ("padding", self.padding),
        ];
        parts.sort_by_key(|&(_, bytes)| std::cmp::Reverse(bytes));
        parts
    }
}

impl fmt::Display for Costs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let merged = if self.sectors_merged == self.sectors_kept {
            String::new()
        } else {
            format!(", merged into {}", self.sectors_merged)
        };
        let ship = if self.ship == 0 {
            String::new()
        } else {
            format!("dropship {} B, ", self.ship)
        };
        let pods = if self.pods == 0 {
            String::new()
        } else {
            format!("drop pods {} B, ", self.pods)
        };
        write!(
            f,
            "sectors {} B ({} of {} generated kept{merged}), zones {} B, squads {} B, characters and palettes {} B, transplanted classes {} B, {ship}{pods}scripts {} B, starting locations {} B, tag index {} B, padding {} B; tag growth {} B, available {} B",
            self.sectors,
            self.sectors_kept,
            self.sectors_generated,
            self.zones,
            self.squads,
            self.characters,
            self.classes,
            self.scripts,
            self.starts,
            self.index,
            self.padding,
            self.tag_growth,
            self.available
        )
    }
}

/// A step the kit takes on a map short of tag headroom, in the order it
/// takes them: each is tried only when the build without it is over the
/// headroom, and every step before it is kept. Classes are cut
/// ([`Classes::cut_to_fit`]) only after every step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Coplanar neighbouring sectors become one convex sector
    /// (`pathfind::merge::coplanar`): the same ground in fewer sectors,
    /// links and vertices.
    MergeSectors,
    /// The first six drop zones chosen are kept, not up to twelve: they
    /// alternate between the bases' sides and spread farthest first.
    /// Placed `ff_drop` labels are never dropped.
    FewerDrops,
}

impl Fit {
    /// Every step, in the order the kit takes them.
    pub const ALL: [Fit; 2] = [Fit::MergeSectors, Fit::FewerDrops];

    /// What the step gives up, for the report.
    pub fn reason(self) -> &'static str {
        match self {
            Fit::MergeSectors => {
                "coplanar sectors merged into convex ones: the same ground in fewer bytes"
            }
            Fit::FewerDrops => "six drop zones kept instead of up to twelve",
        }
    }

    /// The lean build these steps make.
    fn lean(steps: &[Fit]) -> nav::Lean {
        nav::Lean {
            merge: steps.contains(&Fit::MergeSectors),
            fewer_drops: steps.contains(&Fit::FewerDrops),
        }
    }
}

impl fmt::Display for Fit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Fit::MergeSectors => "merge sectors",
            Fit::FewerDrops => "fewer drops",
        })
    }
}

pub struct Package {
    pub id: MapId,
    pub costs: Costs,
    /// Drop zones.
    pub zones: usize,
    pub squads: usize,
    /// The two bases the kit armed, base 0 the home base.
    pub bases: Bases,
    /// What each kept class added, in palette order after the Elite.
    pub classes: Vec<ClassCost>,
    /// The classes cut to fit the headroom, first cut first.
    pub cut: Vec<String>,
    /// The steps taken to fit the headroom, in order ([`Fit`]).
    pub fitted: Vec<Fit>,
    /// The dropship, kept or cut; none when the build asked for none
    /// ([`Dropship::None`]).
    pub ship: Option<ShipReport>,
    /// The Pelican, kept or cut; none when no dropship was asked for.
    pub pelican: Option<ShipReport>,
    /// The drop pods, kept or cut; none when the build asked for none
    /// (a probe, or [`Config::no_pods`]).
    pub pods: Option<PodReport>,
}

struct Built {
    bytes: Vec<u8>,
    costs: Costs,
    zones: usize,
    squads: usize,
    bases: Bases,
    problems: Vec<Problem>,
    classes: Vec<ClassCost>,
    cut: Vec<String>,
    fitted: Vec<Fit>,
    ship: Option<ShipReport>,
    pelican: Option<ShipReport>,
    pods: Option<PodReport>,
}

struct Field {
    tag: TagId,
    offset: usize,
    count: u32,
}

/// Patch a root pair in its original address space. BSPs can share virtual
/// addresses across sections, so look up the file location by salted datum.
fn set_pair(set: &mut TagSet, map: &Map<Cursor<&[u8]>>, field: &Field, address: u32) -> Result<()> {
    let loc = map
        .tag(field.tag)?
        .location
        .context("tag has no local root")?;
    ensure!(
        field.offset + 8 <= loc.size as usize,
        "block field outside its root"
    );
    let at = field.offset;
    let pair = if Some(loc.space) == map.metadata_space() {
        let TagPayload::Pinned { bytes, .. } = &mut set
            .tag_mut(field.tag)
            .context("missing pinned tag")?
            .payload
        else {
            anyhow::bail!("expected a pinned scenario");
        };
        &mut bytes[at..at + 8]
    } else {
        let pin = set.pinned.as_mut().context("missing BSP file prefix")?;
        // PinnedImage::file starts immediately after the 0x800-byte header.
        let rel = loc
            .file_offset()?
            .checked_sub(0x800)
            .context("BSP in cache header")?;
        let start = usize::try_from(rel)? + at;
        pin.file
            .get_mut(start..start + 8)
            .context("BSP outside pinned file prefix")?
    };
    pair[..4].copy_from_slice(&field.count.to_le_bytes());
    pair[4..].copy_from_slice(&address.to_le_bytes());
    Ok(())
}

/// Give each starting location its rewritten team designator in place, when
/// the rows lie in the scenario's own pinned bytes (where a cache keeps a
/// tag's child blocks). False when they do not; the caller then writes the
/// rows as a new block.
fn starts_in_place(set: &mut TagSet, scenario: TagId, rows: &[Vec<u8>]) -> Result<bool> {
    use h1campaign::scenario::{STARTING_LOCATION, STARTING_LOCATIONS};
    let TagPayload::Pinned { address, bytes } = &mut set
        .tag_mut(scenario)
        .context("missing pinned scenario")?
        .payload
    else {
        anyhow::bail!("expected a pinned scenario");
    };
    let field = STARTING_LOCATIONS as usize;
    let pair = bytes
        .get(field..field + 8)
        .context("starting locations outside the scenario root")?;
    ensure!(
        u32::from_le_bytes(pair[..4].try_into()?) as usize == rows.len(),
        "starting location count changed"
    );
    let at = u32::from_le_bytes(pair[4..].try_into()?);
    let len = rows.len() * STARTING_LOCATION;
    let Some(start) = at
        .checked_sub(*address)
        .map(|o| o as usize)
        .filter(|&o| o + len <= bytes.len())
    else {
        return Ok(false);
    };
    let team = bases::START_TEAM..bases::START_TEAM + 2;
    for (i, row) in rows.iter().enumerate() {
        let o = start + i * STARTING_LOCATION;
        bytes[o + team.start..o + team.end].copy_from_slice(&row[team.clone()]);
    }
    Ok(true)
}

/// The kit on `base` with no labels placed: the generator picks the bases
/// and the drops.
#[cfg(any(test, feature = "testkit"))]
fn build(base: &[u8], base_name: &str, mode: Mode, config: &Config) -> Result<Built> {
    build_with_classes(base, base_name, mode, config, &Labels::default(), None)
}

/// The kit on `base`, taking its places from `labels` where any are placed
/// (`ground::build`, docs/game/game-types-without-carriers.md, "Maps for
/// scripted games"; `labels` is the canonical set, `Labels::canonical`),
/// with the transplanted `classes`. A build over the tag
/// headroom takes the [`Fit`] steps in order, then, with `cut_to_fit`,
/// drops the last class and takes them again, until it fits
/// (`transplant`); otherwise it is refused with every part's and class's
/// cost after every step.
///
/// The Elite's behaviour rows are the campaign Elite's, from the base or
/// a class donor ([`elite_rows`]).
#[allow(dead_code)] // the class-gated kit build; no caller yet
fn build_with_classes(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
) -> Result<Built> {
    build_with_elite(base, base_name, mode, config, labels, classes, None)
}

/// [`build_with_classes`] with one more map to read the campaign Elite
/// from, `elite`, tried after the base and before the class donors.
fn build_with_elite(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
    elite: Option<&Donor<'_>>,
) -> Result<Built> {
    let available = headroom(base)?;
    let rows = elite_rows(base, base_name, classes, elite)?;
    fit_within(base, base_name, mode, config, labels, classes, &rows, available)
}

/// The campaign Elite's behaviour rows the kit's Elite takes, from the
/// first of the base, `elite` and the class donors that carries
/// [`ai::character::DONOR_ELITE`]. None carrying it, or the one that does
/// lacking a row, refuses the build by name.
fn elite_rows(
    base: &[u8],
    base_name: &str,
    classes: Option<&Classes<'_>>,
    elite: Option<&Donor<'_>>,
) -> Result<ai::character::DonorRows> {
    let maps = std::iter::once((base_name, base))
        .chain(elite.map(|d| (d.name, d.bytes)))
        .chain(classes.into_iter().flat_map(|c| c.donors.iter().map(|d| (d.name, d.bytes))));
    let mut looked = Vec::new();
    for (name, bytes) in maps {
        let mut map = Map::from_reader(Cursor::new(bytes))
            .with_context(|| format!("reading {name} for the campaign Elite"))?;
        if let Some(rows) = ai::character::donor_rows(&mut map, name).map_err(anyhow::Error::msg)? {
            return Ok(rows);
        }
        looked.push(name);
    }
    anyhow::bail!(
        "Firefight's Elite takes its behaviour rows from the campaign Elite {}, and no map given carries it ({}); give a campaign map such as {ELITE_DONOR_MAP}",
        ai::character::DONOR_ELITE,
        looked.join(", ")
    )
}

/// The most `+0x1c` a kit may take a multiplayer map to:
/// `max_metadata_size(Campaign)`, the largest retail campaign map's
/// (`0xc24000`) plus the arena's growth (`map::arena::GROWTH`). The engine
/// reserves one physical arena for every map type, and a campaign map loads
/// its largest beside its two shared maps, which is more than a multiplayer
/// map of this size plus its one shared map (docs/game/firefight.md, "The
/// kit"). It is not a loader constant: the multiplayer bound
/// `max_metadata_size(Multiplayer)` is the largest retail multiplayer map
/// plus the same growth.
const KIT_METADATA_BUDGET: CacheType = CacheType::Campaign;

/// The one run a zone has: the widest kept ship's where it has one, else the
/// next widest's. A kind the run does not clear at its own hover and reach
/// is left off that zone's path ([`ship::recheck`]).
fn shared_runs(ship_runs: &[(ShipKind, ship::Dims, Vec<Option<ship::Run>>)]) -> Vec<Option<ship::Run>> {
    let mut order: Vec<_> = ship_runs.iter().collect();
    order.sort_by(|a, b| b.1.radius.total_cmp(&a.1.radius));
    let zones = order.first().map_or(0, |o| o.2.len());
    (0..zones)
        .map(|z| order.iter().find_map(|o| o.2.get(z).copied().flatten()))
        .collect()
}

/// The tag memory `base` has left: [`KIT_METADATA_BUDGET`] less its `+0x1c`.
fn headroom(base: &[u8]) -> Result<u32> {
    let map = Map::from_reader(Cursor::new(base))?;
    Ok(max_metadata_size(KIT_METADATA_BUDGET)?.saturating_sub(map.header().meta_size))
}

/// [`build_with_classes`] against `available` bytes of tag headroom.
#[allow(clippy::too_many_arguments)]
fn fit_within(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
    elite: &ai::character::DonorRows,
    available: u32,
) -> Result<Built> {
    let total = classes.map_or(0, |c| c.specs.len());
    let fewest = if classes.is_some_and(|c| c.cut_to_fit) {
        0
    } else {
        total
    };
    // The ship is cut before any class: it is tried with every class, then
    // without, and only then do classes go.
    // A probe is two Elites and a startup script: no ship.
    let asked = config.dropship != Dropship::None && mode == Mode::Kit;
    let asked_pods = !config.no_pods && mode == Mode::Kit;
    // Pods go right after the ship, before any class.
    // The Pelican goes before the Phantom (`retail::KEEP_ORDER`): a ship
    // count is how many of the kinds are tried, in `ShipKind::ALL` order.
    let kinds = if asked && config.dropship != Dropship::Spirit {
        ShipKind::ALL.len()
    } else {
        usize::from(asked)
    };
    let mut attempts: Vec<(usize, usize, bool)> = Vec::new();
    if asked || asked_pods {
        attempts.push((total, kinds, asked_pods));
    }
    if asked {
        for n in (1..kinds).rev() {
            attempts.push((total, n, asked_pods));
        }
    }
    if asked && asked_pods {
        attempts.push((total, 0, true));
    }
    attempts.extend((fewest..=total).rev().map(|keep| (keep, 0, false)));
    let mut over = None;
    let mut ship_cut: Option<String> = None;
    let mut pelican_cut: Option<String> = None;
    let mut pod_cut: Option<String> = None;
    for (keep, ships, pod) in attempts {
        for steps in 0..=Fit::ALL.len() {
            let fit = &Fit::ALL[..steps];
            match assemble(
                base, base_name, mode, config, labels, classes, elite, keep, ships, pod, fit,
                available,
            ) {
                Ok(mut built) => {
                    if let Some(c) = classes {
                        built.cut = c.specs[keep..]
                            .iter()
                            .rev()
                            .map(|s| s.name.clone())
                            .collect();
                    }
                    built.fitted = fit.to_vec();
                    if asked && ships == 0 && built.ship.is_none() {
                        built.ship = Some(ShipReport::cut(
                            ship_cut.unwrap_or_else(|| "over the tag headroom".into()),
                        ));
                    }
                    if kinds > 1 && ships < 2 && built.pelican.is_none() {
                        built.pelican = Some(ShipReport::cut_kind(
                            ShipKind::Pelican,
                            pelican_cut.unwrap_or_else(|| "over the tag headroom".into()),
                        ));
                    }
                    if asked_pods && !pod && built.pods.is_none() {
                        built.pods = Some(PodReport::cut(
                            pod_cut.unwrap_or_else(|| "over the tag headroom".into()),
                        ));
                    }
                    return Ok(built);
                }
                Err(e) if e.downcast_ref::<transplant::Overflow>().is_some() => over = Some(e),
                // `auto` goes without a ship it cannot get; a ship asked
                // for by name is an error.
                Err(e)
                    if config.dropship == Dropship::Auto
                        && e.downcast_ref::<transplant::ShipRefused>().is_some() =>
                {
                    ship_cut = Some(e.to_string());
                    break;
                }
                // The Pelican is always optional.
                Err(e) if e.downcast_ref::<transplant::PelicanRefused>().is_some() => {
                    pelican_cut = Some(e.to_string());
                    break;
                }
                // Pods are always optional.
                Err(e) if e.downcast_ref::<transplant::PodRefused>().is_some() => {
                    pod_cut = Some(e.to_string());
                    break;
                }
                Err(e) => return Err(e),
            }
        }
    }
    Err(over.expect("one build at least"))
}

/// One build of the kit with the first `keep` classes, the first `ships`
/// kinds of ship and the `fit` steps, refused when its growth is over `available`.
#[allow(clippy::too_many_arguments)]
fn assemble(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
    elite_rows: &ai::character::DonorRows,
    keep: usize,
    ships: usize,
    pod: bool,
    fit: &[Fit],
    available: u32,
) -> Result<Built> {
    let mut map = Map::from_reader(Cursor::new(base))?;
    ensure!(
        map.header().cache_type == CacheType::Multiplayer,
        "Firefight needs a multiplayer base"
    );
    ensure!(
        map.header().name.eq_ignore_ascii_case(base_name),
        "base name does not match the map header"
    );
    let base_memory = map.header().meta_size;
    let scenario = map.scenario().context("missing scenario")?.id;
    let nav = nav::prepare(&mut map, config, labels, Fit::lean(fit))?;
    ensure!(
        nav.refused.is_empty(),
        "Firefight layout{}: {}",
        config
            .layout
            .map_or(String::new(), |l| format!(" for {}", l.map)),
        nav.refused.join("; ")
    );
    let zones = nav.zones.len();
    let bases = nav.bases;
    let problems = nav.problems;
    let mut set = TagSet::from_map(&mut map)?;
    ensure!(
        set.find(FourCC(*b"char"), ai::ELITE_PATH).is_none(),
        "base already carries a Firefight character"
    );
    set.add_group(FourCC(*b"char"), None, None);
    // A retail index has no room for the new entry before its tail: move
    // the group table (12 bytes a group) rather than the entry table (16
    // bytes a tag, about 80 KB).
    set.relocate_groups = true;
    let elite = set.add_tag(FourCC(*b"char"), ai::ELITE_PATH, TagPayload::Empty)?;
    // The version travels in the map, so it participates in the package id
    // even when two revisions happen to write identical AI tuning.
    set.intern(&format!("firefight_kit_v{KIT_VERSION}"))?;
    // The ships: the kinds asked for, in `ShipKind::ALL` order, each sized
    // from its own tag and kept only when a zone has a column clear for a
    // ship that big.
    let ship_bsp = nav.ship_bsp;
    let mut ship_report = None;
    let mut pelican_report = None;
    let asked_kinds: Vec<ShipKind> = if config.dropship == Dropship::None || ships == 0 {
        Vec::new()
    } else if config.dropship == Dropship::Spirit {
        vec![ShipKind::Phantom]
    } else {
        ShipKind::ALL[..ships.min(ShipKind::ALL.len())].to_vec()
    };
    let mut asked = Vec::new();
    for kind in &asked_kinds {
        let pick = match kind {
            ShipKind::Phantom => ship::pick(&set, config.dropship)?,
            ShipKind::Pelican => ship::pick_pelican(&set),
        };
        let name = match &pick {
            ship::Pick::Have(_, name) => name.as_str(),
            ship::Pick::Copy(path) => path,
        };
        // The tag in the base map, else in a donor's.
        let dims = ship::dims_in(&mut map, name)
            .or_else(|| {
                classes?.donors.iter().find_map(|d| {
                    ship::dims_in(&mut Map::from_reader(Cursor::new(d.bytes)).ok()?, name)
                })
            })
            .unwrap_or(ship::Dims::default_for(*kind));
        let dims = match config.ship_hovers.and_then(|s| s.median(*kind)) {
            Some((hover, _)) => ship::Dims { hover, ..dims },
            None => dims,
        };
        asked.push((*kind, pick, dims));
    }
    let mut picks: Vec<(ShipKind, ship::Pick)> = Vec::new();
    let mut ship_runs: Vec<(ShipKind, ship::Dims, Vec<Option<ship::Run>>)> = Vec::new();
    for (kind, pick, dims) in asked {
        let runs = match &nav.ship_walls {
            Some(walls) => ship::plan(walls, &nav.zones, dims.radius, dims.hover),
            None => vec![None; nav.zones.len()],
        };
        if runs.iter().any(Option::is_some) {
            picks.push((kind, pick));
            ship_runs.push((kind, dims, runs));
        } else {
            let why = match kind {
                ShipKind::Phantom => "no drop zone has a clear column for it",
                ShipKind::Pelican => "no drop zone has a clear column wide enough for it",
            };
            let report = ShipReport::cut_kind(kind, why);
            match kind {
                ShipKind::Phantom => ship_report = Some(report),
                ShipKind::Pelican => pelican_report = Some(report),
            }
        }
    }
    // The one path a zone is the widest kept ship's; every kind is checked
    // on it at its own hover and reach, and a kind it blocks everywhere is cut.
    let mut valid: Vec<(ShipKind, Vec<bool>)> = Vec::new();
    while let (Some(walls), false) = (&nav.ship_walls, ship_runs.is_empty()) {
        let shared = shared_runs(&ship_runs);
        valid = ship_runs
            .iter()
            .map(|(k, d, _)| (*k, ship::recheck(walls, &shared, d)))
            .collect();
        let Some(blocked) = valid.iter().position(|(_, v)| !v.contains(&true)) else {
            break;
        };
        let kind = ship_runs.remove(blocked).0;
        picks.remove(blocked);
        valid.remove(blocked);
        let report = ShipReport::cut_kind(kind, "its run in and out are blocked at its hover height");
        match kind {
            ShipKind::Phantom => ship_report = Some(report),
            ShipKind::Pelican => pelican_report = Some(report),
        }
    }
    let copies: Vec<&str> = picks
        .iter()
        .filter_map(|(_, pick)| match pick {
            ship::Pick::Copy(path) => Some(*path),
            _ => None,
        })
        .collect();
    // The drop pods, when a zone or a start has an open column.
    let falls = nav.falls;
    let mut pod_report = None;
    let pod_pick = if !pod {
        None
    } else if falls.zone_count() + falls.start_count() == 0 {
        pod_report = Some(PodReport::cut("no zone or start has a clear column to the sky"));
        None
    } else {
        Some(sky::pick(&set))
    };
    let copy_pod = match &pod_pick {
        Some(sky::Pick::Copy(path)) => Some(*path),
        _ => None,
    };
    let added = match classes {
        Some(c) => transplant::add(&mut set, c, keep, &copies, copy_pod)?,
        None if !copies.is_empty() => {
            let phantom_copied = picks
                .iter()
                .any(|(k, p)| *k == ShipKind::Phantom && matches!(p, ship::Pick::Copy(_)));
            return Err(if phantom_copied {
                transplant::ShipRefused("no donor maps for the dropship".into()).into()
            } else {
                transplant::PelicanRefused("no donor maps for the pelican".into()).into()
            });
        }
        None if copy_pod.is_some() => {
            return Err(transplant::PodRefused("no donor maps for the drop pod".into()).into());
        }
        None => transplant::Copied::default(),
    };
    let mut copied = added.ships.iter();
    let mut ship_tags: Vec<(ShipKind, TagId, String)> = Vec::new();
    for (kind, pick) in &picks {
        match pick {
            ship::Pick::Have(id, name) => ship_tags.push((*kind, *id, name.clone())),
            ship::Pick::Copy(path) => {
                if let Some((id, _)) = copied.next() {
                    ship_tags.push((*kind, *id, (*path).to_owned()));
                }
            }
        }
    }
    // A kit with Flood classes and falls over its zones drops its squads in
    // the Flood pod: a tag of our own, built into the map.
    let flood_pod = if copy_pod.is_some() || pod_pick.is_some() {
        let flood = added.factions.contains(&firefight::Faction::Flood)
            && falls.zone_count() > 0;
        if flood {
            let refused = |e: anyhow::Error| {
                anyhow::Error::from(transplant::PodRefused(format!("Flood pod: {e:#}")))
            };
            let facts = content_kit::flood_pod::kit::Facts::read(&mut map).map_err(refused)?;
            let tags = content_kit::flood_pod::kit::install(&mut set, &facts).map_err(refused)?;
            set.intern(&content_kit::flood_pod::inject::marker_string())?;
            Some(tags)
        } else {
            None
        }
    } else {
        None
    };
    let pod_tag = match &pod_pick {
        Some(sky::Pick::Have(id, name)) => Some((*id, name.clone())),
        Some(sky::Pick::Copy(path)) => added.pod.as_ref().map(|(id, _)| (*id, (*path).to_owned())),
        None => None,
    };
    let named: Vec<(&str, TagId, firefight::Faction)> = added
        .costs
        .iter()
        .zip(added.ids.iter().zip(&added.factions))
        .map(|(c, (id, faction))| (c.name.as_str(), *id, *faction))
        .collect();
    let ai = ai::prepare(
        &mut map,
        nav.zones,
        nav.base_zones,
        elite,
        elite_rows,
        &named,
        mode == Mode::Probe,
    )
    .map_err(anyhow::Error::msg)?;
    // One path a zone, cleared for the widest ship kept.
    let sized = |kind: ShipKind| ship_runs.iter().find(|(k, _, _)| *k == kind);
    let shared_runs = shared_runs(&ship_runs);
    let kept_sizes: Vec<(ShipKind, ship::Dims, Vec<bool>)> = ship_tags
        .iter()
        .filter_map(|(k, _, _)| {
            let ok = valid.iter().find(|(v, _)| v == k)?.1.clone();
            Some((*k, sized(*k)?.1, ok))
        })
        .collect();
    let authored = {
        let tags: Vec<(ShipKind, TagId)> = ship_tags.iter().map(|(k, t, _)| (*k, *t)).collect();
        ship::author(&mut map, &tags, &shared_runs, ship_bsp, &kept_sizes)?
    };
    let ship_kept = authored.as_ref().map(|a| a.paths);
    let mut ship_fields = authored.map(|a| a.fields).unwrap_or_default();
    let pods_authored = match &pod_tag {
        Some((tag, _)) => sky::author(
            &mut map,
            *tag,
            flood_pod.as_ref().map(|t| t.vehicle),
            &falls,
            ship_bsp,
            &mut ship_fields,
            if ship_kept.is_some() {
                ship::path_sets(&shared_runs, ship_bsp, &kept_sizes)
            } else {
                Vec::new()
            },
        )?,
        None => None,
    };
    // Each starting location takes its nearest base's team, so team 0
    // spawns at the home base and team 1 at the other.
    let starts_moved = !nav.starts.is_empty() && !starts_in_place(&mut set, scenario, &nav.starts)?;
    set.tag_mut(elite).expect("just added").payload = TagPayload::Built(ai.character);

    let mut fields = Vec::new();
    let mut blocks = Vec::new();
    let mut add = |tag: TagId, offset: usize, count: u32, child: Block| -> Result<()> {
        ensure!(
            (count == 0) == child.bytes.is_empty(),
            "invalid block count at {offset:#x}"
        );
        fields.push(Field { tag, offset, count });
        blocks.push(child);
        Ok(())
    };
    for (id, element) in nav.bsps {
        add(id, SBSP_PATHFINDING_FIELD, 1, element)?;
    }
    for (offset, count, block) in ai.fields.fields {
        add(scenario, offset as usize, count, block)?;
    }
    let mut ship_offsets = Vec::new();
    for (offset, count, block) in ship_fields {
        ship_offsets.push(offset);
        add(scenario, offset, count, block)?;
    }
    if starts_moved {
        add(
            scenario,
            h1campaign::scenario::STARTING_LOCATIONS as usize,
            nav.starts.len() as u32,
            Block::new(nav.starts.concat()),
        )?;
    }
    let allocations = set.append_pinned_blocks(&blocks)?;
    let mut costs = Costs {
        sectors_generated: nav.trim.generated,
        sectors_kept: nav.trim.kept,
        sectors_merged: nav.trim.merged,
        classes: added.costs.iter().map(|c| c.bytes).sum(),
        ..Costs::default()
    };
    let mut squads = 0;
    costs.ship = added.ships.iter().map(|(_, c)| c.bytes).sum();
    costs.pods = added.pod.as_ref().map_or(0, |(_, c)| c.bytes);
    for (field, (address, bytes)) in fields.iter().zip(allocations) {
        set_pair(&mut set, &map, field, address)?;
        if field.tag != scenario {
            costs.sectors += bytes;
        } else if ship_offsets.contains(&field.offset) {
            costs.ship += bytes;
        } else if field.offset == layout::ZONES.0 {
            costs.zones += bytes;
        } else if field.offset == layout::SQUADS.0 || field.offset == layout::SQUAD_GROUPS.0 {
            costs.squads += bytes;
            if field.offset == layout::SQUADS.0 {
                squads = field.count as usize;
            }
        } else if field.offset == h1campaign::scenario::STARTING_LOCATIONS as usize {
            costs.starts += bytes;
        } else if [layout::CHARACTER_PALETTE.0, 0x98].contains(&field.offset) {
            costs.characters += bytes;
        } else {
            costs.scripts += bytes;
        }
    }

    // The pods' share of the vehicle blocks they extended.
    if let Some(a) = &pods_authored {
        costs.pods += a.bytes;
        costs.ship = costs.ship.saturating_sub(a.bytes);
    }

    // Use the real layout (including a tag-index move and alignment), so a
    // refusal can give precisely the same cost breakdown as a successful build.
    set.check_tag_memory = false;
    let (bytes, layout) = set.build()?;
    costs.characters += layout.tags[elite.index()].1 as usize;
    if let Some(t) = &flood_pod {
        costs.pods += [t.vehicle, t.model, t.render, t.physics, t.shader, t.bitmap]
            .iter()
            .map(|id| layout.tags[id.index()].1 as usize)
            .sum::<usize>();
    }
    costs.index = layout.index_in_metadata as usize;
    costs.tag_growth = layout
        .memory_size
        .checked_sub(base_memory)
        .context("pinned map unexpectedly shrank")?;
    costs.padding = (costs.tag_growth as usize)
        .saturating_sub(costs.parts().iter().map(|&(_, bytes)| bytes).sum::<usize>());
    costs.available = available;
    if costs.tag_growth > costs.available {
        let (biggest, bytes) = costs.parts()[0];
        let steps = if fit.is_empty() {
            String::new()
        } else {
            format!(
                " and taking every step to fit ({})",
                fit.iter().map(|f| f.reason()).collect::<Vec<_>>().join("; ")
            )
        };
        return Err(transplant::Overflow(format!(
            "Firefight kit exceeds tag headroom by {} B; the biggest part is {biggest} ({bytes} B), after trimming the sectors to the {} of {} generated the AI can use{steps}: {costs}{}",
            costs.tag_growth - costs.available,
            costs.sectors_kept,
            costs.sectors_generated,
            transplant::describe(&added.costs),
        ))
        .into());
    }
    if let Some(paths) = ship_kept {
        // A kind's own copied tags; the placements and paths are the first
        // kind's, so the reports add up to the ships' cost.
        let copied: usize = added.ships.iter().map(|(_, c)| c.bytes).sum();
        let mut own = added.ships.iter();
        for (n, (kind, pick_tag, tag)) in ship_tags.iter().enumerate() {
            let cost = if picks.iter().any(|(k, p)| k == kind && matches!(p, ship::Pick::Copy(_))) {
                own.next().map(|(_, c)| (c.tags, c.bytes))
            } else {
                None
            };
            let _ = pick_tag;
            let (tags, own_bytes) = cost.unwrap_or((0, 0));
            let report = ShipReport {
                kind: *kind,
                kept: true,
                tag: tag.clone(),
                tags,
                bytes: own_bytes + if n == 0 { costs.ship.saturating_sub(copied) } else { 0 },
                paths,
                why: None,
                dims: sized(*kind).map(|(_, d, _)| *d),
                hover_source: config.ship_hovers.and_then(|s| {
                    let (_, n) = s.median(*kind)?;
                    Some(format!("{n} campaign waypoints (median; {})", s.maps(*kind)))
                }),
            };
            match kind {
                ShipKind::Phantom => ship_report = Some(report),
                ShipKind::Pelican => pelican_report = Some(report),
            }
        }
    }
    if let (Some(a), Some((_, tag))) = (&pods_authored, pod_tag) {
        pod_report = Some(PodReport {
            kept: true,
            tag,
            tags: added.pod.as_ref().map_or(0, |(_, c)| c.tags) + flood_pod.as_ref().map_or(0, |_| 6),
            bytes: costs.pods,
            zone_falls: falls.zone_count(),
            open_starts: falls.start_count(),
            starts: falls.starts.len(),
            why: None,
        });
        let _ = a;
    }
    Ok(Built {
        bytes,
        costs,
        zones,
        squads,
        bases,
        problems,
        classes: added.costs,
        cut: Vec::new(),
        fitted: Vec::new(),
        ship: ship_report,
        pelican: pelican_report,
        pods: pod_report,
    })
}

/// Write `bytes` (the kit's map on `base`) as a versioned file patch
/// package; its id.
fn install(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    layout: Option<&builtin::Layout>,
    bytes: &[u8],
    maps_dir: &Path,
    package_dir: &str,
    replace: bool,
) -> Result<MapId> {
    let mut manifest = Manifest::retail(&base_name.to_ascii_lowercase(), Sha256::of(base));
    manifest.package.name = match mode {
        Mode::Kit => format!("Firefight – {base_name}"),
        Mode::Probe => format!("Firefight Probe – {base_name}"),
    };
    // The layout's digest is in the label, which a PPF patch carries. A
    // layout that changes the map changes the patch, so the id follows it.
    let label = match layout {
        None => format!("Firefight kit v{KIT_VERSION}"),
        Some(l) => format!("Firefight kit v{KIT_VERSION} layout {}", l.digest()),
    };
    let patch = content::patch::diff(base, bytes, &label);
    let sha256 = Sha256::of(&patch.bytes);
    let file = format!("kit-{sha256}.{}", patch.format.extension());
    manifest.patches.push(Layer {
        file: file.clone(),
        sha256,
    });
    content::store::write_package(
        maps_dir,
        package_dir,
        &manifest,
        &[(&file, &patch.bytes)],
        replace,
    )?;
    Ok(manifest.map_id())
}

/// Install a versioned file patch in the standard map library. The source
/// stays on the local machine and is read only. Both modes use the same
/// builder, packaging and headroom check as the agent recipe. `elite` is
/// the disc map the Elite's behaviour rows come from (a campaign map, such
/// as [`ELITE_DONOR_MAP`]); without one, the base must carry the campaign
/// Elite, which no retail multiplayer map does.
#[allow(clippy::too_many_arguments)]
pub fn build_package(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    elite: Option<&Donor<'_>>,
    maps_dir: &Path,
    package_dir: &str,
    replace: bool,
) -> Result<Package> {
    build_package_with(base, base_name, mode, config, None, elite, maps_dir, package_dir, replace)
}

/// [`build_package`] with the transplanted `classes` (stage two). The
/// package id follows the class list, so machines with the same donors
/// and list build the same id. The Elite's behaviour rows come from the
/// base, else `elite`, else a class donor.
#[allow(clippy::too_many_arguments)]
pub fn build_package_with(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    classes: Option<&Classes<'_>>,
    elite: Option<&Donor<'_>>,
    maps_dir: &Path,
    package_dir: &str,
    replace: bool,
) -> Result<Package> {
    let built =
        build_with_elite(base, base_name, mode, config, &Labels::default(), classes, elite)?;
    let id = install(
        base,
        base_name,
        mode,
        config.layout,
        &built.bytes,
        maps_dir,
        package_dir,
        replace,
    )?;
    Ok(Package {
        id,
        costs: built.costs,
        zones: built.zones,
        squads: built.squads,
        bases: built.bases,
        classes: built.classes,
        cut: built.cut,
        fitted: built.fitted,
        ship: built.ship,
        pelican: built.pelican,
        pods: built.pods,
    })
}

#[cfg(test)]
mod tests;
