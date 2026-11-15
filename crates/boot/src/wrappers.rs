//! Boot stopgaps that extend a kernel HLE handler instead of replacing it:
//! the handler is moved to a page of ours and the wrapper either jumps to
//! it unchanged or composes the call out of supported ones (guest calls
//! through `ExecCtx::call_guest`, so blocking and faults behave as usual).
//!
//! Each is a blocker for crates/kernel, listed in docs/BOOT.md.

use std::sync::Arc;

use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi, guest::Permissions};

use crate::stopgaps::Uses;

pub const WRAP_BASE: u32 = 0xa0ff_1000;

fn push(mem: &mut Guest, cpu: &mut CpuAbi, words: &[u32]) -> Result<(), GuestFault> {
    for w in words.iter().rev() {
        cpu.esp = cpu.esp.wrapping_sub(4);
        mem.set_u32(cpu.esp, *w)?;
    }
    Ok(())
}

const NT_QUERY_INFORMATION_FILE: u32 = 211;
const FILE_NETWORK_OPEN_INFORMATION: u32 = 34;

/// NtQueryInformationFile(FileNetworkOpenInformation) from the kernel's
/// FileBasicInformation (4) and FileStandardInformation (5).
fn network_open(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    inner: u32,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let esp0 = cpu.esp;
    let ret = mem.u32(esp0)?;
    let a: Vec<u32> = (1..=5).map(|i| mem.u32(esp0 + 4 * i)).collect::<Result<_, _>>()?;
    let (handle, iosb, out, len) = (a[0], a[1], a[2], a[3]);
    if len < 56 {
        drop(mem);
        cpu.eax = 0xc000_0004; // STATUS_INFO_LENGTH_MISMATCH
        cpu.esp = esp0 + 24;
        return Ok(TransferAbi::ret(ret));
    }
    // Scratch below the caller's frame; the kernel handlers use no guest stack.
    let basic = esp0 - 0x100;
    let standard = esp0 - 0xc0;
    cpu.esp = esp0 + 24;
    let caller_esp = cpu.esp;
    // Standard first: it needs no access right. Basic needs
    // FILE_READ_ATTRIBUTES in crates/kernel, which XAPI's GENERIC_WRITE
    // open of T:\TitleMeta.xbx lacks; the retail kernel answers anyway, so
    // fall back to zero times and FILE_ATTRIBUTE_NORMAL.
    push(&mut mem, cpu, &[handle, iosb, standard, 24, 5])?;
    ctx.call_guest(cpu, &mut mem, inner, move |cpu, ctx| {
        if cpu.eax != 0 {
            cpu.esp = caller_esp;
            return Ok(TransferAbi::ret(ret));
        }
        let rt = Arc::clone(ctx.runtime());
        let mut mem = rt.memory();
        push(&mut mem, cpu, &[handle, iosb, basic, 40, 4])?;
        ctx.call_guest(cpu, &mut mem, inner, move |cpu, ctx| {
            cpu.esp = caller_esp;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let mut b = [0u8; 56];
            match cpu.eax {
                0 => {
                    for i in 0..32 {
                        b[i] = mem.u8(basic + i as u32)?;
                    }
                    for i in 0..4 {
                        b[48 + i] = mem.u8(basic + 32 + i as u32)?;
                    }
                }
                0xc000_0022 => b[48] = 0x80,
                _ => return Ok(TransferAbi::ret(ret)),
            }
            for i in 0..16 {
                b[32 + i] = mem.u8(standard + i as u32)?;
            }
            mem.write_bytes(out, &b, 1)?;
            mem.set_u32(iosb, 0)?;
            mem.set_u32(iosb + 4, 56)?;
            cpu.eax = 0;
            Ok(TransferAbi::ret(ret))
        })
    })
}

/// Install the wrappers. Call before the tracer, after the kernel.
/// Maps the shadow page every wrapper parks its inner override on.
pub fn map_shadow(rt: &runtime::Runtime) -> anyhow::Result<()> {
    rt.memory().map_zeroed(WRAP_BASE, 0x1000, Permissions::RX)?;
    Ok(())
}

pub fn install(rt: &runtime::Runtime, uses: &Uses) -> anyhow::Result<()> {
    let token = runtime::kernel::function_token(NT_QUERY_INFORMATION_FILE);
    let inner = WRAP_BASE + NT_QUERY_INFORMATION_FILE * 4;
    let u: Uses = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        let prev = d.register_override(token, "boot:NtQueryInformationFile", move |cpu, ctx, _| {
            let class = ctx.runtime().memory().u32(cpu.esp + 20)?;
            if class != FILE_NETWORK_OPEN_INFORMATION {
                return Ok(TransferAbi::jump(inner));
            }
            u.lock().unwrap().entry(NT_QUERY_INFORMATION_FILE).or_insert(("NtQueryInformationFile(34)", 0)).1 += 1;
            network_open(cpu, ctx, inner)
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
    Ok(())
}

/// NtOpenFile (option word 6) and NtCreateFile (option word 9): with no
/// APC delivery, FILE_SYNCHRONOUS_IO_ALERT (0x10) behaves as
/// FILE_SYNCHRONOUS_IO_NONALERT (0x20), the only mode crates/kernel
/// admits. Rewrites the (dead after the call) argument word in place.
pub fn install_alertable(rt: &runtime::Runtime, uses: &Uses) {
    rt.edit_dispatch(|d| {
        for (ord, name, word) in [(202u32, "NtOpenFile", 6u32), (190, "NtCreateFile", 9)] {
            let token = runtime::kernel::function_token(ord);
            let inner = WRAP_BASE + ord * 4;
            let u: Uses = Arc::clone(uses);
            let prev = d.register_override(token, format!("boot:{name}"), move |cpu, ctx, _| {
                let mut mem = ctx.runtime().memory();
                let at = cpu.esp + 4 * word;
                let opt = mem.u32(at)?;
                let _ = name;
                if opt & 0x30 == 0 {
                    // Overlapped (asynchronous) open: crates/kernel only does
                    // synchronous handles, so force FILE_SYNCHRONOUS_IO_NONALERT.
                    mem.set_u32(at, opt | 0x20)?;
                    // Synchronous I/O needs SYNCHRONIZE access, or the open
                    // is STATUS_INVALID_PARAMETER (Halo's DELETE-only opens).
                    let acc = mem.u32(cpu.esp + 8)?;
                    mem.set_u32(cpu.esp + 8, acc | 0x0010_0000)?;
                    let what = if ord == 202 { "NtOpenFile (async forced sync)" } else { "NtCreateFile (async forced sync)" };
                    u.lock().unwrap().entry(ord | 0x8000_0000).or_insert((what, 0)).1 += 1;
                }
                // RootDirectory (HANDLE)-3 is ObDosDevicesDirectory: rebuild
                // the attributes as an absolute "\??\" name on scratch stack.
                let oa_at = cpu.esp + 12;
                let oa = mem.u32(oa_at)?;
                if oa != 0 && mem.u32(oa)? == 0xffff_fffd {
                    let s = mem.u32(oa + 4)?;
                    let (len, buf) = (mem.u16(s)? as u32, mem.u32(s + 4)?);
                    let mut name = b"\\??\\".to_vec();
                    for i in 0..len.min(0x2f0) {
                        name.push(mem.u8(buf + i)?);
                    }
                    let base = cpu.esp - 0x400;
                    mem.set_u32(base, 0)?;
                    mem.set_u32(base + 4, base + 0x10)?;
                    let attrs = mem.u32(oa + 8)?;
                    mem.set_u32(base + 8, attrs)?;
                    mem.set_u16(base + 0x10, name.len() as u16)?;
                    mem.set_u16(base + 0x12, name.len() as u16)?;
                    mem.set_u32(base + 0x14, base + 0x20)?;
                    mem.write_bytes(base + 0x20, &name, 1)?;
                    mem.set_u32(oa_at, base)?;
                    u.lock().unwrap().entry(0x1000 + ord).or_insert(("root -3 (\\??) names", 0)).1 += 1;
                }
                Ok(TransferAbi::jump(inner))
            });
            if let Some(prev) = prev {
                d.restore_override(inner, prev);
            }
        }
    });
}

/// NtCreateMutant / NtReleaseMutant as a binary semaphore through the
/// kernel's own NtCreateSemaphore (193) / NtReleaseSemaphore (222): enough
/// for uncontended and contended non-recursive locking. Recursive
/// acquisition and abandonment are not modelled (blocker: crates/kernel
/// mutant objects).
pub fn install_mutants(rt: &runtime::Runtime, uses: &Uses) {
    rt.edit_dispatch(|d| {
        // (ordinal, name, our words, target ordinal, rewrite)
        let u: Uses = Arc::clone(uses);
        d.register_override(runtime::kernel::function_token(192), "boot:NtCreateMutant", move |cpu, ctx, _| {
            u.lock().unwrap().entry(192).or_insert(("NtCreateMutant (as semaphore)", 0)).1 += 1;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let esp0 = cpu.esp;
            let ret = mem.u32(esp0)?;
            let (h, oa, owner) = (mem.u32(esp0 + 4)?, mem.u32(esp0 + 8)?, mem.u32(esp0 + 12)?);
            cpu.esp = esp0 + 16;
            let caller = cpu.esp;
            push(&mut mem, cpu, &[h, oa, if owner as u8 != 0 { 0 } else { 1 }, 1])?;
            ctx.call_guest(cpu, &mut mem, runtime::kernel::function_token(193), move |cpu, _| {
                cpu.esp = caller;
                Ok(TransferAbi::ret(ret))
            })
        });
        let u: Uses = Arc::clone(uses);
        d.register_override(runtime::kernel::function_token(221), "boot:NtReleaseMutant", move |cpu, ctx, _| {
            u.lock().unwrap().entry(221).or_insert(("NtReleaseMutant (as semaphore)", 0)).1 += 1;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let esp0 = cpu.esp;
            let ret = mem.u32(esp0)?;
            let (h, prev) = (mem.u32(esp0 + 4)?, mem.u32(esp0 + 8)?);
            cpu.esp = esp0 + 12;
            let caller = cpu.esp;
            push(&mut mem, cpu, &[h, 1, prev])?;
            ctx.call_guest(cpu, &mut mem, runtime::kernel::function_token(222), move |cpu, _| {
                cpu.esp = caller;
                Ok(TransferAbi::ret(ret))
            })
        });
    });
}

/// NtQueryFullAttributesFile(ObjectAttributes, FILE_NETWORK_OPEN_INFORMATION*)
/// as NtOpenFile(FILE_READ_ATTRIBUTES | SYNCHRONIZE) + NtQueryInformationFile
/// (class 34, through the wrapper above) + NtClose, all kernel calls on the
/// guest stack (blocker: crates/kernel 210).
pub fn install_full_attributes(rt: &runtime::Runtime, uses: &Uses) {
    let tok = runtime::kernel::function_token;
    let u: Uses = Arc::clone(uses);
    rt.edit_dispatch(|d| {
        d.register_override(tok(210), "boot:NtQueryFullAttributesFile", move |cpu, ctx, _| {
            u.lock().unwrap().entry(210).or_insert(("NtQueryFullAttributesFile (composed)", 0)).1 += 1;
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            let esp0 = cpu.esp;
            let ret = mem.u32(esp0)?;
            let (oa, out) = (mem.u32(esp0 + 4)?, mem.u32(esp0 + 8)?);
            cpu.esp = esp0 + 12;
            let caller = cpu.esp;
            let (handle, iosb) = (esp0 - 0x100, esp0 - 0xf8);
            mem.set_u32(handle, 0)?;
            // NtOpenFile(&h, access, oa, &iosb, share, options)
            push(&mut mem, cpu, &[handle, 0x0010_0080, oa, iosb, 7, 0x20])?;
            ctx.call_guest(cpu, &mut mem, tok(202), move |cpu, ctx| {
                if cpu.eax != 0 {
                    cpu.esp = caller;
                    return Ok(TransferAbi::ret(ret));
                }
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let h = mem.u32(handle)?;
                push(&mut mem, cpu, &[h, iosb, out, 56, FILE_NETWORK_OPEN_INFORMATION])?;
                ctx.call_guest(cpu, &mut mem, tok(NT_QUERY_INFORMATION_FILE), move |cpu, ctx| {
                    let status = cpu.eax;
                    let rt = Arc::clone(ctx.runtime());
                    let mut mem = rt.memory();
                    push(&mut mem, cpu, &[h])?;
                    ctx.call_guest(cpu, &mut mem, tok(187), move |cpu, _| {
                        cpu.esp = caller;
                        cpu.eax = status;
                        Ok(TransferAbi::ret(ret))
                    })
                })
            })
        });
    });
}

/// NtAllocateVirtualMemory: MEM_TOP_DOWN (0x100000) is a placement hint;
/// crates/kernel stops on it. Drop the hint.
pub fn install_top_down(rt: &runtime::Runtime, uses: &Uses) {
    const ORD: u32 = 184;
    rt.edit_dispatch(|d| {
        let token = runtime::kernel::function_token(ORD);
        let inner = WRAP_BASE + ORD * 4;
        let u: Uses = Arc::clone(uses);
        let prev = d.register_override(token, "boot:NtAllocateVirtualMemory", move |cpu, ctx, _| {
            let mut mem = ctx.runtime().memory();
            let at = cpu.esp + 16;
            let ty = mem.u32(at)?;
            if ty & 0x10_0000 != 0 {
                mem.set_u32(at, ty & !0x10_0000)?;
                u.lock().unwrap().entry(ORD).or_insert(("NtAllocateVirtualMemory (MEM_TOP_DOWN dropped)", 0)).1 += 1;
            }
            Ok(TransferAbi::jump(inner))
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
}

/// NtSetInformationFile (226): FileAllocationInformation (19) is a
/// preallocation hint; the kernel rejects it, so succeed without doing anything.
pub fn install_set_allocation(rt: &runtime::Runtime, uses: &Uses) {
    const ORD: u32 = 226;
    rt.edit_dispatch(|d| {
        let token = runtime::kernel::function_token(ORD);
        let inner = WRAP_BASE + ORD * 4;
        let u: Uses = Arc::clone(uses);
        let prev = d.register_override(token, "boot:NtSetInformationFile", move |cpu, ctx, _| {
            let mut mem = ctx.runtime().memory();
            if mem.u32(cpu.esp + 20)? == 19 {
                let iosb = mem.u32(cpu.esp + 8)?;
                mem.set_u32(iosb, 0)?;
                mem.set_u32(iosb + 4, 0)?;
                cpu.eax = 0;
                u.lock().unwrap().entry(ORD).or_insert(("NtSetInformationFile (allocation class ignored)", 0)).1 += 1;
                return mem.ret(cpu, 20);
            }
            Ok(TransferAbi::jump(inner))
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
}

/// ExQueryNonVolatileSetting (24) with XC_MAX_ALL (0xffff), the whole
/// 256-byte EEPROM: crates/kernel stops on it. Answer it the way the boot
/// stopgap did before the kernel round (STATUS_OBJECT_NAME_NOT_FOUND),
/// which the title tolerates.
pub fn install_eeprom_all(rt: &runtime::Runtime, uses: &Uses) {
    const ORD: u32 = 24;
    rt.edit_dispatch(|d| {
        let token = runtime::kernel::function_token(ORD);
        let inner = WRAP_BASE + ORD * 4;
        let u: Uses = Arc::clone(uses);
        let prev = d.register_override(token, "boot:ExQueryNonVolatileSetting", move |cpu, ctx, _| {
            let mem = ctx.runtime().memory();
            if mem.u32(cpu.esp + 4)? == 0xffff {
                cpu.eax = 0xc000_0034;
                u.lock().unwrap().entry(ORD).or_insert(("ExQueryNonVolatileSetting(XC_MAX_ALL) not found", 0)).1 += 1;
                return mem.ret(cpu, 20);
            }
            Ok(TransferAbi::jump(inner))
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
}

/// NtFreeVirtualMemory (199): the kernel stops on releasing or decommitting
/// committed pages ("runtime cannot unmap committed pages"), and every
/// VirtualAlloc Halo makes (XAPI 0x3312cc) commits. Leak instead: MEM_RELEASE
/// with a zero size and MEM_DECOMMIT succeed without unmapping, and the
/// pages stay accessible. Other forms (and bad arguments) go to the kernel.
pub fn install_free_virtual(rt: &runtime::Runtime, uses: &Uses) {
    const ORD: u32 = 199;
    const MEM_DECOMMIT: u32 = 0x4000;
    const MEM_RELEASE: u32 = 0x8000;
    rt.edit_dispatch(|d| {
        let token = runtime::kernel::function_token(ORD);
        let inner = WRAP_BASE + ORD * 4;
        let u: Uses = Arc::clone(uses);
        let prev = d.register_override(token, "boot:NtFreeVirtualMemory", move |cpu, ctx, _| {
            let mut mem = ctx.runtime().memory();
            let (pbase, psize, ty) = (mem.u32(cpu.esp + 4)?, mem.u32(cpu.esp + 8)?, mem.u32(cpu.esp + 12)?);
            let base = mem.u32(pbase)?;
            let size = mem.u32(psize)?;
            if base != 0 && (ty == MEM_RELEASE && size == 0 || ty == MEM_DECOMMIT) {
                mem.set_u32(pbase, base & !0xfff)?;
                cpu.eax = 0;
                u.lock().unwrap().entry(ORD).or_insert(("NtFreeVirtualMemory (leaked, not unmapped)", 0)).1 += 1;
                return mem.ret(cpu, 12);
            }
            Ok(TransferAbi::jump(inner))
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
}

/// AvSendTVEncoderOption (2): the kernel answers only the settings query and
/// stops on anything else. Halo sets the flicker filter (option 0xb, level
/// 3) from 0x14158 after device creation. There is no TV encoder, so a set
/// option is accepted and ignored (`*Result = 0` when it is given).
pub fn install_tv_encoder(rt: &runtime::Runtime, uses: &Uses) {
    const ORD: u32 = 2;
    const AV_OPTION_FLICKER_FILTER: u32 = 0xb;
    rt.edit_dispatch(|d| {
        let token = runtime::kernel::function_token(ORD);
        let inner = WRAP_BASE + ORD * 4;
        let u: Uses = Arc::clone(uses);
        let prev = d.register_override(token, "boot:AvSendTVEncoderOption", move |cpu, ctx, _| {
            let mut mem = ctx.runtime().memory();
            if mem.u32(cpu.esp + 8)? == AV_OPTION_FLICKER_FILTER {
                let result = mem.u32(cpu.esp + 16)?;
                if result != 0 {
                    mem.set_u32(result, 0)?;
                }
                u.lock().unwrap().entry(ORD).or_insert(("AvSendTVEncoderOption (flicker filter ignored)", 0)).1 += 1;
                return mem.ret(cpu, 16);
            }
            Ok(TransferAbi::jump(inner))
        });
        if let Some(prev) = prev {
            d.restore_override(inner, prev);
        }
    });
}

/// NtYieldExecution (238) as a 1 ms KeDelayExecutionThread (99).
///
/// Why: in crates/kernel, a yielding thread is queued Ready with `polling`
/// set, and `Machine::run` advances the clock to the next deadline and runs
/// DPCs while every ready thread is polling, before giving any of them a
/// slice. With a periodic timer DPC armed (XNet's 20 ms timer, DPC
/// 0x332f4c, re-armed from the DPC itself) there is always a next deadline
/// and always a DPC, so the yielding thread never runs again. Halo's movie
/// loop (0x1639c2 -> SwitchToThread 0x2d2084) hits this right after the
/// d:\bink\intro_60.bik probe. A timed delay is a real wait (the thread is
/// not polling), so the scheduler wakes it at its deadline. SwitchToThread
/// maps STATUS_SUCCESS to TRUE ("yielded"), which is what its callers
/// expect after a pause. Owner: kernel (a polling thread must get a slice
/// after one clock advance), docs/BOOT.md.
pub fn install_yield(rt: &runtime::Runtime, uses: &Uses) {
    const ORD: u32 = 238;
    const KE_DELAY_EXECUTION_THREAD: u32 = 99;
    const INTERVAL: u32 = crate::game_glue::BOOT_SCRATCH + 0x100;
    let _ = rt.memory().map_zeroed(crate::game_glue::BOOT_SCRATCH, 0x1000, Permissions::RW);
    rt.edit_dispatch(|d| {
        let token = runtime::kernel::function_token(ORD);
        let delay = runtime::kernel::function_token(KE_DELAY_EXECUTION_THREAD);
        let u: Uses = Arc::clone(uses);
        d.register_override(token, "boot:NtYieldExecution as a 1 ms delay", move |cpu, ctx, _| {
            let mut mem = ctx.runtime().memory();
            u.lock().unwrap().entry(ORD).or_insert(("NtYieldExecution (1 ms delay)", 0)).1 += 1;
            let interval = -10_000i64 as u64; // relative, 1 ms in 100 ns units
            mem.set_u32(INTERVAL, interval as u32)?;
            mem.set_u32(INTERVAL + 4, (interval >> 32) as u32)?;
            let ret = mem.u32(cpu.esp)?;
            cpu.esp = cpu.esp.wrapping_add(4);
            push(&mut mem, cpu, &[ret, 0, 0, INTERVAL])?;
            Ok(TransferAbi::jump(delay))
        });
    });
}
