//! Which weapon tags a served map holds: read from the map the game is
//! handed, after the package rewrite has had it, and recorded in
//! [`placed`](crate::placed) for the code that gives a player a stick.
//!
//! The kit's tags come from the `pogo` package (`content/pogo`,
//! `workspace::PackageRewrite`); this reads what that build left. A map
//! without all three weapons is served without the stick and is cleared.

use std::io::{Read, Seek};

use map::{FourCC, Map};

use crate::placed::{self, Placed};
use crate::{NEVER, Style};

/// The three looks' `weap` datums in the map `map` reads, when it holds
/// all of them, with the name its header carries.
pub fn placed_in(map: impl Read + Seek) -> Option<(Placed, String)> {
    let map = Map::from_reader(map).ok()?;
    let mut weapons = [0u32; 3];
    for style in Style::ALL {
        let path = style.tag_path();
        let tag = map.tags().iter().find(|t| {
            t.group == FourCC(*b"weap")
                && t.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&path))
        })?;
        weapons[style.index()] = tag.id.0;
    }
    Some((Placed { weapons }, map.header().name.clone()))
}

/// Records the weapons of the map `stem` as served (`map`), under its
/// file's stem and under the name its header carries (the loadout looks the
/// loaded map up by that; the two are the same on a retail disc), or clears
/// them when it holds no stick.
pub fn publish(stem: &str, map: impl Read + Seek) {
    if NEVER.contains(&stem.to_ascii_lowercase().as_str()) {
        return;
    }
    match placed_in(map) {
        Some((p, name)) => {
            placed::set(stem, p);
            if !name.is_empty() && !name.eq_ignore_ascii_case(stem) {
                placed::set(&name, p);
            }
        }
        None => placed::clear(stem),
    }
}

