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
use serde::Serialize;
use tags::pathfinding::SBSP_PATHFINDING_FIELD;

mod ai;
mod bases;
pub mod builtin;
pub mod ground;
mod nav;
pub mod retail;
mod transplant;
/// An invented map the kit builds on, for tests here and in other crates.
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use bases::{Bases, Config, Source, Team};
pub use retail::RetailClass;
pub use ground::{Ground, Labels, Problem, Receipt, Recipe};
pub use transplant::{ClassCost, ClassSpec, Classes, Donor};

/// The kit's version: the `firefight` crate's, which session data names.
pub const KIT_VERSION: u32 = firefight::KIT_VERSION;

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
    fn parts(&self) -> [(&'static str, usize); 9] {
        let mut parts = [
            ("sectors", self.sectors),
            ("zones", self.zones),
            ("squads", self.squads),
            ("characters and palettes", self.characters),
            ("transplanted classes", self.classes),
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
        write!(
            f,
            "sectors {} B ({} of {} generated kept{merged}), zones {} B, squads {} B, characters and palettes {} B, transplanted classes {} B, scripts {} B, starting locations {} B, tag index {} B, padding {} B; tag growth {} B, available {} B",
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
fn build_with_classes(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
) -> Result<Built> {
    let available = headroom(base)?;
    fit_within(base, base_name, mode, config, labels, classes, available)
}

/// The tag memory `base` has left: `max_metadata_size(Multiplayer)` less
/// its `+0x1c`.
fn headroom(base: &[u8]) -> Result<u32> {
    let map = Map::from_reader(Cursor::new(base))?;
    Ok(max_metadata_size(CacheType::Multiplayer)?.saturating_sub(map.header().meta_size))
}

/// [`build_with_classes`] against `available` bytes of tag headroom.
fn fit_within(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
    available: u32,
) -> Result<Built> {
    let total = classes.map_or(0, |c| c.specs.len());
    let fewest = if classes.is_some_and(|c| c.cut_to_fit) {
        0
    } else {
        total
    };
    let mut over = None;
    for keep in (fewest..=total).rev() {
        for steps in 0..=Fit::ALL.len() {
            let fit = &Fit::ALL[..steps];
            match assemble(
                base, base_name, mode, config, labels, classes, keep, fit, available,
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
                    return Ok(built);
                }
                Err(e) if e.downcast_ref::<transplant::Overflow>().is_some() => over = Some(e),
                Err(e) => return Err(e),
            }
        }
    }
    Err(over.expect("one build at least"))
}

/// One build of the kit with the first `keep` classes and the `fit`
/// steps, refused when its growth is over `available`.
#[allow(clippy::too_many_arguments)]
fn assemble(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    labels: &Labels,
    classes: Option<&Classes<'_>>,
    keep: usize,
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
    let added = match classes {
        Some(c) => transplant::add(&mut set, c, keep)?,
        None => transplant::Added::default(),
    };
    let named: Vec<(&str, TagId)> = added
        .costs
        .iter()
        .map(|c| c.name.as_str())
        .zip(added.ids.iter().copied())
        .collect();
    let ai = ai::prepare(
        &mut map,
        nav.zones,
        nav.base_zones,
        elite,
        &named,
        mode == Mode::Probe,
    )
    .map_err(anyhow::Error::msg)?;
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
    for (field, (address, bytes)) in fields.iter().zip(allocations) {
        set_pair(&mut set, &map, field, address)?;
        if field.tag != scenario {
            costs.sectors += bytes;
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

    // Use the real layout (including a tag-index move and alignment), so a
    // refusal can give precisely the same cost breakdown as a successful build.
    set.check_tag_memory = false;
    let (bytes, layout) = set.build()?;
    costs.characters += layout.tags[elite.index()].1 as usize;
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
    })
}

/// Write `bytes` (the kit's map on `base`) as a versioned file patch
/// package; its id.
fn install(
    base: &[u8],
    base_name: &str,
    mode: Mode,
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
    let patch = content::patch::diff(base, bytes, &format!("Firefight kit v{KIT_VERSION}"));
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
/// builder, packaging and headroom check as the agent recipe.
pub fn build_package(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    maps_dir: &Path,
    package_dir: &str,
    replace: bool,
) -> Result<Package> {
    build_package_with(
        base,
        base_name,
        mode,
        config,
        None,
        maps_dir,
        package_dir,
        replace,
    )
}

/// [`build_package`] with the transplanted `classes` (stage two). The
/// package id follows the class list, so machines with the same donors
/// and list build the same id.
#[allow(clippy::too_many_arguments)]
pub fn build_package_with(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    config: &Config,
    classes: Option<&Classes<'_>>,
    maps_dir: &Path,
    package_dir: &str,
    replace: bool,
) -> Result<Package> {
    let built = build_with_classes(
        base,
        base_name,
        mode,
        config,
        &Labels::default(),
        classes,
    )?;
    let id = install(
        base,
        base_name,
        mode,
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
    })
}

#[cfg(test)]
mod tests;
