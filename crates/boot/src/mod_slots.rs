//! A mod's own name, description and picture on the map it replaces
//! (docs/game/map-select.md, "A mod's name and picture").
//!
//! A package that replaces a disc map (or a Map Pack map) is served in its
//! place, so the map's slot in the main menu's multiplayer list (globals
//! `+0x178`, element 0, block `+0x10`, `0xc64`-byte slots) *is* the mod while
//! the package is chosen. Every screen reads that slot: the lobby's map name
//! (`0x22d5a7`: `0x19c5f0(map id)` then `0x2afd70(slot)`), the Select Map
//! rows (`0x2c9afc`) and the System Link browser's (`0x253569`) through the
//! same name getter, the Select Map description inline (`0x2c9bee`: slot
//! `+0x24c + 0x100 * language`), and Game Setup's and Select Map's picture
//! from the slot's bitmap datum (`[slot + 8]`, `0x2bbcbb`, `0x2c9bd1`). So the
//! slot is refilled:
//!
//! - **Texts.** The names (nine languages of 32 UTF-16 units at `+0xc`) and
//!   descriptions (nine of 128 at `+0x24c`) are written with the package's
//!   (`[list] name.<lang>`, else `[package] name`; the same for the
//!   description), as a `patch.lvl` record would give them.
//! - **Picture.** Only for a package with an image. A retail slot's bitmap
//!   is not the 220x207 32-bit one a `patch.lvl` preview is copied into, so
//!   the slot takes a free slot's bitmap (a free slot, id `-1`, has one of
//!   its own, docs/game/map-pack.md 8.1), and the game's own record applier
//!   (`0x19be00`, stdcall `(content dir, record)`) fills the slot from a
//!   record built from the slot itself plus the package's texts and image:
//!   it keeps the bitmap reference, copies the preview into that bitmap
//!   through the texture lock, and sets the path to `<dir>\<basename>`,
//!   which with the slot's own directory is the path it had.
//! - **Undo.** A slot changed this load whose package is gone gets its
//!   bitmap reference, names and descriptions back.
//!
//! The list lives in the loaded map's tag memory, so this runs after each
//! refresh of the lists (`0x19c190`, at a main menu load) and when the
//! packages changed in this session (`crate::map_select`). The plan is
//! pure ([`plan`]) over the slots' bytes and tested without a guest.

use std::collections::BTreeMap;
use std::path::Path;

use content::MapId;
use content::map_list::{self, Entry, SLOT_LEN};
use tags::ui_levels::{MP_NAME, MP_PATH, MP_SORT};

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
/// 1.0's `patch.lvl` record applier (stdcall `(content dir, record)`,
/// `ret 8`, AL 1 when applied; docs/game/map-pack.md 8.1).
pub const RECORD_APPLY: u32 = 0x0019_be00;
/// 1.0's content scan (`0x19c120`, no arguments) and the multiplayer
/// list's array build (`0x19c7e0`, stdcall `(array)`), as `0x19c190` runs
/// them; the array is `[0x4ee4e4]`.
pub const CONTENT_SCAN: u32 = 0x0019_c120;
pub const MP_ARRAY_BUILD: u32 = 0x0019_c7e0;
pub const MP_ARRAY: u32 = 0x004e_e4e4;

/// The slot's bitmap reference (group, datum).
const BITMAP: usize = 4;
const NAMES_LEN: usize = 9 * 0x40;
const DESCS_LEN: usize = 9 * 0x100;
const FLAGS: usize = 0xc50;
const TEAMS: usize = 0xc54;
const FREE: u32 = u32::MAX;
/// Slots read at most (1.0's list has 50).
const MAX_SLOTS: u32 = 256;

/// What a package that replaces a map shows on that map's slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fill {
    /// The map file it replaces, lower case (`lockout`, `triplicate`).
    pub map: String,
    pub package: MapId,
    pub names: [String; 9],
    pub descriptions: [String; 9],
    /// [`map_list::PREVIEW_LEN`] bytes, from the package's image; `None`
    /// keeps the map's own picture.
    pub preview: Option<Vec<u8>>,
}

impl Fill {
    /// The fill for `p` (a package chosen for `map`): its texts as a
    /// `patch.lvl` record has them, its image as a preview.
    pub fn of(map: &str, p: &content::Package) -> Self {
        let e = Entry::for_manifest(&p.manifest, 0, None, Vec::new());
        let preview = p.manifest.package.image.as_ref().and_then(|_| {
            map_list::preview_for(&p.manifest, &p.dir, None)
                .map_err(|e| eprintln!("mod slots: {}: no picture ({e})", p.manifest.package.name))
                .ok()
        });
        Fill {
            map: map.to_ascii_lowercase(),
            package: p.id,
            names: e.names,
            descriptions: e.descriptions,
            preview,
        }
    }
}

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

/// What this load's slots had before we changed them, and which package's
/// picture is in a slot: forgotten at each refresh of the lists.
#[derive(Debug, Default)]
pub struct State {
    /// Bytes `+4..+0xb4c` (bitmap reference, names, descriptions) of each
    /// slot we changed, by address.
    originals: BTreeMap<u32, Vec<u8>>,
    /// The package whose picture each slot shows, by address.
    pictures: BTreeMap<u32, MapId>,
}

impl State {
    /// The lists were built again: the tag memory is fresh.
    pub fn reset(&mut self) {
        *self = State::default();
    }
}

/// One change to the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Write `bytes` at `at` (texts, or a slot's own bytes put back).
    Write { at: u32, bytes: Vec<u8> },
    /// Point the slot at `slot` to bitmap `bitmap`, then have the game
    /// apply `record` (a v2 `patch.lvl`) with content dir `dir`.
    Record {
        slot: u32,
        bitmap: [u32; 2],
        dir: String,
        record: Vec<u8>,
        package: MapId,
    },
}

/// The slots a fill applies to: used, its map file, and a disc map, or a
/// Map Pack map the disc has no map of the same file name for (as
/// `Catalogue::with_mods` lists them).
fn targets<'a>(slots: &'a [Slot], fill: &'a Fill) -> impl Iterator<Item = &'a Slot> + 'a {
    let on_disc = move |file: &str| {
        slots.iter().any(|s| {
            s.id() != FREE && category_of(&s.path()) == Category::Halo2 && s.file() == file
        })
    };
    slots.iter().filter(move |s| {
        s.id() != FREE
            && s.file() == fill.map
            && match category_of(&s.path()) {
                Category::Halo2 => true,
                Category::MapPack => !on_disc(&fill.map),
                _ => false,
            }
    })
}

fn utf16_field(text: &str, units: usize) -> Vec<u8> {
    let mut out = vec![0u8; units * 2];
    let mut n = 0;
    for c in text.chars() {
        let mut buf = [0u16; 2];
        let enc = c.encode_utf16(&mut buf);
        if n + enc.len() > units - 1 {
            break;
        }
        for u in enc.iter() {
            out[n * 2..n * 2 + 2].copy_from_slice(&u.to_le_bytes());
            n += 1;
        }
    }
    out
}

/// The names and descriptions block (`+0xc..+0xb4c`) for `fill`.
pub fn texts(fill: &Fill) -> Vec<u8> {
    let mut b = Vec::with_capacity(NAMES_LEN + DESCS_LEN);
    for n in &fill.names {
        b.extend(utf16_field(n, 0x20));
    }
    for d in &fill.descriptions {
        b.extend(utf16_field(d, 0x80));
    }
    b
}

/// The v2 record that refills `slot` as itself with `fill`'s texts and
/// `preview`; `None` when the slot's path doesn't fit a record.
pub fn record_for(slot: &Slot, fill: &Fill, preview: &[u8]) -> Option<(String, Vec<u8>)> {
    let path = slot.path();
    let (dir, _) = path.rsplit_once('\\')?;
    let b = &slot.bytes;
    let e = Entry {
        scenario: path.clone(),
        map_id: slot.id(),
        names: fill.names.clone(),
        descriptions: fill.descriptions.clone(),
        sort: slot.u32(MP_SORT) as i32,
        flags: b[FLAGS],
        teams: b[TEAMS..TEAMS + 16].try_into().ok()?,
        preview: preview.to_vec(),
    };
    Some((dir.to_owned(), e.to_v2().ok()?))
}

/// The steps that make `slots` show `fills`, given what this load has
/// already had done (`state`, updated as if the steps ran).
pub fn plan(slots: &[Slot], fills: &[Fill], state: &mut State) -> Vec<Step> {
    let mut steps = Vec::new();
    // Bitmaps of free slots no used slot shows yet.
    let shown: Vec<[u32; 2]> = slots
        .iter()
        .filter(|s| s.id() != FREE)
        .map(Slot::bitmap)
        .collect();
    let mut spare = slots
        .iter()
        .filter(|s| s.id() == FREE && s.bitmap()[1] != FREE && !shown.contains(&s.bitmap()))
        .map(Slot::bitmap)
        .collect::<Vec<_>>()
        .into_iter();
    let mut done: Vec<u32> = Vec::new();
    for fill in fills {
        for slot in targets(slots, fill) {
            if done.contains(&slot.at) {
                continue;
            }
            done.push(slot.at);
            state
                .originals
                .entry(slot.at)
                .or_insert_with(|| slot.bytes[BITMAP..MP_PATH].to_vec());
            let want = texts(fill);
            if let Some(px) = &fill.preview {
                if state.pictures.get(&slot.at) == Some(&fill.package)
                    && slot.bytes[MP_NAME..MP_PATH] == want[..]
                {
                    continue;
                }
                // Its own spare bitmap from an earlier fill this load, or
                // the next one free.
                let original = state.originals[&slot.at][..8].to_vec();
                let mine = (slot.bytes[BITMAP..BITMAP + 8] != original[..]).then(|| slot.bitmap());
                if let Some(bitmap) = mine.or_else(|| spare.next())
                    && let Some((dir, record)) = record_for(slot, fill, px)
                {
                    state.pictures.insert(slot.at, fill.package);
                    steps.push(Step::Record {
                        slot: slot.at,
                        bitmap,
                        dir,
                        record,
                        package: fill.package,
                    });
                    continue;
                }
            }
            // No picture of its own (now): the map's own bitmap back.
            let original = &state.originals[&slot.at][..8];
            if slot.bytes[BITMAP..BITMAP + 8] != *original {
                state.pictures.remove(&slot.at);
                steps.push(Step::Write {
                    at: slot.at + BITMAP as u32,
                    bytes: original.to_vec(),
                });
            }
            if slot.bytes[MP_NAME..MP_PATH] != want[..] {
                steps.push(Step::Write {
                    at: slot.at + MP_NAME as u32,
                    bytes: want,
                });
            }
        }
    }
    // Slots we changed whose package is gone: their own bytes back.
    let gone: Vec<u32> = state
        .originals
        .keys()
        .copied()
        .filter(|a| !done.contains(a))
        .collect();
    for at in gone {
        let bytes = state.originals.remove(&at).unwrap_or_default();
        state.pictures.remove(&at);
        steps.push(Step::Write {
            at: at + BITMAP as u32,
            bytes,
        });
    }
    steps
}

/// The loaded map's multiplayer slots; empty when it has none (not the
/// main menu, no UI level data).
pub fn read_slots<M: Gm>(m: &M) -> Vec<Slot> {
    let ok = m
        .u32(CACHE_HEADER)
        .filter(|&h| h != 0)
        .and_then(|h| m.u16(h + CACHE_TYPE))
        == Some(MAIN_MENU);
    let Some(matg) = m.u32(GLOBALS_TAG).filter(|&a| ok && a != 0) else {
        return Vec::new();
    };
    let ui = match (m.u32(matg + MATG_UI), m.u32(matg + MATG_UI + 4)) {
        (Some(n), Some(a)) if n >= 1 && a != 0 => a,
        _ => return Vec::new(),
    };
    let (Some(count), Some(addr)) = (m.u32(ui + UI_MP), m.u32(ui + UI_MP + 4)) else {
        return Vec::new();
    };
    if count > MAX_SLOTS || addr == 0 {
        return Vec::new();
    }
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
pub struct WriteGm<'a>(pub &'a mut dyn engine::memory::GuestMemory);

impl Gm for WriteGm<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.write(addr, data).is_ok()
    }
}

/// Guest memory of ours for one record and its content dir. Too big for
/// the features' shared arena (`feature::arena`, 512 KiB for every
/// feature), so it is mapped on its own ([`map_scratch`]), above the
/// campaigns' region (`campaigns::Region`, up to `0xa2fc_0000`).
pub const SCRATCH_LEN: u32 = 0x2e000;
const DIR_AT: u32 = 0x2d400;
pub const SCRATCH_FROM: u32 = 0xa2fc_0000;
const SCRATCH_TO: u32 = 0xa300_0000;

/// Maps [`SCRATCH_LEN`] bytes at the first page-aligned place from
/// [`SCRATCH_FROM`] that nothing maps or claims; its address, `None` when
/// there is none (the mods then keep their maps' pictures).
pub fn map_scratch(g: &mut runtime::guest::Guest) -> Option<u32> {
    let mut base = SCRATCH_FROM;
    let taken = |g: &runtime::guest::Guest, base: u32| {
        let hit = |start: u32, len: u32| {
            u64::from(base) < u64::from(start) + u64::from(len)
                && u64::from(base) + u64::from(SCRATCH_LEN) > u64::from(start)
        };
        g.mappings().iter().any(|r| hit(r.virtual_start, r.len))
            || g.virtual_claims()
                .iter()
                .any(|r| hit(r.virtual_start, r.len))
    };
    while taken(g, base) {
        base = base.checked_add(0x1000)?;
        if base + SCRATCH_LEN > SCRATCH_TO {
            return None;
        }
    }
    g.map_zeroed(base, SCRATCH_LEN, runtime::guest::Permissions::RW)
        .ok()?;
    g.claim_virtual(base, SCRATCH_LEN, "mod slots: a patch.lvl record")
        .ok()?;
    Some(base)
}

/// What a refill did, for the console.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub texts: usize,
    pub pictures: usize,
    pub restored: usize,
    pub failed: usize,
}

/// Runs `plan` in the guest: texts written, records applied by the game's
/// own applier with the slot pointed at its spare bitmap first.
pub async fn run(
    cx: &engine::entry::Cx,
    steps: Vec<Step>,
    scratch: u32,
) -> Result<Done, engine::entry::EntryError> {
    let mut done = Done::default();
    for step in steps {
        match step {
            Step::Write { at, bytes } => {
                let ok = cx.write(|m| WriteGm(m).wr(at, &bytes));
                if !ok {
                    done.failed += 1;
                } else if bytes.len() == NAMES_LEN + DESCS_LEN {
                    done.texts += 1;
                } else {
                    done.restored += 1;
                }
            }
            Step::Record {
                slot,
                bitmap,
                dir,
                record,
                package,
            } => {
                let mut d = dir.into_bytes();
                d.push(0);
                let ok = cx.write(|m| {
                    let mut g = WriteGm(m);
                    g.set_u32(slot + BITMAP as u32, bitmap[0])
                        && g.set_u32(slot + BITMAP as u32 + 4, bitmap[1])
                        && g.wr(scratch, &record)
                        && g.wr(scratch + DIR_AT, &d)
                });
                if !ok {
                    done.failed += 1;
                    continue;
                }
                let r = cx
                    .call(
                        engine::entry::GuestCall::new(RECORD_APPLY)
                            .args([scratch + DIR_AT, scratch]),
                    )
                    .await?;
                if r.eax & 0xff == 1 {
                    done.pictures += 1;
                } else {
                    eprintln!("mod slots: the game refused {package}'s record");
                    done.failed += 1;
                }
            }
        }
    }
    Ok(done)
}

/// The packages that replace a disc or Map Pack map in `<data>/maps`, by
/// the vfs's choice rule (`Library::choose`), each as the [`Fill`] its
/// map's slot takes. Derived (new) maps are content packages of their own
/// (`crate::new_maps`) and not here.
pub fn replacing(data_root: &Path) -> Vec<Fill> {
    let maps = crate::map_override::packages_dir(data_root);
    if !maps.is_dir() {
        return Vec::new();
    }
    let (store, _) = crate::setting_defs::load(data_root, &[]);
    let sandbox = crate::setting_defs::halo1_sandbox(&store);
    let mut lib = content::Library::open_with_sandbox(&maps, Some(sandbox));
    let names: std::collections::BTreeSet<String> = lib
        .packages()
        .iter()
        .filter(|p| !p.manifest.base.derived)
        .map(|p| p.manifest.base.map.to_ascii_lowercase())
        .collect();
    let mut out = Vec::new();
    for name in names {
        if let Some(p) = lib.choose(&name).filter(|p| !p.manifest.base.derived) {
            out.push(Fill::of(&name, p));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
