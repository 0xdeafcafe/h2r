//! One light's passes, `0x00031170` `render_light_passes`
//! (docs/graphics/native-light-gather.md, "One light's passes", slice L5).
//!
//! [`plan`] reads and refuses before any write. [`run`] then makes the
//! guest's steps in its order (**E** `0x00031170`..`0x00031518`), each
//! value read when the guest reads it:
//!
//! 1. the item lists `0x00040e90` (EAX 0), a guest call (the item body
//!    of [`super::items`] runs behind its hook);
//! 2. a set shadow byte becomes the casters' layer 10, written back into
//!    the argument (`0x0003119b`);
//! 3. the constants `0x0004a1e0`, a guest call;
//! 4. the flags (`crate::light_passes::flags_of`) and their stores;
//! 5. with the shadow flag, the shadow projection `0x00048ee0` and layer
//!    10 (`0x00044370`, and when it answers AL, its draw and end); layer
//!    11 when the light casts its shadow, has a specular colour or its
//!    definition has bit 5; the additional casters `0x00030ed0`; layers
//!    14, 12 and, with flag bits 0 and 1, 13. Each of 11 to 14: its layer
//!    words and target when the light's list has the layer
//!    (`0x00014bc0`, `0x00015680`), the stencil off (`0x00015370`), and
//!    the draw `0x00044550` and end `0x000445d0` when the list has it.
//!    Layer 11's begin also calls `0x000496f0`;
//! 6. the tail: two zeroes, stencil off (`0x003f5590`), the scissor
//!    cleared (`0x003f8760`), the depth range (`0x00015720`), the view's
//!    target (`0x00014bc0`), and both flag words zeroed.
//!
//! Every draw is the guest's, so the stages that take a layer from the
//! scene apply unchanged. A hook that tests the return address of one of
//! these calls reads [`caller`]: while the body makes a call, the slot of
//! its return address names the guest's return site.
//!
//! The guest keeps four saved registers below its entry `ESP`; the body
//! makes its calls below them ([`FRAME`]), so each callee runs at the
//! guest's `ESP`. The layer's draw byte is the shadow argument's low byte,
//! as in the guest (`[esp+1Ch]`).

pub mod additional;

use std::cell::Cell;

use super::guest::TAGS;
use super::{Result, visible};
use guest_layout::entry::{Cx, EntryError, GuestCall, Regs, Ret};
use crate::light_constants::Fpu;
use crate::light_passes::{self as lp, FlagInputs, flag};
use guest_layout::memory::{GuestMemory, MemFault};

/// The saved registers below the entry `ESP`.
pub const FRAME: u32 = 0x10;

/// The guest functions the body calls.
pub mod call {
    use guest_layout::points;
    pub const ITEMS: u32 = points::SCENE_ITEMS.base;
    /// The light's constants: EAX the frame block, ECX the context, EDX
    /// the light datum; stack (definition, the image's word, shadow,
    /// weight), `ret 0x10`.
    pub const CONSTANTS: u32 = points::LIGHT_CONSTANTS.base;
    /// The shadow projection: ECX the context, EBX the frame block; stack
    /// (definition), `ret 4`.
    pub const SHADOW_SETUP: u32 = 0x0004_8ee0;
    /// The layer-begin check: EDI the layer; AL whether it draws.
    pub const LAYER_BEGIN: u32 = 0x0004_4370;
    pub const LAYER_DRAW: u32 = points::LAYER_DRAW.base;
    pub const LAYER_END: u32 = points::LAYER_END.base;
    /// The render target: AL 1; stack (target, 0), `ret 8`.
    pub const TARGET: u32 = points::LIGHT_RESTORE_TARGET.base;
    /// The depth clip mode: AX 0.
    pub const CLIP_MODE: u32 = 0x0001_5680;
    /// The stencil mode: BX 0, off.
    pub const STENCIL: u32 = points::RENDER_STENCIL_MODE.base;
    /// Layer 11's blend for its begin.
    pub const RECEIVER_BLEND: u32 = 0x0004_96f0;
    pub const ADDITIONAL_CASTERS: u32 = points::ADDITIONAL_SHADOW_CASTERS.base;
    /// `SetRenderState_StencilEnable`: stack (0), `ret 4`.
    pub const STENCIL_ENABLE: u32 = 0x003f_5590;
    /// `SetScissors`: stack (0, 0, 0), `ret 0xc`.
    pub const SCISSORS: u32 = 0x003f_8760;
    /// The depth range: stack (0, 0), `ret 8`.
    pub const DEPTH_RANGE: u32 = 0x0001_5720;
}

/// The guest's return address after each of its calls.
pub mod site {
    pub const ITEMS: u32 = 0x0003_1197;
    pub const CONSTANTS: u32 = 0x0003_11c8;
    pub const SHADOW_SETUP: u32 = 0x0003_129f;
    pub const CASTER_BEGIN: u32 = 0x0003_12a9;
    pub const CASTER_DRAW: u32 = 0x0003_12b2;
    pub const CASTER_END: u32 = 0x0003_12b7;
    pub const ADDITIONAL_CASTERS: u32 = 0x0003_137d;
    pub const STENCIL_ENABLE: u32 = 0x0003_14e8;
    pub const SCISSORS: u32 = 0x0003_14f0;
    pub const DEPTH_RANGE: u32 = 0x0003_14f7;
    pub const VIEW_TARGET: u32 = 0x0003_1508;
}

/// The constants' argument words, pushed at `0x000311b3`..`0x000311be`.
pub const CONSTANTS_ARGS: u32 = 0x10;

/// The whole guest body's call to the constants, as the callee
/// (`points::LIGHT_CONSTANTS`) sees it at its entry: the return word is
/// [`site::CONSTANTS`], so the call is this body's own and not one a Rust
/// body makes (whose return slot holds a continuation token). A hook on
/// the item lists' return site `site::ITEMS` would never run: a return to
/// a call continuation resumes generated code without applying entry
/// overrides (docs/lifter/LIFTER.md, section 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConstantsCall {
    /// The entry `ESP` of the invocation that made the call.
    pub entry_esp: u32,
    /// The four pushed words: the definition, the image's word, the
    /// shadow argument and the weight.
    pub args: [u32; 4],
}

impl ConstantsCall {
    /// `None` when the return word at `esp` is not this body's.
    pub fn read(m: &dyn GuestMemory, esp: u32) -> Result<Option<Self>, MemFault> {
        if m.u32(esp)? != site::CONSTANTS {
            return Ok(None);
        }
        let arg = |n: u32| m.u32(esp.wrapping_add(4 + 4 * n));
        Ok(Some(Self {
            entry_esp: esp.wrapping_add(4 + CONSTANTS_ARGS + FRAME),
            args: [arg(0)?, arg(1)?, arg(2)?, arg(3)?],
        }))
    }

    /// Whether the call is the one the body makes from `plan`'s entry:
    /// the registers (EAX the frame block, ECX the context, EDX the light)
    /// and the four pushed words, the shadow and weight copied from the
    /// argument slots as the guest pushes them.
    pub fn matches(
        &self,
        m: &dyn GuestMemory,
        plan: &Plan,
        regs: [u32; 3],
    ) -> Result<bool, MemFault> {
        Ok(self.entry_esp == plan.entry_esp
            && regs == [plan.position, plan.context, plan.light]
            && self.args
                == [
                    plan.data,
                    m.u32(CONSTANTS_WORD)?,
                    m.u32(plan.shadow_at())?,
                    m.u32(plan.entry_esp.wrapping_add(0x10))?,
                ])
    }

    /// `ESP` at [`site::VIEW_TARGET`]: on the saved registers ([`FRAME`]),
    /// as after the view target's `ret 8`. Resuming there runs the
    /// guest's own epilogue, which restores those registers and returns
    /// with `ret 0x10`.
    pub fn view_target_esp(&self) -> u32 {
        self.entry_esp.wrapping_sub(FRAME)
    }
}

/// One of layers 11 to 14's blocks, by the guest's return sites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layer {
    pub layer: u32,
    /// After the target call, the clip mode call, the stencil call, the
    /// draw and the end.
    pub target: u32,
    pub clip: u32,
    /// Layer 11's: after `0x000496f0`.
    pub blend: Option<u32>,
    pub stencil: u32,
    pub draw: u32,
    pub end: u32,
}

pub const RECEIVERS: Layer = Layer {
    layer: 11,
    target: 0x0003_1317,
    clip: 0x0003_1323,
    blend: Some(0x0003_132d),
    stencil: 0x0003_1340,
    draw: 0x0003_134d,
    end: 0x0003_1352,
};
pub const TARGET_1: Layer = Layer {
    layer: 14,
    target: 0x0003_13b6,
    clip: 0x0003_13c2,
    blend: None,
    stencil: 0x0003_13d5,
    draw: 0x0003_13e2,
    end: 0x0003_13e7,
};
pub const DIFFUSE: Layer = Layer {
    layer: 12,
    target: 0x0003_1425,
    clip: 0x0003_1431,
    blend: None,
    stencil: 0x0003_1444,
    draw: 0x0003_144f,
    end: 0x0003_1454,
};
pub const SPECULAR: Layer = Layer {
    layer: 13,
    target: 0x0003_14a7,
    clip: 0x0003_14b3,
    blend: None,
    stencil: 0x0003_14c2,
    draw: 0x0003_14cf,
    end: 0x0003_14d4,
};

/// The light's list and its layer mask.
const LIGHT_LIST: u32 = 0x004c_1a60;
const LIGHT_MASK: u32 = LIGHT_LIST + 8;
/// The casters' list's mask, whose layer 10 keeps a set shadow byte.
const CASTER_MASK: u32 = 0x004c_1a80;
/// The word the constants call is pushed (the immediate of the `push` at
/// `0x000311b9`), read from the image.
const CONSTANTS_WORD: u32 = 0x0003_11ba;
/// The layer words: the layer, its second word and its list.
const LAYER_WORD: u32 = 0x0046_7130;
const LAYER_WORD_B: u32 = 0x0046_7134;
const LAYER_LIST: u32 = 0x0046_7138;
/// The current and the view's render target.
const CURRENT_TARGET: u32 = 0x0048_58b4;
const VIEW_TARGET: u32 = 0x0048_58b8;
/// The additional casters' override (byte `+5`) and their count.
const OVERRIDE: u32 = lp::global::OVERRIDE;
const ADDITIONAL_COUNT: u32 = 0x004b_a134;
/// The tail's two zeroes (`0x000314d7`, `0x000314dd`).
const LIGHT_ACTIVE: u32 = 0x0048_58bc;
const STENCIL_STATE: u32 = 0x004b_843c;
/// The definition flag that runs layer 11 without a shadow.
const DEFINITION_RECEIVERS: u8 = 0x20;

thread_local! {
    /// While the body makes a guest call: the slot its return address was
    /// pushed at, and the guest's return site for that call.
    static CALL: Cell<Option<(u32, u32)>> = const { Cell::new(None) };
}

/// The guest's return site for the return address at `slot`, while the
/// body makes the call that pushed it; `None` otherwise (a deeper call,
/// or no body).
pub fn return_site(slot: u32) -> Option<u32> {
    CALL.get()
        .filter(|&(at, _)| at == slot)
        .map(|(_, site)| site)
}

/// The return address at `slot` as the guest's own code would have pushed
/// it: [`return_site`], else the word there.
pub fn caller(m: &dyn GuestMemory, slot: u32) -> Option<u32> {
    return_site(slot).or_else(|| m.u32(slot).ok())
}

/// What [`run`] starts from: the registers and stack words at the entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub entry_esp: u32,
    pub fpu: Fpu,
    /// EAX: the `ligh` datum.
    pub definition: u32,
    /// ECX: the posed volume.
    pub context: u32,
    /// The definition's address (ESI from `0x0003118c`).
    pub data: u32,
    /// Stack 0 and 1: the light datum and its frame block.
    pub light: u32,
    pub position: u32,
}

impl Plan {
    fn shadow_at(&self) -> u32 {
        self.entry_esp.wrapping_add(0xc)
    }
}

/// Reads and refuses before any write: the float state the flag code's
/// comparisons run under, the arguments, the definition, and every global
/// the body reads.
pub fn plan(
    m: &dyn GuestMemory,
    entry_esp: u32,
    definition: u32,
    context: u32,
    fpu: Fpu,
) -> Result<Plan> {
    visible::fp_supported(fpu)?;
    let arg = |n: u32| m.u32(entry_esp.wrapping_add(4 + 4 * n));
    let (light, position) = (arg(0)?, arg(1)?);
    arg(2)?;
    arg(3)?;
    // The saved registers' slots are where the calls go below.
    m.bytes(entry_esp.wrapping_sub(FRAME), FRAME as usize)?;
    let data = m.u32(
        m.u32(TAGS)?
            .wrapping_add((definition & 0xffff) << 4)
            .wrapping_add(8),
    )?;
    m.u8(data)?;
    m.u16(data.wrapping_add(4))?;
    m.u16(data.wrapping_add(0x14))?;
    m.u16(context)?;
    FlagInputs::read(m, data, context)?;
    for global in [
        CONSTANTS_WORD,
        LIGHT_MASK,
        CASTER_MASK,
        OVERRIDE,
        ADDITIONAL_COUNT,
        LAYER_WORD,
        LAYER_WORD_B,
        LAYER_LIST,
        CURRENT_TARGET,
        VIEW_TARGET,
        lp::global::PASS_FLAGS,
        lp::constant::SPECULAR,
    ] {
        m.u32(global)?;
    }
    m.u8(LIGHT_ACTIVE)?;
    m.u32(STENCIL_STATE)?;
    m.u8(lp::global::PASSES)?;
    let over = m.u32(OVERRIDE)?;
    if over != 0 {
        m.u8(over.wrapping_add(5))?;
    }
    Ok(Plan {
        entry_esp,
        fpu,
        definition,
        context,
        data,
        light,
        position,
    })
}

/// What one call did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightPasses {
    /// The shadow byte after step 2.
    pub shadow: bool,
    /// `[0x004ba014]` as the flag code set it.
    pub flags: u32,
    /// The layers whose block ran, as a mask.
    pub reached: u32,
    /// The layers that drew: layer 10 when its begin answered, the others
    /// by the light's list's mask.
    pub drawn: u32,
    pub additional_casters: bool,
}

/// Puts back the naming [`CALL`] held before a call when the call
/// returns, or when the body is dropped in it (a load). A body whose
/// entry is another body's named call (the first-person lights' record
/// call, `super::first_person`) names its own calls inside it and leaves
/// the outer one named after.
struct Restore(Option<(u32, u32)>);

impl Drop for Restore {
    fn drop(&mut self) {
        CALL.set(self.0);
    }
}

/// A guest call made `reserve` bytes below `entry_esp`, its return slot
/// naming the guest's return `site` while it runs ([`return_site`]).
pub(super) async fn named(
    cx: &Cx,
    entry_esp: u32,
    reserve: u32,
    site: u32,
    call: GuestCall,
) -> Result<Regs, EntryError> {
    let call = call.reserve(reserve);
    let _restore = Restore(CALL.replace(Some((call.esp(entry_esp), site))));
    cx.call(call).await
}

/// A guest call below the frame, its return slot naming `site` while it
/// runs.
async fn guest(cx: &Cx, plan: &Plan, site: u32, call: GuestCall) -> Result<Regs, EntryError> {
    named(cx, plan.entry_esp, FRAME, site, call).await
}

/// Makes the passes. Returns as `0x00031170` does (`ret 0x10`, EAX what
/// the last target call left) with what it did.
pub async fn run(cx: &Cx, plan: Plan) -> Result<(Ret, LightPasses), EntryError> {
    run_with(cx, plan, |_, _| Ok(None)).await
}

/// The gather owns the item lists only until this invocation ends. Let a
/// renderer reserve the remaining passes together after the lists are ready.
/// A refusal continues at the shadow byte with those same lists.
pub async fn run_with(
    cx: &Cx,
    plan: Plan,
    typed: impl FnOnce(&mut dyn GuestMemory, &Plan) -> Result<Option<(Ret, LightPasses)>, EntryError>,
) -> Result<(Ret, LightPasses), EntryError> {
    let p = &plan;
    let regs = |call: GuestCall| call.esi(p.data).edi(p.context);

    // 1: the item lists.
    let tags = cx.read(|m| m.u32(TAGS))?;
    guest(
        cx,
        p,
        site::ITEMS,
        regs(GuestCall::new(call::ITEMS)).eax(0).ecx(tags),
    )
    .await?;

    if let Some(issued) = cx.write(|m| typed(m, p))? {
        return Ok(issued);
    }
    let mut out = LightPasses::default();

    // 2: the shadow byte.
    let mut shadow = cx.read(|m| m.u8(p.shadow_at()))?;
    if shadow != 0 {
        shadow = cx.write(|m| -> Result<u8, MemFault> {
            let b = ((m.u32(CASTER_MASK)? >> 10) & 1) as u8;
            m.set_u8(p.shadow_at(), b)?;
            Ok(b)
        })?;
    }
    out.shadow = shadow != 0;

    // 3: the constants.
    let (word, shadow_word, weight) = cx.read(|m| -> Result<_, MemFault> {
        Ok((
            m.u32(CONSTANTS_WORD)?,
            m.u32(p.shadow_at())?,
            m.u32(p.entry_esp.wrapping_add(0x10))?,
        ))
    })?;
    let request = regs(GuestCall::new(call::CONSTANTS))
        .eax(p.position)
        .ecx(p.context)
        .edx(p.light)
        .args([p.data, word, shadow_word, weight]);
    guest(cx, p, site::CONSTANTS, request).await?;

    // 4: the flags.
    let flags = cx.write(|m| -> Result<u32, EntryError> {
        let inputs = FlagInputs::read(m, p.data, p.context)?;
        let f = lp::flags_of(m, &inputs, shadow, p.fpu.mxcsr)
            .map_err(|e| EntryError::IsolatedCall(format!("light pass flags: {e:?}")))?;
        f.write(m)?;
        Ok(f.flags)
    })?;
    out.flags = flags;

    // 5: layer 10.
    let mut flags = flags;
    if flags & flag::SHADOW != 0 {
        out.reached |= 1 << 10;
        let request = regs(GuestCall::new(call::SHADOW_SETUP))
            .eax(flags)
            .ecx(p.context)
            .ebx(p.position)
            .args([p.data]);
        guest(cx, p, site::SHADOW_SETUP, request).await?;
        let begin = GuestCall::new(call::LAYER_BEGIN)
            .esi(p.data)
            .edi(10)
            .ebx(p.position);
        let begun = guest(cx, p, site::CASTER_BEGIN, begin).await?.eax & 0xff != 0;
        if begun {
            out.drawn |= 1 << 10;
            let draw = GuestCall::new(call::LAYER_DRAW)
                .esi(p.data)
                .edi(10)
                .ebx(p.position);
            guest(cx, p, site::CASTER_DRAW, draw).await?;
            let end = GuestCall::new(call::LAYER_END)
                .esi(p.data)
                .edi(10)
                .ebx(p.position);
            guest(cx, p, site::CASTER_END, end).await?;
        }
        flags = cx.read(|m| m.u32(lp::global::PASS_FLAGS))?;
    }

    // Layer 11.
    let receivers = flags & (flag::SHADOW | flag::SPECULAR) != 0
        || cx.read(|m| m.u8(p.data))? & DEFINITION_RECEIVERS != 0;
    if receivers {
        layer(cx, p, &RECEIVERS, &mut out).await?;
        flags = cx.read(|m| m.u32(lp::global::PASS_FLAGS))?;
    }

    // The additional casters.
    let additional = cx.read(|m| -> Result<bool, MemFault> {
        let over = m.u32(OVERRIDE)?;
        Ok(over != 0
            && m.u8(over.wrapping_add(5))? != 0
            && flags & flag::SHADOW != 0
            && m.i32(ADDITIONAL_COUNT)? > 0)
    })?;
    if additional {
        out.additional_casters = true;
        let request = GuestCall::new(call::ADDITIONAL_CASTERS)
            .esi(p.data)
            .edi(LIGHT_LIST);
        guest(cx, p, site::ADDITIONAL_CASTERS, request).await?;
    }

    // Layers 14 and 12, then 13 with flag bits 0 and 1.
    layer(cx, p, &TARGET_1, &mut out).await?;
    layer(cx, p, &DIFFUSE, &mut out).await?;
    let both = flag::CONTEXT_ZERO | flag::SPECULAR;
    if cx.read(|m| m.u32(lp::global::PASS_FLAGS))? & both == both {
        layer(cx, p, &SPECULAR, &mut out).await?;
    }

    // 6: the tail.
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(LIGHT_ACTIVE, 0)?;
        m.set_u32(STENCIL_STATE, 0)
    })?;
    let tail = |target| GuestCall::new(target).esi(p.data).edi(LIGHT_LIST).ebx(0);
    guest(
        cx,
        p,
        site::STENCIL_ENABLE,
        tail(call::STENCIL_ENABLE).args([0]),
    )
    .await?;
    guest(cx, p, site::SCISSORS, tail(call::SCISSORS).args([0, 0, 0])).await?;
    guest(
        cx,
        p,
        site::DEPTH_RANGE,
        tail(call::DEPTH_RANGE).args([0, 0]),
    )
    .await?;
    let view = cx.read(|m| m.u16(VIEW_TARGET))?;
    let request = tail(call::TARGET)
        .eax((u32::from(view) & 0xff00) | 1)
        .args([u32::from(view), 0]);
    let last = guest(cx, p, site::VIEW_TARGET, request).await?;
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(lp::global::PASSES, 0)?;
        m.set_u32(lp::global::PASS_FLAGS, 0)
    })?;
    Ok((Ret::new(last.eax, 0x10), out))
}

/// One of layers 11 to 14: the draw byte from the light's list's mask
/// (written into the shadow argument's low byte), the layer words and the
/// target when it draws, the stencil off, and the draw and end when the
/// byte (read back) is set.
async fn layer(cx: &Cx, p: &Plan, l: &Layer, out: &mut LightPasses) -> Result<(), EntryError> {
    out.reached |= 1 << l.layer;
    let draws = cx.write(|m| -> Result<u8, MemFault> {
        let b = ((m.u32(LIGHT_MASK)? >> l.layer) & 1) as u8;
        m.set_u8(p.shadow_at(), b)?;
        Ok(b)
    })?;
    let regs = |target| GuestCall::new(target).esi(p.data).edi(LIGHT_LIST);
    if draws != 0 {
        // Layer 14 draws into target 1; the others into the view's.
        let (target, eax) = if l.layer == 14 {
            (1, 1)
        } else {
            let t = cx.read(|m| m.u32(VIEW_TARGET))?;
            (t, (t & !0xff) | 1)
        };
        cx.write(|m| -> Result<(), MemFault> {
            m.set_u32(CURRENT_TARGET, target)?;
            m.set_u32(LAYER_WORD, l.layer)?;
            m.set_u32(LAYER_WORD_B, 1)?;
            m.set_u32(LAYER_LIST, LIGHT_LIST)
        })?;
        guest(
            cx,
            p,
            l.target,
            regs(call::TARGET).eax(eax).args([target, 0]),
        )
        .await?;
        guest(cx, p, l.clip, regs(call::CLIP_MODE).eax(0)).await?;
        if let Some(blend) = l.blend {
            // 0x00031323: the byte is set again before the blend.
            cx.write(|m| m.set_u8(p.shadow_at(), 1))?;
            guest(cx, p, blend, regs(call::RECEIVER_BLEND)).await?;
        }
    } else {
        cx.write(|m| m.set_u32(LAYER_WORD, 0))?;
    }
    guest(cx, p, l.stencil, regs(call::STENCIL).ebx(0)).await?;
    if cx.read(|m| m.u8(p.shadow_at()))? != 0 {
        out.drawn |= 1 << l.layer;
        guest(cx, p, l.draw, regs(call::LAYER_DRAW).ebx(0)).await?;
        guest(cx, p, l.end, regs(call::LAYER_END).ebx(0)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_layer_block_runs_in_the_guests_order() {
        for l in [RECEIVERS, TARGET_1, DIFFUSE, SPECULAR] {
            let mut sites = vec![l.target, l.clip];
            sites.extend(l.blend);
            sites.extend([l.stencil, l.draw, l.end]);
            assert!(sites.windows(2).all(|w| w[0] < w[1]), "layer {}", l.layer);
        }
        assert!(RECEIVERS.end < site::ADDITIONAL_CASTERS);
        assert!(site::ADDITIONAL_CASTERS < TARGET_1.target);
        assert!(TARGET_1.end < DIFFUSE.target && DIFFUSE.end < SPECULAR.target);
        assert!(SPECULAR.end < site::STENCIL_ENABLE);
    }

    #[test]
    fn the_bodys_own_constants_call_names_its_invocation() {
        use guest_layout::testing::FakeGuest;
        let entry_esp = 0x0002_0000;
        let plan = Plan {
            entry_esp,
            fpu: Fpu {
                mxcsr: 0x1f80,
                x87_control: 0x027f,
            },
            definition: 0xe100_0003,
            context: 0x0003_0000,
            data: 0x0004_0000,
            light: 0xe200_0005,
            position: 0x0005_0000,
        };
        let mut m = FakeGuest::new();
        m.put_u32(CONSTANTS_WORD, 0x4480_0000)
            .put_u32(entry_esp + 0x0c, 0x0000_0101)
            .put_u32(entry_esp + 0x10, 0x3f80_0000);
        let esp = entry_esp - FRAME - CONSTANTS_ARGS - 4;
        m.put_u32(esp, site::CONSTANTS)
            .put_u32(esp + 4, plan.data)
            .put_u32(esp + 8, 0x4480_0000)
            .put_u32(esp + 0xc, 0x0000_0101)
            .put_u32(esp + 0x10, 0x3f80_0000);
        let call = ConstantsCall::read(&m, esp).unwrap().unwrap();
        assert_eq!(call.entry_esp, entry_esp);
        assert_eq!(call.view_target_esp(), entry_esp - FRAME);
        let regs = [plan.position, plan.context, plan.light];
        assert!(call.matches(&m, &plan, regs).unwrap());
        // EAX is the frame block, not the light.
        assert!(!call.matches(&m, &plan, [plan.light, plan.context, plan.light]).unwrap());
        // Another invocation's frame.
        let other = Plan { entry_esp: entry_esp + 0x40, ..plan.clone() };
        assert!(!call.matches(&m, &other, regs).unwrap());
        // A weight in the entry's argument slot other than the one pushed
        // (the call was read before, so its pushed words are fixed).
        m.put_u32(entry_esp + 0x10, 0);
        assert!(!call.matches(&m, &plan, regs).unwrap());
        // A Rust body's call: its return slot holds a token.
        m.put_u32(esp, 0xdead_0000);
        assert_eq!(ConstantsCall::read(&m, esp).unwrap(), None);
    }

    #[test]
    fn a_return_slot_names_its_site_only_while_its_call_runs() {
        assert_eq!(return_site(0x1000), None);
        CALL.set(Some((0x1000, site::ITEMS)));
        assert_eq!(return_site(0x1000), Some(site::ITEMS));
        assert_eq!(return_site(0x0ff0), None, "a deeper call's slot");
        drop(Restore(None));
        assert_eq!(return_site(0x1000), None);
    }

    #[test]
    fn a_call_named_inside_another_puts_the_outer_naming_back() {
        CALL.set(Some((0x1000, site::ITEMS)));
        let inner = Restore(CALL.replace(Some((0x0f00, site::CONSTANTS))));
        assert_eq!(return_site(0x0f00), Some(site::CONSTANTS));
        assert_eq!(return_site(0x1000), None, "one call named at a time");
        drop(inner);
        assert_eq!(return_site(0x1000), Some(site::ITEMS));
        drop(Restore(None));
    }
}
