//! A converted level assembled **from scratch** ([`h1convert::scratch`]):
//! Halo 2's own tags from the retail base map as it is on the disc, by
//! `(group, path)`; the conversion's tags (every tag it built, and every
//! base tag it edited, such as `matg`'s player unit) from the intermediate
//! map the conversion wrote ([`crate::convert`] still converts onto the
//! base); the scenario and BSPs as the conversion built them. Then the
//! simulation table is written for the new index.

use std::io::Cursor;

use h1convert::scratch::{self, LevelSource};
use map::Map;
use map::build::TagSet;
use tags::repo::{MapSource, SharedFiles};

use crate::Result;

pub use h1convert::scratch::{ScratchReport, file_bytes};

/// The level from scratch. `base` is the retail base map's file, `set` the
/// conversion (the intermediate, pinned on the base) and `bytes` its built
/// map; `extra` are tags kept though nothing names them.
pub fn build(
    base: &[u8],
    set: &TagSet,
    bytes: &[u8],
    extra: &[map::TagId],
) -> Result<(TagSet, ScratchReport)> {
    let schema = tags::schema::Schema::builtin();
    let repo = |e: tags::repo::Error| crate::invalid(format!("from scratch: {e}"));
    let mut base_map = Map::from_reader(Cursor::new(base))?;
    let from_level = scratch::changed(&mut base_map, set);
    let low_detail = scratch::low_detail(&mut base_map)?;
    let mut shared_base = SharedFiles::default();
    shared_base.unread_ok = true;
    let mut shared_level = SharedFiles::default();
    shared_level.unread_ok = true;
    let level_map = Map::from_reader(Cursor::new(bytes))?;
    let mut base_src = MapSource::new(schema, base_map, &mut shared_base).map_err(repo)?;
    // The few 05a words the schema takes for pointers.
    base_src.data_words(&h1convert::fresh::DATA_WORDS);
    let mut src = LevelSource {
        base: base_src,
        level: MapSource::new(schema, level_map, &mut shared_level).map_err(repo)?,
        from_level,
    };
    let from_level = src.from_level.clone();
    let (mut out, report) =
        scratch::build(&mut src, set, &crate::KEEP, extra, &from_level, low_detail)?;
    crate::scenario::write_simulation_table(&mut out);
    out.check_tag_memory = false;
    out.check_file_size = false;
    Ok((out, report))
}
