//! The effects phase of the tick in Rust, always, with no switch
//! (docs/engine/effects-parts.md).
//!
//! Two pieces. The effect walk of `0x176e40` is the tick driver's Rust body
//! for the phase ([`crate::tick::BODIES`], `engine::effects_phase`). This
//! module is the other: an implementation of the part switch
//! `EFFECT_PART_CREATE` (`0x17a8a0`) that decodes each part and hands it to
//! [`sim_effects::dispatch::Parts`]:
//!
//! - every class the switch handles is answered in Rust from the first
//!   part, with retail's leaf calls in retail's order:
//!   - damage: `DAMAGE_AREA_APPLY` (`0xd6c80`, from `0x17aa4f`).
//!   - object: the seven leaves, `EFFECT_PART_PLACE` (`0x177d10`) and
//!     `EFFECT_PART_VELOCITY` (`0x177f60`) among them.
//!   - sound: `SOUND_START_ON_OBJECT` (`0x189060`) or `SOUND_START_AT_POINT`
//!     (`0x1895f0`).
//!   - character: `EFFECT_PART_CHARACTER` (`0x1ca290`), or an early return.
//!   - `coln`: the draw, then the switch's return.
//!   - decal: `EFFECT_PART_PLACE`, `DECAL_CREATE` (`0x17e670`) and the
//!     draws between them.
//!   - light: `LIGHT_CREATE` (`0xc0350`).
//! - `lens`, `MGS2` and `tdtl` return from Rust: retail's arm is the epilogue.
//! - decal, light and lens flare parts also become scene events
//!   ([`sim_effects::scene`]) the frame step puts in the renderer's scene.
//! - any other group sets retail's warning byte and returns.
//! - a part Rust cannot answer panics with its cause; a read retail's arm
//!   makes faulting raises that fault.
//!
//! No RNG draw moves: the damage, sound, light and character arms draw
//! nothing in the switch; the object arm's draws are inside `0x177d10` and
//! `0x177f60`, which Rust calls in retail's order; the `coln` and decal
//! draws are Rust's ([`sim_effects::random`]), made where retail makes
//! them.
//!
//! Hooks this adds: the implementation of `EFFECT_PART_CREATE` (the only
//! one there; `host_audit`'s observers stay outside it) and an
//! `observe_before` on `CACHE_TAGS_LOAD` beside the other arms' resets.

use std::sync::{Mutex, MutexGuard};

use guest_layout::entry::{Cx, EntryResult, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};
use sim_effects::damage::{self, Record};
use sim_effects::dispatch::{Action, Inputs, Parts, Stats};
use sim_effects::{character, coln, decal, light, object, random, sound, Part, PartClass};

/// The feature's id.
const ID: &str = "sim-effects-parts";

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

/// The counters now: the parts by class.
pub fn status() -> Stats {
    parts().stats()
}

fn to_x87(r: &runtime::cpu::X87Abi) -> x87::abi::X87Abi {
    x87::abi::X87Abi {
        st: r.st,
        control: r.control,
        status: r.status,
        tag: r.tag,
        opcode: r.opcode,
        instruction_pointer: r.instruction_pointer,
        data_pointer: r.data_pointer,
        instruction_selector: r.instruction_selector,
        data_selector: r.data_selector,
    }
}

/// The registers, tag, status and control of `x` over `cpu`'s
/// last-instruction pointers, which the helpers leave to generated code.
fn set_x87(cpu: &mut CpuAbi, x: &x87::abi::X87Abi) {
    (cpu.x87.st, cpu.x87.control, cpu.x87.status, cpu.x87.tag) = (x.st, x.control, x.status, x.tag);
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
        PartClass::Character => {
            let mut i = character::Inputs {
                entry: esp,
                effect,
                group,
                tag,
                object: g.u32(effect.wrapping_add(0x48))?,
                seconds: g.u32(effect.wrapping_add(0x64))?,
                location: cpu.ecx,
                a: g.u32(record.wrapping_add(0x14))?,
                b: g.u32(record.wrapping_add(0x18))?,
                source: cpu.eax,
                ..character::Inputs::default()
            };
            if tag != u32::MAX {
                i.node = g.u16(cpu.ecx.wrapping_add(2))?;
            }
            Inputs::Character(i)
        }
        PartClass::Coln => {
            let seed_at = g.u32(random::STREAM)?.wrapping_add(4);
            Inputs::Coln(coln::Inputs {
                entry: esp,
                seed_at,
                seed: g.u32(seed_at)?,
                lo: g.u32(record.wrapping_add(0x28))?,
                hi: g.u32(record.wrapping_add(0x2c))?,
                scale: g.u32(random::SCALE)?,
                fpu: to_x87(&cpu.x87),
            })
        }
        PartClass::Decal => {
            let rng = g.u32(random::STREAM)?;
            let mut part = [0; 7];
            for (w, &o) in part.iter_mut().zip(&decal::PART_WORDS) {
                *w = g.u32(record.wrapping_add(o))?;
            }
            Inputs::Decal(decal::Inputs {
                entry: esp,
                effect,
                tag,
                flags: g.u8(record.wrapping_add(6))?,
                rng,
                seed: g.u32(rng.wrapping_add(4))?,
                position_ptr: arg(2)?,
                direction_ptr,
                source: cpu.eax,
                ecx: cpu.ecx,
                edx: cpu.edx,
                part,
                scale: g.u32(random::SCALE)?,
                limit: g.u32(decal::LIMIT)?,
                radius: g.u32(decal::RADIUS)?,
                fpu: to_x87(&cpu.x87),
            })
        }
        PartClass::Light => {
            let object = g.u32(effect.wrapping_add(0x48))?;
            let dl = cpu.edx as u8;
            let mut i = light::Inputs {
                group,
                tag,
                object,
                dl,
                location: cpu.ecx,
                edx: cpu.edx,
                source: cpu.eax,
                position_ptr: arg(2)?,
                direction_ptr,
                scale,
                ..light::Inputs::default()
            };
            if object != u32::MAX && dl == 0 {
                i.node = g.u16(cpu.ecx.wrapping_add(2))?;
            }
            Inputs::Light(i)
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
    // The adds run under the MXCSR as it stands; one that raises an
    // unmasked exception is the guest's `#XM`.
    let Ok(sums) = object::sums(i.mxcsr, offset, placed) else {
        unimplemented!("effects parts: an object part's offset add raises an unmasked exception (MXCSR {:#x})", i.mxcsr);
    };
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

/// A `char` part that makes its calls: `0x1469f0(seconds)` with the other
/// five arguments already pushed under it, as retail's arm pushes them,
/// then `0x1ca290(ticks, ...)` with `ESI` = the tag.
async fn answer_character(cx: Cx, i: character::Inputs, node: u32) -> EntryResult {
    let e = cx.entry();
    save_regs(&cx)?;
    let rest = i.start_args(node);
    let below = e.esp.wrapping_sub(FRAME + 4 * rest.len() as u32);
    cx.write(|m: &mut dyn GuestMemory| put_words(m, below, &rest))?;
    let [eax, ecx, edx] = i.ticks_regs();
    let r = cx
        .call(
            GuestCall::new(character::TICKS)
                .args([i.seconds])
                .reserve(FRAME + 4 * rest.len() as u32)
                .eax(eax)
                .ecx(ecx)
                .edx(edx)
                .ebx(i.effect)
                .edi(i.source),
        )
        .await?;
    let ticks = r.eax;
    let start = std::iter::once(ticks).chain(rest);
    let r = cx
        .call(GuestCall::new(points::EFFECT_PART_CHARACTER.base).args(start).reserve(FRAME).eax(ticks).ebx(i.effect).esi(i.tag).edi(i.source))
        .await?;
    Ok(Ret::new(r.eax, POP as u16))
}

/// A decal draw under the x87 control word as it stands: one that raises
/// an unmasked exception is the guest's `#MF`.
fn drawn<T>(r: Result<T, random::Refused>) -> T {
    r.unwrap_or_else(|random::Refused| unimplemented!("effects parts: a decal draw raises an unmasked x87 exception"))
}

fn read_u32(cx: &Cx, at: u32) -> Result<u32, MemFault> {
    cx.read(|m: &dyn GuestMemory| m.u32(at))
}

/// A decal part: retail's leaves in order, the draws between them in Rust
/// on a copy of the entry's x87 state (docs/engine/effects-parts.md "How it
/// works", step 9). The stream's seed is read where retail's draws read it.
async fn answer_decal(cx: Cx, i: decal::Inputs) -> EntryResult {
    save_regs(&cx)?;
    let mut fpu = i.fpu;
    if i.gated() {
        let r = cx.call(leaf(decal::NEAR, &[], i.near_regs()).xmm0(u128::from(i.radius))).await?;
        if r.eax & 0xff == 0 {
            let (seed, on) = drawn(i.gate(&mut fpu, read_u32(&cx, i.seed_at())?));
            cx.write(|m: &mut dyn GuestMemory| m.set_u32(i.seed_at(), seed))?;
            if !on {
                return Ok(Ret::new(seed >> 16, POP as u16));
            }
        }
    }
    cx.call(leaf(object::PLACE, &i.place_args(), i.place_regs())).await?;
    let (seed, value) = drawn(i.draw(&mut fpu, read_u32(&cx, i.seed_at())?));
    cx.write(|m: &mut dyn GuestMemory| m.set_u32(i.seed_at(), seed))?;
    let decals = read_u32(&cx, decal::DECALS)?;
    let r = cx.call(leaf(decal::CREATE, &i.create_args(value), i.create_regs(decals))).await?;
    Ok(Ret::new(r.eax, POP as u16))
}

/// A light part: one call, as [`light::Call`] built it.
async fn answer_light(cx: Cx, call: light::Call) -> EntryResult {
    save_regs(&cx)?;
    let r = cx.call(leaf(light::CREATE, &call.args, call.regs).xmm0(u128::from(call.xmm0))).await?;
    Ok(Ret::new(r.eax, POP as u16))
}

/// Return from the switch with `eax` (and the other registers as they
/// are): retail's arm made no call, or only draws Rust made itself.
fn return_now(cpu: &mut CpuAbi, ctx: &mut ExecCtx, eax: u32) -> Result<TransferAbi, GuestFault> {
    let target = ctx.memory().u32(cpu.esp)?;
    cpu.eax = eax;
    cpu.esp = cpu.esp.wrapping_add(4 + POP);
    cpu.eflags_defined &= !ARITH;
    Ok(TransferAbi::ret(target))
}

fn enter(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32) -> Result<TransferAbi, GuestFault> {
    // A read retail's arm makes faulting is the guest's fault, raised here.
    let (part, inputs) = {
        let mem = ctx.memory();
        decode(&mem, cpu)?
    };
    let group = part.group;
    let action = {
        let mut queue = sim_effects::scene::QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        parts().enter(&part, inputs.as_ref(), &mut queue)
    };
    match action {
        Action::Return => return_now(cpu, ctx, group),
        Action::Warn => {
            // `0x17adb3`: `mov al, [WARNED]`; set it to 1 if it was 0.
            let mut mem = ctx.memory();
            let warned = mem.u8(sim_effects::dispatch::WARNED)?;
            if warned == 0 {
                mem.set_u8(sim_effects::dispatch::WARNED, 1)?;
            }
            drop(mem);
            return_now(cpu, ctx, (group & !0xff) | u32::from(warned))
        }
        Action::Damage(call) => {
            hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_damage(cx, call.record)))
        }
        Action::Object(i) => hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_object(cx, i))),
        Action::Sound(call, i) => {
            hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_sound(cx, call, i)))
        }
        Action::Character(call, i) => match call.plan {
            character::Plan::Return(eax) => return_now(cpu, ctx, eax),
            character::Plan::Start { node } => {
                hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_character(cx, i, node)))
            }
        },
        Action::Coln(call) => {
            // 0x259d0's `push ecx` scratch is dead stack below the frame.
            ctx.memory().set_u32(call.seed_at, call.seed)?;
            set_x87(cpu, &call.fpu);
            (cpu.ecx, cpu.edx) = (call.ecx, call.edx);
            return_now(cpu, ctx, call.eax)
        }
        Action::Decal(i) => hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_decal(cx, i))),
        Action::Light(call) => hooks::entry::start_with(cpu, ctx, pc, move |cx| Box::pin(answer_light(cx, call))),
    }
}

pub struct SimEffectsParts;

impl Feature for SimEffectsParts {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The effects phase in Rust: the effect walk and an arm for each part class")
            .doc("docs/engine/effects-parts.md")
            .always_on()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The counters and the scene queue are the process's, not the
        // game's.
        b.snapshot(SnapshotPolicy::Stateless);
        b.raw().implement(&points::EFFECT_PART_CREATE, "effect part arm", enter).context_only();
        // The effect walk's four callees, through their own arm.
        crate::effect_update::install(b);
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
    fn the_feature_installs_the_arm() {
        {
            let mut registry = Registry::new(Switches::defaults());
            registry.add(&SimEffectsParts).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(plan.chains.contains_key(&points::EFFECT_PART_CREATE.base));
            // No observer compares the leaves: the arms are Rust's alone.
            assert!(!plan.chains.contains_key(&points::DAMAGE_AREA_APPLY.base));
        }
    }

    /// The named points are the leaves the arms call.
    #[test]
    fn the_points_are_the_arms_leaves() {
        assert_eq!(points::EFFECT_PART_PLACE.base, object::PLACE);
        assert_eq!(points::EFFECT_PART_VELOCITY.base, object::VELOCITY);
        assert_eq!(points::SOUND_START_ON_OBJECT.base, sound::ON_OBJECT);
        assert_eq!(points::SOUND_START_AT_POINT.base, sound::AT_POINT);
        assert_eq!(points::EFFECT_PART_CHARACTER.base, character::START);
        assert_eq!(points::DECAL_CREATE.base, decal::CREATE);
        assert_eq!(points::LIGHT_CREATE.base, light::CREATE);
    }

    const STACK: u32 = 0x0030_0000;
    const EFFECT: u32 = 0x0040_0000;
    const RECORD: u32 = 0x0040_1000;
    const VECS: u32 = 0x0040_2000;

    /// A switch entry for an object part: the effect, the part record
    /// (words `+0x14..+0x34` equal to their offsets), the three vectors
    /// and the stream pointer, as the switch's caller leaves them.
    fn object_guest() -> (guest_layout::testing::FakeGuest, object::Inputs) {
        let mut g = guest_layout::testing::FakeGuest::new();
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
        use guest_layout::entry::{self, Regs};
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
        use guest_layout::entry::{self, Regs};
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
        use guest_layout::entry::{self, Regs};
        let i = sound::Inputs { tag: 0xe100_0001, effect: EFFECT, effect_20: 0x22, object: 0x0004_0007, source_eax: 0x66, ..Default::default() };
        let on = sound::Call::OnObject { eax: 1, ecx: 2, ebx: 3, esi: 4, edx: 5, args: [6, 7, 8] };
        let record: [u8; sound::RECORD] = std::array::from_fn(|n| n as u8);
        let point = sound::Call::AtPoint { record, tag: 0xe100_0001, scale: 0.5f32.to_bits() };
        for call in [on, point] {
            let mut g = guest_layout::testing::FakeGuest::new();
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
        use guest_layout::entry::{self, Regs};
        use guest_layout::testing::FakeGuest;
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
    /// The `char` answer: the ticks call with the other five arguments
    /// pushed under it, as retail's arm pushes them, then the start with
    /// the ticks first and `ESI` the tag.
    #[test]
    fn the_character_answer_pushes_as_retail() {
        use guest_layout::entry::{self, Regs};
        let i = character::Inputs {
            entry: STACK,
            effect: EFFECT,
            tag: 0xe100_0003,
            object: 0x0004_0007,
            seconds: 2.0f32.to_bits(),
            location: 0x0060_0000,
            a: 0x14,
            b: 0x18,
            source: 0x66,
            ..character::Inputs::default()
        };
        let mut g = guest_layout::testing::FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_character(cx, i, 5)) };
        let regs = Regs { esp: STACK, eax: 0x66, ..Default::default() };
        let run = entry::testing::run(&f, g, regs, |c, m| {
            if c.target == character::TICKS {
                assert_eq!(c.esp(STACK), STACK - 0x110 - 0x18 - 4, "retail's depth");
                let under: Vec<u32> = (0..5).map(|n| m.u32(STACK - 0x110 - 0x14 + 4 * n).unwrap()).collect();
                assert_eq!(under, i.start_args(5));
                return Regs { eax: 60, ..Default::default() };
            }
            Regs { eax: 0x99, ..Default::default() }
        });
        assert_eq!(run.result, Ok(Ret::new(0x99, 0x14)));
        let c = &run.calls;
        assert_eq!((c[0].args.as_slice(), [c[0].eax, c[0].ecx, c[0].edx]), ([i.seconds].as_slice(), i.ticks_regs().map(Some)));
        assert_eq!(c[1].target, character::START);
        assert_eq!(c[1].args, [60, 0x0004_0007, 5, 0x14, 0x18, 0x0060_0008]);
        assert_eq!((c[1].esi, c[1].ebx, c[1].edi), (Some(i.tag), Some(EFFECT), Some(0x66)));
    }

    fn decal_inputs() -> decal::Inputs {
        decal::Inputs {
            entry: STACK,
            effect: EFFECT,
            tag: 0xe100_0005,
            flags: 0,
            rng: 0x0070_0000,
            seed: 0,
            position_ptr: VECS,
            direction_ptr: VECS + 0x10,
            source: 0x66,
            ecx: 0,
            edx: 0,
            part: [0x14, 0x18, 0x1c, 0x30, 0x34, 0, 1.0f32.to_bits()],
            scale: 0x3780_0000,
            limit: 0.5f32.to_bits(),
            radius: 4.0f32.to_bits(),
            fpu: x87::abi::X87Abi { control: 0x027f, ..x87::abi::X87Abi::default() },
        }
    }

    /// The decal answer: near, the gate's draw and its stop, the place, the
    /// draw from what the place left, the create with that `f32`; the
    /// stream word holds each seed where retail's draws leave it.
    #[test]
    fn the_decal_answer_draws_between_the_leaves() {
        use guest_layout::entry::{self, Regs};
        let stream = 0x0070_0004;
        for (near, limit, want) in [
            (1, 0.5f32, vec![decal::NEAR, object::PLACE, decal::CREATE]),
            (0, 0.5, vec![decal::NEAR, object::PLACE, decal::CREATE]),
            (0, 0.1, vec![decal::NEAR]),
        ] {
            let i = decal::Inputs { limit: limit.to_bits(), ..decal_inputs() };
            let mut g = guest_layout::testing::FakeGuest::new();
            g.map(STACK - 0x1000, 0x2000).map(0x0070_0000, 0x1000).map(decal::DECALS & !0xfff, 0x1000);
            g.put_u32(stream, 0).put_u32(decal::DECALS, 0x0080_0000);
            let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_decal(cx, i)) };
            let regs = Regs { esp: STACK, eax: 0x66, ..Default::default() };
            let gate = if near == 0 { random::next(0) } else { 0 };
            let run = entry::testing::run(&f, g, regs, |c, m| {
                assert_eq!(c.esp(STACK), STACK - 0x110 - 4 * c.args.len() as u32 - 4);
                match c.target {
                    decal::NEAR => Regs { eax: 0xab00 | near, ..Default::default() },
                    object::PLACE => {
                        assert_eq!(m.u32(stream).unwrap(), gate, "the gate drew before the place");
                        m.set_u32(stream, 77).unwrap();
                        Regs::default()
                    }
                    decal::CREATE => {
                        let seed = random::next(77);
                        assert_eq!(m.u32(stream).unwrap(), seed);
                        assert_eq!(c.args, i.create_args(((seed >> 16) as f32 / 65536.0).to_bits()));
                        assert_eq!(c.ecx, Some(0x0080_0000));
                        Regs { eax: 0x99, ..Default::default() }
                    }
                    t => panic!("{t:#x}"),
                }
            });
            let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
            assert_eq!(targets, want, "near {near}, limit {limit}");
            if want.len() == 1 {
                assert_eq!(run.result, Ok(Ret::new(random::next(0) >> 16, 0x14)));
                assert_eq!(run.guest.u32(stream).unwrap(), random::next(0));
            } else {
                assert_eq!(run.result, Ok(Ret::new(0x99, 0x14)));
            }
            let c = &run.calls[0];
            assert_eq!((c.eax, c.edi, c.xmm0), (Some(1), Some(VECS), Some(u128::from(4.0f32.to_bits()))));
        }
        // Not gated: no near call, EDI the entry EAX.
        let i = decal::Inputs { flags: 0x10, ..decal_inputs() };
        let mut g = guest_layout::testing::FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000).map(0x0070_0000, 0x1000).map(decal::DECALS & !0xfff, 0x1000);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_decal(cx, i)) };
        let run = entry::testing::run(&f, g, Regs { esp: STACK, eax: 0x66, ..Default::default() }, |_, _| Regs::default());
        assert_eq!(run.calls.iter().map(|c| (c.target, c.edi)).collect::<Vec<_>>(), [(object::PLACE, Some(0x66)), (decal::CREATE, Some(0x66))]);
    }

    #[test]
    fn the_light_answer_makes_retails_call() {
        use guest_layout::entry::{self, Regs};
        let call = light::Call::predict(&light::Inputs { object: u32::MAX, tag: 4, scale: 0.5f32.to_bits(), ..light::Inputs::default() });
        let mut g = guest_layout::testing::FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000);
        let f = move |cx: Cx| -> hooks::EntryFuture { Box::pin(answer_light(cx, call)) };
        let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |c, _| {
            assert_eq!(c.esp(STACK), STACK - 0x110 - 0x14);
            Regs { eax: 0x99, ..Default::default() }
        });
        assert_eq!(run.result, Ok(Ret::new(0x99, 0x14)));
        let c = &run.calls[0];
        assert_eq!((c.target, c.args.as_slice(), c.xmm0), (light::CREATE, call.args.as_slice(), Some(u128::from(0.5f32.to_bits()))));
        assert_eq!([c.eax, c.ecx, c.edx, c.ebx, c.esi, c.edi], call.regs.map(Some));
    }
}
