//! The loaded map's tag paths and the host's tag reads
//! (docs/game/rules-world-api.md, "Advanced data").
//!
//! The game holds no tag paths: its tag entries carry a group, a datum and
//! a data address, and the names are only in the map file. So at
//! `GAME_ACTIVATE` the map's tag index is read through the VFS
//! ([`activate`], as `crate::map_areas::activate` reads the areas record)
//! into a [`TagIndex`]: each tag's path, group and datum. A script's tag
//! table names a path; [`TagIndex::find`] gives its group and datum, the
//! group's typed definition (`tags::defs`) gives a field path its offset
//! ([`super::data::resolve`]), and [`read`] reads the field from the tag's
//! data in guest memory, through the entry the datum names.

use std::collections::BTreeMap;
use std::io::{BufReader, Read, Seek};
use std::sync::{Arc, Mutex};

use guest_layout::tags::{entry_layout, tag_entry_checked};
use guest_layout::{DatumHandle, GuestMemory};
use map::FourCC;
use ::rules::fields::Field;
use tags::defs::Defs;

use crate::bots_nav_auto::VfsFile;

/// A tag path as the index keys it: lower case, `/` between parts, as a
/// script spells it (`rules::weapons` compares the same way).
pub fn canonical(path: &str) -> String {
    path.trim().replace('\\', "/").to_ascii_lowercase()
}

/// The paths of the loaded map's tags, each with its group and full salted
/// datum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagIndex {
    by_path: BTreeMap<String, Vec<(FourCC, u32)>>,
}

impl TagIndex {
    /// The index of `tags`: `(path, group, datum)`. A tag with no path (a
    /// stripped map's) is left out.
    pub fn new(tags: impl IntoIterator<Item = (String, FourCC, u32)>) -> Self {
        let mut by_path: BTreeMap<String, Vec<(FourCC, u32)>> = BTreeMap::new();
        for (path, group, datum) in tags {
            let path = canonical(&path);
            if !path.is_empty() {
                by_path.entry(path).or_default().push((group, datum));
            }
        }
        Self { by_path }
    }

    /// The index of an open map file.
    pub fn of_map<R: Read + Seek>(map: &map::Map<R>) -> Self {
        Self::new(
            map.tags()
                .iter()
                .filter_map(|t| Some((t.name.clone()?, t.group, t.id.0))),
        )
    }

    /// No tag path is known: the map was not read, or holds none.
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// The group and datum of the tag at `path`. A path several groups
    /// share is the object definition's when only one of them is one (a
    /// script's tag is an object); otherwise it is refused.
    pub fn find(&self, path: &str) -> Result<(FourCC, u32), String> {
        let found = self
            .by_path
            .get(&canonical(path))
            .ok_or_else(|| "the map has no such tag".to_string())?;
        match found.as_slice() {
            [one] => Ok(*one),
            many => {
                let mut objects = many.iter().filter(|(g, _)| forge_doc::object_type(&g.0).is_some());
                match (objects.next(), objects.next()) {
                    (Some(one), None) => Ok(*one),
                    _ => Err("the map has several tags of that path".to_string()),
                }
            }
        }
    }

    /// The definition of the group of the tag at `path`, in `defs`.
    pub fn definition<'d>(&self, defs: &'d Defs, path: &str) -> Option<&'d tags::defs::StructDef> {
        defs.group(self.find(path).ok()?.0)
    }
}

static LOADED: Mutex<Option<Arc<TagIndex>>> = Mutex::new(None);

/// The active map's tag index; empty before a map is read.
pub fn current() -> Arc<TagIndex> {
    LOADED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Sets the active map's index ([`activate`], and tests).
pub fn set(index: TagIndex) {
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(index));
}

/// Reads the tag index of the map at guest `path` through the VFS and keeps
/// it for the game that is starting. A map that is not a file the VFS
/// opens, or does not read, leaves the index empty and says why; a script
/// that reads a tag field is then blocked at its start.
pub fn activate(vfs: &Arc<Mutex<vfs::Vfs>>, path: Option<&str>) {
    let index = path
        .and_then(|p| VfsFile::open(Arc::clone(vfs), p).ok())
        .and_then(|f| match map::Map::from_reader(BufReader::with_capacity(64 * 1024, f)) {
            Ok(m) => Some(TagIndex::of_map(&m)),
            Err(e) => {
                eprintln!("rules: the map's tag paths could not be read: {e}");
                None
            }
        })
        .unwrap_or_default();
    set(index);
}

/// The value `tag_int` or `tag_fixed` (`entry`) answers for the field
/// `field` (a path of our names) of the tag at `tag`: the tag's data in
/// guest memory, at the offset the group's definition gives. `None` for a
/// tag or field the map or the definitions lack, a tag the guest has not
/// loaded (the entry's datum or group differs from the index's, or its data
/// address is 0), or a field the entry does not take.
pub fn read(m: &dyn GuestMemory, index: &TagIndex, defs: &Defs, entry: u16, tag: &str, field: &str) -> Option<i32> {
    let (group, datum) = index.find(tag).ok()?;
    let field = super::data::resolve(defs.group(group)?, field).ok()?;
    let at = tag_entry_checked(m, DatumHandle(datum)).ok().flatten()?;
    if m.u32(at.wrapping_add(entry_layout::GROUP)).ok()? != group.as_u32() {
        return None;
    }
    let data = m.u32(at.wrapping_add(entry_layout::ADDRESS)).ok()?;
    if data == 0 {
        return None;
    }
    let bytes = m.bytes(data.checked_add(field.offset)?, field.width()).ok()?;
    Field { offset: 0, kind: field.kind }.read(entry, &bytes)
}

#[cfg(test)]
mod tests {
    use ::rules::caps::id;
    use ::rules::ty::fixed;
    use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, index_layout};
    use guest_layout::testing::FakeGuest;

    use super::*;

    const GROUP: &str = r#"struct vitals 0x8
  0x0 f32 maximum_shield
  0x4 i16 maximum_health
  0x6 u8 grade
  0x7 pad 0x1
group scen 0x10
  0x0 struct vitals vitals
  0x8 u32 count
  0xc unknown 0x4
"#;

    const INDEX: u32 = 0x0060_0000;
    const ENTRIES: u32 = 0x0060_1000;
    const DATA: u32 = 0x0060_2000;
    const BOSS: u32 = 0xe123_0003;

    fn defs() -> Defs {
        let mut d = Defs::default();
        d.parse_into("test", GROUP).unwrap();
        d
    }

    fn index() -> TagIndex {
        TagIndex::new([
            ("Objects\\Boss".to_string(), FourCC(*b"scen"), BOSS),
            ("objects/other".to_string(), FourCC(*b"scen"), 0xe123_0004),
            ("objects/other".to_string(), FourCC(*b"mode"), 0xe123_0005),
            ("objects/dup".to_string(), FourCC(*b"scen"), 0xe123_0006),
            ("objects/dup".to_string(), FourCC(*b"vehi"), 0xe123_0007),
            ("objects/dup".to_string(), FourCC(*b"mode"), 0xe123_0008),
        ])
    }

    /// A guest whose tag entry 3 is the boss's: group `scen`, datum
    /// [`BOSS`], data at [`DATA`].
    fn guest() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(INDEX, 0x100).map(ENTRIES, 0x100).map(DATA, 0x100);
        m.put_u32(TAG_INDEX_GLOBAL, INDEX)
            .put_u32(TAG_ENTRIES_GLOBAL, ENTRIES)
            .put_u32(INDEX + index_layout::TAG_COUNT, 8);
        let at = ENTRIES + 3 * 0x10;
        m.put_u32(at, u32::from_be_bytes(*b"scen"))
            .put_u32(at + 4, BOSS)
            .put_u32(at + 8, DATA);
        m.put_f32(DATA, 0.5)
            .put_u16(DATA + 4, 0xfff6)
            .put_u8(DATA + 6, 7)
            .put_u32(DATA + 8, 90_000);
        m
    }

    #[test]
    fn a_path_is_found_in_either_slash_and_any_case() {
        let i = index();
        assert_eq!(i.find("objects/boss"), Ok((FourCC(*b"scen"), BOSS)));
        assert_eq!(i.find("OBJECTS\\BOSS"), Ok((FourCC(*b"scen"), BOSS)));
        assert!(i.find("objects/nothing").unwrap_err().contains("no such tag"));
        // Only one of the groups is an object: that one.
        assert_eq!(i.find("objects/other"), Ok((FourCC(*b"scen"), 0xe123_0004)));
        // Two are: the script cannot say which.
        assert!(i.find("objects/dup").unwrap_err().contains("several"));
        assert!(TagIndex::default().is_empty());
    }

    #[test]
    fn a_field_reads_from_the_tags_data_through_its_entry() {
        let (m, i, d) = (guest(), index(), defs());
        let read = |e, f| read(&m, &i, &d, e, "objects/boss", f);
        assert_eq!(read(id::TAG_FIXED, "vitals.maximum_shield"), Some(fixed::ONE / 2));
        assert_eq!(read(id::TAG_INT, "vitals.maximum_health"), Some(-10));
        assert_eq!(read(id::TAG_INT, "vitals.grade"), Some(7));
        assert_eq!(read(id::TAG_FIXED, "vitals.grade"), Some(7 << 16));
        assert_eq!(read(id::TAG_INT, "count"), Some(90_000));
        // A float is tag_fixed's only; a field the definitions lack is none.
        assert_eq!(read(id::TAG_INT, "vitals.maximum_shield"), None);
        assert_eq!(read(id::TAG_INT, "vitals.mana"), None);
    }

    #[test]
    fn a_tag_the_guest_has_not_loaded_reads_none() {
        let (i, d) = (index(), defs());
        let get = |m: &FakeGuest| read(m, &i, &d, id::TAG_INT, "objects/boss", "count");
        assert_eq!(get(&guest()), Some(90_000));
        // A stale salt, another group, no data address.
        let mut stale = guest();
        stale.put_u32(ENTRIES + 3 * 0x10 + 4, 0xe124_0003);
        assert_eq!(get(&stale), None);
        let mut other = guest();
        other.put_u32(ENTRIES + 3 * 0x10, u32::from_be_bytes(*b"vehi"));
        assert_eq!(get(&other), None);
        let mut empty = guest();
        empty.put_u32(ENTRIES + 3 * 0x10 + 8, 0);
        assert_eq!(get(&empty), None);
        // No tag index at all.
        assert_eq!(get(&FakeGuest::new()), None);
        // A tag the map lacks, or whose group has no definition.
        assert_eq!(read(&guest(), &i, &d, id::TAG_INT, "objects/none", "count"), None);
        assert_eq!(read(&guest(), &i, &d, id::TAG_INT, "objects/other", "count"), None);
        let models = TagIndex::new([("objects/model".to_string(), FourCC(*b"mode"), BOSS)]);
        assert_eq!(read(&guest(), &models, &d, id::TAG_INT, "objects/model", "count"), None);
    }
}
