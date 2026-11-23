//! The spatial trace (docs/runtime/sim.md, "The spatial trace";
//! docs/editor/playtest.md): where a `detail = "spatial"` match's players
//! were, spawned, fought, picked things up and got stuck, written beside
//! the report as `spatial.bin`.
//!
//! Pure, as the tally is: the boot glue (`sim_glue::spatial`) turns each
//! tick's snapshot into [`Pose`]s, passes the damage, kill and brain
//! observations on, and answers the two map questions through [`Ground`]
//! (the bots' nav polygon under a point, and whether a line between two
//! points is clear). [`Recorder`] keeps the events, [`Trace`] is the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::metrics::{KillEvent, KillKind};

/// The file's first bytes.
pub const MAGIC: &[u8; 4] = b"H2SP";
/// The file's format version.
pub const VERSION: u32 = 1;
/// The trace's file name in a run's directory.
pub const FILE_NAME: &str = "spatial.bin";
/// Game ticks a second.
pub const TICK_RATE: f32 = 30.0;
/// A position sample every this many ticks from the match's start: five a
/// second.
pub const SAMPLE_EVERY: u32 = 6;
/// A spawn this near a starting location (world units) is that location's.
pub const START_REACH: f32 = 2.0;
/// A pickup this near a netgame equipment placement is that placement's.
pub const ITEM_REACH: f32 = 2.5;
/// The eye over a biped's origin, for the lines a spawn is seen along.
pub const EYE: f32 = 0.6;

pub type V3 = [f32; 3];

/// A point and the bots' nav polygon under it, if any.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub pos: V3,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poly: Option<u32>,
}

impl Place {
    pub fn distance(&self, other: &Place) -> f32 {
        dist(self.pos, other.pos)
    }
}

/// A player starting location of the scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Start {
    pub place: Place,
    /// Its team (`scenario_layout`'s `team`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i16>,
}

/// A netgame equipment placement of the scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub place: Place,
    /// Its own respawn time, seconds; `None` for the collection's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respawn_s: Option<u16>,
}

/// The bots' nav mesh as the measures need it: each polygon's centre and
/// the links between polygons.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub centers: Vec<V3>,
    /// `(from, to)` polygon indices.
    pub links: Vec<(u32, u32)>,
}

/// What a trace is of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub map: String,
    pub game_type: String,
    pub seed: u64,
    pub team_play: bool,
    /// The game tick the match was counted from.
    pub start_tick: u32,
    /// The last game tick seen.
    pub end_tick: u32,
    #[serde(default)]
    pub starts: Vec<Start>,
    #[serde(default)]
    pub items: Vec<Item>,
    /// The nav mesh, when one was loaded (written after the header).
    #[serde(skip)]
    pub mesh: Option<Mesh>,
}

impl Header {
    /// Game seconds from the start to the last tick.
    pub fn seconds(&self) -> f32 {
        self.end_tick.saturating_sub(self.start_tick) as f32 / TICK_RATE
    }
}

/// A weapon object and, when known, its definition tag datum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Weapon {
    pub datum: u32,
    pub definition: Option<u32>,
}

/// One side of a damage or a kill.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Party {
    pub player: u8,
    pub team: Option<i8>,
    /// Where they were at the last sample of the tick; `None` with no unit.
    pub at: Option<Place>,
    /// What they held.
    pub weapon: Option<Weapon>,
}

/// The nearest living enemy at a spawn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Nearest {
    pub player: u8,
    pub distance: f32,
}

/// One thing that happened, at a game tick.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A living player's position and facing, every [`SAMPLE_EVERY`] ticks.
    Sample {
        tick: u32,
        player: u8,
        at: Place,
        /// Radians about Z from +X.
        yaw: f32,
        /// Radians up from the horizontal.
        pitch: f32,
    },
    /// A player came alive.
    Spawn {
        tick: u32,
        player: u8,
        team: Option<i8>,
        at: Place,
        /// The starting location within [`START_REACH`], by index.
        start: Option<u16>,
        nearest: Option<Nearest>,
        /// Living enemies with a clear line to the spawn; `None` with no
        /// walls loaded.
        in_sight: Option<u8>,
    },
    /// A damage report against a player's unit.
    Damage {
        tick: u32,
        attacker: Option<Party>,
        victim: Party,
        amount: f32,
    },
    /// A death, classified as the tally classifies it.
    Kill {
        tick: u32,
        killer: Option<Party>,
        victim: Party,
        kind: KillKind,
    },
    /// A weapon new to a living player's slots, or more grenades.
    Pickup {
        tick: u32,
        player: u8,
        at: Place,
        /// The weapon taken; `None` for grenades.
        weapon: Option<Weapon>,
        /// The netgame equipment placement within [`ITEM_REACH`].
        item: Option<u16>,
    },
    /// A bot's stuck spell: from `tick`, `ticks` long.
    Stuck {
        tick: u32,
        player: u8,
        at: Place,
        toward: Option<V3>,
        ticks: u32,
    },
    /// A bot dropped its path and planned again.
    Repath {
        tick: u32,
        player: u8,
        at: Place,
        toward: Option<V3>,
    },
}

impl Event {
    pub fn tick(&self) -> u32 {
        match self {
            Event::Sample { tick, .. }
            | Event::Spawn { tick, .. }
            | Event::Damage { tick, .. }
            | Event::Kill { tick, .. }
            | Event::Pickup { tick, .. }
            | Event::Stuck { tick, .. }
            | Event::Repath { tick, .. } => *tick,
        }
    }

    /// The segment between a damage's or a kill's two places.
    pub fn segment(&self) -> Option<(Place, Place)> {
        let (a, v) = match self {
            Event::Damage {
                attacker, victim, ..
            } => (attacker.as_ref()?, victim),
            Event::Kill { killer, victim, .. } => (killer.as_ref()?, victim),
            _ => return None,
        };
        if a.player == v.player {
            return None;
        }
        Some((a.at?, v.at?))
    }
}

/// A match's spatial trace.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub header: Header,
    /// In the order they were seen (game tick order, near enough: a hit
    /// or kill inside tick N+1 follows tick N's samples).
    pub events: Vec<Event>,
}

/// The trace file beside `report`: `report.json` → `spatial.bin`,
/// `<name>.report.json` → `<name>.spatial.bin`.
pub fn trace_path(report: &Path) -> PathBuf {
    let name = report
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let stem = stem
        .strip_suffix("report")
        .map(|s| s.trim_end_matches('.'))
        .unwrap_or(stem);
    let file = if stem.is_empty() {
        FILE_NAME.to_string()
    } else {
        format!("{stem}.{FILE_NAME}")
    };
    report.with_file_name(file)
}

impl Trace {
    /// The kill log as the report has it.
    pub fn kills(&self) -> Vec<KillEvent> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::Kill {
                    tick,
                    killer,
                    victim,
                    kind,
                } => Some(KillEvent {
                    tick: *tick,
                    killer: killer.map(|k| k.player),
                    victim: victim.player,
                    kind: *kind,
                }),
                _ => None,
            })
            .collect()
    }

    /// Whether the trace's kills are the report's, one for one; the first
    /// difference otherwise.
    pub fn agrees_with(&self, report_kills: &[KillEvent]) -> Result<(), String> {
        let mine = self.kills();
        if let Some(i) = (0..mine.len().max(report_kills.len()))
            .find(|&i| mine.get(i) != report_kills.get(i))
        {
            return Err(format!(
                "the trace's kill {i} is {:?}, the report's {:?} ({} against {})",
                mine.get(i),
                report_kills.get(i),
                mine.len(),
                report_kills.len()
            ));
        }
        Ok(())
    }

    pub fn count(&self, f: impl Fn(&Event) -> bool) -> usize {
        self.events.iter().filter(|e| f(e)).count()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(Vec::with_capacity(64 + self.events.len() * 32));
        w.0.extend_from_slice(MAGIC);
        w.u32(VERSION);
        let header = serde_json::to_vec(&self.header).unwrap_or_default();
        w.u32(header.len() as u32);
        w.0.extend_from_slice(&header);
        match &self.header.mesh {
            None => w.u8(0),
            Some(m) => {
                w.u8(1);
                w.u32(m.centers.len() as u32);
                for c in &m.centers {
                    w.v3(*c);
                }
                w.u32(m.links.len() as u32);
                for &(a, b) in &m.links {
                    w.u32(a);
                    w.u32(b);
                }
            }
        }
        w.u32(self.events.len() as u32);
        for e in &self.events {
            w.event(e);
        }
        w.0
    }

    pub fn from_bytes(b: &[u8]) -> Result<Trace, String> {
        let mut r = Reader { b, at: 0 };
        if r.take(4)? != MAGIC {
            return Err("not a spatial trace (no H2SP)".into());
        }
        let version = r.u32()?;
        if version != VERSION {
            return Err(format!(
                "spatial trace version {version}; this h2 reads {VERSION}"
            ));
        }
        let len = r.u32()? as usize;
        let mut header: Header = serde_json::from_slice(r.take(len)?)
            .map_err(|e| format!("the trace's header: {e}"))?;
        header.mesh = match r.u8()? {
            0 => None,
            1 => {
                let n = r.count(12)?;
                let centers = (0..n).map(|_| r.v3()).collect::<Result<_, _>>()?;
                let n = r.count(8)?;
                let links = (0..n)
                    .map(|_| Ok((r.u32()?, r.u32()?)))
                    .collect::<Result<_, String>>()?;
                Some(Mesh { centers, links })
            }
            t => return Err(format!("the trace's mesh flag {t}")),
        };
        let n = r.count(1)?;
        let mut events = Vec::with_capacity(n);
        for _ in 0..n {
            events.push(r.event()?);
        }
        if r.at != b.len() {
            return Err(format!("{} bytes after the last event", b.len() - r.at));
        }
        Ok(Trace { header, events })
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.to_bytes())
            .map_err(|e| format!("writing {}: {e}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Trace, String> {
        let b = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        Trace::from_bytes(&b).map_err(|e| format!("{}: {e}", path.display()))
    }
}

// ---- The file's encoding: little-endian, an event a tag and its fields ----

const NO_U32: u32 = u32::MAX;
const NO_U16: u16 = u16::MAX;
const NO_U8: u8 = u8::MAX;
const NO_TEAM: i8 = i8::MIN;

const T_SAMPLE: u8 = 1;
const T_SPAWN: u8 = 2;
const T_DAMAGE: u8 = 3;
const T_KILL: u8 = 4;
const T_PICKUP: u8 = 5;
const T_STUCK: u8 = 6;
const T_REPATH: u8 = 7;

fn kind_code(k: KillKind) -> u8 {
    match k {
        KillKind::Kill => 0,
        KillKind::Suicide => 1,
        KillKind::Betrayal => 2,
        KillKind::World => 3,
    }
}

fn kind_of(c: u8) -> Result<KillKind, String> {
    Ok(match c {
        0 => KillKind::Kill,
        1 => KillKind::Suicide,
        2 => KillKind::Betrayal,
        3 => KillKind::World,
        c => return Err(format!("kill kind {c}")),
    })
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn v3(&mut self, v: V3) {
        v.iter().for_each(|c| self.f32(*c));
    }
    fn opt_v3(&mut self, v: Option<V3>) {
        self.u8(v.is_some() as u8);
        if let Some(v) = v {
            self.v3(v);
        }
    }
    fn place(&mut self, p: &Place) {
        self.v3(p.pos);
        self.u32(p.poly.unwrap_or(NO_U32));
    }
    fn opt_place(&mut self, p: Option<&Place>) {
        self.u8(p.is_some() as u8);
        if let Some(p) = p {
            self.place(p);
        }
    }
    fn weapon(&mut self, w: Option<Weapon>) {
        self.u8(w.is_some() as u8);
        if let Some(w) = w {
            self.u32(w.datum);
            self.u32(w.definition.unwrap_or(NO_U32));
        }
    }
    fn party(&mut self, p: &Party) {
        self.u8(p.player);
        self.0.push(p.team.unwrap_or(NO_TEAM) as u8);
        self.opt_place(p.at.as_ref());
        self.weapon(p.weapon);
    }
    fn opt_party(&mut self, p: Option<&Party>) {
        self.u8(p.is_some() as u8);
        if let Some(p) = p {
            self.party(p);
        }
    }
    fn event(&mut self, e: &Event) {
        match e {
            Event::Sample {
                tick,
                player,
                at,
                yaw,
                pitch,
            } => {
                self.u8(T_SAMPLE);
                self.u32(*tick);
                self.u8(*player);
                self.place(at);
                self.f32(*yaw);
                self.f32(*pitch);
            }
            Event::Spawn {
                tick,
                player,
                team,
                at,
                start,
                nearest,
                in_sight,
            } => {
                self.u8(T_SPAWN);
                self.u32(*tick);
                self.u8(*player);
                self.0.push(team.unwrap_or(NO_TEAM) as u8);
                self.place(at);
                self.u16(start.unwrap_or(NO_U16));
                self.u8(nearest.is_some() as u8);
                if let Some(n) = nearest {
                    self.u8(n.player);
                    self.f32(n.distance);
                }
                self.u8(in_sight.unwrap_or(NO_U8));
            }
            Event::Damage {
                tick,
                attacker,
                victim,
                amount,
            } => {
                self.u8(T_DAMAGE);
                self.u32(*tick);
                self.opt_party(attacker.as_ref());
                self.party(victim);
                self.f32(*amount);
            }
            Event::Kill {
                tick,
                killer,
                victim,
                kind,
            } => {
                self.u8(T_KILL);
                self.u32(*tick);
                self.opt_party(killer.as_ref());
                self.party(victim);
                self.u8(kind_code(*kind));
            }
            Event::Pickup {
                tick,
                player,
                at,
                weapon,
                item,
            } => {
                self.u8(T_PICKUP);
                self.u32(*tick);
                self.u8(*player);
                self.place(at);
                self.weapon(*weapon);
                self.u16(item.unwrap_or(NO_U16));
            }
            Event::Stuck {
                tick,
                player,
                at,
                toward,
                ticks,
            } => {
                self.u8(T_STUCK);
                self.u32(*tick);
                self.u8(*player);
                self.place(at);
                self.opt_v3(*toward);
                self.u32(*ticks);
            }
            Event::Repath {
                tick,
                player,
                at,
                toward,
            } => {
                self.u8(T_REPATH);
                self.u32(*tick);
                self.u8(*player);
                self.place(at);
                self.opt_v3(*toward);
            }
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len());
        let Some(end) = end else {
            return Err(format!("the trace ends early, at byte {}", self.b.len()));
        };
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap_or_default()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap_or_default()))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap_or_default()))
    }
    /// A count of items at least `each` bytes long, checked against what
    /// is left so a bad count cannot ask for a huge allocation.
    fn count(&mut self, each: usize) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n.saturating_mul(each) > self.b.len() - self.at {
            return Err(format!("a count of {n} is more than the file holds"));
        }
        Ok(n)
    }
    fn flag(&mut self) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            f => Err(format!("flag byte {f}")),
        }
    }
    fn v3(&mut self) -> Result<V3, String> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn opt_v3(&mut self) -> Result<Option<V3>, String> {
        Ok(if self.flag()? { Some(self.v3()?) } else { None })
    }
    fn place(&mut self) -> Result<Place, String> {
        let pos = self.v3()?;
        let poly = Some(self.u32()?).filter(|&p| p != NO_U32);
        Ok(Place { pos, poly })
    }
    fn opt_place(&mut self) -> Result<Option<Place>, String> {
        Ok(if self.flag()? { Some(self.place()?) } else { None })
    }
    fn team(&mut self) -> Result<Option<i8>, String> {
        Ok(Some(self.u8()? as i8).filter(|&t| t != NO_TEAM))
    }
    fn weapon(&mut self) -> Result<Option<Weapon>, String> {
        if !self.flag()? {
            return Ok(None);
        }
        let datum = self.u32()?;
        let definition = Some(self.u32()?).filter(|&d| d != NO_U32);
        Ok(Some(Weapon { datum, definition }))
    }
    fn party(&mut self) -> Result<Party, String> {
        Ok(Party {
            player: self.u8()?,
            team: self.team()?,
            at: self.opt_place()?,
            weapon: self.weapon()?,
        })
    }
    fn opt_party(&mut self) -> Result<Option<Party>, String> {
        Ok(if self.flag()? { Some(self.party()?) } else { None })
    }
    fn event(&mut self) -> Result<Event, String> {
        let tag = self.u8()?;
        let tick = self.u32()?;
        Ok(match tag {
            T_SAMPLE => Event::Sample {
                tick,
                player: self.u8()?,
                at: self.place()?,
                yaw: self.f32()?,
                pitch: self.f32()?,
            },
            T_SPAWN => Event::Spawn {
                tick,
                player: self.u8()?,
                team: self.team()?,
                at: self.place()?,
                start: Some(self.u16()?).filter(|&s| s != NO_U16),
                nearest: if self.flag()? {
                    Some(Nearest {
                        player: self.u8()?,
                        distance: self.f32()?,
                    })
                } else {
                    None
                },
                in_sight: Some(self.u8()?).filter(|&n| n != NO_U8),
            },
            T_DAMAGE => Event::Damage {
                tick,
                attacker: self.opt_party()?,
                victim: self.party()?,
                amount: self.f32()?,
            },
            T_KILL => Event::Kill {
                tick,
                killer: self.opt_party()?,
                victim: self.party()?,
                kind: kind_of(self.u8()?)?,
            },
            T_PICKUP => Event::Pickup {
                tick,
                player: self.u8()?,
                at: self.place()?,
                weapon: self.weapon()?,
                item: Some(self.u16()?).filter(|&i| i != NO_U16),
            },
            T_STUCK => Event::Stuck {
                tick,
                player: self.u8()?,
                at: self.place()?,
                toward: self.opt_v3()?,
                ticks: self.u32()?,
            },
            T_REPATH => Event::Repath {
                tick,
                player: self.u8()?,
                at: self.place()?,
                toward: self.opt_v3()?,
            },
            t => return Err(format!("event tag {t} at byte {}", self.at - 5)),
        })
    }
}

// ---- The recorder ----

/// The two map questions the trace asks, answered by the bots' nav mesh
/// and walls in a run; [`NoGround`] answers neither.
pub trait Ground {
    /// The nav polygon a biped standing at `p` is on.
    fn poly(&self, _p: V3) -> Option<u32> {
        None
    }
    /// Whether lines can be told: the map's walls are loaded.
    fn sees(&self) -> bool {
        false
    }
    /// Whether the line `a`-`b` is clear of the map's collision (asked
    /// only when [`Ground::sees`]).
    fn clear(&self, _a: V3, _b: V3) -> bool {
        true
    }
}

/// No mesh, no walls.
pub struct NoGround;

impl Ground for NoGround {}

/// One player after a tick, as the glue reads it from the snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pose {
    pub player: u8,
    pub team: Option<i32>,
    /// Their unit is alive.
    pub alive: bool,
    pub pos: V3,
    /// Where they aim (or face), unit length.
    pub facing: V3,
    /// What they hold.
    pub weapon: Option<Weapon>,
    /// The four weapon slots' object datums (`u32::MAX` empty).
    pub weapons: Option<[u32; 4]>,
    pub grenades: Option<u32>,
}

impl Pose {
    fn team8(&self) -> Option<i8> {
        self.team.and_then(|t| i8::try_from(t).ok())
    }
}

/// A stuck spell under way.
#[derive(Clone, Debug)]
struct Spell {
    tick: u32,
    at: Place,
    toward: Option<V3>,
    ticks: u32,
}

/// Turns the glue's observations into a [`Trace`].
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    trace: Trace,
    last_tick: Option<u32>,
    poses: BTreeMap<u8, Pose>,
    spells: BTreeMap<u8, Spell>,
    repaths: BTreeMap<u8, u32>,
}

impl Recorder {
    /// A recorder for the match `header` names; its places are set by
    /// [`Recorder::set_places`] and its ticks by [`Recorder::start`].
    pub fn new(header: Header) -> Recorder {
        Recorder {
            trace: Trace {
                header,
                events: Vec::new(),
            },
            ..Default::default()
        }
    }

    /// The scenario's starting locations and equipment placements.
    pub fn set_places(&mut self, starts: Vec<Start>, items: Vec<Item>) {
        self.trace.header.starts = starts;
        self.trace.header.items = items;
    }

    /// The match (re)starts at `tick`: every event so far is dropped, as
    /// the tally drops its own.
    pub fn start(&mut self, tick: u32) {
        self.trace.events.clear();
        self.trace.header.start_tick = tick;
        self.trace.header.end_tick = tick;
        self.last_tick = None;
        self.poses.clear();
        self.spells.clear();
        self.repaths.clear();
    }

    pub fn events(&self) -> &[Event] {
        &self.trace.events
    }

    fn place(ground: &dyn Ground, pos: V3) -> Place {
        Place {
            pos,
            poly: ground.poly(pos),
        }
    }

    fn party(&self, player: u8, ground: &dyn Ground) -> Party {
        let pose = self.poses.get(&player);
        Party {
            player,
            team: pose.and_then(Pose::team8),
            at: pose.map(|p| Self::place(ground, p.pos)),
            weapon: pose.and_then(|p| p.weapon),
        }
    }

    fn nearest_start(&self, pos: V3) -> Option<u16> {
        nearest(self.trace.header.starts.iter().map(|s| s.place.pos), pos, START_REACH)
    }

    fn nearest_item(&self, pos: V3) -> Option<u16> {
        nearest(self.trace.header.items.iter().map(|i| i.place.pos), pos, ITEM_REACH)
    }

    fn enemies(&self, a: &Pose, b: &Pose) -> bool {
        a.player != b.player
            && (!self.trace.header.team_play || a.team.is_none() || a.team != b.team)
    }

    /// After game tick `tick`: spawns, pickups and the position samples.
    pub fn tick(&mut self, tick: u32, poses: &[Pose], ground: &dyn Ground) {
        self.last_tick = Some(tick);
        self.trace.header.end_tick = tick;
        let sample = tick.wrapping_sub(self.trace.header.start_tick) % SAMPLE_EVERY == 0;
        for p in poses {
            let before = self.poses.get(&p.player).cloned();
            let was_alive = before.as_ref().is_some_and(|b| b.alive);
            if p.alive && !was_alive {
                self.spawn(tick, p, poses, ground);
            } else if !p.alive && was_alive {
                self.end_spell(p.player);
            } else if p.alive
                && let Some(b) = &before
            {
                for weapon in picked_up(b, p) {
                    let item = self.nearest_item(p.pos);
                    self.trace.events.push(Event::Pickup {
                        tick,
                        player: p.player,
                        at: Self::place(ground, p.pos),
                        weapon,
                        item,
                    });
                }
            }
            if p.alive && sample {
                let (yaw, pitch) = angles(p.facing);
                self.trace.events.push(Event::Sample {
                    tick,
                    player: p.player,
                    at: Self::place(ground, p.pos),
                    yaw,
                    pitch,
                });
            }
            self.poses.insert(p.player, p.clone());
        }
    }

    fn spawn(&mut self, tick: u32, p: &Pose, poses: &[Pose], ground: &dyn Ground) {
        let foes: Vec<&Pose> = poses
            .iter()
            .filter(|o| o.alive && self.enemies(p, o))
            .collect();
        let nearest = foes
            .iter()
            .map(|o| Nearest {
                player: o.player,
                distance: dist(p.pos, o.pos),
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance));
        let eye = |v: V3| [v[0], v[1], v[2] + EYE];
        let in_sight = ground.sees().then(|| {
            let n = foes
                .iter()
                .filter(|o| ground.clear(eye(o.pos), eye(p.pos)))
                .count();
            u8::try_from(n).unwrap_or(u8::MAX - 1)
        });
        let start = self.nearest_start(p.pos);
        self.trace.events.push(Event::Spawn {
            tick,
            player: p.player,
            team: p.team8(),
            at: Self::place(ground, p.pos),
            start,
            nearest,
            in_sight,
        });
    }

    /// A damage report inside the tick after the last one: `victim`
    /// (a player) hurt by `attacker` (if a player) by `amount`.
    pub fn damage(
        &mut self,
        tick: u32,
        attacker: Option<u8>,
        victim: u8,
        amount: f32,
        ground: &dyn Ground,
    ) {
        let attacker = attacker.map(|a| self.party(a, ground));
        let victim = self.party(victim, ground);
        self.trace.events.push(Event::Damage {
            tick,
            attacker,
            victim,
            amount: if amount.is_finite() { amount.max(0.0) } else { 0.0 },
        });
    }

    /// A death, as `PLAYER_KILLED` gives it to the tally
    /// ([`crate::metrics::Tally::kill`]), classified the same way.
    pub fn kill(
        &mut self,
        tick: u32,
        killer: Option<u8>,
        victim: u8,
        betrayal: bool,
        ground: &dyn Ground,
    ) {
        let kind = match killer {
            None => KillKind::World,
            Some(k) if k == victim => KillKind::Suicide,
            Some(_) if betrayal => KillKind::Betrayal,
            Some(_) => KillKind::Kill,
        };
        let killer = killer.map(|k| self.party(k, ground));
        let victim_party = self.party(victim, ground);
        self.end_spell(victim);
        self.trace.events.push(Event::Kill {
            tick,
            killer,
            victim: victim_party,
            kind,
        });
    }

    /// A bot's brain this tick: whether it is stuck, its repaths so far
    /// (a running count; the rises count, as the tally takes them) and
    /// where it is going.
    pub fn brain(
        &mut self,
        player: u8,
        stuck: bool,
        repaths: u32,
        toward: Option<V3>,
        ground: &dyn Ground,
    ) {
        let (Some(tick), Some(pose)) = (self.last_tick, self.poses.get(&player)) else {
            return;
        };
        let pos = pose.pos;
        let last = self.repaths.insert(player, repaths).unwrap_or(0);
        if repaths > last {
            self.trace.events.push(Event::Repath {
                tick,
                player,
                at: Self::place(ground, pos),
                toward,
            });
        }
        if stuck {
            match self.spells.get_mut(&player) {
                Some(s) => s.ticks += 1,
                None => {
                    self.spells.insert(
                        player,
                        Spell {
                            tick,
                            at: Self::place(ground, pos),
                            toward,
                            ticks: 1,
                        },
                    );
                }
            }
        } else {
            self.end_spell(player);
        }
    }

    fn end_spell(&mut self, player: u8) {
        if let Some(s) = self.spells.remove(&player) {
            self.trace.events.push(Event::Stuck {
                tick: s.tick,
                player,
                at: s.at,
                toward: s.toward,
                ticks: s.ticks,
            });
        }
    }

    /// The trace: the open stuck spells closed, the places' polygons
    /// found where they were not, and `mesh` attached.
    pub fn finish(mut self, ground: &dyn Ground, mesh: Option<Mesh>) -> Trace {
        let open: Vec<u8> = self.spells.keys().copied().collect();
        for p in open {
            self.end_spell(p);
        }
        let h = &mut self.trace.header;
        for place in h
            .starts
            .iter_mut()
            .map(|s| &mut s.place)
            .chain(h.items.iter_mut().map(|i| &mut i.place))
        {
            if place.poly.is_none() {
                place.poly = ground.poly(place.pos);
            }
        }
        h.mesh = mesh;
        self.trace
    }
}

/// What a living player picked up since `before`: each weapon datum new to
/// their slots (the held one's definition when it is the new one), and
/// `None` for more grenades.
fn picked_up(before: &Pose, now: &Pose) -> Vec<Option<Weapon>> {
    let mut out = Vec::new();
    if let (Some(a), Some(b)) = (before.weapons, now.weapons) {
        for d in b.iter().filter(|d| **d != u32::MAX && !a.contains(d)) {
            let definition = now
                .weapon
                .filter(|w| w.datum == *d)
                .and_then(|w| w.definition);
            out.push(Some(Weapon {
                datum: *d,
                definition,
            }));
        }
    }
    if let (Some(a), Some(b)) = (before.grenades, now.grenades)
        && b > a
    {
        out.push(None);
    }
    out
}

/// The index of the nearest of `points` to `p` within `reach`.
fn nearest(points: impl Iterator<Item = V3>, p: V3, reach: f32) -> Option<u16> {
    points
        .enumerate()
        .map(|(i, q)| (i, dist(p, q)))
        .filter(|&(_, d)| d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .and_then(|(i, _)| u16::try_from(i).ok())
}

pub fn dist(a: V3, b: V3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Yaw about Z from +X and pitch up from the horizontal of `f`.
fn angles(f: V3) -> (f32, f32) {
    let yaw = f[1].atan2(f[0]);
    let flat = (f[0] * f[0] + f[1] * f[1]).sqrt();
    let pitch = f[2].atan2(flat);
    (
        if yaw.is_finite() { yaw } else { 0.0 },
        if pitch.is_finite() { pitch } else { 0.0 },
    )
}

// ---- The measures (docs/editor/playtest.md, "What is measured") ----

/// A bin of a layer: a nav polygon, or a [`GRID`]-unit XY square for a
/// place off the mesh (or a trace without one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cell {
    Poly(u32),
    Grid(i32, i32),
}

/// The side of a [`Cell::Grid`] square, world units.
pub const GRID: f32 = 2.0;

impl Place {
    pub fn cell(&self) -> Cell {
        match self.poly {
            Some(p) => Cell::Poly(p),
            None => Cell::Grid(
                (self.pos[0] / GRID).floor() as i32,
                (self.pos[1] / GRID).floor() as i32,
            ),
        }
    }
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cell::Poly(p) => write!(f, "polygon {p}"),
            Cell::Grid(x, y) => write!(f, "square ({x}, {y})"),
        }
    }
}

/// A measure refused for want of events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TooFew {
    pub measure: &'static str,
    /// What was counted (`kills`, `spawns`, ...).
    pub what: &'static str,
    pub have: usize,
    pub need: usize,
}

impl std::fmt::Display for TooFew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: too few {} ({}, at least {} needed)",
            self.measure, self.what, self.have, self.need
        )
    }
}

/// `Ok` when `have` reaches `need`, else the refusal.
pub fn enough(
    measure: &'static str,
    what: &'static str,
    have: usize,
    need: usize,
) -> Result<(), TooFew> {
    if have >= need {
        Ok(())
    } else {
        Err(TooFew {
            measure,
            what,
            have,
            need,
        })
    }
}

/// A running mean of positions.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mean {
    sum: [f64; 3],
    n: u64,
}

impl Mean {
    pub fn add(&mut self, p: V3) {
        for (s, c) in self.sum.iter_mut().zip(p) {
            *s += f64::from(c);
        }
        self.n += 1;
    }
    pub fn get(&self) -> V3 {
        let n = self.n.max(1) as f64;
        [
            (self.sum[0] / n) as f32,
            (self.sum[1] / n) as f32,
            (self.sum[2] / n) as f32,
        ]
    }
}

/// One bin of a counted layer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bin {
    pub cell: Cell,
    /// The mean of the places counted in it.
    pub at: V3,
    pub count: u32,
}

/// Places counted by [`Cell`].
#[derive(Clone, Debug, Default)]
pub struct Bins(BTreeMap<Cell, (Mean, u32)>);

impl Bins {
    pub fn add(&mut self, p: &Place) {
        let e = self.0.entry(p.cell()).or_default();
        e.0.add(p.pos);
        e.1 += 1;
    }
    /// The bins, the most counted first.
    pub fn into_vec(self) -> Vec<Bin> {
        let mut v: Vec<Bin> = self
            .0
            .into_iter()
            .map(|(cell, (m, count))| Bin {
                cell,
                at: m.get(),
                count,
            })
            .collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.cell.cmp(&b.cell)));
        v
    }
}

/// Every measure over a set of traces (one map's, over seeds), each
/// refused on its own when its events are too few.
#[derive(Clone, Debug)]
pub struct Measures {
    pub traces: usize,
    pub seconds: f32,
    pub heat: Result<heat::Heat, TooFew>,
    pub spawns: Result<spawns::Spawns, TooFew>,
    pub flow: Result<flow::Flow, TooFew>,
    pub sight: Result<sight::Sight, TooFew>,
    pub items: Result<items::Items, TooFew>,
    pub stuck: Result<stuck::Stuck, TooFew>,
}

pub fn measure(traces: &[Trace]) -> Measures {
    Measures {
        traces: traces.len(),
        seconds: traces.iter().map(|t| t.header.seconds()).sum(),
        heat: heat::heat(traces),
        spawns: spawns::spawns(traces),
        flow: flow::flow(traces),
        sight: sight::sight(traces),
        items: items::items(traces),
        stuck: stuck::stuck(traces),
    }
}

fn xyz(p: V3) -> String {
    format!("({:.1}, {:.1}, {:.1})", p[0], p[1], p[2])
}

fn secs(s: Option<f32>) -> String {
    s.map_or("-".into(), |s| format!("{s:.1} s"))
}

impl Measures {
    /// The measures as `h2 sim spatial` prints them.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut o = String::new();
        let _ = writeln!(
            o,
            "{} trace(s), {:.0} game seconds",
            self.traces, self.seconds
        );
        let refused = |o: &mut String, e: &TooFew| {
            let _ = writeln!(o, "{e}");
        };
        match &self.heat {
            Ok(h) => {
                let _ = writeln!(
                    o,
                    "heat: {} deaths in {} bins, {} kills in {}",
                    h.deaths.iter().map(|b| b.count).sum::<u32>(),
                    h.deaths.len(),
                    h.kills.iter().map(|b| b.count).sum::<u32>(),
                    h.kills.len()
                );
                for b in h.deaths.iter().take(5) {
                    let _ = writeln!(o, "  deaths {:>4}  {} at {}", b.count, b.cell, xyz(b.at));
                }
            }
            Err(e) => refused(&mut o, e),
        }
        match &self.spawns {
            Ok(s) => {
                let _ = writeln!(o, "spawns: {} spawns, {} spawn kills", s.spawns, s.spawn_kills.len());
                let _ = writeln!(
                    o,
                    "  start  uses  life     <3 s deaths  in sight  first fight"
                );
                for r in &s.starts {
                    let _ = writeln!(
                        o,
                        "  {:>5}  {:>4}  {:>7}  {:>11}  {:>8}  {}",
                        r.start.map_or("-".into(), |s| s.to_string()),
                        r.uses,
                        secs(r.mean_life_s),
                        r.spawn_deaths,
                        r.mean_in_sight.map_or("-".into(), |n| format!("{n:.2}")),
                        secs(r.mean_first_engagement_s)
                    );
                }
                for (team, s) in &s.opening {
                    let _ = writeln!(o, "  team {team}'s first engagement: {s:.1} s");
                }
            }
            Err(e) => refused(&mut o, e),
        }
        match &self.flow {
            Ok(f) => {
                let _ = writeln!(
                    o,
                    "flow: {} samples over {} bins, {} crossings{}",
                    f.samples,
                    f.cells.len(),
                    f.crossings.iter().map(|c| c.count).sum::<u32>(),
                    f.never
                        .as_ref()
                        .map_or(String::new(), |n| format!(", {} polygons never entered", n.len()))
                );
                for d in f.cells.iter().take(5) {
                    let _ = writeln!(o, "  {:>7.1} s  {} at {}", d.seconds, d.cell, xyz(d.at));
                }
            }
            Err(e) => refused(&mut o, e),
        }
        match &self.sight {
            Ok(s) => {
                let _ = writeln!(
                    o,
                    "sight: {} segments, median {:.1} units, longest {:.1}",
                    s.segments,
                    s.median,
                    s.longest.first().map_or(0.0, |l| l.distance)
                );
                for (w, h) in &s.by_weapon {
                    let name = if *w == sight::UNKNOWN_WEAPON {
                        "unknown".to_string()
                    } else {
                        format!("{w:#010x}")
                    };
                    let _ = writeln!(o, "  {name}: {:?} (per {} units)", h, sight::BUCKET);
                }
            }
            Err(e) => refused(&mut o, e),
        }
        match &self.items {
            Ok(i) => {
                let _ = writeln!(o, "items: {} pickups, {} loose", i.pickups, i.loose);
                for p in &i.placements {
                    let _ = writeln!(
                        o,
                        "  item {:>3}  {:>4} pickups  {:>5.2}/min  first {}  {} kills{}",
                        p.item,
                        p.pickups,
                        p.per_minute,
                        secs(p.first_pickup_s),
                        p.kills,
                        if p.never { "  never picked" } else { "" }
                    );
                }
            }
            Err(e) => refused(&mut o, e),
        }
        match &self.stuck {
            Ok(s) => {
                let _ = writeln!(
                    o,
                    "stuck: {} spots{}",
                    s.spots.len(),
                    s.islands
                        .as_ref()
                        .map_or(String::new(), |i| format!(", {} islands no start reaches", i.len()))
                );
                for p in s.spots.iter().take(5) {
                    let _ = writeln!(
                        o,
                        "  {:>5} ticks  {} spells  {} repaths  {} at {}",
                        p.ticks,
                        p.spells,
                        p.repaths,
                        p.cell,
                        xyz(p.at)
                    );
                }
            }
            Err(e) => refused(&mut o, e),
        }
        o
    }
}

pub mod flow;
pub mod heat;
pub mod items;
pub mod sight;
pub mod spawns;
pub mod stuck;

#[cfg(test)]
mod tests;

/// Builders for the measures' invented traces.
#[cfg(test)]
pub(crate) mod invent {
    use super::*;

    /// A place on polygon `poly` at X = `x`.
    pub fn on(poly: u32, x: f32) -> Place {
        Place {
            pos: [x, 0.0, 0.0],
            poly: Some(poly),
        }
    }

    pub fn party(player: u8, team: i8, at: Place, weapon: Option<u32>) -> Party {
        Party {
            player,
            team: Some(team),
            at: Some(at),
            weapon: weapon.map(|d| Weapon {
                datum: d,
                definition: Some(0xd000 + d),
            }),
        }
    }

    pub fn kill(tick: u32, killer: Party, victim: Party) -> Event {
        Event::Kill {
            tick,
            killer: Some(killer),
            victim,
            kind: KillKind::Kill,
        }
    }

    pub fn damage(tick: u32, attacker: Party, victim: Party) -> Event {
        Event::Damage {
            tick,
            attacker: Some(attacker),
            victim,
            amount: 0.5,
        }
    }

    pub fn spawn(tick: u32, player: u8, team: i8, start: Option<u16>, at: Place) -> Event {
        Event::Spawn {
            tick,
            player,
            team: Some(team),
            at,
            start,
            nearest: None,
            in_sight: Some(0),
        }
    }

    pub fn sample(tick: u32, player: u8, at: Place) -> Event {
        Event::Sample {
            tick,
            player,
            at,
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    /// A trace of `ticks` ticks from 0 with `events`.
    pub fn trace(ticks: u32, events: Vec<Event>) -> Trace {
        Trace {
            header: Header {
                map: "invented".into(),
                game_type: "slayer".into(),
                team_play: true,
                end_tick: ticks,
                ..Default::default()
            },
            events,
        }
    }
}
