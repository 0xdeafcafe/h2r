//! Runtime registration of the DirectSound overrides.
//!
//! [`register`] installs one safe override per [`ENTRIES`] item at its XBE
//! address. Each call:
//!
//! 1. holds one guest memory guard, then the [`DirectSound`] lock, then the
//!    arena lock (always in this order), reads `argc` stack dwords above the
//!    return PC and runs [`invoke`];
//! 2. drops every guard and delivers the completion events it produced to
//!    the kernel through [`Events`] (the kernel relocks guest memory);
//! 3. on success writes EAX, pops the return PC plus `4 * argc`, and either
//!    returns to the caller or first runs each queued stream callback as a
//!    guest stdcall through `ExecCtx::call_guest`, returning afterwards;
//! 4. on a [`Stop`] leaves CPU and stack untouched and stops the runner.
//!    A guest memory fault is a runtime FAULT instead.
use crate::host::{EventSink, GuestMemory, MemFault, StreamCallback};
use crate::overrides::{ENTRIES, Entry, invoke};
use crate::state::{AudioClock, Consumer, ConsumerConflict, DirectSound, Lease, Stop};
use runtime::guest::{Access, FaultReason, MapError, Permissions};
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestFault, Runtime, TransferAbi};
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A kernel event pinned independently of its guest handle. Dropping it
/// releases the pin. Called and dropped with no guest memory or DirectSound
/// guard held.
pub trait EventRef: Send {
    /// Signals the event even if the guest has since closed its handle.
    fn signal(&mut self) -> Result<(), String>;
}

/// Kernel event signalling for `XMEDIAPACKET.hCompletionEvent`. Called with
/// no guest memory or DirectSound guard held.
pub trait Events: Send {
    /// Signals by guest handle. Used when the event was not pinned, or the
    /// pin failed.
    fn signal(&mut self, guest_handle: u32) -> Result<(), String>;
    /// Pins the event behind `guest_handle` when a packet carrying it is
    /// queued, so the completion still reaches it if the guest closes the
    /// handle first. `None` (the default) means this sink does not pin and
    /// completions go through [`signal`](Self::signal). A pin is used for
    /// exactly one completion, in queue order per handle, then dropped; a
    /// packet that never completes drops its pin when its stream is released.
    fn reference(&mut self, _guest_handle: u32) -> Option<Result<Box<dyn EventRef>, String>> {
        None
    }
}

/// Guest addresses this crate maps its allocations in: the upper half of the
/// shared HLE window `0xa000_0000..0xa100_0000` that `bink-hle` documents as
/// `HLE_RESERVED` (bink searches upward from the window's start).
pub const ARENA: std::ops::Range<u32> = 0xa080_0000..0xa100_0000;
/// Pages are mapped on demand in chunks of at least this many bytes.
const CHUNK: u32 = 0x1_0000;

/// Zeroed guest allocations for the objects and buffers DirectSound hands to
/// the game. Mapped lazily inside [`ARENA`]; existing mappings are skipped,
/// never adopted. Freed blocks are reused first-fit and zeroed again.
#[derive(Debug, Default)]
pub struct Arena {
    mapped: Vec<(u32, u32)>,
    /// Bump cursor and end of the last mapped chunk.
    cursor: u32,
    end: u32,
    /// Allocation -> the block it occupies.
    live: HashMap<u32, (u32, u32)>,
    free: Vec<(u32, u32)>,
}

impl Arena {
    /// Allocations not yet freed.
    pub fn live(&self) -> usize {
        self.live.len()
    }
    /// Mapped chunks, `(base, len)`.
    pub fn mapped(&self) -> &[(u32, u32)] {
        &self.mapped
    }

    fn place(base: u32, len: u32, size: u32, align: u32) -> Option<u32> {
        let p = base.checked_add(align - 1)? & !(align - 1);
        (p.checked_add(size)? <= base.checked_add(len)?).then_some(p)
    }

    pub fn alloc(&mut self, guest: &mut Guest, size: u32, align: u32) -> Option<u32> {
        let align = align.max(1);
        if !align.is_power_of_two() {
            return None;
        }
        let size = size.max(1);
        let block = match self
            .free
            .iter()
            .position(|&(b, l)| Self::place(b, l, size, align).is_some())
        {
            Some(i) => self.free.swap_remove(i),
            None => {
                if Self::place(self.cursor, self.end - self.cursor, size, align).is_none() {
                    let len = size.checked_add(align)?.checked_add(CHUNK - 1)? & !(CHUNK - 1);
                    let base = self.map_chunk(guest, len)?;
                    // A gap before a non-contiguous chunk is simply lost.
                    self.cursor = base;
                    self.end = base + len;
                }
                let p = Self::place(self.cursor, self.end - self.cursor, size, align)?;
                let block = (self.cursor, p + size - self.cursor);
                self.cursor = p + size;
                block
            }
        };
        let p = Self::place(block.0, block.1, size, align)?;
        guest
            .write_bytes(block.0, &vec![0; block.1 as usize], 1)
            .ok()?;
        self.live.insert(p, block);
        Some(p)
    }

    /// Unknown or already freed addresses are ignored.
    pub fn free(&mut self, addr: u32) {
        if let Some(block) = self.live.remove(&addr) {
            self.free.push(block);
        }
    }

    fn map_chunk(&mut self, guest: &mut Guest, len: u32) -> Option<u32> {
        let mut va = self.end.max(ARENA.start);
        loop {
            let end = va.checked_add(len).filter(|e| *e <= ARENA.end)?;
            if let Some(m) = guest.mappings().iter().find(|m| {
                (va as u64) < m.virtual_start as u64 + m.len as u64
                    && (end as u64) > m.virtual_start as u64
            }) {
                va = m.virtual_start.checked_add(m.len)?.checked_add(0xfff)? & !0xfff;
                continue;
            }
            match guest.map_zeroed(va, len, Permissions::RW) {
                Ok(()) => {
                    self.mapped.push((va, len));
                    return Some(va);
                }
                Err(MapError::InvalidRange) => va = end,
                Err(_) => return None,
            }
        }
    }
}

/// A [`Stop`] raised by an override, with where it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopRecord {
    pub addr: u32,
    pub name: &'static str,
    pub args: Vec<u32>,
    pub stop: Stop,
}

/// An event signal the kernel refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalFailure {
    pub handle: u32,
    pub error: String,
}

/// Everything the overrides share. Create it once, [`register`] it, and keep
/// the `Arc` to inspect stops, failures and the DirectSound state.
pub struct Hle {
    ds: Mutex<DirectSound>,
    arena: Mutex<Arena>,
    events: Mutex<Box<dyn Events>>,
    /// Callbacks produced outside an override (host-driven `do_work`); the
    /// next override call runs them before returning to its caller.
    callbacks: Mutex<VecDeque<StreamCallback>>,
    /// Pinned events by guest handle, one per queued packet, oldest first.
    pins: Mutex<HashMap<u64, Box<dyn EventRef>>>,
    /// Event operation batches in the order the state produced them. A batch
    /// is published while the `DirectSound` lock is held, so batch order is
    /// state order whatever thread delivers.
    outbox: Mutex<VecDeque<Vec<EventOp>>>,
    /// Held while draining `outbox`, so batches reach the sink in order.
    delivering: Mutex<()>,
    stops: Mutex<Vec<StopRecord>>,
    signal_failures: Mutex<Vec<SignalFailure>>,
}

fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Hle {
    pub fn new(ds: DirectSound, events: Box<dyn Events>) -> Arc<Self> {
        Arc::new(Self {
            ds: Mutex::new(ds),
            arena: Mutex::new(Arena::default()),
            events: Mutex::new(events),
            callbacks: Mutex::new(VecDeque::new()),
            pins: Mutex::new(HashMap::new()),
            outbox: Mutex::new(VecDeque::new()),
            delivering: Mutex::new(()),
            stops: Mutex::new(Vec::new()),
            signal_failures: Mutex::new(Vec::new()),
        })
    }
    /// The DirectSound state. Do not hold this while a runner is active on
    /// another thread longer than needed: every override takes it.
    pub fn direct_sound(&self) -> MutexGuard<'_, DirectSound> {
        lock(&self.ds)
    }
    pub fn arena(&self) -> MutexGuard<'_, Arena> {
        lock(&self.arena)
    }
    /// Every stop so far, oldest first; the last is the one the runner
    /// stopped on.
    pub fn take_stops(&self) -> Vec<StopRecord> {
        std::mem::take(&mut lock(&self.stops))
    }
    pub(crate) fn record_stop(&self, stop: StopRecord) {
        lock(&self.stops).push(stop);
    }
    pub fn take_signal_failures(&self) -> Vec<SignalFailure> {
        std::mem::take(&mut lock(&self.signal_failures))
    }
    pub fn pending_callbacks(&self) -> usize {
        lock(&self.callbacks).len()
    }

    /// Host-driven `DirectSound::do_work`: completes whatever the mixer has
    /// finished (a host render, the app's discard renderer) without waiting
    /// for the game's next `DirectSoundDoWork`. Call with no guest guard held
    /// and no runner on this thread. Events are delivered here; callbacks
    /// wait for the next override call on a guest thread.
    pub fn do_work(&self, runtime: &Runtime) -> Result<(), Stop> {
        self.host_call(runtime, |ds, bridge| ds.do_work(bridge).map(|_| ()))
    }

    /// Controlled audio clock for hosts with no mixer sink: renders `frames`
    /// output frames (discarded), then completes what that finished. See
    /// [`DirectSound::advance_audio`]; the same single-consumer rule applies.
    pub fn advance_audio(&self, runtime: &Runtime, frames: u64) -> Result<(), Stop> {
        self.host_call(runtime, |ds, bridge| {
            ds.advance_audio(bridge, frames).map(|_| ())
        })
    }

    /// [`advance_audio`](Self::advance_audio) for the time `clock` owes after
    /// `elapsed` more host time. The caller owns the clock and the time source.
    pub fn tick(
        &self,
        runtime: &Runtime,
        clock: &mut AudioClock,
        elapsed: Duration,
    ) -> Result<u64, Stop> {
        let frames = clock.advance(elapsed);
        self.advance_audio(runtime, frames)?;
        Ok(frames)
    }

    /// [`DirectSound::render_leased`] from the host, with events delivered
    /// and callbacks owed as for [`Hle::do_work`].
    pub(crate) fn render_leased(
        &self,
        runtime: &Runtime,
        out: &mut [f32],
        lease: &Lease,
    ) -> Result<(), Stop> {
        self.host_call(runtime, |ds, bridge| ds.render_leased(bridge, out, lease))
    }

    fn host_call(
        &self,
        runtime: &Runtime,
        f: impl FnOnce(&mut DirectSound, &mut Bridge<'_>) -> Result<(), Stop>,
    ) -> Result<(), Stop> {
        let (result, finished) = {
            let mut guard = runtime.memory();
            let mut ds = lock(&self.ds);
            let mut arena = lock(&self.arena);
            let mut bridge = Bridge::new(&mut guard, &mut arena);
            let r = f(&mut ds, &mut bridge);
            let mut finished = bridge.finish();
            self.publish(std::mem::take(&mut finished.ops));
            (r, finished)
        };
        self.drain();
        lock(&self.callbacks).extend(finished.callbacks);
        result
    }

    /// Queues one call's event operations. Call with the `DirectSound` lock
    /// still held: that is what orders batches like the state changes that
    /// produced them.
    fn publish(&self, ops: Vec<EventOp>) {
        if !ops.is_empty() {
            lock(&self.outbox).push_back(ops);
        }
    }

    /// Delivers every published batch in order. Runs with no guest or
    /// DirectSound guard held; one drainer at a time, so a concurrent pump
    /// cannot signal a packet before the call that queued it has pinned it.
    fn drain(&self) {
        let _order = lock(&self.delivering);
        loop {
            let batch = lock(&self.outbox).pop_front();
            let Some(batch) = batch else { break };
            self.deliver(batch);
        }
    }

    /// Delivers one batch in the order the shim produced it. Pins are
    /// tracked by packet token, so a failed `reference` for one packet can
    /// never hand another packet's pin to a signal or release.
    fn deliver(&self, ops: Vec<EventOp>) {
        for op in ops {
            match op {
                EventOp::Retain(handle, token) => {
                    let pinned = lock(&self.events).reference(handle);
                    match pinned {
                        Some(Ok(pin)) => {
                            lock(&self.pins).insert(token, pin);
                        }
                        Some(Err(error)) => {
                            lock(&self.signal_failures).push(SignalFailure { handle, error })
                        }
                        None => {}
                    }
                }
                EventOp::Signal(handle, token) => {
                    let pin = lock(&self.pins).remove(&token);
                    let result = match pin {
                        Some(mut pin) => pin.signal(),
                        None => lock(&self.events).signal(handle),
                    };
                    if let Err(error) = result {
                        lock(&self.signal_failures).push(SignalFailure { handle, error });
                    }
                }
                EventOp::Release(_, token) => drop(lock(&self.pins).remove(&token)),
            }
        }
    }

    /// Event pins currently held (one per queued packet with an event).
    pub fn pinned_events(&self) -> usize {
        lock(&self.pins).len()
    }

    /// Starts a thread that drives the audio side from outside the game:
    /// every `period` it completes finished packets ([`Hle::do_work`]), or,
    /// with `render`, first advances the mixer by the real time elapsed
    /// (through an [`AudioClock`] capped at 250 ms) so a host with no sink
    /// still plays buffers out and completes packets. Use `render: false`
    /// when the app's `AudioOut` (a device or its discard renderer) already
    /// consumes the mixer. Stops and joins when the [`Pump`] drops. A
    /// [`Stop`] it hits is recorded in [`Hle::take_stops`] (address 0, name
    /// `"Hle::pump"`) and ends the thread. With `render` it claims the mixer
    /// as [`Consumer::Pump`] (released when the [`Pump`] drops) and fails if
    /// another consumer holds it.
    pub fn spawn_pump(
        self: &Arc<Self>,
        runtime: Arc<Runtime>,
        period: Duration,
        render: bool,
    ) -> Result<Pump, ConsumerConflict> {
        let lease = if render {
            Some(Arc::new(lock(&self.ds).lease_consumer(Consumer::Pump)?))
        } else {
            None
        };
        let thread_lease = lease.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let hle = Arc::clone(self);
        let flag = Arc::clone(&stop);
        let rate = lock(&hle.ds)
            .mixer()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sample_rate();
        let join = std::thread::Builder::new()
            .name("dsound-pump".into())
            .spawn(move || {
                let mut clock = AudioClock::new(rate, Duration::from_millis(250));
                let mut last = Instant::now();
                while !flag.load(Ordering::Acquire) {
                    std::thread::sleep(period);
                    let now = Instant::now();
                    let result = if let Some(lease) = &thread_lease {
                        let frames = clock.advance(now - last);
                        hle.host_call(&runtime, |ds, bridge| {
                            ds.advance_audio_leased(bridge, frames, lease).map(|_| ())
                        })
                    } else {
                        hle.do_work(&runtime)
                    };
                    last = now;
                    if let Err(stop) = result {
                        lock(&hle.stops).push(StopRecord {
                            addr: 0,
                            name: "Hle::pump",
                            args: Vec::new(),
                            stop,
                        });
                        return;
                    }
                }
            })
            .expect("spawning the dsound pump thread");
        Ok(Pump {
            stop,
            join: Some(join),
            claim: lease.map(|l| (Arc::clone(self), l)),
        })
    }
}

/// The thread [`Hle::spawn_pump`] started. Dropping it stops and joins.
pub struct Pump {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    /// Set for a render pump: gives the mixer back on drop.
    claim: Option<(Arc<Hle>, Arc<Lease>)>,
}

impl Pump {
    /// The thread has exited (dropped, or it hit a [`Stop`]).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        if let Some((hle, lease)) = self.claim.take() {
            lock(&hle.ds).release_lease(&lease);
        }
    }
}

/// One event operation, in the order the shim produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EventOp {
    Retain(u32, u64),
    Signal(u32, u64),
    Release(u32, u64),
}

struct Finished {
    ops: Vec<EventOp>,
    callbacks: Vec<StreamCallback>,
    fault: Option<GuestFault>,
}

/// [`crate::Host`] over one held runtime guard and the arena.
struct Bridge<'a> {
    guest: &'a mut Guest,
    arena: &'a mut Arena,
    ops: Vec<EventOp>,
    callbacks: Vec<StreamCallback>,
    fault: Cell<Option<GuestFault>>,
}

impl<'a> Bridge<'a> {
    fn new(guest: &'a mut Guest, arena: &'a mut Arena) -> Self {
        Self {
            guest,
            arena,
            ops: Vec::new(),
            callbacks: Vec::new(),
            fault: Cell::new(None),
        }
    }
    fn record(&self, fault: GuestFault) -> MemFault {
        let m = MemFault {
            addr: fault.addr,
            len: fault.len as u32,
        };
        self.fault.set(Some(fault));
        m
    }
    fn finish(self) -> Finished {
        Finished {
            ops: self.ops,
            callbacks: self.callbacks,
            fault: self.fault.take(),
        }
    }
}

impl GuestMemory for Bridge<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), MemFault> {
        self.guest
            .read_bytes(addr, out, 1)
            .map_err(|f| self.record(f))
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.guest
            .write_bytes(addr, data, 1)
            .map_err(|f| self.record(f))
    }
    fn alloc(&mut self, size: u32, align: u32) -> Option<u32> {
        self.arena.alloc(self.guest, size, align)
    }
    fn free(&mut self, addr: u32) {
        self.arena.free(addr);
    }
}

impl EventSink for Bridge<'_> {
    fn signal_event(&mut self, handle: u32, token: u64) {
        self.ops.push(EventOp::Signal(handle, token));
    }
    fn retain_event(&mut self, handle: u32, token: u64) {
        self.ops.push(EventOp::Retain(handle, token));
    }
    fn release_event(&mut self, handle: u32, token: u64) {
        self.ops.push(EventOp::Release(handle, token));
    }
    fn stream_callback(&mut self, cb: StreamCallback) {
        self.callbacks.push(cb);
    }
}

/// The runtime fault behind a shim `MemFault`; the shim's own span checks
/// (wrap-around, oversized copies) have none and report `AddressOverflow`.
fn guest_fault(m: MemFault, recorded: Option<GuestFault>) -> GuestFault {
    match recorded {
        Some(f) if f.addr == m.addr && f.len as u32 == m.len => f,
        _ => GuestFault {
            addr: m.addr,
            len: m.len as usize,
            access: Access::Read,
            reason: FaultReason::AddressOverflow,
        },
    }
}

/// An address [`register`] refused because something already overrides it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub addr: u32,
    /// Name of the override already installed (possibly disabled).
    pub existing: String,
    pub ours: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    /// Every occupied address, in [`ENTRIES`] order. Nothing was installed.
    /// `existing == ours` everywhere means this set was already registered.
    #[error("dsound overrides not installed: {} addresses already overridden", .0.len())]
    Occupied(Vec<Conflict>),
}

/// What [`register`] installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub installed: Vec<Entry>,
}

impl Registration {
    pub fn addresses(&self) -> Vec<u32> {
        self.installed.iter().map(|e| e.va).collect()
    }
}

/// The dispatch name of an entry.
pub fn override_name(e: &Entry) -> String {
    format!("dsound::{}", e.name)
}

/// Install every entry in [`ENTRIES`], or nothing. Fails closed with
/// [`RegisterError::Occupied`] if any address already has an override,
/// including this crate's own (registering twice).
pub fn register(dispatch: &mut Dispatch, hle: &Arc<Hle>) -> Result<Registration, RegisterError> {
    let conflicts: Vec<_> = ENTRIES
        .iter()
        .filter_map(|e| {
            dispatch.override_name(e.va).map(|existing| Conflict {
                addr: e.va,
                existing: existing.to_owned(),
                ours: override_name(e),
            })
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(RegisterError::Occupied(conflicts));
    }
    for &e in ENTRIES {
        let hle = Arc::clone(hle);
        let previous = dispatch.register_override(e.va, override_name(&e), move |cpu, ctx, _| {
            call(&hle, e, cpu, ctx)
        });
        debug_assert!(previous.is_none(), "checked above");
    }
    Ok(Registration {
        installed: ENTRIES.to_vec(),
    })
}

/// Disable everything [`register`] installed. `Dispatch` has no removal, so
/// the disabled entries keep their names and a later [`register`] reports
/// them; re-enable with `Dispatch::set_enabled`.
pub fn unregister(dispatch: &mut Dispatch, registration: &Registration) {
    for e in &registration.installed {
        dispatch.set_enabled(e.va, false);
    }
}

fn call(
    hle: &Hle,
    e: Entry,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
) -> Result<TransferAbi, GuestFault> {
    let (ret_pc, args, result, finished) = {
        let mut guard = ctx.memory();
        let ret_pc = guard.u32(cpu.esp)?;
        let args = (0..e.argc)
            .map(|i| guard.u32(cpu.esp.wrapping_add(4 + 4 * i)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ds = lock(&hle.ds);
        let mut arena = lock(&hle.arena);
        let mut bridge = Bridge::new(&mut guard, &mut arena);
        let result = invoke(&mut ds, &mut bridge, e.va, &args);
        let mut finished = bridge.finish();
        // Published under the DirectSound lock: batch order is state order.
        hle.publish(std::mem::take(&mut finished.ops));
        (ret_pc, args, result, finished)
    };
    hle.drain();
    match result {
        Ok(r) => {
            cpu.eax = r.eax;
            cpu.esp = cpu.esp.wrapping_add(4).wrapping_add(r.pop_bytes);
            let mut queue = std::mem::take(&mut *lock(&hle.callbacks));
            queue.extend(finished.callbacks);
            run_callbacks(cpu, ctx, queue, r.eax, ret_pc)
        }
        Err(Stop::Memory(m)) => {
            // Packets that completed before the fault still owe callbacks.
            lock(&hle.callbacks).extend(finished.callbacks);
            Err(guest_fault(m, finished.fault))
        }
        Err(stop) => {
            // Callbacks a stopped call produced still belong to the game.
            lock(&hle.callbacks).extend(finished.callbacks);
            let name = override_name(&e);
            let detail = format!("hle: {stop}");
            lock(&hle.stops).push(StopRecord {
                addr: e.va,
                name: e.name,
                args,
                stop,
            });
            let backtrace = ctx.backtrace(cpu.eip);
            Ok(TransferAbi::stop(ctx.emit(Event::Unsupported {
                pc: cpu.eip,
                mnemonic: name,
                detail,
                backtrace,
            })))
        }
    }
}

/// Run `queue` as `callback(stream_context, packet_context, status)` stdcalls
/// in order, then return `eax` to `ret_pc`. The override's own frame is
/// already popped, so each callback runs as if called from the original
/// caller's return site.
fn run_callbacks(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut queue: VecDeque<StreamCallback>,
    eax: u32,
    ret_pc: u32,
) -> Result<TransferAbi, GuestFault> {
    let Some(cb) = queue.pop_front() else {
        cpu.eax = eax;
        return Ok(TransferAbi::ret(ret_pc));
    };
    let runtime = Arc::clone(ctx.runtime());
    let mut guard = runtime.memory();
    let esp = cpu.esp.wrapping_sub(12);
    guard.set_u32(esp, cb.stream_context)?;
    guard.set_u32(esp.wrapping_add(4), cb.packet_context)?;
    guard.set_u32(esp.wrapping_add(8), cb.status)?;
    cpu.esp = esp;
    ctx.call_guest(cpu, &mut guard, cb.function, move |cpu, ctx| {
        run_callbacks(cpu, ctx, queue, eax, ret_pc)
    })
}
