//! The contact commit `0x00318c10` as a take-over arm (`parity`,
//! docs/engine/havok-convex.md, "The commit arm"). The model is
//! `sim_havok::commit::head`: [`capture`] runs the call on a journal up to
//! its world listener walk (`0x00318320`), and holds every store it makes
//! (the constraint's point, property and ID, the island's sizes, the
//! frame with the event, the walk's two arguments and its return to
//! `0x00318d9b`) with the registers at the walk's entry.
//!
//! The walk calls Halo's contact callback, which stays guest code: an
//! answered commit is left at the walk's entry and the caller makes the
//! call in place, the lifted tail running from its site. An audited
//! commit is compared at the same point, when the guest's own walk is
//! entered from that site. The x87 stack is empty there, as at the
//! entry; the x87 condition codes, the sticky exception flags and the XMM
//! registers are not modelled.
use super::alloc::View;
use super::answer::{Answer, Reg};
use runtime::{CpuAbi, Guest};
use sim_core::det;
use sim_havok::commit::{self as model, Args, Calls, Fallback, Head};
use sim_havok::jacobian::Fpu;
use sim_havok::mem::Journal;

pub use sim_havok::commit::{WALK_SITE, WORLD_WALK};

/// The calls the head makes out, which the arm does not answer.
struct Refuse;

impl Calls for Refuse {
    fn world_add(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
        Err(Fallback::First)
    }

    fn reserve(&mut self, _: &mut Journal, _: u32, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
        Err(Fallback::Grow)
    }

    fn expand(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
        Err(Fallback::Grow)
    }

    // The head reaches neither.
    fn listener(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
        Err(Fallback::Memory)
    }

    fn remove(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<u32, Fallback> {
        Err(Fallback::Memory)
    }
}

pub struct Prediction {
    answer: Answer,
    pub head: Head,
}

impl Prediction {
    /// At the walk's entry: esp, the seven registers and every store.
    pub fn matches(&self, cpu: &CpuAbi, g: &Guest) -> bool {
        self.answer.matches(cpu, g)
    }

    /// Leave the call at the walk's entry: the stores, the registers, esp
    /// at the pushed return.
    pub fn apply(&self, cpu: &mut CpuAbi, g: &mut Guest) -> bool {
        self.answer.apply(cpu, g)
    }
}

/// The model's state at the walk for the call at its entry, or the reason
/// it declines.
pub fn capture(cpu: &CpuAbi, g: &Guest) -> Result<Prediction, Fallback> {
    if !det::x87_env(cpu.x87.control, cpu.x87.status, cpu.x87.tag, 8) {
        return Err(Fallback::Float);
    }
    g.u32(cpu.esp).map_err(|_| Fallback::Memory)?;
    let arg = |n: u32| g.u32(cpu.esp.wrapping_add(4 + 4 * n)).map_err(|_| Fallback::Memory);
    let args = Args {
        manager: cpu.ecx,
        a: arg(0)?,
        b: arg(1)?,
        input: arg(2)?,
        contact: arg(3)?,
        esp: cpu.esp,
        eax: cpu.eax,
        ebx: cpu.ebx,
        ebp: cpu.ebp,
        esi: cpu.esi,
        edi: cpu.edi,
    };
    let view = View(g);
    let mut j = Journal::new(&view);
    let fpu = Fpu { mxcsr: cpu.mxcsr, control: cpu.x87.control };
    let head = match model::head(&mut j, &args, fpu, &mut Refuse)? {
        Ok(head) => head,
        Err(_) => return Err(Fallback::Full),
    };
    let writes = j.finish();
    let [eax, ecx, edx, ebx, ebp, esi, edi] = head.walk_regs(&args);
    let regs = vec![
        (Reg::Eax, eax),
        (Reg::Ecx, ecx),
        (Reg::Edx, edx),
        (Reg::Ebx, ebx),
        (Reg::Ebp, ebp),
        (Reg::Esi, esi),
        (Reg::Edi, edi),
    ];
    let answer = Answer::new(writes, regs, head.walk_esp(), g).ok_or(Fallback::Memory)?;
    Ok(Prediction { answer, head })
}
