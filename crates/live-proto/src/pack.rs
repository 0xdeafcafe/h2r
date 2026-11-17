//! Packed telemetry frames (docs/live/live-telemetry.md, "Packed frames"): a
//! versioned bit-packed form of [`Frame`] for the ingest socket, the live
//! stream to the web and recorded frames at rest. JSON stays for the public
//! read API and the low-rate events.
//!
//! A frame is quantised (positions to 1/512 world unit, yaw to 12 bits,
//! directions octahedral 2 x 12 bits, ...) and written least significant
//! bit first. Numbers use Exp-Golomb codes, so small values are short.
//! A stream of frames from one [`Encoder`] sends a keyframe, then deltas
//! against the frame before: each entity of a list is matched with the
//! previous frame's by its key and sends only the fields that changed
//! (positions as differences), and runs of unchanged entities cost a few
//! bits. Strings go in a table that lives as long as the keyframe.
//!
//! `web/src/api/pack.ts` decodes the same format; the golden vectors in
//! `web/test/fixtures/pack` (written by this module's tests) prove both
//! sides agree.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use crate::game::{
    ACTIONS, Frame, FrameActor, FrameHit, FrameMachine, FrameObject, FramePlayer, FrameVehicle,
    GRENADES, MAX_FRAME_ACTIONS, MAX_FRAME_ACTORS, MAX_FRAME_HITS, MAX_FRAME_MACHINES,
    MAX_FRAME_OBJECTS, MAX_FRAME_VEHICLES, MAX_PLAYERS, ObjectKind, STANCES,
};

/// The packed format's version, the first four bits of every frame. Version
/// 2 adds a vehicle's parent and aim; version 1 frames (recorded ones, an
/// older game's) still decode.
pub const VERSION: u64 = 2;
/// Positions are whole multiples of 1/512 world unit (about 6 mm).
pub const POS_SCALE: f64 = 512.0;
/// Yaw: a full turn in 4096 steps.
pub const ANGLE_BITS: u32 = 12;
/// Directions: octahedral, two components of 12 bits.
pub const DIR_BITS: u32 = 12;
/// Speeds in 1/16 unit a second.
pub const SPEED_SCALE: f64 = 16.0;
/// Hit amounts in 1/256 of a vitality.
pub const AMOUNT_SCALE: f64 = 256.0;
/// Machine device positions (0..1) in 10 bits.
pub const DEVICE_BITS: u32 = 10;
/// Most strings a stream's table holds before the encoder starts over with
/// a keyframe.
pub const MAX_STRINGS: usize = 4096;
/// Longest string (bytes).
pub const MAX_STRING: usize = 255;
/// Most new strings one frame may add.
const MAX_NEW_STRINGS: u64 = 1024;
/// Positions beyond this many world units are refused.
const MAX_COORD: f64 = 1.0e9;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackError {
    #[error("packed frame ends early")]
    Short,
    #[error("packed frame: {0}")]
    Bad(&'static str),
    #[error("packed frame version {0} is not supported")]
    Version(u64),
    /// A delta frame whose base this decoder does not have (a dropped
    /// frame, or joining mid-stream): wait for the next keyframe.
    #[error("delta frame {seq} of stream {sid} needs a keyframe first")]
    Gap { sid: u64, seq: u64 },
    #[error("cannot pack frame: {0}")]
    Value(&'static str),
}

type Result<T> = std::result::Result<T, PackError>;

// ---------------------------------------------------------------- bits

/// Writes bits least significant first.
#[derive(Debug, Default, Clone)]
pub struct BitWriter {
    bytes: Vec<u8>,
    len: usize,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The low `n` bits of `v` (n <= 64).
    pub fn bits(&mut self, mut v: u64, mut n: u32) {
        debug_assert!(n == 64 || v >> n == 0, "{v} does not fit {n} bits");
        while n > 0 {
            let off = (self.len % 8) as u32;
            if off == 0 {
                self.bytes.push(0);
            }
            let take = (8 - off).min(n);
            let last = self.bytes.last_mut().unwrap();
            *last |= ((v & ((1 << take) - 1)) as u8) << off;
            v = v.checked_shr(take).unwrap_or(0);
            n -= take;
            self.len += take as usize;
        }
    }

    pub fn bit(&mut self, b: bool) {
        self.bits(u64::from(b), 1);
    }

    /// Exp-Golomb code of order `k`: `z` zeros, a one, then the low
    /// `z + k` bits of `v + 2^k`, where `v + 2^k` has `z + k + 1` bits.
    pub fn eg(&mut self, v: u64, k: u32) {
        debug_assert!(v < 1 << 44, "{v} is too large to code");
        let x = v + (1 << k);
        let n = 64 - x.leading_zeros();
        let z = n - 1 - k;
        self.bits(0, z);
        self.bit(true);
        self.bits(x & ((1 << (n - 1)) - 1), n - 1);
    }

    /// Signed Exp-Golomb: zig-zag (0, -1, 1, -2, ...) then [`Self::eg`].
    pub fn seg(&mut self, v: i64, k: u32) {
        self.eg(zigzag(v), k);
    }

    pub fn bit_len(&self) -> usize {
        self.len
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Reads what [`BitWriter`] wrote.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        BitReader { bytes, pos: 0 }
    }

    pub fn bits(&mut self, n: u32) -> Result<u64> {
        if self.pos + n as usize > self.bytes.len() * 8 {
            return Err(PackError::Short);
        }
        // Up to 56 bits lie in the eight bytes from the first one: one
        // little-endian word, shifted (when there are eight to read).
        let at = self.pos / 8;
        if n <= 56
            && let Some(word) = self.bytes.get(at..at + 8)
        {
            let word = u64::from_le_bytes(word.try_into().expect("eight bytes"));
            let v = (word >> (self.pos % 8)) & ((1u64 << n) - 1);
            self.pos += n as usize;
            return Ok(v);
        }
        let mut v = 0u64;
        let mut got = 0;
        while got < n {
            let off = (self.pos % 8) as u32;
            let take = (8 - off).min(n - got);
            let byte = u64::from(self.bytes[self.pos / 8] >> off) & ((1 << take) - 1);
            v |= byte << got;
            got += take;
            self.pos += take as usize;
        }
        Ok(v)
    }

    pub fn bit(&mut self) -> Result<bool> {
        Ok(self.bits(1)? == 1)
    }

    pub fn eg(&mut self, k: u32) -> Result<u64> {
        let mut z = 0;
        while !self.bit()? {
            z += 1;
            if z > 40 {
                return Err(PackError::Bad("number code too long"));
            }
        }
        if z + k > 52 {
            return Err(PackError::Bad("number too large"));
        }
        let low = self.bits(z + k)?;
        Ok(((1u64 << (z + k)) | low) - (1 << k))
    }

    pub fn seg(&mut self, k: u32) -> Result<i64> {
        Ok(unzigzag(self.eg(k)?))
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

// ------------------------------------------------------- quantisation

fn sign(c: f64) -> f64 {
    if c >= 0.0 { 1.0 } else { -1.0 }
}

const DIR_MAX: f64 = ((1u32 << DIR_BITS) - 2) as f64;

/// A direction as two octahedral components (`0..=4094`, 2047 is zero).
pub fn oct(v: [f64; 3]) -> Result<[u32; 2]> {
    let l1 = v[0].abs() + v[1].abs() + v[2].abs();
    if !l1.is_finite() || l1 < 1e-12 {
        return Err(PackError::Value("direction must be finite and non-zero"));
    }
    let (mut x, mut y) = (v[0] / l1, v[1] / l1);
    if v[2] < 0.0 {
        let (ax, ay) = (x.abs(), y.abs());
        x = (1.0 - ay) * sign(x);
        y = (1.0 - ax) * sign(y);
    }
    let q = |c: f64| ((c.clamp(-1.0, 1.0) + 1.0) * 0.5 * DIR_MAX).round() as u32;
    Ok([q(x), q(y)])
}

/// The unit vector [`oct`] packed.
pub fn unoct(q: [u32; 2]) -> [f64; 3] {
    let mut x = q[0] as f64 / DIR_MAX * 2.0 - 1.0;
    let mut y = q[1] as f64 / DIR_MAX * 2.0 - 1.0;
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        let (ax, ay) = (x.abs(), y.abs());
        x = (1.0 - ay) * sign(x);
        y = (1.0 - ax) * sign(y);
    }
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

fn qpos(v: [f64; 3]) -> Result<[i64; 3]> {
    if !v.iter().all(|c| c.is_finite() && c.abs() < MAX_COORD) {
        return Err(PackError::Value("position must be finite"));
    }
    Ok(v.map(|c| (c * POS_SCALE).round() as i64))
}

fn dpos(q: [i64; 3]) -> [f64; 3] {
    q.map(|c| c as f64 / POS_SCALE)
}

fn qyaw(yaw: f64) -> Result<u64> {
    if !yaw.is_finite() {
        return Err(PackError::Value("yaw must be finite"));
    }
    let steps = 1i64 << ANGLE_BITS;
    Ok(((yaw / TAU * steps as f64).round() as i64).rem_euclid(steps) as u64)
}

fn dyaw(q: u64) -> f64 {
    let a = q as f64 * TAU / (1u64 << ANGLE_BITS) as f64;
    if a > PI { a - TAU } else { a }
}

fn qfixed(v: f64, scale: f64, what: &'static str) -> Result<u64> {
    if !v.is_finite() || !(0.0..1.0e12).contains(&v) {
        return Err(PackError::Value(what));
    }
    Ok((v * scale).round() as u64)
}

const DEVICE_MAX: f64 = ((1u32 << DEVICE_BITS) - 1) as f64;

// ------------------------------------------------------------- values

/// An object id: a datum (8 hex digits), a player (`p<index>`) or text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Id {
    Datum(u32),
    Player(u32),
    Text(String),
}

impl Id {
    fn parse(s: &str) -> Id {
        if s.len() == 8
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Id::Datum(u32::from_str_radix(s, 16).unwrap());
        }
        if let Some(n) = s.strip_prefix('p')
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
            && (n == "0" || !n.starts_with('0'))
            && let Ok(index) = n.parse::<u32>()
            && index < 1 << 31
        {
            return Id::Player(index);
        }
        Id::Text(s.to_owned())
    }

    fn text(&self) -> String {
        match self {
            Id::Datum(d) => format!("{d:08x}"),
            Id::Player(p) => format!("p{p}"),
            Id::Text(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Val {
    None,
    Bool(bool),
    Int(u64),
    Pos([i64; 3]),
    Dir([u32; 2]),
    Str(String),
    Id(Id),
    List(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Bool,
    /// Three signed Exp-Golomb (k 16), as differences k 7 in a delta.
    Pos,
    /// [`ANGLE_BITS`] bits.
    Angle,
    /// Two [`DIR_BITS`] fields.
    Dir,
    /// A string table index (Exp-Golomb k 3).
    Str,
    /// Two bits: 0 datum (32 bits), 1 player (Exp-Golomb k 2), 2 string.
    Id,
    /// Exp-Golomb of order k.
    Uint(u32),
    /// A fixed number of bits.
    Bits(u32),
    /// A count (Exp-Golomb k 0), then 3 bits each.
    Actions,
}

#[derive(Clone, Copy)]
struct Field {
    kind: Kind,
    opt: bool,
    /// Part of the entity's key (never in a delta's changed mask).
    key: bool,
}

const fn f(kind: Kind) -> Field {
    Field {
        kind,
        opt: false,
        key: false,
    }
}
const fn o(kind: Kind) -> Field {
    Field {
        kind,
        opt: true,
        key: false,
    }
}
const fn k(kind: Kind) -> Field {
    Field {
        kind,
        opt: false,
        key: true,
    }
}

const PLAYER: &[Field] = &[
    k(Kind::Uint(2)), // index
    f(Kind::Bool),    // alive
    f(Kind::Pos),
    f(Kind::Angle),   // yaw
    o(Kind::Id),      // vehicle
    o(Kind::Str),     // species
    o(Kind::Dir),     // aim
    o(Kind::Str),     // weapon
    o(Kind::Uint(1)), // shots
    o(Kind::Bits(3)), // stance
    o(Kind::Uint(6)), // speed
    o(Kind::Actions),
    o(Kind::Bits(1)), // grenade
];
const VEHICLE: &[Field] = &[
    k(Kind::Id),
    f(Kind::Str), // kind
    f(Kind::Pos),
    f(Kind::Dir),  // forward
    f(Kind::Dir),  // up
    f(Kind::Bool), // occupied
    o(Kind::Id),   // parent (version 2)
    o(Kind::Dir),  // aim (version 2)
];
/// Version 1's vehicle: [`VEHICLE`] without its parent and aim.
const VEHICLE_V1: &[Field] = VEHICLE.split_at(6).0;
const ACTOR: &[Field] = &[
    k(Kind::Id),
    f(Kind::Str), // species
    f(Kind::Pos),
    f(Kind::Angle),
    o(Kind::Dir),     // aim
    o(Kind::Str),     // weapon
    o(Kind::Uint(1)), // shots
    o(Kind::Str),     // team
    o(Kind::Id),      // vehicle
    o(Kind::Bits(3)), // stance
    o(Kind::Uint(6)), // speed
    o(Kind::Actions),
    o(Kind::Bits(1)), // grenade
];
const HIT: &[Field] = &[
    o(Kind::Id),  // from
    o(Kind::Id),  // to
    o(Kind::Str), // weapon
    f(Kind::Pos),
    o(Kind::Uint(6)), // amount
    o(Kind::Str),     // kind
];
const MACHINE: &[Field] = &[
    o(Kind::Str), // node
    k(Kind::Id),
    f(Kind::Str), // kind
    f(Kind::Pos),
    f(Kind::Dir),
    f(Kind::Dir),
    o(Kind::Bits(DEVICE_BITS)), // position
];
const OBJECT: &[Field] = &[
    k(Kind::Bits(4)), // kind
    k(Kind::Uint(6)), // index
    f(Kind::Bool),    // gone
    o(Kind::Pos),
    o(Kind::Dir),
    o(Kind::Dir),
];

/// The delta-coded lists, in wire order (hits are always sent whole).
const LISTS: [(&[Field], usize); 5] = [
    (PLAYER, MAX_PLAYERS),
    (VEHICLE, MAX_FRAME_VEHICLES),
    (ACTOR, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
];
/// Version 1's lists.
const LISTS_V1: [(&[Field], usize); 5] = [
    (PLAYER, MAX_PLAYERS),
    (VEHICLE_V1, MAX_FRAME_VEHICLES),
    (ACTOR, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
];

/// The lists of a version this decoder reads.
fn lists_of(version: u64) -> Option<&'static [(&'static [Field], usize); 5]> {
    match version {
        1 => Some(&LISTS_V1),
        VERSION => Some(&LISTS),
        _ => None,
    }
}

type Entity = Vec<Val>;

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> Result<Val>) -> Result<Val> {
    v.map_or(Ok(Val::None), f)
}

fn enum_code(table: &[&str], s: &str, what: &'static str) -> Result<Val> {
    table
        .iter()
        .position(|t| *t == s)
        .map(|i| Val::Int(i as u64))
        .ok_or(PackError::Value(what))
}

fn actions_val(actions: &Option<Vec<String>>) -> Result<Val> {
    match actions.as_deref() {
        None | Some([]) => Ok(Val::None),
        Some(list) => {
            if list.len() > MAX_FRAME_ACTIONS {
                return Err(PackError::Value("too many actions"));
            }
            list.iter()
                .map(|a| {
                    ACTIONS
                        .iter()
                        .position(|t| t == a)
                        .map(|i| i as u8)
                        .ok_or(PackError::Value("unknown action"))
                })
                .collect::<Result<Vec<u8>>>()
                .map(Val::List)
        }
    }
}

fn string(s: &str) -> Result<Val> {
    if s.len() > MAX_STRING {
        return Err(PackError::Value("string too long"));
    }
    Ok(Val::Str(s.to_owned()))
}

fn id(s: &str) -> Result<Val> {
    if s.len() > MAX_STRING {
        return Err(PackError::Value("id too long"));
    }
    Ok(Val::Id(Id::parse(s)))
}

fn shots(n: Option<u32>) -> Val {
    n.filter(|n| *n > 0)
        .map_or(Val::None, |n| Val::Int(u64::from(n)))
}

fn player_vals(p: &FramePlayer) -> Result<Entity> {
    Ok(vec![
        Val::Int(u64::from(p.index)),
        Val::Bool(p.alive),
        Val::Pos(qpos(p.pos)?),
        Val::Int(qyaw(p.yaw)?),
        opt(p.vehicle.as_deref(), id)?,
        opt(p.species.as_deref(), string)?,
        opt(p.aim, |a| oct(a).map(Val::Dir))?,
        opt(p.weapon.as_deref(), string)?,
        shots(p.shots),
        opt(p.stance.as_deref(), |s| {
            enum_code(&STANCES, s, "unknown stance")
        })?,
        opt(p.speed, |s| qfixed(s, SPEED_SCALE, "speed").map(Val::Int))?,
        actions_val(&p.actions)?,
        opt(p.grenade.as_deref(), |g| {
            enum_code(&GRENADES, g, "unknown grenade")
        })?,
    ])
}

fn vehicle_vals(v: &FrameVehicle) -> Result<Entity> {
    Ok(vec![
        id(&v.id)?,
        string(&v.kind)?,
        Val::Pos(qpos(v.pos)?),
        Val::Dir(oct(v.forward)?),
        Val::Dir(oct(v.up)?),
        Val::Bool(v.occupied),
        opt(v.parent.as_deref(), id)?,
        opt(v.aim, |a| oct(a).map(Val::Dir))?,
    ])
}

fn actor_vals(a: &FrameActor) -> Result<Entity> {
    Ok(vec![
        id(&a.id)?,
        string(&a.species)?,
        Val::Pos(qpos(a.pos)?),
        Val::Int(qyaw(a.yaw)?),
        opt(a.aim, |v| oct(v).map(Val::Dir))?,
        opt(a.weapon.as_deref(), string)?,
        shots(a.shots),
        opt(a.team.as_deref(), string)?,
        opt(a.vehicle.as_deref(), id)?,
        opt(a.stance.as_deref(), |s| {
            enum_code(&STANCES, s, "unknown stance")
        })?,
        opt(a.speed, |s| qfixed(s, SPEED_SCALE, "speed").map(Val::Int))?,
        actions_val(&a.actions)?,
        opt(a.grenade.as_deref(), |g| {
            enum_code(&GRENADES, g, "unknown grenade")
        })?,
    ])
}

fn hit_vals(h: &FrameHit) -> Result<Entity> {
    Ok(vec![
        opt(h.from.as_deref(), id)?,
        opt(h.to.as_deref(), id)?,
        opt(h.weapon.as_deref(), string)?,
        Val::Pos(qpos(h.pos)?),
        opt(h.amount, |a| {
            qfixed(a, AMOUNT_SCALE, "hit amount").map(Val::Int)
        })?,
        opt(h.kind.as_deref(), string)?,
    ])
}

fn machine_vals(m: &FrameMachine) -> Result<Entity> {
    Ok(vec![
        opt(m.node.as_deref(), string)?,
        id(&m.id)?,
        string(&m.kind)?,
        Val::Pos(qpos(m.pos)?),
        Val::Dir(oct(m.forward)?),
        Val::Dir(oct(m.up)?),
        opt(m.position, |p| {
            if !(0.0..=1.0).contains(&p) {
                return Err(PackError::Value("machine position must be 0 to 1"));
            }
            Ok(Val::Int((p * DEVICE_MAX).round() as u64))
        })?,
    ])
}

fn object_vals(x: &FrameObject) -> Result<Entity> {
    if x.i >= 1 << 20 {
        return Err(PackError::Value("object index too large"));
    }
    Ok(vec![
        Val::Int(u64::from(x.k.code())),
        Val::Int(u64::from(x.i)),
        Val::Bool(x.gone),
        opt(x.p, |p| qpos(p).map(Val::Pos))?,
        opt(x.f, |v| oct(v).map(Val::Dir))?,
        opt(x.u, |v| oct(v).map(Val::Dir))?,
    ])
}

// Entity values back to frame types. Every value was written by the
// matching schema, so a mismatch is a decoder bug or a hostile frame.

fn as_int(v: &Val) -> Result<u64> {
    match v {
        Val::Int(n) => Ok(*n),
        _ => Err(PackError::Bad("expected a number")),
    }
}
fn as_opt_int(v: &Val) -> Result<Option<u64>> {
    match v {
        Val::None => Ok(None),
        v => as_int(v).map(Some),
    }
}
fn as_bool(v: &Val) -> Result<bool> {
    match v {
        Val::Bool(b) => Ok(*b),
        _ => Err(PackError::Bad("expected a flag")),
    }
}
fn as_pos(v: &Val) -> Result<[f64; 3]> {
    match v {
        Val::Pos(q) => Ok(dpos(*q)),
        _ => Err(PackError::Bad("expected a position")),
    }
}
fn as_opt_pos(v: &Val) -> Result<Option<[f64; 3]>> {
    match v {
        Val::None => Ok(None),
        v => as_pos(v).map(Some),
    }
}
fn as_dir(v: &Val) -> Result<[f64; 3]> {
    match v {
        Val::Dir(q) => Ok(unoct(*q)),
        _ => Err(PackError::Bad("expected a direction")),
    }
}
fn as_opt_dir(v: &Val) -> Result<Option<[f64; 3]>> {
    match v {
        Val::None => Ok(None),
        v => as_dir(v).map(Some),
    }
}
fn as_str(v: &Val) -> Result<String> {
    match v {
        Val::Str(s) => Ok(s.clone()),
        _ => Err(PackError::Bad("expected a string")),
    }
}
fn as_opt_str(v: &Val) -> Result<Option<String>> {
    match v {
        Val::None => Ok(None),
        v => as_str(v).map(Some),
    }
}
fn as_id(v: &Val) -> Result<String> {
    match v {
        Val::Id(id) => Ok(id.text()),
        _ => Err(PackError::Bad("expected an id")),
    }
}
fn as_opt_id(v: &Val) -> Result<Option<String>> {
    match v {
        Val::None => Ok(None),
        v => as_id(v).map(Some),
    }
}
fn as_enum(v: &Val, table: &[&str]) -> Result<Option<String>> {
    as_opt_int(v)?
        .map(|i| {
            table
                .get(i as usize)
                .map(|s| (*s).to_owned())
                .ok_or(PackError::Bad("enum value out of range"))
        })
        .transpose()
}
fn as_actions(v: &Val) -> Result<Option<Vec<String>>> {
    match v {
        Val::None => Ok(None),
        Val::List(l) => l
            .iter()
            .map(|a| {
                ACTIONS
                    .get(usize::from(*a))
                    .map(|s| (*s).to_owned())
                    .ok_or(PackError::Bad("unknown action"))
            })
            .collect::<Result<Vec<_>>>()
            .map(Some),
        _ => Err(PackError::Bad("expected actions")),
    }
}
fn as_u32(v: &Val) -> Result<u32> {
    u32::try_from(as_int(v)?).map_err(|_| PackError::Bad("number out of range"))
}

fn player_of(e: &Entity) -> Result<FramePlayer> {
    Ok(FramePlayer {
        index: as_u32(&e[0])?,
        alive: as_bool(&e[1])?,
        pos: as_pos(&e[2])?,
        yaw: dyaw(as_int(&e[3])?),
        vehicle: as_opt_id(&e[4])?,
        species: as_opt_str(&e[5])?,
        aim: as_opt_dir(&e[6])?,
        weapon: as_opt_str(&e[7])?,
        shots: as_opt_int(&e[8])?.map(|n| n as u32),
        stance: as_enum(&e[9], &STANCES)?,
        speed: as_opt_int(&e[10])?.map(|q| q as f64 / SPEED_SCALE),
        actions: as_actions(&e[11])?,
        grenade: as_enum(&e[12], &GRENADES)?,
    })
}

fn vehicle_of(e: &Entity) -> Result<FrameVehicle> {
    Ok(FrameVehicle {
        id: as_id(&e[0])?,
        kind: as_str(&e[1])?,
        pos: as_pos(&e[2])?,
        forward: as_dir(&e[3])?,
        up: as_dir(&e[4])?,
        occupied: as_bool(&e[5])?,
        // Version 1 vehicles have neither.
        parent: e.get(6).map(as_opt_id).transpose()?.flatten(),
        aim: e.get(7).map(as_opt_dir).transpose()?.flatten(),
    })
}

fn actor_of(e: &Entity) -> Result<FrameActor> {
    Ok(FrameActor {
        id: as_id(&e[0])?,
        species: as_str(&e[1])?,
        pos: as_pos(&e[2])?,
        yaw: dyaw(as_int(&e[3])?),
        aim: as_opt_dir(&e[4])?,
        weapon: as_opt_str(&e[5])?,
        shots: as_opt_int(&e[6])?.map(|n| n as u32),
        team: as_opt_str(&e[7])?,
        vehicle: as_opt_id(&e[8])?,
        stance: as_enum(&e[9], &STANCES)?,
        speed: as_opt_int(&e[10])?.map(|q| q as f64 / SPEED_SCALE),
        actions: as_actions(&e[11])?,
        grenade: as_enum(&e[12], &GRENADES)?,
    })
}

fn hit_of(e: &Entity) -> Result<FrameHit> {
    Ok(FrameHit {
        from: as_opt_id(&e[0])?,
        to: as_opt_id(&e[1])?,
        weapon: as_opt_str(&e[2])?,
        pos: as_pos(&e[3])?,
        amount: as_opt_int(&e[4])?.map(|q| q as f64 / AMOUNT_SCALE),
        kind: as_opt_str(&e[5])?,
    })
}

fn machine_of(e: &Entity) -> Result<FrameMachine> {
    Ok(FrameMachine {
        node: as_opt_str(&e[0])?,
        id: as_id(&e[1])?,
        kind: as_str(&e[2])?,
        pos: as_pos(&e[3])?,
        forward: as_dir(&e[4])?,
        up: as_dir(&e[5])?,
        position: as_opt_int(&e[6])?.map(|q| q as f64 / DEVICE_MAX),
    })
}

fn object_of(e: &Entity) -> Result<FrameObject> {
    Ok(FrameObject {
        k: ObjectKind::from_code(as_int(&e[0])? as u8)
            .ok_or(PackError::Bad("unknown object kind"))?,
        i: as_u32(&e[1])?,
        gone: as_bool(&e[2])?,
        p: as_opt_pos(&e[3])?,
        f: as_opt_dir(&e[4])?,
        u: as_opt_dir(&e[5])?,
    })
}

/// A frame as entity lists: players, vehicles, actors, machines, objects
/// (the [`LISTS`] order) and hits.
struct Lists {
    lists: [Vec<Entity>; 5],
    hits: Vec<Entity>,
}

fn collect<T>(items: &[T], f: fn(&T) -> Result<Entity>, max: usize) -> Result<Vec<Entity>> {
    if items.len() > max {
        return Err(PackError::Value("too many entities in a list"));
    }
    items.iter().map(f).collect()
}

impl Lists {
    fn of(frame: &Frame) -> Result<Lists> {
        Ok(Lists {
            lists: [
                collect(&frame.players, player_vals, MAX_PLAYERS)?,
                collect(&frame.vehicles, vehicle_vals, MAX_FRAME_VEHICLES)?,
                collect(&frame.actors, actor_vals, MAX_FRAME_ACTORS)?,
                collect(&frame.machines, machine_vals, MAX_FRAME_MACHINES)?,
                collect(&frame.objects, object_vals, MAX_FRAME_OBJECTS)?,
            ],
            hits: collect(&frame.hits, hit_vals, MAX_FRAME_HITS)?,
        })
    }

    /// Every string the lists refer to, in wire order.
    fn strings(&self) -> impl Iterator<Item = &str> {
        self.lists
            .iter()
            .take(3)
            .chain(std::iter::once(&self.hits))
            .chain(self.lists.iter().skip(3))
            .flatten()
            .flatten()
            .filter_map(|v| match v {
                Val::Str(s) | Val::Id(Id::Text(s)) => Some(s.as_str()),
                _ => None,
            })
    }
}

fn frame_of(
    game_id: &str,
    tick: u64,
    bsp: Option<u64>,
    lists: &[Vec<Entity>; 5],
    hits: &[Entity],
) -> Result<Frame> {
    // Sized up front: collecting through `Result` would grow it by doubling.
    fn map<T>(l: &[Entity], f: fn(&Entity) -> Result<T>) -> Result<Vec<T>> {
        let mut out = Vec::with_capacity(l.len());
        for e in l {
            out.push(f(e)?);
        }
        Ok(out)
    }
    Ok(Frame {
        game_id: game_id.to_owned(),
        tick,
        players: map(&lists[0], player_of)?,
        vehicles: map(&lists[1], vehicle_of)?,
        actors: map(&lists[2], actor_of)?,
        hits: map(hits, hit_of)?,
        bsp: bsp
            .map(|b| u32::try_from(b).map_err(|_| PackError::Bad("bsp out of range")))
            .transpose()?,
        machines: map(&lists[3], machine_of)?,
        objects: map(&lists[4], object_of)?,
    })
}

/// What `decode(encode(frame))` gives: the frame with every value
/// quantised as the packed format carries it.
pub fn quantize(frame: &Frame) -> Result<Frame> {
    let lists = Lists::of(frame)?;
    frame_of(
        &frame.game_id,
        frame.tick,
        frame.bsp.map(u64::from),
        &lists.lists,
        &lists.hits,
    )
}

// ------------------------------------------------------------- coding

/// A stream's string table as the encoder keeps it.
#[derive(Default, Clone)]
struct Table {
    index: HashMap<String, u64>,
}

fn write_str_literal(w: &mut BitWriter, s: &str) {
    w.eg(s.len() as u64, 3);
    for b in s.bytes() {
        w.bits(u64::from(b), 8);
    }
}

fn read_str_literal(r: &mut BitReader) -> Result<String> {
    let len = r.eg(3)?;
    if len > MAX_STRING as u64 {
        return Err(PackError::Bad("string too long"));
    }
    let bytes = (0..len)
        .map(|_| r.bits(8).map(|b| b as u8))
        .collect::<Result<Vec<u8>>>()?;
    String::from_utf8(bytes).map_err(|_| PackError::Bad("string is not UTF-8"))
}

fn write_value(w: &mut BitWriter, kind: Kind, v: &Val, prev: Option<&Val>, t: &Table) {
    match (kind, v) {
        (Kind::Bool, Val::Bool(b)) => w.bit(*b),
        (Kind::Pos, Val::Pos(q)) => match prev {
            Some(Val::Pos(p)) => (0..3).for_each(|i| w.seg(q[i] - p[i], 7)),
            _ => q.iter().for_each(|c| w.seg(*c, 16)),
        },
        (Kind::Angle, Val::Int(n)) => w.bits(*n, ANGLE_BITS),
        (Kind::Dir, Val::Dir([x, y])) => {
            w.bits(u64::from(*x), DIR_BITS);
            w.bits(u64::from(*y), DIR_BITS);
        }
        (Kind::Str, Val::Str(s)) => w.eg(t.index[s], 3),
        (Kind::Id, Val::Id(id)) => match id {
            Id::Datum(d) => {
                w.bits(0, 2);
                w.bits(u64::from(*d), 32);
            }
            Id::Player(p) => {
                w.bits(1, 2);
                w.eg(u64::from(*p), 2);
            }
            Id::Text(s) => {
                w.bits(2, 2);
                w.eg(t.index[s], 3);
            }
        },
        (Kind::Uint(k), Val::Int(n)) => w.eg(*n, k),
        (Kind::Bits(n), Val::Int(v)) => w.bits(*v, n),
        (Kind::Actions, Val::List(l)) => {
            w.eg(l.len() as u64, 0);
            l.iter().for_each(|a| w.bits(u64::from(*a), 3));
        }
        (kind, v) => unreachable!("{kind:?} cannot hold {v:?}"),
    }
}

fn read_value(
    r: &mut BitReader,
    kind: Kind,
    prev: Option<&Val>,
    strings: &[String],
) -> Result<Val> {
    let string = |i: u64| {
        strings
            .get(i as usize)
            .cloned()
            .ok_or(PackError::Bad("string index out of range"))
    };
    Ok(match kind {
        Kind::Bool => Val::Bool(r.bit()?),
        Kind::Pos => match prev {
            Some(Val::Pos(p)) => {
                let mut q = *p;
                for c in &mut q {
                    *c = c
                        .checked_add(r.seg(7)?)
                        .ok_or(PackError::Bad("position out of range"))?;
                }
                Val::Pos(q)
            }
            _ => Val::Pos([r.seg(16)?, r.seg(16)?, r.seg(16)?]),
        },
        Kind::Angle => Val::Int(r.bits(ANGLE_BITS)?),
        Kind::Dir => {
            let (x, y) = (r.bits(DIR_BITS)? as u32, r.bits(DIR_BITS)? as u32);
            if x > DIR_MAX as u32 || y > DIR_MAX as u32 {
                return Err(PackError::Bad("direction out of range"));
            }
            Val::Dir([x, y])
        }
        Kind::Str => Val::Str(string(r.eg(3)?)?),
        Kind::Id => match r.bits(2)? {
            0 => Val::Id(Id::Datum(r.bits(32)? as u32)),
            1 => {
                let p = r.eg(2)?;
                if p >= 1 << 31 {
                    return Err(PackError::Bad("player index out of range"));
                }
                Val::Id(Id::Player(p as u32))
            }
            2 => Val::Id(Id::Text(string(r.eg(3)?)?)),
            _ => return Err(PackError::Bad("unknown id form")),
        },
        Kind::Uint(k) => Val::Int(r.eg(k)?),
        Kind::Bits(n) => {
            let v = r.bits(n)?;
            if n == DEVICE_BITS && v > DEVICE_MAX as u64 {
                return Err(PackError::Bad("device position out of range"));
            }
            Val::Int(v)
        }
        Kind::Actions => {
            let n = r.eg(0)?;
            if n > MAX_FRAME_ACTIONS as u64 {
                return Err(PackError::Bad("too many actions"));
            }
            Val::List(
                (0..n)
                    .map(|_| r.bits(3).map(|a| a as u8))
                    .collect::<Result<Vec<u8>>>()?,
            )
        }
    })
}

fn write_field(w: &mut BitWriter, field: Field, v: &Val, prev: Option<&Val>, t: &Table) {
    if field.opt {
        w.bit(!matches!(v, Val::None));
    }
    if !matches!(v, Val::None) {
        write_value(w, field.kind, v, prev, t);
    }
}

fn read_field(
    r: &mut BitReader,
    field: Field,
    prev: Option<&Val>,
    strings: &[String],
) -> Result<Val> {
    if field.opt && !r.bit()? {
        return Ok(Val::None);
    }
    read_value(r, field.kind, prev, strings)
}

fn write_entity(w: &mut BitWriter, schema: &[Field], e: &Entity, t: &Table) {
    for (field, v) in schema.iter().zip(e) {
        write_field(w, *field, v, None, t);
    }
}

fn read_entity(r: &mut BitReader, schema: &[Field], strings: &[String]) -> Result<Entity> {
    schema
        .iter()
        .map(|field| read_field(r, *field, None, strings))
        .collect()
}

fn read_count(r: &mut BitReader, max: usize) -> Result<usize> {
    let n = r.eg(2)?;
    if n > max as u64 {
        return Err(PackError::Bad("list too long"));
    }
    Ok(n as usize)
}

fn write_full(w: &mut BitWriter, schema: &[Field], list: &[Entity], t: &Table) {
    w.eg(list.len() as u64, 2);
    for e in list {
        write_entity(w, schema, e, t);
    }
}

fn read_full(
    r: &mut BitReader,
    schema: &[Field],
    max: usize,
    strings: &[String],
) -> Result<Vec<Entity>> {
    let n = read_count(r, max)?;
    (0..n).map(|_| read_entity(r, schema, strings)).collect()
}

/// An entity's key: its key fields (at most two).
fn key_of<'a>(schema: &[Field], e: &'a Entity) -> [Option<&'a Val>; 2] {
    let mut key = [None; 2];
    for (slot, (_, v)) in key
        .iter_mut()
        .zip(schema.iter().zip(e).filter(|(f, _)| f.key))
    {
        *slot = Some(v);
    }
    key
}

/// A list against the previous frame's: the count, then until it is
/// reached either `0` and a run of unchanged entities taken in order from
/// the previous list, or `1` and one entity: `1`, the previous entity it
/// is (relative to the one after the last taken), a changed-field mask and
/// the changed fields; or `0` and the whole entity.
fn write_delta(w: &mut BitWriter, schema: &[Field], list: &[Entity], prev: &[Entity], t: &Table) {
    // Most frames list the same entities in the same order: match by
    // position, and only key a map when that fails.
    let same_order = list.len() == prev.len()
        && list
            .iter()
            .zip(prev)
            .all(|(e, p)| key_of(schema, e) == key_of(schema, p));
    let matches: Vec<Option<usize>> = if same_order {
        (0..list.len()).map(Some).collect()
    } else {
        let mut keys: HashMap<[Option<&Val>; 2], usize> = HashMap::with_capacity(prev.len());
        for (j, e) in prev.iter().enumerate() {
            keys.entry(key_of(schema, e)).or_insert(j);
        }
        let mut used = vec![false; prev.len()];
        list.iter()
            .map(|e| {
                let j = *keys.get(&key_of(schema, e))?;
                (!std::mem::replace(&mut used[j], true)).then_some(j)
            })
            .collect()
    };
    w.eg(list.len() as u64, 2);
    let mut cursor = 0usize;
    let mut i = 0;
    while i < list.len() {
        let mut run = 0;
        while i + run < list.len()
            && matches[i + run] == Some(cursor + run)
            && list[i + run] == prev[cursor + run]
        {
            run += 1;
        }
        if run > 0 {
            w.bit(false);
            w.eg(run as u64 - 1, 2);
            i += run;
            cursor += run;
            continue;
        }
        w.bit(true);
        match matches[i] {
            Some(j) => {
                w.bit(true);
                w.seg(j as i64 - cursor as i64, 0);
                let before = &prev[j];
                for ((field, v), p) in schema.iter().zip(&list[i]).zip(before) {
                    if !field.key {
                        w.bit(v != p);
                    }
                }
                for ((field, v), p) in schema.iter().zip(&list[i]).zip(before) {
                    if !field.key && v != p {
                        write_field(w, *field, v, Some(p), t);
                    }
                }
                cursor = j + 1;
            }
            None => {
                w.bit(false);
                write_entity(w, schema, &list[i], t);
            }
        }
        i += 1;
    }
}

/// Reads a delta list against `prev`, the frame before's, which it takes
/// apart: an unchanged entity (a run) is moved, not copied. A changed one
/// is copied from where its original is now (it may be referred to again).
fn read_delta(
    r: &mut BitReader,
    schema: &[Field],
    max: usize,
    prev: &mut [Entity],
    strings: &[String],
) -> Result<Vec<Entity>> {
    const HERE: usize = usize::MAX;
    let n = read_count(r, max)?;
    let mut out: Vec<Entity> = Vec::with_capacity(n);
    // Where each of `prev` was moved to in `out` (HERE: not moved).
    let mut moved: Vec<usize> = Vec::new();
    let mut cursor = 0usize;
    while out.len() < n {
        if !r.bit()? {
            let run = r.eg(2)? as usize + 1;
            if out.len() + run > n || cursor + run > prev.len() {
                return Err(PackError::Bad("run past the list"));
            }
            if moved.is_empty() {
                moved.resize(prev.len(), HERE);
            }
            for (k, e) in prev[cursor..cursor + run].iter_mut().enumerate() {
                moved[cursor + k] = out.len();
                out.push(std::mem::take(e));
            }
            cursor += run;
            continue;
        }
        if r.bit()? {
            let j = cursor as i64 + r.seg(0)?;
            let j = usize::try_from(j)
                .ok()
                .filter(|j| *j < prev.len())
                .ok_or(PackError::Bad("entity refers past the list"))?;
            let before = match moved.get(j) {
                Some(&at) if at != HERE => &out[at],
                _ => &prev[j],
            };
            let mut mask = 0u32;
            for (idx, f) in schema.iter().enumerate() {
                if !f.key && r.bit()? {
                    mask |= 1 << idx;
                }
            }
            let mut e = before.clone();
            for (idx, field) in schema.iter().enumerate() {
                if mask & (1 << idx) != 0 {
                    e[idx] = read_field(r, *field, Some(&before[idx]), strings)?;
                }
            }
            out.push(e);
            cursor = j + 1;
        } else {
            out.push(read_entity(r, schema, strings)?);
        }
    }
    Ok(out)
}

/// What the encoder and a decoder of one stream share after each frame.
#[derive(Clone, Default)]
struct StreamState {
    seq: u64,
    /// The format version its frames were in.
    version: u64,
    game_id: String,
    tick: u64,
    bsp: Option<u64>,
    strings: Vec<String>,
    lists: [Vec<Entity>; 5],
}

/// Encodes one stream of frames: a keyframe, then deltas against the frame
/// before, with a keyframe at least every `keyframe_every` frames, on a new
/// game and whenever a delta cannot be sent.
pub struct Encoder {
    sid: u64,
    keyframe_every: u64,
    since_key: u64,
    next_seq: u64,
    state: Option<(StreamState, Table)>,
}

impl Encoder {
    /// `sid` names the stream for decoders that follow several at once.
    pub fn new(sid: u64, keyframe_every: u64) -> Self {
        Encoder {
            sid,
            keyframe_every: keyframe_every.max(1),
            since_key: 0,
            next_seq: 0,
            state: None,
        }
    }

    pub fn sid(&self) -> u64 {
        self.sid
    }

    /// The sequence number the next frame gets.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The next frame is a keyframe.
    pub fn force_keyframe(&mut self) {
        self.state = None;
    }

    /// One packed frame; `Ok((bytes, keyframe))`. A frame that cannot be
    /// packed leaves the stream as it was.
    pub fn encode(&mut self, frame: &Frame) -> Result<(Vec<u8>, bool)> {
        let lists = Lists::of(frame)?;
        if frame.game_id.len() > MAX_STRING {
            return Err(PackError::Value("game id too long"));
        }
        let mut new_strings: Vec<&str> = Vec::new();
        let key = match &self.state {
            None => true,
            Some((s, t)) => {
                let mut fresh = std::collections::HashSet::new();
                for st in lists.strings() {
                    if !t.index.contains_key(st) && fresh.insert(st) {
                        new_strings.push(st);
                    }
                }
                self.since_key >= self.keyframe_every
                    || s.game_id != frame.game_id
                    || frame.tick < s.tick
                    || s.strings.len() + new_strings.len() > MAX_STRINGS
                    || new_strings.len() as u64 > MAX_NEW_STRINGS
            }
        };
        let (mut state, mut table) = if key {
            new_strings.clear();
            let mut fresh = std::collections::HashSet::new();
            for st in lists.strings() {
                if fresh.insert(st) {
                    new_strings.push(st);
                }
            }
            if new_strings.len() as u64 > MAX_NEW_STRINGS {
                return Err(PackError::Value("too many strings"));
            }
            (StreamState::default(), Table::default())
        } else {
            // Nothing below fails: the stream moves on with this frame.
            self.state.take().unwrap()
        };
        let seq = self.next_seq;
        let mut w = BitWriter::new();
        w.bits(VERSION, 4);
        w.bit(key);
        w.eg(self.sid, 0);
        w.eg(seq, 8);
        let bsp = frame.bsp.map(u64::from);
        if key {
            write_str_literal(&mut w, &frame.game_id);
            w.eg(frame.tick, 20);
            w.bit(bsp.is_some());
            if let Some(b) = bsp {
                w.eg(b, 1);
            }
        } else {
            w.eg(frame.tick - state.tick, 2);
            w.bit(bsp != state.bsp);
            if bsp != state.bsp {
                w.bit(bsp.is_some());
                if let Some(b) = bsp {
                    w.eg(b, 1);
                }
            }
        }
        w.eg(new_strings.len() as u64, 1);
        for s in &new_strings {
            write_str_literal(&mut w, s);
            table
                .index
                .insert((*s).to_owned(), state.strings.len() as u64);
            state.strings.push((*s).to_owned());
        }
        for (n, (schema, _)) in LISTS.iter().enumerate() {
            if n == 3 {
                write_full(&mut w, HIT, &lists.hits, &table);
            }
            if key {
                write_full(&mut w, schema, &lists.lists[n], &table);
            } else {
                write_delta(&mut w, schema, &lists.lists[n], &state.lists[n], &table);
            }
        }
        state.seq = seq;
        state.version = VERSION;
        state.game_id = frame.game_id.clone();
        state.tick = frame.tick;
        state.bsp = bsp;
        state.lists = lists.lists;
        self.state = Some((state, table));
        self.next_seq += 1;
        self.since_key = if key { 1 } else { self.since_key + 1 };
        Ok((w.finish(), key))
    }
}

/// A decoded frame and where it sat in its stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub sid: u64,
    pub seq: u64,
    pub keyframe: bool,
    pub frame: Frame,
}

/// Decodes packed frames of any number of streams (keyed by `sid`).
#[derive(Default)]
pub struct Decoder {
    streams: HashMap<u64, StreamState>,
}

/// Most streams one decoder follows.
const MAX_STREAMS: usize = 256;

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets a stream (its game ended).
    pub fn forget(&mut self, sid: u64) {
        self.streams.remove(&sid);
    }

    pub fn decode(&mut self, bytes: &[u8]) -> Result<Decoded> {
        let mut r = BitReader::new(bytes);
        let version = r.bits(4)?;
        let lists = lists_of(version).ok_or(PackError::Version(version))?;
        let key = r.bit()?;
        let sid = r.eg(0)?;
        let seq = r.eg(8)?;
        // A delta takes the stream's state apart (it is replaced either
        // way; after a frame that does not decode, the next delta is a gap).
        let mut prev = if key {
            StreamState::default()
        } else {
            match self.streams.get(&sid) {
                // A delta is against a frame of its own version.
                Some(s) if s.seq.checked_add(1) == Some(seq) && s.version == version => {
                    self.streams.remove(&sid).expect("there")
                }
                _ => return Err(PackError::Gap { sid, seq }),
            }
        };
        let mut state = StreamState {
            seq,
            version,
            strings: std::mem::take(&mut prev.strings),
            ..StreamState::default()
        };
        if key {
            state.game_id = read_str_literal(&mut r)?;
            state.tick = r.eg(20)?;
            state.bsp = if r.bit()? { Some(r.eg(1)?) } else { None };
        } else {
            state.game_id = std::mem::take(&mut prev.game_id);
            state.tick = prev
                .tick
                .checked_add(r.eg(2)?)
                .ok_or(PackError::Bad("tick out of range"))?;
            state.bsp = prev.bsp;
            if r.bit()? {
                state.bsp = if r.bit()? { Some(r.eg(1)?) } else { None };
            }
        }
        let fresh = r.eg(1)?;
        if fresh > MAX_NEW_STRINGS || state.strings.len() as u64 + fresh > MAX_STRINGS as u64 {
            return Err(PackError::Bad("too many strings"));
        }
        for _ in 0..fresh {
            state.strings.push(read_str_literal(&mut r)?);
        }
        let mut hits = Vec::new();
        for (n, (schema, max)) in lists.iter().enumerate() {
            if n == 3 {
                hits = read_full(&mut r, HIT, MAX_FRAME_HITS, &state.strings)?;
            }
            state.lists[n] = if key {
                read_full(&mut r, schema, *max, &state.strings)?
            } else {
                read_delta(&mut r, schema, *max, &mut prev.lists[n], &state.strings)?
            };
        }
        let frame = frame_of(&state.game_id, state.tick, state.bsp, &state.lists, &hits)?;
        if !self.streams.contains_key(&sid) && self.streams.len() >= MAX_STREAMS {
            return Err(PackError::Bad("too many streams"));
        }
        self.streams.insert(sid, state);
        Ok(Decoded {
            sid,
            seq,
            keyframe: key,
            frame,
        })
    }
}

/// A frame packed on its own (a keyframe of stream 0): a recorded frame.
pub fn encode_keyframe(frame: &Frame) -> Result<Vec<u8>> {
    Encoder::new(0, 1).encode(frame).map(|(bytes, _)| bytes)
}

/// Decodes a frame packed on its own.
pub fn decode_keyframe(bytes: &[u8]) -> Result<Frame> {
    let d = Decoder::new().decode(bytes)?;
    if !d.keyframe {
        return Err(PackError::Bad("not a keyframe"));
    }
    Ok(d.frame)
}

/// Bytes as base64url without padding (a recorded frame's `packed`).
pub fn to_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn from_base64(text: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| PackError::Bad("packed frame is not base64url"))
}

// ---------------------------------------------------------- containers

/// Appends one record to a container: its length (LEB128), then its
/// bytes. A binary WebSocket message or a packed HTTP body is a container.
pub fn push_record(out: &mut Vec<u8>, record: &[u8]) {
    let mut n = record.len() as u64;
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(record);
}

/// The records of a container.
pub fn records(mut bytes: &[u8]) -> impl Iterator<Item = Result<&[u8]>> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let mut n = 0u64;
        let mut shift = 0;
        loop {
            let Some((&b, rest)) = bytes.split_first() else {
                bytes = &[];
                return Some(Err(PackError::Short));
            };
            bytes = rest;
            n |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                bytes = &[];
                return Some(Err(PackError::Bad("record length too long")));
            }
        }
        if n as usize > bytes.len() {
            bytes = &[];
            return Some(Err(PackError::Short));
        }
        let (record, rest) = bytes.split_at(n as usize);
        bytes = rest;
        Some(Ok(record))
    })
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
