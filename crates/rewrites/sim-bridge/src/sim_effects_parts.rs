//! `impl.sim.effects-parts` (on by default): the effects phase of the tick
//! in Rust (docs/engine/effects-parts.md).
//!
//! Two pieces. The effect walk of `0x176e40` is the tick driver's Rust body
//! for the phase ([`crate::tick::bodies`], `engine::effects_phase`). This
//! module is the other: an implementation of the part switch
//! `EFFECT_PART_CREATE` (`0x17a8a0`) that decodes each part and hands it to
//! [`sim_effects::dispatch::Parts`]:
//!
//! - `jpt!`, `obje` and `snd!` parts each have an arm. Until an arm is
//!   proven the guest's arm runs and observers at its leaves compare what
//!   the guest passes with Rust's prediction. Once proven Rust makes the
//!   leaf calls itself, and one part in 64 is still the guest's and
//!   compared. A difference returns that arm's parts to the guest until
//!   the next map load.
//!   - damage: `DAMAGE_AREA_APPLY` (`0xd6c80`, from `0x17aa4f`).
//!   - object: `EFFECT_PART_PLACE` (`0x177d10`, from `0x17abe2`) at entry
//!     and return, then `EFFECT_PART_VELOCITY` (`0x177f60`, from
//!     `0x17ac3a`). Rust's answer calls all seven leaves in retail's order.
//!   - sound: `SOUND_START_ON_OBJECT` (`0x189060`, from `0x17ad24`) or
//!     `SOUND_START_AT_POINT` (`0x1895f0`, from `0x17ad9a`).
//! - `lens`, `MGS2` and `tdtl` return from Rust: retail's arm is the epilogue.
//! - decal, light and lens flare parts become scene events
//!   ([`sim_effects::scene`]) the frame step puts in the renderer's scene.
//! - every other part runs the guest's arm, counted by its class.
//!
//! No RNG draw moves: the damage and sound arms draw nothing, the object
//! arm's two draws are inside `0x177d10` and `0x177f60`, which Rust calls
//! in retail's order with retail's arguments, and every other arm that
//! draws (decal, coln) is still the guest's.
//!
//! Hooks this adds, and their order: the implementation of
//! `EFFECT_PART_CREATE` (the only one there; `host_audit`'s observers stay
//! outside it); an `observe_before` on `DAMAGE_AREA_APPLY`,
//! `EFFECT_PART_VELOCITY`, `SOUND_START_ON_OBJECT` and
//! `SOUND_START_AT_POINT`, and an `observe_before` and an `observe_after`
//! on `EFFECT_PART_PLACE` (each point's only hooks); and an
//! `observe_before` on `CACHE_TAGS_LOAD` beside the other arms' resets.
//! Observers need no order: each filters on its caller's return address.

use std::sync::{Mutex, MutexGuard};

use engine::entry::{Cx, EntryError, EntryResult, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};
use sim_effects::damage::{self, Record};
use sim_effects::dispatch::{Action, Arms, Inputs, Parts, Stats};
use sim_effects::{object, sound, Part, PartClass};

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
/// The switch's frame below its entry `ESP`: `0x100` of locals and the four
/// registers it saves. Every leaf is called from this depth.
const FRAME: u32 = damage::FRAME;

/// The process's part state: Rust owned, rebuilt at each map load.
static PARTS_STATE: Mutex<Parts> = Mutex::new(Parts::new());

fn parts() -> MutexGuard<'static, Parts> {
    PARTS_STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The counters now: the parts by what happened to them, and the arms.
pub fn status() -> (Stats, Arms) {
    let p = parts();
    (p.stats(), p.arms())
}

fn vec3(g: &Guest, at: u32) -> Result<[u32; 3], GuestFault> {
    Ok([g.u32(at)?, g.u32(at.wrapping_add(4))?, g.u32(at.wrapping_add(8))?])
}

/// The part and its arm's inputs, from the switch's entry: `[esp+4]` the
/// effect datum, `[esp+8]` the part record, `[esp+0xc]` and `[esp+0x10]`
/// the position and direction pointers, `[esp+0x14]` the scale; `EAX` the
/// source vector, `ECX` the location, `DL` a flag. Only what retail's arm
/// reads on its path is read.
fn decode(g: &Guest, cpu: &CpuAbi) -> Result<(Part, Option<Inputs>), GuestFault> {
    let esp = cpu.esp;
    let arg = |n: u32| g.u32(esp.wrapping_add(4 + 4 * n));
    let (effect, record) = (arg(0)?, arg(1)?);
    let group = g.u32(record.wrapping_add(8))?;
    let tag = g.u32(record.wrapping_add(0x10))?;
    let direction_ptr = arg(3)?;
    let (position, direction, scale) = (vec3(g, arg(2)?)?, vec3(g, direction_ptr)?, arg(4)?);
    let part = Part {
        tick: g.u32(g.u32(points::GAME_TIME.base)?.wrapping_add(8))?,
        definition: g.u32(effect.wrapping_add(4))?,
        group,
        tag,
        position: position.map(f32::from_bits),
        direction: direction.map(f32::from_bits),
        scale: f32::from_bits(scale),
    };
    let inputs = match part.class() {
        PartClass::Damage => Inputs::Damage(damage::Inputs {
            tag,
            owner: vec3(g, effect.wrapping_add(0x4c))?,
            effect_1c: g.u32(effect.wrapping_add(0x1c))?,
            effect_20: g.u32(effect.wrapping_add(0x20))?,
            position,
            direction,
            scale,
            word_7c: g.u16(damage::INIT_WORD_7C)?,
            word_22: g.u16(damage::INIT_WORD_22)?,
            one: g.u32(damage::INIT_ONE)?,
        }),
        PartClass::Object => {
            let mut part = [0; 7];
            for (w, &o) in part.iter_mut().zip(&object::PART_WORDS) {
                *w = g.u32(record.wrapping_add(o))?;
            }
            Inputs::Object(object::Inputs {
                frame: esp.wrapping_sub(FRAME),
                effect,
                tag,
                object: g.u32(effect.wrapping_add(0x48))?,
                source: cpu.eax,
                position,
                direction_ptr,
                direction,
                source_vec: vec3(g, cpu.eax)?,
                rng: g.u32(object::RNG)?,
                part,
                mxcsr: cpu.mxcsr,
            })
        }
        PartClass::Sound => {
            let object = g.u32(effect.wrapping_add(0x48))?;
            let dl = cpu.edx as u8;
            let on_object = object != u32::MAX && dl == 0;
            let mut i = sound::Inputs {
                tag,
                object,
                dl,
                location: cpu.ecx,
                effect,
                effect_1c: g.u32(effect.wrapping_add(0x1c))?,
                effect_20: g.u32(effect.wrapping_add(0x20))?,
                source_eax: cpu.eax,
                position,
                direction,
                scale,
                ..sound::Inputs::default()
            };
            if on_object {
                i.node = g.u16(cpu.ecx.wrapping_add(2))?;
            } else {
                i.listener = vec3(g, g.u32(sound::SOURCE)?)?;
                i.pack = sound::Pack {
                    low: g.u32(sound::PACK_LOW)?,
                    high: g.u32(sound::PACK_HIGH)?,
                    z: g.u32(sound::PACK_Z)?,
                    xy: g.u32(sound::PACK_XY)?,
                    mxcsr: cpu.mxcsr,
                    control: cpu.x87.control,
                };
            }
            Inputs::Sound(i)
        }
        _ => return Ok((part, None)),
    };
    Ok((part, Some(inputs)))
}

fn put_words(m: &mut dyn GuestMemory, at: u32, words: &[u32]) -> Result<(), MemFault> {
    for (n, &w) in words.iter().enumerate() {
        m.set_u32(at.wrapping_add(4 * n as u32), w)?;
    }
    Ok(())
}

/// The four registers the switch saves, where it saves them.
fn save_regs(cx: &Cx) -> Result<(), MemFault> {
    let e = cx.entry();
    cx.write(|m: &mut dyn GuestMemory| {
        let saved = [e.edi, e.esi, e.ebp, e.ebx];
        put_words(m, e.esp.wrapping_sub(FRAME), &saved)
    })
}

/// An answered damage part: the record where retail's arm keeps it, then
/// `0xd6c80(record, -1)` with the arm's stack depth. Returns as the switch
/// does.
async fn answer_damage(cx: Cx, record: Record) -> EntryResult {
    let e = cx.entry();
    let at = e.esp.wrapping_sub(damage::RECORD_BELOW_ENTRY);
    save_regs(&cx)?;
    cx.write(|m: &mut dyn GuestMemory| {
        let words: Vec<u32> =
            record.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().expect("four bytes"))).collect();
        put_words(m, at, &words)
    })?;
    let call = GuestCall::new(points::DAMAGE_AREA_APPLY.base)
        .args([at, damage::NO_OBJECT])
        .reserve(FRAME)
        .ebx(cx.arg(0)?)
        .edi(e.eax);
    let r = cx.call(call).await?;
    Ok(Ret::new(r.eax, POP as u16))
}

/// A leaf call from the switch's frame with retail's six registers
/// (`EAX`, `ECX`, `EDX`, `EBX`, `ESI`, `EDI`).
fn leaf(target: u32, args: &[u32], r: [u32; 6]) -> GuestCall {
    GuestCall::new(target)
        .args(args.iter().copied())
        .reserve(FRAME)
        .eax(r[0])
        .ecx(r[1])
        .edx(r[2])
        .ebx(r[3])
        .esi(r[4])
        .edi(r[5])
}

/// An answered object part: retail's seven leaves in retail's order
/// (docs/engine/effects-parts.md "How it works", step 5). The vectors, the stream
/// pointer and the part words are read where retail reads them, after
/// `0xa7640`, so the leaves see what retail's arm would pass them.
async fn answer_object(cx: Cx, entry: object::Inputs) -> EntryResult {
    let e = cx.entry();
    let record = cx.arg(1)?;
    save_regs(&cx)?;
    let p = entry.placement();
    cx.call(leaf(object::INIT, &entry.init_args(), entry.init_regs())).await?;
    let r = cx.call(GuestCall::new(object::CHECK).reserve(FRAME).eax(p).ebx(entry.effect).esi(p).edi(e.eax)).await?;
    if r.eax & 0xff == 0 {
        return Ok(Ret::new(r.eax, POP as u16));
    }
    let i = cx.read(|m: &dyn GuestMemory| -> Result<object::Inputs, MemFault> {
        let v = |at: u32| -> Result<[u32; 3], MemFault> { Ok([m.u32(at)?, m.u32(at.wrapping_add(4))?, m.u32(at.wrapping_add(8))?]) };
        let mut part = [0; 7];
        for (w, &o) in part.iter_mut().zip(&object::PART_WORDS) {
            *w = m.u32(record.wrapping_add(o))?;
        }
        Ok(object::Inputs {
            position: v(m.u32(e.esp.wrapping_add(0xc))?)?,
            direction_ptr: m.u32(e.esp.wrapping_add(0x10))?,
            direction: v(m.u32(e.esp.wrapping_add(0x10))?)?,
            source_vec: v(e.eax)?,
            rng: m.u32(object::RNG)?,
            part,
            ..entry
        })
    })?;
    cx.write(|m: &mut dyn GuestMemory| put_words(m, i.frame.wrapping_add(object::COPIES), &i.copies()))?;
    cx.call(leaf(object::PLACE, &i.place_args(), i.place_regs())).await?;
    let (offset, placed) = cx.read(|m: &dyn GuestMemory| -> Result<_, MemFault> {
        let v = |at: u32| -> Result<[u32; 3], MemFault> { Ok([m.u32(at)?, m.u32(at.wrapping_add(4))?, m.u32(at.wrapping_add(8))?]) };
        Ok((v(i.effect.wrapping_add(0x30))?, v(i.frame.wrapping_add(object::SUMS))?))
    })?;
    // The dispatch refuses an MXCSR with an exception unmasked, so the sums
    // cannot fault here.
    let sums = object::sums(i.mxcsr, offset, placed)
        .map_err(|_| EntryError::IsolatedCall("effect object arm: an addss faulted under a masked MXCSR".into()))?;
    cx.write(|m: &mut dyn GuestMemory| put_words(m, i.frame.wrapping_add(object::SUMS), &sums))?;
    cx.call(leaf(object::VELOCITY, &i.velocity_args(), i.velocity_regs()).xmm0(u128::from(sums[2]))).await?;
    let [_, _, _, _, p30, p34] = i.velocity_regs();
    let r = cx.call(GuestCall::new(object::NEW).args([p]).reserve(FRAME).edx(p).ebx(i.effect).esi(p30).edi(p34)).await?;
    let created = r.eax;
    if created == u32::MAX {
        return Ok(Ret::new(created, POP as u16));
    }
    cx.call(GuestCall::new(object::AFTER_NEW).args([1, 0]).reserve(FRAME).ebx(i.effect).esi(created).edi(p34))
        .await?;
    let r = cx
        .call(GuestCall::new(object::ATTACH).args([created]).reserve(FRAME).ebx(i.effect).esi(created).edi(p34))
        .await?;
    Ok(Ret::new(r.eax, POP as u16))
}

/// An answered sound part: one leaf call, as [`sound::Call`] built it.
async fn answer_sound(cx: Cx, call: sound::Call, i: sound::Inputs) -> EntryResult {
    let e = cx.entry();
    save_regs(&cx)?;
    let g = match call {
        sound::Call::OnObject { eax, ecx, ebx, esi, edx, args } => GuestCall::new(points::SOUND_START_ON_OBJECT.base)
            .args(args)
            .reserve(FRAME)
            .eax(eax)
            .ecx(ecx)
            .edx(edx)
            .ebx(ebx)
            .esi(esi)
            .edi(e.eax),
        sound::Call::AtPoint { record, tag, scale } => {
            let at = e.esp.wrapping_sub(sound::RECORD_BELOW_ENTRY);
            cx.write(|m: &mut dyn GuestMemory| {
                let words: Vec<u32> =
                    record.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().expect("four bytes"))).collect();
                put_words(m, at, &words)
            })?;
            let [eax, ecx, ebx, esi, edi] = sound::Call::point_regs(&i);
            GuestCall::new(points::SOUND_START_AT_POINT.base)
                .args([tag])
                .reserve(FRAME)
                .eax(eax)
                .ecx(ecx)
                .edx(at)
                .ebx(ebx)
                .esi(esi)
                .edi(edi)
                .xmm0(u128::from(scale))
        }
    };
    let r = cx.call(g).await?;
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
        decode(&mem, cpu)
    };
    let (action, group) = match decoded {
        Ok((part, inputs)) => {
            let mut queue = sim_effects::scene::QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            (parts().enter(&part, inputs.as_ref(), &mut queue), part.group)
        }
        Err(_) => {
            parts().fault();
            (Action::Guest, 0)
        }
    };
    match action {
        Action::Guest => original(cpu, ctx, pc),
        Action::Return => {
            let target = ctx.memory().u32(cpu.esp)?;
            cpu.eax = group;
            cpu.esp = cpu.esp.wrapping_add(4 + POP);
            cpu.eflags_defined &= !ARITH;
            Ok(TransferAbi::ret(target))
        }
        Action::Damage(call) => {
            hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_damage(cx, call.record)))
        }
        Action::Object(i) => hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_object(cx, i))),
        Action::Sound(call, i) => {
            hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_sound(cx, call, i)))
        }
    }
}

/// The first mismatch of an arm, said once per map.
fn mismatch(arm: &str, first: bool, at: Option<usize>) {
    if first {
        eprintln!(
            "sim: effects {arm} arm: the guest's leaf call differs from Rust's at {:#x}; \
             the guest answers every {arm} part until the next map load",
            at.unwrap_or(0)
        );
    }
}

/// The six registers the comparisons read, in [`leaf`]'s order.
fn regs(cpu: &CpuAbi) -> [u32; 6] {
    [cpu.eax, cpu.ecx, cpu.edx, cpu.ebx, cpu.esi, cpu.edi]
}

fn words<const N: usize>(g: &Guest, at: u32) -> Result<[u32; N], GuestFault> {
    let mut out = [0; N];
    for (n, w) in out.iter_mut().enumerate() {
        *w = g.u32(at.wrapping_add(4 * n as u32))?;
    }
    Ok(out)
}

/// At `0xd6c80`, called from the switch's damage arm: compare.
fn compare_damage(g: &Guest, esp: u32) {
    let Ok(DAMAGE_RETURN) = g.u32(esp) else { return };
    let seen = (|| -> Result<damage::Call, GuestFault> {
        let at = g.u32(esp.wrapping_add(4))?;
        let mut record = [0u8; damage::SIZE];
        g.read_bytes(at, &mut record, 1)?;
        Ok(damage::Call { record, except: g.u32(esp.wrapping_add(8))? })
    })();
    let mut p = parts();
    let Ok(seen) = seen else {
        p.damage.declined_fault();
        return;
    };
    let first = p.damage.stats().mismatched == 0;
    if p.damage.seen(&seen) == Some(false) {
        mismatch("damage", first, p.damage.stats().first_mismatch_at);
    }
}

/// At `0x177d10`'s entry from the object arm: the copies, the arguments
/// and the registers. `S` is `ESP + 0x28` (nine arguments and the return).
fn compare_place(g: &Guest, cpu: &CpuAbi) {
    let Ok(object::PLACE_RETURN) = g.u32(cpu.esp) else { return };
    let frame = cpu.esp.wrapping_add(0x28);
    let mut p = parts();
    let Some(plan) = p.object.pending_mut().filter(|plan| plan.inputs.frame == frame) else { return };
    let seen = (|| -> Result<object::PlaceSeen, GuestFault> {
        Ok(object::PlaceSeen {
            words: words(g, frame.wrapping_add(object::COPIES))?,
            args: words(g, cpu.esp.wrapping_add(4))?,
            regs: regs(cpu),
        })
    })();
    let Ok(seen) = seen else {
        p.object.declined_fault();
        return;
    };
    match plan.place_difference(&seen) {
        None => plan.placed = true,
        Some(at) => {
            let first = p.object.stats().mismatched == 0;
            p.object.verdict(Some(at));
            mismatch("object", first, Some(at));
        }
    }
}

/// At `0x177d10`'s return to the object arm (`ESP` back at `S`): the sums
/// the arm will make from what it placed.
fn predict_sums(g: &Guest, cpu: &CpuAbi) {
    let mut p = parts();
    let Some(plan) = p.object.pending_mut().filter(|plan| plan.placed && plan.sums.is_none()) else { return };
    if plan.inputs.frame != cpu.esp {
        return;
    }
    let i = plan.inputs;
    let read = (|| -> Result<([u32; 3], [u32; 3]), GuestFault> {
        Ok((vec3(g, i.effect.wrapping_add(0x30))?, vec3(g, i.frame.wrapping_add(object::SUMS))?))
    })();
    match read.map(|(offset, placed)| object::sums(i.mxcsr, offset, placed)) {
        Ok(Ok(sums)) => plan.sums = Some(sums),
        // Left pending: counted unreached at the next object part.
        _ => p.object.declined_fault(),
    }
}

/// At `0x177f60`'s entry from the object arm: the sums, the arguments and
/// the registers. `S` is `ESP + 0x14`.
fn compare_velocity(g: &Guest, cpu: &CpuAbi) {
    let Ok(object::VELOCITY_RETURN) = g.u32(cpu.esp) else { return };
    let frame = cpu.esp.wrapping_add(0x14);
    let mut p = parts();
    let Some(plan) = p.object.pending_mut().filter(|plan| plan.inputs.frame == frame && plan.placed) else {
        return;
    };
    let seen = (|| -> Result<object::VelocitySeen, GuestFault> {
        Ok(object::VelocitySeen {
            words: words(g, frame.wrapping_add(object::SUMS))?,
            args: words(g, cpu.esp.wrapping_add(4))?,
            regs: regs(cpu),
        })
    })();
    let Ok(seen) = seen else {
        p.object.declined_fault();
        return;
    };
    let at = plan.velocity_difference(&seen);
    let first = p.object.stats().mismatched == 0;
    if !p.object.verdict(at) {
        mismatch("object", first, at);
    }
}

/// At `0x189060` or `0x1895f0` from the sound arm: compare the call.
fn compare_sound(g: &Guest, cpu: &CpuAbi, on_object: bool) {
    let want = if on_object { sound::ON_OBJECT_RETURN } else { sound::AT_POINT_RETURN };
    if g.u32(cpu.esp).ok() != Some(want) {
        return;
    }
    let seen = (|| -> Result<sound::Call, GuestFault> {
        let args: [u32; 3] = words(g, cpu.esp.wrapping_add(4))?;
        if on_object {
            return Ok(sound::Call::OnObject {
                eax: cpu.eax,
                ecx: cpu.ecx,
                ebx: cpu.ebx,
                esi: cpu.esi,
                edx: cpu.edx,
                args,
            });
        }
        let mut record = [0u8; sound::RECORD];
        g.read_bytes(cpu.edx, &mut record, 1)?;
        let scale = u32::from_le_bytes(cpu.xmm[0][..4].try_into().expect("four bytes"));
        Ok(sound::Call::AtPoint { record, tag: args[0], scale })
    })();
    let mut p = parts();
    let Ok(seen) = seen else {
        p.sound.declined_fault();
        return;
    };
    let first = p.sound.stats().mismatched == 0;
    if p.sound.seen(&seen) == Some(false) {
        mismatch("sound", first, p.sound.stats().first_mismatch_at);
    }
}

pub struct SimEffectsParts;

impl Feature for SimEffectsParts {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The effects phase in Rust: the effect walk and the damage, object and sound part arms")
            .doc("docs/engine/effects-parts.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The arms' proofs and the scene queue are the process's, not the
        // game's: both answers leave the same guest state.
        b.snapshot(SnapshotPolicy::Stateless);
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw().implement(&points::EFFECT_PART_CREATE, "effect part arm", enter).context_only();
        b.raw()
            .observe_before(&points::DAMAGE_AREA_APPLY, "effect damage arm: compare", |o| {
                compare_damage(&o.guest(), o.cpu.esp)
            })
            .optional();
        b.raw()
            .observe_before(&points::EFFECT_PART_PLACE, "effect object arm: compare the placing", |o| {
                compare_place(&o.guest(), o.cpu)
            })
            .optional();
        b.raw()
            .observe_after(&points::EFFECT_PART_PLACE, "effect object arm: predict the sums", |o| {
                predict_sums(&o.guest(), o.cpu)
            })
            .optional();
        b.raw()
            .observe_before(&points::EFFECT_PART_VELOCITY, "effect object arm: compare the velocity", |o| {
                compare_velocity(&o.guest(), o.cpu)
            })
            .optional();
        b.raw()
            .observe_before(&points::SOUND_START_ON_OBJECT, "effect sound arm: compare on the object", |o| {
                compare_sound(&o.guest(), o.cpu, true)
            })
            .optional();
        b.raw()
            .observe_before(&points::SOUND_START_AT_POINT, "effect sound arm: compare at a point", |o| {
                compare_sound(&o.guest(), o.cpu, false)
            })
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
            for p in [
                &points::DAMAGE_AREA_APPLY,
                &points::EFFECT_PART_PLACE,
                &points::EFFECT_PART_VELOCITY,
                &points::SOUND_START_ON_OBJECT,
                &points::SOUND_START_AT_POINT,
            ] {
                assert_eq!(plan.chains.contains_key(&p.base), on, "{spec:?} {}", p.name);
            }
        }
    }

    /// The points the observers sit on are the leaves the arms call.
    #[test]
    fn the_points_are_the_arms_leaves() {
        assert_eq!(points::EFFECT_PART_PLACE.base, object::PLACE);
        assert_eq!(points::EFFECT_PART_VELOCITY.base, object::VELOCITY);
        assert_eq!(points::SOUND_START_ON_OBJECT.base, sound::ON_OBJECT);
        assert_eq!(points::SOUND_START_AT_POINT.base, sound::AT_POINT);
    }

    const STACK: u32 = 0x0030_0000;
    const EFFECT: u32 = 0x0040_0000;
    const RECORD: u32 = 0x0040_1000;
    const VECS: u32 = 0x0040_2000;

    /// A switch entry for an object part: the effect, the part record
    /// (words `+0x14..+0x34` equal to their offsets), the three vectors
    /// and the stream pointer, as the switch's caller leaves them.
    fn object_guest() -> (engine::testing::FakeGuest, object::Inputs) {
        let mut g = engine::testing::FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000).map(EFFECT, 0x3000).map(object::RNG & !0xfff, 0x1000);
        let (pos, dir, src) = (VECS, VECS + 0x10, VECS + 0x20);
        g.put_u32(STACK + 4, EFFECT).put_u32(STACK + 8, RECORD).put_u32(STACK + 0xc, pos).put_u32(STACK + 0x10, dir);
        g.put_u32(EFFECT + 0x48, 0x0005_0001).put_u32(object::RNG, 0x0070_0000);
        for (n, v) in [1.0f32, 2.0, 3.0].iter().enumerate() {
            g.put_f32(EFFECT + 0x30 + 4 * n as u32, *v);
        }
        for (n, w) in (1..=9u32).enumerate() {
            g.put_u32(VECS + [0, 4, 8, 0x10, 0x14, 0x18, 0x20, 0x24, 0x28][n], w);
        }
        for o in object::PART_WORDS {
            g.put_u32(RECORD + o, o);
        }
        g.put_u32(RECORD + 0x10, 0xe100_0002);
        let i = object::Inputs {
            frame: STACK - 0x110,
            effect: EFFECT,
            tag: 0xe100_0002,
            object: 0x0005_0001,
            source: src,
            position: [1, 2, 3],
            direction_ptr: dir,
            direction: [4, 5, 6],
            source_vec: [7, 8, 9],
            rng: 0x0070_0000,
            part: object::PART_WORDS,
            mxcsr: 0x1f80,
        };
        (g, i)
    }

    /// The object answer calls the seven leaves in retail's order, each
    /// from the switch's depth with retail's arguments and registers, and
    /// leaves the copies and the sums where retail's arm does before the
    /// leaves that read them.
    #[test]
    fn the_object_answer_calls_the_leaves_in_retails_order() {
        use engine::entry::{self, Regs};
        let (g, i) = object_guest();
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_object(cx, i)) };
        let regs = Regs { esp: STACK, eax: i.source, ebx: 0x22, ebp: 0x33, esi: 0x44, edi: 0x55, ..Default::default() };
        let s = i.frame;
        let run = entry::testing::run(&f, g, regs, |c, m| {
            assert_eq!(c.esp(STACK), s - 4 * c.args.len() as u32 - 4, "{:#x} from the switch's depth", c.target);
            match c.target {
                object::CHECK => Regs { eax: 0xabcd_0001, ..Default::default() },
                object::PLACE => {
                    assert_eq!(m.bytes(s + object::COPIES, 36).unwrap(), (1..=9u32).flat_map(u32::to_le_bytes).collect::<Vec<_>>());
                    for (n, v) in [0.5f32, 0.25, 0.125].iter().enumerate() {
                        m.set_u32(c.args[3] + 4 * n as u32, v.to_bits()).unwrap();
                    }
                    Regs::default()
                }
                object::VELOCITY => {
                    let sums: Vec<f32> = (0..3).map(|n| f32::from_bits(m.u32(s + object::SUMS + 4 * n).unwrap())).collect();
                    assert_eq!(sums, [1.5, 2.25, 3.125]);
                    Regs::default()
                }
                object::NEW => Regs { eax: 0x0006_0002, ..Default::default() },
                object::ATTACH => Regs { eax: 0x77, ..Default::default() },
                _ => Regs::default(),
            }
        });
        assert_eq!(run.result, Ok(Ret::new(0x77, 0x14)));
        let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        use object::*;
        assert_eq!(targets, [INIT, CHECK, PLACE, VELOCITY, NEW, AFTER_NEW, ATTACH]);
        let c = &run.calls;
        let six = |c: &GuestCall| [c.eax, c.ecx, c.edx, c.ebx, c.esi, c.edi].map(Option::unwrap);
        assert_eq!((c[0].args.as_slice(), six(&c[0])), (i.init_args().as_slice(), i.init_regs()));
        assert_eq!((c[1].eax, c[1].esi), (Some(i.placement()), Some(i.placement())));
        assert_eq!((c[2].args.as_slice(), six(&c[2])), (i.place_args().as_slice(), i.place_regs()));
        assert_eq!((c[3].args.as_slice(), six(&c[3])), (i.velocity_args().as_slice(), i.velocity_regs()));
        assert_eq!(c[3].xmm0, Some(u128::from(3.125f32.to_bits())));
        assert_eq!((c[4].args.as_slice(), c[4].edx), ([i.placement()].as_slice(), Some(i.placement())));
        assert_eq!((c[5].args.as_slice(), c[5].esi), ([1, 0].as_slice(), Some(0x0006_0002)));
        assert_eq!(c[6].args, [0x0006_0002]);
        let saved: Vec<u32> = (0..4).map(|n| run.guest.u32(STACK - 0x104 - 4 * n).unwrap()).collect();
        assert_eq!(saved, [0x22, 0x33, 0x44, 0x55]);
    }

    /// `0xa7640` refusing, or `0xb7b40` making no object, returns as
    /// retail's arm does, with the leaf's `EAX`.
    #[test]
    fn the_object_answer_returns_where_retails_arm_does() {
        use engine::entry::{self, Regs};
        for (stop, eax, calls) in [(object::CHECK, 0xabcd_0000, 2), (object::NEW, u32::MAX, 5)] {
            let (g, i) = object_guest();
            let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_object(cx, i)) };
            let regs = Regs { esp: STACK, eax: i.source, ..Default::default() };
            let run = entry::testing::run(&f, g, regs, |c, _| Regs {
                eax: if c.target == stop { eax } else { 1 },
                ..Default::default()
            });
            assert_eq!((run.result, run.calls.len()), (Ok(Ret::new(eax, 0x14)), calls), "{stop:#x}");
        }
    }

    /// The sound answer: on the object one call with the registers and
    /// arguments; at a point the record in the switch's frame, `EDX` on it
    /// and the scale in `XMM0`.
    #[test]
    fn the_sound_answer_makes_retails_call() {
        use engine::entry::{self, Regs};
        let i = sound::Inputs { tag: 0xe100_0001, effect: EFFECT, effect_20: 0x22, object: 0x0004_0007, source_eax: 0x66, ..Default::default() };
        let on = sound::Call::OnObject { eax: 1, ecx: 2, ebx: 3, esi: 4, edx: 5, args: [6, 7, 8] };
        let record: [u8; sound::RECORD] = std::array::from_fn(|n| n as u8);
        let point = sound::Call::AtPoint { record, tag: 0xe100_0001, scale: 0.5f32.to_bits() };
        for call in [on, point] {
            let mut g = engine::testing::FakeGuest::new();
            g.map(STACK - 0x1000, 0x2000);
            let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_sound(cx, call, i)) };
            let regs = Regs { esp: STACK, eax: 0x66, ..Default::default() };
            let run = entry::testing::run(&f, g, regs, |c, m| {
                assert_eq!(c.esp(STACK), STACK - 0x110 - 4 * c.args.len() as u32 - 4);
                if c.target == sound::AT_POINT {
                    assert_eq!(m.bytes(c.edx.unwrap(), sound::RECORD).unwrap(), record);
                }
                Regs { eax: 0x99, ..Default::default() }
            });
            assert_eq!(run.result, Ok(Ret::new(0x99, 0x14)));
            let c = &run.calls[0];
            match call {
                sound::Call::OnObject { .. } => {
                    assert_eq!((c.target, c.args.as_slice()), (sound::ON_OBJECT, [6, 7, 8].as_slice()));
                    assert_eq!([c.eax, c.ecx, c.ebx, c.esi, c.edx, c.edi], [1, 2, 3, 4, 5, 0x66].map(Some));
                }
                sound::Call::AtPoint { .. } => {
                    assert_eq!((c.target, c.args.as_slice()), (sound::AT_POINT, [0xe100_0001].as_slice()));
                    assert_eq!(c.edx, Some(STACK - 0x100));
                    assert_eq!(c.xmm0, Some(u128::from(0.5f32.to_bits())));
                    assert_eq!([c.eax, c.ecx, c.ebx, c.esi, c.edi], sound::Call::point_regs(&i).map(Some));
                }
            }
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
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_damage(cx, record)) };
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
