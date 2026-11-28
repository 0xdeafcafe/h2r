//! `ForgeExt`: the `forge` record of the `SimExt` blob (docs/game/forge-design-slices.md,
//! "F7 in detail"). It holds what Forge adds that retail has no home for: emitter
//! counters, logic node state, barrier switches and volume occupancy.
//!
//! The record is plain bytes (`encode`, `decode`) in one canonical form: every list
//! is sorted by op id with no repeats, so two machines that reach one state write
//! one byte string, and the sim hash folds the same digest. [`VERSION`] and
//! [`MAX_BYTES`] equal `sim_world::ext::FORGE`'s version and cap.
//!
//! An emitter's shots come from the tick count alone ([`Emitter::shot`]); the record
//! only counts what fired, so a machine that joins late and a machine that never
//! stopped agree on the next shot.

use std::collections::BTreeMap;

/// The record's layout version, `sim_world::ext::FORGE`'s.
pub const VERSION: u16 = 1;
/// The record's cap in bytes, `sim_world::ext::FORGE`'s.
pub const MAX_BYTES: usize = 64 * 1024;
/// An object count in one volume's occupancy.
pub const MAX_OCCUPANTS: usize = 256;

/// Why a record was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes end inside a field.
    Truncated,
    /// Bytes remain after the last list.
    Trailing,
    /// A list not in strictly ascending id order.
    Order(&'static str),
    /// A byte that is not 0 or 1 where a flag is.
    Flag,
    /// A volume with more occupants than [`MAX_OCCUPANTS`].
    Occupants(u64),
    /// The record would be over [`MAX_BYTES`].
    TooLarge(usize),
}

/// What the record keeps for one emitter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EmitterState {
    /// Shots made, counted from the first.
    pub fired: u32,
    /// Shots skipped because the emitter was at its limit of live objects.
    pub skipped: u32,
}

/// One logic node's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeState {
    /// A latch or a counter's output, or a timer's running flag.
    pub on: bool,
    /// A counter's value, or a timer's ticks left.
    pub value: i32,
}

/// The sim hash's view of Forge's own state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForgeExt {
    /// The seed of the `random` node's generator, never the game's.
    pub seed: u64,
    pub emitters: BTreeMap<u64, EmitterState>,
    pub nodes: BTreeMap<u64, NodeState>,
    /// A barrier's enable switch.
    pub switches: BTreeMap<u64, bool>,
    /// The object ids inside each volume, ascending.
    pub volumes: BTreeMap<u64, Vec<u32>>,
}

impl ForgeExt {
    /// An empty record holds nothing, so the container leaves it out.
    pub fn is_empty(&self) -> bool {
        self.seed == 0
            && self.emitters.is_empty()
            && self.nodes.is_empty()
            && self.switches.is_empty()
            && self.volumes.is_empty()
    }

    /// The canonical bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut o = Vec::new();
        o.extend_from_slice(&self.seed.to_le_bytes());
        put_len(&mut o, self.emitters.len());
        for (id, e) in &self.emitters {
            o.extend_from_slice(&id.to_le_bytes());
            o.extend_from_slice(&e.fired.to_le_bytes());
            o.extend_from_slice(&e.skipped.to_le_bytes());
        }
        put_len(&mut o, self.nodes.len());
        for (id, n) in &self.nodes {
            o.extend_from_slice(&id.to_le_bytes());
            o.push(u8::from(n.on));
            o.extend_from_slice(&n.value.to_le_bytes());
        }
        put_len(&mut o, self.switches.len());
        for (id, on) in &self.switches {
            o.extend_from_slice(&id.to_le_bytes());
            o.push(u8::from(*on));
        }
        put_len(&mut o, self.volumes.len());
        for (id, inside) in &self.volumes {
            if inside.len() > MAX_OCCUPANTS {
                return Err(Error::Occupants(*id));
            }
            o.extend_from_slice(&id.to_le_bytes());
            put_len(&mut o, inside.len());
            for object in inside {
                o.extend_from_slice(&object.to_le_bytes());
            }
        }
        if o.len() > MAX_BYTES {
            return Err(Error::TooLarge(o.len()));
        }
        Ok(o)
    }

    /// The record the bytes hold; a byte string `encode` would not write is refused.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge(bytes.len()));
        }
        let mut r = Reader(bytes);
        let mut x = ForgeExt {
            seed: r.u64()?,
            ..Self::default()
        };
        let mut last = None;
        for _ in 0..r.u32()? {
            let id = r.id(&mut last, "emitters")?;
            x.emitters.insert(
                id,
                EmitterState {
                    fired: r.u32()?,
                    skipped: r.u32()?,
                },
            );
        }
        last = None;
        for _ in 0..r.u32()? {
            let id = r.id(&mut last, "nodes")?;
            x.nodes.insert(
                id,
                NodeState {
                    on: r.flag()?,
                    value: r.u32()? as i32,
                },
            );
        }
        last = None;
        for _ in 0..r.u32()? {
            let id = r.id(&mut last, "switches")?;
            x.switches.insert(id, r.flag()?);
        }
        last = None;
        for _ in 0..r.u32()? {
            let id = r.id(&mut last, "volumes")?;
            let n = r.u32()? as usize;
            if n > MAX_OCCUPANTS {
                return Err(Error::Occupants(id));
            }
            let mut inside = Vec::with_capacity(n);
            for _ in 0..n {
                let object = r.u32()?;
                if inside.last().is_some_and(|p| *p >= object) {
                    return Err(Error::Order("occupants"));
                }
                inside.push(object);
            }
            x.volumes.insert(id, inside);
        }
        if !r.0.is_empty() {
            return Err(Error::Trailing);
        }
        Ok(x)
    }

    /// Record a volume's occupants this tick and say who entered and who left.
    /// `now` need not be sorted or free of repeats.
    pub fn occupy(&mut self, volume: u64, now: &[u32]) -> Crossing {
        let mut next: Vec<u32> = now.to_vec();
        next.sort_unstable();
        next.dedup();
        next.truncate(MAX_OCCUPANTS);
        let before = self.volumes.remove(&volume).unwrap_or_default();
        let entered = next.iter().copied().filter(|o| before.binary_search(o).is_err()).collect();
        let left = before.iter().copied().filter(|o| next.binary_search(o).is_err()).collect();
        if !next.is_empty() {
            self.volumes.insert(volume, next);
        }
        Crossing { entered, left }
    }
}

/// Who crossed a volume's boundary in one tick, ascending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Crossing {
    pub entered: Vec<u32>,
    pub left: Vec<u32>,
}

/// An emitter's schedule, from the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emitter {
    /// Ticks between bursts; 0 never fires.
    pub period: u32,
    /// The first burst's tick.
    pub phase: u32,
    /// Objects a burst makes.
    pub burst: u16,
    /// Bursts to make in all; 0 is no limit.
    pub total: u32,
    /// Live objects it may have made at once; 0 is no limit.
    pub alive_limit: u16,
}

/// What an emitter does on one tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shot {
    /// Not a scheduled tick, the emitter is disabled, or its bursts are used up.
    None,
    /// Make this many objects.
    Fire(u16),
    /// A scheduled burst the live-object limit refused: skip it and warn.
    Skip,
}

impl Emitter {
    /// What happens on `tick` given the record's count, the enable input and how
    /// many of its objects are alive. It reads the tick, never a wall clock.
    pub fn shot(&self, state: &EmitterState, tick: u32, enabled: bool, alive: u32) -> Shot {
        if !enabled || self.period == 0 || self.burst == 0 || tick < self.phase {
            return Shot::None;
        }
        if (tick - self.phase) % self.period != 0 {
            return Shot::None;
        }
        if self.total != 0 && state.fired >= self.total {
            return Shot::None;
        }
        if self.alive_limit != 0 && alive + u32::from(self.burst) > u32::from(self.alive_limit) {
            return Shot::Skip;
        }
        Shot::Fire(self.burst)
    }

    /// Count a shot into the record.
    pub fn record(state: &mut EmitterState, shot: Shot) {
        match shot {
            Shot::Fire(_) => state.fired += 1,
            Shot::Skip => state.skipped += 1,
            Shot::None => {}
        }
    }
}

fn put_len(o: &mut Vec<u8>, n: usize) {
    o.extend_from_slice(&(n as u32).to_le_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let (head, rest) = self.0.split_first_chunk::<N>().ok_or(Error::Truncated)?;
        self.0 = rest;
        Ok(*head)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        self.take().map(u32::from_le_bytes)
    }
    fn u64(&mut self) -> Result<u64, Error> {
        self.take().map(u64::from_le_bytes)
    }
    fn flag(&mut self) -> Result<bool, Error> {
        match self.take::<1>()?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Flag),
        }
    }
    /// The next id, which must be above the last.
    fn id(&mut self, last: &mut Option<u64>, list: &'static str) -> Result<u64, Error> {
        let id = self.u64()?;
        if last.is_some_and(|p| p >= id) {
            return Err(Error::Order(list));
        }
        *last = Some(id);
        Ok(id)
    }
}
