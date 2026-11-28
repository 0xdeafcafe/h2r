//! The first-person lights: `0x000404b0` and the record replay of each
//! slot, `0x000402b0` (docs/graphics/native-light-gather.md, "The
//! first-person lights", slice L6).
//!
//! [`plan`] reads and refuses before any write; a refused plan runs the
//! guest function whole. [`run`] then makes the guest's steps in its
//! order (**E** `0x000404b0`..`0x0004085f`), for each slot of the table
//! `0x004c6700` below `[0x00509440]` whose count is above zero:
//!
//! 1. the light volume into the frame (`0x000c17f0`): the Rust leaf
//!    [`super::volume::light_volume`], or the guest function when it
//!    refuses;
//! 2. the slot's lists (`0x000402b0`, EDX the slot, stack the mode), a
//!    guest call, which [`run_records`] makes behind its hook;
//! 3. the constants `0x0004a1e0` with shadow 0 and weight one (the
//!    image's two pushed words), a guest call;
//! 4. the flags: bit 0 for a kind-0 volume, bit 1 for a colour B
//!    component above zero (`comiss`, `ja`: a NaN is not), the passes
//!    byte set;
//! 5. layers 11, 14, 12 and, with flag bits 0 and 1, 13 on the light's
//!    list, each as the passes' layer block: the layer words and target
//!    when the list's mask has the layer (`0x00014bc0`, `0x00015680`, and
//!    for 11 `0x000496f0`), the stencil off (`0x00015370`), and the draw
//!    `0x00044550` and end `0x000445d0` when the draw byte is set;
//! 6. the tail: two zeroes, stencil off (`0x003f5590`), the scissor
//!    cleared (`0x003f8760`), the view's target (`0x00014bc0`, AL 1), and
//!    both flag words zeroed. The body's inline depth range (the device's
//!    `+0x1c10` words, `0x003faae0`, `0x003f6190`) only fed D3D and is
//!    not made.
//!
//! Every callee is a guest call made below the guest's frame ([`FRAME`]),
//! and while it runs its return slot names the guest's return site
//! ([`super::passes::caller`]), so the hooks that tell the first-person
//! layers apart by their caller (`native_lights` layer draws and the
//! walk's layer builds) see the guest's site. The loop's light index and
//! slot pointer are written where the guest keeps them ([`slot`]): the
//! layer-draw hook reads them from the frame.
//!
//! With typed passes (L8), the guest runs `0x000404b0` itself and
//! [`final_slot`] reads its frame at its own constants call. Only the
//! loop's last slot is taken: the bridge then commits the slot and
//! returns from the whole function as its loop exit would, with EAX from
//! [`FinalSlot::ret`] and the caller's saved registers ([`Saved`]). No
//! guest code resumes inside the skipped slot.
//!
//! [`run_records`] is `0x000402b0`: both lists zeroed, the slot's records
//! copied to the record tail at `[0x004c0b7c]` and the count set past
//! them, then each record that passes the mode's filter appended as
//! [`super::items::record_items_with`] does in the light's kind 1, or
//! made by the guest's per-part calls (`0x000449e0`, `0x00045ce0`,
//! `0x000460d0`, `0x00041040`) when the port does not take it. A replayed
//! record reads the pixel and attachment bytes at its new index, which
//! hold what the last gathered light left there.

use std::ops::Range;

pub mod slots;

use super::frame::{By, FIRST_PERSON_COUNT, FIRST_PERSON_SLOT, FIRST_PERSON_TABLE};
use super::guest::{LIGHTS, NONE, TAGS};
use super::items::{self, LIST, LISTS, Made};
use super::passes::{self, Layer};
use super::{Refusal, Result, visible, volume};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use crate::light_constants::{Call, Fpu};
use crate::light_passes::{flag, global as lp};
use guest_layout::memory::{GuestMemory, MemFault};

/// The frame below the entry `ESP`: `0x88` bytes of locals and four
/// saved registers. The calls are made below it.
pub const FRAME: u32 = 0x98;

/// The locals, as offsets below the entry `ESP`.
pub mod slot {
    /// The slot index (`[esp+0x14]` after the saves), zeroed at entry.
    pub const INDEX: u32 = 0x84;
    /// The slot's light datum's address (`[esp+0x18]`), `0x004c6b00` on.
    pub const ENTRY: u32 = 0x80;
    /// The layer's draw byte (`[esp+0x13]`).
    pub const DRAWS: u32 = 0x85;
    /// The posed volume, `0x7c` bytes up to the entry `ESP`.
    pub const VOLUME: u32 = 0x7c;
}

/// The guest functions the body calls.
pub mod call {
    use guest_layout::points;
    pub use crate::render::light_gather::passes::call::{
        CLIP_MODE, CONSTANTS, LAYER_DRAW, LAYER_END, RECEIVER_BLEND, SCISSORS, STENCIL,
        STENCIL_ENABLE, TARGET,
    };
    /// The light volume: EAX the frame's volume, EDX the light datum;
    /// stack (1), `ret 4`.
    pub const LIGHT_VOLUME: u32 = 0x000c_17f0;
    pub const RECORDS: u32 = points::FIRST_PERSON_LIGHT_RECORDS.base;
    /// The original's depth-range push, which the port does not make:
    /// the push buffer's room and the device's pending state.
    pub const PUSH_SPACE: u32 = 0x003f_aae0;
    pub const PUSH_STATE: u32 = 0x003f_6190;
}

/// The guest's return address after each of its calls.
pub mod site {
    pub const LIGHT_VOLUME: u32 = 0x0004_0528;
    pub const RECORDS: u32 = 0x0004_0565;
    pub const CONSTANTS: u32 = 0x0004_0583;
    pub const STENCIL_ENABLE: u32 = 0x0004_07b5;
    pub const SCISSORS: u32 = 0x0004_07bd;
    pub const VIEW_TARGET: u32 = 0x0004_0823;
}

pub const RECEIVERS: Layer = Layer {
    layer: 11,
    target: 0x0004_060c,
    clip: 0x0004_0618,
    blend: Some(0x0004_0622),
    stencil: 0x0004_0635,
    draw: 0x0004_0642,
    end: 0x0004_0647,
};
pub const TARGET_1: Layer = Layer {
    layer: 14,
    target: 0x0004_067f,
    clip: 0x0004_068b,
    blend: None,
    stencil: 0x0004_069e,
    draw: 0x0004_06ab,
    end: 0x0004_06b0,
};
pub const DIFFUSE: Layer = Layer {
    layer: 12,
    target: 0x0004_06ee,
    clip: 0x0004_06fa,
    blend: None,
    stencil: 0x0004_070d,
    draw: 0x0004_0718,
    end: 0x0004_071d,
};
pub const SPECULAR: Layer = Layer {
    layer: 13,
    target: 0x0004_0774,
    clip: 0x0004_0780,
    blend: None,
    stencil: 0x0004_078f,
    draw: 0x0004_079c,
    end: 0x0004_07a1,
};

/// The slots of the table, and a slot's light datum and count.
const SLOTS: i32 = 8;
const SLOT_LIGHT: u32 = 0x400;
const SLOT_COUNT: u32 = 0x404;
const SLOT_RECORDS: i16 = 32;
/// The words the constants call is pushed: the weight one
/// (`0x00040565`) and the scale (`0x0004056c`), read from the image.
const WEIGHT_WORD: u32 = 0x0004_0566;
const SCALE_WORD: u32 = 0x0004_056d;
/// Colour B, which the constants call leaves.
const COLOUR_B: u32 = 0x0048_5950;
/// The light's list and its layer mask.
const LIGHT_LIST: u32 = 0x004c_1a60;
const LIGHT_MASK: u32 = LIGHT_LIST + 8;
/// The layer words: the layer, its second word and its list.
const LAYER_WORD: u32 = 0x0046_7130;
const LAYER_WORD_B: u32 = 0x0046_7134;
const LAYER_LIST: u32 = 0x0046_7138;
/// The current and the view's render target.
const CURRENT_TARGET: u32 = 0x0048_58b4;
const VIEW_TARGET: u32 = 0x0048_58b8;
/// The tail's two zeroes.
const LIGHT_ACTIVE: u32 = 0x0048_58bc;
const STENCIL_STATE: u32 = 0x004b_843c;

fn slot_base(index: u32) -> u32 {
    FIRST_PERSON_TABLE.wrapping_add(index.wrapping_mul(FIRST_PERSON_SLOT))
}

/// What [`run`] starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub entry_esp: u32,
    pub fpu: Fpu,
}

impl Plan {
    fn at(&self, offset: u32) -> u32 {
        self.entry_esp.wrapping_sub(offset)
    }

    /// The layer draw byte in the guest frame, committed with the typed slot.
    pub fn draw_byte(&self) -> u32 {
        self.at(slot::DRAWS)
    }
}

/// A slot's light as the loop reads it (`0x000404eb`..`0x00040517`): no
/// salt is checked.
#[derive(Clone, Copy, Debug)]
struct Light {
    datum: u32,
    record: u32,
    /// `(definition & 0xffff) << 4`, ECX at the volume call.
    entry: u32,
    data: u32,
}

impl Light {
    fn read(m: &dyn GuestMemory, slot: u32) -> Result<Self, MemFault> {
        let datum = m.u32(slot)?;
        let pool = m.u32(m.u32(LIGHTS)?.wrapping_add(0x44))?;
        let record = pool.wrapping_add((datum & 0xffff).wrapping_mul(0x110));
        let entry = (m.u32(record.wrapping_add(4))? & 0xffff) << 4;
        let data = m.u32(m.u32(TAGS)?.wrapping_add(entry).wrapping_add(8))?;
        Ok(Self {
            datum,
            record,
            entry,
            data,
        })
    }
}

/// Reads and refuses before any write: the float state the flags'
/// comparisons and the volume run under, the frame, the table, each
/// slot's light, and the globals the body reads. A count past the
/// table's eight slots is not taken.
pub fn plan(m: &dyn GuestMemory, entry_esp: u32, fpu: Fpu) -> Result<Plan> {
    visible::fp_supported(fpu)?;
    m.u32(entry_esp.wrapping_add(4))?;
    m.bytes(entry_esp.wrapping_sub(FRAME), FRAME as usize)?;
    let n = m.i32(FIRST_PERSON_COUNT)?;
    if n > SLOTS {
        return Err(Refusal::Unsupported);
    }
    for s in 0..n.max(0) as u32 {
        let entry = slot_base(s).wrapping_add(SLOT_LIGHT);
        if m.u16(slot_base(s).wrapping_add(SLOT_COUNT))? as i16 > 0 {
            let l = Light::read(m, entry)?;
            m.bytes(l.record.wrapping_add(0x84), 12)?;
        }
    }
    for global in [
        WEIGHT_WORD,
        SCALE_WORD,
        COLOUR_B,
        COLOUR_B + 4,
        COLOUR_B + 8,
        LIGHT_MASK,
        LAYER_WORD,
        LAYER_WORD_B,
        LAYER_LIST,
        CURRENT_TARGET,
        VIEW_TARGET,
        STENCIL_STATE,
        lp::PASS_FLAGS,
    ] {
        m.u32(global)?;
    }
    m.u8(LIGHT_ACTIVE)?;
    m.u8(lp::PASSES)?;
    Ok(Plan { entry_esp, fpu })
}

/// The light datums the table holds for this view's first-person pass,
/// in slot order (`[0x00509440]` slots of `0x004c6b00 + 0x408·n`), a
/// slot with no records included: the lights retail's pass
/// (`0x000404b0`) replays or considered. A count past the table's eight
/// slots reads eight; a negative one, none.
pub fn recorded(m: &dyn GuestMemory) -> std::result::Result<Vec<u32>, MemFault> {
    let n = m.i32(FIRST_PERSON_COUNT)?.clamp(0, SLOTS) as u32;
    (0..n)
        .map(|s| m.u32(slot_base(s).wrapping_add(SLOT_LIGHT)))
        .collect()
}

/// What one slot's light did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirstPersonLight {
    /// The slot.
    pub index: u32,
    /// The light datum, `+0x400`.
    pub light: u32,
    pub volume: By,
    /// `[0x004ba014]` as the loop set it.
    pub flags: u32,
    /// The layers whose block ran, and those that drew, as masks.
    pub reached: u32,
    pub drawn: u32,
}

/// What one call did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FirstPersonLights {
    /// `[0x00509440]` at entry.
    pub count: i32,
    /// The slots with records, in order.
    pub lights: Vec<FirstPersonLight>,
}

/// The light's registers at its calls: ESI the record (the device in the
/// tail's last three), EDI the datum, EBX the definition's data until
/// layer 11's stencil call and 0 after.
fn regs(target: u32, l: &Light, ebx: u32) -> GuestCall {
    GuestCall::new(target).esi(l.record).edi(l.datum).ebx(ebx)
}

/// Layer results from an admitted first-person graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotPasses {
    pub flags: u32,
    pub reached: u32,
    pub drawn: u32,
}

/// Called after replaying this slot's records and before its first device
/// call. `None` leaves the complete passes and tail to this body. `Some`
/// means the graph, its guest globals and its device tail all committed.
pub type OnPasses<'a> = dyn FnMut(&mut dyn GuestMemory, &Plan, &Call) -> Result<Option<SlotPasses>, EntryError>
    + Send
    + 'a;

/// Makes the call. Returns as `0x000404b0` does (`ret 4`, AL 1 over the
/// slot pointer past the last slot, or over the count when it is not
/// above zero) with what it did.
pub async fn run(cx: &Cx, plan: Plan) -> Result<(Ret, FirstPersonLights), EntryError> {
    run_with(cx, plan, &mut |_, _, _| Ok(None)).await
}

/// Offer each slot's prepared lists and call to the graph in draw order.
pub async fn run_with(
    cx: &Cx,
    plan: Plan,
    on_passes: &mut OnPasses<'_>,
) -> Result<(Ret, FirstPersonLights), EntryError> {
    let p = &plan;
    cx.write(|m| m.set_u32(p.at(slot::INDEX), 0))?;
    let n = cx.read(|m| m.i32(FIRST_PERSON_COUNT))?;
    let mut out = FirstPersonLights {
        count: n,
        lights: Vec::new(),
    };
    if n <= 0 {
        return Ok((Ret::new((n as u32 & !0xff) | 1, 4), out));
    }
    let mut index = 0u32;
    let mut entry = slot_base(0).wrapping_add(SLOT_LIGHT);
    cx.write(|m| m.set_u32(p.at(slot::ENTRY), entry))?;
    loop {
        if cx.read(|m| m.u16(entry.wrapping_add(4)))? as i16 > 0 {
            out.lights
                .push(light(cx, p, index, entry, on_passes).await?);
        }
        index = index.wrapping_add(1);
        entry = entry.wrapping_add(FIRST_PERSON_SLOT);
        cx.write(|m| -> Result<(), MemFault> {
            m.set_u32(p.at(slot::INDEX), index)?;
            m.set_u32(p.at(slot::ENTRY), entry)
        })?;
        if index as i32 >= cx.read(|m| m.i32(FIRST_PERSON_COUNT))? {
            break;
        }
    }
    Ok((Ret::new((entry & !0xff) | 1, 4), out))
}

/// Called under the scene path after a slot's records are replayed, with
/// the call its constants would take: true when the slot went into the
/// scene, so its constants, flags, layers and tail do not run; false
/// leaves the whole slot to this body.
pub type AtSlot<'a> = dyn FnMut(&mut dyn GuestMemory, &Plan, &Call) -> bool + Send + 'a;

/// The flags word of a slot [`run_scene`]'s `at_slot` took; the flag code
/// never sets every bit.
const TAKEN: u32 = u32::MAX;

/// What [`run_scene`] did: the slots that went into the scene.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneSlots {
    pub count: i32,
    /// The light datums of the slots the scene took, in order.
    pub cut: Vec<u32>,
    /// The slots the guest's steps made.
    pub guest: Vec<u32>,
}

/// [`run`] under the scene path (docs/graphics/native-lights.md, "First-
/// person lights from the scene"): each slot's volume and records as the
/// guest makes them, then `at_slot`. A slot it takes stops there; a slot
/// it refuses makes the rest of its steps. At the loop's exit, when any
/// slot was taken, `at_exit` draws them from the scene, and then the tail
/// runs once for the last one taken: the state it restores is the same
/// for every slot, and the draws see it as the guest's layers would.
pub async fn run_scene(
    cx: &Cx,
    plan: Plan,
    at_slot: &mut AtSlot<'_>,
    at_exit: &mut (dyn FnMut(&mut dyn GuestMemory, &[u32]) + Send),
) -> Result<(Ret, SceneSlots), EntryError> {
    let p = &plan;
    cx.write(|m| m.set_u32(p.at(slot::INDEX), 0))?;
    let n = cx.read(|m| m.i32(FIRST_PERSON_COUNT))?;
    let mut out = SceneSlots { count: n, ..SceneSlots::default() };
    if n <= 0 {
        return Ok((Ret::new((n as u32 & !0xff) | 1, 4), out));
    }
    let mut index = 0u32;
    let mut entry = slot_base(0).wrapping_add(SLOT_LIGHT);
    let mut last = None;
    cx.write(|m| m.set_u32(p.at(slot::ENTRY), entry))?;
    loop {
        if cx.read(|m| m.u16(entry.wrapping_add(4)))? as i16 > 0 {
            // A taken slot's result carries [`TAKEN`] as its flags.
            let mut offer = |m: &mut dyn GuestMemory, p: &Plan, c: &Call| -> Result<Option<SlotPasses>, EntryError> {
                Ok(at_slot(m, p, c).then_some(SlotPasses { flags: TAKEN, reached: 0, drawn: 0 }))
            };
            let done = light(cx, p, index, entry, &mut offer).await?;
            if done.flags == TAKEN {
                out.cut.push(done.light);
                last = Some(entry);
            } else {
                out.guest.push(done.light);
            }
        }
        index = index.wrapping_add(1);
        entry = entry.wrapping_add(FIRST_PERSON_SLOT);
        cx.write(|m| -> Result<(), MemFault> {
            m.set_u32(p.at(slot::INDEX), index)?;
            m.set_u32(p.at(slot::ENTRY), entry)
        })?;
        if index as i32 >= cx.read(|m| m.i32(FIRST_PERSON_COUNT))? {
            break;
        }
    }
    if let Some(at) = last {
        cx.write(|m| at_exit(m, &out.cut));
        let l = cx.read(|m| Light::read(m, at))?;
        tail(cx, p, &l).await?;
    }
    Ok((Ret::new((entry & !0xff) | 1, 4), out))
}

/// Steps 1 to 6 for the slot `index`, its light datum at `entry`.
async fn light(
    cx: &Cx,
    p: &Plan,
    index: u32,
    entry: u32,
    on_passes: &mut OnPasses<'_>,
) -> Result<FirstPersonLight, EntryError> {
    let e = p.entry_esp;
    let l = cx.read(|m| Light::read(m, entry))?;
    let volume_at = p.at(slot::VOLUME);

    // 1: the volume.
    let volume_by = match cx.read(|m| volume::light_volume(m, volume_at, l.datum, 1)) {
        Ok(posed) => {
            cx.write(|m| posed.writes.apply(m))?;
            By::Rust
        }
        Err(why) => {
            let request = regs(call::LIGHT_VOLUME, &l, l.data)
                .eax(volume_at)
                .ecx(l.entry)
                .edx(l.datum)
                .args([1]);
            passes::named(cx, e, FRAME, site::LIGHT_VOLUME, request).await?;
            By::Guest(why)
        }
    };

    // 2: the slot's lists.
    let mode = cx.read(|m| m.u32(e.wrapping_add(4)))?;
    let request = regs(call::RECORDS, &l, l.data)
        .ecx(mode)
        .edx(index)
        .args([mode]);
    passes::named(cx, e, FRAME, site::RECORDS, request).await?;

    // 3: the constants, shadow 0 and weight one.
    let (weight, scale) =
        cx.read(|m| -> Result<_, MemFault> { Ok((m.u32(WEIGHT_WORD)?, m.u32(SCALE_WORD)?)) })?;
    let constants = Call {
        frame: l.record.wrapping_add(0x84),
        context: volume_at,
        light: l.datum,
        definition: l.data,
        shadow: 0,
        weight,
    };
    if let Some(accepted) = cx.write(|m| on_passes(m, p, &constants))? {
        return Ok(FirstPersonLight {
            index,
            light: l.datum,
            volume: volume_by,
            flags: accepted.flags,
            reached: accepted.reached,
            drawn: accepted.drawn,
        });
    }
    let request = regs(call::CONSTANTS, &l, l.data)
        .eax(constants.frame)
        .ecx(volume_at)
        .edx(l.datum)
        .args([l.data, scale, 0, weight]);
    passes::named(cx, e, FRAME, site::CONSTANTS, request).await?;

    // 4: the flags.
    let flags = cx.write(|m| -> Result<u32, MemFault> {
        let mut f = u32::from(m.u16(volume_at)? == 0);
        m.set_u8(lp::PASSES, 1)?;
        let mut lit = false;
        for c in 0..3 {
            if m.f32(COLOUR_B + 4 * c)? > 0.0 {
                lit = true;
                break;
            }
        }
        if lit {
            f |= flag::SPECULAR;
        }
        m.set_u32(lp::PASS_FLAGS, f)?;
        Ok(f)
    })?;

    // 5: layers 11, 14 and 12, then 13 with flag bits 0 and 1.
    let mut done = FirstPersonLight {
        index,
        light: l.datum,
        volume: volume_by,
        flags,
        reached: 0,
        drawn: 0,
    };
    layer(cx, p, &RECEIVERS, &l, l.data, &mut done).await?;
    layer(cx, p, &TARGET_1, &l, 0, &mut done).await?;
    layer(cx, p, &DIFFUSE, &l, 0, &mut done).await?;
    let thirteen = cx.read(|m| -> Result<bool, MemFault> {
        Ok(m.u32(lp::PASS_FLAGS)? & flag::CONTEXT_ZERO != 0
            && (m.u32(lp::PASS_FLAGS)? >> 1) & 1 != 0)
    })?;
    if thirteen {
        layer(cx, p, &SPECULAR, &l, 0, &mut done).await?;
    }

    // 6: the tail.
    tail(cx, p, &l).await?;
    Ok(done)
}

/// Step 6, the tail, for the light `l`: two zeroes, the stencil off, the
/// scissor cleared, the view's target, and both flag words zeroed. The
/// body's inline depth range (the device's `+0x1c10` words and their
/// push, `0x003faae0` and `0x003f6190`) only fed D3D: no device exists,
/// so it is not made.
async fn tail(cx: &Cx, p: &Plan, l: &Light) -> Result<(), EntryError> {
    let e = p.entry_esp;
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(LIGHT_ACTIVE, 0)?;
        m.set_u32(STENCIL_STATE, 0)
    })?;
    let request = regs(call::STENCIL_ENABLE, l, 0).args([0]);
    passes::named(cx, e, FRAME, site::STENCIL_ENABLE, request).await?;
    let request = regs(call::SCISSORS, l, 0).args([0, 0, 0]);
    passes::named(cx, e, FRAME, site::SCISSORS, request).await?;
    let view = u32::from(cx.read(|m| m.u16(VIEW_TARGET))?);
    let request = regs(call::TARGET, l, 0).eax(1).args([view, 0]);
    passes::named(cx, e, FRAME, site::VIEW_TARGET, request).await?;
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u8(lp::PASSES, 0)?;
        m.set_u32(lp::PASS_FLAGS, 0)
    })?;
    Ok(())
}

/// One of layers 11 to 14: the draw byte from the light's list's mask,
/// the layer words and the target when it draws, the stencil off, and
/// the draw and end when the byte (read back) is set. `begin_ebx` is EBX
/// at the target, clip and blend calls.
async fn layer(
    cx: &Cx,
    p: &Plan,
    block: &Layer,
    l: &Light,
    begin_ebx: u32,
    out: &mut FirstPersonLight,
) -> Result<(), EntryError> {
    let e = p.entry_esp;
    let draws_at = p.draw_byte();
    out.reached |= 1 << block.layer;
    let draws = cx.write(|m| -> Result<u8, MemFault> {
        let b = ((m.u32(LIGHT_MASK)? >> block.layer) & 1) as u8;
        m.set_u8(draws_at, b)?;
        Ok(b)
    })?;
    if draws != 0 {
        // Layer 14 draws into target 1; the others into the view's.
        let (target, eax) = if block.layer == 14 {
            (1, 1)
        } else {
            let t = cx.read(|m| m.u32(VIEW_TARGET))?;
            (t, (t & !0xff) | 1)
        };
        cx.write(|m| -> Result<(), MemFault> {
            m.set_u32(CURRENT_TARGET, target)?;
            m.set_u32(LAYER_WORD, block.layer)?;
            m.set_u32(LAYER_WORD_B, 1)?;
            m.set_u32(LAYER_LIST, LIGHT_LIST)
        })?;
        let request = regs(call::TARGET, l, begin_ebx).eax(eax).args([target, 0]);
        passes::named(cx, e, FRAME, block.target, request).await?;
        let request = regs(call::CLIP_MODE, l, begin_ebx).eax(0);
        passes::named(cx, e, FRAME, block.clip, request).await?;
        if let Some(blend) = block.blend {
            // 0x00040618: the byte is set again before the blend.
            cx.write(|m| m.set_u8(draws_at, 1))?;
            let request = regs(call::RECEIVER_BLEND, l, begin_ebx);
            passes::named(cx, e, FRAME, blend, request).await?;
        }
    } else {
        cx.write(|m| m.set_u32(LAYER_WORD, 0))?;
    }
    let request = regs(call::STENCIL, l, 0);
    passes::named(cx, e, FRAME, block.stencil, request).await?;
    if cx.read(|m| m.u8(draws_at))? != 0 {
        out.drawn |= 1 << block.layer;
        let request = regs(call::LAYER_DRAW, l, 0);
        passes::named(cx, e, FRAME, block.draw, request).await?;
        let request = regs(call::LAYER_END, l, 0);
        passes::named(cx, e, FRAME, block.end, request).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------
// L8: the final slot typed inside the guest's own body.

/// The constants' four argument words, pushed at `0x00040565`..`0x00040571`.
pub const CONSTANTS_ARGS: u32 = 0x10;

/// The entry `ESP` of the invocation whose call to `0x0004a1e0` returns
/// to [`site::CONSTANTS`], from `ESP` at the callee's entry: the return
/// word, the four arguments and the frame.
pub fn entry_esp_at_constants(callee_esp: u32) -> u32 {
    callee_esp
        .wrapping_add(4)
        .wrapping_add(CONSTANTS_ARGS)
        .wrapping_add(FRAME)
}

/// The registers at the callee's entry when the guest's own body makes
/// its constants call (`0x0004057e`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtConstants {
    pub esp: u32,
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    pub ebx: u32,
    pub esi: u32,
    pub edi: u32,
}

/// The guest's last slot at its constants call, and how the whole
/// function returns when that slot's passes and tail are committed
/// elsewhere.
///
/// The loop's exit after the view target stores only both pass words
/// (zeroed) and its two locals (the index and the slot pointer). The
/// typed tail's journal already zeroes both pass words
/// (`typed_tail::plan_layers`). The locals lie below [`Self::exit_esp`]
/// once `ret 4` has run, so nothing reads them and the return leaves
/// them as they are. No write follows the commit.
#[derive(Clone, Debug)]
pub struct FinalSlot {
    /// The constants call as the L6 body makes it.
    pub call: Call,
    /// `ret 4` with AL 1 over the slot pointer past the last slot.
    pub ret: Ret,
    /// `ESP` after that `ret 4`: past the return word and the mode.
    pub exit_esp: u32,
}

/// Reads the guest's frame at its own constants call and accepts only
/// the loop's last slot. ESI must be the light's record, EBX its
/// definition's data and EDI its datum, as [`regs`] makes them; EAX, ECX,
/// EDX and the four pushed words must be the call the L6 body makes.
/// Earlier slots are refused: resuming the guest's loop after a skipped
/// slot would need registers that this hook does not reconstruct.
pub fn final_slot(m: &dyn GuestMemory, plan: &Plan, r: &AtConstants) -> Result<FinalSlot> {
    if m.u32(r.esp)? != site::CONSTANTS || entry_esp_at_constants(r.esp) != plan.entry_esp {
        return Err(Refusal::Layout);
    }
    let index = m.u32(plan.at(slot::INDEX))?;
    let entry = m.u32(plan.at(slot::ENTRY))?;
    if index >= SLOTS as u32 || entry != slot_base(index).wrapping_add(SLOT_LIGHT) {
        return Err(Refusal::Layout);
    }
    let l = Light::read(m, entry)?;
    let volume_at = plan.at(slot::VOLUME);
    let args = r.esp.wrapping_add(4);
    let (weight, scale) = (m.u32(WEIGHT_WORD)?, m.u32(SCALE_WORD)?);
    if r.esi != l.record
        || r.ebx != l.data
        || r.edi != l.datum
        || r.eax != l.record.wrapping_add(0x84)
        || r.ecx != volume_at
        || r.edx != l.datum
        || m.u32(args)? != l.data
        || m.u32(args.wrapping_add(4))? != scale
        || m.u32(args.wrapping_add(8))? != 0
        || m.u32(args.wrapping_add(12))? != weight
    {
        return Err(Refusal::Layout);
    }
    // 0x00040823 onward: the loop re-reads the count after this slot.
    // A typed slot makes no guest call, so the count it reads is this one.
    let next = index.wrapping_add(1);
    if (next as i32) < m.i32(FIRST_PERSON_COUNT)? {
        return Err(Refusal::Unsupported);
    }
    let past = entry.wrapping_add(FIRST_PERSON_SLOT);
    Ok(FinalSlot {
        call: Call {
            frame: r.eax,
            context: volume_at,
            light: l.datum,
            definition: l.data,
            shadow: 0,
            weight,
        },
        ret: Ret::new((past & !0xff) | 1, 4),
        exit_esp: plan.entry_esp.wrapping_add(8),
    })
}

// ---------------------------------------------------------------------
// 0x000402b0: one slot's lists.

/// `0x000402b0`'s frame: `0x48` bytes of locals and four saved registers.
pub const RECORDS_FRAME: u32 = 0x58;

/// Its locals, as offsets below the entry `ESP`.
pub mod records_slot {
    /// The record's pixel byte (`[esp+0x1c]` in the loop). The dword
    /// there is pushed to `0x00041040`, its upper bytes what the stack
    /// held.
    pub const PIXEL: u32 = 0x3c;
    /// The part scratch `0x00045ce0` fills and `0x00041040` reads.
    pub const SCRATCH: u32 = 0x2c;
}

/// What [`run_records`] starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordsPlan {
    pub entry_esp: u32,
    /// EAX at entry: its upper half is the return of a slot with no
    /// records.
    pub entry_eax: u32,
    /// EDX: the slot.
    pub slot: u32,
    /// Stack 0: the mode, which picks the record filter.
    pub mode: u32,
    /// The flag bits a record must have one of (`0x0004033a`).
    pub filter: u32,
}

impl RecordsPlan {
    fn base(&self) -> u32 {
        slot_base(self.slot)
    }

    fn count_at(&self) -> u32 {
        self.base().wrapping_add(SLOT_COUNT)
    }
}

/// Reads and refuses before any write: the slot (one of the eight), its
/// count (at most 32), the record tail it is copied to (below `0x8000`
/// and clear of the table), the lists and the frame.
pub fn plan_records(
    m: &dyn GuestMemory,
    entry_esp: u32,
    entry_eax: u32,
    slot: u32,
    fpu: Fpu,
) -> Result<RecordsPlan> {
    visible::fp_supported(fpu)?;
    if slot >= SLOTS as u32 {
        return Err(Refusal::Unsupported);
    }
    let mode = m.u32(entry_esp.wrapping_add(4))?;
    m.bytes(
        entry_esp.wrapping_sub(RECORDS_FRAME),
        RECORDS_FRAME as usize + 8,
    )?;
    m.bytes(LISTS + LIST, 2 * LIST as usize)?;
    let base = slot_base(slot);
    let count = m.u16(base.wrapping_add(SLOT_COUNT))? as i16;
    if count > SLOT_RECORDS {
        return Err(Refusal::Unsupported);
    }
    if count > 0 {
        let start = m.u32(items::MAIN_COUNT)?;
        let n = count as u32;
        if start >= 0x8000 || start + n > 0x8000 {
            return Err(Refusal::Unsupported);
        }
        let to = items::record_at(start);
        let bytes = n * items::RECORD;
        if to < base.wrapping_add(bytes) && base < to.wrapping_add(bytes) {
            return Err(Refusal::Layout);
        }
        m.bytes(base, bytes as usize)?;
        m.bytes(to, bytes as usize)?;
        m.u32(items::COUNT)?;
    }
    let filter = match mode {
        0 => u32::MAX,
        2 => 0x10,
        _ => 0x20,
    };
    Ok(RecordsPlan {
        entry_esp,
        entry_eax,
        slot,
        mode,
        filter,
    })
}

/// What one call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRecords {
    pub slot: u32,
    /// The records' new indices, from `[0x004c0b7c]`.
    pub records: Range<u32>,
    /// Per record that passed the filter, by its new index, in order.
    pub made: Vec<(u32, Made)>,
}

/// The light's kind the replay hands `0x00041040`.
const KIND: u32 = 1;

/// Makes the call. Returns as `0x000402b0` does (`ret 4`, EAX the number
/// of records walked) with what it did.
pub async fn run_records(cx: &Cx, plan: RecordsPlan) -> Result<(Ret, SlotRecords), EntryError> {
    let p = &plan;
    // 0x000402ca: both lists' count, first and layers.
    cx.write(|m| -> Result<(), MemFault> {
        for header in [LISTS + LIST, LISTS + 2 * LIST] {
            for offset in [4, 0xc, 8] {
                m.set_u32(header + offset, 0)?;
            }
        }
        Ok(())
    })?;
    let count = cx.read(|m| m.u16(p.count_at()))? as i16;
    let mut out = SlotRecords {
        slot: p.slot,
        records: 0..0,
        made: Vec::new(),
    };
    if count <= 0 {
        let eax = (p.entry_eax & 0xffff_0000) | u32::from(count as u16);
        return Ok((Ret::new(eax, 4), out));
    }
    // 0x000402fe: the copy after the main view's records.
    let n = count as u32;
    let start = cx.read(|m| m.u32(items::MAIN_COUNT))?;
    if start >= 0x8000 || start + n > 0x8000 {
        return Err(EntryError::IsolatedCall(format!(
            "first-person records start at {start:#x}"
        )));
    }
    cx.write(|m| -> Result<(), MemFault> {
        m.set_u32(items::COUNT, start)?;
        let bytes = m.bytes(p.base(), (n * items::RECORD) as usize)?;
        m.write(items::record_at(start), &bytes)?;
        m.set_u32(items::COUNT, start.wrapping_add(n))
    })?;
    out.records = start..start + n;
    let ret_slot = p.entry_esp.wrapping_add(4);
    let mut record = p.base();
    let mut index = start;
    let mut walked = 0u32;
    cx.write(|m| m.set_u32(ret_slot, record))?;
    loop {
        if cx.read(|m| items::selected(m, record, p.filter))? {
            match cx.read(|m| items::record_items_with(m, index, KIND, true)) {
                Ok(writes) => {
                    cx.write(|m| writes.apply(m))?;
                    out.made.push((index, Made::Rust));
                }
                Err(why) => {
                    guest_record(cx, p, record, index).await?;
                    out.made.push((index, Made::Guest(why)));
                }
            }
        }
        walked = walked.wrapping_add(1);
        record = record.wrapping_add(items::RECORD);
        index = index.wrapping_add(1);
        cx.write(|m| m.set_u32(ret_slot, record))?;
        if walked as i32 >= i32::from(cx.read(|m| m.u16(p.count_at()))? as i16) {
            break;
        }
    }
    Ok((Ret::new(walked, 4), out))
}

/// A guest call below the record loop's frame.
async fn below(cx: &Cx, call: GuestCall) -> Result<u32, EntryError> {
    Ok(cx.call(call.reserve(RECORDS_FRAME)).await?.eax)
}

/// The slot's record at `record`, replayed at `index`, by the guest's
/// per-part calls (`0x000403c1`..`0x0004047c`): its geometry, then per
/// part its material, its subparts and its items, with the pixel byte
/// at the new index, the override shader of the attachment byte there,
/// the light's kind 1 and the fifth argument 1.
async fn guest_record(cx: &Cx, p: &RecordsPlan, record: u32, index: u32) -> Result<(), EntryError> {
    let e = p.entry_esp;
    let pixel_at = e.wrapping_sub(records_slot::PIXEL);
    let scratch = e.wrapping_sub(records_slot::SCRATCH);
    let (flags, class, shader_override, edx) = cx.write(|m| -> Result<_, MemFault> {
        let flags = m.u32(record)?;
        let class = m.u32(record.wrapping_add(0xc))? & 0xf;
        let pixel = m.u8(items::PIXEL_SIZES.wrapping_add(index))?;
        // movsx si: the plan keeps the index under 0x8000.
        let attachment = m.u8(items::ATTACHMENT_BYTES.wrapping_add(index))?;
        m.set_u8(pixel_at, pixel)?;
        let shader_override = match attachment {
            0xff => NONE,
            a => m.u32(items::OVERRIDES + 4 * u32::from(a))?,
        };
        // 0x000403ff: EDX is 0x1f0 from the selector test with the pixel
        // byte in its low byte, or the attachment byte when it names an
        // override (0x000403e7); its low byte is then cleared.
        let edx = if attachment == 0xff { 0x100 } else { 0 };
        Ok((flags, class, shader_override, edx))
    })?;
    let placement = (flags >> 12) & 0xffff_ff01;
    let request = GuestCall::new(items::call::RECORD_GEOMETRY)
        .eax(index)
        .ecx(placement)
        .edx(edx)
        .ebx(record)
        .esi(index)
        .edi(p.filter)
        .args([placement]);
    let geometry = below(cx, request).await?;
    if geometry == 0 {
        return Ok(());
    }
    let pixel_word = cx.read(|m| m.u32(pixel_at))?;
    let mut j = 0u32;
    while (j as i32) < cx.read(|m| m.i32(geometry))? {
        let request = GuestCall::new(items::call::PART_MATERIAL)
            .eax(scratch)
            .ecx(index)
            .ebx(record)
            .esi(index)
            .edi(j)
            .args([j, class]);
        if below(cx, request).await? as u8 != 0 {
            let mask = cx.read(|m| m.u32(record.wrapping_add(0x14)))?;
            let request = GuestCall::new(items::call::PART_VISIBLE)
                .eax(j)
                .ebx(mask)
                .esi(index)
                .edi(j)
                .args([index]);
            if below(cx, request).await? as u8 != 0 {
                let shader = match shader_override {
                    NONE => cx.read(|m| m.u32(scratch))?,
                    s => s,
                };
                if shader != NONE {
                    let request = GuestCall::new(items::call::PART_ITEMS)
                        .eax(shader)
                        .ecx(j)
                        .edx(index)
                        .ebx(mask)
                        .esi(index)
                        .edi(j)
                        .args([KIND, pixel_word, scratch, shader, 1, u32::MAX, 0]);
                    below(cx, request).await?;
                }
            }
        }
        j += 1;
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
        assert!(site::CONSTANTS < RECEIVERS.target);
        assert!(RECEIVERS.end < TARGET_1.target && TARGET_1.end < DIFFUSE.target);
        assert!(DIFFUSE.end < SPECULAR.target && SPECULAR.end < site::STENCIL_ENABLE);
        assert!(site::SCISSORS < site::VIEW_TARGET);
    }

    #[test]
    fn the_recorded_lights_are_the_tables_slots_up_to_its_count() {
        let mut m = guest_layout::memory::FlatMemory::new(FIRST_PERSON_TABLE, 0x0005_0000);
        for s in 0..SLOTS as u32 {
            m.set_u32(slot_base(s) + SLOT_LIGHT, 0x00a0_0000 | s)
                .unwrap();
        }
        m.set_i32(FIRST_PERSON_COUNT, 3).unwrap();
        assert_eq!(
            recorded(&m).unwrap(),
            [0x00a0_0000, 0x00a0_0001, 0x00a0_0002]
        );
        m.set_i32(FIRST_PERSON_COUNT, 12).unwrap();
        assert_eq!(recorded(&m).unwrap().len(), SLOTS as usize, "eight at most");
        m.set_i32(FIRST_PERSON_COUNT, -1).unwrap();
        assert!(recorded(&m).unwrap().is_empty());
    }

    #[test]
    fn the_frame_holds_the_volume_and_the_loops_locals() {
        assert_eq!(slot::VOLUME as usize, volume::BYTES);
        assert!(slot::INDEX < FRAME - 0x10 && slot::DRAWS > slot::INDEX);
        assert_eq!(slot_base(0) + SLOT_LIGHT, 0x004c_6b00);
        assert!(records_slot::PIXEL < RECORDS_FRAME - 0x10);
    }
}
