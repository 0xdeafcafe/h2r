//! Build a stage-one Firefight package from the owner's multiplayer cache.
//! Only the authored character gets a new datum. Scenario and BSP roots keep
//! their addresses and point at new block trees in resident tag memory.
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
mod nav;

pub const KIT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Kit,
    Probe,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Costs {
    pub sectors: usize,
    pub zones: usize,
    pub squads: usize,
    pub characters: usize,
    pub scripts: usize,
    /// Actual increase in header +0x1c, including page rounding and any
    /// relocated tag index. File tables and patch compression do not count.
    pub tag_growth: u32,
    pub available: u32,
}

impl fmt::Display for Costs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sectors {} B, zones {} B, squads {} B, characters and palettes {} B, scripts {} B; tag growth {} B, available {} B",
            self.sectors,
            self.zones,
            self.squads,
            self.characters,
            self.scripts,
            self.tag_growth,
            self.available
        )
    }
}

pub struct Package {
    pub id: MapId,
    pub costs: Costs,
    pub zones: usize,
    pub squads: usize,
}

struct Built {
    bytes: Vec<u8>,
    costs: Costs,
    zones: usize,
    squads: usize,
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

fn build(base: &[u8], base_name: &str, mode: Mode) -> Result<Built> {
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
    let nav = nav::prepare(&mut map)?;
    let zones = nav.zones.len();
    let mut set = TagSet::from_map(&mut map)?;
    ensure!(
        set.find(FourCC(*b"char"), ai::ELITE_PATH).is_none(),
        "base already carries a Firefight character"
    );
    set.add_group(FourCC(*b"char"), None, None);
    let elite = set.add_tag(FourCC(*b"char"), ai::ELITE_PATH, TagPayload::Empty)?;
    // The version travels in the map, so it participates in the package id
    // even when two revisions happen to write identical AI tuning.
    set.intern(&format!("firefight_kit_v{KIT_VERSION}"))?;
    let ai =
        ai::prepare(&mut map, nav.zones, elite, mode == Mode::Probe).map_err(anyhow::Error::msg)?;
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
    let allocations = set.append_pinned_blocks(&blocks)?;
    let mut costs = Costs::default();
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
    costs.tag_growth = layout
        .memory_size
        .checked_sub(base_memory)
        .context("pinned map unexpectedly shrank")?;
    costs.available = max_metadata_size(CacheType::Multiplayer)?.saturating_sub(base_memory);
    ensure!(
        costs.tag_growth <= costs.available,
        "Firefight kit exceeds tag headroom: {costs}"
    );
    Ok(Built {
        bytes,
        costs,
        zones,
        squads,
    })
}

/// Install a versioned file patch in the standard map library. The source
/// stays on the local machine and is read only. Both modes use the same
/// builder, packaging and headroom check as the agent recipe.
pub fn build_package(
    base: &[u8],
    base_name: &str,
    mode: Mode,
    maps_dir: &Path,
    package_dir: &str,
    replace: bool,
) -> Result<Package> {
    let built = build(base, base_name, mode)?;
    let mut manifest = Manifest::retail(&base_name.to_ascii_lowercase(), Sha256::of(base));
    manifest.package.name = match mode {
        Mode::Kit => format!("Firefight – {base_name}"),
        Mode::Probe => format!("Firefight Probe – {base_name}"),
    };
    let patch = content::patch::diff(base, &built.bytes, &format!("Firefight kit v{KIT_VERSION}"));
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
    Ok(Package {
        id: manifest.map_id(),
        costs: built.costs,
        zones: built.zones,
        squads: built.squads,
    })
}

#[cfg(test)]
mod tests;
