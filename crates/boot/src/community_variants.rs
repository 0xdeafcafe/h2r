//! Community game variants: classics the Halo 2 community played in custom
//! games (Zombies, SWAT, Jousting, Hide and Seek and so on), added to the
//! game's own list of built-in variants and marked with the "two players"
//! icon (U+E006) where the retail ones carry the padlock (U+E008).
//!
//! The design, the addresses and the sources are in
//! docs/community-gametypes.md. In short: the built-in variants are a block
//! in the `goof` tag of `mainmenu.map`, each a name, a game type, a list of
//! `(setting, value)` pairs (the choices a player makes in the editor) and a
//! description number. `0x149f88` turns every entry into a variant at load,
//! through the game's own setting code, and files it as a built-in, which
//! the game shows read-only and copies on edit. So this module only:
//!
//! 1. appends our entries to that block before `0x149f88` reads it,
//! 2. serves our names and descriptions through the localized-string lookup
//!    `0x1a0180` (ids in a private range), and
//! 3. maps our description numbers to those ids where the variant list asks
//!    `0x19fd00` for a description.
//!
//! Variants are defined symbolically ([`Setting`]); every pair is checked at
//! run time against the setting definitions (`sily` tags) the game itself
//! offers in the editor, and a variant with a pair the game would not offer
//! is left out. Retail entries are copied unchanged and saved variants are
//! never touched. Set `H2_NO_COMMUNITY_VARIANTS=1` to skip all of it.

use std::sync::Arc;

use runtime::Guest;
use runtime::guest::Permissions;

// ---- guest addresses (hook points; see docs/community-gametypes.md) ----

/// Loads the built-in variants from the `goof` tag (`[esp+4]`).
const BUILTINS_LOAD: u32 = 0x0014_9f88;
/// Localized string of a `unic` tag: EAX = tag, EDI = string id, `[esp+4]`
/// = a 0x100-unit UTF-16 buffer. `ret 4`.
const LOCALIZED_STRING: u32 = 0x001a_0180;
/// Integer 0..=100 (ECX) to the string id of its decimal text in EAX (0
/// out of range). Plain `ret`.
const NUMBER_STRING_ID: u32 = 0x0019_fd00;
/// The return address of the variant list's description lookup, the one
/// `NUMBER_STRING_ID` call we answer for.
const LIST_DESCRIPTION_RETURN: u32 = 0x002c_a1fd;
/// The tag instance table: 16-byte entries, address at `+8`.
const TAG_INSTANCES: u32 = 0x004e_3b44;

// ---- goof layout (read from 0x149f88 and 0x2369b3) ----

/// `+0x18`: setting pages; each `+8` count, `+0xc` address of `sily` refs.
const GOOF_PAGES: u32 = 0x18;
const PAGE_SIZE: u32 = 0x18;
/// `+0x28`: built-in variants.
const GOOF_BUILTINS: u32 = 0x28;
const ENTRY_SIZE: u32 = 0x14;
/// `sily`: `+0` setting, `+0x1c` count, `+0x20` options of `0xc` bytes,
/// value at `+4`.
const SILY_OPTIONS: u32 = 0x1c;
const OPTION_SIZE: u32 = 0xc;

/// Our guest page: settings pairs, then the merged entry block.
const DATA_BASE: u32 = 0xa0fd_0000;
const DATA_SIZE: u32 = 0x4000;
const ENTRIES_AT: u32 = DATA_BASE + 0x2000;
const MAX_ENTRIES: u32 = (DATA_SIZE - 0x2000) / ENTRY_SIZE;
const _: () = assert!(MAX_ENTRIES > 64);

/// The icon after every community name, where retail names carry U+E008.
pub const ICON: char = '\u{e006}';

// ---- the symbolic model ----

/// The game type of a built-in entry, as the list stores it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameType {
    Slayer = 0,
    Oddball = 1,
    Juggernaut = 2,
    King = 3,
    Ctf = 4,
    Assault = 5,
    Territories = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weapon {
    Random = 2,
    BattleRifle = 3,
    Magnum = 4,
    Smg = 5,
    PlasmaPistol = 6,
    PlasmaRifle = 7,
    RocketLauncher = 8,
    Shotgun = 9,
    SniperRifle = 10,
    BruteShot = 11,
    Needler = 12,
    Carbine = 13,
    BeamRifle = 14,
    EnergySword = 17,
    BrutePlasmaRifle = 18,
    SentinelBeam = 19,
}

/// "Weapons on Map".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponSet {
    None = 1,
    Rockets = 2,
    Shotguns = 3,
    Swords = 4,
    BruteShots = 5,
    Rifles = 11,
    Sniping = 12,
    Pistols = 14,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shields {
    Normal = 0,
    None = 1,
    Overshields = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightVehicle {
    Warthog = 1,
    GaussWarthog = 2,
    Ghost = 3,
    Spectre = 5,
    None = 7,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turret {
    MachineGun = 1,
    Plasma = 4,
    None = 6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Slow = 0,
    Normal = 1,
    Fast = 2,
}

/// One choice in the variant editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Points, or seconds for Oddball, King and Territories; 0 is none.
    ScoreToWin(u32),
    /// Seconds; 0 is none.
    TimeLimit(u32),
    /// 0 is unlimited.
    Lives(u32),
    RespawnSeconds(u32),
    SuicidePenaltySeconds(u32),
    Shields(Shields),
    MotionSensor(bool),
    ActiveCamo(bool),
    ExtraDamage(bool),
    DamageResistance(bool),
    TeamPlay(bool),
    FriendlyFire(bool),
    MaxPlayers(u32),
    StartingWeapon(Weapon),
    /// `None` is no secondary weapon.
    SecondaryWeapon(Option<Weapon>),
    StartingGrenades(bool),
    WeaponsOnMap(WeaponSet),
    GrenadesOnMap(bool),
    /// Weapon respawn "Double Time".
    WeaponsRespawnFast,
    OvershieldPickups(bool),
    CamoPickups(bool),
    LightVehicle(LightVehicle),
    SecondLightVehicle(LightVehicle),
    /// The heavy vehicles off.
    NoHeavyVehicle,
    Banshee(bool),
    PrimaryTurret(Turret),
    SecondaryTurret(Turret),
    JuggernautActiveCamo(bool),
    JuggernautOvershield(bool),
    JuggernautMotionSensor(bool),
    JuggernautDamageResistance(bool),
    JuggernautMovement(Speed),
    BallSpeed(Speed),
    BallToughness(bool),
    /// Seconds between moves; 0 is off.
    MovingHill(u32),
}

impl Setting {
    /// The `(setting, value)` pair the game's list stores for this choice.
    /// The numbers are the ones the editor's own definitions use.
    pub fn encode(self, ty: GameType) -> (u32, i32) {
        let b = |v: bool| i32::from(v);
        match self {
            Setting::ScoreToWin(v) => (score_setting(ty), v as i32),
            Setting::TimeLimit(v) => (10, v as i32),
            Setting::Lives(v) => (16, v as i32),
            Setting::RespawnSeconds(v) => (17, v as i32),
            Setting::SuicidePenaltySeconds(v) => (18, v as i32),
            Setting::Shields(s) => (19, s as i32),
            Setting::MotionSensor(v) => (20, b(v)),
            Setting::ActiveCamo(v) => (21, b(v)),
            Setting::FriendlyFire(v) => (24, b(v)),
            Setting::JuggernautOvershield(v) => (71, b(v)),
            Setting::JuggernautActiveCamo(v) => (72, b(v)),
            Setting::JuggernautMotionSensor(v) => (73, b(v)),
            Setting::LightVehicle(v) => (76, v as i32),
            Setting::SecondLightVehicle(v) => (77, v as i32),
            Setting::NoHeavyVehicle => (78, 4),
            Setting::Banshee(v) => (79, if v { 1 } else { 4 }),
            Setting::PrimaryTurret(t) => (81, t as i32),
            Setting::SecondaryTurret(t) => (82, t as i32),
            Setting::WeaponsOnMap(s) => (83, s as i32),
            Setting::OvershieldPickups(v) => (84, b(v)),
            Setting::CamoPickups(v) => (85, b(v)),
            Setting::GrenadesOnMap(v) => (86, b(v)),
            Setting::WeaponsRespawnFast => (87, 3),
            Setting::StartingGrenades(v) => (88, b(v)),
            Setting::StartingWeapon(w) => (89, w as i32),
            Setting::MaxPlayers(v) => (90, v as i32),
            Setting::TeamPlay(v) => (91, b(v)),
            Setting::SecondaryWeapon(w) => (94, w.map_or(1, |w| w as i32)),
            Setting::JuggernautMovement(s) => (96, s as i32),
            Setting::BallToughness(v) => (102, b(v)),
            Setting::ExtraDamage(v) => (104, b(v)),
            Setting::DamageResistance(v) => (105, b(v)),
            Setting::JuggernautDamageResistance(v) => (108, b(v)),
            Setting::BallSpeed(s) => (45, s as i32),
            Setting::MovingHill(v) => (40, v as i32),
        }
    }
}

/// Each game type keeps its score to win in its own setting.
fn score_setting(ty: GameType) -> u32 {
    match ty {
        GameType::Ctf => 1,
        GameType::Slayer => 2,
        GameType::Oddball => 3,
        GameType::King => 4,
        GameType::Juggernaut => 7,
        GameType::Territories => 8,
        GameType::Assault => 9,
    }
}

pub struct Variant {
    pub name: &'static str,
    pub ty: GameType,
    pub settings: &'static [Setting],
    pub description: &'static str,
}

impl Variant {
    pub fn pairs(&self) -> Vec<(u32, i32)> {
        self.settings.iter().map(|s| s.encode(self.ty)).collect()
    }
    /// The name as the list shows it: the name, a space and the icon.
    pub fn display_name(&self) -> String {
        format!("{} {ICON}", self.name)
    }
}

use Setting::*;

/// The community variants, in list order. Sources and the reasoning for each
/// choice are in docs/community-gametypes.md.
pub const VARIANTS: &[Variant] = &[
    Variant {
        name: "Zombies",
        ty: GameType::Slayer,
        settings: &[
            TeamPlay(true),
            StartingWeapon(Weapon::Shotgun),
            SecondaryWeapon(Some(Weapon::EnergySword)),
            StartingGrenades(false),
            GrenadesOnMap(false),
            WeaponsOnMap(WeaponSet::Shotguns),
            MotionSensor(false),
            OvershieldPickups(false),
            CamoPickups(false),
            LightVehicle(LightVehicle::None),
            SecondLightVehicle(LightVehicle::None),
            NoHeavyVehicle,
            Banshee(false),
            ScoreToWin(0),
        ],
        description: "Humans on red with shotguns, zombies on green with swords. When a zombie gets you, switch to green. Last human standing wins.",
    },
    Variant {
        name: "Hide and Seek",
        ty: GameType::Juggernaut,
        settings: &[
            StartingWeapon(Weapon::EnergySword),
            SecondaryWeapon(None),
            StartingGrenades(false),
            WeaponsOnMap(WeaponSet::None),
            GrenadesOnMap(false),
            ActiveCamo(true),
            MotionSensor(false),
            JuggernautActiveCamo(false),
            JuggernautMotionSensor(true),
            JuggernautMovement(Speed::Fast),
            Lives(1),
            TimeLimit(300),
        ],
        description: "Everyone hides with active camo and no motion sensor. The Juggernaut seeks with a sword, a motion sensor and extra speed. One life each.",
    },
    Variant {
        name: "Jousting",
        ty: GameType::Slayer,
        settings: &[
            LightVehicle(LightVehicle::Warthog),
            SecondLightVehicle(LightVehicle::None),
            NoHeavyVehicle,
            Banshee(false),
            StartingWeapon(Weapon::Shotgun),
            SecondaryWeapon(None),
            StartingGrenades(false),
            WeaponsOnMap(WeaponSet::None),
            GrenadesOnMap(false),
        ],
        description: "Pairs in Warthogs charge each other: one drives, one rides with a shotgun. Best on Coagulation.",
    },
    Variant {
        name: "SWAT",
        ty: GameType::Slayer,
        settings: &[
            TeamPlay(true),
            Shields(Shields::None),
            MotionSensor(false),
            StartingWeapon(Weapon::BattleRifle),
            SecondaryWeapon(Some(Weapon::Magnum)),
            WeaponsOnMap(WeaponSet::None),
            GrenadesOnMap(false),
            StartingGrenades(false),
            OvershieldPickups(false),
            CamoPickups(false),
            ScoreToWin(50),
        ],
        description: "No shields, no motion sensor, battle rifles and magnums. One headshot kills. 50 points to win.",
    },
    Variant {
        name: "Duel",
        ty: GameType::Slayer,
        settings: &[
            MaxPlayers(2),
            StartingWeapon(Weapon::Magnum),
            SecondaryWeapon(Some(Weapon::Random)),
            WeaponsOnMap(WeaponSet::None),
            ScoreToWin(15),
        ],
        description: "One on one, magnum and a random weapon, nothing on the map. Winner stays on. Try Lockout or Ascension.",
    },
    Variant {
        name: "Clint Eastwood",
        ty: GameType::Juggernaut,
        settings: &[
            Shields(Shields::None),
            StartingWeapon(Weapon::Shotgun),
            SecondaryWeapon(Some(Weapon::Magnum)),
            WeaponsOnMap(WeaponSet::None),
            JuggernautDamageResistance(true),
            JuggernautMovement(Speed::Fast),
        ],
        description: "No shields, a shotgun and a magnum. The Juggernaut is tough and fast. Best on Ascension.",
    },
    Variant {
        name: "Sun King",
        ty: GameType::King,
        settings: &[
            TeamPlay(true),
            Shields(Shields::None),
            RespawnSeconds(3),
            SuicidePenaltySeconds(0),
            StartingWeapon(Weapon::RocketLauncher),
            SecondaryWeapon(None),
            WeaponsOnMap(WeaponSet::Rockets),
            ScoreToWin(180),
            TimeLimit(720),
        ],
        description: "Team King of the Hill with rockets and no shields. You'll spend the whole game on fire.",
    },
    Variant {
        name: "Tower of Power",
        ty: GameType::Slayer,
        settings: &[
            TeamPlay(true),
            Shields(Shields::None),
            RespawnSeconds(3),
            LightVehicle(LightVehicle::None),
            PrimaryTurret(Turret::MachineGun),
            StartingWeapon(Weapon::Shotgun),
            SecondaryWeapon(None),
            WeaponsOnMap(WeaponSet::Shotguns),
            StartingGrenades(false),
            GrenadesOnMap(false),
            OvershieldPickups(false),
            CamoPickups(false),
            ScoreToWin(100),
        ],
        description: "Teams fight for the tower with shotguns and no shields. Made for Ascension.",
    },
    Variant {
        name: "Wheeeeee!",
        ty: GameType::Oddball,
        settings: &[
            TeamPlay(true),
            Shields(Shields::None),
            ExtraDamage(true),
            BallSpeed(Speed::Fast),
            BallToughness(true),
            StartingWeapon(Weapon::RocketLauncher),
            WeaponsOnMap(WeaponSet::Rockets),
            WeaponsRespawnFast,
            ScoreToWin(30),
        ],
        description: "Rockets, extra damage and a fast, tough ball carrier. How high can you fly?",
    },
    Variant {
        name: "Cops and Robbers",
        ty: GameType::Slayer,
        settings: &[
            TeamPlay(true),
            Lives(3),
            Shields(Shields::None),
            OvershieldPickups(false),
            CamoPickups(false),
            StartingWeapon(Weapon::Shotgun),
            SecondaryWeapon(Some(Weapon::PlasmaPistol)),
            WeaponsOnMap(WeaponSet::None),
        ],
        description: "Blue cops use only shotguns, red robbers only plasma pistols. Robbers who give up walk to jail. Three lives.",
    },
    Variant {
        name: "Bounce Practice",
        ty: GameType::Slayer,
        settings: &[
            ScoreToWin(0),
            TimeLimit(0),
            RespawnSeconds(3),
            SuicidePenaltySeconds(0),
            Shields(Shields::Overshields),
            DamageResistance(true),
            WeaponsOnMap(WeaponSet::None),
            GrenadesOnMap(false),
        ],
        description: "Practice super bounces and crouch jumps: no score, no time limit, tough players, quick respawns and nothing to pick up.",
    },
    Variant {
        name: "Glitch Hunt",
        ty: GameType::Slayer,
        settings: &[
            ScoreToWin(0),
            TimeLimit(0),
            RespawnSeconds(3),
            SuicidePenaltySeconds(0),
            Shields(Shields::Overshields),
            DamageResistance(true),
            StartingWeapon(Weapon::EnergySword),
            SecondaryWeapon(Some(Weapon::RocketLauncher)),
            FriendlyFire(false),
        ],
        description: "Explore the maps and find their secrets: sword flying, rocket and grenade jumps, tough players, no score and no clock.",
    },
];

// ---- guest memory seen by the logic ----

pub trait Mem {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool;
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool;
    fn u32(&self, a: u32) -> Option<u32> {
        let mut b = [0; 4];
        self.rd(a, &mut b).then_some(u32::from_le_bytes(b))
    }
    fn set_u32(&mut self, a: u32, v: u32) -> bool {
        self.wr(a, &v.to_le_bytes())
    }
}

impl Mem for Guest {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.read_bytes(addr, out, 1).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.write_bytes(addr, data, 1).is_ok()
    }
}

// ---- strings ----

/// Our string ids: a private index range (the low 24 bits) no map uses.
const ID_BASE: u32 = 0x00ff_e000;
pub fn is_ours(id: u32) -> bool {
    id & 0x00ff_ff00 == ID_BASE
}
pub fn name_id(i: usize) -> u32 {
    ID_BASE | i as u32
}
pub fn description_id(i: usize) -> u32 {
    ID_BASE | 0x80 | i as u32
}
/// The text for one of our ids.
pub fn text(id: u32) -> Option<String> {
    if !is_ours(id) {
        return None;
    }
    let i = (id & 0x7f) as usize;
    let v = VARIANTS.get(i)?;
    Some(if id & 0x80 == 0 {
        v.display_name()
    } else {
        v.description.to_owned()
    })
}

/// Description numbers: retail uses 0 to 38 of the 0 to 100 the game
/// accepts; ours start here.
pub const DESCRIPTION_BASE: u32 = 80;
/// The id the variant list should show for description number `n`, if it
/// is one of ours.
pub fn description_for(n: u32) -> Option<u32> {
    let i = n.checked_sub(DESCRIPTION_BASE)? as usize;
    (i < VARIANTS.len()).then(|| description_id(i))
}

/// What `0x19fd00` returns instead of the original, given its return
/// address and number: our description id for the variant list only.
pub fn description_answer(ret: u32, n: u32) -> Option<u32> {
    if ret == LIST_DESCRIPTION_RETURN {
        description_for(n)
    } else {
        None
    }
}

/// The UTF-16 the lookup writes: at most 0xff units and a NUL.
pub fn utf16_buffer(s: &str) -> Vec<u8> {
    let mut units: Vec<u16> = s.encode_utf16().take(0xff).collect();
    units.push(0);
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

// ---- the setting definitions the game offers ----

/// Every `(setting, value)` the editor offers, read from the `sily` tags
/// the `goof` tag's pages name.
pub fn offered<M: Mem>(
    m: &M,
    goof: u32,
    tags: u32,
) -> Option<std::collections::HashMap<u32, Vec<i32>>> {
    let mut out = std::collections::HashMap::<u32, Vec<i32>>::new();
    let (pages, at) = (m.u32(goof + GOOF_PAGES)?, m.u32(goof + GOOF_PAGES + 4)?);
    for p in 0..pages.min(0x100) {
        let page = at + p * PAGE_SIZE;
        let (n, refs) = (m.u32(page + 8)?, m.u32(page + 0xc)?);
        for r in 0..n.min(0x100) {
            let datum = m.u32(refs + r * 8 + 4)?;
            if datum == u32::MAX {
                continue;
            }
            let sily = m.u32(tags + (datum & 0xffff) * 16 + 8)?;
            let setting = m.u32(sily)?;
            let (k, opts) = (m.u32(sily + SILY_OPTIONS)?, m.u32(sily + SILY_OPTIONS + 4)?);
            let values = out.entry(setting).or_default();
            for o in 0..k.min(0x100) {
                values.push(m.u32(opts + o * OPTION_SIZE + 4)? as i32);
            }
        }
    }
    Some(out)
}

/// The variants whose every pair is offered, by index.
pub fn usable(offered: &std::collections::HashMap<u32, Vec<i32>>) -> Vec<usize> {
    let ok = |(s, v): (u32, i32)| offered.get(&s).is_some_and(|vs| vs.contains(&v));
    (0..VARIANTS.len())
        .filter(|i| VARIANTS[*i].pairs().into_iter().all(ok))
        .collect()
}

/// One list entry: name id, game type, settings block, description number.
pub fn entry(i: usize, pairs_at: u32) -> [u8; ENTRY_SIZE as usize] {
    let v = &VARIANTS[i];
    let mut e = [0; ENTRY_SIZE as usize];
    e[0..4].copy_from_slice(&name_id(i).to_le_bytes());
    e[4..8].copy_from_slice(&(v.ty as u32).to_le_bytes());
    e[8..12].copy_from_slice(&(v.settings.len() as u32).to_le_bytes());
    e[12..16].copy_from_slice(&pairs_at.to_le_bytes());
    e[16..20].copy_from_slice(&(DESCRIPTION_BASE + i as u32).to_le_bytes());
    e
}

/// Append the variants in `which` to the `goof` tag's built-in list. The
/// retail entries are copied unchanged in front, into `base` (settings
/// pairs) and `entries` (the merged block); the tag then points at the
/// merged block. Does nothing (`Some(false)`) if the tag already points
/// there. `None` is a read or write failure, with the tag unchanged.
pub fn extend_builtins<M: Mem>(
    m: &mut M,
    goof: u32,
    which: &[usize],
    base: u32,
    entries: u32,
) -> Option<bool> {
    let (n, old) = (
        m.u32(goof + GOOF_BUILTINS)?,
        m.u32(goof + GOOF_BUILTINS + 4)?,
    );
    if old == entries || which.is_empty() {
        return Some(false);
    }
    if n + which.len() as u32 > (DATA_BASE + DATA_SIZE - entries) / ENTRY_SIZE {
        return None;
    }
    let mut block = vec![0; (n * ENTRY_SIZE) as usize];
    if !m.rd(old, &mut block) {
        return None;
    }
    let mut at = base;
    for &i in which {
        let pairs: Vec<u8> = VARIANTS[i]
            .pairs()
            .into_iter()
            .flat_map(|(s, v)| s.to_le_bytes().into_iter().chain(v.to_le_bytes()))
            .collect();
        if at + pairs.len() as u32 > entries || !m.wr(at, &pairs) {
            return None;
        }
        block.extend_from_slice(&entry(i, at));
        at += pairs.len() as u32;
    }
    if !m.wr(entries, &block) {
        return None;
    }
    // The block first, then the count that makes it visible.
    (m.set_u32(goof + GOOF_BUILTINS + 4, entries)
        && m.set_u32(goof + GOOF_BUILTINS, n + which.len() as u32))
    .then_some(true)
}

// ---- hooks ----

fn original(rt: &runtime::Runtime, addr: u32) -> Option<Arc<runtime::exec::OriginalHandler>> {
    let o = rt.resolve_original(addr);
    if o.is_none() {
        eprintln!("community variants: no generated entry at {addr:#x}; hook skipped");
    }
    o.map(Arc::new)
}

/// Install the three hooks. A missing original skips its hook and says so.
pub fn install(rt: &runtime::Runtime) -> anyhow::Result<()> {
    if std::env::var_os("H2_NO_COMMUNITY_VARIANTS").is_some() {
        return Ok(());
    }
    rt.memory()
        .map_zeroed(DATA_BASE, DATA_SIZE, Permissions::RW)?;
    rt.edit_dispatch(|d| {
        if let Some(o) = original(rt, BUILTINS_LOAD) {
            d.register_override(BUILTINS_LOAD, "community:builtins", move |cpu, ctx, _| {
                {
                    let mut mem = ctx.runtime().memory();
                    let goof = mem.u32(cpu.esp + 4)?;
                    let tags = mem.u32(TAG_INSTANCES)?;
                    let which = offered(&*mem, goof, tags).map(|o| usable(&o)).unwrap_or_default();
                    if which.len() < VARIANTS.len() {
                        let left: Vec<_> =
                            (0..VARIANTS.len()).filter(|i| !which.contains(i)).map(|i| VARIANTS[i].name).collect();
                        eprintln!("community variants: left out (a setting the game does not offer): {left:?}");
                    }
                    match extend_builtins(&mut *mem, goof, &which, DATA_BASE, ENTRIES_AT) {
                        Some(true) => eprintln!("community variants: {} added to the built-in list", which.len()),
                        Some(false) => {}
                        None => eprintln!("community variants: could not extend the built-in list"),
                    }
                }
                Ok(o.invoke(cpu, ctx, BUILTINS_LOAD))
            });
        }
        if let Some(o) = original(rt, LOCALIZED_STRING) {
            d.register_override(LOCALIZED_STRING, "community:strings", move |cpu, ctx, _| {
                if let Some(t) = text(cpu.edi) {
                    let mut mem = ctx.runtime().memory();
                    let buf = mem.u32(cpu.esp + 4)?;
                    mem.write_bytes(buf, &utf16_buffer(&t), 1)?;
                    return mem.ret(cpu, 4);
                }
                Ok(o.invoke(cpu, ctx, LOCALIZED_STRING))
            });
        }
        if let Some(o) = original(rt, NUMBER_STRING_ID) {
            d.register_override(NUMBER_STRING_ID, "community:descriptions", move |cpu, ctx, _| {
                if cpu.ecx >= DESCRIPTION_BASE {
                    let mem = ctx.runtime().memory();
                    if let Some(id) = description_answer(mem.u32(cpu.esp)?, cpu.ecx) {
                        cpu.eax = id;
                        return mem.ret(cpu, 0);
                    }
                }
                Ok(o.invoke(cpu, ctx, NUMBER_STRING_ID))
            });
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Sparse little-endian memory; unwritten bytes read as missing.
    #[derive(Default)]
    struct Fake(HashMap<u32, u8>);
    impl Mem for Fake {
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

    const GOOF: u32 = 0x8000_0000;
    const OLD: u32 = 0x8000_1000;
    const TAGS: u32 = 0x8001_0000;

    /// A goof tag with `n` invented retail entries and setting pages that
    /// offer `offer`.
    fn world(n: u32, offer: &[(u32, &[i32])]) -> Fake {
        let mut m = Fake::default();
        m.wr(GOOF, &[0; 0x100]);
        for i in 0..n {
            let e: Vec<u8> = [0x0100_0000 + i, i % 7, 0, 0, i]
                .iter()
                .flat_map(|w: &u32| w.to_le_bytes())
                .collect();
            m.wr(OLD + i * ENTRY_SIZE, &e);
        }
        m.set_u32(GOOF + GOOF_BUILTINS, n);
        m.set_u32(GOOF + GOOF_BUILTINS + 4, OLD);
        // One page naming every sily, plus a null reference.
        let (page, refs) = (0x8000_2000, 0x8000_3000);
        m.set_u32(GOOF + GOOF_PAGES, 1);
        m.set_u32(GOOF + GOOF_PAGES + 4, page);
        m.wr(page, &[0; PAGE_SIZE as usize]);
        m.set_u32(page + 8, offer.len() as u32 + 1);
        m.set_u32(page + 0xc, refs);
        m.set_u32(refs, 0);
        m.set_u32(refs + 4, u32::MAX);
        for (k, (setting, values)) in offer.iter().enumerate() {
            let k = k as u32;
            let sily = 0x8010_0000 + k * 0x1000;
            m.set_u32(refs + (k + 1) * 8 + 4, 0xe000_0000 | (k + 5));
            m.set_u32(TAGS + (k + 5) * 16 + 8, sily);
            m.wr(sily, &[0; 0x24]);
            m.set_u32(sily, *setting);
            m.set_u32(sily + SILY_OPTIONS, values.len() as u32);
            m.set_u32(sily + SILY_OPTIONS + 4, sily + 0x100);
            for (j, v) in values.iter().enumerate() {
                let o = sily + 0x100 + j as u32 * OPTION_SIZE;
                m.set_u32(o, 0);
                m.set_u32(o + 4, *v as u32);
                m.set_u32(o + 8, 0x0700_0000);
            }
        }
        m
    }

    /// Every pair our variants use, offered.
    fn all_offered() -> Vec<(u32, Vec<i32>)> {
        let mut by: HashMap<u32, Vec<i32>> = HashMap::new();
        for v in VARIANTS {
            for (s, x) in v.pairs() {
                by.entry(s).or_default().push(x);
            }
        }
        by.into_iter().collect()
    }

    #[test]
    fn settings_encode_as_the_editor_stores_them() {
        assert_eq!(
            StartingWeapon(Weapon::Shotgun).encode(GameType::Slayer),
            (89, 9)
        );
        assert_eq!(SecondaryWeapon(None).encode(GameType::Slayer), (94, 1));
        assert_eq!(
            SecondaryWeapon(Some(Weapon::EnergySword)).encode(GameType::Slayer),
            (94, 17)
        );
        assert_eq!(Shields(Shields::None).encode(GameType::Slayer), (19, 1));
        assert_eq!(Banshee(false).encode(GameType::Slayer), (79, 4));
        assert_eq!(Banshee(true).encode(GameType::Slayer), (79, 1));
        assert_eq!(MotionSensor(false).encode(GameType::Slayer), (20, 0));
        // Score to win follows the game type.
        let score: Vec<_> = [
            GameType::Slayer,
            GameType::Oddball,
            GameType::Juggernaut,
            GameType::King,
            GameType::Ctf,
            GameType::Assault,
            GameType::Territories,
        ]
        .into_iter()
        .map(|t| ScoreToWin(5).encode(t).0)
        .collect();
        assert_eq!(score, [2, 3, 7, 4, 1, 9, 8]);
    }

    #[test]
    fn variants_are_well_formed() {
        assert!((8..=12).contains(&VARIANTS.len()));
        let mut names = std::collections::HashSet::new();
        for v in VARIANTS {
            // The keyboard allows 16 characters; the copy starts from this.
            assert!(v.name.chars().count() <= 16, "{}", v.name);
            assert!(names.insert(v.name), "duplicate {}", v.name);
            // No setting twice: the game applies them in order and the
            // later would win silently.
            let mut seen = std::collections::HashSet::new();
            for (s, _) in v.pairs() {
                assert!(seen.insert(s), "{} sets {s} twice", v.name);
            }
            assert!(!v.description.is_empty() && v.description.encode_utf16().count() < 0xff);
            assert!(v.display_name().ends_with(ICON));
            assert!(!v.display_name().contains('\u{e008}'), "never the padlock");
        }
        // Retail names to avoid (docs/community-gametypes.md).
        for clash in [
            "Rockets",
            "Swords",
            "Snipers",
            "Fiesta",
            "Dodgeball",
            "Juggernaut",
            "Ninjanaut",
        ] {
            assert!(!names.contains(clash));
        }
    }

    #[test]
    fn string_ids_are_private_and_round_trip() {
        for (i, v) in VARIANTS.iter().enumerate() {
            assert!(is_ours(name_id(i)) && is_ours(description_id(i)));
            assert_eq!(text(name_id(i)).unwrap(), v.display_name());
            assert_eq!(text(description_id(i)).unwrap(), v.description);
            assert_eq!(
                description_for(DESCRIPTION_BASE + i as u32),
                Some(description_id(i))
            );
        }
        // Retail-shaped ids (length byte, small index) are never ours.
        for id in [
            0x0700_0406,
            0x0100_030c,
            0x0200_031d,
            0x1c00_164d,
            0,
            u32::MAX,
        ] {
            assert!(!is_ours(id) && text(id).is_none());
        }
        // Retail description numbers and numbers past ours pass through.
        for n in [0, 2, 38, 79, DESCRIPTION_BASE + VARIANTS.len() as u32, 100] {
            assert_eq!(description_for(n), None);
        }
        assert!(
            DESCRIPTION_BASE + VARIANTS.len() as u32 <= 100,
            "the game clamps to 100"
        );
    }

    #[test]
    fn utf16_buffer_is_nul_terminated_and_bounded() {
        let b = utf16_buffer("Zombies \u{e006}");
        assert_eq!(b.len(), 10 * 2);
        assert_eq!(&b[16..20], &[0x06, 0xe0, 0, 0]);
        let long = "x".repeat(400);
        assert_eq!(utf16_buffer(&long).len(), 0x100 * 2);
    }

    #[test]
    fn extend_appends_after_retail_entries_unchanged() {
        let offer = all_offered();
        let offer: Vec<(u32, &[i32])> = offer.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let mut m = world(39, &offer);
        let retail_before: Vec<u8> = {
            let mut b = vec![0; 39 * ENTRY_SIZE as usize];
            assert!(m.rd(OLD, &mut b));
            b
        };
        let which = usable(&offered(&m, GOOF, TAGS).unwrap());
        assert_eq!(which, (0..VARIANTS.len()).collect::<Vec<_>>());
        assert_eq!(
            extend_builtins(&mut m, GOOF, &which, DATA_BASE, ENTRIES_AT),
            Some(true)
        );

        let n = m.u32(GOOF + GOOF_BUILTINS).unwrap();
        assert_eq!(n as usize, 39 + VARIANTS.len());
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(ENTRIES_AT));
        // Retail entries: byte for byte, in place; the old block untouched.
        let mut b = vec![0; retail_before.len()];
        assert!(m.rd(ENTRIES_AT, &mut b));
        assert_eq!(b, retail_before);
        assert!(m.rd(OLD, &mut b));
        assert_eq!(b, retail_before);
        // Ours decode back to the symbolic settings.
        for (k, v) in VARIANTS.iter().enumerate() {
            let e = ENTRIES_AT + (39 + k as u32) * ENTRY_SIZE;
            assert_eq!(m.u32(e), Some(name_id(k)));
            assert_eq!(m.u32(e + 4), Some(v.ty as u32));
            let (count, at) = (m.u32(e + 8).unwrap(), m.u32(e + 12).unwrap());
            let got: Vec<(u32, i32)> = (0..count)
                .map(|j| {
                    (
                        m.u32(at + j * 8).unwrap(),
                        m.u32(at + j * 8 + 4).unwrap() as i32,
                    )
                })
                .collect();
            assert_eq!(got, v.pairs());
            assert_eq!(m.u32(e + 16), Some(DESCRIPTION_BASE + k as u32));
        }
        // Again: already ours, nothing changes.
        assert_eq!(
            extend_builtins(&mut m, GOOF, &which, DATA_BASE, ENTRIES_AT),
            Some(false)
        );
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS), Some(n));
    }

    #[test]
    fn a_variant_with_an_unoffered_value_is_left_out() {
        // Offer everything except a shotgun start.
        let mut offer = all_offered();
        for (s, v) in &mut offer {
            if *s == 89 {
                v.retain(|x| *x != Weapon::Shotgun as i32);
            }
        }
        let offer: Vec<(u32, &[i32])> = offer.iter().map(|(s, v)| (*s, v.as_slice())).collect();
        let m = world(3, &offer);
        let which = usable(&offered(&m, GOOF, TAGS).unwrap());
        for (i, v) in VARIANTS.iter().enumerate() {
            let shotgun = v.settings.contains(&StartingWeapon(Weapon::Shotgun));
            assert_eq!(which.contains(&i), !shotgun, "{}", v.name);
        }
        // With nothing offered nothing is added and the tag is unchanged.
        let mut m = world(3, &[]);
        let which = usable(&offered(&m, GOOF, TAGS).unwrap());
        assert!(which.is_empty());
        assert_eq!(
            extend_builtins(&mut m, GOOF, &which, DATA_BASE, ENTRIES_AT),
            Some(false)
        );
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(OLD));
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS), Some(3));
    }

    #[test]
    fn a_failed_read_leaves_the_tag_alone() {
        let mut m = world(5, &[]);
        // The retail block claims more entries than exist.
        m.set_u32(GOOF + GOOF_BUILTINS, 6);
        assert_eq!(
            extend_builtins(&mut m, GOOF, &[0, 1], DATA_BASE, ENTRIES_AT),
            None
        );
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(OLD));
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS), Some(6));
        // A list that would not fit our page is refused the same way.
        m.set_u32(GOOF + GOOF_BUILTINS, MAX_ENTRIES);
        assert_eq!(
            extend_builtins(&mut m, GOOF, &[0], DATA_BASE, ENTRIES_AT),
            None
        );
        assert_eq!(m.u32(GOOF + GOOF_BUILTINS + 4), Some(OLD));
    }
}
