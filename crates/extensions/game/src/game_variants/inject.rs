//! The store's variants in the game's variant list
//! (docs/game/community-gametypes.md, "The merged block"):
//!
//! 1. at each load of the main menu's tags, [`entries`] turns each store
//!    variant into a `goof` built-in entry: a name id and a description
//!    number of ours, its list type and the `(setting, value)` pairs of its
//!    record, each one the editor offers (a value the editor does not offer
//!    leaves the variant out, by name). `community_variants` appends them
//!    after the retail and community entries ([`crate::community_variants::extend_with`]);
//! 2. the string lookup serves their names ("<name> U+E007") and
//!    descriptions from a private id range ([`text`], [`FAMILY`]);
//! 3. at `GAME_ACTIVATE`, before the game copies the options, a variant
//!    built from one of our entries gets its bundle's `hx` block written
//!    into the options' record ([`apply_block`]): the pairs carry the
//!    settings, the block the extras and the script reference, so the host
//!    runs the variant's script.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use variants::record::{GameType, GameVariant, MAX_NAME_UNITS, SettingId, all_settings};

use super::store::Store;
use crate::community_variants::{Mem, NewEntry};
use crate::ui_kit::ids::Family;

/// Our string ids: names at `+i`, descriptions at `+0x80 + i`.
pub const FAMILY: Family = crate::ui_kit::ids::family("game variants");
/// The icon after every store variant's name: the download arrow, where
/// retail names carry the padlock (U+E008) and community ones two people
/// (U+E006).
pub const ICON: char = '\u{e007}';
/// The most store variants the list takes (the ids' room; the game's own
/// limit of 101 built-ins is applied when the block is merged).
pub const MAX_LISTED: usize = 0x7f;
/// Description numbers: 94 to 99 name the first six variants' own
/// descriptions; 100 (the game's highest) a shared line for the rest.
/// Retail uses 0 to 38 and the community variants 80 to 93.
pub const DESCRIPTION_BASE: u32 = 94;
pub const DESCRIBED: usize = 6;
pub const SHARED_DESCRIPTION: u32 = 100;
/// The shared line's id.
const SHARED_ID: u32 = FAMILY.base | 0xff;
const SHARED_TEXT: &str = "A game variant from this machine's game-variant store.";

/// One store variant as the last load listed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub display: String,
    pub description: String,
    pub engine: i32,
    /// The record's name field (`+4`, 0x40 bytes) as the loader writes it.
    pub name_field: [u8; 0x40],
    /// The bundle's `hx` block.
    pub block: [u8; variants::ext::LEN],
}

static LISTED: Mutex<Vec<Listed>> = Mutex::new(Vec::new());

fn listed() -> MutexGuard<'static, Vec<Listed>> {
    LISTED.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn is_ours(id: u32) -> bool {
    id & 0xffff_ff00 == FAMILY.base
}
pub fn name_id(i: usize) -> u32 {
    FAMILY.base | i as u32
}
pub fn description_id(i: usize) -> u32 {
    FAMILY.base | 0x80 | i as u32
}

/// The text for one of our ids, from the last listing.
pub fn text(id: u32) -> Option<String> {
    if !is_ours(id) {
        return None;
    }
    if id == SHARED_ID {
        return Some(SHARED_TEXT.into());
    }
    text_in(&listed(), id)
}

fn text_in(l: &[Listed], id: u32) -> Option<String> {
    let v = l.get((id & 0x7f) as usize)?;
    Some(if id & 0x80 == 0 {
        v.display.clone()
    } else if v.description.is_empty() {
        SHARED_TEXT.into()
    } else {
        v.description.clone()
    })
}

/// The id the variant list should show for description number `n`, if it
/// is one of ours.
pub fn description_for(n: u32) -> Option<u32> {
    if n == SHARED_DESCRIPTION {
        return Some(SHARED_ID);
    }
    let i = n.checked_sub(DESCRIPTION_BASE)? as usize;
    (i < DESCRIBED).then(|| description_id(i))
}

fn description_number(i: usize) -> u32 {
    if i < DESCRIBED {
        DESCRIPTION_BASE + i as u32
    } else {
        SHARED_DESCRIPTION
    }
}

/// The name as the list shows it: the name, a space and [`ICON`], in at
/// most the record's 0x1f units.
pub fn display_name(name: &str) -> String {
    let room = MAX_NAME_UNITS - 2;
    let mut units: Vec<u16> = name.trim().encode_utf16().take(room).collect();
    // Never end on half a surrogate pair.
    if units.last().is_some_and(|u| (0xd800..0xdc00).contains(u)) {
        units.pop();
    }
    format!("{} {ICON}", String::from_utf16_lossy(&units))
}

/// The record's name field as `0x149f88` writes it: up to 0x1f units, the
/// rest zero.
pub fn name_field(display: &str) -> [u8; 0x40] {
    let mut out = [0; 0x40];
    for (i, u) in display.encode_utf16().take(MAX_NAME_UNITS).enumerate() {
        out[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
    }
    out
}

/// The record's `(setting, value)` pairs for the built-in loader: every
/// setting of its type the editor offers. `Err` names a value the editor
/// does not offer.
pub fn pairs(v: &GameVariant, offered: &HashMap<u32, Vec<i32>>) -> Result<Vec<(u32, i32)>, String> {
    let ty = v.game_type();
    let mut out = Vec::new();
    for s in all_settings() {
        if !variants::schema::applies(s, ty) {
            continue;
        }
        let Some(values) = offered.get(&s.0) else {
            continue;
        };
        let Some(value) = v.get(SettingId(s.0)) else {
            continue;
        };
        if !values.contains(&value) {
            return Err(format!(
                "setting {} is {value}, which the editor does not offer",
                s.0
            ));
        }
        out.push((s.0, value));
    }
    Ok(out)
}

/// The store's variants as list entries, at most `room`, in store order;
/// the listing the strings and the start hook read becomes these. Each
/// variant left out is named in the lines.
pub fn entries(
    store: &Store,
    offered: &HashMap<u32, Vec<i32>>,
    room: usize,
) -> (Vec<NewEntry>, Vec<String>) {
    let mut out = Vec::new();
    let mut l: Vec<Listed> = Vec::new();
    let mut lines = Vec::new();
    for e in store.entries() {
        let name = &e.bundle.name;
        if out.len() >= room.min(MAX_LISTED) {
            lines.push(format!(
                "game variants: {name} left out: the list is full (101 built-ins)"
            ));
            continue;
        }
        let Ok(v) = GameVariant::from_bytes(&e.record) else {
            continue;
        };
        let ty = v.game_type();
        let Some(list_type) = GameType::ALL.iter().position(|t| *t == ty) else {
            lines.push(format!(
                "game variants: {name} left out: engine {} has no list type",
                ty.engine()
            ));
            continue;
        };
        let pairs = match pairs(&v, offered) {
            Ok(p) => p,
            Err(why) => {
                lines.push(format!("game variants: {name} left out: {why}"));
                continue;
            }
        };
        let mut display = display_name(name);
        let mut n = 2;
        while l
            .iter()
            .any(|x| x.display == display && x.engine == ty.engine())
        {
            display = display_name(&format!("{name} {n}"));
            n += 1;
        }
        let i = out.len();
        out.push(NewEntry {
            name_id: name_id(i),
            list_type: list_type as u32,
            pairs,
            description: description_number(i),
        });
        l.push(Listed {
            name_field: name_field(&display),
            display,
            description: e.bundle.description.clone(),
            engine: ty.engine(),
            block: *v.ext_block(),
        });
    }
    *listed() = l;
    (out, lines)
}

/// The options' record (`options + 0x134`): when it is a variant built
/// from one of our entries (its name field and engine), writes its
/// bundle's block there. Returns the line to log; `None` when the record
/// is not ours or already carries the block.
pub fn apply_block<M: Mem>(m: &mut M, options: u32) -> Option<String> {
    let record = options.wrapping_add(crate::variant_ext::OPTIONS_VARIANT);
    let mut head = [0u8; 0x48];
    if !m.rd(record, &mut head) {
        return None;
    }
    let engine = i32::from_le_bytes(head[0x44..0x48].try_into().unwrap());
    let found = listed()
        .iter()
        .find(|l| l.engine == engine && l.name_field[..] == head[4..0x44])
        .cloned()?;
    let at = record.wrapping_add(variants::ext::AT as u32);
    let mut now = [0u8; variants::ext::LEN];
    if m.rd(at, &mut now) && now == found.block {
        return None;
    }
    if !m.wr(at, &found.block) {
        return Some(format!(
            "game variants: {}: its block could not be written",
            found.display
        ));
    }
    let e = variants::ext::Ext::in_effect(&found.block);
    Some(format!(
        "game variants: {} plays with its file's options: {}",
        found.display,
        e.summary()
    ))
}

/// A script reload's restart names a new script for the running variant
/// (`crate::rules::reload`): the listed entry the record was built from
/// (its name field and engine) takes `block`, so [`apply_block`] at the
/// restart's `GAME_ACTIVATE` writes the new block, not the file's. Until
/// the next list load. Returns whether an entry matched.
pub fn retarget(name_field: &[u8], engine: i32, block: [u8; variants::ext::LEN]) -> bool {
    let mut l = listed();
    let mut hit = false;
    for e in l.iter_mut().filter(|e| e.engine == engine && e.name_field[..] == *name_field) {
        e.block = block;
        hit = true;
    }
    hit
}

/// Sets the listing directly (tests).
#[cfg(test)]
pub fn set_listed(l: Vec<Listed>) {
    *listed() = l;
}
