//! `native-flare-query`: the flares' visibility tests made in Rust
//! (native-only plan, `nor-8f`; docs/graphics/d3d-visibility.md, "The
//! query in Rust"). On with the native renderer.
//!
//! | Guest | Cut |
//! |---|---|
//! | the query pass `0x0002dd30`'s walker call `0x00020bb0(view, 0x1f, 0, 0, 1, 0x0002e540)` | every entry's quad from [`scene_extract::flares::Lens::query`], one count pass through [`Host::query`], the area at `+6` and the set's bit as the walker writes them |
//! | the reader `0x00020770` | each entry's byte from its count two frames back ([`scene_extract::flares::read_byte`]), `0x00020e50` for an inactive entry, the view's read frame |
//!
//! The pass's own state setup (the target, the render states, the vertex
//! program) stays the guest's: only its walker call is cut, so the count
//! reads the depth the guest bound. The water ocean's gate makes its own
//! registration and listed walker call in Rust ([`register`],
//! [`walk_listed`], from `water_end::ocean::gate`). The draw pass's walker
//! call runs the guest body, and the reader reads a slot the guest ended
//! from the device ([`Host::device_result`]). Both hooks are
//! `implement`s at their entries, placed after `native-targets`; no other
//! hook sits there.
//!
//! The slots: the cut keeps the counts of the slots it ended; the bitmaps
//! at `0x005233f0` stay the one record of which slots hold a test.
//! [`State::scrub`] clears the bits of the cut's slots in a set, so a
//! guest body is never asked for a test it did not end.
//!
//! The query's walker call and every reader call are the cut's: neither
//! guest body runs for them. A call the cut cannot make writes nothing and
//! returns, its entries keeping their areas and bytes, counted
//! `scene_dropped_flare_<call>_<cause>`; a device read refused for a slot
//! the guest ended is `scene_dropped_flare_read_device`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest};
use scene_extract::GuestRead;
pub use scene_extract::flares::Quad;
use scene_extract::flares::{self as fl, Entry, Lens, ReadConstants};

pub const ID: &str = "native-flare-query";

/// The walker call the cut takes: the query pass's.
pub const QUERY_CALLBACK: u32 = 0x0002_e540;
pub const QUERY_MASK: u32 = 0x1f;

/// The counts of one count pass, in quad order.
pub trait Counts: Send + Sync {
    /// `None` while the GPU has not returned them.
    fn read(&self) -> Option<Arc<[u32]>>;
}

/// A slot's count on the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceResult {
    Ready(u32),
    Pending,
    /// The device refused the read.
    Refused,
}

/// What answers the GPU's side: boot's glue over the shared device.
pub trait Host: Send + Sync {
    /// Places the count pass at this point in the frame: each quad's
    /// samples that pass `LESSEQUAL` against the bound depth. `None`:
    /// not placed, nothing recorded.
    fn query(&self, mem: &mut dyn GuestMemory, quads: Vec<Quad>) -> Option<Arc<dyn Counts>>;
    /// `GetVisibilityTestResult(slot)`, for a slot the guest's walker
    /// ended.
    fn device_result(&self, mem: &mut dyn GuestMemory, slot: u32) -> DeviceResult;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call runs the guest body.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A slot the cut ended.
#[derive(Clone)]
pub enum SlotCount {
    /// Ended with no quad: the callback's 0.
    Ready(u32),
    /// Quad `k` of a count pass.
    Counted(Arc<dyn Counts>, usize),
    /// The sum of quads `range` of a count pass (one test drawn as
    /// several quads).
    Summed(Arc<dyn Counts>, std::ops::Range<usize>),
}

impl SlotCount {
    /// The count, `None` while pending.
    pub fn get(&self) -> Option<u32> {
        match self {
            Self::Ready(n) => Some(*n),
            Self::Counted(c, k) => c.read().and_then(|v| v.get(*k).copied()),
            Self::Summed(c, r) => c.read().and_then(|v| v.get(r.clone()).map(|s| s.iter().fold(0u32, |a, &n| a.saturating_add(n)))),
        }
    }
}

/// The cut's slots.
#[derive(Default)]
pub struct State {
    slots: HashMap<u32, SlotCount>,
}

impl State {
    /// Clears the bits, and forgets the counts, of the cut's slots in
    /// `sets` (bit `s` is set `s`), so a guest body never asks the device
    /// for them.
    pub fn scrub(&mut self, mem: &mut dyn GuestMemory, sets: u8) {
        self.slots.retain(|&slot, _| {
            let set = slot % 3;
            if sets & (1 << set) == 0 {
                return true;
            }
            let (word, bit) = fl::ended_bit(set, slot / 3);
            if let Ok(v) = mem.u32(word) {
                let _ = mem.set_u32(word, v & !bit);
            }
            false
        });
    }

    /// A slot's count, from the cut's own or the device's; `None` pending.
    fn count(&self, mem: &mut dyn GuestMemory, host: &dyn Host, slot: u32) -> Option<u32> {
        match self.slots.get(&slot) {
            Some(c) => c.get(),
            None => match host.device_result(mem, slot) {
                DeviceResult::Ready(n) => Some(n),
                DeviceResult::Pending => None,
                DeviceResult::Refused => {
                    READ.dropped("device");
                    None
                }
            },
        }
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(STATE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default())
}

/// Every set, for [`State::scrub`].
pub const ALL_SETS: u8 = 0b111;

/// One of the two calls, with its arm.
pub struct Arm {
    name: &'static str,
    calls: AtomicU64,
    cut: AtomicU64,
    dropped: Mutex<std::collections::BTreeMap<&'static str, u64>>,
}

impl Arm {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            calls: AtomicU64::new(0),
            cut: AtomicU64::new(0),
            dropped: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    /// A call not made, or a placed pass that did not count, for `cause`:
    /// its entries keep their areas and bytes.
    pub fn dropped(&self, cause: &'static str) {
        *self.dropped.lock().unwrap_or_else(|e| e.into_inner()).entry(cause).or_default() += 1;
    }

    fn counters(&self) -> Vec<(String, u64)> {
        let mut out = vec![
            (format!("flare_{}_calls", self.name), self.calls.load(Ordering::Relaxed)),
            (format!("flare_{}_cut", self.name), self.cut.load(Ordering::Relaxed)),
        ];
        out.extend(
            self.dropped
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(c, n)| (format!("scene_dropped_flare_{}_{c}", self.name), *n)),
        );
        out
    }
}

pub static QUERY: Arm = Arm::new("query");
pub static READ: Arm = Arm::new("read");

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    QUERY.counters().into_iter().chain(READ.counters()).collect()
}

/// The guest's memory as `scene_extract` reads it.
struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// One entry the walk ends: its quad (`None`: the callback's 0) and area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ended {
    pub index: u32,
    pub quad: Option<Quad>,
    /// The callback's return, held to `[0, 0xffff]` as the walker stores it.
    pub area: u16,
}

/// What the walker would end for `view` at `frame`: every active entry of
/// the view in the query mask whose slot is not still pending. Nothing
/// written. `Err` names why the call is dropped.
pub fn plan_walk(mem: &mut dyn GuestMemory, host: &dyn Host, state: &State, view: u32, frame: u32) -> Result<Vec<Ended>, &'static str> {
    let ro = Ro(&*mem);
    let n = fl::count(&ro).ok_or("count")?;
    let lens = Lens::read(&ro).ok_or("unread")?;
    let set = fl::query_set(frame);
    let mut planned = Vec::new();
    for i in 0..n {
        let ro = Ro(&*mem);
        let e = Entry::read(&ro, i).ok_or("unread")?;
        if !e.active() || e.view() != view || QUERY_MASK & (1 << e.class()) == 0 {
            continue;
        }
        let (word, bit) = fl::ended_bit(set, i);
        let ended = ro.read_u32(word).ok_or("unread")? & bit != 0;
        let quad = lens.query(&ro, &e).map_err(|u| match u {
            fl::Unmade::Unread => "unread",
            fl::Unmade::Table => "table",
        })?;
        if ended && state.count(mem, host, fl::slot(set, i)).is_none() {
            continue;
        }
        let area = quad.map_or(0, |q| q.area.min(0xffff) as u16);
        planned.push(Ended { index: i, quad, area });
    }
    Ok(planned)
}

/// Places the count pass for `planned`: each entry's count. `Err`: not
/// placed.
pub fn place(mem: &mut dyn GuestMemory, host: &dyn Host, planned: &[Ended]) -> Result<Vec<(u32, SlotCount)>, &'static str> {
    let quads: Vec<Quad> = planned.iter().filter_map(|e| e.quad).collect();
    let counts = if quads.is_empty() { None } else { Some(host.query(mem, quads).ok_or("placed")?) };
    let mut k = 0;
    Ok(planned
        .iter()
        .map(|e| {
            let c = match (&e.quad, &counts) {
                (Some(_), Some(c)) => {
                    k += 1;
                    SlotCount::Counted(Arc::clone(c), k - 1)
                }
                _ => SlotCount::Ready(0),
            };
            (e.index, c)
        })
        .collect())
}

/// The walker's writes for `planned`: each area at `+6` and the set's bit;
/// the counts become the cut's slots.
pub fn commit_walk(mem: &mut dyn GuestMemory, state: &mut State, frame: u32, planned: &[Ended], counts: Vec<(u32, SlotCount)>) -> Result<(), &'static str> {
    let set = fl::query_set(frame);
    for (e, (i, c)) in planned.iter().zip(counts) {
        mem.set_u16(fl::entry_addr(i) + 6, e.area).map_err(|_| "unwritable")?;
        let (word, bit) = fl::ended_bit(set, i);
        let v = mem.u32(word).map_err(|_| "unread")?;
        mem.set_u32(word, v | bit).map_err(|_| "unwritable")?;
        state.slots.insert(fl::slot(set, i), c);
    }
    Ok(())
}

/// What `0x000209b0` matches an entry's head against (its key, EAX): the
/// low bit, the view, the class, the owner and the top six bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub bit: bool,
    pub view: u32,
    pub class: u32,
    pub owner: u32,
    pub tail: u32,
}

impl Key {
    fn matches(&self, head: u32) -> bool {
        head & 1 == u32::from(self.bit)
            && (head >> 1) & 3 == self.view
            && (head >> 3) & 7 == self.class
            && (head >> 6) & 0xfff == self.owner
            && (head >> 18) & 0x3f == self.tail
    }

    /// The head `0x00020a82..0x00020ad7` builds over `head`.
    fn head(&self, head: u32) -> u32 {
        let put = |h: u32, v: u32, mask: u32| ((v ^ h) & mask) ^ h;
        let h = put(head, u32::from(self.bit), 1);
        let h = put(h, self.view << 1, 6);
        let h = put(h, self.class << 3, 0x38);
        let h = put(h, self.owner << 6, 0x3_ffc0);
        put(h, self.tail << 18, 0xfc_0000)
    }
}

/// The gates `0x000209b0` returns -1 behind: the array off, no current
/// view, `[0x00485607]` set, `[0x004850c8]` not 0.
pub const REGISTER_BYTE: u32 = 0x0048_5607;
pub const REGISTER_WORD: u32 = 0x0048_50c8;

/// `0x000209b0(eax key, payload, size)`: the in-use entry whose head
/// matches `key`, else the first free entry (freed by `0x00020e50`, its
/// head built, the count raised past it); an entry not active this frame
/// takes the payload's size (`+4` bits 2..6) and the payload at `+8`, and
/// becomes in use and active. `None` where the guest returns -1.
pub fn register(mem: &mut dyn GuestMemory, state: &mut State, key: Key, payload: &[u8]) -> Result<Option<u32>, &'static str> {
    let r = |mem: &dyn GuestMemory, at: u32| mem.u32(at).map_err(|_| "unread");
    let byte = |mem: &dyn GuestMemory, at: u32| mem.u8(at).map_err(|_| "unread");
    if byte(&*mem, fl::ON)? == 0
        || r(&*mem, fl::CURRENT_VIEW)? == u32::MAX
        || byte(&*mem, REGISTER_BYTE)? != 0
        || r(&*mem, REGISTER_WORD)? != 0
    {
        return Ok(None);
    }
    let n = r(&*mem, fl::COUNT)? as i32;
    let mut found = None;
    for i in 0..n.max(0) as u32 {
        let at = fl::entry_addr(i);
        if r(&*mem, at + 4)? & 1 != 0 && key.matches(r(&*mem, at)?) {
            found = Some(i);
            break;
        }
    }
    let index = match found {
        Some(i) => i,
        None => {
            // The free scan runs to the end of the array, not the count.
            let Some(i) = (0..(fl::COUNT - fl::ENTRIES) / fl::ENTRY_SIZE)
                .map(|i| r(&*mem, fl::entry_addr(i) + 4).map(|f| (i, f)))
                .find(|v| v.as_ref().map_or(true, |&(_, f)| f & 1 == 0))
                .transpose()?
                .map(|(i, _)| i)
            else {
                return Ok(None);
            };
            free(mem, state, i)?;
            let at = fl::entry_addr(i);
            let head = key.head(r(&*mem, at)?);
            mem.set_u32(at, head).map_err(|_| "unwritable")?;
            if (i + 1) as i32 > r(&*mem, fl::COUNT)? as i32 {
                mem.set_u32(fl::COUNT, i + 1).map_err(|_| "unwritable")?;
            }
            i
        }
    };
    let at = fl::entry_addr(index);
    let flags = r(&*mem, at + 4)?;
    if flags & 2 == 0 {
        let size = payload.len() as u32;
        let flags = ((((size * 4) ^ flags) & 0x7c) ^ flags) | 3;
        mem.set_u32(at + 4, flags).map_err(|_| "unwritable")?;
        mem.write(at + 8, payload).map_err(|_| "unwritable")?;
    }
    Ok(Some(index))
}

/// A listed entry's test, made by its caller: quads whose counts sum, or
/// a count known with no pass.
#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    Quads(Vec<Quad>),
    Known(u32),
}

/// One entry of a listed walker call: its test and the area the walker
/// stores at `+6`.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub index: u32,
    pub test: Test,
    pub area: u16,
}

/// A listed walker call `0x00020bb0(view, mask, list, n, 1, callback)`
/// whose callback's tests are `listed`: each active entry of the view in
/// `mask` whose slot is not still pending is tested (one count pass for
/// every quad), its area stored and its bit set, its count the cut's
/// slot. Nothing when the walker's gates hold.
pub fn walk_listed(mem: &mut dyn GuestMemory, host: &dyn Host, state: &mut State, view: u32, mask: u32, listed: &[Listed]) -> Result<(), &'static str> {
    let Some(frame) = fl::walk_frame(&Ro(&*mem), view) else { return Ok(()) };
    let set = fl::query_set(frame);
    let mut planned = Vec::new();
    for l in listed {
        let e = Entry::read(&Ro(&*mem), l.index).ok_or("unread")?;
        if !e.active() || e.view() != view || mask & (1 << e.class()) == 0 {
            continue;
        }
        let (word, bit) = fl::ended_bit(set, l.index);
        let ended = mem.u32(word).map_err(|_| "unread")? & bit != 0;
        if ended && state.count(mem, host, fl::slot(set, l.index)).is_none() {
            continue;
        }
        planned.push(l);
    }
    let quads: Vec<Quad> = planned.iter().flat_map(|l| match &l.test {
        Test::Quads(q) => q.clone(),
        Test::Known(_) => Vec::new(),
    }).collect();
    let counts = if quads.is_empty() { None } else { Some(host.query(mem, quads).ok_or("placed")?) };
    let mut k = 0;
    for l in planned {
        let c = match (&l.test, &counts) {
            (Test::Quads(q), Some(c)) if !q.is_empty() => {
                k += q.len();
                SlotCount::Summed(Arc::clone(c), k - q.len()..k)
            }
            (Test::Known(n), _) => SlotCount::Ready(*n),
            _ => SlotCount::Ready(0),
        };
        mem.set_u16(fl::entry_addr(l.index) + 6, l.area).map_err(|_| "unwritable")?;
        let (word, bit) = fl::ended_bit(set, l.index);
        let v = mem.u32(word).map_err(|_| "unread")?;
        mem.set_u32(word, v | bit).map_err(|_| "unwritable")?;
        state.slots.insert(fl::slot(set, l.index), c);
    }
    Ok(())
}

/// The installed host and the cut's slots, for a caller outside the two
/// hooks (the water ocean's gate). `None` without a host.
pub fn with_host<R>(f: impl FnOnce(&dyn Host, &mut State) -> R) -> Option<R> {
    let host = host()?;
    Some(with_state(|state| f(&*host, state)))
}

/// `0x00020e50(eax index)`: the entry's bits cleared in all three sets,
/// the entry zeroed, its flags `0xff80`.
pub fn free(mem: &mut dyn GuestMemory, state: &mut State, index: u32) -> Result<(), &'static str> {
    for set in 0..3 {
        let (word, bit) = fl::ended_bit(set, index);
        let v = mem.u32(word).map_err(|_| "unread")?;
        mem.set_u32(word, v & !bit).map_err(|_| "unwritable")?;
        state.slots.remove(&fl::slot(set, index));
    }
    let at = fl::entry_addr(index);
    for k in 0..8 {
        mem.set_u32(at + 4 * k, if k == 1 { 0xff80 } else { 0 }).map_err(|_| "unwritable")?;
    }
    Ok(())
}

/// The byte the reader leaves for `e` read from `set`: `None` when it
/// leaves the entry (not ended in the set, or its count pending).
fn read_entry(mem: &mut dyn GuestMemory, host: &dyn Host, state: &State, e: &Entry, set: u32, k: &ReadConstants) -> Result<Option<u8>, &'static str> {
    let (word, bit) = fl::ended_bit(set, e.index);
    if mem.u32(word).map_err(|_| "unread")? & bit == 0 {
        return Ok(None);
    }
    let Some(samples) = state.count(mem, host, fl::slot(set, e.index)) else { return Ok(None) };
    let s = fl::smoothing(&Ro(&*mem), e).ok_or("unread")?;
    Ok(Some(fl::read_byte(e.byte(), samples, e.area(), e.class(), s, k)))
}

/// The reader `0x00020770` for `view` at `frame`.
pub fn cut_read(mem: &mut dyn GuestMemory, host: &dyn Host, state: &mut State, view: u32, frame: u32) -> Result<(), &'static str> {
    let k = ReadConstants::read(&Ro(&*mem)).ok_or("unread")?;
    let set = fl::read_set(frame);
    let n = fl::count(&Ro(&*mem)).ok_or("count")?;
    for i in 0..n {
        let e = Entry::read(&Ro(&*mem), i).ok_or("unread")?;
        if e.view() != view {
            continue;
        }
        if !e.active() {
            free(mem, state, i)?;
            continue;
        }
        if let Some(b) = read_entry(mem, host, state, &e, set, &k)? {
            mem.set_u8(fl::entry_addr(i) + 3, b).map_err(|_| "unwritable")?;
        }
    }
    mem.set_u32(fl::READ_FRAME + 4 * view, frame).map_err(|_| "unwritable")?;
    Ok(())
}

/// Whether the native flare query is placed: with the native renderer
/// on, unless `native-flare-query=off`.
pub fn on(switches: &hooks::Switches) -> bool {
    switches.feature(ID, true)
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn walk_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let args: [u32; 6] = {
        let mem = rt.memory();
        std::array::from_fn(|k| mem.u32(cpu.esp.wrapping_add(4 + 4 * k as u32)).unwrap_or(0))
    };
    let [view, mask, list, _count, query, callback] = args;
    if callback != QUERY_CALLBACK || query & 0xff != 1 || list != 0 || mask != QUERY_MASK {
        // The draw pass (no tests): it polls no slot of the query's
        // classes. The water's query is the ocean gate's, in Rust.
        return original(cpu, ctx, addr);
    }
    let arm = &QUERY;
    arm.calls.fetch_add(1, Ordering::Relaxed);
    let made = host().ok_or("host").and_then(|host| {
        let mut mem = rt.memory();
        // The walker's gates: it does nothing.
        let Some(frame) = fl::walk_frame(&Ro(&*mem), view) else { return Ok(()) };
        with_state(|state| {
            let planned = plan_walk(&mut *mem, &*host, state, view, frame)?;
            let counts = place(&mut *mem, &*host, &planned)?;
            commit_walk(&mut *mem, state, frame, &planned, counts)
        })
    });
    match made {
        Ok(()) => {
            arm.cut.fetch_add(1, Ordering::Relaxed);
        }
        Err(cause) => arm.dropped(cause),
    }
    ctx.memory().ret(cpu, 0x18)
}

fn read_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let arm = &READ;
    arm.calls.fetch_add(1, Ordering::Relaxed);
    let rt = Arc::clone(ctx.runtime());
    let made = host().ok_or("host").and_then(|host| {
        let mut mem = rt.memory();
        let Some((view, frame)) = fl::read_gate(&Ro(&*mem)) else { return Ok(()) };
        with_state(|state| cut_read(&mut *mem, &*host, state, view, frame))
    });
    match made {
        Ok(()) => {
            arm.cut.fetch_add(1, Ordering::Relaxed);
        }
        Err(cause) => arm.dropped(cause),
    }
    ctx.memory().ret(cpu, 0)
}

pub struct NativeFlareQuery;

impl Feature for NativeFlareQuery {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The flares' visibility tests and their reader made in Rust").doc("docs/graphics/d3d-visibility.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The cut's counts and checks are session state no save holds: a
        // load reads the bytes the save kept, and a slot with no count is
        // asked of the device only when the guest ended it.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&guest_layout::points::VISIBILITY_WALK, "native_flare_query_walk", walk_call).context_only();
        b.raw().implement(&guest_layout::points::VISIBILITY_READ, "native_flare_query_read", read_call).context_only();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    /// Counts the test sets by hand.
    struct Fixed(Mutex<Option<Arc<[u32]>>>);

    impl Counts for Fixed {
        fn read(&self) -> Option<Arc<[u32]>> {
            self.0.lock().unwrap().clone()
        }
    }

    #[derive(Default)]
    struct Gpu {
        passes: Mutex<Vec<(Vec<Quad>, Arc<Fixed>)>>,
        device: Mutex<HashMap<u32, u32>>,
    }

    impl Host for Gpu {
        fn query(&self, _: &mut dyn GuestMemory, quads: Vec<Quad>) -> Option<Arc<dyn Counts>> {
            let c = Arc::new(Fixed(Mutex::new(None)));
            self.passes.lock().unwrap().push((quads, Arc::clone(&c)));
            Some(c)
        }
        fn device_result(&self, _: &mut dyn GuestMemory, slot: u32) -> DeviceResult {
            self.device.lock().unwrap().get(&slot).map_or(DeviceResult::Pending, |n| DeviceResult::Ready(*n))
        }
    }

    impl Gpu {
        /// The last pass's counts come back as `n` each.
        fn land(&self, n: u32) {
            let passes = self.passes.lock().unwrap();
            let (quads, c) = passes.last().unwrap();
            *c.0.lock().unwrap() = Some(vec![n; quads.len()].into());
        }
    }

    const TAGS_TABLE: u32 = 0x0060_0000;
    const TAG: u32 = 0x0060_1000;

    /// The array with one active flare in view 0 (class 0, a lens of
    /// radius 0.01 ten units ahead) and one water entry (class 5) the
    /// guest's walker ended, and the camera and constants.
    fn mem(frame: u32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        for (a, v) in [
            (fl::k::ONE, 1.0),
            (fl::k::HALF, 0.5),
            (fl::k::TWO, 2.0),
            (fl::k::EPSILON, 1e-4),
            (fl::k::HALF_MAX, 128.0),
            (fl::k::AREA_MAX, 65_536.0),
            (fl::k::Z_SCALE, 16_777_215.0),
            (fl::k::DIRECTION, 2.0 / 255.0),
            (fl::k::ALONG, 1.0),
            (fl::k::BYTE, 255.0),
            (fl::k::BYTE_MAX, 255.0),
            (fl::WORLD_SCALE, 1.0),
            (fl::FAR, 1000.0),
        ] {
            f(&mut m, a, v);
        }
        for a in fl::k::CLASS4 {
            f(&mut m, a, 1.0);
        }
        // Identity view; projection x, y scale 1, w = view z.
        for (i, v) in [(0, 1.0), (4, 1.0), (8, 1.0)] {
            f(&mut m, fl::VIEW + 4 * i, v);
        }
        for (i, v) in [(0, 1.0), (5, 1.0), (10, 1.0), (11, 1.0)] {
            f(&mut m, fl::PROJECTION + 4 * i, v);
        }
        m.set_u32(fl::VIEWPORT, 0).unwrap();
        m.set_u32(fl::VIEWPORT + 4, 480 | 640 << 16).unwrap();
        m.set_u8(fl::ON, 1).unwrap();
        m.set_u8(fl::ON + 1, 1).unwrap();
        m.set_u32(fl::FRAME, frame).unwrap();
        m.set_u32(fl::ARRAY_FRAME, frame).unwrap();
        m.set_u32(fl::CURRENT_VIEW, 0).unwrap();
        m.set_u32(fl::COUNT, 2).unwrap();
        m.set_u32(fl::TAGS, TAGS_TABLE).unwrap();
        m.set_u32(TAGS_TABLE + 16 * 7 + 8, TAG).unwrap();
        f(&mut m, TAG + 0x10, 0.01);
        m.set_u32(TAG + 0x14, 2).unwrap();
        // Entry 0: the flare.
        let e = fl::entry_addr(0);
        m.set_u32(e, 0).unwrap();
        m.set_u32(e + 4, 2).unwrap();
        m.set_u32(e + 8, 7).unwrap();
        f(&mut m, e + 0x14, 10.0);
        // Entry 1: the water's (class 5), area 0x4b000 held to 0xffff by
        // its walker.
        let w = fl::entry_addr(1);
        m.set_u32(w, 5 << 3).unwrap();
        m.set_u32(w + 4, 2 | 0xffff << 16).unwrap();
        m.set_u32(w + 8, u32::MAX).unwrap();
        m
    }

    fn walk(m: &mut FlatMemory, gpu: &Gpu, state: &mut State, frame: u32) {
        m.set_u32(fl::FRAME, frame).unwrap();
        m.set_u32(fl::ARRAY_FRAME, frame).unwrap();
        let planned = plan_walk(m, gpu, state, 0, frame).unwrap();
        let counts = place(m, gpu, &planned).unwrap();
        commit_walk(m, state, frame, &planned, counts).unwrap();
    }

    fn read(m: &mut FlatMemory, gpu: &Gpu, state: &mut State, frame: u32) -> u8 {
        m.set_u32(fl::FRAME, frame).unwrap();
        cut_read(m, gpu, state, 0, frame).unwrap();
        m.u8(fl::entry_addr(0) + 3).unwrap()
    }

    /// The count of frame 10's quad reaches the byte at frame 12's read,
    /// not at 10's or 11's: the reader's set is the one the walker wrote
    /// two frames before.
    #[test]
    fn the_visibility_byte_lags_two_frames() {
        let gpu = Gpu::default();
        let mut state = State::default();
        let mut m = mem(10);
        walk(&mut m, &gpu, &mut state, 10);
        let passes = gpu.passes.lock().unwrap().len();
        assert_eq!(passes, 1, "one pass for the flare; the water's entry is not the query's");
        assert_eq!(m.u16(fl::entry_addr(0) + 6).unwrap(), 16, "its area stored");
        gpu.land(16);
        assert_eq!(read(&mut m, &gpu, &mut state, 10), 0, "frame 10 reads set 2");
        walk(&mut m, &gpu, &mut state, 11);
        assert_eq!(read(&mut m, &gpu, &mut state, 11), 0, "frame 11 reads set 0");
        walk(&mut m, &gpu, &mut state, 12);
        assert_eq!(read(&mut m, &gpu, &mut state, 12), 31, "frame 12 reads set 1, frame 10's");
    }

    /// The water's entry (class 5) was ended by the guest's walker: the
    /// reader takes its count from the device, unsmoothed.
    #[test]
    fn the_water_query_counts_its_quad() {
        let gpu = Gpu::default();
        let mut state = State::default();
        let mut m = mem(12);
        let set = fl::read_set(12);
        let (word, bit) = fl::ended_bit(set, 1);
        m.set_u32(word, bit).unwrap();
        gpu.device.lock().unwrap().insert(fl::slot(set, 1), 0x8000);
        cut_read(&mut m, &gpu, &mut state, 0, 12).unwrap();
        assert_eq!(m.u8(fl::entry_addr(1) + 3).unwrap(), 127, "half of 0xffff seen");
        assert_eq!(m.u32(fl::READ_FRAME).unwrap(), 12);
    }

    /// Before a guest body runs, the cut's slots lose their bits, so the
    /// device is never asked for a test it did not end.
    #[test]
    fn a_guest_body_never_sees_the_cuts_slots() {
        let gpu = Gpu::default();
        let mut state = State::default();
        let mut m = mem(10);
        walk(&mut m, &gpu, &mut state, 10);
        let (word, bit) = fl::ended_bit(fl::query_set(10), 0);
        assert_ne!(m.u32(word).unwrap() & bit, 0);
        state.scrub(&mut m, ALL_SETS);
        assert_eq!(m.u32(word).unwrap() & bit, 0);
        assert!(state.slots.is_empty());
    }

    /// `0x000209b0` finds an in-use entry by its key; a new key takes the
    /// first free entry past it, raises the count and carries its payload.
    #[test]
    fn register_finds_the_key_or_takes_a_free_entry() {
        let mut state = State::default();
        let mut m = mem(10);
        // Entry 1 (the water's, class 5 view 0) is in use and active.
        m.set_u32(fl::entry_addr(1) + 4, 3).unwrap();
        let water = Key { view: 0, class: 5, ..Key::default() };
        assert_eq!(register(&mut m, &mut state, water, &7u32.to_le_bytes()), Ok(Some(1)));
        assert_eq!(m.u32(fl::entry_addr(1) + 8).unwrap(), u32::MAX, "an active entry keeps its payload");
        let new = Key { view: 1, class: 5, ..Key::default() };
        assert_eq!(register(&mut m, &mut state, new, &0x1234u32.to_le_bytes()), Ok(Some(0)), "entry 0 is not in use");
        let e = fl::entry_addr(0);
        assert_eq!(m.u32(e).unwrap(), 1 << 1 | 5 << 3);
        assert_eq!(m.u32(e + 4).unwrap(), 0xff80 | (4 * 4) | 3);
        assert_eq!(m.u32(e + 8).unwrap(), 0x1234);
        assert_eq!(m.u32(fl::COUNT).unwrap(), 2);
        m.set_u8(REGISTER_BYTE, 1).unwrap();
        assert_eq!(register(&mut m, &mut state, new, &[]), Ok(None), "a gate returns -1");
    }

    /// A listed entry's quads are counted in one pass and summed into its
    /// slot; a known count needs no pass; the reader takes either.
    #[test]
    fn a_listed_test_sums_its_quads() {
        let gpu = Gpu::default();
        let mut state = State::default();
        let mut m = mem(10);
        let q = |x0: i32| Quad { x0, y0: 0, x1: x0 + 4, y1: 4, z: 0.0, w: 0.0, area: 16 };
        let listed = [Listed { index: 1, test: Test::Quads(vec![q(0), q(4)]), area: 0xffff }];
        walk_listed(&mut m, &gpu, &mut state, 0, 0x20, &listed).unwrap();
        assert_eq!(gpu.passes.lock().unwrap()[0].0.len(), 2);
        let (word, bit) = fl::ended_bit(fl::query_set(10), 1);
        assert_ne!(m.u32(word).unwrap() & bit, 0);
        assert_eq!(m.u16(fl::entry_addr(1) + 6).unwrap(), 0xffff);
        gpu.land(0x4000);
        assert_eq!(state.slots[&fl::slot(fl::query_set(10), 1)].get(), Some(0x8000));
        // Two frames later the reader carries it: half of 0xffff seen.
        m.set_u32(fl::FRAME, 12).unwrap();
        cut_read(&mut m, &gpu, &mut state, 0, 12).unwrap();
        assert_eq!(m.u8(fl::entry_addr(1) + 3).unwrap(), 127);
        // A class outside the mask is left; a known count needs no pass.
        let mut m = mem(10);
        walk_listed(&mut m, &gpu, &mut state, 0, 0x1f, &listed).unwrap();
        assert_eq!(m.u32(word).unwrap() & bit, 0);
        let known = [Listed { index: 1, test: Test::Known(0xffff), area: 0xffff }];
        walk_listed(&mut m, &gpu, &mut state, 0, 0x20, &known).unwrap();
        assert_eq!(gpu.passes.lock().unwrap().len(), 1);
        assert_eq!(state.slots[&fl::slot(fl::query_set(10), 1)].get(), Some(0xffff));
    }

    /// An inactive entry of the view is freed as `0x00020e50` frees it.
    #[test]
    fn the_reader_frees_an_inactive_entry() {
        let gpu = Gpu::default();
        let mut state = State::default();
        let mut m = mem(12);
        m.set_u32(fl::entry_addr(0) + 4, 0).unwrap();
        cut_read(&mut m, &gpu, &mut state, 0, 12).unwrap();
        assert_eq!(m.u32(fl::entry_addr(0) + 4).unwrap(), 0xff80);
        assert_eq!(m.u32(fl::entry_addr(0) + 8).unwrap(), 0);
    }
}
