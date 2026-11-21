//! Machine-local bot build artifacts. All files here are disposable.
//! A cache hit preserves the existing data-root formats and their stamps.

use std::path::{Path, PathBuf};

use content::{Hasher, Sha256};

use crate::{cache, vis};

use super::{write_atomic, write_bots};

pub fn default_dir() -> Option<PathBuf> {
    let absolute = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if cfg!(target_os = "macos") {
        Some(absolute("HOME")?.join("Library/Caches/h2recomp/bots"))
    } else if cfg!(windows) {
        Some(absolute("LOCALAPPDATA")?.join("h2recomp/cache/bots"))
    } else {
        let base = absolute("XDG_CACHE_HOME").or_else(|| Some(absolute("HOME")?.join(".cache")))?;
        Some(base.join("h2recomp/bots"))
    }
}

pub(super) fn nav_path(dir: &Path, key: Sha256) -> PathBuf {
    dir.join(format!("nav-{}-{}", cache::VERSION, cache::GENERATOR))
        .join(key.to_string())
}

pub(super) fn grid_path(dir: &Path, key: Sha256) -> PathBuf {
    dir.join(format!("vis-{}", vis::VERSION))
        .join(key.to_string())
}

fn checksum(key: Sha256, payload: &[u8]) -> Sha256 {
    let mut hash = Hasher::default();
    hash.update(&key.0);
    hash.update(payload);
    hash.finish()
}

fn read(path: &Path, key: Sha256) -> Option<Vec<u8>> {
    let mut bytes = std::fs::read(path).ok()?;
    let end = bytes.len().checked_sub(32)?;
    if bytes[end..] != checksum(key, &bytes[..end]).0 {
        return None;
    }
    bytes.truncate(end);
    Some(bytes)
}

fn save(path: &Path, key: Sha256, mut bytes: Vec<u8>) -> std::io::Result<()> {
    let digest = checksum(key, &bytes);
    bytes.extend_from_slice(&digest.0);
    std::fs::create_dir_all(path.parent().expect("cache entries have a directory"))?;
    write_atomic(path, &bytes)
}

/// A whole map's BSPs make up one atomic entry. Each original .bots
/// payload is prefixed by its length, in BSP order.
pub(super) fn save_nav(shared: &Path, key: Sha256, stamp: cache::Stamp, bsps: &[cache::MapData]) {
    let mut bundle = Vec::new();
    for bsp in bsps {
        let bytes = bsp.to_bytes(stamp);
        let Ok(len) = u32::try_from(bytes.len()) else {
            return;
        };
        bundle.extend_from_slice(&len.to_le_bytes());
        bundle.extend_from_slice(&bytes);
    }
    let _ = save(&nav_path(shared, key), key, bundle);
}

/// Check the complete bundle before materializing any local files.
pub(super) fn restore_nav(
    shared: &Path,
    key: Sha256,
    dir: &Path,
    map: &str,
    stamp: cache::Stamp,
) -> Option<Vec<vis::GridInput>> {
    let bytes = read(&nav_path(shared, key), key)?;
    let mut payload = bytes.as_slice();
    let mut bsps = Vec::new();
    let mut blobs = Vec::new();
    while !payload.is_empty() {
        let (len, rest) = payload.split_at_checked(4)?;
        let len = u32::from_le_bytes(len.try_into().ok()?) as usize;
        let (blob, rest) = rest.split_at_checked(len)?;
        let (data, built) = cache::MapData::from_bytes(blob)?;
        if built != stamp {
            return None;
        }
        bsps.push(vis::GridInput {
            data,
            stamp,
            digest: Some(Sha256::of(blob)),
        });
        blobs.push(blob);
        payload = rest;
    }
    if bsps.is_empty() {
        return None;
    }
    std::fs::create_dir_all(dir).ok()?;
    write_bots(dir, map, blobs.into_iter()).ok()?;
    Some(bsps)
}

pub(crate) fn restore_grid(
    shared: &Path,
    key: Sha256,
    stamp: cache::Stamp,
    walls: &crate::los::Walls,
) -> Option<(vis::Vis, Vec<u8>)> {
    let bytes = read(&grid_path(shared, key), key)?;
    let (grid, built) = vis::Vis::from_bytes(&bytes).ok()?;
    (stamp == built && grid.fits(walls)).then_some((grid, bytes))
}

pub(crate) fn save_grid(shared: &Path, key: Sha256, stamp: cache::Stamp, grid: &vis::Vis) {
    let _ = save(&grid_path(shared, key), key, grid.to_bytes(stamp));
}
