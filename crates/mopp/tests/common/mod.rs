//! Opens maps inside the user's own disc image (read-only). Shared by the
//! examples and the #[ignore] disc tests; nothing from the disc is copied.
#![allow(dead_code)]
use anyhow::{Context, Result};
use map::{Map, TagId};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use tags::geometry::TagView;

pub type MapReader = Map<BufReader<File>>;

/// `H2_DISC`, else `disc/halo2.iso` in the repository.
pub fn disc_path() -> PathBuf {
    std::env::var_os("H2_DISC")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso"))
}

pub fn open_map(name: &str) -> Result<MapReader> {
    let path = disc_path();
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))
        .with_context(|| format!("open {}", path.display()))?;
    let partition = iso.partition_offset();
    let want = format!("/maps/{name}.map");
    let e = iso
        .entries()?
        .into_iter()
        .find(|e| !e.is_dir && e.path.eq_ignore_ascii_case(&want))
        .with_context(|| format!("no {want} on disc"))?;
    Ok(Map::from_region(
        BufReader::new(File::open(&path)?),
        partition + u64::from(e.sector) * xiso::SECTOR,
        u64::from(e.size),
    )?)
}

/// Every map name under /maps/, sorted.
pub fn map_names() -> Result<Vec<String>> {
    let path = disc_path();
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    let mut names: Vec<String> = iso
        .entries()?
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            let p = e.path.to_ascii_lowercase();
            let rest = p.strip_prefix("/maps/")?.strip_suffix(".map")?;
            (!rest.contains('/')).then(|| rest.to_owned())
        })
        .collect();
    names.sort();
    Ok(names)
}

/// BSP tag IDs from the scenario's BSP block (+0x210, stride 0x44).
pub fn scenario_bsps<R: std::io::Read + std::io::Seek>(map: &mut Map<R>) -> Result<Vec<TagId>> {
    let Some(scenario) = map.scenario().cloned() else {
        return Ok(Vec::new());
    };
    let Some(location) = scenario.location else {
        return Ok(Vec::new());
    };
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect())
}

/// One structure BSP: its collision BSP, and its MOPP block bytes
/// (`sbsp+0x1e0`: its count and bytes).
pub struct Level {
    pub name: String,
    pub bsp_index: usize,
    pub collision: tags::collision::Bsp,
    pub mopp_count: u32,
    pub mopp: Vec<u8>,
}

pub fn load_level<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    name: &str,
    index: usize,
    id: TagId,
) -> Result<Level> {
    let tag = map.tag(id)?.clone();
    let loc = tag.location.context("bsp has no payload")?;
    let bytes = map.read_tag_bytes(id)?;
    let base = loc.address;
    let view = TagView::new(&bytes, base);
    let h = view.at(0, 0x240)?;
    let coll = view.block(h, 0x14, tags::collision::Bsp::STRUCTURE_SIZE)?;
    let at = coll[0].as_ptr() as usize - bytes.as_ptr() as usize;
    let collision = tags::collision::Bsp::decode_structure(&bytes, base, at)?;
    let count = u32::from_le_bytes(h[0x1e0..0x1e4].try_into()?);
    let addr = u32::from_le_bytes(h[0x1e4..0x1e8].try_into()?);
    let mopp = view.range(addr, u64::from(count))?.to_vec();
    Ok(Level {
        name: name.to_owned(),
        bsp_index: index,
        collision,
        mopp_count: count,
        mopp,
    })
}

/// Every structure BSP on the disc (multiplayer, campaign, mainmenu).
pub fn all_levels() -> Result<Vec<Level>> {
    let mut out = Vec::new();
    for name in map_names()? {
        if name == "shared" || name == "single_player_shared" {
            continue;
        }
        let mut map = open_map(&name)?;
        for (i, id) in scenario_bsps(&mut map)?.into_iter().enumerate() {
            out.push(load_level(&mut map, &name, i, id)?);
        }
    }
    Ok(out)
}

pub mod metrics;
pub mod replace;
