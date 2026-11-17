//! More than one campaign in the menus (docs/game/multiple-campaigns.md).
//!
//! 1.0's campaign data is already keyed by a campaign id, but its menus use
//! campaign 1 only: the level select's list is built for campaign 1
//! (`0x19c190` calls `0x19c6d0(list, 1)`), and every writer of the menu's
//! campaign selection (`0x54e7c0`) writes 1. Profiles keep map ids, not
//! campaign ids. So the data path is ours and small, and a few hooks
//! carry the campaign through:
//!
//! - **Lists.** Each installed campaign's levels go into the globals tag's
//!   two level lists ([`extend`]): first into the lists' own free slots,
//!   then, when they run out, into copies of the lists in our memory. Done
//!   the first time a reader runs after a map load (the lists live in the
//!   loaded map's tag memory).
//! - **Which campaign the level select shows.** Our picker screen sets it
//!   ([`Selection`]); the list build gets that id in place of its 1, and
//!   the level select's constructor leaves it in `0x54e7c0`.
//! - **Starting a level.** The menu's start (`0x23643f`) looks the level up
//!   by `(0x54e7c0, 0x54e7c4)`; before it runs, `0x54e7c0` is set to the
//!   campaign the selected map id belongs to.
//! - **Progress.** A profile's progress is the highest map id finished per
//!   difficulty, compared across campaigns, so another campaign's ids (all
//!   above Halo 2's) would unlock all of Halo 2. Another campaign's
//!   progress and last level are kept here instead ([`Progress`],
//!   `<data>/campaigns/progress.toml`) and the profile is left as retail.
//!
//! With no campaign installed nothing is hooked and nothing is written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use content::campaign_list::{
    self as cl, Built, CampaignManifest, FREE, START_LEN, UI_CAMPAIGN_LEN, UI_LEVEL_LEN,
};
use serde::{Deserialize, Serialize};

use crate::layout_glue::Gm;

/// The feature's id on the plan.
pub const FEATURE: &str = "campaigns";

/// The globals tag (`matg`) of the loaded map.
pub const GLOBALS_TAG: u32 = 0x004e_034c;
/// `matg +0x170`: one element whose block `+0` is the start-level list.
pub const MATG_START: u32 = 0x170;
/// `matg +0x178`: one element (`0x18` bytes) whose blocks `+0` and `+8`
/// are the level select's campaigns and levels.
pub const MATG_UI: u32 = 0x178;
pub const UI_CAMPAIGNS: u32 = 0;
pub const UI_LEVELS: u32 = 8;
/// The menu's selection: campaign id, map id, difficulty.
pub const SELECTED_CAMPAIGN: u32 = 0x0054_e7c0;
pub const SELECTED_MAP: u32 = 0x0054_e7c4;
/// The level select's data array of map ids (`0x19c670` returns it).
pub const LEVEL_ARRAY: u32 = 0x004e_e4e8;
/// Halo 2's own campaign.
pub const RETAIL_CAMPAIGN: u32 = 1;
/// Most elements a list we copy may have.
const MAX_ELEMENTS: u32 = 256;

/// The campaigns installed under `<data>/campaigns`: every
/// `<key>/campaign.toml` that parses, with the levels whose map exists
/// (`<key>/<map>.map`, or the disc's own `maps\<map>.map`). A campaign
/// left with no level is not installed. Lines say what was skipped.
pub fn installed(
    data_root: &Path,
    disc_has: &dyn Fn(&str) -> bool,
) -> (Vec<CampaignManifest>, Vec<String>) {
    let mut out = Vec::new();
    let mut lines = Vec::new();
    let root = data_root.join("campaigns");
    let Ok(dirs) = std::fs::read_dir(&root) else {
        return (out, lines);
    };
    let mut dirs: Vec<PathBuf> = dirs
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let file = dir.join(cl::MANIFEST_FILE);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut m = match CampaignManifest::parse(&text) {
            Ok(m) => m,
            Err(e) => {
                lines.push(format!("campaigns: {} skipped: {e}", file.display()));
                continue;
            }
        };
        if dir.file_name().and_then(|n| n.to_str()) != Some(m.campaign.key.as_str()) {
            lines.push(format!(
                "campaigns: {} skipped: its key is {:?}, not its folder's name",
                file.display(),
                m.campaign.key
            ));
            continue;
        }
        let before = m.levels.len();
        m.levels
            .retain(|l| dir.join(format!("{}.map", l.map)).is_file() || disc_has(&l.map));
        if m.levels.len() < before {
            lines.push(format!(
                "campaigns: {}: {} of {before} levels have no map yet; listing the rest",
                m.campaign.key,
                before - m.levels.len()
            ));
        }
        if m.levels.is_empty() {
            lines.push(format!(
                "campaigns: {}: no level has a map; not listed",
                m.campaign.key
            ));
            continue;
        }
        out.push(m);
    }
    (out, lines)
}

// ---------------------------------------------------------------------------
// The lists.

/// A block's `(count, address)` and its element size.
#[derive(Clone, Copy, Debug)]
struct Block {
    /// Where the `(count, address)` pair is.
    at: u32,
    count: u32,
    addr: u32,
    len: u32,
}

fn block<M: Gm>(m: &M, at: u32, len: u32) -> Option<Block> {
    let count = m.u32(at)?;
    let addr = m.u32(at + 4)?;
    (count <= MAX_ELEMENTS && (count == 0 || addr != 0)).then_some(Block {
        at,
        count,
        addr,
        len,
    })
}

fn elements<M: Gm>(m: &M, b: &Block) -> Option<Vec<Vec<u8>>> {
    let mut all = vec![0u8; (b.count * b.len) as usize];
    if !m.rd(b.addr, &mut all) {
        return None;
    }
    Some(all.chunks(b.len as usize).map(<[u8]>::to_vec).collect())
}

/// Element 0's address of the one-element block at `matg + at`.
fn element0<M: Gm>(m: &M, matg: u32, at: u32) -> Option<u32> {
    let n = m.u32(matg + at)?;
    let addr = m.u32(matg + at + 4)?;
    (n >= 1 && addr != 0).then_some(addr)
}

/// Guest memory of ours the lists may be copied into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub base: u32,
    pub len: u32,
}

impl Region {
    /// Room for three copied lists of [`MAX_COPIED`] elements each.
    pub const LEN: u32 = 0x6_0000;
    /// Ours, below the level boot's buffers' neighbours: `0xa2f6_0000` to
    /// `0xa2fc_0000` (the layouts use `0xa2f0_0000`-`0xa2f2_1000`, the level
    /// boot `0xa2f4_0000`-`0xa2f4_3000`).
    pub const BASE: u32 = 0xa2f6_0000;
    pub const OURS: Region = Region {
        base: Self::BASE,
        len: Self::LEN,
    };
}

/// The most elements a list we copy may have.
pub const MAX_COPIED: u32 = 64;
const _: () = assert!(
    MAX_COPIED * (START_LEN as u32 + UI_CAMPAIGN_LEN as u32 + UI_LEVEL_LEN as u32) + 0x30
        <= Region::LEN
);

/// What [`extend`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Extended {
    /// No globals tag, or it has no campaign list.
    pub no_lists: bool,
    /// The lists already hold our campaigns.
    pub already: bool,
    /// Records written into free slots, and appended in copies, per list
    /// (start, campaigns, levels).
    pub in_place: [u32; 3],
    pub appended: [u32; 3],
}

impl Extended {
    pub fn line(&self) -> String {
        if self.no_lists {
            return "campaigns: the loaded map has no campaign level list".into();
        }
        if self.already {
            return "campaigns: the level lists already hold our campaigns".into();
        }
        format!(
            "campaigns: level lists extended: start {}+{}, campaigns {}+{}, levels {}+{} (free slots + copied)",
            self.in_place[0],
            self.appended[0],
            self.in_place[1],
            self.appended[1],
            self.in_place[2],
            self.appended[2]
        )
    }
}

/// One list's new contents: records go into free slots first, then after
/// the end. `free` says whether an element is a free slot. Returns the
/// list and how many went in place.
fn merge(
    mut old: Vec<Vec<u8>>,
    new: Vec<Vec<u8>>,
    free: impl Fn(&[u8]) -> bool,
    keep: impl Fn(&[u8], &mut Vec<u8>),
) -> (Vec<Vec<u8>>, u32, u32) {
    let (mut placed, mut appended) = (0, 0);
    let mut slots = (0..old.len())
        .filter(|&i| free(&old[i]))
        .collect::<Vec<_>>()
        .into_iter();
    for mut r in new {
        match slots.next() {
            Some(i) => {
                keep(&old[i], &mut r);
                old[i] = r;
                placed += 1;
            }
            None => {
                old.push(r);
                appended += 1;
            }
        }
    }
    (old, placed, appended)
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// Writes `list` over block `b`: in place when it is no longer, else into
/// our region at `*next` and the block repointed.
fn store<M: Gm>(
    m: &mut M,
    b: &Block,
    list: &[Vec<u8>],
    region: &Region,
    next: &mut u32,
) -> Result<(), String> {
    let bytes: Vec<u8> = list.concat();
    if list.len() as u32 > MAX_COPIED {
        return Err(format!(
            "a list of {} is more than {MAX_COPIED}",
            list.len()
        ));
    }
    if list.len() as u32 == b.count {
        return m
            .wr(b.addr, &bytes)
            .then_some(())
            .ok_or("a list is not writable".into());
    }
    let at = (*next + 0xf) & !0xf;
    if at + bytes.len() as u32 > region.base + region.len {
        return Err("our list memory is full".into());
    }
    if !m.wr(at, &bytes) || !m.set_u32(b.at + 4, at) || !m.set_u32(b.at, list.len() as u32) {
        return Err("our list memory is not writable".into());
    }
    *next = at + bytes.len() as u32;
    Ok(())
}

/// Puts `built`'s campaigns into the loaded map's level lists (see the
/// module docs). Idempotent: lists that already hold our first campaign's
/// id are left alone. A level's picture is its `picture` level's, found by
/// map name through the start list; else the free slot's own; else the
/// list's first.
pub fn extend<M: Gm>(m: &mut M, built: &[Built], region: &Region) -> Result<Extended, String> {
    let mut r = Extended::default();
    let Some(matg) = m.u32(GLOBALS_TAG).filter(|&a| a != 0) else {
        r.no_lists = true;
        return Ok(r);
    };
    let Some(first) = built.first() else {
        r.already = true;
        return Ok(r);
    };
    let Some(start) = element0(m, matg, MATG_START).and_then(|e| block(m, e, START_LEN as u32))
    else {
        r.no_lists = true;
        return Ok(r);
    };
    let starts = elements(m, &start).ok_or("the start list is unreadable")?;
    if starts.iter().any(|e| u32_at(e, 0) == first.campaign_id) {
        r.already = true;
        return Ok(r);
    }
    // Map name -> map id, from the retail start records.
    let retail: BTreeMap<String, u32> = starts
        .iter()
        .filter_map(|e| cl::read_start(e))
        .filter(|(c, _, _)| *c != FREE)
        .filter_map(|(_, id, p)| Some((p.rsplit('\\').next()?.to_ascii_lowercase(), id)))
        .collect();
    let mut next = region.base;
    let new: Vec<Vec<u8>> = built
        .iter()
        .flat_map(|b| {
            b.levels
                .iter()
                .map(move |l| cl::start_record(b.campaign_id, l).to_vec())
        })
        .collect();
    let free = |e: &[u8]| u32_at(e, 0) == FREE && u32_at(e, 4) == FREE;
    let (list, placed, appended) = merge(starts, new, free, |_, _| {});
    store(m, &start, &list, region, &mut next)?;
    (r.in_place[0], r.appended[0]) = (placed, appended);

    // The level select's lists, when this map has them.
    let Some(ui) = element0(m, matg, MATG_UI) else {
        return Ok(r);
    };
    if let Some(b) = block(m, ui + UI_CAMPAIGNS, UI_CAMPAIGN_LEN as u32) {
        let old = elements(m, &b).ok_or("the campaign list is unreadable")?;
        let new = built.iter().map(cl::ui_campaign_record).collect();
        let (list, placed, appended) = merge(old, new, |e| u32_at(e, 0) == FREE, |_, _| {});
        store(m, &b, &list, region, &mut next)?;
        (r.in_place[1], r.appended[1]) = (placed, appended);
    }
    if let Some(b) = block(m, ui + UI_LEVELS, UI_LEVEL_LEN as u32) {
        let old = elements(m, &b).ok_or("the level select's list is unreadable")?;
        let picture_of = |id: u32| {
            old.iter().find(|e| u32_at(e, 4) == id).map(|e| {
                [
                    u32_at(e, cl::UI_LEVEL_PICTURE),
                    u32_at(e, cl::UI_LEVEL_PICTURE + 4),
                ]
            })
        };
        let fallback = old.first().map_or([0, FREE], |e| {
            [
                u32_at(e, cl::UI_LEVEL_PICTURE),
                u32_at(e, cl::UI_LEVEL_PICTURE + 4),
            ]
        });
        // A level with a picture of its own choosing carries it; others
        // take the free slot's (marked by FREE here).
        let new: Vec<Vec<u8>> = built
            .iter()
            .flat_map(|b| {
                b.levels.iter().map(|l| {
                    let own = l
                        .picture
                        .as_ref()
                        .and_then(|p| retail.get(p))
                        .and_then(|&id| picture_of(id));
                    cl::ui_level_record(b.campaign_id, l, own.unwrap_or([FREE, FREE]))
                })
            })
            .collect();
        let at = cl::UI_LEVEL_PICTURE;
        let (list, placed, appended) = merge(old.clone(), new, free, |slot, rec| {
            if u32_at(rec, at) == FREE {
                rec[at..at + 8].copy_from_slice(&slot[at..at + 8]);
            }
        });
        let list: Vec<Vec<u8>> = list
            .into_iter()
            .map(|mut e| {
                if u32_at(&e, at) == FREE {
                    e[at..at + 4].copy_from_slice(&fallback[0].to_le_bytes());
                    e[at + 4..at + 8].copy_from_slice(&fallback[1].to_le_bytes());
                }
                e
            })
            .collect();
        store(m, &b, &list, region, &mut next)?;
        (r.in_place[2], r.appended[2]) = (placed, appended);
    }
    Ok(r)
}

/// The campaign id a map id belongs to, among `built`.
pub fn campaign_of(built: &[Built], map_id: u32) -> Option<&Built> {
    built
        .iter()
        .find(|b| b.levels.iter().any(|l| l.map_id == map_id))
}

// ---------------------------------------------------------------------------
// Which campaign the menu shows.

/// The campaign the level select shows (1 until the picker says
/// otherwise), and whether the next level-select request is ours to pass.
#[derive(Debug)]
pub struct Selection {
    campaign: AtomicU32,
    pass: AtomicBool,
    building: AtomicBool,
}

impl Default for Selection {
    fn default() -> Self {
        Selection {
            campaign: AtomicU32::new(RETAIL_CAMPAIGN),
            pass: AtomicBool::new(false),
            building: AtomicBool::new(false),
        }
    }
}

impl Selection {
    pub fn campaign(&self) -> u32 {
        self.campaign.load(Ordering::SeqCst)
    }
    pub fn set(&self, id: u32) {
        self.campaign.store(id, Ordering::SeqCst);
    }
    /// The picker is about to open the level select itself: let that one
    /// request through.
    pub fn let_through(&self) {
        self.pass.store(true, Ordering::SeqCst);
    }
    /// Whether this request is the one let through (and forget it).
    pub fn take_pass(&self) -> bool {
        self.pass.swap(false, Ordering::SeqCst)
    }
    pub fn set_building(&self, on: bool) {
        self.building.store(on, Ordering::SeqCst);
    }
    pub fn building(&self) -> bool {
        self.building.load(Ordering::SeqCst)
    }
    /// Why a snapshot can't be taken now: the level select is being built,
    /// or a level-select request is to be let through. The campaign shown
    /// is menu state only; in a game, the level's campaign comes from the
    /// map (`State::lookup_campaign`, `fix_selection`).
    pub fn snapshot_live(&self) -> Option<String> {
        if self.building() {
            return Some("the level select is being built".into());
        }
        self.pass
            .load(Ordering::SeqCst)
            .then(|| "a level-select request is to be let through".into())
    }
}

// ---------------------------------------------------------------------------
// Progress.

/// Another campaign's progress for one profile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignProgress {
    /// The highest map id finished, per difficulty (easy .. legendary);
    /// `-1` none.
    pub finished: [i64; 4],
    /// The last level started, `-1` none.
    pub last: i64,
}

impl CampaignProgress {
    fn fresh() -> Self {
        CampaignProgress {
            finished: [-1; 4],
            last: -1,
        }
    }
}

/// `progress.toml`: `[<profile>.<campaign key>]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProgressFile(pub BTreeMap<String, BTreeMap<String, CampaignProgress>>);

/// The progress of every campaign past Halo 2's, per profile, in
/// `<data>/campaigns/progress.toml` (written whole, through a rename).
pub struct Progress {
    path: Option<PathBuf>,
    file: Mutex<ProgressFile>,
}

pub const PROGRESS_FILE: &str = "progress.toml";

impl Progress {
    /// Reads `path` (a missing or unreadable file is empty; an unreadable
    /// one is never overwritten).
    pub fn load(path: PathBuf) -> (Self, Option<String>) {
        let (file, why, keep) = match std::fs::read_to_string(&path) {
            Ok(t) => match toml::from_str(&t) {
                Ok(f) => (f, None, true),
                Err(e) => (
                    ProgressFile::default(),
                    Some(format!(
                        "campaigns: {} unreadable ({e}); progress not kept this run",
                        path.display()
                    )),
                    false,
                ),
            },
            Err(_) => (ProgressFile::default(), None, true),
        };
        (
            Progress {
                path: keep.then_some(path),
                file: Mutex::new(file),
            },
            why,
        )
    }

    /// In memory only (tests, headless).
    pub fn memory() -> Self {
        Progress {
            path: None,
            file: Mutex::new(ProgressFile::default()),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut ProgressFile) -> (R, bool)) -> R {
        let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let (r, changed) = f(&mut file);
        if changed
            && let Some(path) = &self.path
            && let Err(e) = write(path, &file)
        {
            eprintln!("campaigns: could not write {}: {e}", path.display());
        }
        r
    }

    /// Level `map_id` finished on `difficulty` by `profile`: raises that
    /// difficulty's progress. Returns whether it changed.
    pub fn finished(&self, profile: &str, key: &str, difficulty: u32, map_id: u32) -> bool {
        let d = difficulty.min(3) as usize;
        self.with(|f| {
            let p =
                f.0.entry(profile.into())
                    .or_default()
                    .entry(key.into())
                    .or_insert_with(CampaignProgress::fresh);
            let changed = i64::from(map_id) > p.finished[d];
            if changed {
                p.finished[d] = map_id.into();
            }
            (changed, changed)
        })
    }

    /// Level `map_id` started by `profile`.
    pub fn started(&self, profile: &str, key: &str, map_id: u32) {
        self.with(|f| {
            let p =
                f.0.entry(profile.into())
                    .or_default()
                    .entry(key.into())
                    .or_insert_with(CampaignProgress::fresh);
            let changed = p.last != i64::from(map_id);
            p.last = map_id.into();
            ((), changed)
        })
    }

    /// The highest map id any of `profiles` finished in `key` on any
    /// difficulty, as the game's own `0x190565` answers for Halo 2; `-1`
    /// for none, which shows the first level only.
    pub fn max(&self, profiles: &[String], key: &str) -> i32 {
        self.with(|f| {
            let m = profiles
                .iter()
                .filter_map(|p| f.0.get(p)?.get(key))
                .flat_map(|c| c.finished)
                .max()
                .unwrap_or(-1);
            (m.clamp(-1, i64::from(i32::MAX)) as i32, false)
        })
    }

    pub fn get(&self, profile: &str, key: &str) -> Option<CampaignProgress> {
        self.with(|f| (f.0.get(profile).and_then(|p| p.get(key)).cloned(), false))
    }
}

fn write(path: &Path, file: &ProgressFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string(file).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

// ---------------------------------------------------------------------------
// The picker's rows.

/// The picker: the installed campaigns whose `order` is below
/// [`cl::HALO2_ORDER`] (Halo 1), then Halo 2, then the rest, each group in
/// the campaigns' own order. Row `n` names its campaign id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub rows: Vec<(u32, String)>,
}

impl Picker {
    pub fn new(built: &[Built]) -> Self {
        let above = |b: &&Built| b.order.is_some_and(|o| o < cl::HALO2_ORDER);
        let row = |b: &Built| (b.campaign_id, b.names[0].clone());
        let mut rows: Vec<(u32, String)> = built.iter().filter(above).map(row).collect();
        rows.push((RETAIL_CAMPAIGN, "Halo 2".to_owned()));
        rows.extend(built.iter().filter(|b| !above(b)).map(row));
        Picker { rows }
    }
    /// The campaign row `row` picks.
    pub fn campaign(&self, row: i16) -> Option<u32> {
        usize::try_from(row)
            .ok()
            .and_then(|r| self.rows.get(r))
            .map(|r| r.0)
    }
}

/// Everything the hooks share.
pub struct State {
    pub built: Vec<Built>,
    pub selection: Selection,
    pub progress: Progress,
}

impl State {
    /// The key of the campaign a map id belongs to, when it is one of ours.
    pub fn key_of(&self, map_id: u32) -> Option<&str> {
        campaign_of(&self.built, map_id).map(|b| b.key.as_str())
    }
    /// The key of campaign id `id`, when it is one of ours.
    pub fn key_of_campaign(&self, id: u32) -> Option<&str> {
        self.built
            .iter()
            .find(|b| b.campaign_id == id)
            .map(|b| b.key.as_str())
    }
    /// Before a lookup by `(campaign, map)` (`0x19c270`, `0x19c440`): the
    /// campaign to ask for instead, when the caller's doesn't own the map.
    /// Map ids are unique across campaigns, so the map names its campaign:
    /// one of ours for our ids, else Halo 2's when the caller asked one of
    /// ours. 1.0's callers that hard-code campaign 1 (the level select's
    /// press `0x2c5118`, Continue `0x199ab7`, `0x24a9e9`) would otherwise
    /// find no record for one of our levels.
    pub fn lookup_campaign(&self, campaign: u32, map: u32) -> Option<u32> {
        let want = match campaign_of(&self.built, map) {
            Some(b) => b.campaign_id,
            None if self.key_of_campaign(campaign).is_some() => RETAIL_CAMPAIGN,
            None => campaign,
        };
        (want != campaign).then_some(want)
    }
    /// Before the menu's campaign start: the campaign of the selected map.
    pub fn fix_selection<M: Gm>(&self, m: &mut M) -> Option<u32> {
        let map = m.u32(SELECTED_MAP)?;
        let want = campaign_of(&self.built, map).map_or(RETAIL_CAMPAIGN, |b| b.campaign_id);
        let now = m.u32(SELECTED_CAMPAIGN)?;
        (now != want && (now == RETAIL_CAMPAIGN || self.key_of_campaign(now).is_some()))
            .then(|| m.set_u32(SELECTED_CAMPAIGN, want).then_some(want))
            .flatten()
    }
}

// ---------------------------------------------------------------------------
// At boot.

/// The disc's map names (lower case) and `mainmenu.map`'s retail ids: the
/// level select's campaign ids and the start list's map ids.
pub fn read_disc(disc: &Path) -> anyhow::Result<(Vec<String>, Vec<u32>, Vec<u32>)> {
    use anyhow::Context;
    let open = || std::fs::File::open(disc).with_context(|| format!("opening {}", disc.display()));
    let mut iso = xiso::Xiso::open(std::io::BufReader::new(open()?)).context("reading the disc")?;
    let base = iso.partition_offset();
    let entries = iso.entries()?.clone();
    let names: Vec<String> = entries
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| {
            let p = e.path.to_ascii_lowercase();
            let (dir, file) = p.rsplit_once('/')?;
            (dir.ends_with("maps") && file.ends_with(".map"))
                .then(|| file[..file.len() - 4].to_owned())
        })
        .collect();
    let e = entries
        .iter()
        .find(|e| e.path.to_ascii_lowercase().ends_with("maps/mainmenu.map"))
        .context("the disc has no mainmenu.map")?;
    let mut m = map::Map::from_region(
        std::io::BufReader::new(open()?),
        base + u64::from(e.sector) * xiso::SECTOR,
        e.size.into(),
    )
    .context("parsing mainmenu.map")?;
    let tag = m
        .globals()
        .map(|t| t.id)
        .context("mainmenu.map has no globals tag")?;
    let matg = m.read_tag_bytes(tag)?;
    let mut read = |addr: u32, size: u32| -> Option<Vec<u8>> {
        let off = m.file_offset(addr, size).ok()?;
        m.read_file_range(off, size.into()).ok()
    };
    let le = |b: &[u8], at: usize| {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    let list = |read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
                el: u32,
                at: usize,
                len: usize|
     -> Vec<Vec<u8>> {
        let Some(head) = read(el, at as u32 + 8) else {
            return Vec::new();
        };
        let (Some(n), Some(a)) = (le(&head, at), le(&head, at + 4)) else {
            return Vec::new();
        };
        if n == 0 || n > MAX_ELEMENTS {
            return Vec::new();
        }
        read(a, n * len as u32)
            .map(|b| b.chunks(len).map(<[u8]>::to_vec).collect())
            .unwrap_or_default()
    };
    let start_el = le(&matg, MATG_START as usize + 4).context("no start list")?;
    let maps: Vec<u32> = list(&mut read, start_el, 0, START_LEN)
        .iter()
        .map(|e| u32_at(e, 4))
        .collect();
    let campaigns: Vec<u32> = match le(&matg, MATG_UI as usize + 4) {
        Some(ui) if le(&matg, MATG_UI as usize) != Some(0) => {
            list(&mut read, ui, UI_CAMPAIGNS as usize, UI_CAMPAIGN_LEN)
                .iter()
                .map(|e| u32_at(e, 0))
                .collect()
        }
        _ => Vec::new(),
    };
    let mut campaigns = campaigns;
    campaigns.extend(
        list(&mut read, start_el, 0, START_LEN)
            .iter()
            .map(|e| u32_at(e, 0)),
    );
    Ok((names, campaigns, maps))
}

/// The state the hooks share, when at least one campaign is installed
/// under `<data>/campaigns`; `None` (and the disc unread) otherwise. Says
/// what it found.
pub fn boot_state(data_root: &Path, disc: &Path) -> Option<State> {
    if !data_root.join("campaigns").is_dir() {
        return None;
    }
    let (names, retail_campaigns, retail_maps) = match read_disc(disc) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "campaigns: the disc's level lists are unreadable ({e:#}); one campaign, as retail"
            );
            return None;
        }
    };
    let (ms, lines) = installed(data_root, &|m| names.iter().any(|n| n == m));
    for l in lines {
        eprintln!("{l}");
    }
    if ms.is_empty() {
        return None;
    }
    let built = match cl::build(&ms, &retail_campaigns, &retail_maps) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("campaigns: {e}; one campaign, as retail");
            return None;
        }
    };
    let (progress, why) = Progress::load(data_root.join("campaigns").join(PROGRESS_FILE));
    if let Some(w) = why {
        eprintln!("{w}");
    }
    for b in &built {
        eprintln!(
            "campaigns: {} is campaign {} with {} levels (map ids {:#x}..={:#x})",
            b.key,
            b.campaign_id,
            b.levels.len(),
            b.levels.first().map_or(0, |l| l.map_id),
            b.levels.last().map_or(0, |l| l.map_id)
        );
    }
    Some(State {
        built,
        selection: Selection::default(),
        progress,
    })
}

/// Our campaigns' map files, for the VFS to serve as `maps\<map>.map`
/// where the disc has no such map (`vfs::CampaignMapsBackend`).
pub fn host_maps(data_root: &Path, state: &State) -> Vec<(String, PathBuf)> {
    state
        .built
        .iter()
        .flat_map(|b| {
            b.levels.iter().filter_map(move |l| {
                let p = data_root
                    .join("campaigns")
                    .join(&b.key)
                    .join(format!("{}.map", l.map));
                p.is_file().then(|| (l.map.clone(), p))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
