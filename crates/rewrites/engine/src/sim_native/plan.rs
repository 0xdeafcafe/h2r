//! Buffer one animation segment's stores until every input and destination is
//! known. Later loads see earlier stores, including through physical aliases.
use super::Memory;
use std::mem::MaybeUninit;
use runtime::{
    Guest, GuestFault,
    guest::{Access, FaultReason},
};

/// Memory held exclusively throughout a segment. A successful write check
/// guarantees the same write will succeed at commit. Loads must be plain RAM.
pub trait SegmentMemory: Memory {
    /// Physical start of a contiguous range with the requested access.
    fn checked_offset(&self, addr: u32, len: usize, access: Access, alignment: u32) -> Option<u32>;
    /// An access trap is installed: `checked_offset` answers nothing.
    fn trapped(&self) -> bool {
        false
    }
}

impl SegmentMemory for Guest {
    fn trapped(&self) -> bool {
        self.has_trap()
    }
    fn checked_offset(&self, addr: u32, len: usize, access: Access, alignment: u32) -> Option<u32> {
        if self.has_trap() {
            return None;
        }
        // One page of the fast page table answers most accesses with one
        // lookup; it gives the physical offset the walk below would.
        if let Some(physical) = self.fast_physical(addr, len, access, alignment) {
            return Some(physical);
        }
        self.check(addr, len, access, alignment).ok()?;
        let maps = self.mappings();
        let i = maps
            .partition_point(|m| m.virtual_start <= addr)
            .checked_sub(1)?;
        let mapping = &maps[i];
        let offset = addr - mapping.virtual_start;
        // Leave accesses straddling virtual mappings to the lifted instruction.
        (u64::from(offset) + len as u64 <= u64::from(mapping.len))
            .then(|| mapping.physical_start + offset)
    }
}

const PAGE: u32 = 0x1000;

/// Why a segment declined, kept for the panic that names it
/// ([`take_decline`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// The float environment is not the ports'.
    Float,
    /// `pc` is neither the body's entry nor one of its sites.
    Boundary,
    /// An access trap is installed; no access is answered.
    Trap,
    /// The body's text is unreadable, or lies in more than [`SPANS`]
    /// physical pieces.
    Text { start: u32, end: u32 },
    /// A load from memory the plan cannot read.
    Read { addr: u32, len: u32 },
    /// A store to memory the plan cannot write.
    Write { addr: u32, len: u32 },
    /// A store into the body's own text, or its inlined leaf's.
    TextWrite { addr: u32, len: u32 },
    /// The store log is full and the body allows no spill.
    LogFull { addr: u32 },
    /// The body's own fault (an operation it does not model).
    Body { addr: u32, len: u32, access: Access, reason: FaultReason },
}

impl std::fmt::Display for Decline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Float => write!(f, "the float environment is not the ports'"),
            Self::Boundary => write!(f, "not the entry or a site"),
            Self::Trap => write!(f, "an access trap is installed"),
            Self::Text { start, end } => write!(f, "text {start:#x}..{end:#x} is not readable in {SPANS} physical spans"),
            Self::Read { addr, len } => write!(f, "read of {len} bytes at {addr:#010x}"),
            Self::Write { addr, len } => write!(f, "write of {len} bytes at {addr:#010x}"),
            Self::TextWrite { addr, len } => write!(f, "write of {len} bytes at {addr:#010x} into its own text"),
            Self::LogFull { addr } => write!(f, "store log full at a write to {addr:#010x}"),
            Self::Body { addr, len, access, reason } => {
                write!(f, "{access:?} of {len} bytes at {addr:#010x}: {reason:?}")
            }
        }
    }
}

thread_local! {
    static LAST_DECLINE: std::cell::Cell<Option<(u32, Decline)>> = const { std::cell::Cell::new(None) };
}

fn note_decline(pc: u32, why: Decline) {
    LAST_DECLINE.with(|last| last.set(Some((pc, why))));
}

/// The last segment this thread declined: its `pc` and why. Taken once.
pub fn take_decline() -> Option<(u32, Decline)> {
    LAST_DECLINE.with(std::cell::Cell::take)
}

/// Physical pieces of a body's text, merged where contiguous.
pub const SPANS: usize = 6;

/// The XBE loader maps every image page on its own
/// (`runtime::loader::load_xbe_with`), so text crossing a page boundary
/// lies in several mappings. Each page is looked up alone.
#[derive(Clone, Copy, Default)]
struct Spans {
    at: [(u32, u32); SPANS],
    n: u8,
}

impl Spans {
    fn of<M: SegmentMemory>(ram: &M, start: u32, end: u32) -> Option<Self> {
        let mut spans = Self::default();
        let mut addr = start;
        while addr < end {
            let next = (addr & !(PAGE - 1)).checked_add(PAGE).map_or(end, |e| e.min(end));
            let len = next - addr;
            let lo = ram.checked_offset(addr, len as usize, Access::Read, 1)?;
            spans.push(lo, lo.checked_add(len)?)?;
            addr = next;
        }
        Some(spans)
    }

    fn push(&mut self, lo: u32, hi: u32) -> Option<()> {
        if let Some(last) = usize::from(self.n).checked_sub(1).map(|i| &mut self.at[i])
            && last.1 == lo
        {
            last.1 = hi;
            return Some(());
        }
        *self.at.get_mut(usize::from(self.n))? = (lo, hi);
        self.n += 1;
        Some(())
    }

    fn overlaps(&self, lo: u64, hi: u64) -> bool {
        self.at[..usize::from(self.n)]
            .iter()
            .any(|&(s, e)| lo < u64::from(e) && u64::from(s) < hi)
    }
}

/// `[addr, addr + len)` crosses a page boundary.
fn crosses_page(addr: u32, len: usize) -> bool {
    len > 1 && u64::from(addr & (PAGE - 1)) + len as u64 > u64::from(PAGE)
}

/// Lengths of the pieces of `[addr, addr + len)` that stay within a page.
fn page_pieces(addr: u32, len: usize) -> impl Iterator<Item = (u32, usize)> {
    let mut at = (addr, len);
    std::iter::from_fn(move || {
        let (addr, left) = at;
        (left != 0).then(|| {
            let room = (PAGE - (addr & (PAGE - 1))) as usize;
            let take = room.min(left);
            at = (addr.wrapping_add(take as u32), left - take);
            (addr, take)
        })
    })
}

// Common segments use only the inline log. A larger node or event copy
// spills and indexes its stores by physical address for forwarding.
#[derive(Clone, Copy, Default)]
pub(crate) struct Store {
    addr: u32,
    physical: u32,
    bytes: [u8; 4],
    len: u8,
    alignment: u32,
}

/// The inline store log, held by the caller of a segment for as long as
/// its [`Plan`] borrows it. Its slots are written before they are read
/// (`Plan::count` of them), so it is never filled: a segment makes no copy
/// of the log, neither to set it up nor to move the plan.
pub(crate) struct Log<const N: usize>([MaybeUninit<Store>; N]);

impl<const N: usize> Log<N> {
    pub(crate) fn new() -> Self {
        Self([const { MaybeUninit::uninit() }; N])
    }

    /// The first `count` stores, each written by `Plan::write_bytes`.
    fn written(&self, count: usize) -> &[Store] {
        let written = &self.0[..count.min(N)];
        // SAFETY: `Plan` writes slot `i` before it counts it (`count` only
        // grows past `i` after `self.0[i]` is written), and `Store` is
        // plain `Copy` data, so the first `count` slots are initialised.
        unsafe { std::slice::from_raw_parts(written.as_ptr().cast::<Store>(), written.len()) }
    }
}

pub(crate) struct Plan<'a, M, const N: usize> {
    text: Spans,
    inline_text: std::ops::Range<u64>,
    ram: &'a mut M,
    log: &'a mut Log<N>,
    count: usize,
    allow_spill: bool,
    spill: Vec<Store>,
    forwarded: Option<std::collections::HashMap<u32, [Option<u8>; 4]>>,
    /// The last access the plan declined, matched to the body's error.
    why: std::cell::Cell<Option<Decline>>,
}

impl<'a, M: SegmentMemory, const N: usize> Plan<'a, M, N> {
    fn new(ram: &'a mut M, log: &'a mut Log<N>, start: u32, end: u32) -> Result<Self, Decline> {
        if ram.trapped() {
            return Err(Decline::Trap);
        }
        let text = (start <= end)
            .then(|| Spans::of(&*ram, start, end))
            .flatten()
            .ok_or(Decline::Text { start, end })?;
        Ok(Self {
            ram,
            text,
            inline_text: 0..0,
            log,
            count: 0,
            allow_spill: false,
            spill: Vec::new(),
            forwarded: None,
            why: std::cell::Cell::new(None),
        })
    }

    pub(crate) fn for_segment(
        ram: &'a mut M,
        log: &'a mut Log<N>,
        cpu: &runtime::CpuAbi,
        pc: u32,
        text: std::ops::Range<u32>,
        sites: &[u32],
    ) -> Result<Self, Decline> {
        if !super::supported(cpu) {
            return Err(Decline::Float);
        }
        if pc != text.start && sites.binary_search(&pc).is_err() {
            return Err(Decline::Boundary);
        }
        Self::new(ram, log, text.start, text.end)
    }

    /// Record why the plan refuses `addr`, and the fault the body sees.
    fn decline(&self, addr: u32, len: usize, access: Access, why: Decline) -> GuestFault {
        self.why.set(Some(why));
        GuestFault {
            addr,
            len,
            access,
            reason: FaultReason::InvalidBuffer,
        }
    }

    /// Why the body's `fault` happened: the plan's refusal of that access,
    /// or the body's own fault.
    fn explain(&self, fault: &GuestFault) -> Decline {
        let refused = self.why.get().filter(|why| {
            let addr = match *why {
                Decline::Read { addr, .. }
                | Decline::Write { addr, .. }
                | Decline::TextWrite { addr, .. }
                | Decline::LogFull { addr } => addr,
                _ => return false,
            };
            fault.reason == FaultReason::InvalidBuffer && addr == fault.addr
        });
        refused.unwrap_or(Decline::Body {
            addr: fault.addr,
            len: fault.len as u32,
            access: fault.access,
            reason: fault.reason,
        })
    }

    /// The stores logged so far, oldest first; the spill follows them.
    fn logged(&self) -> &[Store] {
        self.log.written(self.count)
    }

    /// The value of each physical byte after the segment. Keep only the
    /// last write to an overlapping address, including through aliases.
    pub(crate) fn final_bytes(&self) -> Vec<(u32, u8)> {
        let mut bytes = Vec::new();
        for store in self.logged().iter().chain(&self.spill).rev() {
            for (i, &value) in store.bytes[..usize::from(store.len)].iter().enumerate() {
                bytes.push((store.physical + i as u32, value));
            }
        }
        // Stable sort keeps the newest byte first at each address.
        bytes.sort_by_key(|&(addr, _)| addr);
        bytes.dedup_by_key(|(addr, _)| *addr);
        bytes
    }

    /// Called before executing the segment. If the leaf has no contiguous
    /// instruction mapping it cannot run inline (its own gather checks this).
    pub(crate) fn protect_inline_text(&mut self, start: u32, end: u32) {
        let len = end - start;
        if let Some(offset) = self.ram.checked_offset(start, len as usize, Access::Read, 1) {
            let lo = u64::from(offset);
            self.inline_text = lo..lo + u64::from(len);
        }
    }

    pub(crate) fn allow_large_segment(&mut self) {
        self.allow_spill = true;
    }

    fn index_store(words: &mut std::collections::HashMap<u32, [Option<u8>; 4]>, store: &Store) {
        for (i, &byte) in store.bytes[..usize::from(store.len)].iter().enumerate() {
            let p = store.physical + i as u32;
            words.entry(p / 4).or_default()[(p % 4) as usize] = Some(byte);
        }
    }

    fn commit(self) -> Result<(), GuestFault> {
        for store in self.log.written(self.count).iter().chain(&self.spill) {
            self.ram.write_bytes(
                store.addr,
                &store.bytes[..usize::from(store.len)],
                store.alignment,
            )?;
        }
        Ok(())
    }
}

impl<const N: usize> Plan<'_, Guest, N> {
    /// The caller may subscribe to physical writes and read this range to
    /// build a derived lookup table. All guest stores still go via the log.
    /// A staged write in either mapping makes the underlying RAM stale.
    pub(crate) fn unstaged_read(&mut self, addr: u32, len: usize) -> Option<&mut Guest> {
        let start = u64::from(self.ram.checked_offset(addr, len, Access::Read, 1)?);
        let end = start + len as u64;
        let changed = self.log.written(self.count).iter().chain(&self.spill).any(|s| {
            let written = u64::from(s.physical);
            written < end && start < written + u64::from(s.len)
        });
        (!changed).then_some(&mut *self.ram)
    }
}

impl<M: SegmentMemory, const N: usize> Memory for Plan<'_, M, N> {
    fn physical_read(&self, addr: u32, len: usize) -> Option<u32> {
        self.ram.checked_offset(addr, len, Access::Read, 1)
    }

    fn read_bytes(&self, addr: u32, dst: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        let Some(physical) = self.ram.checked_offset(addr, dst.len(), Access::Read, alignment) else {
            // A load across two page mappings reads each page's piece.
            if crosses_page(addr, dst.len()) && addr.is_multiple_of(alignment.max(1)) {
                let mut rest = &mut *dst;
                for (at, len) in page_pieces(addr, rest.len()) {
                    let (piece, tail) = rest.split_at_mut(len);
                    self.read_bytes(at, piece, 1)?;
                    rest = tail;
                }
                return Ok(());
            }
            let why = Decline::Read { addr, len: dst.len() as u32 };
            return Err(self.decline(addr, dst.len(), Access::Read, why));
        };
        self.ram.read_bytes(addr, dst, alignment)?;
        if let Some(words) = &self.forwarded {
            for (i, byte) in dst.iter_mut().enumerate() {
                let p = physical + i as u32;
                if let Some(Some(value)) = words.get(&(p / 4)).map(|w| w[(p % 4) as usize]) {
                    *byte = value;
                }
            }
            return Ok(());
        }
        let start = u64::from(physical);
        let end = start + dst.len() as u64;
        for store in self.logged() {
            let written = u64::from(store.physical);
            let lo = start.max(written);
            let hi = end.min(written + u64::from(store.len));
            if lo < hi {
                dst[(lo - start) as usize..(hi - start) as usize].copy_from_slice(
                    &store.bytes[(lo - written) as usize..(hi - written) as usize],
                );
            }
        }
        Ok(())
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], alignment: u32) -> Result<(), GuestFault> {
        let len = src.len() as u32;
        let Some(physical) = self.ram.checked_offset(addr, src.len(), Access::Write, alignment) else {
            // A store across two page mappings logs each page's piece.
            if crosses_page(addr, src.len()) && addr.is_multiple_of(alignment.max(1)) {
                let mut rest = src;
                for (at, len) in page_pieces(addr, rest.len()) {
                    let (piece, tail) = rest.split_at(len);
                    self.write_bytes(at, piece, 1)?;
                    rest = tail;
                }
                return Ok(());
            }
            return Err(self.decline(addr, src.len(), Access::Write, Decline::Write { addr, len }));
        };
        let start = u64::from(physical);
        let end = start + src.len() as u64;
        if self.text.overlaps(start, end) || (start < self.inline_text.end && self.inline_text.start < end) {
            return Err(self.decline(addr, src.len(), Access::Write, Decline::TextWrite { addr, len }));
        }
        let mut store = Store::default();
        let Some(dst) = store.bytes.get_mut(..src.len()) else {
            return Err(self.decline(addr, src.len(), Access::Write, Decline::Write { addr, len }));
        };
        dst.copy_from_slice(src);
        store.addr = addr;
        store.physical = physical;
        store.len = src.len() as u8;
        store.alignment = alignment;
        if self.count < N {
            self.log.0[self.count] = MaybeUninit::new(store);
            self.count += 1;
        } else {
            // Bound malformed copy counts before anything reaches guest RAM.
            if !self.allow_spill || self.spill.len() >= 65536 {
                return Err(self.decline(addr, src.len(), Access::Write, Decline::LogFull { addr }));
            }
            let (log, count) = (&*self.log, self.count);
            let words = self.forwarded.get_or_insert_with(|| {
                let mut words = std::collections::HashMap::new();
                for previous in log.written(count) {
                    Self::index_store(&mut words, previous);
                }
                words
            });
            Self::index_store(words, &store);
            self.spill.push(store);
        }
        Ok(())
    }
}

/// Enter only on a declared boundary. Hold the RAM borrow through scatter
/// so checked destinations cannot be remapped after the calculation.
pub(crate) fn segment<const N: usize, M: SegmentMemory>(
    cpu: &mut runtime::CpuAbi,
    ram: &mut M,
    pc: u32,
    text: std::ops::Range<u32>,
    sites: &[u32],
    body: impl FnOnce(&mut runtime::CpuAbi, &mut Plan<'_, M, N>, u32) -> super::Result,
) -> Result<Option<runtime::TransferAbi>, GuestFault> {
    let mut log = Log::new();
    let mut plan = match Plan::for_segment(ram, &mut log, cpu, pc, text, sites) {
        Ok(plan) => plan,
        Err(why) => {
            note_decline(pc, why);
            return Ok(None);
        }
    };
    let mut next = *cpu;
    let transfer = match body(&mut next, &mut plan, pc) {
        Ok(transfer) => transfer,
        Err(fault) => {
            note_decline(pc, plan.explain(&fault));
            return Ok(None);
        }
    };
    plan.commit()?;
    *cpu = next;
    Ok(Some(transfer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::unit;
    const TEXT_START: u32 = crate::points::SIM_UNIT_POSE.base;
    const TEXT_LEN: u32 = 0x112dbb - TEXT_START;
    use runtime::{CpuAbi, guest::Permissions};

    #[test]
    fn overlapping_stores_forward_through_a_second_mapping() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN, 0x4000, Permissions::RX)
            .unwrap();
        for addr in [0x10000, 0x20000] {
            ram.map(addr, 0x1000, 0, Permissions::RW).unwrap();
        }
        ram.map(0x30000, TEXT_LEN, 0x4000, Permissions::RW)
            .unwrap();
        let mut log = Log::new();
        let mut plan = Plan::<_, 64>::new(&mut ram, &mut log, TEXT_START, TEXT_START + TEXT_LEN).unwrap();
        assert!(plan.set_u8(0x30002, 0xf1).is_err());
        // A parent can spill into another mapping of its inlined leaf.
        plan.protect_inline_text(0x20080, 0x200c0);
        assert!(plan.set_u32(0x1007e, 0xf1).is_err());
        plan.set_u32(0x10002, 0x44332211).unwrap();
        plan.set_u8(0x20004, 0xfa).unwrap();
        assert_eq!(plan.u32(0x20001).unwrap(), 0xfa221100);
        assert_eq!(plan.u16(0x10004).unwrap(), 0x44fa);
        plan.commit().unwrap();
        assert_eq!(ram.u32(0x10002).unwrap(), 0x44fa2211);
    }

    #[test]
    fn a_plan_borrows_its_store_log_so_moving_it_copies_no_log() {
        // The manager's 128-store log is 2.5 KB; the plan is a few words.
        assert!(size_of::<Log<128>>() >= 128 * size_of::<Store>());
        assert!(size_of::<Plan<'static, Guest, 128>>() <= 256);
    }

    #[test]
    fn the_fast_page_table_gives_the_offset_the_mapping_walk_gives() {
        let layout = |ram: &mut Guest| {
            ram.map(0x10000, 0x2000, 0, Permissions::RW).unwrap();
            ram.map(0x20000, 0x1000, 0x2000, Permissions::READ).unwrap();
            ram.map(0x30000, 0x1000, 0x3000, Permissions::RX).unwrap();
            // An alias of the first page, and a partial page the table skips.
            ram.map(0x40000, 0x1000, 0, Permissions::RW).unwrap();
            ram.map(0x50800, 0x400, 0x4800, Permissions::RW).unwrap();
        };
        let (mut walked, mut paged) = (Guest::new(), Guest::new());
        layout(&mut walked);
        layout(&mut paged);
        paged.prepare_fast_pages();
        let mut fast = 0;
        for base in [0x10000u32, 0x11ffc, 0x20000, 0x30010, 0x40ffe, 0x50800, 0x50bfe, 0x60000] {
            for offset in [0u32, 1, 2, 4] {
                for len in [1usize, 2, 4, 8, 16] {
                    for access in [Access::Read, Access::Write, Access::Execute] {
                        for alignment in [1, 4] {
                            let addr = base + offset;
                            let want = walked.checked_offset(addr, len, access, alignment);
                            assert_eq!(walked.fast_physical(addr, len, access, alignment), None);
                            assert_eq!(
                                paged.checked_offset(addr, len, access, alignment),
                                want,
                                "{addr:#x} len {len} {access:?} align {alignment}"
                            );
                            fast += usize::from(paged.fast_physical(addr, len, access, alignment).is_some());
                        }
                    }
                }
            }
        }
        // The table answered the one-page accesses itself.
        assert!(fast > 100, "{fast}");
    }

    #[test]
    fn spilled_stores_forward_through_aliases_and_commit_in_order() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN, 0x4000, Permissions::RX).unwrap();
        for va in [0x10000, 0x20000] {
            ram.map(va, 0x1000, 0, Permissions::RW).unwrap();
        }
        let mut log = Log::new();
        let mut plan = Plan::<_, 2>::new(&mut ram, &mut log, TEXT_START, TEXT_START + TEXT_LEN).unwrap();
        plan.allow_large_segment();
        plan.set_u32(0x10002, 0x44332211).unwrap();
        plan.set_u16(0x10005, 0xabcd).unwrap();
        plan.set_u32(0x20000, 0x76543210).unwrap();
        assert_eq!(plan.u32(0x20002).unwrap(), 0xcd337654);
        assert!(plan.unstaged_read(0x10001, 2).is_none());
        assert!(plan.unstaged_read(0x20004, 1).is_none());
        assert!(plan.unstaged_read(0x20007, 1).is_some());
        plan.set_u8(0x10003, 0x12).unwrap();
        assert_eq!(
            plan.final_bytes(),
            [(0, 0x10), (1, 0x32), (2, 0x54), (3, 0x12), (4, 0x33), (5, 0xcd), (6, 0xab)]
        );
        plan.commit().unwrap();
        assert_eq!(ram.u32(0x20002).unwrap(), 0xcd331254);
    }

    #[test]
    fn late_protected_push_discards_frame_store_and_cpu() {
        let mut ram = Guest::new();
        ram.map(TEXT_START, TEXT_LEN, 0x4000, Permissions::RX)
            .unwrap();
        let sp = 0x20000;
        ram.map(sp - 0x1000, 0x1000, 0, Permissions::READ).unwrap();
        ram.map(sp, 0x1000, 0x1000, Permissions::RW).unwrap();
        let mut cpu = CpuAbi {
            esp: sp,
            ebx: 0xdeadbeef,
            ..CpuAbi::default()
        };
        cpu.xmm[0] = [0x99; 16];
        let before = cpu;
        // After the second overlay weight lookup, the fraction store to +20
        // succeeds in the plan before the initializer's return push fails.
        assert_eq!(unit::run(&mut cpu, &mut ram, 0x1127f5).unwrap(), None);
        assert_eq!(cpu, before);
        assert_eq!(ram.u32(sp + 0x20).unwrap(), 0);
    }

    /// The XBE loader's layout: one mapping per page. Each page here has
    /// no physical neighbour, so every page is its own span.
    fn paged(ram: &mut Guest, start: u32, end: u32, physical: u32, permissions: Permissions) {
        let (mut va, mut p) = (start & !(PAGE - 1), physical);
        while va < end {
            ram.map(va, PAGE, p, permissions).unwrap();
            (va, p) = (va + PAGE, p + 2 * PAGE);
        }
    }

    #[test]
    fn text_across_page_mappings_plans_and_stays_guarded() {
        // ANIM_MANAGER_0x1c6f60's text crosses 0x1c7000.
        let (start, end) = (0x1c6f60, 0x1c70f7);
        let mut ram = Guest::new();
        paged(&mut ram, start, end, 0x10000, Permissions::RWX);
        assert_eq!(ram.mappings().len(), 2);
        let mut log = Log::new();
        let mut plan = Plan::<_, 8>::new(&mut ram, &mut log, start, end).unwrap();
        assert_eq!(usize::from(plan.text.n), 2);
        // The pages are writable; the guard is the plan's.
        for addr in [start, 0x1c6ffe, 0x1c7000, end - 1] {
            assert!(plan.set_u8(addr, 1).is_err(), "{addr:#x}");
            assert_eq!(plan.why.get(), Some(Decline::TextWrite { addr, len: 1 }));
        }
        for addr in [start - 1, end] {
            plan.set_u8(addr, 1).unwrap();
        }
    }

    #[test]
    fn physically_adjacent_pages_merge_and_too_many_spans_decline() {
        let mut ram = Guest::new();
        ram.map(0x40000, PAGE, 0x10000, Permissions::RX).unwrap();
        ram.map(0x41000, PAGE, 0x11000, Permissions::RX).unwrap();
        let mut log = Log::<8>::new();
        let plan = Plan::new(&mut ram, &mut log, 0x40f00, 0x41100).unwrap();
        assert_eq!(&plan.text.at[..usize::from(plan.text.n)], &[(0x10f00, 0x11100)]);
        drop(plan);
        let start = 0x100000;
        let end = start + SPANS as u32 * PAGE + 1;
        paged(&mut ram, start, end, 0x20000, Permissions::RX);
        let refused = Plan::new(&mut ram, &mut log, start, end).err();
        assert_eq!(refused, Some(Decline::Text { start, end }));
        // An unmapped page in the text declines the same way.
        let refused = Plan::new(&mut ram, &mut log, 0x41f00, 0x42100).err();
        assert_eq!(refused, Some(Decline::Text { start: 0x41f00, end: 0x42100 }));
    }

    #[test]
    fn a_word_across_two_page_mappings_loads_and_stores_in_pieces() {
        let mut ram = Guest::new();
        ram.map(0x40000, PAGE, 0x10000, Permissions::RX).unwrap();
        ram.map(0x50000, PAGE, 0x20000, Permissions::RW).unwrap();
        ram.map(0x51000, PAGE, 0x30000, Permissions::RW).unwrap();
        for (i, b) in [0x11, 0x22, 0x33, 0x44].into_iter().enumerate() {
            ram.set_u8(0x50ffe + i as u32, b).unwrap();
        }
        let mut log = Log::new();
        let mut plan = Plan::<_, 8>::new(&mut ram, &mut log, 0x40000, 0x40010).unwrap();
        assert_eq!(plan.u32(0x50ffe).unwrap(), 0x44332211);
        plan.set_u32(0x50ffe, 0xddccbbaa).unwrap();
        assert_eq!(plan.u16(0x50fff).unwrap(), 0xccbb);
        plan.commit().unwrap();
        let bytes: Vec<u8> = (0..4).map(|i| ram.u8(0x50ffe + i).unwrap()).collect();
        assert_eq!(bytes, [0xaa, 0xbb, 0xcc, 0xdd]);
    }

    type Body = fn(&mut CpuAbi, &mut Plan<'_, Guest, 2>) -> crate::sim_native::Result;
    const AT: u32 = 0x40000;

    fn declined(ram: &mut Guest, cpu: CpuAbi, pc: u32, body: Body) -> Option<(u32, Decline)> {
        let mut cpu = cpu;
        let before = cpu;
        let t = segment::<2, _>(&mut cpu, ram, pc, AT..AT + 0x10, &[AT + 8], |c, g, _| body(c, g));
        assert_eq!(t, Ok(None));
        assert_eq!(cpu, before);
        let why = take_decline();
        assert_eq!(take_decline(), None, "taken once");
        why
    }

    #[test]
    fn a_declined_segment_names_why() {
        use crate::sim_native::Result;
        use runtime::TransferAbi;
        fn read(c: &mut CpuAbi, g: &mut Plan<'_, Guest, 2>) -> Result {
            c.eax = g.u32(0x90000)?;
            Ok(TransferAbi::ret(0))
        }
        fn write(_: &mut CpuAbi, g: &mut Plan<'_, Guest, 2>) -> Result {
            g.set_u16(0x50010, 1)?;
            Ok(TransferAbi::ret(0))
        }
        fn own_text(_: &mut CpuAbi, g: &mut Plan<'_, Guest, 2>) -> Result {
            g.set_u32(AT + 12, 1)?;
            Ok(TransferAbi::ret(0))
        }
        fn full(_: &mut CpuAbi, g: &mut Plan<'_, Guest, 2>) -> Result {
            for addr in [0x40100, 0x40104, 0x40108] {
                g.set_u32(addr, 1)?;
            }
            Ok(TransferAbi::ret(0))
        }
        // A probe the body survives is not its reason.
        fn own_fault(_: &mut CpuAbi, g: &mut Plan<'_, Guest, 2>) -> Result {
            let _ = g.u32(0x90000);
            Err(GuestFault { addr: AT, len: 0, access: Access::Execute, reason: FaultReason::InvalidOperation })
        }
        fn never(_: &mut CpuAbi, _: &mut Plan<'_, Guest, 2>) -> Result {
            unreachable!("the plan declines before the body")
        }
        let mut ram = Guest::new();
        ram.map(AT, PAGE, 0x10000, Permissions::RWX).unwrap();
        ram.map(0x50000, PAGE, 0x20000, Permissions::READ).unwrap();
        let cpu = CpuAbi::default();
        let cases: [(u32, Body, Decline); 6] = [
            (AT, read, Decline::Read { addr: 0x90000, len: 4 }),
            (AT, write, Decline::Write { addr: 0x50010, len: 2 }),
            (AT + 8, own_text, Decline::TextWrite { addr: AT + 12, len: 4 }),
            (AT, full, Decline::LogFull { addr: 0x40108 }),
            (AT, own_fault, Decline::Body { addr: AT, len: 0, access: Access::Execute, reason: FaultReason::InvalidOperation }),
            (AT + 4, never, Decline::Boundary),
        ];
        for (pc, body, why) in cases {
            assert_eq!(declined(&mut ram, cpu, pc, body), Some((pc, why)), "{why}");
        }
        let float = CpuAbi { mxcsr: 0, ..cpu };
        assert_eq!(declined(&mut ram, float, AT, never), Some((AT, Decline::Float)));
        ram.install_trap(4);
        assert_eq!(declined(&mut ram, cpu, AT, never), Some((AT, Decline::Trap)));
    }
}
