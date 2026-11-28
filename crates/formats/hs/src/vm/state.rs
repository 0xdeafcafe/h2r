//! The VM's persistent state and its canonical bytes: what a world save
//! holds for HaloScript (docs/engine/sim-world.md, scripts section).
//!
//! Little-endian, no padding. Threads are written in slot order with their
//! frames; the pool's allocation cursor and next salt are part of the
//! state because they decide the next thread's slot and handle.
use super::thread::{Category, Dest, Frame, Pool, THREAD_CAPACITY, Thread};
use super::value::Cell;
use crate::Datum;

/// What [`super::Vm::export_state`] returns and `import_state` takes.
#[derive(Debug, Clone)]
pub struct VmState {
    pub(crate) pool: Pool,
    pub(crate) globals: Vec<Cell>,
    pub(crate) enabled: bool,
    pub(crate) host_strings: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateError {
    Truncated,
    Trailing,
    Corrupt(&'static str),
    /// A state for a scenario with another global count.
    Globals { found: usize, expected: usize },
}
impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => f.write_str("script state ends inside a field"),
            Self::Trailing => f.write_str("script state has trailing bytes"),
            Self::Corrupt(what) => write!(f, "script state: {what}"),
            Self::Globals { found, expected } => {
                write!(f, "script state has {found} globals, the scenario has {expected}")
            }
        }
    }
}
impl std::error::Error for StateError {}

struct Cur<'a>(&'a [u8]);
impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], StateError> {
        if self.0.len() < n {
            return Err(StateError::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, StateError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, StateError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, StateError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    /// A count bounded by the bytes left, so a corrupt length cannot
    /// allocate more than the input holds.
    fn count(&mut self, min_bytes_each: usize) -> Result<usize, StateError> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min_bytes_each) > self.0.len() {
            return Err(StateError::Truncated);
        }
        Ok(n)
    }
}

fn category_byte(c: Category) -> u8 {
    match c {
        Category::Script => 0,
        Category::GlobalInit => 1,
        Category::Transient => 2,
        Category::External => 3,
    }
}
fn category_from(b: u8) -> Result<Category, StateError> {
    Ok(match b {
        0 => Category::Script,
        1 => Category::GlobalInit,
        2 => Category::Transient,
        3 => Category::External,
        _ => return Err(StateError::Corrupt("thread category")),
    })
}

impl VmState {
    /// The canonical bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        let u8_ = |o: &mut Vec<u8>, v: u8| o.push(v);
        let u16_ = |o: &mut Vec<u8>, v: u16| o.extend_from_slice(&v.to_le_bytes());
        let u32_ = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
        u8_(&mut o, self.enabled as u8);
        u32_(&mut o, self.pool.free_scan_start as u32);
        u16_(&mut o, self.pool.next_salt);
        u32_(&mut o, self.pool.slots.len() as u32);
        for slot in &self.pool.slots {
            let Some(t) = slot else {
                u8_(&mut o, 0);
                continue;
            };
            u8_(&mut o, 1);
            u16_(&mut o, t.salt);
            u8_(&mut o, category_byte(t.category));
            u8_(&mut o, t.flags);
            match t.script {
                Some(s) => {
                    u8_(&mut o, 1);
                    u16_(&mut o, s);
                }
                None => {
                    u8_(&mut o, 0);
                    u16_(&mut o, 0);
                }
            }
            u32_(&mut o, t.wake as u32);
            u32_(&mut o, t.saved_wake as u32);
            u32_(&mut o, t.frames.len() as u32);
            for f in &t.frames {
                u32_(&mut o, f.expr.0);
                u16_(&mut o, f.bytes);
                match f.pending {
                    Dest::Local(i) => {
                        u8_(&mut o, 0);
                        u32_(&mut o, i as u32);
                    }
                    Dest::Global(i) => {
                        u8_(&mut o, 1);
                        u32_(&mut o, i as u32);
                    }
                }
                u32_(&mut o, f.locals.len() as u32);
                for c in &f.locals {
                    u32_(&mut o, c.0);
                }
            }
        }
        u32_(&mut o, self.globals.len() as u32);
        for c in &self.globals {
            u32_(&mut o, c.0);
        }
        u32_(&mut o, self.host_strings.len() as u32);
        for s in &self.host_strings {
            u32_(&mut o, s.len() as u32);
            o.extend_from_slice(s);
        }
        o
    }

    /// A state from [`VmState::encode`]'s bytes; anything else is an error.
    pub fn decode(bytes: &[u8]) -> Result<Self, StateError> {
        let mut c = Cur(bytes);
        let enabled = match c.u8()? {
            0 => false,
            1 => true,
            _ => return Err(StateError::Corrupt("enabled byte")),
        };
        let free_scan_start = c.u32()? as usize;
        let next_salt = c.u16()?;
        let slot_count = c.count(1)?;
        if slot_count > THREAD_CAPACITY || free_scan_start > THREAD_CAPACITY {
            return Err(StateError::Corrupt("thread pool size"));
        }
        let mut slots = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            if c.u8()? == 0 {
                slots.push(None);
                continue;
            }
            let salt = c.u16()?;
            let category = category_from(c.u8()?)?;
            let flags = c.u8()?;
            let has_script = c.u8()?;
            let script_index = c.u16()?;
            let script = match has_script {
                0 => None,
                1 => Some(script_index),
                _ => return Err(StateError::Corrupt("script flag")),
            };
            let wake = c.u32()? as i32;
            let saved_wake = c.u32()? as i32;
            let frame_count = c.count(11)?;
            if frame_count == 0 {
                return Err(StateError::Corrupt("thread without a root frame"));
            }
            let mut frames = Vec::with_capacity(frame_count);
            for _ in 0..frame_count {
                let expr = Datum(c.u32()?);
                let bytes = c.u16()?;
                let tag = c.u8()?;
                let at = c.u32()? as usize;
                let pending = match tag {
                    0 => Dest::Local(at),
                    1 => Dest::Global(at),
                    _ => return Err(StateError::Corrupt("frame destination")),
                };
                let n = c.count(4)?;
                let mut locals = Vec::with_capacity(n);
                for _ in 0..n {
                    locals.push(Cell(c.u32()?));
                }
                let mut f = Frame::new(expr);
                f.bytes = bytes;
                f.pending = pending;
                f.locals = locals;
                frames.push(f);
            }
            slots.push(Some(Thread { salt, category, flags, script, wake, saved_wake, frames }));
        }
        let n = c.count(4)?;
        let mut globals = Vec::with_capacity(n);
        for _ in 0..n {
            globals.push(Cell(c.u32()?));
        }
        let n = c.count(4)?;
        let mut host_strings = Vec::with_capacity(n);
        for _ in 0..n {
            let len = c.count(1)?;
            host_strings.push(c.take(len)?.to_vec());
        }
        if !c.0.is_empty() {
            return Err(StateError::Trailing);
        }
        Ok(Self {
            pool: Pool { slots, free_scan_start, next_salt },
            globals,
            enabled,
            host_strings,
        })
    }

    /// Occupied thread slots.
    pub fn thread_count(&self) -> usize {
        self.pool.slots.iter().flatten().count()
    }
    pub fn globals(&self) -> &[Cell] {
        &self.globals
    }
}
