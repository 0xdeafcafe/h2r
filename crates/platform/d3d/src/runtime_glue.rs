//! Registration with `crates/runtime/runtime`: every D3D entry point becomes a
//! `Dispatch` override, so guest code never runs the original D3D (which
//! programs the NV2A through MMIO, e.g. `0x3fe16b` under CreateDevice).
//!
//! ```ignore
//! let cfg = d3d::DeviceConfig::halo2(push_buffer, scratch);
//! d3d::runtime_glue::map_device_memory(&mut guest, &cfg)?;
//! let device = d3d::runtime_glue::shared(cfg, d3d::GfxBackend::new(ctx));
//! let report = d3d::register(&mut dispatch, device.clone());
//! ```
//!
//! The XBE must be loaded first: CreateDevice reads
//! `D3DSIMPLERENDERSTATEENCODE` from the image
//! ([`crate::pushbuf::SimpleEncode`]).
//!
//! What is overridden ([`Registration`]):
//!
//! * every implemented entry in [`abi::entries`];
//! * every entry in [`abi::UNIMPLEMENTED`]: calling one stops the guest
//!   with a named error rather than running into the NV2A;
//! * the D3D-internal functions that execute GPIO port I/O
//!   ([`abi::PORT_IO`]), so generated code never runs those instructions;
//! * not [`abi::NATIVE`]: pure helpers whose original code is safe to run.
//!
//! Each handler copies `CpuAbi` into [`Regs`], runs the adapter against the
//! locked guest memory, writes `EAX`/`EDX` back and returns with
//! `Guest::ret(cpu, cleanup)`. Failures:
//!
//! * a guest memory fault inside D3D (also while draining the push buffer)
//!   is returned as the runtime's own
//!   `GuestFault` (the runtime records `Event::MemoryFault`);
//! * any other [`D3dError`] is recorded on the device ([`Device::last_stop`])
//!   and handed to the stop hook, whose event id becomes
//!   `TransferAbi::stop(id)`. [`default_stop`] emits the runtime's
//!   `Event::Unsupported` at the entry address: `mnemonic` is the entry
//!   name, `detail` the same text as [`HleStop::error`], and `backtrace`
//!   `ctx.backtrace(entry)`. [`register_with`] takes any other hook.
//!
//! Guest stores made by generated code are not seen here: run the device
//! with [`UploadPolicy::Always`](crate::UploadPolicy::Always) (or `Verify`)
//! until the runtime reports writes to [`Device::note_write`].

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use runtime::guest::{MapError, Permissions};
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestFault, TransferAbi};

use crate::abi::{self, Outcome, Regs, SharedDevice};
use crate::backend::Backend;
use crate::device::{Device, DeviceConfig};
use crate::events::{D3dError, HleStop};
use crate::guest::{GuestError, GuestMemory};
use crate::pushbuf::PushError;

/// [`GuestMemory`] over the runtime's `Guest`. The first runtime fault is
/// kept so it can be returned as itself instead of as a D3D error.
pub struct GuestAdapter<'a> {
    guest: &'a mut Guest,
    fault: Cell<Option<GuestFault>>,
}

impl<'a> GuestAdapter<'a> {
    /// Wraps a locked guest.
    pub fn new(guest: &'a mut Guest) -> Self {
        Self {
            guest,
            fault: Cell::new(None),
        }
    }

    /// The first fault seen, if any.
    pub fn take_fault(&self) -> Option<GuestFault> {
        self.fault.take()
    }

    fn failed(&self, fault: GuestFault, addr: u32, len: usize) -> GuestError {
        let first = self.fault.take().unwrap_or(fault);
        self.fault.set(Some(first));
        GuestError {
            addr,
            len: u32::try_from(len).unwrap_or(u32::MAX),
        }
    }
}

impl GuestMemory for GuestAdapter<'_> {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), GuestError> {
        let len = out.len();
        self.guest
            .read_bytes(addr, out, 1)
            .map_err(|f| self.failed(f, addr, len))
    }
    /// A fixed-size read: a load on the guest's fast path, where the
    /// default (through [`read`](Self::read)'s slice) was a `memmove` call.
    fn read_u32(&self, addr: u32) -> Result<u32, GuestError> {
        let mut b = [0; 4];
        self.guest
            .read_bytes(addr, &mut b, 1)
            .map_err(|f| self.failed(f, addr, 4))?;
        Ok(u32::from_le_bytes(b))
    }
    fn read_append(&self, addr: u32, len: usize, out: &mut Vec<u8>) -> Result<(), GuestError> {
        crate::guest::check_range(addr, len)?;
        self.guest
            .read_append(addr, len, out, 1)
            .map_err(|f| self.failed(f, addr, len))
    }
    /// Copies straight into a fresh vector: no zeroing first.
    fn read_vec(&self, addr: u32, len: usize) -> Result<Vec<u8>, GuestError> {
        let mut v = Vec::new();
        self.read_append(addr, len, &mut v)?;
        Ok(v)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), GuestError> {
        match self.guest.write_bytes(addr, data, 1) {
            Ok(()) => Ok(()),
            Err(f) => Err(self.failed(f, addr, data.len())),
        }
    }
    /// Writes nothing and records nothing when the access is refused.
    fn try_write(&mut self, addr: u32, data: &[u8]) -> bool {
        self.guest.write_bytes(addr, data, 1).is_ok()
    }
}

/// A stop, as the hook sees it.
pub struct Stop<'a> {
    /// Guest address of the entry point.
    pub entry: u32,
    /// Its name.
    pub name: &'static str,
    /// Why.
    pub error: &'a D3dError,
    /// `error` rendered: the text recorded in [`HleStop::error`].
    pub detail: &'a str,
}

/// Emits the event for a stop and returns its id.
pub type StopHook = Arc<dyn Fn(&mut ExecCtx, &Stop<'_>) -> u32 + Send + Sync>;

/// `Event::Unsupported` at the entry: the entry name as the mnemonic, the
/// stop text as the detail, and the guest call chain the runner recorded.
/// The same text is on the device ([`Device::last_stop`]).
pub fn default_stop(ctx: &mut ExecCtx, stop: &Stop<'_>) -> u32 {
    let backtrace = ctx.backtrace(stop.entry);
    ctx.emit(Event::Unsupported {
        pc: stop.entry,
        mnemonic: stop.name.to_owned(),
        detail: stop.detail.to_owned(),
        backtrace,
    })
}

/// What [`register`] installed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registration {
    /// Overrides that implement the entry.
    pub implemented: Vec<(u32, &'static str)>,
    /// Overrides that always stop (named, not implemented).
    pub stopping: Vec<(u32, &'static str)>,
    /// Addresses left to the original code.
    pub native: Vec<(u32, &'static str)>,
}

/// A device ready to share between handlers.
pub fn shared<B: Backend>(cfg: DeviceConfig, backend: B) -> SharedDevice<B> {
    Arc::new(Mutex::new(Device::new(cfg, backend)))
}

/// Maps the push buffer and scratch ranges of `cfg`, and the two pages of
/// the NV2A register window if it has one ([`crate::regs`]), as zeroed
/// read/write guest memory (they are host-chosen; nothing else may overlap
/// them).
pub fn map_device_memory(guest: &mut Guest, cfg: &DeviceConfig) -> Result<(), MapError> {
    guest.map_zeroed(cfg.push_buffer.addr, cfg.push_buffer.len, Permissions::RW)?;
    guest.map_zeroed(cfg.scratch.addr, cfg.scratch.len, Permissions::RW)?;
    if let Some(base) = cfg.nv2a_window {
        let w = crate::regs::RegisterWindow::new(base).map_err(|_| MapError::InvalidRange)?;
        for p in w.pages() {
            guest.map_zeroed(p.addr, p.len, Permissions::RW)?;
        }
    }
    Ok(())
}

/// Installs every D3D override with [`default_stop`].
pub fn register<B>(dispatch: &mut Dispatch, device: SharedDevice<B>) -> Registration
where
    B: Backend + Send + 'static,
{
    register_with(dispatch, device, Arc::new(default_stop))
}

/// Installs every D3D override; stops go through `on_stop`.
pub fn register_with<B>(
    dispatch: &mut Dispatch,
    device: SharedDevice<B>,
    on_stop: StopHook,
) -> Registration
where
    B: Backend + Send + 'static,
{
    let mut out = Registration::default();
    for e in abi::entries::<B>() {
        let (addr, name) = (e.addr, e.name);
        if abi::stops(addr) {
            out.stopping.push((addr, name));
        } else {
            out.implemented.push((addr, name));
        }
        let dev = device.clone();
        let hook = on_stop.clone();
        let _ = dispatch.register_override(addr, name, move |cpu, ctx, _selector| {
            // Frame stats: the D3D entries are their own phase.
            let _phase = timeline::phase::enter(timeline::phase::Phase::D3d);
            let regs = regs(cpu);
            // One memory guard and one device lock for the whole call: the
            // drain, the entry, what it leaves to do and the return (each
            // guard and lock taken again was a gate handshake and a mutex
            // round trip on every D3D call).
            let (error, fault) = {
                // The post guard also checks the context's live return
                // continuation while this guest-memory guard is held.
                let mut guest = ctx.runtime().memory();
                let mut mem = GuestAdapter::new(&mut guest);
                match dev.lock() {
                    Err(_) => (
                        D3dError::Faulted("D3D device mutex poisoned by an earlier panic".into()),
                        mem.take_fault(),
                    ),
                    // Push-buffer callbacks queued before this call run
                    // first, where the GPU would have run them; the entry
                    // itself runs again afterwards (see
                    // `deliver_push_callback`).
                    Ok(mut d) => {
                        // `render.residual` (crate::residual): unarmed, one
                        // relaxed load; armed, the caller and the device's
                        // tallies around the drain and the entry.
                        // The caller a draw with no native stage is
                        // counted under (crate::gap).
                        crate::gap::set_caller(mem.read_u32(regs.esp).unwrap_or(0));
                        let mut watch = crate::residual::armed().then(|| {
                            crate::residual::Watch::start(
                                mem.read_u32(regs.esp).unwrap_or(0),
                                d.residual_tally(),
                            )
                        });
                        // An armed swallow checks the entry's caller before
                        // the draws its drain executes.
                        let ret = d
                            .swallowing()
                            .then(|| mem.read_u32(regs.esp).unwrap_or(0));
                        let drain = ret.map_or(Ok(()), |ret| {
                            d.swallow_caller(&mem, regs.esp, ret, |token| {
                                ctx.live_continuations()
                                    .iter()
                                    .any(|c| c.token == token && c.origin == "feature: after hooks")
                            })
                        })
                        .and_then(|()| d.drain_until_callback(&mut mem));
                        if let Some(w) = &mut watch {
                            w.drained(d.residual_tally());
                        }
                        match drain {
                            Ok(Some(cb)) => {
                                if let Some(w) = watch {
                                    crate::residual::record(&w.finish(addr, name, d.residual_tally(), false));
                                }
                                drop(d);
                                drop(guest);
                                return deliver_push_callback(cpu, ctx, &dev, cb, addr);
                            }
                            Ok(None) => match d
                                .swallow_before_entry(e.name)
                                .and_then(|()| d.call_residual_entry(&e, &mut mem, &regs))
                            {
                                Ok(o) => {
                                    // Always on: an integer add in this
                                    // thread's frame (one relaxed load and
                                    // nothing else when frame stats are off).
                                    timeline::phase::count(timeline::phase::Counter::D3dEntries, 1);
                                    if let Some(w) = watch {
                                        crate::residual::record(&w.finish(addr, name, d.residual_tally(), true));
                                    }
                                    apply(cpu, o);
                                    let destroys = d.take_pending_destroys();
                                    let vblanks = d.take_pending_vblanks();
                                    drop(d);
                                    if destroys.is_empty() && vblanks.is_empty() {
                                        return guest.ret(cpu, o.cleanup);
                                    }
                                    // Resources the entry destroyed go to the
                                    // original D3D_DestroyResource, and vblanks
                                    // counted by Swap to the callback
                                    // (crate::vblank), before the caller resumes.
                                    let ret = guest.ret(cpu, o.cleanup)?;
                                    drop(guest);
                                    let saved = *cpu;
                                    return destroy_then_return(
                                        cpu,
                                        ctx,
                                        destroys.into(),
                                        vblanks.into(),
                                        saved,
                                        ret.target_pc,
                                    );
                                }
                                Err(err) => {
                                    d.swallow_entry_failed(e.name);
                                    (err, mem.take_fault())
                                }
                            },
                            Err(err) => {
                                d.swallow_entry_failed(e.name);
                                (err, mem.take_fault())
                            }
                        }
                    }
                }
            };
            let error = match (error, fault) {
                // A guest access failed, directly or while draining the
                // push buffer: the runtime's own fault.
                (D3dError::Guest(_) | D3dError::Push(PushError::Guest(_)), Some(fault)) => {
                    return Err(fault);
                }
                (e, _) => e,
            };
            let detail = error.to_string();
            if let Ok(mut d) = dev.lock() {
                d.note_stop(HleStop {
                    entry: addr,
                    name,
                    error: detail.clone(),
                });
            }
            let id = hook(
                ctx,
                &Stop {
                    entry: addr,
                    name,
                    error: &error,
                    detail: &detail,
                },
            );
            Ok(TransferAbi::stop(id))
        });
    }
    out.native = abi::NATIVE.iter().map(|n| (n.0, n.1)).collect();
    out
}

/// Calls a push-buffer callback ([`crate::device::PushCallback`]) the way
/// D3D's interrupt handler does (`0x3ff3c5`: `function(context)`, cdecl),
/// then enters `entry` again with every register as it was, so the entry
/// drains the rest of the queue and runs.
///
/// The argument goes just below the entry's `ESP` (above it are the
/// caller's return address and arguments, untouched) and the callback
/// starts with the direction flag clear.
///
/// The device is marked as delivering `cb` until the callback returns
/// ([`Device::begin_push_callback`]): a D3D entry called from the callback
/// stops with a named error instead of draining the work queued after it.
/// If the callback never returns (an unwind), the mark stays and every
/// later D3D call stops the same way.
pub fn deliver_push_callback<B: Backend + Send + 'static>(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    device: &SharedDevice<B>,
    cb: crate::device::PushCallback,
    entry: u32,
) -> Result<TransferAbi, GuestFault> {
    const DIRECTION_FLAG: u32 = 1 << 10;
    let saved = *cpu;
    let runtime = Arc::clone(ctx.runtime());
    let mut guest = runtime.memory();
    let esp = saved.esp.wrapping_sub(4);
    guest.set_u32(esp, cb.context)?;
    cpu.esp = esp;
    cpu.eflags &= !DIRECTION_FLAG;
    if let Ok(mut d) = device.lock() {
        d.begin_push_callback(cb);
    }
    let dev = device.clone();
    let call = ctx.call_guest(cpu, &mut guest, cb.function, move |cpu, _ctx| {
        if let Ok(mut d) = dev.lock() {
            d.end_push_callback();
        }
        // cdecl: the callback leaves its argument; the restore drops it.
        *cpu = saved;
        Ok(TransferAbi::jump(entry))
    });
    // Not entered (a fault or the continuation limit): nothing to wait for.
    if !matches!(&call, Ok(t) if t.kind == runtime::abi::CALL)
        && let Ok(mut d) = device.lock()
    {
        d.end_push_callback();
    }
    call
}

/// The original `D3D_DestroyResource` (`0x3fb520`; [`abi::NATIVE`]).
pub const DESTROY_RESOURCE: u32 = 0x3fb520;

/// Calls the original `D3D_DestroyResource` (`EDI` = resource; `ret`) for
/// each header the entry destroyed ([`Device::destroy_resource`]), oldest
/// first, then delivers `vblanks` ([`crate::vblank::deliver_then_return`])
/// and returns to `ret_pc` with every register as in `saved` (the entry's
/// results applied, its frame popped). Each call runs below `saved.esp`,
/// with the direction flag clear.
pub fn destroy_then_return(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut destroys: std::collections::VecDeque<u32>,
    vblanks: std::collections::VecDeque<crate::device::VBlank>,
    saved: CpuAbi,
    ret_pc: u32,
) -> Result<TransferAbi, GuestFault> {
    const DIRECTION_FLAG: u32 = 1 << 10;
    let Some(header) = destroys.pop_front() else {
        return crate::vblank::deliver_then_return(cpu, ctx, vblanks, saved, ret_pc);
    };
    let runtime = Arc::clone(ctx.runtime());
    let mut guest = runtime.memory();
    *cpu = saved;
    cpu.edi = header;
    cpu.eflags &= !DIRECTION_FLAG;
    ctx.call_guest(cpu, &mut guest, DESTROY_RESOURCE, move |cpu, ctx| {
        destroy_then_return(cpu, ctx, destroys, vblanks, saved, ret_pc)
    })
}

fn regs(cpu: &CpuAbi) -> Regs {
    Regs {
        eax: cpu.eax,
        ecx: cpu.ecx,
        edx: cpu.edx,
        ebx: cpu.ebx,
        esi: cpu.esi,
        edi: cpu.edi,
        esp: cpu.esp,
    }
}

fn apply(cpu: &mut CpuAbi, o: Outcome) {
    if let Some(v) = o.eax {
        cpu.eax = v;
    }
    if let Some(v) = o.edx {
        cpu.edx = v;
    }
}
