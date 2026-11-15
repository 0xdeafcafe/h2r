//! Runtime registration of the native XAPI replacements.
//!
//! [`register`] installs one safe override per verified address in
//! [`OVERRIDES`]. Each one holds a single guest memory guard, then the XAPI
//! lock (lock order: guest memory, XAPI, shared VFS), decodes the stdcall
//! frame through [`Xapi::invoke`], and leaves with `TransferAbi::ret` to the
//! popped return PC. Nothing here dispatches guest code with a guard held.
//!
//! [`service_input`] is the per-frame host pump: `sync_input` under the memory
//! guard, `take_signals`, drop every guard, deliver the batch to the kernel
//! through [`Events`], then `acknowledge_signals`.
use crate::{
    Error, Events, GuestMemory, MemoryFault, Result, Storage, ThreadState, Xapi,
    adapter::{Frame, Function, Outcome},
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, GuestFault, Runtime, TransferAbi,
    guest::{Access, FaultReason},
};
use std::{
    cell::Cell,
    sync::{Arc, Mutex, MutexGuard},
};

/// Addresses in the retail Halo 2 XBE (title 4D530064, XDK 5849). See
/// docs/xapi.md for how each one was identified.
pub mod addr {
    pub const XLAUNCH_NEW_IMAGE_A: u32 = 0x002d_07f2;
    pub const XMOUNT_UTILITY_DRIVE: u32 = 0x002d_5b77;
    pub const XFORMAT_UTILITY_DRIVE: u32 = 0x002d_5c7c;
    pub const XAUTO_POWER_DOWN_RESET_TIMER: u32 = 0x002d_6004;
    pub const XAPI_INIT_AUTO_POWER_DOWN: u32 = 0x002d_6031;
    pub const XINIT_DEVICES: u32 = 0x0040_8c57;
    /// `jmp XINIT_DEVICES`; callers reach the same S8 frame through it.
    pub const XINIT_DEVICES_THUNK: u32 = 0x0040_98c0;
    pub const XPEEK_DEVICES: u32 = 0x0040_9884;
    pub const XGET_DEVICE_CHANGES: u32 = 0x0040_98c5;
    pub const XINPUT_OPEN: u32 = 0x0040_9932;
    pub const XINPUT_CLOSE: u32 = 0x0040_9988;
    pub const XINPUT_GET_CAPABILITIES: u32 = 0x0040_9994;
    pub const XINPUT_GET_STATE: u32 = 0x0040_9b6c;
    pub const XINPUT_SET_STATE: u32 = 0x0040_9bdf;
    pub const XREAD_MU_META_DATA: u32 = 0x0041_136f;

    /// The gamepad `XPP_DEVICE_TYPE` object (`push 4086f8h` at `0x124aae`).
    /// Pass it to `Config::for_gamepad`.
    pub const XDEVICE_TYPE_GAMEPAD: u32 = 0x0040_86f8;
    /// `_tls_index`, read by GetLastError/SetLastError at `0x2d1d50`/`0x2d1d7f`.
    pub const TLS_INDEX: u32 = 0x0048_0100;
    /// XAPI's selected cache-database ROW (`[0x461d80]`). The mount path
    /// stores it at `0x2d5ac8` and writes the selected partition index
    /// (`partition - 3`) into that 12-byte row from `0x2d5b22`; `0x2d5d0a`
    /// re-reads the database and indexes it by this row, failing with
    /// STATUS_UNSUCCESSFUL while it is still -1.
    pub const CACHE_ROW: u32 = 0x0046_1d80;
}

/// One override: XBE address, adapter, dispatch name. Every entry is stdcall;
/// the callee pops `4 * function.stack_words()` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Override {
    pub addr: u32,
    pub function: Function,
    pub name: &'static str,
}

const fn entry(addr: u32, function: Function, name: &'static str) -> Override {
    Override {
        addr,
        function,
        name,
    }
}

/// The 15 verified override addresses. XGetDevices and XInputPoll have no
/// verified out-of-line address and are deliberately absent.
pub const OVERRIDES: [Override; 15] = {
    use Function::*;
    use addr::*;
    [
        entry(
            XLAUNCH_NEW_IMAGE_A,
            XLaunchNewImage,
            "xapi::XLaunchNewImageA",
        ),
        entry(
            XMOUNT_UTILITY_DRIVE,
            XMountUtilityDrive,
            "xapi::XMountUtilityDrive",
        ),
        entry(
            XFORMAT_UTILITY_DRIVE,
            XFormatUtilityDrive,
            "xapi::XFormatUtilityDrive",
        ),
        entry(
            XAUTO_POWER_DOWN_RESET_TIMER,
            XAutoPowerDownResetTimer,
            "xapi::XAutoPowerDownResetTimer",
        ),
        entry(
            XAPI_INIT_AUTO_POWER_DOWN,
            XapiInitAutoPowerDown,
            "xapi::XapiInitAutoPowerDown",
        ),
        entry(XINIT_DEVICES, XInitDevices, "xapi::XInitDevices"),
        entry(
            XINIT_DEVICES_THUNK,
            XInitDevices,
            "xapi::XInitDevices (thunk)",
        ),
        entry(XPEEK_DEVICES, XPeekDevices, "xapi::XPeekDevices"),
        entry(
            XGET_DEVICE_CHANGES,
            XGetDeviceChanges,
            "xapi::XGetDeviceChanges",
        ),
        entry(XINPUT_OPEN, XInputOpen, "xapi::XInputOpen"),
        entry(XINPUT_CLOSE, XInputClose, "xapi::XInputClose"),
        entry(
            XINPUT_GET_CAPABILITIES,
            XInputGetCapabilities,
            "xapi::XInputGetCapabilities",
        ),
        entry(XINPUT_GET_STATE, XInputGetState, "xapi::XInputGetState"),
        entry(XINPUT_SET_STATE, XInputSetState, "xapi::XInputSetState"),
        entry(XREAD_MU_META_DATA, XReadMUMetaData, "xapi::XReadMUMetaData"),
    ]
};

/// Guest state the overrides keep coherent with the generated XAPI code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GuestLayout {
    /// `_tls_index`. The current thread's last-error cell is
    /// `[[fs:4] + 4 * [tls_index]] + last_error_offset`, as GetLastError at
    /// `0x2d1d3e` reads it.
    pub tls_index: u32,
    pub last_error_offset: u32,
    /// Published after a successful XMountUtilityDrive; `None` skips it.
    pub cache_row: Option<CacheRow>,
}

/// The row of this title's entry in the kernel's utility-cache database
/// (`\Device\Harddisk0\Partition0` + 0x800). The override does not rewrite
/// the database, so this must name the row the kernel already holds for the
/// title with `Config::utility_partition`. `kernel::rawdisk` seeds exactly
/// one entry, the title's, in row 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CacheRow {
    /// Guest address of the selected-row global.
    pub global: u32,
    pub row: u32,
}

impl Default for GuestLayout {
    /// The retail 5849 image.
    fn default() -> Self {
        Self {
            tls_index: addr::TLS_INDEX,
            last_error_offset: 8,
            cache_row: Some(CacheRow {
                global: addr::CACHE_ROW,
                row: 0,
            }),
        }
    }
}

/// An address [`register`] refused because something already overrides it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub addr: u32,
    /// Name of the override already installed (possibly disabled).
    pub existing: String,
    pub ours: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// Every occupied address, in [`OVERRIDES`] order. Nothing was installed.
    /// `existing == ours` everywhere means this set was already registered.
    Occupied(Vec<Conflict>),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self::Occupied(conflicts) = self;
        write!(f, "xapi overrides not installed; occupied:")?;
        for c in conflicts {
            write!(f, " {:#010x} ({}, wanted {})", c.addr, c.existing, c.ours)?;
        }
        Ok(())
    }
}

impl std::error::Error for RegisterError {}

/// What [`register`] installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub installed: Vec<Override>,
}

impl Registration {
    pub fn addresses(&self) -> Vec<u32> {
        self.installed.iter().map(|o| o.addr).collect()
    }
}

/// Install every override in [`OVERRIDES`], or nothing. Fails closed with
/// [`RegisterError::Occupied`] if any address already has an override
/// (including this crate's own, so registering twice is reported). The
/// power-down pair is part of the same edit, as docs/xapi.md requires.
pub fn register<S: Storage + Send + 'static>(
    dispatch: &mut Dispatch,
    xapi: &Arc<Mutex<Xapi<S>>>,
    layout: GuestLayout,
) -> std::result::Result<Registration, RegisterError> {
    let conflicts: Vec<_> = OVERRIDES
        .iter()
        .filter_map(|o| {
            dispatch.override_name(o.addr).map(|existing| Conflict {
                addr: o.addr,
                existing: existing.to_owned(),
                ours: o.name,
            })
        })
        .collect();
    if !conflicts.is_empty() {
        return Err(RegisterError::Occupied(conflicts));
    }
    for o in OVERRIDES {
        let xapi = Arc::clone(xapi);
        let previous = dispatch.register_override(o.addr, o.name, move |cpu, ctx, _| {
            call(&xapi, layout, o, cpu, ctx)
        });
        debug_assert!(previous.is_none(), "checked above");
    }
    Ok(Registration {
        installed: OVERRIDES.to_vec(),
    })
}

/// Disable everything [`register`] installed. `Dispatch` has no removal, so
/// the disabled entries keep their names: a later [`register`] reports them.
/// Re-enable with `Dispatch::set_enabled` if needed.
pub fn unregister(dispatch: &mut Dispatch, registration: &Registration) {
    for o in &registration.installed {
        dispatch.set_enabled(o.addr, false);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Functions whose failure path stores a Win32 code in the guest last-error
/// cell. Their cell is resolved and write-checked before the service runs.
fn sets_last_error(function: Function) -> bool {
    use Function::*;
    matches!(
        function,
        XInitDevices
            | XGetDeviceChanges
            | XPeekDevices
            | XInputOpen
            | XInputClose
            | XMountUtilityDrive
            | XFormatUtilityDrive
    )
}

fn last_error_cell(
    guest: &Guest,
    fs_base: u32,
    layout: GuestLayout,
) -> std::result::Result<u32, GuestFault> {
    let tls_array = guest.u32(fs_base.wrapping_add(4))?;
    let index = guest.u32(layout.tls_index)?;
    let block = guest.u32(tls_array.wrapping_add(index.wrapping_mul(4)))?;
    let cell = block.wrapping_add(layout.last_error_offset);
    guest.check(cell, 4, Access::Write, 1)?;
    Ok(cell)
}

enum Leave {
    Return(u32),
    Launch,
}

fn call<S: Storage>(
    xapi: &Mutex<Xapi<S>>,
    layout: GuestLayout,
    o: Override,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
) -> std::result::Result<TransferAbi, GuestFault> {
    let leave = {
        let mut guard = ctx.memory();
        let cell = if sets_last_error(o.function) {
            Some(last_error_cell(&guard, cpu.fs_base, layout)?)
        } else {
            None
        };
        let mut thread = ThreadState {
            last_error: match cell {
                Some(cell) => guard.u32(cell)?,
                None => 0,
            },
        };
        let before = thread.last_error;
        let mut frame = Frame {
            eax: cpu.eax,
            esp: cpu.esp,
        };
        let mut memory = RuntimeMemory::new(&mut guard);
        let outcome = lock(xapi).invoke(o.function, &mut frame, &mut memory, &mut thread);
        let outcome = outcome.map_err(|e| memory.guest_fault(e))?;
        if let Some(cell) = cell.filter(|_| thread.last_error != before) {
            guard.set_u32(cell, thread.last_error)?;
        }
        if o.function == Function::XMountUtilityDrive
            && frame.eax == 1
            && let Some(cache) = layout.cache_row
        {
            guard.set_u32(cache.global, cache.row)?;
        }
        match outcome {
            Outcome::Return { pc } => {
                cpu.eax = frame.eax;
                cpu.esp = frame.esp;
                Leave::Return(pc)
            }
            Outcome::Launch => Leave::Launch,
            Outcome::Original => unreachable!("only replacements are registered"),
        }
    };
    Ok(match leave {
        Leave::Return(pc) => TransferAbi::ret(pc),
        // CPU and stack are untouched: the host consumes
        // Xapi::take_launch_request and reloads outside every guard.
        Leave::Launch => TransferAbi::stop(hle_stop(
            ctx,
            cpu.eip,
            o.name,
            "title launch requested; consume Xapi::take_launch_request".into(),
        )),
    })
}

/// Record an HLE stop. The runtime has no HLE-specific event yet, so this uses
/// `Event::Unsupported` with an `hle:` detail and the override name as the
/// mnemonic; `ExecCtx::report` renders it with the guest backtrace.
fn hle_stop(ctx: &mut ExecCtx, pc: u32, name: &str, detail: String) -> u32 {
    let backtrace = ctx.backtrace(pc);
    ctx.emit(Event::Unsupported {
        pc,
        mnemonic: name.to_owned(),
        detail: format!("hle: {detail}"),
        backtrace,
    })
}

/// [`GuestMemory`] over one held runtime guard. Accesses use alignment 1 and
/// keep the runtime's own fault so the reason survives the XAPI error type.
pub struct RuntimeMemory<'a> {
    guest: &'a mut Guest,
    fault: Cell<Option<GuestFault>>,
}

impl<'a> RuntimeMemory<'a> {
    pub fn new(guest: &'a mut Guest) -> Self {
        Self {
            guest,
            fault: Cell::new(None),
        }
    }

    fn record(&self, fault: GuestFault) -> Error {
        let error = Error::Memory(MemoryFault {
            address: fault.addr,
            length: fault.len,
            write: fault.access == Access::Write,
        });
        self.fault.set(Some(fault));
        error
    }

    /// The runtime fault behind an XAPI error. Address arithmetic that
    /// overflowed inside the adapter has no runtime fault; it is reported as
    /// `AddressOverflow`.
    pub fn guest_fault(&self, error: Error) -> GuestFault {
        match error {
            Error::Memory(m) => match self.fault.take() {
                Some(f)
                    if f.addr == m.address
                        && f.len == m.length
                        && (f.access == Access::Write) == m.write =>
                {
                    f
                }
                _ => GuestFault {
                    addr: m.address,
                    len: m.length,
                    access: if m.write { Access::Write } else { Access::Read },
                    reason: FaultReason::AddressOverflow,
                },
            },
            // invoke maps every Win32 error to a guest result itself.
            Error::Win32(_) => GuestFault {
                addr: 0,
                len: 0,
                access: Access::Read,
                reason: FaultReason::InvalidOperation,
            },
        }
    }
}

impl GuestMemory for RuntimeMemory<'_> {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()> {
        let access = if write { Access::Write } else { Access::Read };
        self.guest
            .check(address, length, access, 1)
            .map_err(|f| self.record(f))
    }

    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<()> {
        self.guest
            .read_bytes(address, bytes, 1)
            .map_err(|f| self.record(f))
    }

    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        self.guest
            .write_bytes(address, bytes, 1)
            .map_err(|f| self.record(f))
    }
}

/// The per-frame input pump. Call it between guest slices, with no guest
/// memory, XAPI or VFS guard held and no runner active on this thread:
///
/// 1. lock guest memory, then XAPI; `sync_input` writes completion statuses;
/// 2. `take_signals`;
/// 3. drop both guards;
/// 4. deliver the batch to `events` (the kernel relocks guest memory);
/// 5. lock XAPI again and `acknowledge_signals`.
///
/// Returns the first error after all five steps ran, so a failed write or
/// signal never skips the acknowledgement of the others.
pub fn service_input<S: Storage>(
    runtime: &Runtime,
    xapi: &Mutex<Xapi<S>>,
    events: &mut impl Events,
) -> Result<()> {
    let (written, batch) = {
        let mut guard = runtime.memory();
        let mut x = lock(xapi);
        let written = x.sync_input(&mut RuntimeMemory::new(&mut guard));
        (written, x.take_signals())
    };
    let report = batch.deliver(events);
    let acknowledged = lock(xapi).acknowledge_signals(report);
    written.and(acknowledged)
}
