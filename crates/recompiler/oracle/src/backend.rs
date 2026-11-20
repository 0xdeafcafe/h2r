use std::collections::{BTreeMap, BTreeSet};

use iced_x86::{
    CpuidFeature, Decoder, DecoderError, DecoderOptions, InstructionInfoFactory, Mnemonic,
    OpAccess, Register,
};
use unicorn_engine::{
    RegisterX86 as R, Unicorn,
    unicorn_const::{Arch, HookType, MemType, Mode, Prot, X86CpuModel, uc_error},
};

use crate::{flags, *};

#[derive(Default)]
struct Hooks {
    fault: Option<Fault>,
    pending_writes: BTreeSet<u32>,
    committed_writes: BTreeSet<u32>,
    reads: Vec<MemoryRead>,
}

struct Machine {
    uc: Unicorn<'static, Hooks>,
    initial_pages: BTreeMap<u64, Vec<u8>>,
}

fn backend_error(e: uc_error) -> Stop {
    Stop::BackendError(format!("{e:?}"))
}

fn gpr(r: Register) -> Option<(usize, u32)> {
    let index = match r.full_register32() {
        Register::EAX => 0,
        Register::ECX => 1,
        Register::EDX => 2,
        Register::EBX => 3,
        Register::ESP => 4,
        Register::EBP => 5,
        Register::ESI => 6,
        Register::EDI => 7,
        _ => return None,
    };
    let mask = match r {
        Register::AH | Register::CH | Register::DH | Register::BH => 0xff00,
        _ => match r.size() {
            1 => 0xff,
            2 => 0xffff,
            _ => u32::MAX,
        },
    };
    Some((index, mask))
}

fn load(uc: &mut Unicorn<Hooks>, s: &CpuState) -> Result<(), uc_error> {
    for (r, v) in [
        (R::EAX, s.eax),
        (R::ECX, s.ecx),
        (R::EDX, s.edx),
        (R::EBX, s.ebx),
        (R::ESP, s.esp),
        (R::EBP, s.ebp),
        (R::ESI, s.esi),
        (R::EDI, s.edi),
        (R::EIP, s.eip),
        (R::EFLAGS, s.eflags),
        (R::MXCSR, s.mxcsr),
    ] {
        uc.reg_write(r, v.into())?;
    }
    let f = &s.x87;
    for (r, v) in [
        (R::FPCW, u64::from(f.control)),
        (R::FPSW, f.status.into()),
        (R::FOP, f.opcode.into()),
        (R::FIP, f.instruction_pointer.into()),
        (R::FDP, f.data_pointer.into()),
        (R::FCS, f.code_segment.into()),
        (R::FDS, f.data_segment.into()),
    ] {
        uc.reg_write(r, v)?;
    }
    // Set TOP before logical ST(i), tags after writing the raw values.
    for (n, bytes) in f.st.iter().enumerate() {
        uc.reg_write_long(R::ST0 as i32 + n as i32, bytes)?;
    }
    uc.reg_write(R::FPTAG, f.tag.into())?;
    for (n, bytes) in s.xmm.iter().enumerate() {
        uc.reg_write_long(R::XMM0 as i32 + n as i32, bytes)?;
    }
    Ok(())
}

fn snapshot(uc: &Unicorn<Hooks>) -> Result<CpuState, uc_error> {
    let mut s = CpuState::default();
    for (r, slot) in [
        (R::EAX, &mut s.eax),
        (R::ECX, &mut s.ecx),
        (R::EDX, &mut s.edx),
        (R::EBX, &mut s.ebx),
        (R::ESP, &mut s.esp),
        (R::EBP, &mut s.ebp),
        (R::ESI, &mut s.esi),
        (R::EDI, &mut s.edi),
        (R::EIP, &mut s.eip),
        (R::EFLAGS, &mut s.eflags),
        (R::MXCSR, &mut s.mxcsr),
    ] {
        *slot = uc.reg_read(r)? as u32;
    }
    s.x87.control = uc.reg_read(R::FPCW)? as u16;
    s.x87.status = uc.reg_read(R::FPSW)? as u16;
    s.x87.tag = uc.reg_read(R::FPTAG)? as u16;
    s.x87.opcode = uc.reg_read(R::FOP)? as u16;
    s.x87.instruction_pointer = uc.reg_read(R::FIP)? as u32;
    s.x87.data_pointer = uc.reg_read(R::FDP)? as u32;
    s.x87.code_segment = uc.reg_read(R::FCS)? as u16;
    s.x87.data_segment = uc.reg_read(R::FDS)? as u16;
    for (n, slot) in s.x87.st.iter_mut().enumerate() {
        slot.copy_from_slice(&uc.reg_read_long(R::ST0 as i32 + n as i32)?);
    }
    for (n, slot) in s.xmm.iter_mut().enumerate() {
        slot.copy_from_slice(&uc.reg_read_long(R::XMM0 as i32 + n as i32)?);
    }
    Ok(s)
}

fn range(address: u32, size: usize) -> Result<std::ops::Range<u64>, Stop> {
    let end = u64::from(address)
        .checked_add(size as u64)
        .filter(|&v| v <= 1u64 << 32)
        .ok_or_else(|| Stop::InvalidInput("range wraps 4 GiB".into()))?;
    Ok(u64::from(address)..end)
}

fn pages(r: std::ops::Range<u64>) -> impl Iterator<Item = u64> {
    let start = r.start & !4095;
    let end = if r.is_empty() {
        start
    } else {
        (r.end + 4095) & !4095
    };
    (start..end).step_by(PAGE_SIZE)
}

fn setup(bytes: &[u8], state: &CpuState, mem: &[(u32, Vec<u8>)]) -> Result<Machine, Stop> {
    if state.eflags & (1 << 17) != 0 {
        return Err(Stop::InvalidInput(
            "VM86 is outside the protected-mode harness".into(),
        ));
    }
    if state.mxcsr & !0xffbf != 0 {
        return Err(Stop::InvalidInput(
            "Pentium III MXCSR has reserved bits (including DAZ)".into(),
        ));
    }
    let code = range(state.eip, bytes.len())?;
    // The exclusive end must fit in EIP; avoid a synthetic successful wrap.
    if code.end == 1u64 << 32 {
        return Err(Stop::InvalidInput("code ends at 4 GiB".into()));
    }
    let code_pages: BTreeSet<_> = pages(code.clone()).collect();
    let mut data_pages = BTreeSet::new();
    let mut seeded = Vec::new();
    for (address, bytes) in mem {
        let r = range(*address, bytes.len())?;
        if !r.is_empty() {
            if seeded
                .iter()
                .any(|other: &std::ops::Range<u64>| r.start < other.end && other.start < r.end)
            {
                return Err(Stop::InvalidInput("overlapping memory seeds".into()));
            }
            for p in pages(r.clone()) {
                if code_pages.contains(&p) {
                    return Err(Stop::InvalidInput("data overlaps a code page".into()));
                }
                data_pages.insert(p);
            }
            seeded.push(r);
        }
    }
    let mut uc = Unicorn::new_with_data(Arch::X86, Mode::MODE_32, Hooks::default())
        .map_err(backend_error)?;
    uc.ctl_set_cpu_model(X86CpuModel::PENTIUM3 as i32)
        .map_err(backend_error)?;
    for &p in &code_pages {
        uc.mem_map(p, PAGE_SIZE as u64, Prot::READ | Prot::EXEC)
            .map_err(backend_error)?;
    }
    for &p in &data_pages {
        uc.mem_map(p, PAGE_SIZE as u64, Prot::READ | Prot::WRITE)
            .map_err(backend_error)?;
    }
    if !bytes.is_empty() {
        uc.mem_write(code.start, bytes).map_err(backend_error)?;
    }
    for (address, bytes) in mem {
        if !bytes.is_empty() {
            uc.mem_write((*address).into(), bytes)
                .map_err(backend_error)?;
        }
    }
    // CR0.PE is set by MODE_32; explicitly enable native x87 exceptions and SSE.
    uc.reg_write(R::CR0, 0x21).map_err(backend_error)?;
    uc.reg_write(R::CR4, 0x600).map_err(backend_error)?;
    load(&mut uc, state).map_err(backend_error)?;
    let initial = data_pages
        .into_iter()
        .map(|p| uc.mem_read_as_vec(p, PAGE_SIZE).map(|v| (p, v)))
        .collect::<Result<_, _>>()
        .map_err(backend_error)?;
    uc.add_intr_hook(|uc, vector| {
        uc.get_data_mut().fault = Some(match vector {
            0 => Fault::DivideError,
            6 => Fault::InvalidOpcode,
            _ => Fault::Exception { vector },
        });
        let _ = uc.emu_stop();
    })
    .map_err(backend_error)?;
    uc.add_mem_hook(HookType::MEM_INVALID, 1, 0, |uc, kind, address, _, _| {
        let access = match kind {
            MemType::WRITE_UNMAPPED | MemType::WRITE_PROT => Access::Write,
            MemType::FETCH_UNMAPPED | MemType::FETCH_PROT => Access::Execute,
            _ => Access::Read,
        };
        let protection = matches!(
            kind,
            MemType::READ_PROT | MemType::WRITE_PROT | MemType::FETCH_PROT
        );
        uc.get_data_mut().fault = Some(Fault::PageFault {
            address: address as u32,
            access,
            protection,
        });
        false
    })
    .map_err(backend_error)?;
    uc.add_mem_hook(
        HookType::MEM_READ | HookType::MEM_WRITE,
        1,
        0,
        |uc, kind, address, size, _| {
            if kind == MemType::WRITE {
                for a in address..address + size as u64 {
                    uc.get_data_mut().pending_writes.insert(a as u32);
                }
            } else {
                uc.get_data_mut().reads.push(MemoryRead {
                    address: address as u32,
                    size,
                });
            }
            true
        },
    )
    .map_err(backend_error)?;
    Ok(Machine {
        uc,
        initial_pages: initial,
    })
}

/// Restrict the decoder's advertised features too: QEMU CPU selection does not
/// reliably reject every newer instruction. Deliberately omit system/IO cases.
fn allowed(i: &iced_x86::Instruction) -> bool {
    !i.is_privileged()
        && !i.is_call_far()
        && !i.is_call_far_indirect()
        && !i.is_jmp_far()
        && !i.is_jmp_far_indirect()
        && !(0..i.op_count()).any(|n| i.op_register(n).is_segment_register())
        && i.cpuid_features().iter().all(|f| {
            matches!(
                f,
                CpuidFeature::INTEL8086
                    | CpuidFeature::INTEL186
                    | CpuidFeature::INTEL286
                    | CpuidFeature::INTEL386
                    | CpuidFeature::INTEL486
                    | CpuidFeature::FPU
                    | CpuidFeature::FPU287
                    | CpuidFeature::FPU387
                    | CpuidFeature::CMOV
                    | CpuidFeature::MMX
                    | CpuidFeature::SSE
                    | CpuidFeature::FXSR
                    | CpuidFeature::CX8
            )
        })
        && !matches!(
            i.mnemonic(),
            Mnemonic::Hlt
                | Mnemonic::In
                | Mnemonic::Out
                | Mnemonic::Insb
                | Mnemonic::Insw
                | Mnemonic::Insd
                | Mnemonic::Outsb
                | Mnemonic::Outsw
                | Mnemonic::Outsd
                | Mnemonic::Int
                | Mnemonic::Int3
                | Mnemonic::Into
                | Mnemonic::Iret
                | Mnemonic::Iretd
                | Mnemonic::Lss
                | Mnemonic::Lds
                | Mnemonic::Les
                | Mnemonic::Lfs
                | Mnemonic::Lgs
                | Mnemonic::Popf
                | Mnemonic::Popfd
                | Mnemonic::Cli
                | Mnemonic::Sti
                | Mnemonic::Retf
                | Mnemonic::Sgdt
                | Mnemonic::Sidt
                | Mnemonic::Sldt
                | Mnemonic::Str
                | Mnemonic::Lar
                | Mnemonic::Lsl
                | Mnemonic::Verr
                | Mnemonic::Verw
                | Mnemonic::Arpl
        )
}

pub(super) fn run(bytes: &[u8], state: CpuState, mem: &[(u32, Vec<u8>)]) -> Outcome {
    let mut out = Outcome::initial(state.clone(), Stop::Completed);
    let Machine {
        mut uc,
        initial_pages: initial,
    } = match setup(bytes, &state, mem) {
        Ok(v) => v,
        Err(stop) => {
            out.stop = stop;
            return out;
        }
    };
    let start = u64::from(state.eip);
    let end = start + bytes.len() as u64;
    let mut factory = InstructionInfoFactory::new();
    let mut steps = 0;
    'execute: loop {
        let pc = match uc.reg_read(R::EIP) {
            Ok(v) => v,
            Err(e) => {
                out.stop = backend_error(e);
                break;
            }
        };
        if pc == end {
            break;
        }
        if pc < start || pc >= end {
            out.stop = Stop::EscapedCode;
            break;
        }
        let mut decoder = Decoder::with_ip(
            32,
            &bytes[(pc - start) as usize..],
            pc,
            DecoderOptions::NONE,
        );
        let i = decoder.decode();
        if decoder.last_error() == DecoderError::NoMoreBytes {
            out.stop = Stop::InvalidInput("truncated instruction encoding".into());
            break;
        }
        if i.is_invalid() {
            // Let the independent engine decide #UD for an invalid encoding.
        } else if !allowed(&i) {
            out.stop = Stop::Unsupported(format!("{:?} / {:?}", i.mnemonic(), i.cpuid_features()));
            break;
        }
        let cl = match uc.reg_read(R::ECX) {
            Ok(v) => v as u8,
            Err(e) => {
                out.stop = backend_error(e);
                break;
            }
        };
        let mut effect = flags::effect(&i, cl);
        if matches!(i.mnemonic(), Mnemonic::Shld | Mnemonic::Shrd)
            && effect.undefined == flags::ARITHMETIC
            && i.op0_kind() != iced_x86::OpKind::Register
        {
            out.stop =
                Stop::Unsupported("undefined memory destination for oversized double shift".into());
            break;
        }
        let repeat = i.is_string_instruction() && (i.has_rep_prefix() || i.has_repne_prefix());
        // An empty REP performs no comparison and preserves all flags.
        if repeat {
            let count = uc.reg_read(R::ECX).unwrap_or(0);
            let count = if (0..i.op_count()).any(|n| {
                matches!(
                    i.op_kind(n),
                    iced_x86::OpKind::MemorySegSI | iced_x86::OpKind::MemoryESDI
                )
            }) {
                count & 0xffff
            } else {
                count
            };
            if count == 0 {
                effect = flags::FlagEffect {
                    modified: 0,
                    undefined: 0,
                };
            }
        }
        let info = factory.info(&i);
        let invalid_register = info.used_registers().iter().any(|r| {
            matches!(
                r.access(),
                OpAccess::Read | OpAccess::CondRead | OpAccess::ReadWrite | OpAccess::ReadCondWrite
            ) && gpr(r.register()).is_some_and(|(n, mask)| out.defined_gprs[n] & mask != mask)
        });
        // A zero-count shift/rotate does not read CF even if static metadata says so.
        let read_flags =
            if effect.modified == 0 && matches!(i.mnemonic(), Mnemonic::Rcl | Mnemonic::Rcr) {
                0
            } else {
                flags::from_iced(i.rflags_read())
            };
        if invalid_register || read_flags & !out.defined_flags != 0 {
            out.stop = Stop::UndefinedInput;
            break;
        }
        let mut record = InstructionOutcome {
            eip: pc as u32,
            bytes: bytes[(pc - start) as usize..][..i.len()].to_vec(),
            eflags: 0,
            undefined_flags: effect.undefined,
            defined_flags: out.defined_flags,
            completed: false,
        };
        loop {
            if steps == MAX_STEPS {
                out.stop = Stop::StepLimit;
                record.eflags = uc.reg_read(R::EFLAGS).unwrap_or(0) as u32;
                out.instructions.push(record);
                break 'execute;
            }
            steps += 1;
            uc.get_data_mut().pending_writes.clear();
            let result = uc.emu_start(pc, end, 0, 1);
            if let Some(fault) = uc.get_data_mut().fault.take() {
                out.stop = Stop::Fault(fault);
            } else if let Err(e) = result {
                out.stop = if e == uc_error::INSN_INVALID {
                    Stop::Fault(Fault::InvalidOpcode)
                } else {
                    backend_error(e)
                };
            }
            record.eflags = uc.reg_read(R::EFLAGS).unwrap_or(0) as u32;
            if out.stop != Stop::Completed {
                out.instructions.push(record);
                break 'execute;
            }
            let writes = std::mem::take(&mut uc.get_data_mut().pending_writes);
            uc.get_data_mut().committed_writes.extend(writes);
            out.defined_flags = (out.defined_flags | effect.modified) & !effect.undefined;
            record.defined_flags = out.defined_flags;
            if repeat && uc.reg_read(R::EIP).unwrap_or(pc) == pc {
                continue;
            }
            record.completed = true;
            for r in info.used_registers() {
                if matches!(r.access(), OpAccess::Write | OpAccess::ReadWrite)
                    && let Some((n, mask)) = gpr(r.register())
                {
                    out.defined_gprs[n] |= mask;
                }
            }
            if matches!(i.mnemonic(), Mnemonic::Bsf | Mnemonic::Bsr)
                && record.eflags & flags::ZF != 0
                && let Some((n, mask)) = gpr(i.op0_register())
            {
                out.defined_gprs[n] &= !mask;
            }
            // SDM SHLD/SHRD counts greater than a 16-bit destination make the result undefined too.
            if matches!(i.mnemonic(), Mnemonic::Shld | Mnemonic::Shrd)
                && effect.undefined == flags::ARITHMETIC
                && let Some((n, mask)) = gpr(i.op0_register())
            {
                out.defined_gprs[n] &= !mask;
            }
            out.instructions.push(record);
            break;
        }
    }
    match snapshot(&uc) {
        Ok(s) => out.state = s,
        Err(e) => out.stop = backend_error(e),
    }
    out.reads = std::mem::take(&mut uc.get_data_mut().reads);
    let mut dirty = std::mem::take(&mut uc.get_data_mut().committed_writes);
    // Include partial side effects on faults, not merely successful hook events.
    for (page, before) in initial {
        match uc.mem_read_as_vec(page, PAGE_SIZE) {
            Ok(after) => {
                for (n, (a, b)) in before.iter().zip(&after).enumerate() {
                    if a != b {
                        dirty.insert((page + n as u64) as u32);
                    }
                }
            }
            Err(e) => {
                out.stop = backend_error(e);
                return out;
            }
        }
    }
    for address in dirty {
        if let Some(last) = out.writes.last_mut()
            && u64::from(last.address) + last.bytes.len() as u64 == u64::from(address)
        {
            match uc.mem_read_as_vec(address.into(), 1) {
                Ok(b) => last.bytes.push(b[0]),
                Err(e) => {
                    out.stop = backend_error(e);
                    return out;
                }
            }
        } else {
            match uc.mem_read_as_vec(address.into(), 1) {
                Ok(bytes) => out.writes.push(MemoryWrite { address, bytes }),
                Err(e) => {
                    out.stop = backend_error(e);
                    return out;
                }
            }
        }
    }
    out
}
