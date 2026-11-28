//! The guest side of `sim_anim::model_markers`: `b8d30`'s marker lookup
//! with the group scan `1d8f00` and the transform fill `1d8f50` as one
//! gather, one kernel call and one scatter (docs/engine/markers.md;
//! phase 1c of docs/engine/sim-rewrite.md).
//!
//! The register-level port (`sim_native::marker`) stays the vehicle: it
//! takes the entry, resolves the parent chain in guest code and reads the
//! model. At the point it would call `1d8f50` — the render model's tag
//! handle in `ecx`, the name in `edi`, the node array in `esi`, the node
//! matrices in `ebp`, the output in `ebx`, the mirror flag at
//! `[esp + 0xc]` and the capacity at `[esp + 0x20]` — [`run`] gathers the
//! group table, the instances, the name-filter window and the matrices,
//! lets the kernel build every record in Rust, and writes what `1d8f50`
//! writes: the `0x70`-byte records and the count in `eax`. The stack is
//! left as the guest call leaves it (the six pushed arguments and the
//! callee's frame are dead bytes below `esp`), the execution continues at
//! `b8ded`, and everything the family writes below the arguments stays
//! unwritten: nothing outside it reads those bytes.
//!
//! It declines before writing anything, and the port panics with
//! `anim marker: <reason>` (no guest call runs) when MXCSR is not masked, nearest and PE-sticky, when the kernel
//! leaves the fast path, when the model has more groups than the port's
//! scan bound without a match, when the group has more instances than
//! the kernel gathers, when a read faults, or when the output overlaps a
//! range it reads.
use crate::function_eval::constant::{ONE, SMALL};
use crate::points::TAG_ENTRIES;
use crate::sim_native::Memory;
use ::sim_anim::markers::{self, Constants, Marker};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_core::det;
use sim_core::math::NodeMatrix;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Where `b8d30` continues after `1d8f50` returns.
pub const AFTER_MARKERS: u32 = 0x000b_8ded;
/// The port's group-scan bound: past it the lifted loop resumes.
pub const GROUPS_MAX: usize = 256;
/// The largest instance count the kernel gathers.
pub const INSTANCES_MAX: usize = 4096;
/// The dividend the basis leaf divides by the squared norm `[0x45dc0c]`.
const TWO: u32 = 0x0045_dc0c;
/// The render model's marker groups: count `+0x58`, array `+0x5c`; each
/// group is a name, an instance count and an instance pointer.
const GROUP_COUNT: u32 = 0x58;
const GROUPS: u32 = 0x5c;
const GROUP_BYTES: u32 = 12;
/// The slots `b8d30`'s frame holds at the call.
const MIRROR_SLOT: u32 = 0xc;
const CAPACITY_SLOT: u32 = 0x20;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// On by default: the kernel runs wherever the marker port runs. Off
/// leaves the register-level path, the tests' reference.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Why the kernel declined the call; the port panics with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR not masked, nearest and PE-sticky, or an operand off the
    /// fast path.
    Float,
    /// A guest read faulted.
    Fault,
    /// The model has more groups than the scan bound and none matched.
    Scan,
    /// The group has more instances than the kernel gathers.
    Count,
    /// The output overlaps a range the call reads.
    Alias,
}

impl Decline {
    /// The reason as the panic names it.
    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Fault => "fault",
            Decline::Scan => "scan",
            Decline::Count => "count",
            Decline::Alias => "alias",
        }
    }
}

/// Call counts since start, for timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Calls the kernel filled.
    pub native: u64,
    /// Marker records it wrote.
    pub markers: u64,
}

static NATIVE: AtomicU64 = AtomicU64::new(0);
static WRITTEN: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> Stats {
    Stats {
        native: NATIVE.load(Ordering::Relaxed),
        markers: WRITTEN.load(Ordering::Relaxed),
    }
}

type Step<T> = Result<T, Decline>;

fn read<T>(r: Result<T, GuestFault>) -> Step<T> {
    r.map_err(|_| Decline::Fault)
}

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// What the scatter writes, decided before anything is written.
struct Plan {
    out: u32,
    records: Vec<Marker>,
}

fn plan(c: &CpuAbi, g: &impl Memory) -> Step<Plan> {
    if !det::fast_env(c.mxcsr) {
        return Err(Decline::Float);
    }
    let sp = c.esp;
    // The registers `b8d30` holds at the call to `1d8f50`.
    let (handle, name) = (c.ecx, c.edi);
    let (nodes_at, matrices_at, out) = (c.esi, c.ebp, c.ebx);
    let mirror = read(g.u8(sp.wrapping_add(MIRROR_SLOT)))? != 0;
    let capacity = read(g.u16(sp.wrapping_add(CAPACITY_SLOT)))? as i16 as i32;
    let tags = read(g.u32(TAG_ENTRIES.base))?;
    let k = Constants {
        one: read(g.u32(ONE))?,
        small: read(g.u32(SMALL))?,
        two: read(g.u32(TWO))?,
    };

    // `1d8f00`'s guards and scan.
    let mut sel = Vec::new();
    let mut instances = Vec::new();
    let mut read_spans: Vec<(u32, u32)> = Vec::new();
    if let Some(render) = group_table(g, tags, handle, name)? {
        let (count, array) = render;
        let scan = (count as usize).min(GROUPS_MAX);
        let mut names = vec![0u32; scan];
        for (n, i) in names.iter_mut().zip(0..scan) {
            *n = read(g.u32(array.wrapping_add(GROUP_BYTES * i as u32)))?;
        }
        read_spans.push((array, array.wrapping_add(GROUP_BYTES * scan as u32)));
        let Some(index) = markers::find_group(&names, name) else {
            if count as usize > GROUPS_MAX {
                return Err(Decline::Scan);
            }
            return Ok(Plan {
                out,
                records: Vec::new(),
            });
        };
        let group = array.wrapping_add(GROUP_BYTES * index as u32);
        let count = read(g.u32(group.wrapping_add(4)))? as i32;
        let at = read(g.u32(group.wrapping_add(8)))?;
        if count > INSTANCES_MAX as i32 {
            return Err(Decline::Count);
        }
        if count > 0 {
            instances = vec![0u8; count as usize * markers::INSTANCE_BYTES];
            read(g.read_bytes(at, &mut instances, 1))?;
            read_spans.push((at, at.wrapping_add(instances.len() as u32)));
            // The name filter reads a signed byte off the node array, so
            // the window covers `base - 128` to `base + 128`.
            let window_at = nodes_at.wrapping_sub(128);
            let mut window = [0u8; 256];
            read(g.read_bytes(window_at, &mut window, 1))?;
            read_spans.push((window_at, window_at.wrapping_add(256)));
            sel = markers::select(&instances, Some(&window), capacity);
        }
    }

    // The matrices the composed nodes read: gathered to the largest node
    // the selection names.
    let matrices = if sel.is_empty() {
        Vec::new()
    } else {
        let last = sel.iter().map(|s| u32::from(s.node)).max().unwrap();
        let mut bytes = vec![0u8; 0x34 * (last as usize + 1)];
        read(g.read_bytes(matrices_at, &mut bytes, 1))?;
        read_spans.push((matrices_at, matrices_at.wrapping_add(bytes.len() as u32)));
        bytes
            .chunks_exact(0x34)
            .map(|m| NodeMatrix::from_bytes(m.try_into().unwrap()))
            .collect::<Vec<_>>()
    };

    // The output must not overlap anything the call reads: the guest
    // interleaves its writes with those reads.
    let writes = (
        out,
        out.wrapping_add(markers::MARKER_BYTES as u32 * sel.len() as u32),
    );
    if writes.1 < writes.0 || read_spans.iter().any(|&r| overlaps(writes, r)) {
        return Err(Decline::Alias);
    }
    let records = markers::model_markers(&k, &instances, &sel, &matrices, mirror)
        .map_err(|_| Decline::Float)?;
    Ok(Plan { out, records })
}

/// The render model's group table, or `None` when `1d8f00` would not
/// scan: a `-1` handle or name, a zero name, a `-1` table entry or no
/// groups.
fn group_table(g: &impl Memory, tags: u32, handle: u32, name: u32) -> Step<Option<(u32, u32)>> {
    if handle == u32::MAX || name == u32::MAX || name == 0 {
        return Ok(None);
    }
    let render = read(g.u32(tags.wrapping_add((handle & 0xffff) * 16 + 8)))?;
    if render == u32::MAX {
        return Ok(None);
    }
    let count = read(g.u32(render.wrapping_add(GROUP_COUNT)))? as i32;
    if count <= 0 {
        return Ok(None);
    }
    let array = read(g.u32(render.wrapping_add(GROUPS)))?;
    Ok(Some((count as u32, array)))
}

fn scatter(c: &mut CpuAbi, g: &mut impl Memory, p: &Plan) -> Result<TransferAbi, GuestFault> {
    for (n, m) in p.records.iter().enumerate() {
        g.write_bytes(
            p.out.wrapping_add(markers::MARKER_BYTES as u32 * n as u32),
            &m.to_bytes(),
            1,
        )?;
    }
    c.eax = p.records.len() as u32;
    NATIVE.fetch_add(1, Ordering::Relaxed);
    WRITTEN.fetch_add(p.records.len() as u64, Ordering::Relaxed);
    Ok(TransferAbi::resume(AFTER_MARKERS))
}

/// The marker fill, entered where the port would call `1d8f50`.
/// `Ok(None)`: the kernel is switched off; the port makes the guest call.
/// A decline panics `anim marker: <reason>`.
pub fn run(c: &mut CpuAbi, g: &mut impl Memory) -> Result<Option<TransferAbi>, GuestFault> {
    if !enabled() {
        return Ok(None);
    }
    match fill(c, g)? {
        Ok(transfer) => Ok(Some(transfer)),
        Err(why) => unimplemented!("anim marker: {}", why.name()),
    }
}

/// [`run`] without the switch: the gather, the kernel call and the
/// scatter. `Ok(Err(why))`: declined, nothing written.
pub fn fill(c: &mut CpuAbi, g: &mut impl Memory) -> Result<Result<TransferAbi, Decline>, GuestFault> {
    match plan(c, g) {
        Ok(plan) => scatter(c, g, &plan).map(Ok),
        Err(why) => Ok(Err(why)),
    }
}

#[cfg(test)]
mod tests;
