//! The superbounce game type (docs/game/superbounce.md, "The superbounce
//! game type"): a HUD that says whether the local player's level pair is
//! primed, lines over the map for the spots a primed landing launches
//! from, and the `superbounce.state` agent method.
//!
//! **When it is on.** The active variant carries the extension block's
//! superbounce flag ([`variants::ext::Ext::superbounce`], through
//! [`crate::variant_ext::Decision::superbounce`]), or is the built-in
//! "Superbounce" variant ([`crate::community_variants`], which has no field
//! for the flag, so it is told by name), or `H2_SUPERBOUNCE=1` forces it.
//!
//! **Each tick** (a tick observer, only while on) it reads the first local
//! player's unit, its position, whether it is airborne, its vertical
//! velocity and aim, and the word of its one pair against the level
//! (`live_telemetry::havok_agents::fixed_pair`, pair `+0x0c`; -3 is
//! primed), asks for the loaded map's spots (a background scan, once per
//! map and structure BSP, cached), finds the nearest bounce spot and
//! publishes all of it as one [`State`]. The HUD ([`crate::superbounce_hud`],
//! through [`hud_state`]), the world lines ([`gpu::Lines`])
//! and the agent method ([`reply`]) read that state.
//!
//! **The charge, the next thing to do and the landings**
//! (docs/game/superbounce.md, "Charge, crouch spots and landings") are on
//! the same state: [`State::charge`] and [`State::verb`] from the
//! [`superbounce_charge`](crate::superbounce_charge) tracker, which also
//! files [`State::last_landing`] and picks [`State::nav_target`];
//! [`State::landing`] from the assist's prediction
//! ([`crate::superbounce_assist::predict`]). The standing-capsule test
//! behind `under` is [`crate::superbounce_capsule`], over the collision the
//! scan reads.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use engine::GuestMemory;
use superbounce::{Kind, Spot, Vec3};

pub use crate::superbounce_charge::{
    Charge, ChargePhase, Course, Landing, LandingResult, NavKind, NavTarget, Predicted, Verb,
};
use crate::superbounce_capsule::Room;
use crate::superbounce_charge::{Tick, Tracker};

/// The feature's name.
pub const FEATURE: &str = "superbounce";
/// `H2_SUPERBOUNCE=1` forces the mode on in any game.
pub const ENV: &str = "H2_SUPERBOUNCE";
/// The built-in variant's name ([`crate::community_variants::VARIANTS`]).
pub const VARIANT_NAME: &str = "Superbounce";
/// The pair word of a primed player, as a signed number.
pub const PRIMED_WORD: i32 = -3;
/// Metres in a world unit (10 feet).
pub const METRES_PER_UNIT: f32 = 3.048;
/// Where an object's position is, in its body.
const OBJECT_POSITION: u32 = 0x30;
/// The most spot lines drawn: the nearest ones.
pub const MAX_LINES: usize = 40;
/// A bounce line's brightness while the player is not primed.
pub const DIM: f32 = 0.35;
/// Bounce strips that carry their launch label: the nearest few.
pub const BOUNCE_LABELS: usize = 3;
/// World units (from the camera) where a line starts to fade, and where
/// it is gone.
pub const FADE_NEAR: f32 = 25.0;
pub const FADE_FAR: f32 = 45.0;
/// A slot's width when the scan gives none.
pub const RIBBON_WIDTH: f32 = 0.2;

// ---- pure rules ----

/// Whether a level pair word is the primed one (-3).
pub fn primed(word: u32) -> bool {
    word as i32 == PRIMED_WORD
}

/// A variant name (as the record or the list holds it, with the list's
/// trailing icon) that is the built-in Superbounce.
pub fn is_superbounce_name(name: &str) -> bool {
    name.trim_end_matches(|c: char| c.is_whitespace() || ('\u{e000}'..='\u{f8ff}').contains(&c))
        == VARIANT_NAME
}

// ---- the published state ----

/// How the spot scan stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ScanState {
    /// Nothing asked for: no map yet.
    #[default]
    Idle,
    Scanning,
    Ready,
    Failed(String),
}

/// The bounce spot nearest the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Near {
    /// World units, to the spot's edge.
    pub distance: f32,
    pub launch: f32,
    pub fall: f32,
    pub edge: [Vec3; 2],
}

/// Everything the HUD, the lines and the agent method show, as of the last
/// tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub active: bool,
    /// The level pair's word; `None` without a pair (between lives).
    pub word: Option<u32>,
    pub position: Option<Vec3>,
    pub nearest: Option<Near>,
    pub scan: ScanState,
    pub map: Option<String>,
    pub bounce: usize,
    pub crouch: usize,
    /// The biped is off the ground (biped flags bit 0, as `world.object`'s
    /// `physics.airborne` reads it); false without a biped.
    pub airborne: bool,
    /// Vertical velocity, world units a second, up positive.
    pub vz: Option<f32>,
    /// The unit's aim as a yaw, radians from +X toward +Y. The first-person
    /// camera looks along it.
    pub yaw: Option<f32>,
    /// The charge meter: the phase of the recipe and its fill; `None` is
    /// hidden (not primed, and not crouched by a crouch spot).
    pub charge: Option<Charge>,
    /// The recipe's step for the HUD's prompt line, while `under` or
    /// `held`.
    pub verb: Option<Verb>,
    /// The navpoint's target: the nearest crouch spot while not primed,
    /// the nearest strip while primed. `None` without a position or a spot
    /// of that kind.
    pub nav_target: Option<NavTarget>,
    /// The landing the assist predicts, while airborne and falling toward
    /// a strip.
    pub landing: Option<Predicted>,
    /// The last landing within 0.5 of a strip, filed at touchdown; kept
    /// until the next one or the game's end.
    pub last_landing: Option<Landing>,
    /// Ticks the primed body has been still (speed under 0.03 units/s);
    /// 0 while not primed. The primed fill is 1 less this over 38.
    pub still_ticks: u32,
    /// Whether the standing capsule fits at the player's origin, by the
    /// capsule test against the scanned collision
    /// ([`crate::superbounce_capsule`]); `None` before the scan is ready.
    pub standing_fits: Option<bool>,
}

impl State {
    const EMPTY: State = State {
        active: false,
        word: None,
        position: None,
        nearest: None,
        scan: ScanState::Idle,
        map: None,
        bounce: 0,
        crouch: 0,
        airborne: false,
        vz: None,
        yaw: None,
        charge: None,
        verb: None,
        nav_target: None,
        landing: None,
        last_landing: None,
        still_ticks: 0,
        standing_fits: None,
    };

    pub fn primed(&self) -> bool {
        self.word.is_some_and(primed)
    }

    /// Where the nearest spot is from where the player looks: radians
    /// clockwise from straight ahead, in (-pi, pi] (a spot to the right is
    /// positive, as an arrow on screen turns). None without a position, an
    /// aim or a spot, or standing on it.
    pub fn bearing(&self) -> Option<f32> {
        bearing(self.position?, self.yaw?, self.nearest?.edge)
    }
}

/// [`State::bearing`] for a player at `at` aiming along `yaw`, to the
/// nearest point of `edge`, in the horizontal plane.
pub fn bearing(at: Vec3, yaw: f32, edge: [Vec3; 2]) -> Option<f32> {
    use std::f32::consts::{PI, TAU};
    let [a, b] = edge;
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 0.0 {
        (((at[0] - a[0]) * ex + (at[1] - a[1]) * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (a[0] + t * ex - at[0], a[1] + t * ey - at[1]);
    if dx.hypot(dy) < 0.05 || !yaw.is_finite() {
        return None;
    }
    // World yaw turns from +X toward +Y, which is to the left (Z up).
    let left = dy.atan2(dx) - yaw;
    let mut cw = (-left).rem_euclid(TAU);
    if cw > PI {
        cw -= TAU;
    }
    Some(cw)
}

static STATE: Mutex<State> = Mutex::new(State::EMPTY);
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state as of the last tick.
pub fn state() -> State {
    lock(&STATE).clone()
}

/// Whether the mode was on at the last tick (one atomic load).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The state for the HUD: `None` while the mode is off (one atomic load
/// then). The window asks each present.
pub fn hud_state() -> Option<State> {
    if !active() {
        return None;
    }
    let s = state();
    s.active.then_some(s)
}

/// `superbounce.state`'s result for `s`.
pub fn reply(s: &State) -> agent_proto::superbounce::State {
    use agent_proto::superbounce as wire;
    let (status, error) = match &s.scan {
        ScanState::Idle => ("idle", None),
        ScanState::Scanning => ("scanning", None),
        ScanState::Ready => ("ready", None),
        ScanState::Failed(why) => ("failed", Some(why.clone())),
    };
    let v3 = |v: Vec3| v.map(f64::from);
    wire::State {
        active: s.active,
        primed: s.primed(),
        word: s.word.map(|w| format!("{w:08x}")),
        word_signed: s.word.map(|w| w as i32),
        position: s.position.map(v3),
        nearest: s.nearest.map(|n| wire::Nearest {
            distance: f64::from(n.distance),
            launch: f64::from(n.launch),
            fall: f64::from(n.fall),
            edge: n.edge.map(v3),
        }),
        scan: wire::Scan {
            status: status.into(),
            map: s.map.clone(),
            error,
            bounce: s.bounce,
            crouch: s.crouch,
        },
        airborne: s.position.map(|_| s.airborne),
        vz: s.vz.map(f64::from),
        yaw: s.yaw.map(f64::from),
        charge: s.charge.map(|c| wire::Charge {
            phase: match c.phase {
                ChargePhase::Near => wire::ChargePhase::Near,
                ChargePhase::Under => wire::ChargePhase::Under,
                ChargePhase::Held => wire::ChargePhase::Held,
                ChargePhase::Primed => wire::ChargePhase::Primed,
                ChargePhase::Lost => wire::ChargePhase::Lost,
            },
            fill: f64::from(c.fill),
            still_ticks: s.still_ticks,
        }),
        verb: s.verb.map(|v| match v {
            Verb::LetGoOfCrouch => wire::Verb::LetGoOfCrouch,
            Verb::WalkOutToStand => wire::Verb::WalkOutToStand,
        }),
        nav_target: s.nav_target.map(|n| wire::NavTarget {
            at: v3(n.at),
            kind: match n.kind {
                NavKind::Crouch => wire::NavKind::Crouch,
                NavKind::Strip => wire::NavKind::Strip,
            },
            distance: f64::from(n.distance),
        }),
        landing: s.landing.map(|p| wire::Predicted {
            at: v3(p.at),
            verdict: match p.verdict {
                Course::OnSlot => wire::Course::OnSlot,
                Course::Fixable => wire::Course::Fixable,
                Course::Miss => wire::Course::Miss,
            },
            speed: f64::from(p.speed),
            need: f64::from(p.need),
        }),
        last_landing: s.last_landing.map(|l| wire::Landing {
            result: match l.result {
                LandingResult::Bounce => wire::LandingResult::Bounce,
                LandingResult::NotPrimed => wire::LandingResult::NotPrimed,
                LandingResult::Missed => wire::LandingResult::Missed,
                LandingResult::TooSlow => wire::LandingResult::TooSlow,
                LandingResult::NoBounce => wire::LandingResult::NoBounce,
            },
            tick: l.tick,
            at: v3(l.at),
        }),
        standing_fits: s.standing_fits,
    }
}

/// `superbounce.state` takes no parameters: null or `{}`.
pub fn no_params(params: &serde_json::Value) -> Result<(), &'static str> {
    if params.is_null() || params.as_object().is_some_and(|f| f.is_empty()) {
        Ok(())
    } else {
        Err("superbounce.state takes no parameters")
    }
}

/// `superbounce.state` as the agent answers it.
pub fn state_reply() -> agent_proto::superbounce::State {
    reply(&state())
}

// ---- the scan ----

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ScanKey {
    map: String,
    /// The loaded map file's header checksum, so a different file of the
    /// same name (an editor or Map Pack override) is scanned again.
    checksum: u32,
    bsp: usize,
}

/// One scan's result: the spots, and the collision they were found in
/// (for the standing-capsule test).
struct Scanned {
    spots: Vec<Spot>,
    room: Room,
}

#[derive(Default)]
struct Scans {
    current: Option<ScanKey>,
    spots: Arc<Vec<Spot>>,
    /// The current key's collision; empty until its scan is ready.
    room: Arc<Room>,
    status: ScanState,
    cache: HashMap<ScanKey, (Arc<Vec<Spot>>, Arc<Room>)>,
    /// Scans running now, so leaving a map and coming back does not start a
    /// second one.
    inflight: HashSet<ScanKey>,
}

fn scans() -> &'static Mutex<Scans> {
    static S: OnceLock<Mutex<Scans>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

/// The spots of the structure BSP `bsp` of an open map, and its collision
/// for the capsule test (`superbounce::scan_map` is `scan` over
/// `map_polys`). Isolated so that the scan taking a raw resource source is
/// a one-line change here.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
fn scan_one<R: Read + Seek>(
    map: &mut map::Map<R>,
    raw: &mut dyn tags::geometry::RawSource,
    bsp: usize,
) -> Result<Scanned, String> {
    let _ = &raw;
    let polys = superbounce::map_polys(map, bsp).map_err(|e| e.to_string())?;
    Ok(Scanned {
        spots: superbounce::scan(&polys),
        room: Room::new(&polys),
    })
}

#[cfg(feature = "gpu")]
fn scan(key: &ScanKey) -> Result<Scanned, String> {
    let (mut map, files) = crate::native_map::open(&key.map).map_err(|e| e.to_string())?;
    if map.header().checksum != key.checksum {
        return Err("the mounted map file is not the loaded one".into());
    }
    let mut raw = crate::native_world_glue::Raw(files);
    scan_one(&mut map, &mut raw, key.bsp)
}

#[cfg(not(feature = "gpu"))]
fn scan(_: &ScanKey) -> Result<Scanned, String> {
    Err("this build has no map access".into())
}

/// What a tick sees of the scan: the key's spots, starting the scan the
/// first time the key is seen.
struct ScanView {
    spots: Arc<Vec<Spot>>,
    room: Arc<Room>,
    status: ScanState,
}

fn request_scan(key: ScanKey) -> ScanView {
    let mut s = lock(scans());
    if s.current.as_ref() != Some(&key) {
        s.current = Some(key.clone());
        if let Some((spots, room)) = s.cache.get(&key).cloned() {
            s.spots = spots;
            s.room = room;
            s.status = ScanState::Ready;
        } else if s.inflight.contains(&key) {
            s.spots = Arc::default();
            s.room = Arc::default();
            s.status = ScanState::Scanning;
        } else {
            s.spots = Arc::default();
            s.room = Arc::default();
            s.status = ScanState::Scanning;
            s.inflight.insert(key.clone());
            let k = key.clone();
            let spawned = std::thread::Builder::new()
                .name("superbounce-scan".into())
                .spawn(move || {
                    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&k)))
                        .unwrap_or_else(|_| Err("the scan panicked".into()));
                    finish_scan(k, found);
                });
            if let Err(e) = spawned {
                s.inflight.remove(&key);
                s.status = ScanState::Failed(format!("no scan thread: {e}"));
            }
        }
    }
    ScanView {
        spots: Arc::clone(&s.spots),
        room: Arc::clone(&s.room),
        status: s.status.clone(),
    }
}

fn finish_scan(key: ScanKey, found: Result<Scanned, String>) {
    let mut s = lock(scans());
    s.inflight.remove(&key);
    let here = s.current.as_ref() == Some(&key);
    match found {
        Ok(Scanned { spots, room }) => {
            eprintln!(
                "superbounce: {} spots on {} (BSP {})",
                spots.len(),
                key.map,
                key.bsp
            );
            let (spots, room) = (Arc::new(spots), Arc::new(room));
            s.cache.insert(key, (Arc::clone(&spots), Arc::clone(&room)));
            if here {
                s.spots = spots;
                s.room = room;
                s.status = ScanState::Ready;
            }
        }
        Err(why) => {
            eprintln!("superbounce: no spots on {} (BSP {}): {why}", key.map, key.bsp);
            if here {
                s.status = ScanState::Failed(why);
            }
        }
    }
}

/// The spots of the current scan, for the world lines.
pub fn current_spots() -> Arc<Vec<Spot>> {
    Arc::clone(&lock(scans()).spots)
}

/// The collision of the current scan, for the standing-capsule test;
/// empty until the scan is ready.
pub fn current_room() -> Arc<Room> {
    Arc::clone(&lock(scans()).room)
}

/// Whether the standing capsule fits at `origin` on the loaded map, while
/// the mode is on and its scan is ready (`world.object`'s
/// `physics.standing_fits`); `None` otherwise.
pub fn standing_fits_at(origin: Vec3) -> Option<bool> {
    if !active() {
        return None;
    }
    let room = current_room();
    (!room.is_empty()).then(|| room.standing_fits(origin))
}

/// `world.object`'s `physics.standing_fits`, from [`standing_fits_at`].
pub struct StandingFitsNow;

impl agent::StandingFits for StandingFitsNow {
    fn standing_fits(&mut self, origin: [f64; 3]) -> Option<bool> {
        standing_fits_at(origin.map(|v| v as f32))
    }
}

// ---- reading the game ----

fn forced() -> bool {
    static F: OnceLock<bool> = OnceLock::new();
    *F.get_or_init(|| std::env::var(ENV).is_ok_and(|v| v == "1"))
}

/// The active variant's name in the game globals, in a multiplayer game.
fn variant_name(m: &dyn GuestMemory) -> Option<String> {
    use crate::skulls_custom::{GAME_GLOBALS, GLOBALS_MODE};
    let g = m.u32(GAME_GLOBALS).ok().filter(|&g| g != 0)?;
    if m.u32(g.wrapping_add(GLOBALS_MODE)).ok()? != crate::skulls_mp::MODE_MULTIPLAYER {
        return None;
    }
    let mut b = [0u8; 2 * variants::record::NAME_UNITS];
    m.read(
        g.wrapping_add(crate::variant_ext::ACTIVE_VARIANT + variants::record::NAME_AT as u32),
        &mut b,
    )
    .ok()?;
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// The agent's override (`superbounce.set`): [`FOLLOW`], [`FORCE_ON`] or
/// [`FORCE_OFF`].
static OVERRIDE: AtomicU8 = AtomicU8::new(FOLLOW);
const FOLLOW: u8 = 0;
const FORCE_ON: u8 = 1;
const FORCE_OFF: u8 = 2;

/// `superbounce.set`: `Some(true)` forces the mode on, `Some(false)` off,
/// `None` follows the game again. Returns the override's name. The tick
/// picks it up at its next check (within [`RECHECK`] ticks).
pub fn set_override(on: Option<bool>) -> &'static str {
    let (v, name) = match on {
        Some(true) => (FORCE_ON, "on"),
        Some(false) => (FORCE_OFF, "off"),
        None => (FOLLOW, "game"),
    };
    OVERRIDE.store(v, Ordering::Relaxed);
    name
}

/// Whether the mode is on in the game running now.
fn on_now(m: &dyn GuestMemory) -> bool {
    match OVERRIDE.load(Ordering::Relaxed) {
        FORCE_ON => return true,
        FORCE_OFF => return false,
        _ => {}
    }
    forced()
        || crate::variant_ext::in_game(&crate::layout_glue::ReadGm(m)).superbounce()
        || variant_name(m).is_some_and(|n| is_superbounce_name(&n))
}

/// The local player's word, position and motion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Live {
    word: Option<u32>,
    position: Option<Vec3>,
    airborne: bool,
    vz: Option<f32>,
    yaw: Option<f32>,
    /// World units a second.
    velocity: Option<Vec3>,
    /// The crouch request, unit flags bit 23.
    request: bool,
    /// The game tick.
    tick: Option<u32>,
}

/// The game tick, from the game-time record.
fn game_tick(m: &dyn GuestMemory) -> Option<u32> {
    use live_telemetry::layout::{GAME_TIME_PTR, TIME_TICK};
    let time = m.u32(GAME_TIME_PTR).ok().filter(|&t| t != 0)?;
    m.u32(time.wrapping_add(TIME_TICK)).ok()
}

fn read_live(m: &dyn GuestMemory) -> Live {
    let tick = game_tick(m);
    let Ok(units) = crate::skulls_custom::local_units(m) else {
        return Live {
            tick,
            ..Live::default()
        };
    };
    let Some(u) = units.first() else {
        return Live {
            tick,
            ..Live::default()
        };
    };
    let position = (|| {
        let at = u.body.wrapping_add(OBJECT_POSITION);
        let f = |i: u32| m.u32(at.wrapping_add(4 * i)).ok().map(f32::from_bits);
        Some([f(0)?, f(1)?, f(2)?])
    })();
    let word = live_telemetry::havok_agents::fixed_pair_word(m, u.unit)
        .ok()
        .flatten();
    use live_telemetry::layout::{
        BIPED_FLAGS, BODY_VELOCITY, UNIT_AIM, UNIT_FLAG_CROUCH, UNIT_FLAGS,
    };
    let f = |at: u32| {
        m.u32(u.body.wrapping_add(at))
            .ok()
            .map(f32::from_bits)
            .filter(|v| v.is_finite())
    };
    let vz = f(BODY_VELOCITY + 8);
    let velocity = (|| Some([f(BODY_VELOCITY)?, f(BODY_VELOCITY + 4)?, vz?]))();
    let request = m
        .u32(u.body.wrapping_add(UNIT_FLAGS))
        .is_ok_and(|flags| flags & UNIT_FLAG_CROUCH != 0);
    let yaw = match (f(UNIT_AIM), f(UNIT_AIM + 4)) {
        (Some(x), Some(y)) if x != 0.0 || y != 0.0 => Some(y.atan2(x)),
        _ => None,
    };
    // Bit 0 of the biped flags, as `world.object` reads `physics.airborne`.
    let airborne = u.kind == engine::view::objects::BIPED
        && m.u16(u.body.wrapping_add(BIPED_FLAGS))
            .is_ok_and(|flags| flags & 1 != 0);
    Live {
        word,
        position,
        airborne,
        vz,
        yaw,
        velocity,
        request,
        tick,
    }
}

/// The loaded map's cache header checksum (`+0x2d0` in the header the name
/// is at `+0x198` of).
const CACHE_CHECKSUM: u32 = live_telemetry::layout::CACHE_MAP_NAME - 0x198 + 0x2d0;

/// The scan key of the loaded map and its active structure BSP.
fn scan_key(m: &dyn GuestMemory) -> Option<ScanKey> {
    use crate::skulls_custom::{GAME_GLOBALS, GLOBALS_ACTIVE_BSP};
    let map = crate::health::loaded_map_name(m).ok().filter(|n| !n.is_empty())?;
    let g = m.u32(GAME_GLOBALS).ok().filter(|&g| g != 0)?;
    let bsp = m.u16(g.wrapping_add(GLOBALS_ACTIVE_BSP)).ok()? as i16;
    Some(ScanKey {
        map,
        checksum: m.u32(CACHE_CHECKSUM).ok()?,
        bsp: usize::try_from(bsp).ok()?,
    })
}

/// How often (in ticks) a mode that is off, or on, is decided again, so a
/// guest's late host block is picked up. A second at 30 Hz.
const RECHECK: u32 = 30;
static TICKS: AtomicU32 = AtomicU32::new(0);
/// The charge and landing state kept from tick to tick.
static TRACKER: Mutex<Tracker> = Mutex::new(Tracker::new());

fn reset() {
    ACTIVE.store(false, Ordering::Relaxed);
    *lock(&STATE) = State::EMPTY;
    *lock(&TRACKER) = Tracker::new();
}

/// Decides on or off from the game now. Going off clears the state.
fn decide(o: &hooks::Obs) {
    let on = {
        let guest = o.guest();
        on_now(&*guest)
    };
    if ACTIVE.swap(on, Ordering::Relaxed) && !on {
        *lock(&STATE) = State::EMPTY;
        *lock(&TRACKER) = Tracker::new();
    }
}

fn on_tick(o: &hooks::Obs) {
    // Off costs a counter and a flag: the decision is made at the game's
    // start and again every RECHECK ticks.
    if TICKS.fetch_add(1, Ordering::Relaxed) % RECHECK == 0 {
        decide(o);
    }
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let (live, key) = {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        (read_live(m), scan_key(m))
    };
    let map = key.as_ref().map(|k| k.map.clone());
    let view = key.map(request_scan);
    let (spots, room, scan) = match view {
        Some(v) => (v.spots, v.room, v.status),
        None => (Arc::default(), Arc::default(), ScanState::Idle),
    };
    let nearest = live.position.and_then(|p| {
        superbounce::nearest(&spots, p).map(|(s, d)| Near {
            distance: d,
            launch: s.launch,
            fall: s.fall,
            edge: s.edge,
        })
    });
    let count = |k: Kind| spots.iter().filter(|s| s.kind == k).count();
    let standing_fits = live
        .position
        .filter(|_| !room.is_empty())
        .map(|p| room.standing_fits(p));
    let step = lock(&TRACKER).step(
        &Tick {
            tick: live.tick,
            word: live.word,
            position: live.position,
            velocity: live.velocity,
            airborne: live.airborne,
            request: live.request,
            crouch_held: crate::superbounce_assist::crouch_held(),
            standing_fits,
        },
        &spots,
    );
    let landing = match (live.position, live.velocity) {
        (Some(position), Some(velocity)) => crate::superbounce_assist::predict(
            &spots,
            &crate::superbounce_assist::Body {
                position,
                velocity,
                yaw: live.yaw.unwrap_or(0.0),
                airborne: live.airborne,
            },
        ),
        _ => None,
    };
    *lock(&STATE) = State {
        active: true,
        word: live.word,
        position: live.position,
        nearest,
        scan,
        map,
        bounce: count(Kind::Bounce),
        crouch: count(Kind::Crouch),
        airborne: live.airborne,
        vz: live.vz,
        yaw: live.yaw,
        charge: step.charge,
        verb: step.verb,
        nav_target: step.nav_target,
        landing,
        last_landing: step.last_landing,
        still_ticks: step.still_ticks,
        standing_fits,
    };
}

// ---- the feature ----

/// The tick observer that publishes the state.
pub struct SuperbounceMode;

impl hooks::Feature for SuperbounceMode {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce game type: HUD and spot lines")
            .doc("docs/game/superbounce.md")
            // A tick observer and start and end observers only: a recorded
            // film's hook digest does not include it.
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // State is rebuilt from guest RAM every tick and the scan cache holds
        // only map-derived spots, so a snapshot needs nothing from us. The
        // charge tracker (still ticks, the lost second, the last landing)
        // is presentation only: after a load it restarts from the next tick.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.game().on_start("superbounce: a new game", |o| {
            reset();
            TICKS.store(1, Ordering::Relaxed);
            decide(o);
        });
        b.game().on_end("superbounce: the game ended", |_| reset());
        b.frame().on_tick("superbounce: state", on_tick);
        Ok(())
    }
}

/// Adds the feature to `registry`.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&SuperbounceMode)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("superbounce: {e}"))
}

/// Registers the world-line source before boot (windowed hosts).
#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use super::*;
    use crate::world_lines::{ImageMarks, WorldLine, WorldLineSource};

    /// Draws the spot lines while the mode is on.
    pub struct Lines;

    pub fn register(host: &mut crate::Host) {
        host.add_world_line_source(Arc::new(Lines));
    }

    impl WorldLineSource for Lines {
        fn active(&self) -> bool {
            super::active()
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let s = state();
            let spots = current_spots();
            let callouts = crate::superbounce_world::frame(guest, &s, &spots);
            (!callouts.is_empty()).then(|| Arc::new(Sample { callouts }) as Arc<dyn ImageMarks>)
        }
    }

    #[derive(Debug)]
    struct Sample {
        callouts: Vec<crate::superbounce_prompts::Callout>,
    }

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            for c in &self.callouts {
                visitor(WorldLine {
                    edges: &c.edges,
                    anchor: c.anchor,
                    color: c.color,
                    dashed: false,
                    label: c.label.as_deref(),
                    style: c.style,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_override_names_itself_and_can_be_undone() {
        assert_eq!(set_override(Some(true)), "on");
        assert_eq!(OVERRIDE.load(Ordering::Relaxed), FORCE_ON);
        assert_eq!(set_override(Some(false)), "off");
        assert_eq!(OVERRIDE.load(Ordering::Relaxed), FORCE_OFF);
        assert_eq!(set_override(None), "game");
        assert_eq!(OVERRIDE.load(Ordering::Relaxed), FOLLOW);
    }

    #[test]
    fn the_word_is_primed_at_minus_three_only() {
        assert!(primed(0xffff_fffd));
        assert!(primed((-3i32) as u32));
        for w in [0, 1, 0xffff_fffe, 0xffff_fffc, 3] {
            assert!(!primed(w), "{w:#x}");
        }
        let s = State {
            word: Some(0xffff_fffd),
            ..State::EMPTY
        };
        assert!(s.primed());
        assert!(!State::EMPTY.primed());
    }

    fn near(distance: f32) -> Near {
        Near {
            distance,
            launch: 17.85,
            fall: 0.0,
            edge: [[0.0; 3]; 2],
        }
    }

    #[test]
    fn the_bearing_turns_clockwise_from_where_the_player_looks() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let edge = |x: f32, y: f32| [[x, y - 1.0, 0.0], [x, y + 1.0, 0.0]];
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
        // Facing +X: a spot ahead, to the right (-Y), behind.
        assert!(close(bearing([0.0; 3], 0.0, edge(5.0, 0.0)), 0.0));
        let right = [[0.0, -5.0, 0.0], [0.0, -5.0, 0.0]];
        assert!(close(bearing([0.0; 3], 0.0, right), FRAC_PI_2));
        let behind = bearing([0.0; 3], 0.0, edge(-5.0, 0.0)).unwrap();
        assert!((behind.abs() - PI).abs() < 1e-4);
        // Facing +Y (yaw pi/2), a spot on +X is to the right.
        assert!(close(
            bearing([0.0; 3], FRAC_PI_2, edge(5.0, 0.0)),
            FRAC_PI_2
        ));
        // The edge's nearest point, not its middle; height is ignored.
        let long = [[2.0, 0.0, 9.0], [2.0, 50.0, 9.0]];
        assert!(close(bearing([0.0; 3], 0.0, long), 0.0));
        // On the spot, or without an aim, there is no arrow.
        assert_eq!(bearing([5.0, 0.0, 0.0], 0.0, edge(5.0, 0.0)), None);
        assert_eq!(bearing([0.0; 3], f32::NAN, edge(5.0, 0.0)), None);
        let s = State {
            position: Some([-5.0, 0.0, 0.0]),
            yaw: Some(0.0),
            nearest: Some(near(5.0)),
            ..State::EMPTY
        };
        assert!(close(s.bearing(), 0.0));
        assert_eq!(State { yaw: None, ..s }.bearing(), None);
    }

    #[test]
    fn the_state_method_takes_no_parameters() {
        use serde_json::json;
        assert!(no_params(&json!(null)).is_ok());
        assert!(no_params(&json!({})).is_ok());
        assert!(no_params(&json!({"x": 1})).is_err());
        assert!(no_params(&json!([1])).is_err());
    }

    #[test]
    fn the_variant_is_told_by_its_name_with_or_without_the_icon() {
        assert!(is_superbounce_name("Superbounce"));
        assert!(is_superbounce_name("Superbounce \u{e006}"));
        assert!(is_superbounce_name("Superbounce \u{e008}"));
        assert!(!is_superbounce_name("Bounce Practice \u{e006}"));
        assert!(!is_superbounce_name("Superbounce 2"));
        assert!(!is_superbounce_name(""));
    }

    #[test]
    fn the_agent_reply_carries_the_word_both_ways() {
        let s = State {
            active: true,
            word: Some(0xffff_fffd),
            position: Some([1.0, 2.0, 3.0]),
            nearest: Some(near(2.0)),
            scan: ScanState::Failed("x".into()),
            map: Some("ascension".into()),
            bounce: 3,
            crouch: 1,
            airborne: true,
            vz: Some(12.5),
            yaw: Some(0.5),
            ..State::EMPTY
        };
        let r = reply(&s);
        assert!(r.active && r.primed);
        assert_eq!(r.word.as_deref(), Some("fffffffd"));
        assert_eq!(r.word_signed, Some(-3));
        assert_eq!(r.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(r.scan.status, "failed");
        assert_eq!(r.scan.error.as_deref(), Some("x"));
        assert_eq!((r.scan.bounce, r.scan.crouch), (3, 1));
        assert_eq!(r.airborne, Some(true));
        assert_eq!((r.vz, r.yaw), (Some(12.5), Some(0.5)));
        let idle = reply(&State::EMPTY);
        assert_eq!((idle.airborne, idle.vz, idle.yaw), (None, None, None));
        assert_eq!(idle.scan.status, "idle");
        assert!(!idle.active && !idle.primed && idle.word.is_none());
        assert!(idle.charge.is_none() && idle.nav_target.is_none());
        assert!(idle.landing.is_none() && idle.last_landing.is_none());
        assert!(idle.verb.is_none() && idle.standing_fits.is_none());
    }

    #[test]
    fn the_agent_reply_carries_the_charge_the_target_and_the_landings() {
        use agent_proto::superbounce as wire;
        let s = State {
            active: true,
            charge: Some(Charge {
                phase: ChargePhase::Primed,
                fill: 0.5,
            }),
            still_ticks: 19,
            verb: Some(Verb::WalkOutToStand),
            nav_target: Some(NavTarget {
                at: [1.0, 2.0, 3.0],
                kind: NavKind::Strip,
                distance: 4.0,
            }),
            landing: Some(Predicted {
                at: [1.0, 2.0, -3.0],
                verdict: Course::Fixable,
                speed: 5.5,
                need: 4.0,
            }),
            last_landing: Some(Landing {
                result: LandingResult::TooSlow,
                tick: 6753,
                at: [0.5, 0.5, -3.0],
            }),
            standing_fits: Some(false),
            ..State::EMPTY
        };
        let r = reply(&s);
        let c = r.charge.unwrap();
        assert_eq!(
            (c.phase, c.fill, c.still_ticks),
            (wire::ChargePhase::Primed, 0.5, 19)
        );
        assert_eq!(r.verb, Some(wire::Verb::WalkOutToStand));
        let n = r.nav_target.unwrap();
        assert_eq!(
            (n.at, n.kind, n.distance),
            ([1.0, 2.0, 3.0], wire::NavKind::Strip, 4.0)
        );
        let p = r.landing.unwrap();
        assert_eq!(
            (p.verdict, p.speed, p.need),
            (wire::Course::Fixable, 5.5, 4.0)
        );
        let l = r.last_landing.unwrap();
        assert_eq!(
            (l.result, l.tick, l.at),
            (wire::LandingResult::TooSlow, 6753, [0.5, 0.5, -3.0])
        );
        assert_eq!(r.standing_fits, Some(false));
    }

    #[test]
    fn standing_fits_is_absent_while_the_mode_is_off() {
        ACTIVE.store(false, Ordering::Relaxed);
        assert_eq!(standing_fits_at([0.0; 3]), None);
    }
}
