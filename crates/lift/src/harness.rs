//! Run the image from its entry point on the IR interpreter until the
//! first kernel call (`docs/LIFTER.md` §9).
//!
//! This is a semantic check of the lowering, not the runtime: it loads the
//! image into one flat 64 MB memory, gives the entry thread a stack, and
//! interprets instruction by instruction. Calls are followed into their
//! callees (the return address is in guest memory, so `ret` just works);
//! the run stops at the first call through a kernel import slot, before any
//! kernel behaviour would be needed.
//!
//! Code comes from the CFG-driven [`Lifter`], function by function, exactly
//! as a backend gets it: reaching an address no lifted function contains is
//! a stop ([`Stop::NotLifted`]), not a fallback to lowering it alone.

use std::collections::HashMap;

use ir::interp::{self, Cpu, Exit, FlatMemory, Flow};
use ir::{CallKind, Control, Gpr, IrInsn};
use xbe::Xbe;

use crate::Lifter;

/// Guest RAM size; the image and the stack both live inside it.
pub const RAM: usize = 64 << 20;
/// Top of the entry thread's stack, just below the end of RAM.
pub const STACK_TOP: u32 = (RAM as u32) - 0x1000;
/// Return address planted under the entry's frame. Returning to it is the
/// one synthetic successful exit.
pub const HARNESS_EXIT: u32 = 0xFFFF_FFFC;

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// A call through a kernel import slot. `target` is what the slot held
    /// (the unpatched `0x80000000 | ordinal` in a raw image).
    KernelCall { pc: u32, slot: u32, ordinal: u32, target: u32, continuation: u32 },
    /// The interpreter stopped: fault, unsupported instruction, ...
    Exit(Exit),
    /// Execution reached an address that no lifted function contains (not
    /// a discovered function's entry, and not reached from one).
    NotLifted(u32),
    /// The entry function returned to [`HARNESS_EXIT`].
    Returned,
    StepLimit,
}

pub struct Run {
    pub stop: Stop,
    pub steps: usize,
    pub cpu: Cpu,
    pub mem: FlatMemory,
    /// Distinct instruction addresses executed.
    pub executed: usize,
    /// Entries of the functions lifted to run it, in the order reached.
    pub functions: Vec<u32>,
}

/// Load headers and sections the way the Xbox loader lays them out:
/// headers at the base address, each section's raw data at its virtual
/// address, virtual tails zero.
pub fn load_image(xbe: &Xbe) -> Result<FlatMemory, String> {
    let mut mem = FlatMemory::new(0, RAM);
    let headers = xbe.data().get(..xbe.size_of_headers as usize).ok_or("headers past end of file")?;
    if !mem.load(xbe.base, headers) {
        return Err("headers don't fit in RAM".into());
    }
    for s in &xbe.sections {
        let raw = xbe.section_bytes(s).map_err(|e| e.to_string())?;
        let raw = &raw[..raw.len().min(s.vsize as usize)];
        if s.va as usize + s.vsize as usize > RAM || !mem.load(s.va, raw) {
            return Err(format!("section {} doesn't fit in RAM", s.name));
        }
    }
    Ok(mem)
}

/// The entry thread's state before its first instruction: image loaded,
/// EIP at the entry, and a stack whose return address is [`HARNESS_EXIT`]
/// (the entry is entered as if called). Other backends start from this too,
/// so their runs are comparable.
pub fn initial_state(xbe: &Xbe) -> Result<(Cpu, FlatMemory), String> {
    let mut mem = load_image(xbe)?;
    let mut cpu = Cpu { eip: xbe.entry, ..Cpu::default() };
    cpu.set_reg(Gpr::Esp, STACK_TOP - 4);
    // No KPCR exists yet: point FS outside RAM so any FS access faults
    // instead of quietly reading low memory.
    cpu.fs_base = RAM as u32;
    mem.load(STACK_TOP - 4, &HARNESS_EXIT.to_le_bytes());
    Ok((cpu, mem))
}

/// Run from the entry point until the first kernel call.
pub fn run_to_kernel(xbe: &Xbe, lifter: &Lifter, max_steps: usize) -> Result<Run, String> {
    let (mut cpu, mut mem) = initial_state(xbe)?;
    // Every instruction of every function lifted so far. Shared code has
    // the same semantics in each copy, so the first copy is kept.
    let mut cache: HashMap<u32, IrInsn> = HashMap::new();
    let mut executed = std::collections::HashSet::new();
    let mut functions = Vec::new();
    let mut pc = xbe.entry;
    let mut steps = 0;
    let stop = loop {
        if steps == max_steps {
            break Stop::StepLimit;
        }
        if pc == HARNESS_EXIT {
            break Stop::Returned;
        }
        if !cache.contains_key(&pc) {
            let Some(f) = lifter.lift(pc) else { break Stop::NotLifted(pc) };
            functions.push(pc);
            for insn in f.blocks.into_values().flat_map(|b| b.insns) {
                cache.entry(insn.pc).or_insert(insn);
            }
        }
        let insn = &cache[&pc];
        executed.insert(pc);
        steps += 1;
        match interp::step(&mut cpu, &mut mem, insn) {
            Ok(flow) => {
                if let (Flow::Call { target, continuation }, Control::Call { kind: CallKind::Kernel { slot, ordinal }, .. }) =
                    (flow, &insn.control)
                {
                    break Stop::KernelCall { pc, slot: *slot, ordinal: *ordinal, target, continuation };
                }
                pc = flow.target();
            }
            Err(e) => break Stop::Exit(e),
        }
    };
    let executed = executed.len();
    Ok(Run { stop, steps, cpu, mem, executed, functions })
}
