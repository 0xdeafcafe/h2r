//! Native simulation leaves. The installer uses the original for unmasked
//! SSE exceptions so the guest can deliver them at the faulting instruction.
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

pub mod bsp;
pub mod codec;
pub mod collision;
pub mod evaluator;
pub mod graph;
mod lanes;
pub mod marker;
pub mod mopp_query;
pub mod object;
pub mod object_base;
pub(crate) mod plan;
pub mod pose;
pub mod root;
pub(crate) mod ray;
pub mod stream;
pub mod unit;

pub use collision::{geometry_test, point_to_leaf};
pub use plan::{Decline, SegmentMemory, take_decline};
#[cfg(any(test, feature = "differential"))]
pub use lanes::with as with_simd;
pub use lanes::{enabled as simd_enabled, set_enabled as set_simd};
pub use pose::{compose, pose_matrix, rotation_codec};

pub type Result = std::result::Result<TransferAbi, GuestFault>;

pub fn supported(cpu: &CpuAbi) -> bool {
    cpu.mxcsr & 0x1f80 == 0x1f80
        && cpu.x87.control & 0x3f == 0x3f
        && cpu.x87.status & 0x80 == 0
        && cpu.x87.control & 0x300 != 0x100
}

type Vec4 = [u32; 4];

fn unpack(bits: u128) -> Vec4 {
    std::array::from_fn(|i| (bits >> (32 * i)) as u32)
}

fn pack(v: Vec4) -> u128 {
    v.into_iter()
        .rev()
        .fold(0, |bits, x| (bits << 32) | u128::from(x))
}

fn xmm(c: &CpuAbi, i: usize) -> Vec4 {
    unpack(u128::from_le_bytes(c.xmm[i]))
}

fn put_xmm(c: &mut CpuAbi, i: usize, v: Vec4) {
    c.xmm[i] = pack(v).to_le_bytes();
}

fn scalar(x: u32) -> Vec4 {
    [x, 0, 0, 0]
}

// After PE becomes sticky, operations on ordinary finite values with
// ordinary finite results cannot change MXCSR. Restrict the fast path to
// magnitudes with headroom on either side of the normal range so even a
// host that flushes subnormals never changes the answer. Rust's separate
// binary32 operations round once each. All other cases use the SSE model.
// The rule and the operations are `sim_core::lanes`'.
use Op::*;
use sim_core::lanes::{Op, binary1 as binary32, ordinary1 as ordinary};

fn op(c: &mut CpuAbi, op: Op, a: u32, b: u32) -> u32 {
    if c.mxcsr & 0x6020 == 0x20 && ordinary(a) && ordinary(b) {
        return binary32(op, a, b);
    }
    match op {
        Add => simd::addss(&mut c.mxcsr, u128::from(a), b),
        Sub => simd::subss(&mut c.mxcsr, u128::from(a), b),
        Mul => simd::mulss(&mut c.mxcsr, u128::from(a), b),
    }
    .expect("masked SSE exceptions") as u32
}

// Packed SSE. The fast path runs on host vector lanes unless
// `impl.sim.native_simd` is off; see `lanes` for why the bits match.
fn vector(c: &mut CpuAbi, kind: Op, a: Vec4, b: Vec4) -> Vec4 {
    if c.mxcsr & 0x6020 == 0x20 {
        if lanes::enabled() {
            if lanes::ordinary(a, b) {
                return lanes::binary32(kind, a, b);
            }
        } else if lanes::scalar::ordinary(a, b) {
            return lanes::scalar::binary32(kind, a, b);
        }
    }
    let (a, b) = (pack(a), pack(b));
    unpack(
        match kind {
            Add => simd::addps(&mut c.mxcsr, a, b),
            Sub => simd::subps(&mut c.mxcsr, a, b),
            Mul => simd::mulps(&mut c.mxcsr, a, b),
        }
        .expect("masked SSE exceptions"),
    )
}

const STATUS: u32 = 0x8d5;
fn flags(c: &mut CpuAbi, value: u32, valid: u32) {
    c.eflags = (c.eflags & !STATUS) | value;
    c.eflags_defined = (c.eflags_defined & !STATUS) | valid;
}

fn compare(c: &mut CpuAbi, a: u32, b: u32) -> u32 {
    let f = simd::comiss(&mut c.mxcsr, a, b).expect("masked SSE exceptions");
    flags(c, f, STATUS);
    f
}

fn szp(v: u32) -> u32 {
    ((v >> 31) << 7) | (u32::from(v == 0) << 6) | (u32::from((v as u8).count_ones() % 2 == 0) << 2)
}

pub(super) fn logic_flags(c: &mut CpuAbi, v: u32) {
    flags(c, szp(v), STATUS & !0x10);
}

pub(super) fn add_flags(c: &mut CpuAbi, a: u32, b: u32) {
    let (r, carry) = a.overflowing_add(b);
    let overflow = (!(a ^ b) & (a ^ r)) >> 31;
    flags(
        c,
        szp(r) | u32::from(carry) | ((a ^ b ^ r) & 0x10) | (overflow << 11),
        STATUS,
    );
}

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn push(c: &mut CpuAbi, g: &mut impl Memory, value: u32) -> std::result::Result<(), GuestFault> {
    let addr = c.esp.wrapping_sub(4);
    g.set_u32(addr, value)?;
    c.esp = addr;
    Ok(())
}

fn pop(c: &mut CpuAbi, g: &impl Memory) -> std::result::Result<u32, GuestFault> {
    let value = g.u32(c.esp)?;
    c.esp = at(c.esp, 4);
    Ok(value)
}

/// The runtime uses its checked guest bus. The differential harness records
/// each store here, including temporary stores that later get overwritten.
pub trait Memory {
    fn read_bytes(
        &self,
        addr: u32,
        dst: &mut [u8],
        alignment: u32,
    ) -> std::result::Result<(), GuestFault>;
    fn write_bytes(
        &mut self,
        addr: u32,
        src: &[u8],
        alignment: u32,
    ) -> std::result::Result<(), GuestFault>;
    /// Start of a physically contiguous readable span. Flat memory providers
    /// have identity addresses. A mapped provider must translate aliases.
    fn physical_read(&self, addr: u32, len: usize) -> Option<u32> {
        (u64::from(addr).checked_add(len as u64)? <= 1 << 32).then_some(addr)
    }
    fn call(&mut self, c: &mut CpuAbi, target: u32, site: u32) -> Result
    where
        Self: Sized,
    {
        call(c, self, target, site)
    }
    fn ret(&self, c: &mut CpuAbi, pop: u32) -> Result
    where
        Self: Sized,
    {
        ret(self, c, pop)
    }
    fn load<const N: usize>(&self, addr: u32) -> std::result::Result<[u8; N], GuestFault> {
        let mut bytes = [0; N];
        self.read_bytes(addr, &mut bytes, 1)?;
        Ok(bytes)
    }
    fn u8(&self, a: u32) -> std::result::Result<u8, GuestFault> {
        Ok(self.load::<1>(a)?[0])
    }
    fn u16(&self, a: u32) -> std::result::Result<u16, GuestFault> {
        Ok(u16::from_le_bytes(self.load(a)?))
    }
    fn u32(&self, a: u32) -> std::result::Result<u32, GuestFault> {
        Ok(u32::from_le_bytes(self.load(a)?))
    }
    fn u64(&self, a: u32) -> std::result::Result<u64, GuestFault> {
        Ok(u64::from_le_bytes(self.load(a)?))
    }
    fn u128(&self, a: u32) -> std::result::Result<u128, GuestFault> {
        Ok(u128::from_le_bytes(self.load(a)?))
    }
    fn set_u8(&mut self, a: u32, v: u8) -> std::result::Result<(), GuestFault> {
        self.write_bytes(a, &[v], 1)
    }
    fn set_u16(&mut self, a: u32, v: u16) -> std::result::Result<(), GuestFault> {
        self.write_bytes(a, &v.to_le_bytes(), 1)
    }
    fn set_u32(&mut self, a: u32, v: u32) -> std::result::Result<(), GuestFault> {
        self.write_bytes(a, &v.to_le_bytes(), 1)
    }
    fn set_u64(&mut self, a: u32, v: u64) -> std::result::Result<(), GuestFault> {
        self.write_bytes(a, &v.to_le_bytes(), 1)
    }
}

impl Memory for Guest {
    fn physical_read(&self, addr: u32, len: usize) -> Option<u32> {
        plan::SegmentMemory::checked_offset(self, addr, len, runtime::guest::Access::Read, 1)
    }

    fn read_bytes(
        &self,
        a: u32,
        dst: &mut [u8],
        alignment: u32,
    ) -> std::result::Result<(), GuestFault> {
        Guest::read_bytes(self, a, dst, alignment)
    }
    fn write_bytes(
        &mut self,
        a: u32,
        src: &[u8],
        alignment: u32,
    ) -> std::result::Result<(), GuestFault> {
        Guest::write_bytes(self, a, src, alignment)
    }
}

fn ret(g: &impl Memory, c: &mut CpuAbi, cleanup: u32) -> Result {
    Ok(TransferAbi::ret({
        let target = pop(c, g)?;
        c.esp = at(c.esp, cleanup);
        target
    }))
}

pub(crate) fn sub_flags(c: &mut CpuAbi, a: u32, b: u32, width: u32) {
    let mask = u32::MAX >> (32 - width);
    let (a, b) = (a & mask, b & mask);
    let r = a.wrapping_sub(b) & mask;
    let sign = 1 << (width - 1);
    let f = (szp(r) & !0x80)
        | (u32::from(r & sign != 0) << 7)
        | u32::from(a < b)
        | ((a ^ b ^ r) & 0x10)
        | (u32::from((a ^ b) & (a ^ r) & sign != 0) << 11);
    flags(c, f, STATUS);
}

fn call(c: &mut CpuAbi, g: &mut impl Memory, target: u32, resume: u32) -> Result {
    push(c, g, resume)?;
    Ok(TransferAbi::call(target, resume))
}

fn low(reg: u32, value: u8) -> u32 {
    (reg & !0xff) | u32::from(value)
}
fn object_slot(handle: u32) -> u32 {
    (handle & 0xffff) * 12
}
fn tag_data(g: &impl Memory, table: u32, handle: u32) -> std::result::Result<u32, GuestFault> {
    g.u32(at(table, (handle & 0xffff) * 16 + 8))
}

fn test_byte(c: &mut CpuAbi, v: u8) {
    flags(c, szp(u32::from(v)) | u32::from(v & 0x80), STATUS & !0x10);
}
