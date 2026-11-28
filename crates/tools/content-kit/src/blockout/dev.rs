//! The dev set: the four role tiles and the font atlas as tags
//! (docs/game/forge-blockout.md, "The dev textures").
//!
//! Each raster of `dev_texture` is a `bitm` ([`crate::blockout::texture`]) and a
//! `shad` on the map's template ([`crate::blockout::Facts::layout`]) with the bitmap
//! as its base map, at `forge\blockout\dev\<family>` and
//! `forge\blockout\dev\font`. The rasters are a pure function of
//! `dev_texture::VERSION`; a process encodes them once ([`encoded`]), and
//! [`encoded_in`] keeps the encoded chains in a cache directory for the
//! next process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use dev_texture::{Family, VERSION};
use forge_shapes::Role;
use h1convert::shad::{self, Bound, Role as MapRole};
use map::build::{TagPayload, TagSet};
use map::{FourCC, TagId};

use crate::blockout::install::{Facts, groups};
use crate::blockout::texture::{self, Encoded};


/// The tile family of a role.
pub fn family(role: Role) -> Family {
    match role {
        Role::Floor => Family::Floor,
        Role::Wall => Family::Wall,
        Role::Ceiling => Family::Ceiling,
        Role::Trim => Family::Trim,
    }
}

/// The tag path of a role's tile.
pub fn tile_path(role: Role) -> String {
    let name = match role {
        Role::Floor => "floor",
        Role::Wall => "wall",
        Role::Ceiling => "ceiling",
        Role::Trim => "trim",
    };
    format!(r"{}\{name}", crate::blockout::data::data().dev_folder)
}

/// The tag path of the font atlas.
pub fn font_path() -> String {
    format!(r"{}\font", crate::blockout::data::data().dev_folder)
}

/// The dev set's encoded rasters: the tiles in role order, then the font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rasters {
    pub tiles: [Encoded; 4],
    pub font: Encoded,
}

impl Rasters {
    /// Draws and encodes every raster.
    pub fn make() -> Result<Rasters> {
        let tiles = [Role::Floor, Role::Wall, Role::Ceiling, Role::Trim]
            .map(|r| texture::encode(&dev_texture::tile(family(r))));
        let [a, b, c, d] = tiles;
        Ok(Rasters {
            tiles: [a?, b?, c?, d?],
            font: texture::encode(&dev_texture::font::font())?,
        })
    }

    fn all(&self) -> impl Iterator<Item = &Encoded> {
        self.tiles.iter().chain(std::iter::once(&self.font))
    }
}

/// The rasters, encoded once a process.
pub fn encoded() -> Result<&'static Rasters> {
    static ONCE: OnceLock<std::result::Result<Rasters, String>> = OnceLock::new();
    ONCE.get_or_init(|| Rasters::make().map_err(|e| format!("{e:#}")))
        .as_ref()
        .map_err(|e| anyhow!("the dev textures: {e}"))
}

/// The cache file's first bytes.
const MAGIC: &[u8; 8] = b"H2BODEV\0";

/// The cache file under `data`: `<data>/cache/forge-blockout/dev-<VERSION>`.
pub fn cache_path(data: &Path) -> PathBuf {
    data.join("cache")
        .join("forge-blockout")
        .join(format!("dev-{VERSION}"))
}

/// The rasters, read from the cache under `data` when it holds this
/// version's, else encoded and written there. A cache that cannot be
/// written is not an error: the rasters are still returned.
pub fn encoded_in(data: &Path) -> Result<Rasters> {
    let path = cache_path(data);
    if let Ok(bytes) = std::fs::read(&path)
        && let Some(r) = read_cache(&bytes)
    {
        return Ok(r);
    }
    let rasters = encoded()?.clone();
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_ok()
    {
        let _ = std::fs::write(&path, write_cache(&rasters));
    }
    Ok(rasters)
}

/// The cache bytes: the magic, the version, then each image's width,
/// height, chain length and chain.
pub fn write_cache(r: &Rasters) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend(VERSION.to_le_bytes());
    for e in r.all() {
        out.extend(e.width.to_le_bytes());
        out.extend(e.height.to_le_bytes());
        out.extend((e.chain.len() as u32).to_le_bytes());
        out.extend(&e.chain);
    }
    out
}

/// The rasters a cache holds, `None` when it is another version's or its
/// sizes are not the rasters' own.
pub fn read_cache(bytes: &[u8]) -> Option<Rasters> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (version, mut rest) = rest.split_at_checked(4)?;
    if u32::from_le_bytes(version.try_into().ok()?) != VERSION {
        return None;
    }
    let word = |rest: &mut &[u8]| -> Option<u32> {
        let (w, tail) = rest.split_at_checked(4)?;
        *rest = tail;
        Some(u32::from_le_bytes(w.try_into().ok()?))
    };
    let sizes = [
        (dev_texture::TILE_SIZE, dev_texture::TILE_SIZE),
        (dev_texture::TILE_SIZE, dev_texture::TILE_SIZE),
        (dev_texture::TILE_SIZE, dev_texture::TILE_SIZE),
        (dev_texture::TILE_SIZE, dev_texture::TILE_SIZE),
        (dev_texture::font::ATLAS_SIZE, dev_texture::font::ATLAS_SIZE),
    ];
    let mut images = Vec::with_capacity(sizes.len());
    for (w, h) in sizes {
        let (width, height, len) = (word(&mut rest)?, word(&mut rest)?, word(&mut rest)? as usize);
        if (width, height) != (w, h) || len != texture::chain_len(w, h).ok()? {
            return None;
        }
        let (chain, tail) = rest.split_at_checked(len)?;
        rest = tail;
        images.push(Encoded {
            width,
            height,
            chain: chain.to_vec(),
        });
    }
    if !rest.is_empty() {
        return None;
    }
    let font = images.pop()?;
    let tiles: [Encoded; 4] = images.try_into().ok()?;
    Some(Rasters { tiles, font })
}

/// The dev set's shaders in a set: one a role's tile, and the font's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevSet {
    pub tiles: [TagId; 4],
    pub font: TagId,
}

impl DevSet {
    pub fn tile(&self, role: Role) -> TagId {
        self.tiles[role.index()]
    }
}

/// The set's dev shaders: found when every one is there, else the missing
/// ones' `bitm` and `shad` added.
pub fn install(set: &mut TagSet, facts: &Facts) -> Result<DevSet> {
    groups(set);
    let rasters = encoded()?;
    let mut one = |path: String, raster: &Encoded| -> Result<TagId> {
        if let Some(id) = set.find(FourCC(*b"shad"), &path) {
            return Ok(id);
        }
        let bitmap = match set.find(FourCC(*b"bitm"), &path) {
            Some(id) => id,
            None => texture::bitmap(raster)?
                .install(set, &path)
                .map_err(|e| anyhow!("{path}.bitm: {e}"))?,
        };
        let maps = HashMap::from([(
            MapRole::Base,
            Bound {
                id: bitmap,
                scale: [1.0, 1.0],
                lod_offset: shad::lod_offset(raster.width as u16, raster.height as u16),
            },
        )]);
        let shader = facts
            .library
            .build(facts.layout, &maps)
            .map_err(|e| anyhow!("{path}.shad: {e}"))?;
        Ok(set.add_tag(FourCC(*b"shad"), &path, TagPayload::Built(shader))?)
    };
    let mut tiles = [TagId::NULL; 4];
    for role in Role::ALL {
        tiles[role.index()] = one(tile_path(role), &rasters.tiles[role.index()])?;
    }
    let font = one(font_path(), &rasters.font)?;
    Ok(DevSet { tiles, font })
}
