//! The media objects in the runtime (docs/live/live-voice.md, 3.3).
//!
//! [`register`] puts the host's [`MediaObjects`] under the title's own XPP
//! code at four addresses:
//!
//! | Address | Original | Here |
//! |---|---|---|
//! | `0x411143` | XVoiceCreateMediaObjectEx (`ret 1Ch`) | [`MediaObjects::create`]; the object is the original's static slot |
//! | `0x410e77` | XMO Process (`ret 0Ch`) | [`MediaObjects::process`] |
//! | `0x4112dc` | XMO Release (`ret 4`) | refcount at +0xc; [`MediaObjects::release`] at zero |
//! | `0x332f4c` | the engine's 20 ms DPC (`ret 10h`) | [`MediaObjects::tick`], each completion's callback, then the original DPC |
//!
//! The object a caller gets is the slot XPP itself would have used: the
//! microphone's at `0x4842b0 + port * 0x1c`, the headphone's at `0x484320 +
//! port * 0x1c`, with the original vtable (`0x417544`) at +0, the refcount
//! at +0xc, and the callback and context at +0x10 and +0x14, as
//! `0x411143` stores them. AddRef (`0x41058f`) stays generated: it only
//! increments +0xc. GetInfo, GetStatus, Discontinuity and Flush stay
//! generated too; the engine never calls them.
//!
//! Completions are delivered at the start of the engine's own DPC, so a
//! packet finishes every 20 ms of guest time, on the engine's cadence, and
//! the callback runs in the DPC as XPP's USB completion would. A completion
//! with an event instead of a callback is not delivered (the engine always
//! passes a callback).

use crate::{
    io::VoiceIo,
    xmo::{Completion, Delivery, Fault, Format, Handle, Kind, MediaObjects, Memory, hresult},
};
use runtime::{
    CpuAbi, ExecCtx, Guest, GuestFault, Runtime, TransferAbi,
    exec::OriginalHandler,
    guest::{Access, FaultReason},
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};

/// Addresses in the retail 5849 image.
pub mod addr {
    pub const CREATE: u32 = 0x0041_1143;
    pub const PROCESS: u32 = 0x0041_0e77;
    pub const RELEASE: u32 = 0x0041_12dc;
    pub const ENGINE_DPC: u32 = 0x0033_2f4c;
    pub const VTABLE: u32 = 0x0041_7544;
    pub const MICROPHONE_TYPE: u32 = 0x0040_88b0;
    pub const HEADPHONE_TYPE: u32 = 0x0040_88bc;
    pub const MICROPHONE_SLOTS: u32 = 0x0048_42b0;
    pub const HEADPHONE_SLOTS: u32 = 0x0048_4320;
    pub const SLOT_SIZE: u32 = 0x1c;
}

/// What the overrides share: the objects, the host devices, and which
/// guest slot is which object.
pub struct Voice {
    pub objects: MediaObjects,
    pub io: Box<dyn VoiceIo>,
    slots: BTreeMap<u32, Handle>,
}

impl Voice {
    pub fn new(io: Box<dyn VoiceIo>) -> Self {
        Self {
            objects: MediaObjects::new(),
            io,
            slots: BTreeMap::new(),
        }
    }

    /// The guest slot of an open object.
    pub fn slot(&self, handle: Handle) -> Option<u32> {
        self.slots
            .iter()
            .find_map(|(&slot, &h)| (h == handle).then_some(slot))
    }
}

pub type Shared = Arc<Mutex<Voice>>;

fn lock(v: &Mutex<Voice>) -> MutexGuard<'_, Voice> {
    v.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why [`register`] installed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// Something already overrides this address.
    Occupied { addr: u32, existing: String },
    /// No generated code at the engine DPC to run after the completions.
    NoOriginalDpc,
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupied { addr, existing } => {
                write!(f, "voice: {addr:#010x} already has the override {existing}")
            }
            Self::NoOriginalDpc => write!(f, "voice: no generated engine DPC at 0x332f4c"),
        }
    }
}

impl std::error::Error for RegisterError {}

/// Install the four overrides, or nothing.
pub fn register(runtime: &Runtime, voice: &Shared) -> Result<(), RegisterError> {
    // Resolve before edit_dispatch: resolve_original takes the dispatch read
    // lock, which edit_dispatch holds for writing.
    let dpc = runtime
        .resolve_original(addr::ENGINE_DPC)
        .ok_or(RegisterError::NoOriginalDpc)?;
    let dpc = Arc::new(dpc);
    let all = [addr::CREATE, addr::PROCESS, addr::RELEASE, addr::ENGINE_DPC];
    runtime.edit_dispatch(|d| {
        for a in all {
            if let Some(existing) = d.override_name(a) {
                return Err(RegisterError::Occupied {
                    addr: a,
                    existing: existing.to_owned(),
                });
            }
        }
        let v = Arc::clone(voice);
        d.register_override(
            addr::CREATE,
            "voice::XVoiceCreateMediaObjectEx",
            move |cpu, ctx, _| stdcall(cpu, ctx, 7, |g, args| create(&v, g, args)),
        );
        let v = Arc::clone(voice);
        d.register_override(
            addr::PROCESS,
            "voice::XMediaObject::Process",
            move |cpu, ctx, _| stdcall(cpu, ctx, 3, |g, args| process(&v, g, args)),
        );
        let v = Arc::clone(voice);
        d.register_override(
            addr::RELEASE,
            "voice::XMediaObject::Release",
            move |cpu, ctx, _| stdcall(cpu, ctx, 1, |g, args| release(&v, g, args[0])),
        );
        let v = Arc::clone(voice);
        d.register_override(addr::ENGINE_DPC, "voice::engine DPC", move |cpu, ctx, _| {
            let done = {
                let mut g = ctx.memory();
                let mut voice = lock(&v);
                let Voice { objects, io, .. } = &mut *voice;
                let mut m = GuestMemory::new(&mut g);
                let done = objects.tick(&mut m, io.as_mut());
                let done = done.map_err(|f| m.fault(f))?;
                for c in &done {
                    c.write(&mut m).map_err(|f| m.fault(f))?;
                }
                done
            };
            deliver(cpu, ctx, done.into_iter(), Arc::clone(&dpc))
        });
        Ok(())
    })
}

/// Run each completion's callback, then the original DPC with the CPU as
/// it entered.
fn deliver(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut rest: std::vec::IntoIter<Completion>,
    dpc: Arc<OriginalHandler>,
) -> Result<TransferAbi, GuestFault> {
    loop {
        let Some(c) = rest.next() else {
            return Ok(dpc.invoke(cpu, ctx, addr::ENGINE_DPC));
        };
        let Delivery::Callback {
            function,
            context,
            packet_context,
            status,
        } = c.delivery
        else {
            continue;
        };
        let runtime = Arc::clone(ctx.runtime());
        let mut g = runtime.memory();
        let esp = cpu.esp.wrapping_sub(12);
        g.set_u32(esp, context)?;
        g.set_u32(esp.wrapping_add(4), packet_context)?;
        g.set_u32(esp.wrapping_add(8), status)?;
        cpu.esp = esp;
        return ctx.call_guest(cpu, &mut g, function, move |cpu, ctx| {
            deliver(cpu, ctx, rest, dpc)
        });
    }
}

/// Decode a stdcall frame of `words` arguments, run `f` with guest memory
/// held, return its value in EAX and pop the frame.
fn stdcall(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    words: usize,
    f: impl FnOnce(&mut Guest, &[u32]) -> Result<u32, GuestFault>,
) -> Result<TransferAbi, GuestFault> {
    let mut g = ctx.memory();
    let pc = g.u32(cpu.esp)?;
    let mut args = [0u32; 7];
    for (i, a) in args.iter_mut().take(words).enumerate() {
        *a = g.u32(cpu.esp.wrapping_add(4 + 4 * i as u32))?;
    }
    cpu.eax = f(&mut g, &args[..words])?;
    cpu.esp = cpu.esp.wrapping_add(4 + 4 * words as u32);
    Ok(TransferAbi::ret(pc))
}

/// XVoiceCreateMediaObjectEx(type, port, max packets, format, callback,
/// context, out).
fn create(voice: &Mutex<Voice>, g: &mut Guest, a: &[u32]) -> Result<u32, GuestFault> {
    let [ty, port, max_packets, format, callback, context, out] = a.try_into().unwrap();
    let (kind, slots) = match ty {
        addr::MICROPHONE_TYPE => (Kind::Microphone, addr::MICROPHONE_SLOTS),
        addr::HEADPHONE_TYPE => (Kind::Headphone, addr::HEADPHONE_SLOTS),
        // The third XPP type (0x4088c8) and anything else: no such device.
        _ => return Ok(hresult::DEVICE_NOT_CONNECTED),
    };
    if format == 0 || out == 0 {
        return Ok(hresult::E_INVALIDARG);
    }
    let mut bytes = [0; Format::SIZE];
    g.read_bytes(format, &mut bytes, 1)?;
    let mut voice = lock(voice);
    let handle = match voice.objects.create(
        kind,
        port,
        max_packets,
        Format::parse(&bytes),
        callback,
        context,
    ) {
        Ok(h) => h,
        Err(hr) => return Ok(hr),
    };
    let slot = slots + port * addr::SLOT_SIZE;
    let mut object = [0u8; addr::SLOT_SIZE as usize];
    object[0..4].copy_from_slice(&addr::VTABLE.to_le_bytes());
    object[0x0b] = 0x22;
    object[0x0c..0x10].copy_from_slice(&1u32.to_le_bytes());
    object[0x10..0x14].copy_from_slice(&callback.to_le_bytes());
    object[0x14..0x18].copy_from_slice(&context.to_le_bytes());
    let written = g
        .write_bytes(slot, &object, 1)
        .and_then(|()| g.set_u32(out, slot));
    if let Err(fault) = written {
        voice.objects.release(handle);
        return Err(fault);
    }
    voice.slots.insert(slot, handle);
    Ok(hresult::S_OK)
}

/// Process(this, src, dst).
fn process(voice: &Mutex<Voice>, g: &mut Guest, a: &[u32]) -> Result<u32, GuestFault> {
    let mut voice = lock(voice);
    let Some(&handle) = voice.slots.get(&a[0]) else {
        return Ok(hresult::E_INVALIDARG);
    };
    let mut m = GuestMemory::new(g);
    voice
        .objects
        .process(&mut m, handle, a[1], a[2])
        .map_err(|f| m.fault(f))
}

/// Release(this): the new count; the object goes at zero. Queued packets
/// are dropped without their callbacks, since the engine releases while it
/// tears the talker down (0x334bc6).
fn release(voice: &Mutex<Voice>, g: &mut Guest, this: u32) -> Result<u32, GuestFault> {
    let count = g.u32(this.wrapping_add(0xc))?.wrapping_sub(1);
    g.set_u32(this.wrapping_add(0xc), count)?;
    if count == 0 {
        let mut voice = lock(voice);
        if let Some(handle) = voice.slots.remove(&this) {
            voice.objects.release(handle);
        }
    }
    Ok(count)
}

/// [`Memory`] over a held guard, keeping the runtime's fault.
struct GuestMemory<'a> {
    guest: &'a mut Guest,
    last: Cell<Option<GuestFault>>,
}

impl<'a> GuestMemory<'a> {
    fn new(guest: &'a mut Guest) -> Self {
        Self {
            guest,
            last: Cell::new(None),
        }
    }

    fn keep(&self, f: GuestFault) -> Fault {
        let fault = Fault {
            address: f.addr,
            length: f.len,
            write: f.access == Access::Write,
        };
        self.last.set(Some(f));
        fault
    }

    fn fault(&self, f: Fault) -> GuestFault {
        self.last.take().unwrap_or(GuestFault {
            addr: f.address,
            len: f.length,
            access: if f.write { Access::Write } else { Access::Read },
            reason: FaultReason::InvalidOperation,
        })
    }
}

impl Memory for GuestMemory<'_> {
    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<(), Fault> {
        self.guest
            .read_bytes(address, bytes, 1)
            .map_err(|f| self.keep(f))
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Fault> {
        self.guest
            .write_bytes(address, bytes, 1)
            .map_err(|f| self.keep(f))
    }
}
