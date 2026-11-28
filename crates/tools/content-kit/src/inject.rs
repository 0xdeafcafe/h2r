//! The kit added to a multiplayer map: the map's tags decomposed
//! ([`TagSet::from_map`]), the three looks' tags added ([`kit::install_all`])
//! and the map built again. The same map and kit give the same bytes.
//!
//! A new tag's datum is the builder's: the index slot it takes, salted
//! with the set's base (`(salt_base + slot) << 16 | slot`), which is what
//! [`TagSet::add_tag`] returns and what the game holds the tag by. The
//! three `weap` datums are the map's [`Placed`].
//!
//! The kit's tags take tag memory, which a map type has a fixed amount of
//! (`map::writer::max_metadata_size`). A map without the room is refused,
//! with what the kit takes and what the map has ([`Costs`]).

use std::fmt;
use std::io::Cursor;

use anyhow::{Context, Result, bail, ensure};
use map::build::TagSet;
use map::writer::max_metadata_size;
use map::{CacheType, Map};
use serde::{Deserialize, Serialize};

use crate::kit::{self, Facts};
use crate::placed::Placed;
use crate::{KIT_VERSION, Style};

/// What the kit costs a map.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Costs {
    /// The increase in the map's tag memory (header `+0x1c`), with page
    /// rounding and the index's growth.
    pub tag_growth: u32,
    /// The tag memory the map had free before the kit.
    pub available: u32,
    /// The increase in the file's size: the tags, the atlases and the
    /// models' vertex data.
    pub file_growth: i64,
    /// Tags added.
    pub tags: u32,
}

impl fmt::Display for Costs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} tags; tag memory {} B of {} B free; file {:+} B",
            self.tags, self.tag_growth, self.available, self.file_growth
        )
    }
}

/// A map with the kit in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Added {
    pub bytes: Vec<u8>,
    /// The three looks' `weap` datums.
    pub placed: Placed,
    pub costs: Costs,
}

/// The string a map carrying this kit interns.
pub fn marker_string() -> String {
    format!("pogo_kit_v{KIT_VERSION}")
}

/// What [`add_pogo`] reads from the map, given instead.
#[derive(Clone, Debug, Default)]
pub struct Given {
    /// The weapon class and type, not read from the player graphs.
    pub class: Option<(String, String)>,
    /// The tag memory a multiplayer map may have, not the engine's.
    pub tag_memory: Option<u32>,
}

/// `original` (the multiplayer map `stem`) with the kit added.
pub fn add_pogo(original: &[u8], stem: &str) -> Result<Added> {
    add_pogo_with(original, stem, &Given::default())
}

/// [`add_pogo`] with some of the map's facts given (tests, and tools
/// sizing a map).
pub fn add_pogo_with(original: &[u8], stem: &str, given: &Given) -> Result<Added> {
    let mut map =
        Map::from_reader(Cursor::new(original)).with_context(|| format!("{stem}: not a map"))?;
    let header = map.header().clone();
    if header.cache_type != CacheType::Multiplayer {
        bail!(
            "{stem}: a {:?} map; the pogo kit goes into multiplayer maps",
            header.cache_type
        );
    }
    let facts = match &given.class {
        Some(class) => Facts::read_with_class(&mut map, class.clone()),
        None => Facts::read(&mut map),
    }
    .with_context(|| format!("{stem}: the pogo kit"))?;

    let mut set = TagSet::from_map(&mut map).with_context(|| format!("{stem}: its tags"))?;
    let before = set.slots.len();
    let looks =
        kit::install_all(&mut set, &facts).with_context(|| format!("{stem}: the pogo kit"))?;
    // The version travels in the map.
    set.intern(&marker_string())?;
    // The headroom is checked here, against the layout built, so a
    // refusal says what the kit takes.
    set.check_tag_memory = false;
    let tags = (set.slots.len() - before) as u32;
    let (bytes, layout) = set.build().with_context(|| format!("{stem}: rebuilding"))?;

    let limit = match given.tag_memory {
        Some(n) => n,
        None => max_metadata_size(CacheType::Multiplayer)?,
    };
    let costs = Costs {
        tag_growth: layout
            .memory_size
            .checked_sub(header.meta_size)
            .with_context(|| format!("{stem}: the rebuilt map's tag memory shrank"))?,
        available: limit.saturating_sub(header.meta_size),
        file_growth: bytes.len() as i64 - original.len() as i64,
        tags,
    };
    ensure!(
        costs.tag_growth <= costs.available,
        "{stem}: no room for the pogo kit: {costs}"
    );
    let mut weapons = [0u32; 3];
    for style in Style::ALL {
        weapons[style.index()] = looks[style.index()].weapon.0;
    }
    Ok(Added {
        bytes,
        placed: Placed { weapons },
        costs,
    })
}
