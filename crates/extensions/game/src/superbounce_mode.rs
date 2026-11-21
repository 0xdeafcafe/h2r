//! The superbounce game type (docs/game/superbounce.md, "The superbounce
//! game type"): a HUD plate that says whether the local player's level
//! pair is primed, lines over the map for the spots a primed landing
//! launches from, and the `superbounce.state` agent method.
//!
//! **When it is on.** The active variant carries the extension block's
//! superbounce flag ([`variants::ext::Ext::superbounce`], through
//! [`crate::variant_ext::Decision::superbounce`]), or is the built-in
//! "Superbounce" variant ([`crate::community_variants`], which has no field
//! for the flag, so it is told by name), or `H2_SUPERBOUNCE=1` forces it.
//!
//! **Each tick** (a tick observer, only while on) it reads the first local
//! player's unit, its position and the word of its one pair against the
//! level (`live_telemetry::havok_agents::fixed_pair`, pair `+0x0c`;
//! -3 is primed), asks for the loaded map's spots (a background scan, once
//! per map and structure BSP, cached), finds the nearest bounce spot and
//! publishes all of it as one [`State`]. The HUD plate
//! ([`plate_lines`], drawn by `app`'s watermark renderer), the world lines
//! ([`gpu::Lines`]) and the agent method ([`reply`]) read that state.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use engine::GuestMemory;
use superbounce::{Kind, Spot, Vec3};

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
/// The fall (world units) at which a bounce line is fully red.
pub const FALL_RED: f32 = 3.0;
/// A crouch spot's colour: always drawn bright.
pub const CROUCH_COLOUR: [f32; 3] = [0.0, 0.85, 1.0];

// ---- pure rules ----

/// Whether a level pair word is the primed one (-3).
pub fn primed(word: u32) -> bool {
    word as i32 == PRIMED_WORD
}

/// A bounce line's colour for the fall that reaches it: green for a
/// standing jump (0), through yellow, to red at [`FALL_RED`] and more.
pub fn fall_colour(fall: f32) -> [f32; 3] {
    let t = if fall.is_nan() {
        0.0
    } else {
        (fall / FALL_RED).clamp(0.0, 1.0)
    };
    // Hue 120 degrees (green) down to 0 (red), full saturation and value.
    let h = 2.0 * (1.0 - t);
    if h < 1.0 {
        [1.0, h, 0.0]
    } else {
        [(2.0 - h).max(0.0), 1.0, 0.0]
    }
}

/// `colour` as drawn: dimmed unless the player is primed.
pub fn shade(colour: [f32; 3], primed: bool) -> [f32; 3] {
    if primed {
        colour
    } else {
        colour.map(|c| c * DIM)
    }
}

/// A variant name (as the record or the list holds it, with the list's
/// trailing icon) that is the built-in Superbounce.
pub fn is_superbounce_name(name: &str) -> bool {
    name.trim_end_matches(|c: char| c.is_whitespace() || ('\u{e000}'..='\u{f8ff}').contains(&c))
        == VARIANT_NAME
}

/// One line to draw: a spot, styled.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub edge: [Vec3; 2],
    pub anchor: Vec3,
    pub color: [f32; 3],
    pub dashed: bool,
    pub label: String,
}

/// How `spot` is drawn when the player is, or is not, `primed`.
pub fn line_for(spot: &Spot, primed: bool) -> Line {
    let anchor = [
        (spot.edge[0][0] + spot.edge[1][0]) / 2.0,
        (spot.edge[0][1] + spot.edge[1][1]) / 2.0,
        (spot.edge[0][2] + spot.edge[1][2]) / 2.0,
    ];
    match spot.kind {
        Kind::Bounce => Line {
            edge: spot.edge,
            anchor,
            color: shade(fall_colour(spot.fall), primed),
            dashed: false,
            label: format!("+{:.1}", spot.launch),
        },
        Kind::Crouch => Line {
            edge: spot.edge,
            anchor,
            color: CROUCH_COLOUR,
            dashed: true,
            label: "crouch".into(),
        },
    }
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The lines for the `max` spots nearest `at` (by their edges' middle),
/// nearest first. None without a position.
pub fn lines(spots: &[Spot], at: Option<Vec3>, primed: bool, max: usize) -> Vec<Line> {
    let Some(at) = at else { return Vec::new() };
    let mut near: Vec<(f32, &Spot)> = spots
        .iter()
        .map(|s| {
            let l = line_for(s, primed);
            (distance(l.anchor, at), s)
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.into_iter()
        .take(max)
        .map(|(_, s)| line_for(s, primed))
        .collect()
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
    };

    pub fn primed(&self) -> bool {
        self.word.is_some_and(primed)
    }
}

/// The plate's lines for `s`: how the pair stands, then the nearest spot.
pub fn hud_lines(s: &State) -> Vec<String> {
    let first = match s.word {
        Some(w) if primed(w) => format!("SUPERBOUNCE  PRIMED ({PRIMED_WORD})"),
        Some(w) => format!("SUPERBOUNCE  not primed (word {})", w as i32),
        None => "SUPERBOUNCE  not primed (no pair)".into(),
    };
    let second = match &s.scan {
        ScanState::Scanning | ScanState::Idle if s.nearest.is_none() => "scanning map...".into(),
        ScanState::Failed(why) => format!("no spots: {why}"),
        _ => match s.nearest {
            Some(n) if s.primed() => format!(
                "nearest spot {:.1} m  launch +{:.1}",
                n.distance * METRES_PER_UNIT,
                n.launch
            ),
            Some(n) => format!(
                "nearest spot {:.1} m  crouch-stand to prime",
                n.distance * METRES_PER_UNIT
            ),
            None => "no bounce spots on this map".into(),
        },
    };
    vec![first, second]
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

/// The HUD plate's lines; empty while the mode is off. `app`'s watermark
/// asks for them each refresh.
pub fn plate_lines() -> Vec<String> {
    let s = state();
    if s.active { hud_lines(&s) } else { Vec::new() }
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
    bsp: usize,
}

#[derive(Default)]
struct Scans {
    current: Option<ScanKey>,
    spots: Arc<Vec<Spot>>,
    status: ScanState,
    cache: HashMap<ScanKey, Arc<Vec<Spot>>>,
}

fn scans() -> &'static Mutex<Scans> {
    static S: OnceLock<Mutex<Scans>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

/// The spots of the structure BSP `bsp` of an open map. Isolated so that
/// `superbounce::scan_map` taking a raw resource source is a one-line
/// change here.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
fn scan_one<R: Read + Seek>(
    map: &mut map::Map<R>,
    raw: &mut dyn tags::geometry::RawSource,
    bsp: usize,
) -> Result<Vec<Spot>, String> {
    let _ = &raw;
    superbounce::scan_map(map, bsp).map_err(|e| e.to_string())
}

#[cfg(feature = "gpu")]
fn scan(key: &ScanKey) -> Result<Vec<Spot>, String> {
    let (mut map, files) = crate::native_map::open(&key.map).map_err(|e| e.to_string())?;
    let mut raw = crate::native_world_glue::Raw(files);
    scan_one(&mut map, &mut raw, key.bsp)
}

#[cfg(not(feature = "gpu"))]
fn scan(_: &ScanKey) -> Result<Vec<Spot>, String> {
    Err("this build has no map access".into())
}

/// What a tick sees of the scan: the key's spots, starting the scan the
/// first time the key is seen.
struct ScanView {
    spots: Arc<Vec<Spot>>,
    status: ScanState,
}

fn request_scan(key: ScanKey) -> ScanView {
    let mut s = lock(scans());
    if s.current.as_ref() != Some(&key) {
        s.current = Some(key.clone());
        if let Some(spots) = s.cache.get(&key).cloned() {
            s.spots = spots;
            s.status = ScanState::Ready;
        } else {
            s.spots = Arc::default();
            s.status = ScanState::Scanning;
            let k = key.clone();
            let spawned = std::thread::Builder::new()
                .name("superbounce-scan".into())
                .spawn(move || {
                    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&k)))
                        .unwrap_or_else(|_| Err("the scan panicked".into()));
                    finish_scan(k, found);
                });
            if let Err(e) = spawned {
                s.status = ScanState::Failed(format!("no scan thread: {e}"));
            }
        }
    }
    ScanView {
        spots: Arc::clone(&s.spots),
        status: s.status.clone(),
    }
}

fn finish_scan(key: ScanKey, found: Result<Vec<Spot>, String>) {
    let mut s = lock(scans());
    let here = s.current.as_ref() == Some(&key);
    match found {
        Ok(spots) => {
            eprintln!(
                "superbounce: {} spots on {} (BSP {})",
                spots.len(),
                key.map,
                key.bsp
            );
            let spots = Arc::new(spots);
            s.cache.insert(key, Arc::clone(&spots));
            if here {
                s.spots = spots;
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

/// Whether the mode is on in the game running now.
fn on_now(m: &dyn GuestMemory) -> bool {
    forced()
        || crate::variant_ext::in_game(&crate::layout_glue::ReadGm(m)).superbounce()
        || variant_name(m).is_some_and(|n| is_superbounce_name(&n))
}

/// The local player's word and position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Live {
    word: Option<u32>,
    position: Option<Vec3>,
}

fn read_live(m: &dyn GuestMemory) -> Live {
    let Ok(units) = crate::skulls_custom::local_units(m) else {
        return Live::default();
    };
    let Some(u) = units.first() else {
        return Live::default();
    };
    let position = (|| {
        let at = u.body.wrapping_add(OBJECT_POSITION);
        let f = |i: u32| m.u32(at.wrapping_add(4 * i)).ok().map(f32::from_bits);
        Some([f(0)?, f(1)?, f(2)?])
    })();
    let word = live_telemetry::havok_agents::fixed_pair(m, u.unit)
        .ok()
        .flatten()
        .and_then(|entry| {
            m.u32(entry.wrapping_add(live_telemetry::havok_agents::PAIR_WORD))
                .ok()
        });
    Live { word, position }
}

/// The scan key of the loaded map and its active structure BSP.
fn scan_key(m: &dyn GuestMemory) -> Option<ScanKey> {
    use crate::skulls_custom::{GAME_GLOBALS, GLOBALS_ACTIVE_BSP};
    let map = crate::health::loaded_map_name(m).ok().filter(|n| !n.is_empty())?;
    let g = m.u32(GAME_GLOBALS).ok().filter(|&g| g != 0)?;
    let bsp = m.u16(g.wrapping_add(GLOBALS_ACTIVE_BSP)).ok()? as i16;
    Some(ScanKey {
        map,
        bsp: usize::try_from(bsp).ok()?,
    })
}

fn reset() {
    ACTIVE.store(false, Ordering::Relaxed);
    *lock(&STATE) = State::EMPTY;
}

fn on_tick(o: &hooks::Obs) {
    let (on, live, key) = {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        let on = on_now(m);
        if !on {
            (false, Live::default(), None)
        } else {
            (true, read_live(m), scan_key(m))
        }
    };
    if !on {
        if ACTIVE.swap(false, Ordering::Relaxed) {
            *lock(&STATE) = State::EMPTY;
        }
        return;
    }
    let map = key.as_ref().map(|k| k.map.clone());
    let view = key.map(request_scan);
    let (spots, scan) = match view {
        Some(v) => (v.spots, v.status),
        None => (Arc::default(), ScanState::Idle),
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
    *lock(&STATE) = State {
        active: true,
        word: live.word,
        position: live.position,
        nearest,
        scan,
        map,
        bounce: count(Kind::Bounce),
        crouch: count(Kind::Crouch),
    };
    ACTIVE.store(true, Ordering::Relaxed);
}

// ---- the feature ----

/// The tick observer that publishes the state.
pub struct SuperbounceMode;

impl hooks::Feature for SuperbounceMode {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce game type: HUD plate and spot lines")
            .doc("docs/game/superbounce.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.game().on_start("superbounce: a new game", |_| reset());
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

        fn capture(&self, _: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let s = state();
            let drawn = lines(&current_spots(), s.position, s.primed(), MAX_LINES);
            (!drawn.is_empty()).then(|| Arc::new(Sample(drawn)) as Arc<dyn ImageMarks>)
        }
    }

    #[derive(Debug)]
    struct Sample(Vec<Line>);

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            for l in &self.0 {
                visitor(WorldLine {
                    edges: std::slice::from_ref(&l.edge),
                    anchor: l.anchor,
                    color: l.color,
                    dashed: l.dashed,
                    label: Some(&l.label),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(kind: Kind, at: f32, fall: f32, launch: f32) -> Spot {
        Spot {
            kind,
            edge: [[at, 0.0, 0.0], [at, 2.0, 0.0]],
            stand: [at, -1.0, 0.0],
            fall,
            launch,
            drift: [0.0, 0.0],
            score: 1.0,
        }
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

    #[test]
    fn fall_goes_from_green_through_yellow_to_red() {
        assert_eq!(fall_colour(0.0), [0.0, 1.0, 0.0]);
        assert_eq!(fall_colour(1.5), [1.0, 1.0, 0.0]);
        assert_eq!(fall_colour(3.0), [1.0, 0.0, 0.0]);
        // Clamped past the ends, and a NaN is a standing jump.
        assert_eq!(fall_colour(40.0), [1.0, 0.0, 0.0]);
        assert_eq!(fall_colour(-2.0), [0.0, 1.0, 0.0]);
        assert_eq!(fall_colour(f32::NAN), [0.0, 1.0, 0.0]);
        // Between green and yellow only red rises; past yellow only green falls.
        let a = fall_colour(0.75);
        assert!(a[0] > 0.0 && a[0] < 1.0 && a[1] == 1.0);
        let b = fall_colour(2.25);
        assert!(b[0] == 1.0 && b[1] > 0.0 && b[1] < 1.0);
    }

    #[test]
    fn bounce_lines_dim_unless_primed_and_crouch_never() {
        let bounce = spot(Kind::Bounce, 0.0, 0.0, 17.9);
        let on = line_for(&bounce, true);
        let off = line_for(&bounce, false);
        assert_eq!(on.color, [0.0, 1.0, 0.0]);
        assert_eq!(off.color, [0.0, DIM, 0.0]);
        assert_eq!(on.label, "+17.9");
        let crouch = spot(Kind::Crouch, 0.0, 0.0, 0.0);
        assert_eq!(line_for(&crouch, true), line_for(&crouch, false));
        assert_eq!(line_for(&crouch, false).color, CROUCH_COLOUR);
        assert_eq!(line_for(&crouch, false).label, "crouch");
        assert_eq!(on.anchor, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn only_the_nearest_spots_are_drawn_nearest_first() {
        let spots: Vec<Spot> = (0..10)
            .rev()
            .map(|i| spot(Kind::Bounce, i as f32 * 10.0, 0.0, i as f32))
            .collect();
        let drawn = lines(&spots, Some([0.0, 1.0, 0.0]), true, 3);
        let labels: Vec<_> = drawn.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["+0.0", "+1.0", "+2.0"]);
        assert!(lines(&spots, None, true, 3).is_empty());
        assert_eq!(lines(&spots, Some([0.0; 3]), true, 99).len(), 10);
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
    fn the_plate_says_primed_and_the_launch() {
        let s = State {
            active: true,
            word: Some(0xffff_fffd),
            nearest: Some(near(2.0)),
            scan: ScanState::Ready,
            ..State::EMPTY
        };
        assert_eq!(
            hud_lines(&s),
            ["SUPERBOUNCE  PRIMED (-3)", "nearest spot 6.1 m  launch +17.9"]
        );
    }

    #[test]
    fn the_plate_says_how_to_prime_when_not_primed() {
        let s = State {
            active: true,
            word: Some(0),
            nearest: Some(near(1.0)),
            scan: ScanState::Ready,
            ..State::EMPTY
        };
        assert_eq!(
            hud_lines(&s),
            [
                "SUPERBOUNCE  not primed (word 0)",
                "nearest spot 3.0 m  crouch-stand to prime"
            ]
        );
        let none = State {
            word: None,
            nearest: None,
            scan: ScanState::Ready,
            ..s.clone()
        };
        assert_eq!(
            hud_lines(&none),
            [
                "SUPERBOUNCE  not primed (no pair)",
                "no bounce spots on this map"
            ]
        );
        let other = State {
            word: Some(5),
            ..s
        };
        assert_eq!(hud_lines(&other)[0], "SUPERBOUNCE  not primed (word 5)");
    }

    #[test]
    fn the_plate_says_scanning_and_failures() {
        let s = State {
            active: true,
            word: Some(0),
            scan: ScanState::Scanning,
            ..State::EMPTY
        };
        assert_eq!(hud_lines(&s)[1], "scanning map...");
        let f = State {
            scan: ScanState::Failed("no collision".into()),
            ..s
        };
        assert_eq!(hud_lines(&f)[1], "no spots: no collision");
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
        };
        let r = reply(&s);
        assert!(r.active && r.primed);
        assert_eq!(r.word.as_deref(), Some("fffffffd"));
        assert_eq!(r.word_signed, Some(-3));
        assert_eq!(r.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(r.scan.status, "failed");
        assert_eq!(r.scan.error.as_deref(), Some("x"));
        assert_eq!((r.scan.bounce, r.scan.crouch), (3, 1));
        let idle = reply(&State::EMPTY);
        assert_eq!(idle.scan.status, "idle");
        assert!(!idle.active && !idle.primed && idle.word.is_none());
    }
}
