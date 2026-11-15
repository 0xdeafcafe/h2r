//! Registration with `crates/runtime`: every D3D entry point becomes a
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
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), GuestError> {
        match self.guest.write_bytes(addr, data, 1) {
            Ok(()) => Ok(()),
            Err(f) => Err(self.failed(f, addr, data.len())),
        }
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

/// Maps the push buffer and scratch ranges of `cfg` as zeroed read/write
/// guest memory (they are host-chosen; nothing else may overlap them).
pub fn map_device_memory(guest: &mut Guest, cfg: &DeviceConfig) -> Result<(), MapError> {
    guest.map_zeroed(cfg.push_buffer.addr, cfg.push_buffer.len, Permissions::RW)?;
    guest.map_zeroed(cfg.scratch.addr, cfg.scratch.len, Permissions::RW)
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
        if abi::UNIMPLEMENTED.iter().any(|u| u.0 == addr) {
            out.stopping.push((addr, name));
        } else {
            out.implemented.push((addr, name));
        }
        let h = abi::handler(device.clone(), e);
        let dev = device.clone();
        let hook = on_stop.clone();
        let _ = dispatch.register_override(addr, name, move |cpu, ctx, _selector| {
            let regs = regs(cpu);
            let error = {
                let mut guest = ctx.memory();
                let mut mem = GuestAdapter::new(&mut guest);
                let result = h(&regs, &mut mem);
                let fault = mem.take_fault();
                match result {
                    Ok(o) => {
                        apply(cpu, o);
                        let pending = dev
                            .lock()
                            .map(|mut d| d.take_pending_vblanks())
                            .unwrap_or_default();
                        if pending.is_empty() {
                            return guest.ret(cpu, o.cleanup);
                        }
                        // Vblanks counted by Swap: run the callback for each
                        // before returning to the caller (crate::vblank).
                        let ret = guest.ret(cpu, o.cleanup)?;
                        drop(guest);
                        let saved = *cpu;
                        return crate::vblank::deliver_then_return(
                            cpu,
                            ctx,
                            pending.into(),
                            saved,
                            ret.target_pc,
                        );
                    }
                    // A guest access failed, directly or while draining
                    // the push buffer: the runtime's own fault.
                    Err(D3dError::Guest(_) | D3dError::Push(PushError::Guest(_)))
                        if fault.is_some() =>
                    {
                        return Err(fault.expect("checked"));
                    }
                    Err(e) => e,
                }
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
