//! Level boot: start a campaign level, or a local multiplayer game on one
//! of the disc's multiplayer maps, in place of the main menu the game loads
//! at boot (`h2 play --level`, `H2_LEVEL`), and mark the level's first frame
//! for `H2_PERF_FROM=level`.
//!
//! The research and every address are in docs/runtime/level-boot.md. In short: the
//! main loop asks for the main menu (`0x18f1c0`) while no game runs and none
//! is pending. That function builds game options on its stack (clear and
//! default them, `0x138110`; mode 3, the main-menu scenario path; one
//! default local player, `0x138640`) and queues them (`0x18e790`). The first
//! call is answered here instead with the same three calls building campaign
//! options (mode 1) for the requested scenario and difficulty, filled the way
//! the main menu's own campaign start (`0x23643f`) fills them. A
//! multiplayer map gets mode 2 options filled the way the game's session
//! start (`0x6ec80`) fills them for a local game, with a default Slayer
//! variant from the game's own variant defaults (`0x19d220`). Every later
//! call goes to the original, so a failed load still reaches the menu.
//!
//! Activation (`0x18f000`) runs the original, then reads what loaded from
//! the game globals; the present count at that point is the "loaded" mark
//! [`loaded_at`] that `H2_PERF_FROM=level` waits on.
//!
//! The logic is written over [`Gm`] so it is tested without a guest. Each
//! step says one line on stderr, prefixed `level boot:`.

use std::collections::VecDeque;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agent::sim::calls::{Arg, GameStep};
use anyhow::{Context, Result, bail};
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use content::campaign_list as cl;

use crate::layout_glue::Gm;

// ---- guest addresses (docs/runtime/level-boot.md) ----

/// `request_mainmenu(reason)`, stdcall, `ret 4`.
pub const REQUEST_MAINMENU: u32 = 0x0018_f1c0;
/// Clear and default a game-options block; the block in ESI.
pub const OPTIONS_NEW: u32 = 0x0013_8110;
/// Default local players: stdcall `(count, options)`, `ret 8`.
pub const OPTIONS_PLAYERS: u32 = 0x0013_8640;
/// Queue game options for the next transition; the block in EAX.
pub const OPTIONS_QUEUE: u32 = 0x0018_e790;
/// Build a default game variant: stdcall `(variant out, goof game type)`,
/// `ret 8`; code only (the name lookup is skipped while no globals tag is
/// loaded).
pub const VARIANT_DEFAULTS: u32 = 0x0019_d220;
/// The game's options check: stdcall `(options)`, `ret 4`, AL 1 when valid.
pub const OPTIONS_CHECK: u32 = 0x0013_8180;
/// Game activation after the scenario's cache is loaded; options in EAX,
/// returns AL (1 once the map and its first BSP are in). Hooked through the
/// `game` bridge.
pub const GAME_ACTIVATE: u32 = hooks::points::GAME_ACTIVATE.base;
/// Pointer to the game globals: the active options at `+8`, the "map
/// active" byte at `+0x1120` (activation sets it).
pub const GAME_GLOBALS: u32 = hooks::points::GAME_GLOBALS.base;

/// Game options (`0x1118` bytes).
pub const OPTIONS_SIZE: u32 = 0x1118;
pub const OPT_MODE: u32 = 0x0;
pub const OPT_CAMPAIGN: u32 = 0x14;
pub const OPT_MAP: u32 = 0x18;
pub const OPT_PATH: u32 = 0x1c;
pub const OPT_PATH_LEN: u32 = 0x104;
pub const OPT_DIFFICULTY: u32 = 0x12a;
/// More than one local player (the menu's start sets it from its count).
pub const OPT_SPLIT: u32 = 0x12c;
/// Its inverse, as the menu's start writes it.
pub const OPT_SINGLE: u32 = 0x12d;
/// Session kind byte: 1 for a local game (`0x6ec80` writes 1, 2 or 3 by
/// session type; `OPTIONS_NEW` leaves 0).
pub const OPT_SESSION: u32 = 0x5;
/// The variant's content index (`0x6ec80`: from the session, else -1).
pub const OPT_VARIANT_INDEX: u32 = 0x130;
/// The game variant, `0x130` bytes.
pub const OPT_VARIANT: u32 = 0x134;
pub const VARIANT_SIZE: u32 = 0x130;
pub const MODE_CAMPAIGN: u32 = 1;
pub const MODE_MULTIPLAYER: u32 = 2;
pub const MODE_MAINMENU: u32 = 3;
/// `goof` game type 0, Slayer (docs/game/community-gametypes.md).
pub const GAME_TYPE_SLAYER: u32 = 0;
const GLOBALS_OPTIONS: u32 = 0x8;
const GLOBALS_ACTIVE: u32 = 0x1120;

/// The globals tag's campaign level list, as `0x19c270` reads it:
/// `matg +0x170` is a block whose element 0 starts with a `(count,
/// address)` block of `0x108`-byte levels (`+0` campaign id, `+4` map id,
/// `+8` scenario path, `0x100` bytes).
const MATG_CAMPAIGN: usize = 0x170;
const LEVEL_SIZE: usize = 0x108;
const LEVEL_PATH: usize = 8;
const MAX_LEVELS: u32 = 256;
/// The multiplayer level list (`0x19c5f0`; `h2 live` reads it too):
/// `matg +0x178` element 0, block at `+0x10` of `0xc64`-byte levels, `+0`
/// map id, `+0xb4c` scenario path.
const MATG_MULTIPLAYER: usize = 0x178;
const MP_BLOCK: usize = 0x10;
const MP_LEVEL_SIZE: usize = 0xc64;
const MP_LEVEL_PATH: usize = 0xb4c;

/// The options we build.
const BUFFER: u32 = 0xa2f4_1000;
// Owned by this hook, just past the retail options struct in the same mapped
// two pages. Guest RAM snapshots carry the one-shot across cold Theater seeks.
const REQUEST_TAKEN: u32 = BUFFER + OPTIONS_SIZE;

fn take_level_request(guest: &mut runtime::Guest) -> Result<bool, GuestFault> {
    if guest.u8(REQUEST_TAKEN)? != 0 {
        return Ok(false);
    }
    guest.set_u8(REQUEST_TAKEN, 1)?;
    Ok(true)
}

// ---- requests ----

/// Campaign difficulty, the options' `+0x12a` word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Heroic,
    Legendary,
}

impl Difficulty {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "easy" => Self::Easy,
            "normal" => Self::Normal,
            "heroic" => Self::Heroic,
            "legendary" => Self::Legendary,
            _ => bail!("unknown difficulty {s:?} (easy, normal, heroic or legendary)"),
        })
    }
    pub fn index(self) -> u16 {
        self as u16
    }
}

impl fmt::Display for Difficulty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Heroic => "heroic",
            Self::Legendary => "legendary",
        })
    }
}

/// Which game a level starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Campaign,
    /// A local game, one player, default Slayer.
    Multiplayer,
}

/// A campaign or multiplayer map on the disc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscLevel {
    /// The map file's stem, `03a_oldmombasa`, `ascension`.
    pub name: String,
    /// The cache header's scenario path.
    pub scenario: String,
    pub kind: Kind,
}

/// What to load in place of the main menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    pub scenario: String,
    /// Campaign and map ids from the globals' level list, `u32::MAX` (as
    /// the options default them) when the list does not name the scenario.
    pub campaign: u32,
    pub map: u32,
    /// Campaign only.
    pub difficulty: Difficulty,
    pub kind: Kind,
    pub variant: Option<BootVariant>,
    /// A recorded game's whole options block ([`Request::recorded`]),
    /// queued as it is in place of the block built from the fields above.
    pub options: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootVariant {
    record: variants::GameVariant,
    pub script: Option<Vec<u8>>,
}

impl Request {
    /// Use the supplied portable record for a local multiplayer boot.
    pub fn set_variant(&mut self, text: &str) -> Result<()> {
        if self.kind != Kind::Multiplayer {
            bail!("a game variant needs a multiplayer level");
        }
        let bundle = variants::portable::read(text)?;
        let record = bundle.variant()?;
        if let Some(problem) = variants::record::refused(&record).first() {
            bail!("the game variant is invalid: {}", problem.what);
        }
        self.variant = Some(BootVariant { record, script: bundle.ir()? });
        Ok(())
    }

    /// The request for a recorded game: its options block as the game held
    /// it at activation (`film::start::GameStart::options`). The block is
    /// queued whole; the fields are read from it for the log line and for
    /// [`is_wanted`]. A block of another size, or a mode that is neither a
    /// campaign nor a multiplayer game, is refused.
    pub fn recorded(options: &[u8]) -> Result<Self> {
        if options.len() != OPTIONS_SIZE as usize {
            bail!("the recorded game options are {} bytes, not {OPTIONS_SIZE}", options.len());
        }
        let word = |at: u32| u32::from_le_bytes(options[at as usize..at as usize + 4].try_into().unwrap());
        let kind = match word(OPT_MODE) {
            MODE_CAMPAIGN => Kind::Campaign,
            MODE_MULTIPLAYER => Kind::Multiplayer,
            mode => bail!("the recorded game has options mode {mode}, not a campaign or multiplayer game"),
        };
        let field = &options[OPT_PATH as usize..(OPT_PATH + OPT_PATH_LEN) as usize];
        let end = field.iter().position(|c| *c == 0).unwrap_or(field.len());
        let scenario = String::from_utf8_lossy(&field[..end]).into_owned();
        if scenario.is_empty() || end == field.len() {
            bail!("the recorded game options name no scenario");
        }
        let difficulty = match u16::from_le_bytes([options[OPT_DIFFICULTY as usize], options[OPT_DIFFICULTY as usize + 1]]) {
            0 => Difficulty::Easy,
            2 => Difficulty::Heroic,
            3 => Difficulty::Legendary,
            _ => Difficulty::Normal,
        };
        Ok(Self {
            name: short(&scenario).to_owned(),
            scenario,
            campaign: word(OPT_CAMPAIGN),
            map: word(OPT_MAP),
            difficulty,
            kind,
            variant: None,
            options: Some(options.to_vec()),
        })
    }

    /// What the `requesting` line says after the name.
    pub fn game(&self) -> String {
        match self.kind {
            Kind::Campaign => self.difficulty.to_string(),
            Kind::Multiplayer => self.variant.as_ref().map_or_else(|| "slayer".into(), |v| v.record.name()),
        }
    }
}

/// The level called `name` (a map file stem or a scenario path, any case),
/// or an error listing every level, campaign then multiplayer.
pub fn pick<'a>(levels: &'a [DiscLevel], name: &str) -> Result<&'a DiscLevel> {
    let n = name.trim().trim_end_matches(".map");
    if let Some(l) = levels.iter().find(|l| l.name.eq_ignore_ascii_case(n) || l.scenario.eq_ignore_ascii_case(n)) {
        return Ok(l);
    }
    let list = |k: Kind| {
        let mut v: Vec<&str> = levels.iter().filter(|l| l.kind == k).map(|l| l.name.as_str()).collect();
        v.sort_unstable();
        v
    };
    let (campaign, mp) = (list(Kind::Campaign), list(Kind::Multiplayer));
    if campaign.is_empty() && mp.is_empty() {
        bail!("no level {name:?}: the disc has no campaign or multiplayer maps");
    }
    let mut msg = format!("no level {name:?}; the disc has:");
    for (title, v) in [("campaign", campaign), ("multiplayer", mp)] {
        if !v.is_empty() {
            msg += &format!("\n {title}:\n  {}", v.join("\n  "));
        }
    }
    bail!("{msg}")
}

/// `(campaign id, map id, scenario path)` for each level of a globals tag;
/// `read` resolves a tag-memory address.
pub fn campaign_levels(matg: &[u8], read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>) -> Vec<(u32, u32, String)> {
    let le = |b: &[u8], at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    let (Some(n), Some(addr)) = (le(matg, MATG_CAMPAIGN), le(matg, MATG_CAMPAIGN + 4)) else {
        return Vec::new();
    };
    if n == 0 {
        return Vec::new();
    }
    let Some(head) = read(addr, 8) else {
        return Vec::new();
    };
    let (Some(count), Some(at)) = (le(&head, 0), le(&head, 4)) else {
        return Vec::new();
    };
    if count == 0 || count > MAX_LEVELS {
        return Vec::new();
    }
    let Some(all) = read(at, count * LEVEL_SIZE as u32) else {
        return Vec::new();
    };
    all.as_chunks::<LEVEL_SIZE>()
        .0
        .iter()
        .filter_map(|e| {
            let p = &e[LEVEL_PATH..];
            let p = &p[..p.iter().position(|c| *c == 0).unwrap_or(p.len())];
            Some((le(e, 0)?, le(e, 4)?, String::from_utf8_lossy(p).into_owned()))
        })
        .collect()
}

/// `(map id, scenario path)` for each multiplayer level of a globals tag.
pub fn multiplayer_levels(matg: &[u8], read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>) -> Vec<(u32, String)> {
    let le = |b: &[u8], at: usize| b.get(at..at + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    let (Some(n), Some(addr)) = (le(matg, MATG_MULTIPLAYER), le(matg, MATG_MULTIPLAYER + 4)) else {
        return Vec::new();
    };
    if n == 0 {
        return Vec::new();
    }
    let Some(head) = read(addr, (MP_BLOCK + 8) as u32) else {
        return Vec::new();
    };
    let (Some(count), Some(at)) = (le(&head, MP_BLOCK), le(&head, MP_BLOCK + 4)) else {
        return Vec::new();
    };
    if count == 0 || count > MAX_LEVELS {
        return Vec::new();
    }
    let Some(all) = read(at, count * MP_LEVEL_SIZE as u32) else {
        return Vec::new();
    };
    all.as_chunks::<MP_LEVEL_SIZE>()
        .0
        .iter()
        .filter_map(|e| {
            let p = &e[MP_LEVEL_PATH..MP_LEVEL_PATH + 0x100];
            let p = &p[..p.iter().position(|c| *c == 0).unwrap_or(p.len())];
            Some((le(e, 0)?, String::from_utf8_lossy(p).into_owned()))
        })
        .collect()
}

/// The ids the level list gives `scenario`, if it names it.
pub fn ids_for(levels: &[(u32, u32, String)], scenario: &str) -> Option<(u32, u32)> {
    levels.iter().find(|(_, _, p)| p.eq_ignore_ascii_case(scenario)).map(|(c, m, _)| (*c, *m))
}

/// The disc's campaign and multiplayer maps.
pub fn disc_levels(disc: &Path) -> Result<Vec<DiscLevel>> {
    let mut out = Vec::new();
    for_each_map(disc, |stem, map| {
        let kind = match map.header().cache_type {
            map::CacheType::Campaign => Kind::Campaign,
            map::CacheType::Multiplayer => Kind::Multiplayer,
            _ => return true,
        };
        out.push(DiscLevel { name: stem.to_string(), scenario: map.header().scenario_path.clone(), kind });
        true
    })?;
    Ok(out)
}

type DiscMap = map::Map<std::io::BufReader<std::fs::File>>;

/// Calls `f` with each `.map` on the disc that parses, until it returns false.
fn for_each_map(disc: &Path, mut f: impl FnMut(&str, &mut DiscMap) -> bool) -> Result<()> {
    if disc.is_dir() {
        for entry in std::fs::read_dir(disc).with_context(|| format!("listing {}", disc.display()))? {
            let entry = entry?;
            if !entry.file_name().eq_ignore_ascii_case("maps") || !entry.path().is_dir() {
                continue;
            }
            let maps = entry.path();
            for entry in std::fs::read_dir(&maps).with_context(|| format!("listing {}", maps.display()))? {
                let path = entry?.path();
                if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("map")) || !path.is_file() {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
                    continue;
                };
                let file = std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
                let Ok(mut map) = map::Map::from_reader(std::io::BufReader::new(file)) else {
                    continue;
                };
                if !f(stem, &mut map) {
                    return Ok(());
                }
            }
        }
        return Ok(());
    }
    let open = || std::fs::File::open(disc).with_context(|| format!("opening {}", disc.display()));
    let mut iso = xiso::Xiso::open(std::io::BufReader::new(open()?)).context("reading the disc")?;
    let base = iso.partition_offset();
    for e in iso.entries()?.iter().filter(|e| !e.is_dir && e.path.to_ascii_lowercase().ends_with(".map")) {
        let stem = e.path.rsplit('/').next().unwrap_or(&e.path);
        let stem = &stem[..stem.len() - 4];
        let at = base + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = map::Map::from_region(std::io::BufReader::new(open()?), at, e.size.into()) else {
            continue;
        };
        if !f(stem, &mut m) {
            break;
        }
    }
    Ok(())
}

/// The campaign and map ids for `level`: from its own globals, else the
/// main menu's.
fn level_ids(disc: &Path, level: &DiscLevel) -> Option<(u32, u32)> {
    let mut found = None;
    let _ = for_each_map(disc, |stem, m| {
        if !(stem.eq_ignore_ascii_case(&level.name) || stem.eq_ignore_ascii_case("mainmenu")) {
            return true;
        }
        let Some(id) = m.globals().map(|t| t.id) else {
            return true;
        };
        let Ok(matg) = m.read_tag_bytes(id) else {
            return true;
        };
        let mut read = |addr: u32, size: u32| {
            let off = m.file_offset(addr, size).ok()?;
            m.read_file_range(off, size.into()).ok()
        };
        found = match level.kind {
            Kind::Campaign => ids_for(&campaign_levels(&matg, &mut read), &level.scenario),
            Kind::Multiplayer => multiplayer_levels(&matg, &mut read)
                .iter()
                .find(|(_, p)| p.eq_ignore_ascii_case(&level.scenario))
                .map(|(m, _)| (u32::MAX, *m)),
        };
        found.is_none()
    });
    found
}

/// Resolves `name` against the disc's campaign and multiplayer maps.
pub fn resolve(disc: &Path, name: &str, difficulty: Difficulty) -> Result<Request> {
    resolve_in(disc, None, name, difficulty)
}

/// Resolves `name` against the disc's campaign and multiplayer maps, then,
/// with a data root, the levels of the campaigns installed there
/// (`<data>/campaigns/<key>/`, such as Halo 1's `a30`).
pub fn resolve_in(disc: &Path, data_root: Option<&Path>, name: &str, difficulty: Difficulty) -> Result<Request> {
    let levels = disc_levels(disc)?;
    let level = match pick(&levels, name) {
        Ok(l) => l,
        Err(e) => {
            if let Some(r) = data_root.and_then(|d| installed_request(d, disc, name, difficulty)) {
                return Ok(r);
            }
            return Err(e);
        }
    };
    if level.scenario.len() >= OPT_PATH_LEN as usize {
        bail!("{}: scenario path {:?} is longer than the game options hold", level.name, level.scenario);
    }
    let (campaign, map) = level_ids(disc, level).unwrap_or_else(|| {
        eprintln!(
            "level boot: the globals' level list does not name {}; campaign and map ids left unset",
            level.scenario
        );
        (u32::MAX, u32::MAX)
    });
    Ok(Request {
        name: level.name.clone(),
        scenario: level.scenario.clone(),
        campaign,
        map,
        difficulty,
        kind: level.kind,
        variant: None,
        options: None,
    })
}

/// A level of an installed campaign, called by its map name (`a30`) or
/// scenario path: the start record's campaign id, map id and scenario path
/// as [`crate::campaigns`] builds them, so the game opens
/// `d:\maps\<map>.map`, which `vfs::CampaignMapsBackend` serves from the
/// tag store for Halo 1 levels.
pub fn campaign_request(built: &[cl::Built], name: &str, difficulty: Difficulty) -> Option<Request> {
    let n = name.trim().trim_end_matches(".map");
    built.iter().find_map(|b| {
        b.levels
            .iter()
            .find(|l| l.map.eq_ignore_ascii_case(n) || l.scenario.eq_ignore_ascii_case(n))
            .map(|l| Request {
                name: l.map.clone(),
                scenario: l.scenario.clone(),
                campaign: b.campaign_id,
                map: l.map_id,
                difficulty,
                kind: Kind::Campaign,
                variant: None,
                options: None,
            })
    })
}

/// [`campaign_request`] over the campaigns installed under `data_root`,
/// built against the disc's lists exactly as at boot
/// ([`crate::campaigns::boot_state`]).
fn installed_request(data_root: &Path, disc: &Path, name: &str, difficulty: Difficulty) -> Option<Request> {
    let (names, campaigns, maps) = crate::campaigns::read_disc(disc).ok()?;
    let (ms, _) = crate::campaigns::installed(data_root, &|m| names.iter().any(|n| n == m));
    let built = cl::build(&ms, &campaigns, &maps).ok()?;
    campaign_request(&built, name, difficulty)
}

/// `H2_LEVEL` and `H2_DIFFICULTY`, resolved against `disc` (and the
/// campaigns installed under `data_root`); `None` when `H2_LEVEL` is unset
/// or empty.
pub fn from_env(disc: &Path, data_root: Option<&Path>) -> Result<Option<Request>> {
    from_vars(
        disc,
        data_root,
        std::env::var("H2_LEVEL").ok().as_deref(),
        std::env::var("H2_DIFFICULTY").ok().as_deref(),
    )
}

pub fn from_vars(
    disc: &Path,
    data_root: Option<&Path>,
    level: Option<&str>,
    difficulty: Option<&str>,
) -> Result<Option<Request>> {
    let Some(level) = level.map(str::trim).filter(|l| !l.is_empty()) else {
        return Ok(None);
    };
    let difficulty = difficulty.filter(|d| !d.is_empty()).map(Difficulty::parse).transpose()?.unwrap_or_default();
    resolve_in(disc, data_root, level, difficulty).map(Some)
}

// ---- H2_PERF_FROM ----

/// When `H2_PERF_FROM` opens the profiler session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerfFrom {
    /// At present N.
    Present(u64),
    /// At the first present after the level loaded ([`loaded_at`]).
    Level,
}

impl PerfFrom {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("level") {
            return Some(Self::Level);
        }
        s.parse().ok().map(Self::Present)
    }
    /// Whether the session opens now, with `presents` out and the level
    /// marked loaded at `loaded` presents.
    pub fn due(self, presents: u64, loaded: Option<u64>) -> bool {
        match self {
            Self::Present(n) => presents >= n,
            Self::Level => loaded.is_some_and(|l| presents > l),
        }
    }
}

static LOADED_AT: AtomicU64 = AtomicU64::new(u64::MAX);

/// The present count when the level activated, once it has.
pub fn loaded_at() -> Option<u64> {
    let v = LOADED_AT.load(Ordering::Relaxed);
    (v != u64::MAX).then_some(v)
}

// ---- guest-side logic ----

/// Writes the request into options `buf` that `OPTIONS_NEW` has cleared and
/// defaulted. A campaign level is filled as the main menu's campaign start
/// (`0x23643f`) fills it for one local player; a multiplayer map as the
/// session start (`0x6ec80`) fills a local game, before the variant is
/// built into it. A path longer than the field is refused.
pub fn fill_options<M: Gm>(m: &mut M, buf: u32, req: &Request) -> bool {
    if let Some(options) = &req.options {
        return m.wr(buf, &local_options(options));
    }
    let path = req.scenario.as_bytes();
    if path.len() >= OPT_PATH_LEN as usize || path.contains(&0) {
        return false;
    }
    let mut field = vec![0u8; OPT_PATH_LEN as usize];
    field[..path.len()].copy_from_slice(path);
    let common = m.set_u32(buf + OPT_CAMPAIGN, req.campaign)
        && m.set_u32(buf + OPT_MAP, req.map)
        && m.wr(buf + OPT_PATH, &field);
    common
        && match req.kind {
            Kind::Campaign => {
                m.set_u32(buf + OPT_MODE, MODE_CAMPAIGN)
                    && m.set_u16(buf + OPT_DIFFICULTY, req.difficulty.index())
                    && m.wr(buf + OPT_SPLIT, &[0])
                    && m.wr(buf + OPT_SINGLE, &[1])
            }
            Kind::Multiplayer => {
                m.set_u32(buf + OPT_MODE, MODE_MULTIPLAYER)
                    && m.wr(buf + OPT_SESSION, &[1])
                    && m.set_u32(buf + OPT_VARIANT_INDEX, u32::MAX)
            }
        }
}

/// A recorded options block as it is queued: the session kind byte
/// (`OPT_SESSION`) says a local game wherever the game was played, so the
/// game loads as the menu's own start loads it, with no session or network
/// to join. A campaign's byte is 0 (`OPTIONS_NEW`'s) and stays 0. Every
/// other byte is the recorded one.
pub fn local_options(options: &[u8]) -> Vec<u8> {
    let mut out = options.to_vec();
    if let Some(kind) = out.get_mut(OPT_SESSION as usize)
        && *kind != 0
    {
        *kind = 1;
    }
    out
}

/// One step of building the options, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// `OPTIONS_NEW`, the block in ESI.
    New,
    /// [`fill_options`].
    Fill,
    /// `VARIANT_DEFAULTS(options + OPT_VARIANT, GAME_TYPE_SLAYER)`.
    Variant,
    /// `OPTIONS_PLAYERS(1, options)` after the mode is written.
    Players,
    /// `OPTIONS_CHECK(options)`, reported only.
    Check,
    /// `OPTIONS_QUEUE`, the block in EAX.
    Queue,
}

/// The steps for `req`: a recorded block is written and queued, with no
/// defaults built under it; any other request builds its options
/// ([`steps`]).
pub fn steps_for(req: &Request) -> Vec<Step> {
    if req.options.is_some() {
        vec![Step::Fill, Step::Queue]
    } else {
        steps(req.kind)
    }
}

/// The steps for `kind`.
pub fn steps(kind: Kind) -> Vec<Step> {
    match kind {
        Kind::Campaign => vec![Step::New, Step::Fill, Step::Players, Step::Check, Step::Queue],
        Kind::Multiplayer => vec![
            Step::New, Step::Fill, Step::Variant, Step::Players, Step::Check, Step::Queue,
        ],
    }
}

/// Apply match settings to the System Link lobby's default variant.
pub fn configured_variant(
    raw: &[u8],
    settings: &[(u32, i32)],
) -> std::result::Result<variants::GameVariant, String> {
    let mut v = variants::GameVariant::from_bytes(raw).map_err(|e| e.to_string())?;
    for &(id, value) in settings {
        v.set(variants::SettingId(id), value).map_err(|e| format!("setting {id} = {value}: {e}"))?;
    }
    Ok(v)
}

/// A guest call: target, stack arguments (first argument first), and EAX
/// and ESI when the callee takes them there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuestCall {
    pub target: u32,
    pub args: Vec<u32>,
    pub eax: Option<u32>,
    pub esi: Option<u32>,
}

/// The guest call for a step; `None` for [`Step::Fill`], which is ours.
pub fn call_of(step: Step, buf: u32) -> Option<GuestCall> {
    let (target, args, eax, esi) = match step {
        Step::New => (OPTIONS_NEW, vec![], None, Some(buf)),
        Step::Fill => return None,
        Step::Variant => (VARIANT_DEFAULTS, vec![buf + OPT_VARIANT, GAME_TYPE_SLAYER], None, None),
        Step::Players => (OPTIONS_PLAYERS, vec![1, buf], None, None),
        Step::Check => (OPTIONS_CHECK, vec![buf], None, None),
        Step::Queue => (OPTIONS_QUEUE, vec![], Some(buf), None),
    };
    Some(GuestCall { target, args, eax, esi })
}

/// Records the level boot's writes instead of making them.
#[derive(Default)]
struct Writes(Vec<(u32, Vec<u8>)>);

impl crate::layout_glue::Gm for Writes {
    fn rd(&self, _: u32, _: &mut [u8]) -> bool {
        false
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.push((addr, data.to_vec()));
        true
    }
}

/// The level boot's steps for `req` as guest-call steps over an options
/// block at offset 0 (`call_of` with the block's address
/// given at the call). `None` when the options can't hold the request.
pub fn game_steps(req: &Request) -> Option<Vec<GameStep>> {
    let call = |target: u32, args: Vec<Arg>, eax: Option<Arg>, esi: Option<Arg>, report| {
        GameStep::Call {
            target,
            args,
            eax,
            esi,
            report,
        }
    };
    let opts = Arg::Options(0);
    let mut out = Vec::new();
    for step in steps_for(req) {
        let s = match step {
            Step::New => call(OPTIONS_NEW, vec![], None, Some(opts), None),
            Step::Fill => {
                let mut w = Writes::default();
                if !fill_options(&mut w, 0, req) {
                    return None;
                }
                out.extend(
                    w.0.into_iter()
                        .map(|(offset, bytes)| GameStep::Write { offset, bytes }),
                );
                continue;
            }
            Step::Variant if let Some(variant) = &req.variant => {
                out.push(GameStep::Write {
                    offset: OPT_VARIANT,
                    bytes: variant.record.bytes().to_vec(),
                });
                continue;
            }
            Step::Variant => call(
                VARIANT_DEFAULTS,
                vec![
                    Arg::Options(OPT_VARIANT),
                    Arg::Value(GAME_TYPE_SLAYER),
                ],
                None,
                None,
                None,
            ),
            Step::Players => call(
                OPTIONS_PLAYERS,
                vec![Arg::Value(1), opts],
                None,
                None,
                None,
            ),
            Step::Check => call(OPTIONS_CHECK, vec![opts], None, None, Some("check")),
            Step::Queue => call(OPTIONS_QUEUE, vec![], Some(opts), None, None),
        };
        out.push(s);
    }
    Some(out)
}

/// The active game's options mode (1 campaign, 2 multiplayer), once
/// activation has set the globals' active byte. Cheap: for per-tick hooks.
pub fn active_mode<M: Gm>(m: &M) -> Option<u32> {
    let g = m.u32(GAME_GLOBALS).filter(|g| *g != 0)?;
    if m.u8(g + GLOBALS_ACTIVE)? == 0 {
        return None;
    }
    m.u32(g + GLOBALS_OPTIONS + OPT_MODE)
}

/// The active game's mode and scenario path, once activation has set the
/// globals' active byte.
pub fn active_game<M: Gm>(m: &M) -> Option<(u32, String)> {
    let g = m.u32(GAME_GLOBALS).filter(|g| *g != 0)?;
    if m.u8(g + GLOBALS_ACTIVE)? == 0 {
        return None;
    }
    let o = g + GLOBALS_OPTIONS;
    let mut p = vec![0u8; OPT_PATH_LEN as usize];
    if !m.rd(o + OPT_PATH, &mut p) {
        return None;
    }
    p.truncate(p.iter().position(|c| *c == 0).unwrap_or(p.len()));
    Some((m.u32(o + OPT_MODE)?, String::from_utf8_lossy(&p).into_owned()))
}

/// Whether the game that activated is the level we wait for: the request's
/// scenario, or with no request any game that is not the main menu.
pub fn is_wanted(req: Option<&Request>, mode: u32, path: &str) -> bool {
    match req {
        Some(r) => path.eq_ignore_ascii_case(&r.scenario),
        None => mode != MODE_MAINMENU,
    }
}

/// The last part of a scenario path.
fn short(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

// ---- hooks ----

/// Installs the main-menu replacement when `req` is given, and adds the
/// activation mark to `registry` when `req` is given or `watch`
/// (`H2_PERF_FROM=level`).
pub fn install(
    rt: &runtime::Runtime,
    registry: &mut hooks::Registry,
    req: Option<Request>,
    watch: bool,
) -> Result<()> {
    if req.is_none() && !watch {
        return Ok(());
    }
    let req = req.map(Arc::new);
    registry
        .add(&LoadedMark(req.clone()))
        .map_err(|e| anyhow::anyhow!("level boot: {e}"))?;
    let Some(req) = req else { return Ok(()) };
    let Some(menu) = rt.resolve_original(REQUEST_MAINMENU).map(Arc::new) else {
        eprintln!("level boot: no generated entry at {REQUEST_MAINMENU:#x}; hook skipped");
        return Ok(());
    };
    rt.memory().map_zeroed(BUFFER, 0x2000, Permissions::RW)?;
    rt.edit_dispatch(|d| {
        d.register_override(REQUEST_MAINMENU, "level-boot:menu", move |cpu, ctx, _| {
            if !take_level_request(&mut ctx.runtime().memory())? {
                return Ok(menu.invoke(cpu, ctx, REQUEST_MAINMENU));
            }
            request_level(cpu, ctx, Arc::clone(&req))
        });
    });
    Ok(())
}

/// The activation mark (`level-boot`), a listener on the `game` bridge's
/// start: the first time a game starts, if it is the wanted level (or any
/// level with no request), the present count is [`loaded_at`]. It shares
/// activation with Live telemetry's match start.
pub struct LoadedMark(pub Option<Arc<Request>>);

impl hooks::Feature for LoadedMark {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("level-boot", "Level boot loaded mark").doc("docs/runtime/level-boot.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> std::result::Result<(), hooks::BuildError> {
        let req = self.0.clone();
        b.game().on_start("loaded mark", move |o| {
            if loaded_at().is_some() {
                return;
            }
            let found = active_game(&*o.guest());
            match found {
                Some((mode, path)) if is_wanted(req.as_deref(), mode, &path) => {
                    let n = crate::presents::presents();
                    LOADED_AT.store(n, Ordering::Relaxed);
                    eprintln!("level boot: {} loaded at present {n}", short(&path));
                }
                Some((mode, path)) if req.is_some() => {
                    eprintln!("level boot: {} activated (mode {mode}), not the requested level", short(&path))
                }
                _ => {}
            }
        });
        // `LOADED_AT` is a host-side mark for perf sessions; the game never
        // reads it.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// In place of the first `request_mainmenu(reason)`: run [`steps`] on the
/// options at [`BUFFER`], then return as the original does (`ret 4`, ESI
/// preserved; the game's functions preserve the other callee-saved
/// registers).
fn request_level(cpu: &mut CpuAbi, ctx: &mut ExecCtx, req: Arc<Request>) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    let reason = mem.u32(cpu.esp.wrapping_add(4))?;
    drop(mem);
    eprintln!(
        "level boot: requesting {} {} (scenario {}, campaign {}, map {}; in place of the main menu, reason {reason})",
        req.name,
        req.game(),
        req.scenario,
        req.campaign as i32,
        req.map as i32
    );
    let entry = Entry { ret_pc, esp: cpu.esp, esi: cpu.esi };
    let todo = steps_for(&req).into();
    run_from(cpu, ctx, req, todo, entry)
}

/// The caller's state at the hook.
#[derive(Clone, Copy)]
struct Entry {
    ret_pc: u32,
    esp: u32,
    esi: u32,
}

fn run_from(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    req: Arc<Request>,
    mut todo: VecDeque<Step>,
    entry: Entry,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    while let Some(step) = todo.pop_front() {
        let written = match step {
            Step::Fill => Some(fill_options(&mut *mem, BUFFER, &req)),
            Step::Variant => req.variant.as_ref().map(|variant| {
                Gm::wr(&mut *mem, BUFFER + OPT_VARIANT, variant.record.bytes())
            }),
            _ => None,
        };
        if let Some(written) = written {
            if !written {
                // The hook is spent, so this reaches the original.
                eprintln!("level boot: could not write the options; loading the main menu");
                cpu.esi = entry.esi;
                cpu.esp = entry.esp;
                return Ok(TransferAbi::jump(REQUEST_MAINMENU));
            }
            continue;
        }
        let Some(GuestCall { target, args, eax, esi }) = call_of(step, BUFFER) else { continue };
        for a in args.iter().rev() {
            cpu.esp = cpu.esp.wrapping_sub(4);
            mem.set_u32(cpu.esp, *a)?;
        }
        if let Some(v) = eax {
            cpu.eax = v;
        }
        if let Some(v) = esi {
            cpu.esi = v;
        }
        return ctx.call_guest(cpu, &mut mem, target, move |cpu, ctx| {
            match step {
                Step::Check if cpu.eax & 0xff != 0 => eprintln!("level boot: the game's options check passes"),
                Step::Check => eprintln!(
                    "level boot: the game's options check (0x138180) rejects the options; queueing them anyway"
                ),
                Step::Queue => {
                    eprintln!("level boot: options queued");
                    cpu.esi = entry.esi;
                    cpu.esp = entry.esp.wrapping_add(8);
                    return Ok(TransferAbi::ret(entry.ret_pc));
                }
                _ => {}
            }
            run_from(cpu, ctx, req, todo, entry)
        });
    }
    // Not reached: the last step is the queue.
    cpu.esi = entry.esi;
    cpu.esp = entry.esp.wrapping_add(8);
    Ok(TransferAbi::ret(entry.ret_pc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn a_restored_direct_start_leaves_the_next_menu_request_to_retail() {
        let mut recorded = runtime::Guest::new();
        recorded.map_zeroed(BUFFER, 0x2000, Permissions::RW).unwrap();
        assert!(take_level_request(&mut recorded).unwrap());
        // The retail options constructor can clear the complete options block.
        recorded.write_bytes(BUFFER, &vec![0; OPTIONS_SIZE as usize], 1).unwrap();
        let pages = recorded.read::<0x2000>(BUFFER).unwrap();

        let mut restored = runtime::Guest::new();
        restored.map_zeroed(BUFFER, 0x2000, Permissions::RW).unwrap();
        restored.write_bytes(BUFFER, &pages, 1).unwrap();
        assert!(!take_level_request(&mut restored).unwrap());
    }

    #[test]
    fn provisioned_disc_maps_use_the_level_callbacks() {
        // An Xbox v8 header and an empty tag index, with no retail data.
        let mut map = [0u8; 0x820];
        map[..4].copy_from_slice(b"daeh");
        for (offset, value) in [
            (4, 8u32), (8, 0x820), (0x10, 0x800), (0x14, 0x20),
            (0x140, 1), (0x800, 0x8000_0020), (0x808, 0x8000_0020),
            (0x80c, u32::MAX), (0x810, u32::MAX),
        ] {
            map[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        map[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
        map[0x1bc..0x1c0].copy_from_slice(b"test");
        map[0x7fc..0x800].copy_from_slice(b"toof");
        map[0x81c..].copy_from_slice(b"sgat");

        let disc = std::env::temp_dir().join(format!("h2-level-disc-{}", std::process::id()));
        let maps = disc.join("MAPS");
        std::fs::create_dir_all(&maps).unwrap();
        std::fs::write(maps.join("one.MAP"), map).unwrap();
        std::fs::write(maps.join("two.map"), map).unwrap();
        std::fs::write(maps.join("bad.map"), b"invalid cache").unwrap();
        std::fs::write(maps.join("not-a-map.txt"), map).unwrap();

        let levels = disc_levels(&disc).unwrap();
        assert_eq!(levels.len(), 2);
        assert!(levels.iter().all(|level| level.scenario == "test" && level.kind == Kind::Multiplayer));
        assert!(levels.iter().any(|level| level.name == "one"));
        let mut visits = 0;
        for_each_map(&disc, |_, _| {
            visits += 1;
            false
        }).unwrap();
        assert_eq!(visits, 1);
        std::fs::remove_dir_all(&disc).unwrap();
    }

    /// A sparse byte map standing in for guest memory; unwritten bytes read
    /// as absent.
    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    fn request(difficulty: Difficulty) -> Request {
        Request {
            name: "03a_oldmombasa".into(),
            scenario: r"scenarios\solo\03a_oldmombasa\03a_oldmombasa".into(),
            campaign: 1,
            map: 7,
            difficulty,
            kind: Kind::Campaign,
            variant: None,
            options: None,
        }
    }

    fn mp_request() -> Request {
        Request {
            name: "ascension".into(),
            scenario: r"scenarios\multi\ascension\ascension".into(),
            campaign: u32::MAX,
            map: 80,
            difficulty: Difficulty::Normal,
            kind: Kind::Multiplayer,
            variant: None,
            options: None,
        }
    }

    /// Options as `OPTIONS_NEW` leaves them for our purposes: every byte
    /// present (`0xcc` stands in for its defaults), mode 1, ids -1.
    fn defaulted(m: &mut Fake, buf: u32) {
        m.wr(buf, &vec![0xcc; OPTIONS_SIZE as usize]);
        m.set_u32(buf + OPT_MODE, MODE_CAMPAIGN);
        m.set_u32(buf + OPT_CAMPAIGN, u32::MAX);
        m.set_u32(buf + OPT_MAP, u32::MAX);
    }

    fn path_at(m: &Fake, at: u32) -> String {
        let mut p = vec![0; OPT_PATH_LEN as usize];
        assert!(m.rd(at, &mut p));
        p.truncate(p.iter().position(|c| *c == 0).unwrap());
        String::from_utf8(p).unwrap()
    }

    /// A recorded multiplayer game's options: every byte set, as an online
    /// game that was played on a public session left them.
    fn recorded_block() -> Vec<u8> {
        let mut block = vec![0x5a; OPTIONS_SIZE as usize];
        let put = |block: &mut Vec<u8>, at: u32, bytes: &[u8]| {
            block[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
        };
        put(&mut block, OPT_MODE, &MODE_MULTIPLAYER.to_le_bytes());
        put(&mut block, OPT_SESSION, &[2]);
        put(&mut block, OPT_CAMPAIGN, &u32::MAX.to_le_bytes());
        put(&mut block, OPT_MAP, &80u32.to_le_bytes());
        let mut path = vec![0u8; OPT_PATH_LEN as usize];
        let name = br"scenarios\multi\ascension\ascension";
        path[..name.len()].copy_from_slice(name);
        put(&mut block, OPT_PATH, &path);
        put(&mut block, OPT_DIFFICULTY, &2u16.to_le_bytes());
        block
    }

    #[test]
    fn the_recorded_options_are_queued_byte_for_byte_with_the_mode_set_to_local() {
        let block = recorded_block();
        let req = Request::recorded(&block).unwrap();
        assert_eq!(
            (req.name.as_str(), req.kind, req.campaign, req.map, req.difficulty),
            ("ascension", Kind::Multiplayer, u32::MAX, 80, Difficulty::Heroic)
        );
        // Nothing is built under the block: it is written, then queued.
        assert_eq!(steps_for(&req), [Step::Fill, Step::Queue]);

        let mut m = Fake::default();
        assert!(fill_options(&mut m, BUFFER, &req));
        let mut got = vec![0; OPTIONS_SIZE as usize];
        assert!(m.rd(BUFFER, &mut got));
        let mut want = block.clone();
        want[OPT_SESSION as usize] = 1;
        assert_eq!(got, want);
        assert_eq!(
            got.iter().zip(&block).filter(|(a, b)| a != b).count(),
            1,
            "only the session kind byte differs from the recorded block"
        );

        // The agent API's steps carry the same bytes and queue them.
        let steps = game_steps(&req).unwrap();
        assert!(matches!(&steps[0], GameStep::Write { offset: 0, bytes } if *bytes == want));
        assert!(matches!(steps.last(), Some(GameStep::Call { target: OPTIONS_QUEUE, .. })));
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn a_campaign_blocks_session_byte_stays_as_the_recorded_game_had_it() {
        let mut block = recorded_block();
        block[OPT_MODE as usize] = MODE_CAMPAIGN as u8;
        block[OPT_SESSION as usize] = 0;
        assert_eq!(local_options(&block), block);
        assert_eq!(Request::recorded(&block).unwrap().kind, Kind::Campaign);
    }

    #[test]
    fn a_recorded_block_of_another_size_mode_or_no_scenario_is_refused() {
        let block = recorded_block();
        assert!(Request::recorded(&block[..block.len() - 1]).is_err());
        let mut longer = block.clone();
        longer.push(0);
        assert!(Request::recorded(&longer).is_err());
        let mut menu = block.clone();
        menu[OPT_MODE as usize] = MODE_MAINMENU as u8;
        assert!(Request::recorded(&menu).is_err());
        let mut nameless = block.clone();
        nameless[OPT_PATH as usize] = 0;
        assert!(Request::recorded(&nameless).is_err());
        let mut unterminated = block;
        unterminated[(OPT_PATH + OPT_PATH_LEN - 1) as usize] = b'x';
        unterminated[OPT_PATH as usize..(OPT_PATH + OPT_PATH_LEN) as usize].fill(b'x');
        assert!(Request::recorded(&unterminated).is_err());
    }

    #[test]
    fn difficulty_names_and_indices() {
        for (s, d, i) in [
            ("easy", Difficulty::Easy, 0),
            ("Normal", Difficulty::Normal, 1),
            ("HEROIC", Difficulty::Heroic, 2),
            ("legendary", Difficulty::Legendary, 3),
        ] {
            let p = Difficulty::parse(s).unwrap();
            assert_eq!((p, p.index()), (d, i), "{s}");
            assert_eq!(p.to_string(), s.to_ascii_lowercase());
        }
        assert_eq!(Difficulty::default(), Difficulty::Normal);
        assert!(Difficulty::parse("impossible").is_err());
        assert!(Difficulty::parse("").is_err());
    }

    #[test]
    fn fills_campaign_options_like_the_menu_start() {
        let mut m = Fake::default();
        defaulted(&mut m, BUFFER);
        // A stale longer path must not survive past the terminator.
        m.wr(BUFFER + OPT_PATH, &[b'x'; 0x103]);
        let r = request(Difficulty::Legendary);
        assert!(fill_options(&mut m, BUFFER, &r));
        assert_eq!(m.u32(BUFFER + OPT_MODE), Some(MODE_CAMPAIGN));
        assert_eq!(m.u32(BUFFER + OPT_CAMPAIGN), Some(1));
        assert_eq!(m.u32(BUFFER + OPT_MAP), Some(7));
        assert_eq!(path_at(&m, BUFFER + OPT_PATH), r.scenario);
        assert_eq!(m.u8(BUFFER + OPT_PATH + OPT_PATH_LEN - 1), Some(0), "the whole field is written");
        assert_eq!(m.u16(BUFFER + OPT_DIFFICULTY), Some(3));
        assert_eq!((m.u8(BUFFER + OPT_SPLIT), m.u8(BUFFER + OPT_SINGLE)), (Some(0), Some(1)));
        // Nothing past the fields we own: the next byte after the path
        // field and the tick rate keep the defaults.
        assert_eq!(m.u8(BUFFER + OPT_PATH + OPT_PATH_LEN), Some(0xcc));
        assert_eq!(m.u16(BUFFER + 6), Some(0xcccc));
        assert_eq!(m.u8(BUFFER + OPTIONS_SIZE - 1), Some(0xcc));
    }

    #[test]
    fn fills_a_local_multiplayer_game_like_the_session_start() {
        let mut m = Fake::default();
        defaulted(&mut m, BUFFER);
        let r = mp_request();
        assert!(fill_options(&mut m, BUFFER, &r));
        assert_eq!(m.u32(BUFFER + OPT_MODE), Some(MODE_MULTIPLAYER));
        assert_eq!(m.u8(BUFFER + OPT_SESSION), Some(1), "local");
        assert_eq!(m.u32(BUFFER + OPT_CAMPAIGN), Some(u32::MAX));
        assert_eq!(m.u32(BUFFER + OPT_MAP), Some(80));
        assert_eq!(path_at(&m, BUFFER + OPT_PATH), r.scenario);
        assert_eq!(m.u32(BUFFER + OPT_VARIANT_INDEX), Some(u32::MAX));
        // Campaign-only fields and the variant (built after) are untouched.
        assert_eq!(m.u16(BUFFER + OPT_DIFFICULTY), Some(0xcccc));
        assert_eq!((m.u8(BUFFER + OPT_SPLIT), m.u8(BUFFER + OPT_SINGLE)), (Some(0xcc), Some(0xcc)));
        assert_eq!(m.u8(BUFFER + OPT_VARIANT), Some(0xcc));
        assert_eq!(m.u8(BUFFER + OPT_VARIANT + VARIANT_SIZE - 1), Some(0xcc));
        assert_eq!(r.game(), "slayer");
        assert_eq!(request(Difficulty::Heroic).game(), "heroic");
    }

    #[test]
    fn the_steps_build_then_queue() {
        use Step::*;
        assert_eq!(steps(Kind::Campaign), [New, Fill, Players, Check, Queue]);
        // The variant goes into the cleared block; players after the mode
        // is written (OPTIONS_PLAYERS reads it).
        assert_eq!(steps(Kind::Multiplayer), [New, Fill, Variant, Players, Check, Queue]);
        for k in [Kind::Campaign, Kind::Multiplayer] {
            let s = steps(k);
            let at = |x| s.iter().position(|y| *y == x).unwrap();
            assert!(at(New) < at(Fill) && at(Fill) < at(Players) && at(Players) < at(Queue));
            assert_eq!(s.last(), Some(&Queue));
        }
    }

    #[test]
    fn each_step_calls_the_game_with_its_arguments() {
        let b = BUFFER;
        assert_eq!(
            call_of(Step::New, b),
            Some(GuestCall { target: OPTIONS_NEW, args: vec![], eax: None, esi: Some(b) })
        );
        assert_eq!(call_of(Step::Fill, b), None);
        assert_eq!(
            call_of(Step::Variant, b),
            Some(GuestCall {
                target: VARIANT_DEFAULTS,
                args: vec![b + OPT_VARIANT, GAME_TYPE_SLAYER],
                eax: None,
                esi: None
            })
        );
        assert_eq!(
            call_of(Step::Players, b),
            Some(GuestCall { target: OPTIONS_PLAYERS, args: vec![1, b], eax: None, esi: None })
        );
        assert_eq!(
            call_of(Step::Check, b),
            Some(GuestCall { target: OPTIONS_CHECK, args: vec![b], eax: None, esi: None })
        );
        assert_eq!(
            call_of(Step::Queue, b),
            Some(GuestCall { target: OPTIONS_QUEUE, args: vec![], eax: Some(b), esi: None })
        );
        // The variant fits in the options block.
        const { assert!(OPT_VARIANT + VARIANT_SIZE <= OPTIONS_SIZE) };
        const { assert!(OPT_VARIANT_INDEX + 4 == OPT_VARIANT) };
    }

    #[test]
    fn unknown_ids_stay_unset_and_long_paths_are_refused() {
        let mut m = Fake::default();
        defaulted(&mut m, BUFFER);
        let mut r = request(Difficulty::Easy);
        (r.campaign, r.map) = (u32::MAX, u32::MAX);
        assert!(fill_options(&mut m, BUFFER, &r));
        assert_eq!((m.u32(BUFFER + OPT_CAMPAIGN), m.u32(BUFFER + OPT_MAP)), (Some(u32::MAX), Some(u32::MAX)));
        assert_eq!(m.u16(BUFFER + OPT_DIFFICULTY), Some(0));
        r.scenario = "s".repeat(OPT_PATH_LEN as usize);
        let mut fresh = Fake::default();
        assert!(!fill_options(&mut fresh, BUFFER, &r));
        assert!(fresh.0.is_empty(), "nothing written");
    }

    /// Game globals at `g` whose options name `path` in `mode`.
    fn globals(m: &mut Fake, g: u32, mode: u32, path: &str, active: bool) {
        m.set_u32(GAME_GLOBALS, g);
        m.wr(g, &vec![0; 0x1200]);
        m.set_u32(g + GLOBALS_OPTIONS + OPT_MODE, mode);
        m.wr(g + GLOBALS_OPTIONS + OPT_PATH, path.as_bytes());
        m.wr(g + GLOBALS_ACTIVE, &[active as u8]);
    }

    #[test]
    fn reads_the_active_game_from_the_globals() {
        let mut m = Fake::default();
        assert_eq!(active_game(&m), None, "no globals pointer");
        m.set_u32(GAME_GLOBALS, 0);
        assert_eq!(active_game(&m), None, "null globals");
        let r = request(Difficulty::Normal);
        globals(&mut m, 0x0060_0000, MODE_CAMPAIGN, &r.scenario, false);
        assert_eq!(active_game(&m), None, "not active yet");
        m.wr(0x0060_0000 + GLOBALS_ACTIVE, &[1]);
        assert_eq!(active_game(&m), Some((MODE_CAMPAIGN, r.scenario.clone())));
    }

    #[test]
    fn waits_for_the_requested_scenario_or_any_non_menu_game() {
        let r = request(Difficulty::Normal);
        let menu = r"scenarios\ui\mainmenu\mainmenu";
        assert!(is_wanted(Some(&r), MODE_CAMPAIGN, &r.scenario));
        assert!(is_wanted(Some(&r), MODE_CAMPAIGN, &r.scenario.to_ascii_uppercase()));
        assert!(!is_wanted(Some(&r), MODE_MAINMENU, menu));
        assert!(!is_wanted(Some(&r), MODE_CAMPAIGN, r"scenarios\solo\01a_tutorial\01a_tutorial"));
        // Without a request (H2_PERF_FROM=level alone): the first game that
        // is not the main menu.
        assert!(!is_wanted(None, MODE_MAINMENU, menu));
        assert!(is_wanted(None, MODE_CAMPAIGN, r"scenarios\solo\00a_introduction\00a_introduction"));
        assert!(is_wanted(None, MODE_MULTIPLAYER, r"scenarios\multi\example\example"));
        let mp = mp_request();
        assert!(is_wanted(Some(&mp), MODE_MULTIPLAYER, &mp.scenario));
        assert!(!is_wanted(Some(&mp), MODE_MAINMENU, menu));
        assert_eq!(short(&r.scenario), "03a_oldmombasa");
    }

    #[test]
    fn perf_from_parses_presents_and_level() {
        assert_eq!(PerfFrom::parse("440"), Some(PerfFrom::Present(440)));
        assert_eq!(PerfFrom::parse(" level "), Some(PerfFrom::Level));
        assert_eq!(PerfFrom::parse("LEVEL"), Some(PerfFrom::Level));
        assert_eq!(PerfFrom::parse("menu"), None);
        assert_eq!(PerfFrom::parse(""), None);
        assert!(PerfFrom::Present(10).due(10, None));
        assert!(!PerfFrom::Present(10).due(9, Some(0)));
        // Level: never before the mark, and only from the next present.
        assert!(!PerfFrom::Level.due(5000, None));
        assert!(!PerfFrom::Level.due(830, Some(830)));
        assert!(PerfFrom::Level.due(831, Some(830)));
    }

    fn levels() -> Vec<DiscLevel> {
        let solo = ["01a_tutorial", "03a_oldmombasa", "00a_introduction"].into_iter().map(|n| DiscLevel {
            name: n.into(),
            scenario: format!(r"scenarios\solo\{n}\{n}"),
            kind: Kind::Campaign,
        });
        let multi = ["lockout", "ascension"].into_iter().map(|n| DiscLevel {
            name: n.into(),
            scenario: format!(r"scenarios\multi\{n}\{n}"),
            kind: Kind::Multiplayer,
        });
        solo.chain(multi).collect()
    }

    #[test]
    fn picks_a_level_by_file_name_or_scenario_path() {
        let l = levels();
        assert_eq!(pick(&l, "03a_oldmombasa").unwrap().name, "03a_oldmombasa");
        assert_eq!(pick(&l, "03A_OldMombasa.map").unwrap().name, "03a_oldmombasa");
        assert_eq!(pick(&l, r"scenarios\solo\01a_tutorial\01a_tutorial").unwrap().name, "01a_tutorial");
        let a = pick(&l, "Ascension").unwrap();
        assert_eq!((a.name.as_str(), a.kind), ("ascension", Kind::Multiplayer));
    }

    #[test]
    fn a_bad_name_lists_the_levels() {
        let e = pick(&levels(), "03a_oldmombassa").unwrap_err().to_string();
        assert!(e.contains("03a_oldmombassa"), "{e}");
        let listed: Vec<&str> = e.lines().skip(1).map(str::trim).collect();
        assert_eq!(
            listed,
            ["campaign:", "00a_introduction", "01a_tutorial", "03a_oldmombasa", "multiplayer:", "ascension", "lockout"],
            "by kind, sorted, one per line"
        );
        assert!(pick(&[], "x").unwrap_err().to_string().contains("no campaign or multiplayer maps"));
    }

    /// A synthetic globals tag: the campaign block at `+0x170` points at
    /// one element whose level block holds `entries`.
    fn matg(entries: &[(u32, u32, &str)]) -> (Vec<u8>, BTreeMap<u32, Vec<u8>>) {
        let mut tag = vec![0u8; 0x200];
        tag[MATG_CAMPAIGN..MATG_CAMPAIGN + 4].copy_from_slice(&1u32.to_le_bytes());
        tag[MATG_CAMPAIGN + 4..MATG_CAMPAIGN + 8].copy_from_slice(&0x1000u32.to_le_bytes());
        let mut head = Vec::new();
        head.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        head.extend_from_slice(&0x2000u32.to_le_bytes());
        let mut all = Vec::new();
        for (c, m, p) in entries {
            let mut e = vec![0u8; LEVEL_SIZE];
            e[0..4].copy_from_slice(&c.to_le_bytes());
            e[4..8].copy_from_slice(&m.to_le_bytes());
            e[LEVEL_PATH..LEVEL_PATH + p.len()].copy_from_slice(p.as_bytes());
            all.extend(e);
        }
        (tag, BTreeMap::from([(0x1000, head), (0x2000, all)]))
    }

    #[test]
    fn reads_campaign_and_map_ids_from_the_globals() {
        let (tag, mem) = matg(&[
            (1, 5, r"scenarios\solo\01a_tutorial\01a_tutorial"),
            (1, 9, r"scenarios\solo\03a_oldmombasa\03a_oldmombasa"),
        ]);
        let mut read = |a: u32, n: u32| mem.get(&a).filter(|b| b.len() >= n as usize).map(|b| b[..n as usize].to_vec());
        let all = campaign_levels(&tag, &mut read);
        assert_eq!(all.len(), 2);
        assert_eq!(ids_for(&all, r"SCENARIOS\solo\03a_oldmombasa\03a_oldmombasa"), Some((1, 9)));
        assert_eq!(ids_for(&all, r"scenarios\solo\05a_deltaapproach\05a_deltaapproach"), None);
    }

    /// A synthetic globals tag with a multiplayer list at `+0x178`.
    fn mp_matg(entries: &[(u32, &str)]) -> (Vec<u8>, BTreeMap<u32, Vec<u8>>) {
        let mut tag = vec![0u8; 0x200];
        tag[MATG_MULTIPLAYER..MATG_MULTIPLAYER + 4].copy_from_slice(&1u32.to_le_bytes());
        tag[MATG_MULTIPLAYER + 4..MATG_MULTIPLAYER + 8].copy_from_slice(&0x1000u32.to_le_bytes());
        let mut elem = vec![0u8; MP_BLOCK + 8];
        elem[MP_BLOCK..MP_BLOCK + 4].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        elem[MP_BLOCK + 4..MP_BLOCK + 8].copy_from_slice(&0x3000u32.to_le_bytes());
        let mut all = Vec::new();
        for (id, p) in entries {
            let mut e = vec![0u8; MP_LEVEL_SIZE];
            e[0..4].copy_from_slice(&id.to_le_bytes());
            e[MP_LEVEL_PATH..MP_LEVEL_PATH + p.len()].copy_from_slice(p.as_bytes());
            all.extend(e);
        }
        (tag, BTreeMap::from([(0x1000, elem), (0x3000, all)]))
    }

    #[test]
    fn reads_multiplayer_map_ids_from_the_globals() {
        let (tag, mem) =
            mp_matg(&[(80, r"scenarios\multi\ascension\ascension"), (81, r"scenarios\multi\lockout\lockout")]);
        let mut read = |a: u32, n: u32| mem.get(&a).filter(|b| b.len() >= n as usize).map(|b| b[..n as usize].to_vec());
        let all = multiplayer_levels(&tag, &mut read);
        assert_eq!(
            all,
            [
                (80, r"scenarios\multi\ascension\ascension".to_string()),
                (81, r"scenarios\multi\lockout\lockout".to_string())
            ]
        );
        // The campaign reader finds nothing in it, and the reverse.
        assert!(campaign_levels(&tag, &mut read).is_empty());
        let (ctag, cmem) = matg(&[(1, 5, "x")]);
        let mut cread =
            |a: u32, n: u32| cmem.get(&a).filter(|b| b.len() >= n as usize).map(|b| b[..n as usize].to_vec());
        assert!(multiplayer_levels(&ctag, &mut cread).is_empty());
    }

    #[test]
    fn a_missing_or_implausible_level_list_reads_as_empty() {
        let mut none = |_: u32, _: u32| None;
        assert!(campaign_levels(&[0; 0x10], &mut none).is_empty(), "short tag");
        assert!(campaign_levels(&vec![0; 0x200], &mut none).is_empty(), "empty block");
        let (tag, mut mem) = matg(&[(1, 1, "a")]);
        mem.get_mut(&0x1000).unwrap()[0..4].copy_from_slice(&(MAX_LEVELS + 1).to_le_bytes());
        let mut read = |a: u32, n: u32| mem.get(&a).filter(|b| b.len() >= n as usize).map(|b| b[..n as usize].to_vec());
        assert!(campaign_levels(&tag, &mut read).is_empty());
    }

    /// Against your disc: `H2_DISC=disc/halo2.iso cargo test -p boot --lib
    /// level_boot -- --ignored --nocapture`. Every campaign and multiplayer
    /// map resolves, with a map id from its level list.
    #[test]
    #[ignore = "reads the disc named by H2_DISC"]
    fn every_disc_level_resolves() {
        let disc = std::path::PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
        let levels = disc_levels(&disc).unwrap();
        assert!(!levels.is_empty());
        for l in &levels {
            let r = resolve(&disc, &l.name, Difficulty::Normal).unwrap();
            assert_eq!(r.kind, l.kind);
            eprintln!("{:?} {} {} campaign {} map {}", r.kind, r.name, r.scenario, r.campaign as i32, r.map as i32);
            assert_ne!(r.map, u32::MAX, "{} has no map id", r.name);
        }
    }

    /// `--level a30` with Halo 1 imported: the level of the installed
    /// campaign, with the ids the campaign hooks give its start record and
    /// the scenario path whose basename names the map the VFS serves.
    #[test]
    fn an_installed_campaign_level_resolves_by_map_or_scenario() {
        let m = content::campaign_list::CampaignManifest::parse(content::campaign_list::HALO1).unwrap();
        let retail: Vec<u32> = (0..15).map(|i| 0x65 + i).collect();
        let built = cl::build(&[m], &[1], &retail).unwrap();
        let r = campaign_request(&built, "A30.map", Difficulty::Heroic).unwrap();
        assert_eq!(
            r,
            Request {
                name: "a30".into(),
                scenario: r"scenarios\solo\a30\a30".into(),
                campaign: 2,
                map: 0x1002,
                difficulty: Difficulty::Heroic,
                kind: Kind::Campaign,
                variant: None,
                options: None,
            }
        );
        assert_eq!(campaign_request(&built, r"scenarios\solo\d40\d40", Difficulty::Easy).unwrap().map, 0x100a);
        assert!(campaign_request(&built, "03a_oldmombasa", Difficulty::Easy).is_none());
        assert!(r.scenario.len() < OPT_PATH_LEN as usize);
    }

    #[test]
    fn no_level_variable_means_no_request() {
        let disc = Path::new("/nonexistent/halo2.iso");
        assert_eq!(from_vars(disc, None, None, Some("heroic")).unwrap(), None);
        assert_eq!(from_vars(disc, None, Some("  "), None).unwrap(), None);
        // A bad difficulty is refused before the disc is read.
        let e = from_vars(disc, None, Some("03a_oldmombasa"), Some("hard")).unwrap_err().to_string();
        assert!(e.contains("unknown difficulty"), "{e}");
        assert!(from_vars(disc, None, Some("03a_oldmombasa"), None).is_err(), "no disc");
    }
}
