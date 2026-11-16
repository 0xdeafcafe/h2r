//! Replace a retail level's MOPP with ours, through map::build.
#![allow(dead_code)]
use anyhow::{Context, Result, bail};
use map::build::TagSet;
use map::{Map, TagId};
use mopp::geom::{STRUCTURE_KEY_TYPE, Surface, structure_surfaces};
use std::io::Cursor;
use tags::geometry::TagView;

/// The surfaces a rebuilt retail program indexes: every structure surface,
/// plus each other key of the retail program (instanced geometry and the
/// other key types) as the box its retail path guarantees.
pub fn retail_inputs(
    collision: &tags::collision::Bsp,
    retail: &mopp::Code,
) -> Result<Vec<Surface>> {
    let mut s = structure_surfaces(collision);
    let bounds = mopp::bounds::key_bounds(retail).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    for (key, b) in bounds {
        if key >> 29 != STRUCTURE_KEY_TYPE {
            s.push(Surface {
                key,
                vertices: vec![b.min, b.max],
            });
        }
    }
    Ok(s)
}

pub struct Replaced {
    pub map: Vec<u8>,
    pub block: Vec<u8>,
    pub retail_count: u32,
}

/// Rebuild `map` with BSP `index`'s MOPP block replaced by ours, written
/// over the retail block in its BSP region (it must fit), count updated.
pub fn replace(map: &mut super::MapReader, index: usize) -> Result<Replaced> {
    let id: TagId = *super::scenario_bsps(map)?
        .get(index)
        .context("no such bsp")?;
    let tag = map.tag(id)?.clone();
    let loc = tag.location.context("bsp has no payload")?;
    let bytes = map.read_tag_bytes(id)?;
    let view = TagView::new(&bytes, loc.address);
    let h = view.at(0, 0x240)?;
    let count = u32::from_le_bytes(h[0x1e0..0x1e4].try_into()?);
    let addr = u32::from_le_bytes(h[0x1e4..0x1e8].try_into()?);
    let name = map.header().name.clone();
    let level = super::load_level(map, &name, index, id)?;
    let retail = mopp::Code::parse(&level.mopp)?;
    let block = mopp::build(&retail_inputs(&level.collision, &retail)?).to_block();
    if block.len() > count as usize {
        bail!(
            "our block ({} bytes) is larger than retail's ({count})",
            block.len()
        );
    }
    let mut set = TagSet::from_map(map)?;
    let pinned = set.pinned.as_mut().context("no pinned image")?;
    // File offsets of the sbsp header field and of the block.
    let file_of = |a: u32| loc.space.file_offset + u64::from(a - loc.space.virtual_base);
    let at = |a: u32| (file_of(a) - 0x800) as usize;
    let field = at(loc.address) + 0x1e0;
    pinned.file[field..field + 4].copy_from_slice(&(block.len() as u32).to_le_bytes());
    let start = at(addr);
    pinned.file[start..start + count as usize].fill(0);
    pinned.file[start..start + block.len()].copy_from_slice(&block);
    let (out, _) = set.build()?;
    // Read it back: checksum, same tags, and our block where retail's was.
    let mut built = Map::from_reader(Cursor::new(out.clone()))?;
    built.validate_checksum()?;
    if built.tags().len() != map.tags().len() {
        bail!("tag count changed");
    }
    let back = super::load_level(&mut built, &name, index, id)?;
    if back.mopp != block || back.mopp_count != block.len() as u32 {
        bail!("rebuilt map does not hold our block");
    }
    Ok(Replaced {
        map: out,
        block,
        retail_count: count,
    })
}
