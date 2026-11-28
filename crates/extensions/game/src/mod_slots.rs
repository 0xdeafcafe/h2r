//! Access to the multiplayer slots used by the game and Mods previews.
//! Generated offers bring their text and picture in their own patch.lvl.

use std::collections::BTreeMap;

use content::map_list::SLOT_LEN;
use tags::ui_levels::MP_PATH;

use crate::layout_glue::Gm;
use crate::map_select::{Category, category_of};

/// The globals tag (`matg`) of the loaded map and its UI level data.
pub const GLOBALS_TAG: u32 = 0x004e_034c;
const MATG_UI: u32 = 0x178;
/// The UI level element's multiplayer block.
const UI_MP: u32 = 0x10;
/// The loaded cache's header; its word `+0x10` is 2 for the main menu
/// (`0x19c120` reads the content packages only then).
pub const CACHE_HEADER: u32 = 0x004e_0350;
const CACHE_TYPE: u32 = 0x10;
const MAIN_MENU: u16 = 2;
/// 1.0's content scan (`0x19c120`, no arguments) and the multiplayer
/// list's array build (`0x19c7e0`, stdcall `(array)`), as `0x19c190` runs
/// them; the array is `[0x4ee4e4]`.
pub const CONTENT_SCAN: u32 = 0x0019_c120;
pub const MP_ARRAY_BUILD: u32 = 0x0019_c7e0;
pub const MP_ARRAY: u32 = 0x004e_e4e4;

/// The slot's bitmap reference (group, datum).
const BITMAP: usize = 4;
const FREE: u32 = u32::MAX;
/// Slots read at most (1.0's list has 50).
const MAX_SLOTS: u32 = 256;

/// One slot as read: where it is and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub at: u32,
    pub bytes: Vec<u8>,
}

impl Slot {
    fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }
    pub fn id(&self) -> u32 {
        self.u32(0)
    }
    fn bitmap(&self) -> [u32; 2] {
        [self.u32(BITMAP), self.u32(BITMAP + 4)]
    }
    fn path(&self) -> String {
        let p = &self.bytes[MP_PATH..MP_PATH + 0x100];
        let p = &p[..p.iter().position(|&b| b == 0).unwrap_or(p.len())];
        String::from_utf8_lossy(p).into_owned()
    }
    fn file(&self) -> String {
        self.path()
            .rsplit('\\')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    }
}

/// Clear obsolete generated IDs before rescanning, keeping each spare's
/// bitmap reference for the next record. Disc and Map Pack slots never qualify.
pub fn obsolete(slots: &[Slot], current: &crate::map_select::Catalogue) -> Vec<u32> {
    slots
        .iter()
        .filter_map(|slot| {
            let id = slot.id();
            let path = slot.path();
            let offer = path
                .to_ascii_lowercase()
                .strip_prefix("t:\\$c\\")?
                .split('\\')
                .next()?
                .to_owned();
            let offer = u64::from_str_radix(&offer, 16).ok()?;
            if offer >> 32 != u64::from(crate::new_maps::HALO2_TITLE)
                || !content::map_list::is_new_map_offer(offer as u32)
                || current
                    .all()
                    .any(|map| map.id == id && map.path.eq_ignore_ascii_case(&path))
            {
                return None;
            }
            Some(slot.at)
        })
        .collect()
}

pub fn in_main_menu<M: Gm>(m: &M) -> bool {
    m.u32(CACHE_HEADER)
        .filter(|&h| h != 0)
        .and_then(|h| m.u16(h + CACHE_TYPE))
        == Some(MAIN_MENU)
}

fn slots_block<M: Gm>(m: &M) -> Option<(u32, u32)> {
    let matg = m.u32(GLOBALS_TAG).filter(|&a| a != 0 && in_main_menu(m))?;
    let at = matg.checked_add(MATG_UI)?;
    let ui = match (m.u32(at), m.u32(at.checked_add(4)?)) {
        (Some(n), Some(a)) if n >= 1 && a != 0 => a,
        _ => return None,
    };
    let at = ui.checked_add(UI_MP)?;
    let (count, addr) = (m.u32(at)?, m.u32(at.checked_add(4)?)?);
    (count <= MAX_SLOTS && addr != 0).then_some((count, addr))
}

/// Map ids available to the retail hopper validator in the current menu.
pub(crate) fn listed_ids<M: Gm>(m: &M) -> Option<std::collections::BTreeSet<u32>> {
    let (count, base) = slots_block(m)?;
    let mut ids = std::collections::BTreeSet::new();
    for i in 0..count {
        let id = m.u32(base.checked_add(i * SLOT_LEN as u32)?)?;
        if id != FREE {
            ids.insert(id);
        }
    }
    Some(ids)
}

/// A selected level's scenario path in the title's active multiplayer slots.
pub(crate) fn selected_path<M: Gm>(m: &M, id: u32) -> Option<String> {
    if id == FREE {
        return None;
    }
    let (count, addr) = slots_block(m)?;
    let slot = (0..count)
        .filter_map(|i| addr.checked_add(i * SLOT_LEN as u32))
        .find(|at| m.u32(*at) == Some(id))?;
    let mut path = [0u8; 0x100];
    if !m.rd(slot.checked_add(MP_PATH as u32)?, &mut path) {
        return None;
    }
    let len = path.iter().position(|&byte| byte == 0).unwrap_or(path.len());
    (len != 0).then(|| String::from_utf8_lossy(&path[..len]).into_owned())
}

/// The loaded map's multiplayer slots; empty when it has none (not the
/// main menu, no UI level data).
pub fn read_slots<M: Gm>(m: &M) -> Vec<Slot> {
    let Some((count, addr)) = slots_block(m) else {
        return Vec::new();
    };
    let mut all = vec![0u8; count as usize * SLOT_LEN];
    if !m.rd(addr, &mut all) {
        return Vec::new();
    }
    all.chunks(SLOT_LEN)
        .enumerate()
        .map(|(i, b)| Slot {
            at: addr + (i * SLOT_LEN) as u32,
            bytes: b.to_vec(),
        })
        .collect()
}

/// Guest memory, mutably, as [`Gm`].
pub struct WriteGm<'a>(pub &'a mut dyn guest_layout::memory::GuestMemory);

impl Gm for WriteGm<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.write(addr, data).is_ok()
    }
}

/// Each disc map's (then each Map Pack map's) picture as its slot shows
/// it now, by map file name (`lockout`, `triplicate`): the slot's bitmap
/// datum (`[slot + 8]`), which the level select's picture widget draws
/// (`list_class::Rows::picture`). The Mods browser uses the base preview
/// until it has a package image.
pub fn pictures(slots: &[Slot]) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for want in [Category::Halo2, Category::MapPack] {
        for s in slots {
            let datum = s.bitmap()[1];
            if s.id() != FREE && datum != FREE && category_of(&s.path()) == want {
                out.entry(s.file()).or_insert(datum);
            }
        }
    }
    out
}

#[cfg(test)]
pub mod tests;
