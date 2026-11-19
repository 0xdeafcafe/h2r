//! Packed telemetry frames (docs/live/live-telemetry.md, "Packed frames"): a
//! versioned bit-packed form of [`Frame`] for the ingest socket, the live
//! stream to the web and recorded frames at rest. JSON stays for the public
//! read API and the low-rate events.
//!
//! A frame is quantised (positions to 1/1024 world unit, yaw to 14 bits,
//! directions octahedral 2 x 14 bits, ...) and written least significant
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

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{PI, TAU};

use crate::game::{
    ACTIONS, Frame, FrameActor, FrameAnim, FrameHit, FrameItem, FrameMachine, FrameObject,
    FramePlayer, FrameProjectile, FrameVehicle, GRENADES, MAX_ANIM_FRAME, MAX_FRAME_ACTIONS,
    MAX_FRAME_ACTORS, MAX_FRAME_HITS, MAX_FRAME_ITEMS, MAX_FRAME_MACHINES, MAX_FRAME_OBJECTS,
    MAX_FRAME_PROJECTILES, MAX_FRAME_VEHICLES, MAX_MACHINE_POSITIONS, MAX_PLAYERS, MachinePosition,
    ObjectKind, STANCES,
};

/// Latest encoder version. V7 adds finer grids, sparse vector residuals,
/// repeated edit masks and change-only positions relative to dead reckoning.
/// V6 introduced vehicle velocity, projectiles and loose items.
pub const VERSION: u64 = V7;
/// Highest negotiated live format. Standalone records and default writers use v7.
pub const LIVE_VERSION: u8 = 8;
mod v8;
// Keep historical decoding tied to the wire version when the writer advances.
const V7: u64 = 7;
/// V4 was the last ordinary frame format without a cinematic bit.
pub const V4: u64 = 4;
/// V5 was used for cinematic frames from v4 senders.
pub const CINEMATIC_VERSION: u64 = 5;
/// Positions in 1/1024 world unit (about 3 mm).
pub const POS_SCALE: f64 = 1024.0;
/// Yaw: a full turn in 16384 steps.
pub const ANGLE_BITS: u32 = 14;
/// Directions: two octahedral components of 14 bits.
pub const DIR_BITS: u32 = 14;
/// Speeds in 1/16 unit a second.
pub const SPEED_SCALE: f64 = 16.0;
/// Hit amounts in 1/256 of a vitality.
pub const AMOUNT_SCALE: f64 = 256.0;
/// Velocities in 1/128 unit a second.
pub const VEL_SCALE: f64 = 128.0;
/// Body vitality (0..1) in 6 bits.
pub const BODY_BITS: u32 = 6;
/// Shield vitality in 1/32 of a shield.
pub const SHIELD_SCALE: f64 = 32.0;
/// Animation frames in 1/16 of a frame.
pub const ANIM_SCALE: f64 = 16.0;
/// Legacy machine device positions (0..1) in 10 bits.
pub const DEVICE_BITS: u32 = 10;
/// Version 6: the Exp-Golomb order of a predicted position's error.
pub const PREDICT_K: u32 = 3;
/// Version 6: a prediction spans at most this many ticks; a longer gap
/// codes the plain difference.
pub const PREDICT_TICKS: u64 = 30;
/// Version 6: a velocity component (in 1/64 unit a second) past this is
/// not predicted from.
const PREDICT_VEL: i64 = 1 << 23;
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
    word: u64,
    used: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The low `n` bits of `v` (n <= 64).
    #[inline]
    pub fn bits(&mut self, v: u64, n: u32) {
        debug_assert!(n == 64 || v >> n == 0, "{v} does not fit {n} bits");
        // Most tokens fit in the pending word. A token crossing the end
        // fills it and leaves at most 63 bits for the next one.
        let room = 64 - self.used;
        self.word |= v << self.used;
        if n < room {
            self.used += n;
        } else {
            self.bytes.extend_from_slice(&self.word.to_le_bytes());
            self.word = v.checked_shr(room).unwrap_or(0);
            self.used = n - room;
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
        let suffix = ((x & ((1 << (n - 1)) - 1)) << 1) | 1;
        if z + n <= 64 {
            self.bits(suffix << z, z + n);
        } else {
            self.bits(0, z);
            self.bits(suffix, n);
        }
    }

    /// Signed Exp-Golomb: zig-zag (0, -1, 1, -2, ...) then [`Self::eg`].
    pub fn seg(&mut self, v: i64, k: u32) {
        self.eg(zigzag(v), k);
    }

    pub fn bit_len(&self) -> usize {
        self.bytes.len() * 8 + self.used as usize
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.bytes
            .extend_from_slice(&self.word.to_le_bytes()[..self.used.div_ceil(8) as usize]);
        self.bytes
    }
}

/// Reads what [`BitWriter`] wrote.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    version: u64,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        BitReader {
            bytes,
            pos: 0,
            version: VERSION,
        }
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

    fn remaining(&self) -> usize {
        self.bytes.len() * 8 - self.pos
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
    oct_grid(v, DIR_MAX)
}

fn oct_grid(v: [f64; 3], max: f64) -> Result<[u32; 2]> {
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
    let q = |c: f64| ((c.clamp(-1.0, 1.0) + 1.0) * 0.5 * max).round() as u32;
    Ok([q(x), q(y)])
}

/// The unit vector [`oct`] packed.
pub fn unoct(q: [u32; 2]) -> [f64; 3] {
    unoct_grid(q, DIR_MAX)
}

fn unoct_grid(q: [u32; 2], max: f64) -> [f64; 3] {
    let mut x = q[0] as f64 / max * 2.0 - 1.0;
    let mut y = q[1] as f64 / max * 2.0 - 1.0;
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        let (ax, ay) = (x.abs(), y.abs());
        x = (1.0 - ay) * sign(x);
        y = (1.0 - ax) * sign(y);
    }
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

#[inline]
fn position_scale() -> f64 {
    #[cfg(test)]
    if floor::COARSE_POSITION.get() {
        return 256.0;
    }
    POS_SCALE
}

fn qpos(v: [f64; 3]) -> Result<[i64; 3]> {
    if !v.iter().all(|c| c.is_finite() && c.abs() < MAX_COORD) {
        return Err(PackError::Value("position must be finite"));
    }
    Ok(v.map(|c| (c * position_scale()).round() as i64))
}

fn dpos(q: [i64; 3]) -> [f64; 3] {
    q.map(|c| c as f64 / position_scale())
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
const BODY_MAX: f64 = ((1u32 << BODY_BITS) - 1) as f64;

fn qvel(v: [f64; 3]) -> Result<[i64; 3]> {
    if !v.iter().all(|c| c.is_finite() && c.abs() <= MAX_COORD) {
        return Err(PackError::Value("velocity out of range"));
    }
    Ok(v.map(|c| (c * VEL_SCALE).round() as i64))
}

fn dvel(q: [i64; 3]) -> [f64; 3] {
    q.map(|c| c as f64 / VEL_SCALE)
}

// ------------------------------------------------------------- values

/// An object id: a datum (8 hex digits), a player (`p<index>`) or text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    any(test, feature = "live-floor-bench"),
    derive(serde::Serialize, serde::Deserialize)
)]
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
#[cfg_attr(
    any(test, feature = "live-floor-bench"),
    derive(serde::Serialize, serde::Deserialize)
)]
enum Val {
    None,
    Bool(bool),
    Int(u64),
    Pos([i64; 3]),
    Dir([u32; 2]),
    LegacyDir([u32; 2]), // v1–6 octahedral grid, max 4094
    Str(String),
    Id(Id),
    List(Vec<u8>),
    Vel([i64; 3]),
    Ints(Vec<u64>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Bool,
    /// Three position axes. V7 edits carry sparse residuals against the motion base.
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
    /// Animation phase in sixteenths. A matched v7 edit can advance by its tick span.
    AnimPhase,
    /// A fixed number of bits.
    Bits(u32),
    /// A count (Exp-Golomb k 0), then 3 bits each.
    Actions,
    /// Three velocity axes. V7 edits carry sparse residuals against the last velocity.
    Vel,
    /// A count (Exp-Golomb k 0, at most [`MAX_FRAME_ACTIONS`]), then
    /// Exp-Golomb k 1 each: ticks back.
    Agos,
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
    o(Kind::Bits(1)),         // grenade
    o(Kind::Vel),             // vel (version 3)
    o(Kind::Uint(1)),         // fire
    o(Kind::Bits(BODY_BITS)), // body
    o(Kind::Uint(3)),         // shield
    o(Kind::Uint(0)),         // seat
    o(Kind::Agos),            // actions_ago
    o(Kind::Str),             // anim name (version 4)
    o(Kind::AnimPhase),       // anim frame
];
/// Fields a version 2 or 3 player or actor has: the later ones are last.
const V2_UNIT_FIELDS: usize = 13;
const V3_UNIT_FIELDS: usize = 19;
const PLAYER_V2: &[Field] = PLAYER.split_at(V2_UNIT_FIELDS).0;
const PLAYER_V3: &[Field] = PLAYER.split_at(V3_UNIT_FIELDS).0;
const VEHICLE: &[Field] = &[
    k(Kind::Id),
    f(Kind::Str), // kind
    f(Kind::Pos),
    f(Kind::Dir),  // forward
    f(Kind::Dir),  // up
    f(Kind::Bool), // occupied
    o(Kind::Id),   // parent (version 2)
    o(Kind::Dir),  // aim (version 2)
    o(Kind::Vel),  // vel (version 6)
];
/// Versions 2 to 5's vehicle: [`VEHICLE`] without its velocity.
const VEHICLE_V2: &[Field] = VEHICLE.split_at(8).0;
/// Version 1's vehicle: without its parent and aim too.
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
    o(Kind::Bits(1)),         // grenade
    o(Kind::Vel),             // vel (version 3)
    o(Kind::Uint(1)),         // fire
    o(Kind::Bits(BODY_BITS)), // body
    o(Kind::Uint(3)),         // shield
    o(Kind::Uint(0)),         // seat
    o(Kind::Agos),            // actions_ago
    o(Kind::Str),             // anim name (version 4)
    o(Kind::AnimPhase),       // anim frame
];
const ACTOR_V2: &[Field] = ACTOR.split_at(V2_UNIT_FIELDS).0;
const ACTOR_V3: &[Field] = ACTOR.split_at(V3_UNIT_FIELDS).0;
const HIT: &[Field] = &[
    o(Kind::Id),  // from
    o(Kind::Id),  // to
    o(Kind::Str), // weapon
    f(Kind::Pos),
    o(Kind::Uint(6)), // amount
    o(Kind::Str),     // kind
    o(Kind::Uint(1)), // ago (version 3)
];
const HIT_V2: &[Field] = HIT.split_at(6).0;
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
/// Version 6.
const PROJECTILE: &[Field] = &[
    k(Kind::Id),
    f(Kind::Str), // kind
    f(Kind::Pos),
    o(Kind::Vel),
    o(Kind::Id), // parent
];
/// Version 6.
const ITEM: &[Field] = &[
    k(Kind::Id),
    f(Kind::Str), // kind
    f(Kind::Pos),
    f(Kind::Dir),  // forward
    f(Kind::Dir),  // up
    f(Kind::Bool), // equipment
];

/// How many delta-coded lists a frame has (version 6; older versions the
/// first five).
const LIST_COUNT: usize = 7;

/// The delta-coded lists, in wire order (hits are always sent whole, after
/// the actors).
const LISTS: [(&[Field], usize); LIST_COUNT] = [
    (PLAYER, MAX_PLAYERS),
    (VEHICLE, MAX_FRAME_VEHICLES),
    (ACTOR, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
    (PROJECTILE, MAX_FRAME_PROJECTILES),
    (ITEM, MAX_FRAME_ITEMS),
];
/// Versions 4 and 5's lists.
const LISTS_V4: [(&[Field], usize); 5] = [
    (PLAYER, MAX_PLAYERS),
    (VEHICLE_V2, MAX_FRAME_VEHICLES),
    (ACTOR, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
];
/// Version 3's lists.
const LISTS_V3: [(&[Field], usize); 5] = [
    (PLAYER_V3, MAX_PLAYERS),
    (VEHICLE_V2, MAX_FRAME_VEHICLES),
    (ACTOR_V3, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
];
/// Version 2's lists.
const LISTS_V2: [(&[Field], usize); 5] = [
    (PLAYER_V2, MAX_PLAYERS),
    (VEHICLE_V2, MAX_FRAME_VEHICLES),
    (ACTOR_V2, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
];
/// Version 1's lists.
const LISTS_V1: [(&[Field], usize); 5] = [
    (PLAYER_V2, MAX_PLAYERS),
    (VEHICLE_V1, MAX_FRAME_VEHICLES),
    (ACTOR_V2, MAX_FRAME_ACTORS),
    (MACHINE, MAX_FRAME_MACHINES),
    (OBJECT, MAX_FRAME_OBJECTS),
];

/// What a version this decoder reads has: its lists and its hits.
struct Schema {
    lists: &'static [(&'static [Field], usize)],
    hit: &'static [Field],
    /// The header carries a cut (version 3).
    cut: bool,
    /// The header carries the cinematic bit (versions 5 and 6).
    cinematic: bool,
}

fn schema_of(version: u64) -> Option<Schema> {
    fn schema(
        lists: &'static [(&'static [Field], usize)],
        hit: &'static [Field],
        cut: bool,
        cinematic: bool,
    ) -> Schema {
        Schema {
            lists,
            hit,
            cut,
            cinematic,
        }
    }
    Some(match version {
        1 => schema(&LISTS_V1, HIT_V2, false, false),
        2 => schema(&LISTS_V2, HIT_V2, false, false),
        3 => schema(&LISTS_V3, HIT, true, false),
        V4 => schema(&LISTS_V4, HIT, true, false),
        CINEMATIC_VERSION => schema(&LISTS_V4, HIT, true, true),
        6 => schema(&LISTS, HIT, true, true),
        V7 => schema(&LISTS, HIT, true, true),
        _ => return None,
    })
}

/// Where an entity's position and velocity sit in its fields, when it has
/// both: what a version 6 delta predicts its position from.
fn motion_fields(schema: &[Field]) -> Option<(usize, usize)> {
    let pos = schema.iter().position(|f| f.kind == Kind::Pos && !f.opt)?;
    let vel = schema.iter().position(|f| f.kind == Kind::Vel)?;
    Some((pos, vel))
}

/// The previous velocity a version 6 delta predicts from, `dt` ticks on:
/// only when there is one, in range, and the gap is at most
/// [`PREDICT_TICKS`]. Both sides know it before the position's bits.
fn predict_base(prev_vel: &Val, dt: u64) -> Option<[i64; 3]> {
    match prev_vel {
        Val::Vel(a)
            if dt <= PREDICT_TICKS
                && a.iter()
                    .all(|c| c.unsigned_abs() <= (2 * PREDICT_VEL) as u64) =>
        {
            Some(*a)
        }
        _ => None,
    }
}

/// Version 6's predicted position: the previous one moved by the mean of
/// the previous and the new velocity (the previous alone when there is no
/// new one in range) over `dt` ticks, in position quanta, rounded half up:
/// `p + floor((4 (a + b) dt + 15) / 30)` (1/64 unit a second over 1/30 s
/// in 1/512 unit is a factor of 4/30 for the sum of two). Integers, so
/// every decoder predicts the same.
fn predict(prev_pos: [i64; 3], a: [i64; 3], new_vel: &Val, dt: u64) -> [i64; 3] {
    let b = match new_vel {
        Val::Vel(b)
            if b.iter()
                .all(|c| c.unsigned_abs() <= (2 * PREDICT_VEL) as u64) =>
        {
            *b
        }
        _ => a,
    };
    let dt = dt as i64;
    std::array::from_fn(|i| {
        // Legacy coordinates and velocities are stored internally in the
        // finer v7 grid. Round in the original grid before lifting back.
        prev_pos[i].saturating_add(2 * (2 * (a[i] + b[i]) * dt + 15).div_euclid(30))
    })
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

fn anim_frame(frame: f64) -> Result<Val> {
    if !(0.0..=MAX_ANIM_FRAME).contains(&frame) {
        return Err(PackError::Value("anim frame out of range"));
    }
    qfixed(frame, ANIM_SCALE, "anim frame").map(Val::Int)
}

/// A unit's `anim` from its name and frame fields (versions 1 to 3 have
/// neither).
fn as_anim(e: &Entity, at: usize) -> Result<Option<FrameAnim>> {
    let name = e.get(at).map(as_opt_str).transpose()?.flatten();
    let frame = e.get(at + 1).map(as_opt_int).transpose()?.flatten();
    Ok(match (name, frame) {
        (Some(name), Some(q)) => Some(FrameAnim {
            name,
            frame: q as f64 / ANIM_SCALE,
        }),
        (None, None) => None,
        _ => return Err(PackError::Bad("anim needs its name and frame")),
    })
}

/// A unit's `actions_ago`: only beside actions, one each.
fn agos_val(actions: &Option<Vec<String>>, ago: &Option<Vec<u32>>) -> Result<Val> {
    let n = actions.as_ref().map_or(0, Vec::len);
    match ago.as_ref().filter(|g| !g.is_empty()) {
        None => Ok(Val::None),
        Some(g) if g.len() == n => Ok(Val::Ints(g.iter().map(|a| u64::from(*a)).collect())),
        Some(_) => Err(PackError::Value("actions_ago must match actions")),
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

#[inline]
fn unit_aim_max() -> i32 {
    #[cfg(test)]
    if floor::COARSE_AIM.get() {
        return 4094;
    }
    DIR_MAX as i32
}

fn unit_aim(v: [f64; 3]) -> Result<Val> {
    #[cfg(test)]
    if floor::COARSE_AIM.get() {
        return oct_grid(v, 4094.0).map(Val::Dir);
    }
    oct(v).map(Val::Dir)
}

fn player_vals(p: &FramePlayer) -> Result<Entity> {
    Ok(vec![
        Val::Int(u64::from(p.index)),
        Val::Bool(p.alive),
        Val::Pos(qpos(p.pos)?),
        Val::Int(qyaw(p.yaw)?),
        opt(p.vehicle.as_deref(), id)?,
        opt(p.species.as_deref(), string)?,
        opt(p.aim, unit_aim)?,
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
        opt(p.vel, |v| qvel(v).map(Val::Vel))?,
        opt(p.fire.filter(|m| *m != 0), |m| Ok(Val::Int(u64::from(m))))?,
        opt(p.body, |b| {
            if !(0.0..=1.0).contains(&b) {
                return Err(PackError::Value("body out of range"));
            }
            Ok(Val::Int((b * BODY_MAX).round() as u64))
        })?,
        opt(p.shield, |v| {
            qfixed(v, SHIELD_SCALE, "shield").map(Val::Int)
        })?,
        opt(p.seat, |n| Ok(Val::Int(u64::from(n))))?,
        agos_val(&p.actions, &p.actions_ago)?,
        opt(p.anim.as_ref(), |a| string(&a.name))?,
        opt(p.anim.as_ref(), |a| anim_frame(a.frame))?,
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
        opt(v.vel, |v| qvel(v).map(Val::Vel))?,
    ])
}

fn projectile_vals(p: &FrameProjectile) -> Result<Entity> {
    Ok(vec![
        id(&p.id)?,
        string(&p.kind)?,
        Val::Pos(qpos(p.pos)?),
        opt(p.vel, |v| qvel(v).map(Val::Vel))?,
        opt(p.parent.as_deref(), id)?,
    ])
}

fn item_vals(x: &FrameItem) -> Result<Entity> {
    Ok(vec![
        id(&x.id)?,
        string(&x.kind)?,
        Val::Pos(qpos(x.pos)?),
        Val::Dir(oct(x.forward)?),
        Val::Dir(oct(x.up)?),
        Val::Bool(x.equipment),
    ])
}

fn actor_vals(a: &FrameActor) -> Result<Entity> {
    Ok(vec![
        id(&a.id)?,
        string(&a.species)?,
        Val::Pos(qpos(a.pos)?),
        Val::Int(qyaw(a.yaw)?),
        opt(a.aim, unit_aim)?,
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
        opt(a.vel, |v| qvel(v).map(Val::Vel))?,
        opt(a.fire.filter(|m| *m != 0), |m| Ok(Val::Int(u64::from(m))))?,
        opt(a.body, |b| {
            if !(0.0..=1.0).contains(&b) {
                return Err(PackError::Value("body out of range"));
            }
            Ok(Val::Int((b * BODY_MAX).round() as u64))
        })?,
        opt(a.shield, |v| {
            qfixed(v, SHIELD_SCALE, "shield").map(Val::Int)
        })?,
        opt(a.seat, |n| Ok(Val::Int(u64::from(n))))?,
        agos_val(&a.actions, &a.actions_ago)?,
        opt(a.anim.as_ref(), |a| string(&a.name))?,
        opt(a.anim.as_ref(), |a| anim_frame(a.frame))?,
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
        opt(h.ago, |n| Ok(Val::Int(u64::from(n))))?,
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
        Val::LegacyDir(q) => Ok(unoct_grid(*q, 4094.0)),
        _ => Err(PackError::Bad("expected a direction")),
    }
}
fn as_unit_aim(v: &Val) -> Result<Option<[f64; 3]>> {
    #[cfg(test)]
    if floor::COARSE_AIM.get()
        && let Val::Dir(q) = v
    {
        return Ok(Some(unoct_grid(*q, 4094.0)));
    }
    as_opt_dir(v)
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
fn as_opt_vel(v: &Val) -> Result<Option<[f64; 3]>> {
    match v {
        Val::None => Ok(None),
        Val::Vel(q) => Ok(Some(dvel(*q))),
        _ => Err(PackError::Bad("expected a velocity")),
    }
}

fn as_agos(v: &Val) -> Result<Option<Vec<u32>>> {
    match v {
        Val::None => Ok(None),
        Val::Ints(l) => l
            .iter()
            .map(|n| u32::try_from(*n).map_err(|_| PackError::Bad("ago out of range")))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        _ => Err(PackError::Bad("expected ticks")),
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
        aim: as_unit_aim(&e[6])?,
        weapon: as_opt_str(&e[7])?,
        shots: as_opt_int(&e[8])?.map(|n| n as u32),
        stance: as_enum(&e[9], &STANCES)?,
        speed: as_opt_int(&e[10])?.map(|q| q as f64 / SPEED_SCALE),
        actions: as_actions(&e[11])?,
        grenade: as_enum(&e[12], &GRENADES)?,
        // Versions 1 and 2 have none of these.
        vel: e.get(13).map(as_opt_vel).transpose()?.flatten(),
        fire: e
            .get(14)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|m| m as u32),
        body: e
            .get(15)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|q| q as f64 / BODY_MAX),
        shield: e
            .get(16)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|q| q as f64 / SHIELD_SCALE),
        seat: e
            .get(17)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|n| n as u32),
        actions_ago: e.get(18).map(as_agos).transpose()?.flatten(),
        anim: as_anim(e, 19)?,
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
        // Version 6.
        vel: e.get(8).map(as_opt_vel).transpose()?.flatten(),
    })
}

fn projectile_of(e: &Entity) -> Result<FrameProjectile> {
    Ok(FrameProjectile {
        id: as_id(&e[0])?,
        kind: as_str(&e[1])?,
        pos: as_pos(&e[2])?,
        vel: as_opt_vel(&e[3])?,
        parent: as_opt_id(&e[4])?,
    })
}

fn item_of(e: &Entity) -> Result<FrameItem> {
    Ok(FrameItem {
        id: as_id(&e[0])?,
        kind: as_str(&e[1])?,
        pos: as_pos(&e[2])?,
        forward: as_dir(&e[3])?,
        up: as_dir(&e[4])?,
        equipment: as_bool(&e[5])?,
    })
}

fn actor_of(e: &Entity) -> Result<FrameActor> {
    Ok(FrameActor {
        id: as_id(&e[0])?,
        species: as_str(&e[1])?,
        pos: as_pos(&e[2])?,
        yaw: dyaw(as_int(&e[3])?),
        aim: as_unit_aim(&e[4])?,
        weapon: as_opt_str(&e[5])?,
        shots: as_opt_int(&e[6])?.map(|n| n as u32),
        team: as_opt_str(&e[7])?,
        vehicle: as_opt_id(&e[8])?,
        stance: as_enum(&e[9], &STANCES)?,
        speed: as_opt_int(&e[10])?.map(|q| q as f64 / SPEED_SCALE),
        actions: as_actions(&e[11])?,
        grenade: as_enum(&e[12], &GRENADES)?,
        // Versions 1 and 2 have none of these.
        vel: e.get(13).map(as_opt_vel).transpose()?.flatten(),
        fire: e
            .get(14)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|m| m as u32),
        body: e
            .get(15)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|q| q as f64 / BODY_MAX),
        shield: e
            .get(16)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|q| q as f64 / SHIELD_SCALE),
        seat: e
            .get(17)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|n| n as u32),
        actions_ago: e.get(18).map(as_agos).transpose()?.flatten(),
        anim: as_anim(e, 19)?,
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
        ago: e
            .get(6)
            .map(as_opt_int)
            .transpose()?
            .flatten()
            .map(|n| n as u32),
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

/// A frame as entity lists: players, vehicles, actors, machines, objects,
/// projectiles, items (the [`LISTS`] order) and hits.
#[derive(Default)]
struct Lists {
    lists: [Vec<Entity>; LIST_COUNT],
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
                collect(&frame.projectiles, projectile_vals, MAX_FRAME_PROJECTILES)?,
                collect(&frame.items, item_vals, MAX_FRAME_ITEMS)?,
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
    cut: Option<u64>,
    cinematic: bool,
    lists: &[Vec<Entity>; LIST_COUNT],
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
        machine_positions: Vec::new(),
        objects: map(&lists[4], object_of)?,
        cut,
        cinematic,
        projectiles: map(&lists[5], projectile_of)?,
        items: map(&lists[6], item_of)?,
    })
}

/// What `decode(encode(frame))` gives: the frame with every value
/// quantised as the packed format carries it.
pub fn quantize(frame: &Frame) -> Result<Frame> {
    let lists = Lists::of(frame)?;
    let mut quantized = frame_of(
        &frame.game_id,
        frame.tick,
        frame.bsp.map(u64::from),
        frame.cut,
        frame.cinematic,
        &lists.lists,
        &lists.hits,
    )?;
    quantized
        .machine_positions
        .clone_from(&frame.machine_positions);
    Ok(quantized)
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

/// A vector with a base uses one bit for a zero axis, then codes only
/// nonzero residuals. A keyframe or a newly present field is absolute.
fn write_vector(
    w: &mut BitWriter,
    q: &[i64; 3],
    prev: Option<&[i64; 3]>,
    abs_k: u32,
    delta_k: u32,
) {
    if let Some(p) = prev {
        for (v, base) in q.iter().zip(p) {
            w.bit(v != base);
            if v != base {
                w.seg(v - base, delta_k);
            }
        }
    } else {
        for &v in q {
            w.seg(v, abs_k);
        }
    }
}

fn read_vector(
    r: &mut BitReader,
    prev: Option<&[i64; 3]>,
    abs_k: u32,
    delta_k: u32,
    old_k: u32,
) -> Result<[i64; 3]> {
    let mut q = prev.copied().unwrap_or_default();
    for v in &mut q {
        let residual = if r.version < V7 {
            // Both legacy grids have exactly half the new scale.
            r.seg(if prev.is_some() { old_k } else { abs_k })? * 2
        } else if prev.is_none() {
            r.seg(abs_k)?
        } else if r.bit()? {
            r.seg(delta_k)?
        } else {
            0
        };
        *v = v
            .checked_add(residual)
            .ok_or(PackError::Bad("vector out of range"))?;
    }
    Ok(q)
}

fn write_angle_component(w: &mut BitWriter, value: u32, base: Option<u32>, wrap: bool) {
    if let Some(base) = base {
        let mut d = i64::from(value) - i64::from(base);
        if wrap {
            d = (d + 8192).rem_euclid(16384) - 8192;
        }
        let short = (-64..64).contains(&d);
        w.bit(!short);
        if short {
            w.seg(d, 0);
            return;
        }
    }
    #[cfg(test)]
    if floor::SHORT_AXIS.get() {
        w.bits(u64::from(value), 12);
        return;
    }
    w.bits(u64::from(value), 14);
}

fn read_angle_component(r: &mut BitReader, base: Option<u32>, wrap: bool) -> Result<u32> {
    if let Some(base) = base
        && !r.bit()?
    {
        let d = r.seg(0)?;
        if !(-64..64).contains(&d) {
            return Err(PackError::Bad("angle residual out of range"));
        }
        let v = i64::from(base) + d;
        return if wrap {
            Ok(v.rem_euclid(16384) as u32)
        } else {
            u32::try_from(v).map_err(|_| PackError::Bad("direction out of range"))
        };
    }
    #[cfg(test)]
    if floor::SHORT_AXIS.get() {
        return Ok(r.bits(12)? as u32);
    }
    Ok(r.bits(14)? as u32)
}

fn advanced_phase(prev: Option<&Val>, ticks: u64) -> Option<u64> {
    match prev {
        Some(Val::Int(q)) if (1..=PREDICT_TICKS).contains(&ticks) => Some(q + ticks * 16),
        _ => None,
    }
}

fn write_value(w: &mut BitWriter, kind: Kind, v: &Val, prev: Option<&Val>, t: &Table, ticks: u64) {
    match (kind, v) {
        (Kind::Bool, Val::Bool(b)) => w.bit(*b),
        (Kind::Pos, Val::Pos(q)) => write_vector(
            w,
            q,
            match prev {
                Some(Val::Pos(p)) => Some(p),
                _ => None,
            },
            16,
            1,
        ),
        (Kind::Angle, Val::Int(n)) => write_angle_component(
            w,
            *n as u32,
            match prev {
                Some(Val::Int(p)) => Some(*p as u32),
                _ => None,
            },
            true,
        ),
        (Kind::Dir, Val::Dir(q)) => {
            let base = match prev {
                Some(Val::Dir(p)) => Some(p),
                _ => None,
            };
            for i in 0..2 {
                write_angle_component(w, q[i], base.map(|p| p[i]), false);
            }
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
        (Kind::AnimPhase, Val::Int(n)) => {
            if let Some(phase) = advanced_phase(prev, ticks) {
                w.bit(*n == phase);
                if *n == phase {
                    return;
                }
            }
            w.eg(*n, 4);
        }
        (Kind::Uint(k), Val::Int(n)) => w.eg(*n, k),
        (Kind::Bits(n), Val::Int(v)) => w.bits(*v, n),
        (Kind::Actions, Val::List(l)) => {
            w.eg(l.len() as u64, 0);
            l.iter().for_each(|a| w.bits(u64::from(*a), 3));
        }
        (Kind::Vel, Val::Vel(q)) => write_vector(
            w,
            q,
            match prev {
                Some(Val::Vel(p)) => Some(p),
                _ => None,
            },
            8,
            1,
        ),
        (Kind::Agos, Val::Ints(l)) => {
            w.eg(l.len() as u64, 0);
            l.iter().for_each(|a| w.eg(*a, 1));
        }
        (kind, v) => unreachable!("{kind:?} cannot hold {v:?}"),
    }
}

fn read_value(
    r: &mut BitReader,
    kind: Kind,
    prev: Option<&Val>,
    strings: &[String],
    ticks: u64,
) -> Result<Val> {
    let string = |i: u64| {
        strings
            .get(i as usize)
            .cloned()
            .ok_or(PackError::Bad("string index out of range"))
    };
    Ok(match kind {
        Kind::Bool => Val::Bool(r.bit()?),
        Kind::Pos => Val::Pos(read_vector(
            r,
            match prev {
                Some(Val::Pos(p)) => Some(p),
                _ => None,
            },
            16,
            1,
            7,
        )?),
        Kind::Angle if r.version < V7 => Val::Int(r.bits(12)? * 4),
        Kind::Angle => Val::Int(u64::from(read_angle_component(
            r,
            match prev {
                Some(Val::Int(p)) => Some(*p as u32),
                _ => None,
            },
            true,
        )?)),
        Kind::Dir if r.version < V7 => {
            let q = [r.bits(12)? as u32, r.bits(12)? as u32];
            if q.iter().any(|&v| v > 4094) {
                return Err(PackError::Bad("direction out of range"));
            }
            Val::LegacyDir(q)
        }
        Kind::Dir => {
            let base = match prev {
                Some(Val::Dir(p)) => Some(p),
                _ => None,
            };
            let q = [
                read_angle_component(r, base.map(|p| p[0]), false)?,
                read_angle_component(r, base.map(|p| p[1]), false)?,
            ];
            if q.iter().any(|&v| v > DIR_MAX as u32) {
                return Err(PackError::Bad("direction out of range"));
            }
            Val::Dir(q)
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
        Kind::AnimPhase => {
            if r.version >= V7
                && let Some(phase) = advanced_phase(prev, ticks)
                && r.bit()?
            {
                Val::Int(phase)
            } else {
                Val::Int(r.eg(4)?)
            }
        }
        Kind::Uint(k) => Val::Int(r.eg(k)?),
        Kind::Bits(n) => {
            let v = r.bits(n)?;
            if n == DEVICE_BITS && v > DEVICE_MAX as u64 {
                return Err(PackError::Bad("device position out of range"));
            }
            Val::Int(v)
        }
        Kind::Vel => Val::Vel(read_vector(
            r,
            match prev {
                Some(Val::Vel(p)) => Some(p),
                _ => None,
            },
            8,
            1,
            4,
        )?),
        Kind::Agos => {
            let n = r.eg(0)?;
            if n > MAX_FRAME_ACTIONS as u64 {
                return Err(PackError::Bad("too many action ticks"));
            }
            Val::Ints((0..n).map(|_| r.eg(1)).collect::<Result<Vec<u64>>>()?)
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

fn write_field(
    w: &mut BitWriter,
    field: Field,
    v: &Val,
    prev: Option<&Val>,
    t: &Table,
    ticks: u64,
) {
    if field.opt {
        w.bit(!matches!(v, Val::None));
    }
    if !matches!(v, Val::None) {
        write_value(w, field.kind, v, prev, t, ticks);
    }
}

fn read_field(
    r: &mut BitReader,
    field: Field,
    prev: Option<&Val>,
    strings: &[String],
    ticks: u64,
) -> Result<Val> {
    if field.opt && !r.bit()? {
        return Ok(Val::None);
    }
    read_value(r, field.kind, prev, strings, ticks)
}

fn write_entity(w: &mut BitWriter, schema: &[Field], e: &Entity, t: &Table) {
    for (i, (field, v)) in schema.iter().zip(e).enumerate() {
        #[cfg(test)]
        floor::before_field(schema, i);
        #[cfg(test)]
        floor::absolute(i, v);
        write_field(w, *field, v, None, t, 0);
        #[cfg(test)]
        floor::field(w, i);
        let _ = i;
    }
}

fn read_schema_field(
    r: &mut BitReader,
    schema: &[Field],
    i: usize,
    prev: Option<&Val>,
    strings: &[String],
    ticks: u64,
) -> Result<Val> {
    #[cfg(test)]
    floor::before_field(schema, i);
    let result = read_field(r, schema[i], prev, strings, ticks);
    #[cfg(test)]
    floor::SHORT_AXIS.set(false);
    result
}

fn read_entity(r: &mut BitReader, schema: &[Field], strings: &[String]) -> Result<Entity> {
    schema
        .iter()
        .enumerate()
        .map(|(i, _)| read_schema_field(r, schema, i, None, strings, 0))
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
    #[cfg(test)]
    floor::charge(w, "edit/count");
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

/// Hit positions often coincide exactly with a player's position in the
/// same tick. A four-bit list ordinal replaces three absolute axes only
/// on exact equality; off-body impacts keep their full position.
fn write_hits(w: &mut BitWriter, hits: &[Entity], players: &[Entity], table: &Table) {
    w.eg(hits.len() as u64, 2);
    #[cfg(test)]
    floor::charge(w, "edit/count");
    for hit in hits {
        for (i, field) in HIT.iter().enumerate() {
            if i == 3 && !players.is_empty() {
                let index = players.iter().position(|p| p[2] == hit[i]);
                w.bit(index.is_some());
                if let Some(index) = index {
                    w.bits(index as u64, 4);
                    #[cfg(test)]
                    {
                        floor::absolute(i, &Val::Int(index as u64));
                        floor::field(w, i);
                    }
                    continue;
                }
            }
            #[cfg(test)]
            floor::absolute(i, &hit[i]);
            write_field(w, *field, &hit[i], None, table, 0);
            #[cfg(test)]
            floor::field(w, i);
        }
    }
}

fn read_hits(r: &mut BitReader, players: &[Entity], strings: &[String]) -> Result<Vec<Entity>> {
    let count = read_count(r, MAX_FRAME_HITS)?;
    let mut hits = Vec::with_capacity(count);
    for _ in 0..count {
        let mut hit = Vec::with_capacity(HIT.len());
        for (i, field) in HIT.iter().enumerate() {
            if i == 3 && !players.is_empty() && r.bit()? {
                let player = players
                    .get(r.bits(4)? as usize)
                    .ok_or(PackError::Bad("hit position player out of range"))?;
                hit.push(player[2].clone());
            } else {
                hit.push(read_field(r, *field, None, strings, 0)?);
            }
        }
        hits.push(hit);
    }
    Ok(hits)
}

/// One older quantized sample per unit slot. Keys guard roster changes.
/// Retaining just these fields avoids keeping or cloning an extra frame.
#[derive(Default, Clone, Copy)]
struct UnitAnchor {
    key: Option<u64>,
    yaw: u32,
    aim: Option<[u32; 2]>,
    velocity: Option<[i64; 3]>,
}

impl UnitAnchor {
    fn read(e: &Entity, aim: usize) -> Self {
        Self {
            key: match e[0] {
                Val::Int(n) => Some(n),
                Val::Id(Id::Datum(d)) => Some(u64::from(d)),
                _ => None,
            },
            yaw: match e[3] {
                Val::Int(n) => n as u32,
                _ => unreachable!(),
            },
            aim: match e[aim] {
                Val::Dir(q) => Some(q),
                _ => None,
            },
            velocity: match e[13] {
                Val::Vel(v) => Some(v),
                _ => None,
            },
        }
    }
}

/// The mean velocity over this span, in 1/1024-unit position quanta.
/// A held velocity has equal endpoints. All division rounds ties away
/// from zero, identically in the browser. Saturation is reachable only
/// for out-of-contract wire values.
fn advance_position(p: &mut Val, a: [i64; 3], b: [i64; 3], ticks: u64) {
    let Val::Pos(p) = p else {
        return;
    };
    for i in 0..3 {
        let step = a[i]
            .saturating_add(b[i])
            .saturating_mul((position_scale() as i64 / 256) * ticks as i64);
        p[i] = p[i].saturating_add(step.saturating_add(15 * step.signum()) / 30);
    }
}

impl StreamState {
    /// Predict before computing any run or edit. Equal adjacent spans
    /// can continue a unit's turn and acceleration; an irregular span
    /// uses only its last velocity. Residuals recover the exact sample.
    fn advance(&mut self, ticks: u64) {
        let moving = (1..=PREDICT_TICKS).contains(&ticks);
        for (slot, n, aim) in [(0, 0, 6), (1, 2, 4)] {
            let list = &mut self.lists[n];
            let history = &mut self.anchors[slot];
            history.resize(list.len(), UnitAnchor::default());
            for (e, prior) in list.iter_mut().zip(history) {
                let now = UnitAnchor::read(e, aim);
                let older = std::mem::replace(prior, now);
                if !moving {
                    continue;
                }
                let mut velocity = now.velocity;
                if ticks == self.span && now.key.is_some() && now.key == older.key {
                    e[3] = Val::Int(u64::from(
                        now.yaw.wrapping_mul(2).wrapping_sub(older.yaw) & 16383,
                    ));
                    if let (Some(a), Some(b)) = (older.aim, now.aim) {
                        e[aim] = Val::Dir(std::array::from_fn(|i| {
                            (2 * (b[i] as i32) - a[i] as i32).clamp(0, unit_aim_max()) as u32
                        }));
                    }
                    if let (Some(a), Some(b)) = (older.velocity, now.velocity) {
                        let v =
                            std::array::from_fn(|i| b[i].saturating_add(b[i].saturating_sub(a[i])));
                        e[13] = Val::Vel(v);
                        velocity = Some(v);
                    }
                }
                if let (Some(a), Some(b)) = (now.velocity, velocity) {
                    advance_position(&mut e[2], a, b, ticks);
                }
            }
        }
        if moving {
            for (n, (fields, _)) in LISTS.iter().enumerate() {
                if n == 0 || n == 2 || self.lists[n].is_empty() {
                    continue;
                }
                if let Some((pi, vi)) = motion_fields(fields) {
                    for e in &mut self.lists[n] {
                        if let Val::Vel(v) = e[vi] {
                            advance_position(&mut e[pi], v, v, ticks);
                        }
                    }
                }
            }
        }
        self.span = ticks;
    }
}

fn key_of(e: &Entity, first: usize, second: Option<usize>) -> (&Val, Option<&Val>) {
    (&e[first], second.map(|i| &e[i]))
}

/// A unit's changed-field mask in frequency order. Only the difference
/// from the preceding edit mask is sent, four bits at a time. Scanning
/// set bits makes encoding cheap when few fields toggle.
struct UnitMask {
    wire_bit: [u32; 21],
    field_bits: [[u32; 16]; 5],
}

impl UnitMask {
    const fn new(order: [usize; 20]) -> Self {
        let mut wire_bit = [0; 21];
        let mut field_bits = [[0; 16]; 5];
        let mut i = 0;
        while i < 20 {
            wire_bit[order[i]] = 1 << i;
            let mut nibble = 0;
            while nibble < 16 {
                if nibble & (1 << (i % 4)) != 0 {
                    field_bits[i / 4][nibble] |= 1 << order[i];
                }
                nibble += 1;
            }
            i += 1;
        }
        Self {
            wire_bit,
            field_bits,
        }
    }

    fn write(&self, w: &mut BitWriter, mut difference: u32) {
        let mut wire = 0;
        while difference != 0 {
            wire |= self.wire_bit[difference.trailing_zeros() as usize];
            difference &= difference - 1;
        }
        while wire >= 16 {
            w.bits(u64::from((wire & 15) | 16), 5);
            wire >>= 4;
        }
        w.bits(u64::from(wire), 5);
    }

    fn read(&self, r: &mut BitReader) -> Result<u32> {
        let mut mask = 0;
        for group in &self.field_bits {
            let value = r.bits(5)? as usize;
            mask |= group[value & 15];
            if value < 16 {
                return Ok(mask);
            }
        }
        Err(PackError::Bad("unit mask too long"))
    }
}

fn unit_mask(list: usize) -> Option<&'static UnitMask> {
    // Fire, shield, yaw and shot count toggle most often in the seeded
    // match. A unit's velocity, position, aim and actions form group two.
    static PLAYER_MASK: UnitMask = UnitMask::new([
        14, 16, 3, 8, 13, 2, 6, 11, 20, 9, 10, 7, 1, 4, 5, 12, 15, 17, 18, 19,
    ]);
    static ACTOR_MASK: UnitMask = UnitMask::new([
        14, 16, 3, 6, 13, 2, 4, 11, 20, 9, 10, 5, 1, 7, 8, 12, 15, 17, 18, 19,
    ]);
    match list {
        0 => Some(&PLAYER_MASK),
        2 => Some(&ACTOR_MASK),
        _ => None,
    }
}

/// List count, then operations until the new list is complete. V7 codes
/// the common edit at the cursor as one bit. In read order, 00 copies a
/// predicted run, 010 inserts an entity and 011 edits at a relative slot.
fn write_delta(
    w: &mut BitWriter,
    schema: &[Field],
    list: &[Entity],
    prev: &[Entity],
    t: &Table,
    ticks: u64,
    layout: Option<&UnitMask>,
) {
    // Most frames list the same entities in the same order: match by
    // position, and only key a map when that fails.
    let mut keys = schema
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.key.then_some(i));
    let first = keys.next().expect("a delta schema has a key");
    let second = keys.next();
    let same_order = list.len() == prev.len()
        && list
            .iter()
            .zip(prev)
            .all(|(e, p)| key_of(e, first, second) == key_of(p, first, second));
    let reordered: Vec<Option<usize>> = if same_order {
        Vec::new()
    } else {
        let mut keys = HashMap::with_capacity(prev.len());
        for (j, e) in prev.iter().enumerate() {
            keys.entry(key_of(e, first, second)).or_insert(j);
        }
        let mut used = vec![false; prev.len()];
        list.iter()
            .map(|e| {
                let j = *keys.get(&key_of(e, first, second))?;
                (!std::mem::replace(&mut used[j], true)).then_some(j)
            })
            .collect()
    };
    w.eg(list.len() as u64, 2);
    let mut cursor = 0usize;
    let matched = |i: usize| if same_order { Some(i) } else { reordered[i] };
    #[cfg(test)]
    {
        floor::charge(w, "edit/count");
        floor::matched(schema, list, prev, matched, ticks);
    }
    let mut last_mask = 0u32;
    let mut i = 0;
    while i < list.len() {
        let mut run = 0;
        while i + run < list.len()
            && matched(i + run) == Some(cursor + run)
            && list[i + run] == prev[cursor + run]
        {
            run += 1;
        }
        if run > 0 {
            w.bits(0, 2);
            w.eg(run as u64 - 1, 2);
            #[cfg(test)]
            floor::charge(w, "edit/count");
            i += run;
            cursor += run;
            continue;
        }
        match matched(i) {
            Some(j) => {
                if j == cursor {
                    w.bit(true);
                } else {
                    w.bits(0b110, 3);
                    w.seg(j as i64 - cursor as i64, 0);
                }
                let before = &prev[j];
                let entity = &list[i];
                // Key fields already matched and compare equal.
                let mask = entity
                    .iter()
                    .zip(before)
                    .enumerate()
                    .fold(0u32, |mask, (i, (v, p))| mask | (u32::from(v != p) << i));
                #[cfg(test)]
                floor::charge(w, "edit/count");
                if let Some(layout) = layout {
                    layout.write(w, mask ^ last_mask);
                } else {
                    w.bit(mask != last_mask);
                    if mask != last_mask {
                        for (i, field) in schema.iter().enumerate() {
                            if !field.key {
                                w.bit(mask & (1 << i) != 0);
                            }
                        }
                    }
                }
                #[cfg(test)]
                floor::charge(w, "mask");
                last_mask = mask;
                let mut changed = mask;
                while changed != 0 {
                    let i = changed.trailing_zeros() as usize;
                    #[cfg(test)]
                    floor::before_field(schema, i);
                    write_field(w, schema[i], &entity[i], Some(&before[i]), t, ticks);
                    #[cfg(test)]
                    floor::field(w, i);
                    changed &= changed - 1;
                }
                cursor = j + 1;
            }
            None => {
                w.bits(0b010, 3);
                #[cfg(test)]
                floor::charge(w, "edit/count");
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
    dt: u64,
    list: usize,
) -> Result<Vec<Entity>> {
    const HERE: usize = usize::MAX;
    let motion = if r.version == 6 {
        motion_fields(schema)
    } else {
        None
    };
    let layout = if r.version >= V7 {
        unit_mask(list)
    } else {
        None
    };
    let n = read_count(r, max)?;
    let mut out: Vec<Entity> = Vec::with_capacity(n);
    // Where each of `prev` was moved to in `out` (HERE: not moved).
    let mut moved: Vec<usize> = Vec::new();
    let mut last_mask = 0u32;
    let mut cursor = 0usize;
    enum Edit {
        Run,
        Here,
        Relative,
        New,
    }
    while out.len() < n {
        let op = if r.version >= V7 && r.bit()? {
            Edit::Here
        } else if !r.bit()? {
            Edit::Run
        } else if r.bit()? {
            Edit::Relative
        } else {
            Edit::New
        };
        if matches!(op, Edit::Run) {
            let run = r.eg(2)? as usize + 1;
            if out.len() + run > n || cursor + run > prev.len() {
                return Err(PackError::Bad("run past the list"));
            }
            if moved.is_empty() {
                moved.resize(prev.len(), HERE);
            }
            for (k, e) in prev[cursor..cursor + run].iter_mut().enumerate() {
                let source = &mut moved[cursor + k];
                if *source == HERE {
                    *source = out.len();
                    out.push(std::mem::take(e));
                } else {
                    out.push(out[*source].clone());
                }
            }
            cursor += run;
            continue;
        }
        if !matches!(op, Edit::New) {
            let offset = if matches!(op, Edit::Relative) {
                r.seg(0)?
            } else {
                0
            };
            let j = cursor as i64 + offset;
            let j = usize::try_from(j)
                .ok()
                .filter(|j| *j < prev.len())
                .ok_or(PackError::Bad("entity refers past the list"))?;
            let before = match moved.get(j) {
                Some(&at) if at != HERE => &out[at],
                _ => &prev[j],
            };
            if let Some(layout) = layout {
                last_mask ^= layout.read(r)?;
            } else if r.version < V7 || r.bit()? {
                last_mask = 0;
                for (idx, f) in schema.iter().enumerate() {
                    if !f.key && r.bit()? {
                        last_mask |= 1 << idx;
                    }
                }
            }
            let mut e = before.clone();
            // Only v6 defers the position until its new velocity is read.
            let mut error = None;
            let base = motion.and_then(|(_, vi)| predict_base(&before[vi], dt));
            let mut mask = last_mask;
            while mask != 0 {
                let idx = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                match (motion, base) {
                    (Some((pi, _)), Some(_)) if idx == pi => {
                        error = Some([r.seg(PREDICT_K)?, r.seg(PREDICT_K)?, r.seg(PREDICT_K)?]);
                    }
                    _ => {
                        e[idx] = read_schema_field(r, schema, idx, Some(&before[idx]), strings, dt)?
                    }
                }
            }
            if let (Some((pi, vi)), Some(a), Some(err)) = (motion, base, error) {
                let Val::Pos(pp) = before[pi] else {
                    return Err(PackError::Bad("expected a position"));
                };
                let guess = self::predict(pp, a, &e[vi], dt);
                let mut q = [0i64; 3];
                for c in 0..3 {
                    q[c] = guess[c]
                        .checked_add(err[c] * 2)
                        .ok_or(PackError::Bad("position out of range"))?;
                }
                e[pi] = Val::Pos(q);
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
    span: u64,
    anchors: [Vec<UnitAnchor>; 2],
    strings: Vec<String>,
    lists: [Vec<Entity>; LIST_COUNT],
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
    causal: Option<Box<v8::Writer>>,
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
            causal: None,
        }
    }

    /// Select the version agreed for this connection. New subscribers start at a key.
    pub fn with_version(sid: u64, keyframe_every: u64, version: u8) -> Result<Self> {
        let mut writer = Self::new(sid, keyframe_every);
        match version {
            7 => {}
            LIVE_VERSION => writer.causal = Some(Box::default()),
            v => return Err(PackError::Version(u64::from(v))),
        }
        Ok(writer)
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
        self.since_key = self.keyframe_every;
    }

    /// One packed frame; `Ok((bytes, keyframe))`. A frame that cannot be
    /// packed leaves the stream as it was.
    pub fn encode(&mut self, frame: &Frame) -> Result<(Vec<u8>, bool)> {
        self.encode_with_machine_baseline(frame, None)
    }

    /// On a keyframe, repeat the current positions for readers joining or
    /// recovering their stream. The caller's baseline includes this frame.
    pub fn encode_with_machine_baseline(
        &mut self,
        frame: &Frame,
        baseline: Option<&BTreeMap<u16, MachinePosition>>,
    ) -> Result<(Vec<u8>, bool)> {
        let positions = &frame.machine_positions;
        if positions.len() > MAX_MACHINE_POSITIONS
            || positions.last().is_some_and(|m| m.i > i16::MAX as u16)
            || positions.windows(2).any(|w| w[0].i >= w[1].i)
        {
            return Err(PackError::Value("invalid machine positions"));
        }
        if frame.game_id.len() > MAX_STRING {
            return Err(PackError::Value("game id too long"));
        }
        if let Some(writer) = self.causal.as_mut() {
            if self.next_seq == u64::MAX
                || frame.tick >= 1 << 44
                || frame.cut.is_some_and(|v| v >= 1 << 44)
            {
                return Err(PackError::Value("v8 tick or sequence out of range"));
            }
            let result = writer.encode(
                frame,
                self.sid,
                self.next_seq,
                self.since_key >= self.keyframe_every,
                baseline,
            );
            match &result {
                Ok((_, key)) => {
                    self.since_key = if *key { 1 } else { self.since_key + 1 };
                    self.next_seq += 1;
                }
                // A partial projection or model update cannot become a delta base.
                Err(_) => {
                    self.causal = Some(Box::default());
                    self.since_key = self.keyframe_every;
                }
            }
            return result;
        }
        let lists = Lists::of(frame)?;
        let mut new_strings: Vec<&str> = Vec::new();
        let version = VERSION;
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
                    || frame.cut.is_some()
                    || s.game_id != frame.game_id
                    || frame.tick < s.tick
                    || s.strings.len() + new_strings.len() > MAX_STRINGS
                    || new_strings.len() as u64 > MAX_NEW_STRINGS
            }
        };
        let baseline = baseline.filter(|_| key);
        if baseline.is_some_and(|b| {
            b.len() > MAX_MACHINE_POSITIONS
                || b.iter().any(|(&i, m)| i != m.i || i > i16::MAX as u16)
        }) {
            return Err(PackError::Value("invalid machine position baseline"));
        }
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
        #[cfg(test)]
        floor::start(key);
        w.bits(version, 4);
        w.bit(key);
        w.eg(self.sid, 0);
        w.eg(seq, 8);
        let bsp = frame.bsp.map(u64::from);
        let dt = if key { 0 } else { frame.tick - state.tick };
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
        w.bit(frame.cut.is_some());
        if let Some(c) = frame.cut {
            w.eg(c, 16);
        }
        w.bit(frame.cinematic);
        #[cfg(test)]
        floor::charge(&w, "header");
        w.eg(new_strings.len() as u64, 1);
        for s in &new_strings {
            write_str_literal(&mut w, s);
            table
                .index
                .insert((*s).to_owned(), state.strings.len() as u64);
            state.strings.push((*s).to_owned());
        }
        #[cfg(test)]
        floor::charge(&w, "strings");
        if !key {
            state.advance(dt);
        }
        for (n, (schema, _)) in LISTS.iter().enumerate() {
            if n == 3 {
                #[cfg(test)]
                floor::set_list(LIST_COUNT);
                write_hits(&mut w, &lists.hits, &lists.lists[0], &table);
            }
            #[cfg(test)]
            floor::set_list(n);
            if key {
                write_full(&mut w, schema, &lists.lists[n], &table);
            } else {
                write_delta(
                    &mut w,
                    schema,
                    &lists.lists[n],
                    &state.lists[n],
                    &table,
                    dt,
                    unit_mask(n),
                );
            }
        }
        state.seq = seq;
        state.version = version;
        state.game_id = frame.game_id.clone();
        state.tick = frame.tick;
        state.bsp = bsp;
        state.lists = lists.lists;
        self.state = Some((state, table));
        self.next_seq += 1;
        self.since_key = if key { 1 } else { self.since_key + 1 };
        if let Some(baseline) = baseline {
            write_machine_positions(&mut w, baseline.values());
        } else {
            write_machine_positions(&mut w, frame.machine_positions.iter());
        }
        #[cfg(test)]
        {
            floor::set_list(LIST_COUNT + 1);
            floor::charge(&w, "machine travel");
            floor::end(&w);
        }
        Ok((w.finish(), key))
    }
}

/// A decoded frame and where it sat in its stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub sid: u64,
    pub seq: u64,
    pub keyframe: bool,
    /// This record replaces the entire machine-travel baseline, even if empty.
    pub machine_baseline: bool,
    pub frame: Frame,
}

/// Decodes packed frames of any number of streams (keyed by `sid`).
#[derive(Default)]
pub struct Decoder {
    streams: HashMap<u64, StreamState>,
    causal: HashMap<u64, v8::Stream>,
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
        self.causal.remove(&sid);
    }

    pub fn decode(&mut self, bytes: &[u8]) -> Result<Decoded> {
        if bytes.first().is_some_and(|b| b & 15 == LIVE_VERSION) {
            return v8::decode(self, bytes);
        }
        let mut r = BitReader::new(bytes);
        let version = r.bits(4)?;
        r.version = version;
        let schema = schema_of(version).ok_or(PackError::Version(version))?;
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
        let cut = if schema.cut && r.bit()? {
            Some(r.eg(16)?)
        } else {
            None
        };
        let cinematic = schema.cinematic && r.bit()?;
        let fresh = r.eg(1)?;
        if fresh > MAX_NEW_STRINGS || state.strings.len() as u64 + fresh > MAX_STRINGS as u64 {
            return Err(PackError::Bad("too many strings"));
        }
        for _ in 0..fresh {
            state.strings.push(read_str_literal(&mut r)?);
        }
        let mut hits = Vec::new();
        let dt = state.tick - prev.tick;
        if version >= V7 && !key {
            prev.advance(dt);
            state.anchors = std::mem::take(&mut prev.anchors);
            state.span = dt;
        }
        for (n, (fields, max)) in schema.lists.iter().enumerate() {
            if n == 3 {
                hits = if version >= V7 {
                    read_hits(&mut r, &state.lists[0], &state.strings)?
                } else {
                    read_full(&mut r, schema.hit, MAX_FRAME_HITS, &state.strings)?
                };
            }
            state.lists[n] = if key {
                read_full(&mut r, fields, *max, &state.strings)?
            } else {
                read_delta(
                    &mut r,
                    fields,
                    *max,
                    &mut prev.lists[n],
                    &state.strings,
                    dt,
                    n,
                )?
            };
        }
        let mut frame = frame_of(
            &state.game_id,
            state.tick,
            state.bsp,
            cut,
            cinematic,
            &state.lists,
            &hits,
        )?;
        if version == V7 {
            frame.machine_positions = read_machine_positions(&mut r)?;
        }
        self.causal.remove(&sid);
        if !self.streams.contains_key(&sid) && self.streams.len() + self.causal.len() >= MAX_STREAMS
        {
            return Err(PackError::Bad("too many streams"));
        }
        self.streams.insert(sid, state);
        Ok(Decoded {
            sid,
            seq,
            keyframe: key,
            machine_baseline: version == V7 && key,
            frame,
        })
    }
}

// The tail is absent for zero updates. A valid nonempty tail takes at
// least 15 bits, so the old v7 body's zero padding is unambiguous.
fn write_machine_positions<'a>(
    w: &mut BitWriter,
    positions: impl ExactSizeIterator<Item = &'a MachinePosition>,
) {
    let count = positions.len();
    if count == 0 {
        return;
    }
    w.eg(count as u64 - 1, 2);
    let mut next = 0;
    for m in positions {
        let i = u64::from(m.i);
        w.eg(i - next, 3);
        w.bits(u64::from(m.p), 8);
        next = i + 1;
    }
}

fn read_machine_positions(r: &mut BitReader<'_>) -> Result<Vec<MachinePosition>> {
    let mut positions = Vec::new();
    if r.remaining() > 7 {
        let count = r.eg(2)? + 1;
        if count > MAX_MACHINE_POSITIONS as u64 {
            return Err(PackError::Bad("too many machine positions"));
        }
        positions.reserve_exact(count as usize);
        let mut next = 0u64;
        for _ in 0..count {
            let gap = r.eg(3)?;
            let i = next
                .checked_add(gap)
                .filter(|&i| i <= i16::MAX as u64)
                .ok_or(PackError::Bad("machine position index out of range"))?;
            positions.push(MachinePosition {
                i: i as u16,
                p: r.bits(8)? as u8,
            });
            next = i + 1;
        }
    }
    let padding = r.remaining();
    if padding > 7 || r.bits(padding as u32)? != 0 {
        return Err(PackError::Bad("bad machine position padding"));
    }
    Ok(positions)
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

#[cfg(test)]
#[path = "pack_floor.rs"]
pub(crate) mod floor;

// Integration benches link the regular library: the v7 control has no
// cfg(test) bit-accounting or alternate-grid hooks.
#[cfg(feature = "live-floor-bench")]
#[doc(hidden)]
#[path = "pack_speed.rs"]
pub mod speed;
