//! The asset hashes of a base level: what a mod's `was` and `replaces`
//! name (docs/engine/asset-store.md, "Mods and packages").
//!
//! An asset hash is `tags::repo::normal::hash` over a tag's normal form,
//! read for its map. The form depends on the map (its sound gestalt, its
//! low-detail region, the raw files a padded resource is cut by), so a
//! hash is always taken with the base level's tables. [`OfMap`] gives them
//! to a build ([`crate::build_map_settled`]). The store's catalogue
//! (`<store>/assets.idx`, `tags::repo::catalogue`) answers for the base's
//! own tags; a base with no catalogue, and a tag a package wrote
//! ([`Assets::hash_of`]), are read from the map's bytes and given their
//! forms.
use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use content::Sha256;
use map::{FourCC, Map};
use tags::repo::TagSource;
use tags::repo::extract::{MapSource, SharedFiles};
use tags::repo::{Error, LooseTag, TagKey, catalogue, normal};
use tags::schema::Schema;

/// The asset hashes of a base level.
pub trait Assets {
    /// The asset hash of the base's tag `key`: `None` when the base has no
    /// such tag, `Err` saying why it cannot be given its form.
    fn hash(&mut self, key: &TagKey) -> Result<Option<Sha256>, String>;
    /// The asset hash of `tag`, a loose form of what a package wrote, read
    /// for the base level.
    fn hash_of(&mut self, tag: &LooseTag) -> Result<Sha256, String>;
}

/// The assets of one map: its catalogue when its store has one, and its
/// bytes.
pub struct OfMap<'a> {
    catalogue: Option<(catalogue::Catalogue, String)>,
    base: &'a [u8],
    /// What was asked and answered.
    known: BTreeMap<TagKey, Sha256>,
}

impl<'a> OfMap<'a> {
    /// The assets of the store's map `map` (its stem), whose file is
    /// `base`. A catalogue that does not read is none.
    pub fn in_store(store: &Path, map: &str, base: &'a [u8]) -> Self {
        let catalogue = catalogue::read(store)
            .ok()
            .flatten()
            .map(|c| (c, map.to_owned()));
        Self::new(catalogue, base)
    }

    /// The assets of the map whose bytes are `base`, with no catalogue.
    #[allow(clippy::self_named_constructors)] // `OfMap::of_map(base)` reads as the map's own assets.
    pub fn of_map(base: &'a [u8]) -> Self {
        Self::new(None, base)
    }

    fn new(catalogue: Option<(catalogue::Catalogue, String)>, base: &'a [u8]) -> Self {
        Self {
            catalogue,
            base,
            known: BTreeMap::new(),
        }
    }

    /// The map's bytes as the extractor reads them, for one question.
    fn read<T>(
        &mut self,
        ask: impl FnOnce(&mut MapSource<'_, Cursor<&'a [u8]>>) -> Result<T, String>,
    ) -> Result<T, String> {
        let schema = Schema::builtin();
        let mut shared: SharedFiles<Cursor<&'a [u8]>> = SharedFiles::default();
        let map =
            Map::from_reader(Cursor::new(self.base)).map_err(|e| format!("not a map: {e}"))?;
        let mut source = MapSource::new(schema, map, &mut shared).map_err(|e| e.to_string())?;
        ask(&mut source)
    }
}

/// The normal form of `tag` read for the map `source` gives, hashed.
fn form_hash<R: Read + Seek>(
    source: &mut MapSource<'_, R>,
    tag: &LooseTag,
) -> Result<Sha256, String> {
    let schema = Schema::builtin();
    let gestalt = source
        .keys()
        .into_iter()
        .find(|k| k.group == FourCC(*b"ugh!"))
        .and_then(|key| source.read(&key).ok().flatten());
    let low_detail = source.low_detail().unwrap_or_default();
    let mut raw = move |hash: &[u8; 32]| -> tags::repo::Result<Vec<u8>> {
        let (file, offset, len) = source.raw_place(hash).ok_or_else(|| {
            Error::Invalid(format!(
                "raw resource {} was not read from this map",
                tags::repo::hex(hash)
            ))
        })?;
        source.read_raw(file, offset, len)
    };
    let mut tables = normal::Tables {
        gestalt: gestalt.as_ref(),
        low_detail: &low_detail,
        raw: Some(&mut raw),
        schema: Some(schema),
    };
    let form = normal::form(tag, &mut tables).map_err(|e| e.to_string())?;
    Ok(Sha256(normal::hash(&form)))
}

impl Assets for OfMap<'_> {
    fn hash(&mut self, key: &TagKey) -> Result<Option<Sha256>, String> {
        if let Some(h) = self.known.get(key) {
            return Ok(Some(*h));
        }
        let hash = match self
            .catalogue
            .as_ref()
            .and_then(|(c, map)| c.hash_in(map, key))
        {
            Some(h) => h.parse().ok(),
            None => self.read(|source| match source.read(key) {
                Ok(Some(tag)) => form_hash(source, &tag).map(Some),
                Ok(None) => Ok(None),
                Err(e) => Err(e.to_string()),
            })?,
        };
        if let Some(h) = hash {
            self.known.insert(key.clone(), h);
        }
        Ok(hash)
    }

    fn hash_of(&mut self, tag: &LooseTag) -> Result<Sha256, String> {
        self.read(|source| form_hash(source, tag))
    }
}

/// Reborrow an optional asset table for one call: the trait object's
/// lifetime is shortened to the reborrow's, so a loop can lend it again.
pub(crate) fn lend<'r>(assets: &'r mut Option<&mut dyn Assets>) -> Option<&'r mut dyn Assets> {
    match assets {
        Some(a) => Some(&mut **a),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{A, base, cc};

    fn key() -> TagKey {
        TagKey::new(cc(b"jpt!"), A)
    }

    /// The invented map, kept in a store as the extractor leaves it, with
    /// its catalogue.
    #[test]
    fn the_catalogue_and_the_map_give_one_asset_hash() {
        let bytes = base().build().expect("the invented map builds").0;
        let schema = Schema::builtin();
        let root = std::env::temp_dir().join(format!("workspace-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut shared = SharedFiles::default();
        let map = Map::from_reader(Cursor::new(bytes.as_slice())).unwrap();
        let mut source = MapSource::new(schema, map, &mut shared).unwrap();
        let mut repo = tags::repo::TagRepo::create(&root).unwrap();
        tags::repo::extract::store_map(&mut repo, &mut source, "tiny", false).unwrap();
        let (c, _) = catalogue::build(&root, schema).unwrap();
        let listed = c.of_map("tiny");
        assert!(listed.iter().any(|(k, _)| **k == key()), "{c:?}");

        // Each side of the store: the catalogue, and the map's own bytes.
        let mut by_catalogue = OfMap::in_store(&root, "tiny", &bytes);
        let mut by_map = OfMap::of_map(&bytes);
        for (k, h) in listed {
            assert_eq!(by_catalogue.hash(k).unwrap(), h.parse().ok(), "{k}");
            assert_eq!(by_map.hash(k).unwrap(), h.parse().ok(), "{k}");
        }
        // A tag the map lacks; a tag a package wrote is another asset.
        let nowhere = TagKey::new(cc(b"jpt!"), "invented\\dmg\\nowhere");
        assert!(by_map.hash(&nowhere).unwrap().is_none());
        let set = base();
        let id = set.find(cc(b"jpt!"), A).unwrap();
        let map::build::TagPayload::Built(block) = &set.tag(id).unwrap().payload else {
            panic!("built");
        };
        let def = crate::edit::def_of(cc(b"jpt!")).unwrap();
        let mut rec = tags::defs::from_built(def, block).unwrap();
        rec.set("damage_lower_bound", tags::defs::Val::Float(3.0))
            .unwrap();
        let written = tags::defs::encode(def, key(), &rec, tags::repo::Region::Metadata).unwrap();
        let changed = by_map.hash_of(&written).unwrap();
        assert_eq!(changed, by_catalogue.hash_of(&written).unwrap());
        assert_ne!(Some(changed), by_map.hash(&key()).unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}
