//! Content hashes: of a tag in a [`TagSet`] (an edit's `was`), of a build
//! (the cache key) and of a tag's normal form (the asset hash).
use content::{Hasher, Sha256};
use map::TagId;
use map::build::{Block, FixupKind, RawRef, TagPayload, TagSet};

/// The hash of one tag's content: its group, its bytes and what its fixups
/// say, with every reference by the referenced tag's name (a datum is
/// per-map, a name is not) and every raw resource by its bytes.
///
/// A pinned tag (one a decomposed map keeps where it was) hashes its bytes,
/// so the same tag of the same base file always hashes the same, laid out
/// anew or not being a different thing: take it before the build touches it.
pub fn tag_hash(set: &TagSet, id: TagId) -> Option<Sha256> {
    let entry = set.tag(id)?;
    let mut h = Hasher::default();
    h.update(&entry.group.0);
    match &entry.payload {
        TagPayload::Empty => h.update(b"E"),
        TagPayload::Pinned { address, bytes } => {
            h.update(b"P");
            h.update(&address.to_le_bytes());
            len(&mut h, bytes.len());
            h.update(bytes);
        }
        TagPayload::Built(block) => {
            h.update(b"B");
            block_hash(set, block, &mut h);
        }
        TagPayload::Alias { tag, offset } => {
            h.update(b"A");
            name(set, *tag, &mut h);
            h.update(&offset.to_le_bytes());
        }
    }
    Some(h.finish())
}

/// The asset hash of a tag's normal form (`tags::repo::normal::form`):
/// the hash under which the store's catalogue holds the content a map
/// reads (`tags::repo::catalogue`), and what names the base of a
/// structural patch (docs/engine/asset-store.md, "Normal forms").
///
/// A form is read for one map: make it with the tables that map gives its
/// tags (`tags::repo::normal::Tables`), or a sound, a bitmap or a padded
/// resource hashes as another content than the catalogue's.
pub fn asset_hash(form: &tags::repo::LooseTag) -> Sha256 {
    Sha256(tags::repo::normal::hash(form))
}

fn len(h: &mut Hasher, n: usize) {
    h.update(&(n as u64).to_le_bytes());
}

fn name(set: &TagSet, id: TagId, h: &mut Hasher) {
    if id == TagId::NULL {
        h.update(b"-");
        return;
    }
    match set.slots.get(id.index()).and_then(|s| s.name.as_deref()) {
        Some(n) => {
            len(h, n.len());
            h.update(n.as_bytes());
        }
        None => h.update(&id.0.to_le_bytes()),
    }
    if let Some(t) = set.tag(id) {
        h.update(&t.group.0);
    }
}

fn block_hash(set: &TagSet, b: &Block, h: &mut Hasher) {
    len(h, b.bytes.len());
    h.update(&b.bytes);
    h.update(&b.align.to_le_bytes());
    len(h, b.fixups.len());
    for f in &b.fixups {
        h.update(&f.offset.to_le_bytes());
        match &f.kind {
            FixupKind::Block { count, child } => {
                h.update(b"k");
                h.update(&count.to_le_bytes());
                block_hash(set, child, h);
            }
            FixupKind::Data(child) => {
                h.update(b"d");
                block_hash(set, child, h);
            }
            FixupKind::Pointer(child) => {
                h.update(b"p");
                block_hash(set, child, h);
            }
            FixupKind::TagRef(id) => {
                h.update(b"r");
                name(set, *id, h);
            }
            FixupKind::TagId(id) => {
                h.update(b"i");
                name(set, *id, h);
            }
            FixupKind::StringId(s) => {
                h.update(b"s");
                len(h, s.len());
                h.update(s.as_bytes());
            }
            FixupKind::Raw(r) => {
                h.update(b"w");
                match r {
                    RawRef::Local(id) => match set.raw.get(id.0) {
                        Some(blob) => {
                            h.update(b"l");
                            len(h, blob.bytes.len());
                            h.update(&blob.bytes);
                        }
                        None => h.update(b"?"),
                    },
                    RawRef::External { file, offset } => {
                        h.update(b"x");
                        h.update(format!("{file:?}").as_bytes());
                        h.update(&offset.to_le_bytes());
                    }
                    RawRef::Null => h.update(b"n"),
                }
            }
            FixupKind::Bsp { section, field } => {
                h.update(b"b");
                h.update(&(*section as u64).to_le_bytes());
                h.update(format!("{field:?}").as_bytes());
            }
            FixupKind::TagOffset { count, tag, offset } => {
                h.update(b"o");
                h.update(&count.to_le_bytes());
                name(set, *tag, h);
                h.update(&offset.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::repo::{Body, LooseTag, Node, TagKey, Tree};

    fn tag(path: &str, word: u32) -> LooseTag {
        LooseTag {
            key: TagKey::new(map::FourCC(*b"jpt!"), path),
            body: Body::Tree(Tree {
                nodes: vec![Node {
                    bytes: word.to_le_bytes().to_vec(),
                    count: 1,
                    size: 4,
                    align: 4,
                    fields: Vec::new(),
                }],
                ..Default::default()
            }),
        }
    }

    fn form(path: &str, word: u32) -> LooseTag {
        tags::repo::normal::form(&tag(path, word), &mut tags::repo::normal::Tables::none()).unwrap()
    }

    #[test]
    fn the_asset_hash_is_of_the_content_not_the_key() {
        let a = form("invented\\a", 1);
        assert_eq!(asset_hash(&a), asset_hash(&form("invented\\a", 1)));
        // The same content under another key is the same asset.
        assert_eq!(asset_hash(&a), asset_hash(&form("invented\\b", 1)));
        // Another content is another asset, as is another group.
        assert_ne!(asset_hash(&a), asset_hash(&form("invented\\a", 2)));
        let other = LooseTag {
            key: TagKey::new(map::FourCC(*b"jpt "), "invented\\a"),
            ..tag("invented\\a", 1)
        };
        let mut tables = tags::repo::normal::Tables::none();
        let other = tags::repo::normal::form(&other, &mut tables).unwrap();
        assert_ne!(asset_hash(&a), asset_hash(&other));
        let h = asset_hash(&a);
        assert_eq!(h.to_string().len(), 64);
        assert_eq!(
            h.to_string(),
            tags::repo::hex(&tags::repo::normal::hash(&a))
        );
    }
}
