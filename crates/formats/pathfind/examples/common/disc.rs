//! Opens maps inside the user's own disc image (read-only). Shared by the
//! examples and the #[ignore] disc tests; nothing from the disc is copied.
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
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../disc/halo2.iso"))
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

/// Every map name under /maps/.
#[allow(dead_code)]
pub fn map_names() -> Result<Vec<String>> {
    let path = disc_path();
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path)?))?;
    Ok(iso
        .entries()?
        .into_iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            let p = e.path.to_ascii_lowercase();
            let rest = p.strip_prefix("/maps/")?.strip_suffix(".map")?;
            (!rest.contains('/')).then(|| rest.to_owned())
        })
        .collect())
}

/// BSP tag IDs from the scenario's BSP block (+0x210, stride 0x44).
pub fn scenario_bsps(map: &mut MapReader) -> Result<Vec<TagId>> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect())
}

/// The BSP payload and its virtual address.
pub fn bsp_payload(map: &mut MapReader, id: TagId) -> Result<(Vec<u8>, u32)> {
    let tag = map.tag(id)?.clone();
    let loc = tag.location.context("bsp has no payload")?;
    Ok((map.read_tag_bytes(id)?, loc.address))
}

/// Collision BSP and pathfinding data of one scenario BSP.
#[allow(dead_code)]
pub fn load_bsp(
    map: &mut MapReader,
    index: usize,
) -> Result<(
    tags::collision::Bsp,
    Option<tags::pathfinding::PathfindingData>,
    Vec<u8>,
    u32,
)> {
    let id = *scenario_bsps(map)?.get(index).context("no such bsp")?;
    let (bytes, base) = bsp_payload(map, id)?;
    let view = TagView::new(&bytes, base);
    let h = view.at(0, 0x200)?;
    let coll = view.block(
        h,
        tags::pathfinding::SBSP_COLLISION_FIELD,
        tags::collision::Bsp::STRUCTURE_SIZE,
    )?;
    let at = coll[0].as_ptr() as usize - bytes.as_ptr() as usize;
    let bsp = tags::collision::Bsp::decode_structure(&bytes, base, at)?;
    let pf = tags::pathfinding::PathfindingData::read(map, id)?;
    Ok((bsp, pf, bytes, base))
}
