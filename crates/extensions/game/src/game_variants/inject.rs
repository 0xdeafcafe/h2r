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
//! 3. after the goof loader creates the records, [`restore_listed`] puts
//!    the script index and bundle block on each template. The lobby, pause
//!    menu and saved copies can then resolve its type. [`apply_block`]
//!    restores the active selection's options at `GAME_ACTIVATE`.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use variants::record::{self, GameType, GameVariant, MAX_NAME_UNITS, SettingId, all_settings};

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
/// Retail uses 0 to 38 and the listed scripts from 40
/// (`crate::community_variants::DESCRIPTION_BASE`).
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

/// The block a sim run puts on its game's record ([`set_sim_block`]).
static SIM_BLOCK: Mutex<Option<[u8; variants::ext::LEN]>> = Mutex::new(None);

fn sim_block() -> Option<[u8; variants::ext::LEN]> {
    *SIM_BLOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A sim match that plays a rules script (its `[rules]` table,
/// [`crate::sim_rules::block_for`], docs/runtime/sim.md): [`apply_block`] writes
/// `block`, naming that script, on the game's record at its start. `None`
/// clears it.
pub fn set_sim_block(block: Option<[u8; variants::ext::LEN]>) {
    *SIM_BLOCK.lock().unwrap_or_else(|e| e.into_inner()) = block;
}

fn listed() -> MutexGuard<'static, Vec<Listed>> {
    LISTED.lock().unwrap_or_else(|e| e.into_inner())
}

/// The listing as a machine snapshot keeps it (docs/runtime/snapshots.md,
/// "Feature policies"): the guest's variant list holds our name ids and
/// description numbers, and this listing is what they mean, so it goes
/// with the guest memory that names them. [`set_sim_block`]'s block is
/// the run's setting, as a switch is, and is not saved.
pub fn save_listed() -> Vec<u8> {
    encode_listed(&listed())
}

/// Puts [`save_listed`]'s bytes back. Nothing changes on an error.
pub fn load_listed(bytes: &[u8]) -> Result<(), String> {
    let l = decode_listed(bytes)?;
    *listed() = l;
    Ok(())
}

fn encode_listed(l: &[Listed]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(l.len() as u32).to_le_bytes());
    for v in l {
        for s in [&v.display, &v.description] {
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s.as_bytes());
        }
        out.extend_from_slice(&v.engine.to_le_bytes());
        out.extend_from_slice(&v.name_field);
        out.extend_from_slice(&v.block);
    }
    out
}

fn decode_listed(bytes: &[u8]) -> Result<Vec<Listed>, String> {
    struct Cursor<'a>(&'a [u8]);
    impl<'a> Cursor<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
            if self.0.len() < n {
                return Err("the game-variant listing is cut short".into());
            }
            let (head, rest) = self.0.split_at(n);
            self.0 = rest;
            Ok(head)
        }
        fn u32(&mut self) -> Result<u32, String> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
        fn text(&mut self) -> Result<String, String> {
            let n = self.u32()? as usize;
            String::from_utf8(self.take(n)?.to_vec())
                .map_err(|_| "a game-variant listing name is not UTF-8".to_string())
        }
    }
    let mut c = Cursor(bytes);
    let n = c.u32()? as usize;
    if n > MAX_LISTED {
        return Err(format!(
            "the game-variant listing has {n} entries; the list takes {MAX_LISTED}"
        ));
    }
    let mut l = Vec::with_capacity(n);
    for _ in 0..n {
        let display = c.text()?;
        let description = c.text()?;
        let engine = c.u32()? as i32;
        let name_field: [u8; 0x40] = c.take(0x40)?.try_into().unwrap();
        let block: [u8; variants::ext::LEN] = c.take(variants::ext::LEN)?.try_into().unwrap();
        l.push(Listed {
            display,
            description,
            engine,
            name_field,
            block,
        });
    }
    if !c.0.is_empty() {
        return Err(format!(
            "the game-variant listing has {} bytes past its end",
            c.0.len()
        ));
    }
    Ok(l)
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
    suffixed_name(name, "")
}

fn suffixed_name(name: &str, suffix: &str) -> String {
    let room = MAX_NAME_UNITS - 2 - suffix.len();
    let mut units: Vec<u16> = name.trim().encode_utf16().take(room).collect();
    // Never end on half a surrogate pair.
    if units.last().is_some_and(|u| (0xd800..0xdc00).contains(u)) {
        units.pop();
    }
    format!("{}{suffix} {ICON}", String::from_utf16_lossy(&units))
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
        let Some(list_type) = crate::community_variants::goof_type(ty) else {
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
        while l.iter().any(|x| {
            x.display == display && GameType::from_engine(x.engine).menu_template() == ty.menu_template()
        }) {
            display = suffixed_name(name, &format!(" {n}"));
            n += 1;
        }
        let i = out.len();
        out.push(NewEntry {
            name_id: name_id(i),
            list_type,
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

/// The retail loader builds the goof entry using a retail template. Put the
/// store variant's script reference and record index back before the lobby
/// can copy or edit it. Keep the name and common settings from the loader.
fn restore<'a>(raw: &mut [u8; record::RECORD_SIZE], entries: &'a [Listed]) -> Option<&'a Listed> {
    let engine_bytes = &raw[record::ENGINE_AT..record::ENGINE_AT + 4];
    let engine = i32::from_le_bytes(engine_bytes.try_into().unwrap());
    let entry = entries.iter().find(|entry| {
        let ty = GameType::from_engine(entry.engine);
        entry.name_field[..] == raw[4..record::ENGINE_AT]
            && (entry.engine == engine || ty.menu_template().engine() == engine)
    })?;
    let before = *raw;
    crate::community_variants::finish_record(raw, GameType::from_engine(entry.engine), &entry.block);
    (*raw != before).then_some(entry)
}

pub fn restore_listed(m: &mut dyn guest_layout::GuestMemory, record: u32) -> Result<bool, guest_layout::MemFault> {
    let mut raw = [0; record::RECORD_SIZE];
    m.read(record, &mut raw)?;
    if restore(&mut raw, &listed()).is_none() {
        return Ok(false);
    }
    m.write(record, &raw)?;
    Ok(true)
}

/// The options' record (`options + 0x134`): when it is a variant built
/// from one of our entries (its name field and engine), writes its
/// bundle's block there. Returns the line to log; `None` when the record
/// is not ours or already carries the block.
pub fn apply_block<M: Mem>(m: &mut M, options: u32) -> Option<String> {
    let record = options.wrapping_add(crate::variant_ext::OPTIONS_VARIANT);
    if let Some(block) = sim_block() {
        // A sim's script (`set_sim_block`) replaces the record's block
        // whatever variant built it.
        let at = record.wrapping_add(variants::ext::AT as u32);
        let mut now = [0u8; variants::ext::LEN];
        if m.rd(at, &mut now) && now == block {
            return None;
        }
        return Some(if m.wr(at, &block) {
            format!("game variants: the sim's script is on the record: {}", variants::ext::Ext::in_effect(&block).summary())
        } else {
            "game variants: the sim's script block could not be written".to_string()
        });
    }
    let mut raw = [0; record::RECORD_SIZE];
    if !m.rd(record, &mut raw) {
        return None;
    }
    let entries = listed();
    let found = restore(&mut raw, &entries)?;
    if !m.wr(record, &raw) {
        return Some(format!("game variants: {}: its record could not be written", found.display));
    }
    let e = variants::ext::Ext::in_effect(&found.block);
    Some(format!(
        "game variants: {} plays with its file's options: {}",
        found.display,
        e.summary()
    ))
}

/// The bundle's block [`apply_block`] writes at activation into a record
/// with this name field (`+4`, 0x40 bytes) and engine; `None` when the
/// record is not built from one of our entries. The lobby's start gate
/// reads the script the game will run from it (`crate::rules::gate`).
pub fn bundle_block(name_field: &[u8], engine: i32) -> Option<[u8; variants::ext::LEN]> {
    listed()
        .iter()
        .find(|l| l.engine == engine && l.name_field[..] == *name_field)
        .map(|l| l.block)
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

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    fn sample() -> Vec<Listed> {
        let mut block = [0u8; variants::ext::LEN];
        block[0] = 0x68;
        block[variants::ext::LEN - 1] = 0x78;
        vec![
            Listed {
                display: format!("Bounce {ICON}"),
                description: "Höhe".into(),
                engine: 2,
                name_field: name_field("Bounce"),
                block,
            },
            Listed {
                display: "Plain".into(),
                description: String::new(),
                engine: -1,
                name_field: [0x41; 0x40],
                block: [0; variants::ext::LEN],
            },
        ]
    }

    #[test]
    fn the_listing_survives_a_snapshot() {
        let l = sample();
        assert_eq!(decode_listed(&encode_listed(&l)).unwrap(), l);
        assert_eq!(decode_listed(&encode_listed(&[])).unwrap(), Vec::new());
    }

    #[test]
    fn a_damaged_listing_is_refused_by_name() {
        let bytes = encode_listed(&sample());
        let err = decode_listed(&bytes[..bytes.len() - 1]).unwrap_err();
        assert!(err.contains("cut short"), "{err}");
        let mut long = bytes.clone();
        long.push(0);
        let err = decode_listed(&long).unwrap_err();
        assert!(err.contains("1 bytes past its end"), "{err}");
        let err = decode_listed(&(MAX_LISTED as u32 + 1).to_le_bytes()).unwrap_err();
        assert!(err.contains("entries"), "{err}");
    }

    /// The start hook's feature keeps the listing as its section.
    #[test]
    fn the_start_hook_saves_a_section() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        assert!(r.add(&super::super::StartHook).unwrap());
        let plan = r.build(&hooks::Base).unwrap();
        let policies: Vec<_> = plan
            .snapshot
            .iter()
            .map(|f| (f.id, f.policy.name()))
            .collect();
        assert_eq!(policies, vec![(super::super::FEATURE, "section")]);
        assert!(plan.snapshot_blockers().is_empty());
    }
}
