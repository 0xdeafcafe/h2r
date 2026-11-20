//! Where the interpreter's instructions come from, and stepping it.
//!
//! [`Original`] lifts the image's functions on demand and keeps every
//! instruction it lifted: the lifted original of whatever the guest reaches,
//! entries, resumes and callees alike. [`Assembled`] lowers raw x86 bytes
//! for synthetic tests that have no image.

use std::collections::HashMap;

use ir::IrInsn;
use ir::interp::{self, Cpu, Exit, Flow, Memory};
use runtime::TransferAbi;

/// Instructions by address.
pub trait Code: Send {
    /// The instruction at `pc`, or `None` where there is no code the
    /// lifter knows (an unanalysed address, a kernel import token).
    fn insn(&mut self, pc: u32) -> Option<&IrInsn>;

    /// Lift the function at `entry` first, so instructions it shares with
    /// an overlapping function take this one's edges. The default does
    /// nothing.
    fn prime(&mut self, _entry: u32) {}
}

/// The image's functions, lifted when first reached.
pub struct Original {
    lifter: lift::Lifter<'static>,
    /// Every discovered function start, ascending.
    functions: Vec<u32>,
    cache: HashMap<u32, IrInsn>,
    primed: std::collections::HashSet<u32>,
}

/// How many function starts at or below an address [`Original`] tries
/// before deciding no function holds it.
const NEAREST: usize = 64;

impl Original {
    pub fn new(analysis: &'static analyze::Analysis, xbe: &'static xbe::Xbe<'static>) -> Result<Self, String> {
        let lifter = lift::Lifter::new(analysis, xbe).map_err(|e| format!("lifter: {e}"))?;
        Ok(Original {
            lifter,
            functions: analysis.functions.keys().copied().collect(),
            cache: HashMap::new(),
            primed: Default::default(),
        })
    }

    /// Instructions lifted so far.
    pub fn lifted(&self) -> usize {
        self.cache.len()
    }

    fn take(&mut self, f: ir::IrFunction) -> bool {
        let mut any = false;
        for insn in f.blocks.into_values().flat_map(|b| b.insns) {
            any = true;
            self.cache.entry(insn.pc).or_insert(insn);
        }
        any
    }
}

impl Code for Original {
    fn insn(&mut self, pc: u32) -> Option<&IrInsn> {
        if !self.cache.contains_key(&pc) {
            // The nearest functions at or below pc, until one holds it.
            let below = self.functions.partition_point(|&e| e <= pc);
            for i in (below.saturating_sub(NEAREST)..below).rev() {
                let entry = self.functions[i];
                let Some(f) = self.lifter.lift(entry) else { continue };
                let has = f.blocks.values().any(|b| b.insns.iter().any(|x| x.pc == pc));
                self.take(f);
                if has {
                    break;
                }
            }
        }
        self.cache.get(&pc)
    }

    fn prime(&mut self, entry: u32) {
        if self.primed.insert(entry)
            && let Some(f) = self.lifter.lift(entry)
        {
            self.take(f);
        }
    }
}

/// Raw x86 at a base address, decoded linearly and lowered with no CFG
/// facts ([`lift::Site::Plain`]): for synthetic tests.
pub struct Assembled {
    insns: HashMap<u32, IrInsn>,
}

impl Assembled {
    pub fn new(base: u32, code: &[u8]) -> Self {
        let env = lift::Env::default();
        let decoder = iced_x86::Decoder::with_ip(32, code, base as u64, iced_x86::DecoderOptions::NONE);
        let insns = decoder
            .into_iter()
            .map(|i| {
                let at = (i.ip32() - base) as usize;
                let ir = lift::lower_encoded(&i, &code[at..], &env, lift::Site::Plain);
                (ir.pc, ir)
            })
            .collect();
        Assembled { insns }
    }
}

impl Code for Assembled {
    fn insn(&mut self, pc: u32) -> Option<&IrInsn> {
        self.insns.get(&pc)
    }
}

/// Why the interpreter stopped short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepError {
    /// No instruction at this address.
    NoCode(u32),
    /// The interpreter left: a fault, an unsupported instruction, a stop.
    Exit(Exit),
    /// The step limit, at this address.
    Limit(u32),
}

impl std::fmt::Display for StepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StepError::NoCode(pc) => write!(f, "no lifted code at {pc:#010x}"),
            StepError::Exit(e) => write!(f, "interpreter exit: {e:?}"),
            StepError::Limit(pc) => write!(f, "step limit at {pc:#010x}"),
        }
    }
}

/// One instruction at `cpu.eip`; `cpu.eip` is then where control went.
pub fn step<M: Memory>(code: &mut (impl Code + ?Sized), cpu: &mut Cpu, mem: &mut M) -> Result<Flow, StepError> {
    let pc = cpu.eip;
    let insn = code.insn(pc).ok_or(StepError::NoCode(pc))?;
    let flow = interp::step(cpu, mem, insn).map_err(StepError::Exit)?;
    cpu.eip = flow.target();
    Ok(flow)
}

/// Run from `cpu.eip` to the next call or return, the boundary a
/// segmented port hands back at: the transfer as a port returns it.
pub fn segment<M: Memory>(
    code: &mut (impl Code + ?Sized),
    cpu: &mut Cpu,
    mem: &mut M,
    limit: u64,
) -> Result<TransferAbi, StepError> {
    for _ in 0..limit {
        match step(code, cpu, mem)? {
            Flow::Return { target } => return Ok(TransferAbi::ret(target)),
            Flow::Call { target, continuation } => return Ok(TransferAbi::call(target, continuation)),
            _ => {}
        }
    }
    Err(StepError::Limit(cpu.eip))
}

/// Run from `cpu.eip` until `stop` holds before an instruction (checked
/// first, so a state that already holds stops at once), stepping into
/// calls. Returns the steps taken.
pub fn run_until<M: Memory>(
    code: &mut (impl Code + ?Sized),
    cpu: &mut Cpu,
    mem: &mut M,
    limit: u64,
    mut stop: impl FnMut(&Cpu) -> bool,
) -> Result<u64, StepError> {
    for steps in 0..=limit {
        if stop(cpu) {
            return Ok(steps);
        }
        if steps == limit {
            break;
        }
        step(code, cpu, mem)?;
    }
    Err(StepError::Limit(cpu.eip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ir::interp::SparseMemory;

    const BASE: u32 = 0x1000;

    #[test]
    fn segment_stops_at_the_first_call_and_return() {
        // mov eax, 5; call +0 (to 0x100a); ret
        let code = [0xb8, 5, 0, 0, 0, 0xe8, 0, 0, 0, 0, 0xc3];
        let mut asm = Assembled::new(BASE, &code);
        let mut mem = SparseMemory::new();
        mem.map(0x8000 - 0x40, 0x80);
        mem.load(0x8000, &0xdead_0000u32.to_le_bytes());
        let mut cpu = Cpu { eip: BASE, ..Cpu::default() };
        cpu.gpr[4] = 0x8000;
        let t = segment(&mut asm, &mut cpu, &mut mem, 100).unwrap();
        assert_eq!(t, TransferAbi::call(0x100a, 0x100a));
        assert_eq!(cpu.gpr[0], 5);
        assert_eq!(cpu.gpr[4], 0x7ffc);
        let t = segment(&mut asm, &mut cpu, &mut mem, 100).unwrap();
        assert_eq!(t, TransferAbi::ret(0x100a));
        assert_eq!(cpu.eip, 0x100a);
    }

    #[test]
    fn running_off_the_code_names_the_address() {
        // jmp 0x2000
        let code = [0xe9, 0xfb, 0x0f, 0, 0];
        let mut asm = Assembled::new(BASE, &code);
        let mut cpu = Cpu { eip: BASE, ..Cpu::default() };
        let r = run_until(&mut asm, &mut cpu, &mut SparseMemory::new(), 10, |_| false);
        assert_eq!(r, Err(StepError::NoCode(0x2000)));
    }

    #[test]
    fn run_until_checks_before_each_step_and_honours_the_limit() {
        // inc eax; jmp -3 (back to inc)
        let code = [0x40, 0xeb, 0xfd];
        let mut asm = Assembled::new(BASE, &code);
        let mut cpu = Cpu { eip: BASE, ..Cpu::default() };
        let mut mem = SparseMemory::new();
        assert_eq!(run_until(&mut asm, &mut cpu, &mut mem, 10, |_| true), Ok(0));
        let n = run_until(&mut asm, &mut cpu, &mut mem, 100, |c| c.gpr[0] == 3).unwrap();
        assert_eq!(n, 5);
        assert_eq!(run_until(&mut asm, &mut cpu, &mut mem, 4, |_| false), Err(StepError::Limit(BASE + 1)));
    }
}
