//! `impl.sim.effects-parts` (on by default): the effects phase of the tick
//! in Rust (docs/engine/effects-parts.md).
//!
//! Two pieces. The effect walk of `0x176e40` is the tick driver's Rust body
//! for the phase ([`crate::tick::bodies`], `engine::effects_phase`). This
//! module is the other: an implementation of the part switch
//! `EFFECT_PART_CREATE` (`0x17a8a0`) that decodes each part and hands it to
//! [`sim_effects::dispatch::Parts`]:
//!
//! - `jpt!` damage parts are the damage arm. Until it is proven the guest's
//!   arm runs and an observer at `DAMAGE_AREA_APPLY` (`0xd6c80`, reached
//!   from `0x17aa4f`) compares the record the guest built with Rust's.
//!   Once proven Rust calls `0xd6c80` with its own record, and one part in
//!   64 is still the guest's and compared. A difference returns every part
//!   to the guest until the next map load.
//! - `lens`, `MGS2` and `tdtl` return from Rust: retail's arm is the epilogue.
//! - decal, light and lens flare parts become scene events
//!   ([`sim_effects::scene`]) the frame step puts in the renderer's scene.
//! - every other part runs the guest's arm, counted by its class.
//!
//! No RNG draw moves: the damage arm and the returned arms draw nothing,
//! and every arm that draws (object, decal, coln) is still the guest's.
//!
//! Hooks this adds, and their order: the implementation of
//! `EFFECT_PART_CREATE` (the only one there; `host_audit`'s observers stay
//! outside it), an `observe_before` on `DAMAGE_AREA_APPLY` (the point's
//! only hook) and an `observe_before` on `CACHE_TAGS_LOAD` beside the
//! other arms' resets (observers need no order).

use std::sync::{Mutex, MutexGuard};

use engine::entry::{Cx, EntryResult, GuestCall, Ret};
use engine::memory::GuestMemory;
use engine::points;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};
use sim_effects::arm::Seen;
use sim_effects::damage::{self, Inputs, Record};
use sim_effects::dispatch::{Action, Parts};
use sim_effects::{Part, PartClass};

/// The switch, without its `impl.` prefix. On by default; off, the guest
/// runs the effects phase and every part.
pub const PARTS: &str = "sim.effects-parts";
const ID: &str = "impl.sim.effects-parts";

pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(PARTS, true)
}

/// The switch's argument bytes (`ret 0x14`).
const POP: u32 = 0x14;
/// Where the switch's damage arm calls `0xd6c80` from.
pub const DAMAGE_RETURN: u32 = 0x0017_aa4f;
/// EFLAGS' arithmetic bits, left undefined by an answered part.
const ARITH: u32 = 0x8d5;

/// The process's part state: Rust owned, rebuilt at each map load.
static PARTS_STATE: Mutex<Parts> = Mutex::new(Parts::new());

fn parts() -> MutexGuard<'static, Parts> {
    PARTS_STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The counters now: the parts by what happened to them, and the arm.
pub fn status() -> (sim_effects::dispatch::Stats, sim_effects::arm::Stats) {
    let p = parts();
    (p.stats(), p.arm_stats())
}

/// The part and, for a damage part, the arm's record inputs, from the
/// switch's entry: `[esp+4]` the effect datum, `[esp+8]` the part record,
/// `[esp+0xc]` and `[esp+0x10]` the position and direction pointers,
/// `[esp+0x14]` the scale.
fn decode(g: &Guest, esp: u32) -> Result<(Part, Option<Inputs>), GuestFault> {
    let arg = |n: u32| g.u32(esp.wrapping_add(4 + 4 * n));
    let (effect, record) = (arg(0)?, arg(1)?);
    let group = g.u32(record.wrapping_add(8))?;
    let tag = g.u32(record.wrapping_add(0x10))?;
    let vec = |at: u32| -> Result<[u32; 3], GuestFault> {
        Ok([g.u32(at)?, g.u32(at.wrapping_add(4))?, g.u32(at.wrapping_add(8))?])
    };
    let (position, direction, scale) = (vec(arg(2)?)?, vec(arg(3)?)?, arg(4)?);
    let part = Part {
        tick: g.u32(g.u32(points::GAME_TIME.base)?.wrapping_add(8))?,
        definition: g.u32(effect.wrapping_add(4))?,
        group,
        tag,
        position: position.map(f32::from_bits),
        direction: direction.map(f32::from_bits),
        scale: f32::from_bits(scale),
    };
    if part.class() != PartClass::Damage {
        return Ok((part, None));
    }
    let inputs = Inputs {
        tag,
        owner: vec(effect.wrapping_add(0x4c))?,
        effect_1c: g.u32(effect.wrapping_add(0x1c))?,
        effect_20: g.u32(effect.wrapping_add(0x20))?,
        position,
        direction,
        scale,
        word_7c: g.u16(damage::INIT_WORD_7C)?,
        word_22: g.u16(damage::INIT_WORD_22)?,
        one: g.u32(damage::INIT_ONE)?,
    };
    Ok((part, Some(inputs)))
}

/// An answered damage part: the record where retail's arm keeps it, the
/// four registers it saves where it saves them, then `0xd6c80(record, -1)`
/// with the arm's stack depth. Returns as the switch does.
async fn answer(cx: Cx, record: Record) -> EntryResult {
    let e = cx.entry();
    let at = e.esp.wrapping_sub(damage::RECORD_BELOW_ENTRY);
    cx.write(|m: &mut dyn GuestMemory| {
        for (n, w) in record.chunks_exact(4).enumerate() {
            m.set_u32(at + 4 * n as u32, u32::from_le_bytes(w.try_into().expect("four bytes")))?;
        }
        for (n, r) in [e.ebx, e.ebp, e.esi, e.edi].into_iter().enumerate() {
            m.set_u32(e.esp.wrapping_sub(0x104 + 4 * n as u32), r)?;
        }
        Ok::<_, engine::memory::MemFault>(())
    })?;
    let call = GuestCall::new(points::DAMAGE_AREA_APPLY.base)
        .args([at, damage::NO_OBJECT])
        .reserve(damage::FRAME)
        .ebx(cx.arg(0)?)
        .edi(e.eax);
    let r = cx.call(call).await?;
    Ok(Ret::new(r.eax, POP as u16))
}

/// The original switch.
fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32) -> Result<TransferAbi, GuestFault> {
    let handler = ctx.runtime().resolve_original(pc).ok_or(GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    })?;
    Ok(handler.invoke(cpu, ctx, pc))
}

fn enter(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32) -> Result<TransferAbi, GuestFault> {
    let decoded = {
        let mem = ctx.memory();
        decode(&mem, cpu.esp)
    };
    let action = match decoded {
        Ok((part, inputs)) => {
            let mut queue = sim_effects::scene::QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            let action = parts().enter(&part, inputs.as_ref(), &mut queue);
            (action, part.group)
        }
        Err(_) => {
            parts().fault();
            (Action::Guest, 0)
        }
    };
    match action {
        (Action::Guest, _) => original(cpu, ctx, pc),
        (Action::Return, group) => {
            let target = ctx.memory().u32(cpu.esp)?;
            cpu.eax = group;
            cpu.esp = cpu.esp.wrapping_add(4 + POP);
            cpu.eflags_defined &= !ARITH;
            Ok(TransferAbi::ret(target))
        }
        (Action::Answer(record), _) => {
            hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer(cx, record)))
        }
    }
}

/// At `0xd6c80`, called from the switch's damage arm: compare.
fn compare(g: &Guest, esp: u32) {
    let Ok(DAMAGE_RETURN) = g.u32(esp) else { return };
    let seen = (|| -> Result<Seen, GuestFault> {
        let at = g.u32(esp.wrapping_add(4))?;
        let mut record = [0u8; damage::SIZE];
        g.read_bytes(at, &mut record, 1)?;
        Ok(Seen { record, except: g.u32(esp.wrapping_add(8))? })
    })();
    let mut p = parts();
    let Ok(seen) = seen else {
        p.arm().declined_fault();
        return;
    };
    let first = p.arm_stats().mismatched == 0;
    if p.arm().seen(&seen) == Some(false) && first {
        eprintln!(
            "sim: effects damage arm: the guest's record differs from Rust's at byte {:#x}; \
             the guest answers every damage part until the next map load",
            p.arm_stats().first_mismatch_at.unwrap_or(0)
        );
    }
}

pub struct SimEffectsParts;

impl Feature for SimEffectsParts {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The effects phase in Rust: the effect walk and the damage part arm")
            .doc("docs/engine/effects-parts.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The arm's proof and the scene queue are the process's, not the
        // game's: both answers leave the same guest state.
        b.snapshot(SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&points::EFFECT_PART_CREATE, "effect part arm", enter).context_only();
        b.raw()
            .observe_before(&points::DAMAGE_AREA_APPLY, "effect part arm: compare", |o| compare(&o.guest(), o.cpu.esp))
            .optional();
        b.raw().observe_before(&points::CACHE_TAGS_LOAD, "effect part arm: reset", |_| {
            parts().reset();
            sim_effects::scene::QUEUE.lock().unwrap_or_else(|e| e.into_inner()).clear();
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn the_switch_is_on_by_default_and_installs_the_arm() {
        for (spec, on) in [("", true), ("impl.sim.effects-parts", true), ("-impl.sim.effects-parts", false)] {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&SimEffectsParts).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert_eq!(plan.chains.contains_key(&points::EFFECT_PART_CREATE.base), on, "{spec:?}");
            assert_eq!(plan.chains.contains_key(&points::DAMAGE_AREA_APPLY.base), on, "{spec:?}");
        }
    }

    /// The answered part's record sits where retail's arm keeps it, and its
    /// call has the arm's depth: the callee's `ESP` is the arm's.
    #[test]
    fn the_answer_calls_the_area_damage_from_the_arms_frame() {
        use engine::entry::{self, Regs};
        use engine::testing::FakeGuest;
        const STACK: u32 = 0x0030_0000;
        let record: Record = std::array::from_fn(|i| i as u8);
        let mut g = FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000).put_u32(STACK + 4, 0x0070_0000);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer(cx, record)) };
        let regs = Regs { esp: STACK, eax: 0x11, ebx: 0x22, ebp: 0x33, esi: 0x44, edi: 0x55, ..Default::default() };
        let run = entry::testing::run(&f, g, regs, |c, m| {
            assert_eq!(c.esp(STACK), STACK - 0x11c, "retail's ESP at the call");
            assert_eq!(m.bytes(c.args[0], damage::SIZE).unwrap(), record);
            Regs { eax: 0x99, ..Default::default() }
        });
        assert_eq!(run.result, Ok(Ret::new(0x99, 0x14)));
        let c = &run.calls[0];
        assert_eq!((c.target, c.args.clone()), (points::DAMAGE_AREA_APPLY.base, vec![STACK - 0xdc, u32::MAX]));
        assert_eq!((c.ebx, c.edi), (Some(0x0070_0000), Some(0x11)));
        let saved: Vec<u32> = (0..4).map(|n| run.guest.u32(STACK - 0x104 - 4 * n).unwrap()).collect();
        assert_eq!(saved, [0x22, 0x33, 0x44, 0x55]);
    }
}
