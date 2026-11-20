//! One derived mainmenu per data root. Publish map and datum table together.
//! A competing boot may replace the file, but each reader checks one open file.
use super::{Added, Card};
use content::{Hasher, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

// A source map is at most 64 MiB at the VFS rewrite boundary. Larger
// results still work in memory. Bound reads even if a cache file is damaged.
const MAX_PAYLOAD: u64 = 128 << 20;

pub(super) fn key(original: &[u8], cards: &[Card]) -> Sha256 {
    let mut h = Hasher::default();
    h.update(env!("CAMPAIGN_CARDS_SOURCE_HASH").as_bytes());
    let field = |h: &mut Hasher, bytes: &[u8]| {
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    field(&mut h, original);
    for card in cards {
        field(&mut h, card.map.as_bytes());
        field(&mut h, card.name.as_bytes());
        h.update(&card.width.to_le_bytes());
        h.update(&card.height.to_le_bytes());
        field(&mut h, &card.rgba);
    }
    h.finish()
}

fn digest(key: Sha256, payload: &[u8]) -> Sha256 {
    let mut h = Hasher::default();
    h.update(&key.0);
    h.update(payload);
    h.finish()
}

pub(super) fn load(path: &Path, key: Sha256) -> Option<Added> {
    let mut file = File::open(path).ok()?;
    let size = file.metadata().ok()?.len().checked_sub(64)?;
    if !(4..=MAX_PAYLOAD).contains(&size) {
        return None;
    }
    let mut header = [0; 64];
    file.read_exact(&mut header).ok()?;
    if header[..32] != key.0 {
        return None;
    }
    let mut map = vec![0; size as usize];
    file.read_exact(&mut map).ok()?;
    if header[32..] != digest(key, &map).0 {
        return None;
    }
    let end = map.len() - 4;
    let meta_size = u32::from_le_bytes(map[end..].try_into().ok()?);
    let start = end.checked_sub(meta_size as usize)?;
    let (table, framed) = serde_json::from_slice(&map[start..end]).ok()?;
    map.truncate(start);
    Some(Added { map, table, framed })
}

pub(super) fn store(path: &Path, key: Sha256, added: &Added) -> io::Result<()> {
    let meta = serde_json::to_vec(&(&added.table, added.framed))?;
    let size = added.map.len() as u64 + meta.len() as u64 + 4;
    if size > MAX_PAYLOAD {
        return Ok(());
    }
    let mut bytes = Vec::with_capacity(size as usize + 64);
    bytes.extend_from_slice(&key.0);
    bytes.resize(64, 0);
    bytes.extend_from_slice(&added.map);
    bytes.extend_from_slice(&meta);
    bytes.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    let hash = digest(key, &bytes[64..]);
    bytes[32..64].copy_from_slice(&hash.0);
    content::store::write_atomic(path, &bytes)
}
