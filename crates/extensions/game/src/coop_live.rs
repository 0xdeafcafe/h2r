//! Co-op over Live (docs/live/live-coop.md, "Co-op over Live (built)"): the
//! Xbox Live menu's Co-op Campaign row, the host's co-op screen, its friends
//! list, the guest's join screen, and the arming of the retail synchronous
//! campaign path (`crate::coop_probe`) for these sessions only.
//!
//! **Host.** The row opens the co-op screen (our list class): Level (the
//! disc's campaign levels, named as the level select names them), Difficulty,
//! Invite a friend (a screen with one row per friend; a press sends a Live
//! game invitation to this console's lobby), Status and Start. Opening it
//! arms co-op: the HLE advertises the lobby as a friends-only `coop` session
//! naming the level and difficulty and carrying the host's campaign skulls,
//! presence says "Co-op: <level>", and the probe's hooks act (the peer-count
//! window on the session start, the peer limit, the joinable predicate, the
//! campaign setters on the lobby once a second machine is in). Start is
//! enabled when two machines are in and the checks pass: it puts the level
//! and difficulty on the session with the campaign menu's own setters and
//! closes the screen, back to the lobby, where the game starts the way a
//! System Link or Live game does. Another Live menu row disarms.
//!
//! **Guest.** A watcher asks the HLE which session this console joined (an
//! accepted invite, a friend's presence, the custom games browser) once its
//! squad session has a second machine; a `coop` session arms the hooks for
//! the guest, applies the host's skulls at the campaign start, and opens the
//! join screen (host, level, difficulty, skulls, handshake, Ready) at the
//! next tick (`H2_COOP_JOIN_SCREEN=0` leaves it to the Co-op Campaign row).
//!
//! **Text.** On the Live co-op path the retail lobby line that says co-op
//! needs two players on one console (string id [`COOP_TAKES_TWO`], the only
//! place the game uses it) reads ours instead ([`line_for`]): from the
//! Co-op Campaign row on, and on a friend's console in the host's lobby
//! before it knows it is a co-op guest. Alone, it says to invite a friend
//! first. Split screen keeps the game's own text. Every text of these screens is ours, written into guest
//! memory slots ([`Texts`]) when it changes.
//!
//! The state is pure ([`Lobby`]) and tested without a guest.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};


use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::Flow;
use live_proto::session::Campaign;
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use crate::coop_probe::{self, Arm, Want};
use crate::list_class::{Action, ListClass, Rows};
use crate::ui::Chosen;

/// The screens' ids (`list_class::loader_of`).
pub const SCREEN: &str = "coop";
pub const FRIENDS: &str = "coop.friends";
pub const JOIN: &str = "coop.join";
/// The feature id of the screens and texts (`H2_FEATURES=-coop-screens`).
pub const FEATURE: &str = "coop-screens";

/// The retail lobby line "co-op needs 2 players on a single console"
/// (`coop_takes_two` in the main menu's strings).
pub const COOP_TAKES_TWO: u32 = 0x0e00_01ec;
/// What it says in a co-op session over Live, when nothing more is known.
pub const LOBBY_LINE: &str = "Co-op over Xbox Live: 2 players, one per console.\r\n\
                              Press Y in the lobby to invite a friend.";
/// The host alone in its lobby with `coop.join_in_progress` off: Start
/// Game does nothing until a friend joins (`0x251050` acts only on start
/// state 9; there is no dialog).
pub const INVITE_FIRST: &str = "Invite a friend first: co-op over Xbox Live\r\n\
                                needs at least 2 consoles. Press Y for Friends.";
/// The host alone in its lobby with join in progress on: Start Game
/// starts ([`lift_solo_start`]), and friends join the running game
/// (docs/live/live-coop-join.md 13.4).
pub const SOLO_LINE: &str = "Start Game to play now; friends join in progress.\r\n\
                             Press Y in the lobby to invite a friend.";
/// A guest's lobby: the host starts.
pub const GUEST_LINE: &str = "Co-op over Xbox Live: one player per console.\r\n\
                              Waiting for the host to start the game.";
/// More players than consoles (someone signed in a second controller).
pub const ONE_PER_CONSOLE: &str = "Co-op over Xbox Live: one player per console.\r\n\
                                   Sign out the extra players to start.";

/// What the retail lobby line (start state 7 in co-op mode, `0x2500ab`)
/// says on a console in a Live co-op lobby, from what it knows: its role,
/// and the squad session the tick read (`None`: not read yet). Each line
/// fits the lobby's 52 characters.
pub fn line_for(role: &Role, s: Option<&coop_probe::Session>, max: u32) -> String {
    let host = match (role, s) {
        (Role::Guest(_), _) => false,
        (Role::Host, _) => true,
        (Role::Idle, Some(s)) => s.is_host(),
        (Role::Idle, None) => true,
    };
    if !host {
        return GUEST_LINE.into();
    }
    match s {
        Some(s) if s.peers < 2 => INVITE_FIRST.into(),
        Some(s) if s.peers > max => format!(
            "Co-op over Xbox Live takes up to {max} consoles;\r\n{} are in this lobby.",
            s.peers
        ),
        Some(s) if s.players > s.peers => ONE_PER_CONSOLE.into(),
        _ => lobby_line(max),
    }
}

/// [`line_for`], but a host alone with the solo start on (`solo`,
/// [`Arm::solo_start`]) reads [`SOLO_LINE`].
pub fn host_line(role: &Role, s: Option<&coop_probe::Session>, max: u32, solo: bool) -> String {
    let line = line_for(role, s, max);
    if solo && line == INVITE_FIRST {
        return SOLO_LINE.into();
    }
    line
}

/// The lobby line for a co-op limit of `max` players: [`LOBBY_LINE`] for
/// two, else "up to `max`". Each line fits the lobby's 52 characters.
pub fn lobby_line(max: u32) -> String {
    if max <= coop_probe::MAX_PLAYERS {
        return LOBBY_LINE.into();
    }
    format!(
        "Xbox Live co-op: up to {max} players, one per console.\r\nPress Y in the lobby to invite friends."
    )
}

/// Halo 2's own campaign id.
const RETAIL_CAMPAIGN: u32 = 1;

// ---- the disc's campaign levels ----

/// A campaign level the co-op screen offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Level {
    /// The map file stem, `03a_oldmombasa`.
    pub stem: String,
    pub campaign: u32,
    pub map: u32,
    /// The level select's name for it (English), read from the disc.
    pub name: Option<String>,
}

impl Level {
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.stem)
    }
}

/// `matg +0x178`: the level select's element; its block `+8` holds
/// `0xb50`-byte level records: `+0` campaign id, `+4` map id, `+0x10` nine
/// names of 32 UTF-16 units, English first (docs/halo1/campaign-picker.md).
const MATG_UI: usize = 0x178;
const UI_LEVELS: usize = 8;
const UI_LEVEL_LEN: u32 = 0xb50;
const UI_LEVEL_NAME: usize = 0x10;
const NAME_UNITS: usize = 32;
const MAX_LEVELS: u32 = 256;

fn le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

/// `(campaign, map, name)` of each level select record of a globals tag.
pub fn level_names(
    matg: &[u8],
    read: &mut dyn FnMut(u32, u32) -> Option<Vec<u8>>,
) -> Vec<(u32, u32, String)> {
    let (Some(n), Some(el)) = (le(matg, MATG_UI), le(matg, MATG_UI + 4)) else {
        return Vec::new();
    };
    if n == 0 {
        return Vec::new();
    }
    let Some(head) = read(el, (UI_LEVELS + 8) as u32) else {
        return Vec::new();
    };
    let (Some(count), Some(at)) = (le(&head, UI_LEVELS), le(&head, UI_LEVELS + 4)) else {
        return Vec::new();
    };
    if count == 0 || count > MAX_LEVELS {
        return Vec::new();
    }
    let Some(all) = read(at, count * UI_LEVEL_LEN) else {
        return Vec::new();
    };
    all.chunks(UI_LEVEL_LEN as usize)
        .filter_map(|e| {
            let units: Vec<u16> = e
                .get(UI_LEVEL_NAME..UI_LEVEL_NAME + 2 * NAME_UNITS)?
                .chunks(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|u| *u != 0)
                .collect();
            let name = String::from_utf16_lossy(&units).trim().to_owned();
            Some((le(e, 0)?, le(e, 4)?, name))
        })
        .collect()
}

/// The co-op levels: Halo 2's campaign levels in start-list order (`start`,
/// `(campaign, map, scenario path)`), each with its level select name.
pub fn levels(start: &[(u32, u32, String)], names: &[(u32, u32, String)]) -> Vec<Level> {
    start
        .iter()
        .filter(|(c, _, _)| *c == RETAIL_CAMPAIGN)
        .filter_map(|(c, m, path)| {
            let stem = path.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
            (!stem.is_empty()).then(|| Level {
                stem,
                campaign: *c,
                map: *m,
                name: names
                    .iter()
                    .find(|(nc, nm, n)| nc == c && nm == m && !n.is_empty())
                    .map(|(_, _, n)| n.clone()),
            })
        })
        .collect()
}

/// The disc's co-op levels, from `mainmenu.map`'s globals. `disc` is a
/// disc image, or a directory of the disc's files: the trimmed disc the
/// Xbox and Apple apps carry, which the game reads through
/// `vfs::DirDiscBackend`.
pub fn disc_levels(disc: &Path) -> anyhow::Result<Vec<Level>> {
    use anyhow::Context;
    let mut m = open_mainmenu(disc)?;
    let tag = m
        .globals()
        .map(|t| t.id)
        .context("mainmenu.map has no globals tag")?;
    let matg = m.read_tag_bytes(tag)?;
    let mut read = |addr: u32, size: u32| -> Option<Vec<u8>> {
        let off = m.file_offset(addr, size).ok()?;
        m.read_file_range(off, size.into()).ok()
    };
    let start = crate::level_boot::campaign_levels(&matg, &mut read);
    let names = level_names(&matg, &mut read);
    Ok(levels(&start, &names))
}

/// `mainmenu.map` on the disc, an image or a directory.
///
/// A directory is read file by file, as `vfs::DirDiscBackend` reads it.
/// It is never opened itself: in the Xbox app's container, opening a
/// directory handle is refused (ERROR_ACCESS_DENIED), even for the app's
/// own package, while listing it and opening its files work
/// (docs/runtime/xbox.md, "Files").
fn open_mainmenu(disc: &Path) -> anyhow::Result<map::Map<std::io::BufReader<std::fs::File>>> {
    use anyhow::Context;
    if disc.is_dir() {
        let path = dir_disc_file(disc, &["maps", "mainmenu.map"])
            .with_context(|| format!("the disc {} has no maps/mainmenu.map", disc.display()))?;
        let file =
            std::fs::File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata()?.len();
        return map::Map::from_region(std::io::BufReader::new(file), 0, len)
            .context("parsing mainmenu.map");
    }
    let open = || std::fs::File::open(disc).with_context(|| format!("opening {}", disc.display()));
    let mut iso = xiso::Xiso::open(std::io::BufReader::new(open()?)).context("reading the disc")?;
    let base = iso.partition_offset();
    let e = iso
        .entries()?
        .iter()
        .find(|e| e.path.to_ascii_lowercase().ends_with("maps/mainmenu.map"))
        .cloned()
        .context("the disc has no mainmenu.map")?;
    map::Map::from_region(
        std::io::BufReader::new(open()?),
        base + u64::from(e.sector) * xiso::SECTOR,
        e.size.into(),
    )
    .context("parsing mainmenu.map")
}

/// The file at `parts` under the disc directory `root`, each name matched
/// without regard to case, as on the disc (`vfs::DirDiscBackend`). Each
/// directory is listed (`read_dir`), never opened.
pub fn dir_disc_file(root: &Path, parts: &[&str]) -> Option<std::path::PathBuf> {
    let mut at = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        let e = std::fs::read_dir(&at)
            .ok()?
            .flatten()
            .find(|e| e.file_name().eq_ignore_ascii_case(part))?;
        at = e.path();
        let last = i + 1 == parts.len();
        if at.is_file() != last {
            return None;
        }
    }
    Some(at)
}

// ---- texts that change ----

/// Our dynamic string ids: an index range no retail string uses and no
/// other source of ours claims. The localized-string lookup the text
/// widgets go through matches some private ranges on the low 24 bits only
/// (`community_variants` takes `0x??ffe0xx`, the layouts' family
/// `0x??fff0xx`), so the index differs from theirs, not just the top byte:
/// `0x20ffe000` showed the community variants' names in place of ours.
pub const TEXT_ID_BASE: u32 = TEXT_FAMILY.base;
pub const TEXT_SLOTS: u32 = TEXT_FAMILY.len;
const TEXT_FAMILY: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("co-op texts");
pub const TEXT_SLOT_LEN: u32 = 128;

/// Text slots in guest memory: a string id per slot, its text written when
/// it changes (before a row's texts are set, and every tick).
#[derive(Default)]
pub struct Texts {
    base: AtomicU32,
    slots: Mutex<Vec<(Vec<u8>, bool)>>,
}

impl Texts {
    pub fn id(slot: u32) -> u32 {
        TEXT_ID_BASE + slot
    }
    /// Sets slot `slot`'s text (cut to fit, NUL-terminated UTF-8, as the
    /// locale blob stores strings); its string id.
    pub fn set(&self, slot: u32, text: &str) -> u32 {
        assert!(slot < TEXT_SLOTS, "text slot {slot}");
        let mut cut = text.len().min(TEXT_SLOT_LEN as usize - 1);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut b = text.as_bytes()[..cut].to_vec();
        b.push(0);
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if slots.len() < TEXT_SLOTS as usize {
            slots.resize(TEXT_SLOTS as usize, (vec![0], false));
        }
        let s = &mut slots[slot as usize];
        if s.0 != b {
            *s = (b, true);
        }
        Self::id(slot)
    }
    /// Whether `id` is one of the slots' ids.
    pub fn owns(id: u32) -> bool {
        id.checked_sub(TEXT_ID_BASE)
            .is_some_and(|slot| slot < TEXT_SLOTS)
    }
    /// The guest address of string `id`, when it is one of ours.
    pub fn address(&self, id: u32) -> Option<u32> {
        let base = self.base.load(Ordering::SeqCst);
        (base != 0 && Self::owns(id)).then(|| base + (id - TEXT_ID_BASE) * TEXT_SLOT_LEN)
    }
    /// Writes every changed slot.
    pub fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let base = self.base.load(Ordering::SeqCst);
        if base == 0 {
            return Ok(());
        }
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        for (i, (b, dirty)) in slots.iter_mut().enumerate() {
            if *dirty {
                m.write(base + i as u32 * TEXT_SLOT_LEN, b)?;
                *dirty = false;
            }
        }
        Ok(())
    }
    /// The text in slot `slot`, for tests and logs.
    pub fn get(&self, slot: u32) -> Option<String> {
        let slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let b = &slots.get(slot as usize)?.0;
        Some(String::from_utf8_lossy(&b[..b.len().saturating_sub(1)]).into_owned())
    }
}

/// Slot layout: a screen's title, then a label and a value per row.
const HOST_SLOTS: u32 = 0;
const FRIEND_SLOTS: u32 = 16;
const JOIN_SLOTS: u32 = 64;
const LINE_SLOT: u32 = 96;
/// The three screens' footer legends: host, friends, join.
const FOOTER_SLOTS: [u32; 3] = [97, 98, 99];

/// A co-op screen's button legend, for its footer (as the game's own
/// screens give theirs, docs/game/ui-screens.md): Start (or X) for the
/// host or guest action, and B back. List rows use the native A action.
pub fn prompts(screen: &str) -> Vec<crate::ui_kit::input::Prompt> {
    use crate::ui_kit::input::{Glyph, Prompt};
    let action = match screen {
        SCREEN => Some("Start Game"),
        JOIN => Some("Ready"),
        _ => None,
    };
    let mut p = Vec::new();
    if let Some(a) = action {
        p.push(Prompt::new(&[Glyph::Start], a));
    }
    p.push(Prompt::new(&[Glyph::B], "Back"));
    p
}

/// Screen `screen`'s footer legend's string id, its text set.
fn footer_id(texts: &Texts, screen: &str) -> u32 {
    let slot = match screen {
        SCREEN => FOOTER_SLOTS[0],
        FRIENDS => FOOTER_SLOTS[1],
        _ => FOOTER_SLOTS[2],
    };
    texts.set(slot, &crate::ui_kit::input::footer_legend(&prompts(screen)))
}
/// Rows a friends screen lists at most.
pub const MAX_FRIENDS: usize = 20;

fn title_slot(base: u32) -> u32 {
    base
}
fn label_slot(base: u32, row: i16) -> u32 {
    base + 1 + 2 * row as u32
}
fn value_slot(base: u32, row: i16) -> u32 {
    base + 2 + 2 * row as u32
}

/// A screen of ours as text for the log: its title and each row's texts,
/// with their string ids.
fn describe(rows: &dyn Rows, texts: &Texts, count: usize) -> String {
    let text = |id: u32| {
        let t = id
            .checked_sub(TEXT_ID_BASE)
            .and_then(|slot| texts.get(slot))
            .unwrap_or_else(|| "?".into());
        format!("{t:?} ({id:#010x})")
    };
    let mut out = format!("title {}; rows:", text(rows.title()));
    for row in 0..count.min(i16::MAX as usize) as i16 {
        let cells: Vec<String> = rows
            .texts(row)
            .into_iter()
            .map(|(_, id)| text(id))
            .collect();
        out.push_str(&format!(" [{row}] {}", cells.join(" = ")));
    }
    out
}

// ---- the lobby ----

pub const DIFFICULTIES: [&str; 4] = ["Easy", "Normal", "Heroic", "Legendary"];

/// This console's part in a co-op session.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    #[default]
    Idle,
    /// Hosting from the co-op screen.
    Host,
    /// Joined a host's co-op session.
    Guest(Guest),
}

/// What a guest knows of the session it joined.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Guest {
    pub session: String,
    pub host: String,
    pub campaign: Campaign,
    /// The host's skulls: engine indices, then ours from
    /// [`crate::skulls::COUNT`] (the wire's numbering).
    pub skulls: Vec<u8>,
    /// The host's XNet endpoint, for pings: its LAN address when the
    /// session read gave one (`Session::lan`), else as the server saw it.
    pub endpoint: Option<std::net::SocketAddrV4>,
}

/// What the host's screen checks before Start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checks {
    /// The squad session is in a lobby or game state.
    pub session: bool,
    /// The co-op player limit (`coop.max_players`), one per console.
    pub max: u32,
    pub peers: u32,
    pub players: u32,
    /// The level resolved with campaign and map ids.
    pub level: bool,
    /// The session holds the level and difficulty (the setters took).
    pub holds: bool,
}

impl Checks {
    /// Why Start is not enabled yet, if it is not.
    pub fn blocker(&self) -> Option<String> {
        if !self.level {
            return Some("this level has no ids on the disc".into());
        }
        if !self.session {
            return Some("no lobby yet".into());
        }
        if self.peers < 2 {
            return Some("waiting for a second console".into());
        }
        let max = self.max.max(coop_probe::MAX_PLAYERS);
        let takes = if max == coop_probe::MAX_PLAYERS {
            "co-op takes 2".to_owned()
        } else {
            format!("co-op takes up to {max}")
        };
        if self.peers > max {
            return Some(format!("{} consoles in the lobby; {takes}", self.peers));
        }
        if self.players > max {
            return Some(format!("{} players; {takes}", self.players));
        }
        None
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Inner {
    level: usize,
    difficulty: u8,
    role: Role,
    session: Option<coop_probe::Session>,
    #[serde(skip)]
    live_network: bool,
    /// Friends invited from the screen, by gamertag, oldest first.
    invited: Vec<String>,
    /// The friends screen's rows, as of its opening.
    friends: Vec<live_hle::Friend>,
    /// The guest's join screen is due.
    open_join: bool,
    /// The signed-in profiles, as the tick last read them.
    profiles: Vec<String>,
    /// The last round trip between host and guest, in ms.
    ping: Option<u16>,
    /// The last start state logged: (retail, ours).
    start_state: Option<(u32, u32)>,
    /// What the retail lobby's host last advertised: (campaign, map,
    /// difficulty) of the session.
    advertised: Option<(u32, u32, u16)>,
}

/// What the HLE does for the lobby; a trait so tests run without one.
pub trait Live: Send + Sync {
    fn set_coop(&self, campaign: Option<Campaign>, skulls: Option<Vec<u8>>);
    fn friends(&self) -> Vec<live_hle::Friend>;
    fn invite(&self, to: live_proto::Xuid) -> Result<String, u32>;
    fn joined(&self) -> live_hle::Joined;
    /// Guest: one ping to the host, reporting our last round trip.
    fn ping_host(&self, host: std::net::SocketAddrV4, report: Option<u16>) -> Option<u16>;
    /// Host: the guest's last reported round trip (answering its pings).
    fn guest_ping(&self) -> Option<u16>;
    /// Host: the round trip its co-op session advertises.
    fn set_coop_ping(&self, ms: Option<u16>);
    fn sync_squad_slots(&self, slots: live_hle::SessionSlots);
}

impl Live for live_hle::Hle {
    fn set_coop(&self, campaign: Option<Campaign>, skulls: Option<Vec<u8>>) {
        live_hle::Hle::set_coop(self, campaign, skulls)
    }
    fn friends(&self) -> Vec<live_hle::Friend> {
        live_hle::Hle::friends(self, 0)
    }
    fn invite(&self, to: live_proto::Xuid) -> Result<String, u32> {
        self.invite_to_lobby(0, to)
    }
    fn joined(&self) -> live_hle::Joined {
        live_hle::Hle::joined(self, std::time::Duration::from_millis(500))
    }
    fn ping_host(&self, host: std::net::SocketAddrV4, report: Option<u16>) -> Option<u16> {
        live_hle::Hle::ping_host(self, host, report, PING_TIMEOUT)
    }
    fn guest_ping(&self) -> Option<u16> {
        live_hle::Hle::guest_ping(self, PING_FRESH)
    }
    fn sync_squad_slots(&self, slots: live_hle::SessionSlots) {
        live_hle::Hle::sync_squad_slots(self, slots)
    }
    fn set_coop_ping(&self, ms: Option<u16>) {
        live_hle::Hle::set_coop_ping(self, ms)
    }
}

/// Seats a new console could take in the active Live squad, bounded
/// by both player slots and peer slots (each console needs a peer).
fn squad_slots(s: coop_probe::Session, coop_limit: Option<u32>) -> Option<live_hle::SessionSlots> {
    if !s.live()
        || !s.is_host()
        || s.class == 0
        || s.local_peer >= coop_probe::CEILING
        || !(1..=coop_probe::CEILING).contains(&s.max_players)
        || s.players == 0
        || s.players > s.max_players
        || !(1..=coop_probe::CEILING).contains(&s.max_peers)
        || s.peers == 0
        || s.peers > s.max_peers
    {
        return None;
    }
    let limit = coop_limit.unwrap_or(coop_probe::CEILING);
    let open_players = s.max_players.min(limit).saturating_sub(s.players);
    let open_peers = s.max_peers.min(limit).saturating_sub(s.peers);
    Some(live_hle::SessionSlots {
        public_filled: s.players,
        public_open: open_players.min(open_peers),
        ..Default::default()
    })
}

/// Ping (docs/live/live-coop.md, "Ping"): the guest pings the host's QoS
/// responder every [`PING_EVERY`], waiting up to [`PING_TIMEOUT`]; each
/// ping carries its last round trip, so the host shows the same number; a
/// report older than [`PING_FRESH`] is gone (no guest).
pub const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
pub const PING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
pub const PING_FRESH: std::time::Duration = std::time::Duration::from_secs(6);

/// The co-op state of this console, shared by the screens, the hooks and
/// the guest watcher.
/// The host's campaign skulls for these profiles.
pub type HostSkulls = Box<dyn Fn(&[String]) -> Vec<u8> + Send + Sync>;

pub struct Lobby {
    levels: Vec<Level>,
    inner: Mutex<Inner>,
    arm: Option<Arc<Arm>>,
    live: Option<Arc<dyn Live>>,
    /// The host's campaign skulls (engine indices) for its signed-in
    /// profiles, read at arming.
    host_skulls: HostSkulls,
    pub texts: Arc<Texts>,
}

impl Lobby {
    pub fn new(
        levels: Vec<Level>,
        arm: Option<Arc<Arm>>,
        live: Option<Arc<dyn Live>>,
        host_skulls: HostSkulls,
    ) -> Lobby {
        Lobby {
            levels,
            inner: Mutex::default(),
            arm,
            live,
            host_skulls,
            texts: Arc::default(),
        }
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn levels(&self) -> &[Level] {
        &self.levels
    }
    pub fn role(&self) -> Role {
        self.lock().role.clone()
    }
    /// Hosting or joined: the co-op hooks and text are on.
    pub fn in_coop(&self) -> bool {
        self.lock().role != Role::Idle
    }
    pub fn level(&self) -> Option<&Level> {
        self.levels.get(self.lock().level)
    }
    pub fn difficulty(&self) -> u8 {
        self.lock().difficulty
    }
    /// The campaign the host advertises.
    pub fn campaign(&self) -> Option<Campaign> {
        let (l, d) = {
            let i = self.lock();
            (i.level, i.difficulty)
        };
        let level = self.levels.get(l)?;
        Some(Campaign {
            level: level.stem.clone(),
            name: level.name.clone(),
            difficulty: d,
        })
    }
    /// The level the host's setters put on the session.
    pub fn want(&self) -> Option<Want> {
        let (l, d) = {
            let i = self.lock();
            (i.level, i.difficulty)
        };
        let level = self.levels.get(l)?;
        (level.campaign != u32::MAX && level.map != u32::MAX).then(|| Want {
            name: level.stem.clone(),
            campaign: level.campaign,
            map: level.map,
            difficulty: u16::from(d),
        })
    }
    /// Pushes the host's choice to the HLE and the hooks.
    fn publish(&self) {
        if self.lock().role != Role::Host {
            return;
        }
        if let Some(arm) = &self.arm {
            arm.arm(self.want());
        }
        if let Some(live) = &self.live {
            let profiles = self.lock().profiles.clone();
            let skulls = (self.host_skulls)(&profiles);
            live.set_coop(self.campaign(), (!skulls.is_empty()).then_some(skulls));
        }
    }
    /// The Co-op Campaign row: host (unless joined to someone's co-op
    /// session). The screen to open.
    pub fn open_from_menu(&self) -> &'static str {
        {
            let mut i = self.lock();
            if matches!(i.role, Role::Guest(_)) {
                return JOIN;
            }
            if i.role != Role::Host {
                i.role = Role::Host;
                eprintln!("coop-live: hosting a co-op campaign");
            }
        }
        self.publish();
        SCREEN
    }
    /// Another Live menu row: no longer hosting a co-op campaign.
    pub fn leave(&self) {
        {
            let mut i = self.lock();
            if i.role != Role::Host {
                return;
            }
            i.role = Role::Idle;
            i.invited.clear();
        }
        eprintln!("coop-live: left the co-op campaign; the lobby is a custom game again");
        if let Some(arm) = &self.arm {
            arm.disarm();
        }
        if let Some(live) = &self.live {
            live.set_coop(None, None);
        }
    }
    pub fn step_level(&self) -> String {
        {
            let mut i = self.lock();
            if self.levels.is_empty() {
                return "coop-live: the disc has no campaign levels".into();
            }
            i.level = (i.level + 1) % self.levels.len();
        }
        self.publish();
        format!(
            "coop-live: level {}",
            self.level().map_or("?", Level::label)
        )
    }
    pub fn step_difficulty(&self) -> String {
        let d = {
            let mut i = self.lock();
            i.difficulty = (i.difficulty + 1) % DIFFICULTIES.len() as u8;
            i.difficulty
        };
        self.publish();
        format!("coop-live: difficulty {}", DIFFICULTIES[usize::from(d)])
    }
    /// The squad session as the tick last read it.
    pub fn observe(&self, s: Option<coop_probe::Session>) {
        self.observe_network(s, false);
    }
    fn observe_network(&self, s: Option<coop_probe::Session>, live_network: bool) {
        let mut inner = self.lock();
        inner.session = s.filter(coop_probe::Session::live);
        inner.live_network = live_network;
    }
    /// The signed-in profiles as the tick last read them.
    pub fn set_profiles(&self, p: Vec<String>) {
        let mut i = self.lock();
        if i.profiles != p {
            i.profiles = p;
        }
    }
    pub fn session(&self) -> Option<coop_probe::Session> {
        self.lock().session
    }
    pub fn checks(&self) -> Checks {
        let want = self.want();
        let s = self.lock().session;
        Checks {
            session: s.is_some(),
            max: self.max_players(),
            peers: s.map_or(0, |s| s.peers),
            players: s.map_or(0, |s| s.players),
            level: want.is_some(),
            holds: match (s, &want) {
                (Some(s), Some(w)) => {
                    s.is_campaign()
                        && s.campaign == w.campaign
                        && s.map == w.map
                        && s.difficulty == w.difficulty
                }
                _ => false,
            },
        }
    }
    /// The host's status line.
    pub fn status(&self) -> String {
        let c = self.checks();
        let invited = self.lock().invited.last().cloned();
        if c.peers >= 2 {
            let who = invited.unwrap_or_else(|| "A friend".into());
            return match c.blocker() {
                None if c.holds => format!("{who} is in; the lobby holds the level"),
                None => format!("{who} is in; ready"),
                Some(b) => b,
            };
        }
        match invited {
            Some(g) => format!("Waiting for {g}"),
            None => "Invite a friend".into(),
        }
    }
    /// The friends screen opens: its rows (online friends first).
    pub fn snapshot_friends(&self) -> usize {
        let friends = || {
            let mut rows = self.live.as_ref().map(|l| l.friends()).unwrap_or_default();
            rows.truncate(MAX_FRIENDS);
            rows
        };
        let f = friends();
        let n = f.len();
        self.lock().friends = f;
        n
    }
    pub fn friend(&self, row: i16) -> Option<live_hle::Friend> {
        self.lock().friends.get(usize::try_from(row).ok()?).cloned()
    }
    pub fn invited(&self, gamertag: &str) -> bool {
        self.lock().invited.iter().any(|g| g == gamertag)
    }
    /// A press on a friend's row: invite them to the lobby.
    pub fn invite(&self, row: i16) -> String {
        let Some(f) = self.friend(row) else {
            return "coop-live: no such friend".into();
        };
        if !f.online {
            return format!("coop-live: {} is offline; not invited", f.gamertag);
        }
        let Some(live) = &self.live else {
            return "coop-live: Live is off; no invitation sent".into();
        };
        let result = live.invite(f.xuid);
        match result {
            Ok(session) => {
                let mut i = self.lock();
                i.invited.retain(|g| *g != f.gamertag);
                i.invited.push(f.gamertag.clone());
                format!("coop-live: invited {} to session {session}", f.gamertag)
            }
            Err(e) => format!(
                "coop-live: could not invite {} ({e:#010x}); is the lobby advertised yet?",
                f.gamertag
            ),
        }
    }
    /// What the guest watcher read; whether this console just became a co-op
    /// guest.
    pub fn joined(&self, j: &live_hle::Joined) -> bool {
        self.apply_joined(Self::joined_guest(j))
    }

    fn joined_guest(j: &live_hle::Joined) -> Option<Guest> {
        match (j, j.campaign()) {
            (live_hle::Joined::Known { session, .. }, Some(c)) => Some(Guest {
                session: session.session_id.clone(),
                host: session.host_gamertag.clone(),
                campaign: c.clone(),
                skulls: session.advertisement.skulls.clone().unwrap_or_default(),
                // The host's LAN address when this console is behind its
                // NAT, else its public endpoint.
                endpoint: session.reach_order().first().copied(),
            }),
            _ => None,
        }
    }

    fn apply_joined(&self, guest: Option<Guest>) -> bool {
        let guest = match guest {
            Some(guest) => guest,
            None => {
                // Not (or no longer) in a co-op session.
                let was = matches!(self.lock().role, Role::Guest(_));
                if was {
                    self.lock().role = Role::Idle;
                    if let Some(arm) = &self.arm {
                        arm.disarm();
                    }
                    eprintln!("coop-live: guest: no longer in a co-op session");
                }
                return false;
            }
        };
        let mut i = self.lock();
        if i.role == Role::Host {
            return false;
        }
        let new = !matches!(&i.role, Role::Guest(g) if g.session == guest.session);
        if new {
            eprintln!(
                "coop-live: guest: joined {}'s co-op session {}: {} on {}{}",
                guest.host,
                guest.session,
                guest.campaign.label(),
                guest.campaign.difficulty_name(),
                if guest.skulls.is_empty() {
                    String::new()
                } else {
                    format!(", skulls {:?}", guest.skulls)
                }
            );
            i.open_join = true;
        }
        i.role = Role::Guest(guest);
        drop(i);
        if let Some(arm) = &self.arm {
            arm.arm(None);
        }
        new
    }
    /// The join screen is due (once).
    pub fn take_open_join(&self) -> bool {
        std::mem::take(&mut self.lock().open_join)
    }
    /// The guest's handshake line.
    pub fn handshake(&self) -> String {
        let (role, s) = {
            let i = self.lock();
            (i.role.clone(), i.session)
        };
        let Role::Guest(g) = role else {
            return "Not in a co-op session".into();
        };
        let Some(s) = s else {
            return "No lobby".into();
        };
        if s.peers != 2 {
            return format!("{} consoles in the lobby", s.peers);
        }
        if !s.is_campaign() {
            return "Waiting for the host's campaign".into();
        }
        if s.difficulty != u16::from(g.campaign.difficulty) {
            return "The lobby's difficulty differs".into();
        }
        "OK: the lobby holds the host's campaign".into()
    }
    /// The locale source: our text slots, and the retail co-op lobby line
    /// (co-op needs two players on one console) replaced by ours on the
    /// Live co-op path ([`Lobby::line_ours`]); split screen keeps the
    /// game's own.
    pub fn string(&self, id: u32) -> Option<u32> {
        if id == COOP_TAKES_TWO {
            return self
                .line_ours()
                .then(|| self.texts.address(Texts::id(LINE_SLOT)))?;
        }
        self.texts.address(id)
    }
    /// Whether the retail co-op lobby line is ours: hosting (from the
    /// moment the Co-op Campaign row is chosen) or joined, or, with Live
    /// on, in a networked squad session (class not 0) before this console
    /// knows it is a co-op guest: a friend who accepted an invitation sits
    /// in the host's co-op lobby before the host's advertisement says
    /// co-op. Split screen (no session, or class 0) keeps the game's text.
    pub fn line_ours(&self) -> bool {
        let i = self.lock();
        i.role != Role::Idle || (self.live.is_some() && i.session.is_some_and(|s| s.class != 0))
    }
    /// The lobby line for now ([`host_line`]), into its slot; written to
    /// the guest at the tick.
    pub fn refresh_line(&self) {
        let (role, s) = {
            let i = self.lock();
            (i.role.clone(), i.session)
        };
        self.texts.set(
            LINE_SLOT,
            &host_line(&role, s.as_ref(), self.max_players(), self.solo_start()),
        );
    }
    /// A host may start alone, friends joining in progress
    /// ([`Arm::solo_start`]; false without hooks).
    pub fn solo_start(&self) -> bool {
        self.arm.as_ref().is_some_and(|a| a.solo_start())
    }
    /// The texts' guest address (the feature's allocation).
    pub fn set_text_base(&self, base: u32) {
        self.texts.base.store(base, Ordering::SeqCst);
        self.texts.set(LINE_SLOT, &lobby_line(self.max_players()));
    }
    pub fn guest(&self) -> Option<Guest> {
        match &self.lock().role {
            Role::Guest(g) => Some(g.clone()),
            _ => None,
        }
    }
    /// The host's co-op player limit (`coop.max_players`; 2 without hooks).
    pub fn max_players(&self) -> u32 {
        self.arm
            .as_ref()
            .map_or(coop_probe::MAX_PLAYERS, |a| a.max_players())
    }
    /// The limit the lobby's checks use for session `s` on this console
    /// ([`Arm::limit_for`]).
    pub fn limit_for(&self, s: &coop_probe::Session) -> u32 {
        self.arm
            .as_ref()
            .map_or(coop_probe::MAX_PLAYERS, |a| a.limit_for(s))
    }
    /// Why the co-op screens can't be snapshotted now
    /// (docs/runtime/snapshots.md): hosting or joined (the role, the
    /// invites, the friends read and the ping are host state shaped by
    /// Live; the texts the tick writes follow them), the join screen is
    /// due, or the hooks are armed. Idle, the level and difficulty chosen
    /// are menu state no screen shows until the next hosting.
    pub fn snapshot_live(&self) -> Option<String> {
        let i = self.lock();
        match &i.role {
            Role::Host => return Some("hosting a co-op campaign".into()),
            Role::Guest(g) => return Some(format!("joined {}'s co-op campaign", g.host)),
            Role::Idle => {}
        }
        if i.open_join {
            return Some("the co-op join screen is due".into());
        }
        drop(i);
        self.arm.as_ref().and_then(|a| a.snapshot_live())
    }
    /// The co-op hooks are on.
    pub fn armed(&self) -> bool {
        self.arm.as_ref().is_some_and(|a| a.active())
    }
    /// What the start state hook answered for a lobby of `consoles`;
    /// logged when it changes.
    pub fn note_start_state(&self, retail: u32, ours: u32, consoles: u32) {
        if let Some(line) = self.start_state_line(retail, ours, consoles) {
            eprintln!("{line}");
        }
    }
    /// The log line for a start state answer, when it changed.
    pub fn start_state_line(&self, retail: u32, ours: u32, consoles: u32) -> Option<String> {
        let was = self.lock().start_state.replace((retail, ours));
        if was == Some((retail, ours)) {
            return None;
        }
        Some(if retail == ours {
            format!("coop-live: lobby start state {retail}")
        } else if consoles == 1 {
            format!(
                "coop-live: lobby start state {retail} (co-op wants one console) answered {ours} for 1 console (alone; friends join in progress)"
            )
        } else {
            format!(
                "coop-live: lobby start state {retail} (co-op wants one console) answered {ours} for {consoles} consoles"
            )
        })
    }
    /// The Co-op Campaign row in the retail lobby mode: host. The lobby
    /// sets the level and difficulty itself, so the hooks arm with none.
    pub fn open_retail(&self) {
        self.host_retail();
        want_coop_mode();
    }
    /// Hosting a co-op campaign in the retail lobby, armed as the Co-op
    /// Campaign row arms it, for a lobby already open: the party lobby's
    /// CO-OP tab (`crate::lobby_tabs::switch`), which sets the lobby mode
    /// itself, so no mode is asked for.
    pub fn host_retail(&self) {
        {
            let mut i = self.lock();
            if i.role != Role::Host {
                i.role = Role::Host;
                i.advertised = None;
                eprintln!("coop-live: hosting a co-op campaign in the game's own lobby");
            }
        }
        self.refresh_line();
        if let Some(arm) = &self.arm {
            arm.arm(None);
        }
    }
    /// The retail lobby as the host's session holds it (the pinger, no
    /// guest memory held): the level and difficulty the lobby's own screens
    /// chose, advertised to Live when they change. The level is the one
    /// whose campaign and map ids the session holds; None while the lobby
    /// is not a campaign.
    pub fn sync_retail(&self) {
        if list_ui() {
            return;
        }
        let (role, s) = {
            let i = self.lock();
            (i.role.clone(), i.session)
        };
        if role != Role::Host {
            return;
        }
        let Some(s) = s.filter(|s| s.is_campaign() && s.is_host()) else {
            return;
        };
        let key = (s.campaign, s.map, s.difficulty);
        if self.lock().advertised == Some(key) {
            return;
        }
        let found = self
            .levels
            .iter()
            .position(|l| l.campaign == s.campaign && l.map == s.map);
        {
            let mut i = self.lock();
            if let Some(n) = found {
                i.level = n;
            }
            i.difficulty = u8::try_from(s.difficulty).unwrap_or(0).min(3);
            i.advertised = Some(key);
        }
        eprintln!(
            "coop-live: the lobby holds {} on {}; advertised",
            found.map_or_else(
                || format!("campaign {} map {}", s.campaign, s.map),
                |n| self.levels[n].label().to_owned()
            ),
            DIFFICULTIES[usize::from(self.difficulty())]
        );
        // A level we cannot name keeps what was advertised (the session
        // stays co-op on the server).
        if let (Some(live), Some(_)) = (&self.live, found) {
            let profiles = self.lock().profiles.clone();
            let skulls = (self.host_skulls)(&profiles);
            live.set_coop(self.campaign(), (!skulls.is_empty()).then_some(skulls));
        }
    }
    /// One round of ping (the pinger thread, no guest memory held): a guest
    /// pings its host; a host reads its guest's report and advertises it.
    /// Logs when the number moves.
    pub fn ping_once(&self) {
        self.sync_retail();
        let Some(live) = &self.live else { return };
        let (role, squad) = {
            let inner = self.lock();
            (
                inner.role.clone(),
                inner.session.filter(|_| inner.live_network),
            )
        };
        let coop_limit = (role == Role::Host).then(|| self.max_players());
        let slots = squad.and_then(|s| squad_slots(s, coop_limit));
        live.sync_squad_slots(slots.unwrap_or_default());
        let ms = match role {
            Role::Idle => None,
            Role::Host => {
                let ms = live.guest_ping();
                live.set_coop_ping(ms);
                ms
            }
            Role::Guest(g) => {
                let last = self.lock().ping;
                g.endpoint.and_then(|to| live.ping_host(to, last))
            }
        };
        self.apply_ping(ms);
    }

    fn apply_ping(&self, ms: Option<u16>) {
        let was = std::mem::replace(&mut self.lock().ping, ms);
        if live_hle::ping_moved(was, ms) {
            match ms {
                Some(ms) => eprintln!("coop-live: ping {ms} ms"),
                None if self.in_coop() => eprintln!("coop-live: ping: no answer"),
                None => {}
            }
        }
    }
    pub fn ping(&self) -> Option<u16> {
        self.lock().ping
    }
    pub(crate) fn friend_row_has_action(&self, row: i16) -> bool {
        self.friend(row).is_some_and(|friend| friend.online)
    }

    /// The ping as the screens show it.
    pub fn ping_text(&self) -> String {
        match self.ping() {
            Some(ms) => format!("{ms} ms"),
            None => "-".into(),
        }
    }
}

/// The wire set of a host's campaign skulls: the engine's `on`, then ours
/// (`crate::skulls_custom` indices, from [`crate::skulls::COUNT`]).
pub fn host_campaign_skulls(on: Option<crate::skulls::Set>, ours: &[usize]) -> Vec<u8> {
    let engine = on.into_iter().flat_map(|set| {
        set.into_iter()
            .enumerate()
            .filter(|(_, on)| *on)
            .map(|(i, _)| i)
    });
    live_proto::skulls::normalize(
        engine
            .chain(ours.iter().copied())
            .filter_map(|i| u8::try_from(i).ok()),
    )
    .unwrap_or_default()
}

/// Ours among a host's skulls (wire indices from [`crate::skulls::COUNT`]),
/// as `crate::skulls_custom` numbers them.
pub fn ours_of(host: &[u8]) -> Vec<usize> {
    host.iter()
        .map(|i| usize::from(*i))
        .filter(|i| crate::skulls::is_ours(*i))
        .collect()
}

/// The lobby of this run, once installed.
static LOBBY: OnceLock<Arc<Lobby>> = OnceLock::new();

pub fn lobby() -> Option<&'static Arc<Lobby>> {
    LOBBY.get()
}

/// A co-op guest's host skulls, for the campaign start (`crate::skulls`).
pub fn guest_skulls() -> Option<Vec<u8>> {
    lobby()?.guest().map(|g| g.skulls)
}

// ---- the screens ----

/// The host's co-op screen.
pub struct HostRows(pub Arc<Lobby>);

/// Its rows.
pub const ROW_LEVEL: i16 = 0;
pub const ROW_DIFFICULTY: i16 = 1;
pub const ROW_INVITE: i16 = 2;
pub const ROW_STATUS: i16 = 3;
pub const ROW_PING: i16 = 4;
pub const ROW_START: i16 = 5;
const HOST_ROWS: usize = 6;

impl HostRows {
    /// The `(label, value)` of row `row`.
    pub fn row_texts(&self, row: i16) -> Option<(String, String)> {
        let l = &self.0;
        Some(match row {
            ROW_LEVEL => (
                "Level".into(),
                l.level().map_or("None".into(), |v| v.label().to_owned()),
            ),
            ROW_DIFFICULTY => (
                "Difficulty".into(),
                DIFFICULTIES[usize::from(l.difficulty())].into(),
            ),
            ROW_INVITE => ("Invite a Friend".into(), String::new()),
            ROW_STATUS => ("Status".into(), l.status()),
            ROW_PING => ("Ping".into(), l.ping_text()),
            ROW_START => (
                "Start".into(),
                match l.checks().blocker() {
                    None => "Ready".into(),
                    Some(_) => "Not ready".into(),
                },
            ),
            _ => return None,
        })
    }
}

impl Rows for HostRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn describe(&self, count: usize) -> Option<String> {
        Some(describe(self, &self.0.texts, count))
    }
    fn title(&self) -> u32 {
        self.0.texts.set(title_slot(HOST_SLOTS), "Co-op Campaign")
    }
    fn count(&self) -> usize {
        HOST_ROWS
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some((label, value)) = self.row_texts(row) else {
            return Vec::new();
        };
        let t = &self.0.texts;
        vec![
            (
                crate::ui::LABEL_SLOT,
                t.set(label_slot(HOST_SLOTS, row), &label),
            ),
            (
                crate::ui::VALUE_SLOT,
                t.set(value_slot(HOST_SLOTS, row), &value),
            ),
        ]
    }
    fn press(&self, row: i16) -> Chosen {
        let l = &self.0;
        match row {
            ROW_LEVEL => Chosen::Stepped(l.step_level()),
            ROW_DIFFICULTY => Chosen::Stepped(l.step_difficulty()),
            ROW_INVITE => Chosen::Open(FRIENDS),
            ROW_START => match l.checks().blocker() {
                None => Chosen::Act,
                Some(b) => Chosen::Stepped(format!("coop-live: not ready: {b}")),
            },
            _ => Chosen::Nothing,
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.texts.write(m)
    }
}

/// The friends screen: one row per friend (online first); a press invites.
pub struct FriendRows(pub Arc<Lobby>);

impl FriendRows {
    pub fn row_texts(&self, row: i16) -> Option<(String, String)> {
        let l = &self.0;
        if l.lock().friends.is_empty() {
            return (row == 0).then(|| ("No friends yet".into(), String::new()));
        }
        let f = l.friend(row)?;
        let value = if l.invited(&f.gamertag) {
            "Invited".to_owned()
        } else if let Some(a) = &f.activity {
            a.clone()
        } else if f.playing {
            "Playing".into()
        } else if f.online {
            "Online".into()
        } else {
            "Offline".into()
        };
        Some((f.gamertag, value))
    }
}

impl Rows for FriendRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn describe(&self, count: usize) -> Option<String> {
        Some(describe(self, &self.0.texts, count))
    }
    fn title(&self) -> u32 {
        self.0
            .texts
            .set(title_slot(FRIEND_SLOTS), "Invite a Friend")
    }
    fn count(&self) -> usize {
        self.0.snapshot_friends().max(1)
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some((label, value)) = self.row_texts(row) else {
            return Vec::new();
        };
        if !(0..MAX_FRIENDS as i16).contains(&row) {
            return Vec::new();
        }
        let t = &self.0.texts;
        vec![
            (
                crate::ui::LABEL_SLOT,
                t.set(label_slot(FRIEND_SLOTS, row), &label),
            ),
            (
                crate::ui::VALUE_SLOT,
                t.set(value_slot(FRIEND_SLOTS, row), &value),
            ),
        ]
    }
    fn press(&self, row: i16) -> Chosen {
        if self.0.friend_row_has_action(row) {
            Chosen::Stepped(self.0.invite(row))
        } else {
            Chosen::Nothing
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.texts.write(m)
    }
}

/// The guest's join screen.
pub struct JoinRows(pub Arc<Lobby>);

pub const JOIN_READY: i16 = 5;

impl JoinRows {
    pub fn row_texts(&self, row: i16) -> Option<(String, String)> {
        let g = self.0.guest();
        let get = |f: &dyn Fn(&Guest) -> String| g.as_ref().map_or_else(|| "-".into(), f);
        Some(match row {
            0 => (
                "Host".into(),
                get(&|g| match self.0.ping() {
                    Some(ms) => format!("{}, {ms} ms", g.host),
                    None => g.host.clone(),
                }),
            ),
            1 => ("Level".into(), get(&|g| g.campaign.label().to_owned())),
            2 => (
                "Difficulty".into(),
                get(&|g| DIFFICULTIES[usize::from(g.campaign.difficulty.min(3))].into()),
            ),
            3 => (
                "Skulls".into(),
                get(&|g| match g.skulls.len() {
                    0 => "None".into(),
                    n => format!("{n} from the host"),
                }),
            ),
            4 => ("Handshake".into(), self.0.handshake()),
            JOIN_READY => ("Ready".into(), String::new()),
            _ => return None,
        })
    }
}

impl Rows for JoinRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn describe(&self, count: usize) -> Option<String> {
        Some(describe(self, &self.0.texts, count))
    }
    fn title(&self) -> u32 {
        self.0
            .texts
            .set(title_slot(JOIN_SLOTS), "Join Co-op Campaign")
    }
    fn count(&self) -> usize {
        JOIN_READY as usize + 1
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let Some((label, value)) = self.row_texts(row) else {
            return Vec::new();
        };
        let t = &self.0.texts;
        vec![
            (
                crate::ui::LABEL_SLOT,
                t.set(label_slot(JOIN_SLOTS, row), &label),
            ),
            (
                crate::ui::VALUE_SLOT,
                t.set(value_slot(JOIN_SLOTS, row), &value),
            ),
        ]
    }
    fn press(&self, row: i16) -> Chosen {
        match row {
            JOIN_READY => Chosen::Act,
            _ => Chosen::Nothing,
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.0.texts.write(m)
    }
}

// ---- actions ----

/// Closes `screen` as B does (`crate::ui_kit::lifecycle::close`).
async fn close(cx: &Cx, screen: u32) -> Result<(), EntryError> {
    crate::ui_kit::lifecycle::close(cx, screen).await
}

/// The host's Start: the campaign menu's setters put the level and
/// difficulty on the session (as the probe's tick setup does), then the
/// screen closes, back to the lobby, where the game is started as any
/// lobby's is.
pub async fn host_start(cx: &Cx, lobby: &Lobby, screen: u32) -> Result<(), EntryError> {
    if let Some(b) = lobby.checks().blocker() {
        eprintln!("coop-live: host: not ready: {b}");
        return Ok(());
    }
    let Some(want) = lobby.want() else {
        return Ok(());
    };
    if !lobby.checks().holds {
        for call in coop_probe::setup_calls(&want) {
            let mut g = GuestCall::new(call.point.base);
            if let Some(v) = call.eax {
                g = g.eax(v);
            }
            if let Some(v) = call.esi {
                g = g.esi(v);
            }
            let args: Vec<u32> = call.args.iter().flatten().copied().collect();
            if !args.is_empty() {
                g = g.args(args);
            }
            let r = cx.call(g).await?;
            let ok = r.eax & 0xff != 0;
            eprintln!(
                "coop-live: host: {} returned {}",
                call.point.name,
                if ok { "1" } else { "0 (refused)" }
            );
            if !ok {
                return Ok(());
            }
        }
    }
    eprintln!(
        "coop-live: host: start {} on {}; back to the lobby: start the game there",
        want.name,
        DIFFICULTIES[usize::from(want.difficulty.min(3))]
    );
    close(cx, screen).await
}

/// The guest's Ready: back to the lobby to wait for the host's start.
pub async fn guest_ready(cx: &Cx, lobby: &Lobby, screen: u32) -> Result<(), EntryError> {
    eprintln!(
        "coop-live: guest: ready ({}); waiting for the host to start",
        lobby.handshake()
    );
    close(cx, screen).await
}

// ---- the feature: texts, the session read, the join screen ----

/// Whether the join screen opens by itself (`H2_COOP_JOIN_SCREEN=0`: only
/// from the Co-op Campaign row).
fn auto_join_screen() -> bool {
    list_ui() && std::env::var("H2_COOP_JOIN_SCREEN").map_or(true, |v| v != "0")
}

/// Our list screens (host, friends, join) instead of the game's own co-op
/// lobby: `H2_COOP_LIST_UI=1`. Off, the Co-op Campaign row opens the
/// retail pregame lobby in co-op mode on a Live session
/// (docs/live/live-coop.md, "The retail lobby").
pub fn list_ui() -> bool {
    std::env::var("H2_COOP_LIST_UI").is_ok_and(|v| v == "1")
}

// ---- the retail lobby ----

/// The Co-op Campaign row ran the Custom Game row's path: its next lobby
/// mode call (`LOBBY_MODE_SET(2)`) becomes co-op (0). Until when, so a
/// failed session make never turns a later custom game into co-op.
static COOP_MODE_PENDING: Mutex<Option<std::time::Instant>> = Mutex::new(None);
/// How long the row's press waits for its lobby mode call.
const COOP_MODE_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);
/// The lobby modes `LOBBY_MODE_SET` takes.
pub const MODE_COOP: u32 = 0;
pub const MODE_CUSTOM: u32 = 2;

/// The Co-op Campaign row was pressed: the next custom game lobby is co-op.
pub fn want_coop_mode() {
    *COOP_MODE_PENDING.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(std::time::Instant::now() + COOP_MODE_WINDOW);
}
/// Another row: no co-op mode pending.
pub fn drop_coop_mode() {
    COOP_MODE_PENDING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
}
/// Whether the Co-op Campaign row's press still waits, at `now`, for its
/// lobby mode call; a window run out acts as none.
pub fn coop_mode_waits(pending: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    pending.is_some_and(|until| now <= until)
}
/// Why the co-op screens can't be snapshotted now: the lobby
/// ([`Lobby::snapshot_live`]), or the row's co-op lobby mode waits for its
/// call (a wall-clock window no snapshot holds).
pub fn screens_live(lobby: &Lobby) -> Option<String> {
    let pending = *COOP_MODE_PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if coop_mode_waits(pending, std::time::Instant::now()) {
        return Some("the Co-op Campaign row's lobby mode is pending".into());
    }
    lobby.snapshot_live()
}
/// The mode a `LOBBY_MODE_SET(mode)` call should set: co-op once, for the
/// custom game mode the Co-op Campaign row's press asked for.
pub fn lobby_mode(mode: u32, now: std::time::Instant) -> u32 {
    let mut p = COOP_MODE_PENDING.lock().unwrap_or_else(|e| e.into_inner());
    match *p {
        Some(until) if mode == MODE_CUSTOM => {
            *p = None;
            if now <= until { MODE_COOP } else { mode }
        }
        _ => mode,
    }
}

/// The lobby start state (`LOBBY_START_STATE`) that means "the co-op check
/// failed": two players, not both on this console.
pub const START_COOP_OR_TEAMS: u32 = 7;
pub const START_READY: u32 = 9;
pub const START_NOT_LOADED: u32 = 8;
pub const START_COUNTDOWN: u32 = 10;
/// Whether a start state of 7 is the co-op check on a Live co-op lobby:
/// a networked campaign session of 2 to `max` consoles with one player
/// each. Then the state is what the retail tail gives after a passing
/// check.
///
/// The check (`0x19a3ed`..`0x19a41c`) counts, over the 16 player slots,
/// the players present (`0x19a951`) and those on this console
/// (`0x19ab77`), and passes only for exactly two, both local: split
/// screen. Over Live each console has one local player, so it always
/// fails; the lift stands in for "one player per console, 2 to `max`".
pub fn lift_coop_check(state: u32, s: Option<&coop_probe::Session>, max: u32) -> bool {
    state == START_COOP_OR_TEAMS
        && s.is_some_and(|s| {
            s.is_campaign()
                && s.class != 0
                && s.peers >= 2
                && s.peers == s.players
                && s.peers <= max
        })
}
/// Whether a start state of 7 is the co-op check on a Live co-op host
/// alone: a networked campaign session of one console with one player.
/// With one present and local player the check (`0x19a3ed`, two wanted)
/// fails; with `solo` ([`Arm::solo_start`]) the lift lets the host start,
/// and friends join the running game (docs/live/live-coop-join.md 13.4).
/// A second local player is retail's split screen and passes by itself.
pub fn lift_solo_start(state: u32, s: Option<&coop_probe::Session>, solo: bool) -> bool {
    solo && state == START_COOP_OR_TEAMS
        && s.is_some_and(|s| {
            s.is_campaign() && s.class != 0 && s.is_host() && s.peers == 1 && s.players == 1
        })
}
/// The retail tail after a passing co-op check (`0x19a4c5`): a countdown
/// running gives 10, else everyone loaded 9, else 8.
pub fn tail_state(countdown: i32, all_ready: bool) -> u32 {
    if countdown >= 0 {
        START_COUNTDOWN
    } else if all_ready {
        START_READY
    } else {
        START_NOT_LOADED
    }
}

pub struct CoopScreens {
    lobby: Arc<Lobby>,
}

impl hooks::Feature for CoopScreens {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Co-op over Live: texts, the lobby read, the join screen",
        )
        .doc("docs/live/live-coop.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let base = b.guest_alloc(TEXT_SLOTS * TEXT_SLOT_LEN, Permissions::RW)?;
        self.lobby.set_text_base(base);
        let lobby = Arc::clone(&self.lobby);
        b.locale().source("co-op texts", move |id| lobby.string(id));
        let lobby = Arc::clone(&self.lobby);
        b.frame().on_tick("co-op texts and lobby", move |o| {
            let mut g = o.guest();
            let squad = coop_probe::read_session(&*g);
            // The network class is distinct from the squad parameter class.
            let live_network = squad.is_some_and(|s| {
                crate::layout_glue::Gm::u32(
                    &*g,
                    s.addr
                        .wrapping_add(crate::variant_ext::SESSION_NETWORK_CLASS),
                ) == Some(2)
            });
            lobby.observe_network(squad, live_network);
            lobby.refresh_line();
            let _ = lobby.texts.write(&mut *g);
            lobby.set_profiles(crate::skulls::profiles_or_default(&*g).0);
        });
        // The Co-op Campaign row's lobby: co-op mode, not custom game.
        let _lobby = Arc::clone(&self.lobby);
        b.raw()
            .before(&points::LOBBY_MODE_SET, "co-op mode for the co-op row", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let mut m = rt.memory();
                let at = c.cpu.esp.wrapping_add(4);
                let mode = m.u32(at)?;
                let want = lobby_mode(mode, std::time::Instant::now());
                if want != mode {
                    m.set_u32(at, want)?;
                    eprintln!(
                        "coop-live: the Co-op Campaign row's lobby: mode {mode} (custom game) set as {want} (co-op campaign)"
                    );
                } else {
                    eprintln!("coop-live: lobby mode {mode}");
                }
                Ok(Flow::Continue)
            });
        // The lobby's start state: its co-op check wants both players on
        // one console; on a Live co-op lobby (armed) two consoles are the
        // point.
        {
            let lobby = Arc::clone(&self.lobby);
            let reentry = Arc::new(AtomicBool::new(false));
            b.raw().before(
                &points::LOBBY_START_STATE,
                "co-op over 2 to N consoles",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !lobby.armed() {
                        return Ok(Flow::Continue);
                    }
                    let (lobby, reentry) = (Arc::clone(&lobby), Arc::clone(&reentry));
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(start_state(cx, lobby, reentry))
                    })
                },
            );
        }
        if auto_join_screen() {
            let lobby = Arc::clone(&self.lobby);
            b.raw()
                .before(&points::GAME_TICK, "open the co-op join screen", move |c| {
                    if !lobby.take_open_join() {
                        return Ok(Flow::Continue);
                    }
                    let Some(loader) = crate::list_class::loader_of(JOIN) else {
                        return Ok(Flow::Continue);
                    };
                    eprintln!("coop-live: guest: opening the join screen");
                    let saved = *c.cpu;
                    match open_then_tick(c.cpu, c.ctx, loader, saved) {
                        Ok(t) => Ok(Flow::Return(t)),
                        Err(e) => {
                            *c.cpu = saved;
                            eprintln!("coop-live: guest: the join screen did not open: {e:?}");
                            Ok(Flow::Continue)
                        }
                    }
                })
                // Inside the co-op hooks' lobby setup: it runs first, and
                // both re-enter the tick when they call the guest.
                .after_feature(coop_probe::LIVE_ID)
                .after_feature(coop_probe::ID)
                .after_feature(crate::skulls_custom::FEATURE);
        }
        let lobby = Arc::clone(&self.lobby);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || screens_live(&lobby)));
        Ok(())
    }
}

/// `LOBBY_START_STATE` on an armed console: the retail answer, unless it
/// is the co-op check refusing two to N consoles ([`lift_coop_check`]) or
/// a host alone ([`lift_solo_start`]), then the retail tail's
/// ([`tail_state`]). Logs each change of answer.
async fn start_state(
    cx: Cx,
    lobby: Arc<Lobby>,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    let progress = cx.arg(0)?;
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::LOBBY_START_STATE.base).args([progress]))
        .await;
    reentry.store(false, Ordering::SeqCst);
    let state = r?.eax;
    let session = lobby.session();
    let max = session
        .as_ref()
        .map_or(coop_probe::MAX_PLAYERS, |s| lobby.limit_for(s));
    let lifted = lift_coop_check(state, session.as_ref(), max)
        || lift_solo_start(state, session.as_ref(), lobby.solo_start());
    let out = if lifted {
        let countdown = cx
            .call(GuestCall::new(points::LOBBY_COUNTDOWN.base))
            .await?
            .eax as i32;
        let ready = if countdown >= 0 {
            false
        } else {
            cx.call(GuestCall::new(points::LOBBY_ALL_READY.base))
                .await?
                .eax
                & 0xff
                != 0
        };
        tail_state(countdown, ready)
    } else {
        state
    };
    lobby.note_start_state(state, out, session.as_ref().map_or(0, |s| s.peers));
    Ok(Ret::new(out, 4))
}

/// Opens our screen `loader` for player 0 (`OPEN_SCREEN(1, loader)`), then
/// re-enters the tick with the registers it was entered with.
fn open_then_tick(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    loader: u32,
    saved: CpuAbi,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    *cpu = saved;
    for v in [loader, 1] {
        cpu.esp = cpu.esp.wrapping_sub(4);
        mem.set_u32(cpu.esp, v)?;
    }
    ctx.call_guest(cpu, &mut mem, points::OPEN_SCREEN.base, move |cpu, _| {
        *cpu = saved;
        Ok(TransferAbi::jump(points::GAME_TICK.base))
    })
}

/// The guest watcher: once a second, while this console is in a squad
/// session with a second machine and is not its host, asks the HLE which
/// session it joined (no guest memory is held).
fn watch(lobby: std::sync::Weak<Lobby>, live: Arc<dyn Live>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let Some(lobby) = lobby.upgrade() else { return };
        if lobby.role() == Role::Host {
            continue;
        }
        let in_lobby = lobby
            .session()
            .is_some_and(|s| s.peers >= 2 && !s.is_host());
        if in_lobby || matches!(lobby.role(), Role::Guest(_)) {
            lobby.joined(&live.joined());
        }
    }
}

/// The pinger: a round of [`Lobby::ping_once`] every [`PING_EVERY`].
fn pinger(lobby: std::sync::Weak<Lobby>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(PING_EVERY);
        let Some(lobby) = lobby.upgrade() else { return };
        lobby.ping_once();
    }
}

/// Builds the lobby (the disc's levels), its three screens and the feature,
/// and starts the guest watcher. Live off: nothing.
pub fn install(
    registry: &mut hooks::Registry,
    hle: Option<Arc<live_hle::Hle>>,
    arm: Option<Arc<Arm>>,
    store: Option<Arc<settings::Store>>,
    disc: &Path,
) -> anyhow::Result<bool> {
    let Some(hle) = hle else { return Ok(false) };
    if std::env::var_os("H2_NO_COOP_ROW").is_some() {
        return Ok(false);
    }
    // The early plan builds have no disc path yet; read once it is known.
    let known = !disc.as_os_str().is_empty();
    let levels = if !known {
        Vec::new()
    } else {
        match disc_levels(disc) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("coop-live: cannot read the disc's campaign levels: {e:#}");
                Vec::new()
            }
        }
    };
    let named = levels.iter().filter(|l| l.name.is_some()).count();
    if known {
        eprintln!(
            "coop-live: {} campaign levels ({named} named){}",
            levels.len(),
            if arm.is_some() { "" } else { "; the co-op hooks are off" }
        );
    }
    let skulls: HostSkulls = Box::new(move |profiles| {
        let Some(store) = &store else {
            return Vec::new();
        };
        // The host's own campaign selection: collected and on (the skulls
        // page's rule), for the signed-in profiles as the start applies it;
        // the engine's, then ours.
        host_campaign_skulls(
            crate::skulls::selection(store, profiles),
            &crate::skulls::custom_selection(store, profiles),
        )
    });
    let live: Arc<dyn Live> = hle;
    let lobby = Arc::new(Lobby::new(levels, arm, Some(Arc::clone(&live)), skulls));
    let _ = LOBBY.set(Arc::clone(&lobby));
    for c in classes(&lobby) {
        registry
            .add(&c)
            .map_err(|e| anyhow::anyhow!("coop-live: {e}"))?;
    }
    registry
        .add(&CoopScreens {
            lobby: Arc::clone(&lobby),
        })
        .map_err(|e| anyhow::anyhow!("coop-live: {e}"))?;
    let weak = Arc::downgrade(&lobby);
    let stop = Arc::new(AtomicBool::new(false));
    {
        let (weak, stop) = (weak.clone(), Arc::clone(&stop));
        std::thread::Builder::new()
            .name("h2-coop-ping".into())
            .spawn(move || pinger(weak, stop))?;
    }
    std::thread::Builder::new()
        .name("h2-coop-watch".into())
        .spawn(move || watch(weak, live, stop))?;
    Ok(true)
}

/// The three screens' classes over `lobby`: the host's (Start starts),
/// the friends list, and the guest's (Start says ready).
pub fn classes(lobby: &Arc<Lobby>) -> [ListClass; 3] {
    let host: Action = {
        let lobby = Arc::clone(lobby);
        Arc::new(move |cx, screen| {
            let lobby = Arc::clone(&lobby);
            Box::pin(async move { host_start(&cx, &lobby, screen).await })
        })
    };
    let ready: Action = {
        let lobby = Arc::clone(lobby);
        Arc::new(move |cx, screen| {
            let lobby = Arc::clone(&lobby);
            Box::pin(async move { guest_ready(&cx, &lobby, screen).await })
        })
    };
    [
        ListClass::with_rows("coop-screen", SCREEN, Arc::new(HostRows(Arc::clone(lobby))))
            .action(host)
            .footer(footer_id(&lobby.texts, SCREEN)),
        ListClass::with_rows(
            "coop-friends",
            FRIENDS,
            Arc::new(FriendRows(Arc::clone(lobby))),
        )
        .footer(footer_id(&lobby.texts, FRIENDS)),
        ListClass::with_rows("coop-join", JOIN, Arc::new(JoinRows(Arc::clone(lobby))))
            .action(ready)
            .footer(footer_id(&lobby.texts, JOIN)),
    ]
}

#[cfg(test)]
mod tests;
