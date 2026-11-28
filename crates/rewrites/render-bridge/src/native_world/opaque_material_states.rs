//! The ordinary material's dynamic render and sampler lists (0x1a170 and
//! 0x1ab50). The deferred cases are guest stores, consumed by the next D3D
//! flush. The immediate cases must also update D3D's pushed state.

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::states::{self, ALPHA_SCALE, DEFS, DIRTY, HEADERS, MASKS, SHADOW, SIMPLE, STATE_88};

const SAMPLER_DEFS: u32 = 0x0046_703c;
const SAMPLER_SHADOW: u32 = 0x0040_4dd0;
const SAMPLER_MASKS: u32 = 0x0043_e5e8;
const SET_TEXCOORD_INDEX: u32 = 0x003f_5d20;
const SET_BUMP_ENV: u32 = 0x003f_5e30;
const SET_BORDER_COLOR: u32 = 0x003f_5ea0;
const SET_COLOR_KEY: u32 = 0x003f_5ef0;

pub(super) fn alpha_byte(value: u32, scale: u32, fcw: u16) -> u32 {
    states::alpha_value(f32::from_bits(value), f32::from_bits(scale), fcw)
}

fn mark(m: &mut dyn GuestMemory, bits: u32) -> Result<(), MemFault> {
    m.set_u32(DIRTY, m.u32(DIRTY)? | bits)
}

pub(super) async fn render_state(cx: &Cx, slot: u32, value: u32) -> Result<(), EntryError> {
    match slot {
        0..0x5c => {
            let header = cx.read(|m| m.u32(HEADERS + 4 * slot))?;
            // The pushed register can differ from the shadow (in particular,
            // several slots share a method). Preserve the real setter.
            cx.call(GuestCall::new(SIMPLE).ecx(header).edx(value)).await?;
            cx.write(|m| m.set_u32(SHADOW + 4 * slot, value))?;
        }
        0x5c..0x88 => cx.write(|m| {
            let bits = m.u32(MASKS + 4 * slot)?;
            mark(m, bits)?;
            m.set_u32(SHADOW + 4 * slot, value)
        })?,
        // The body's device copy (`[0x00407488] + 0x790`) only fed D3D,
        // and no device exists.
        0x88 => cx.write(|m| {
            mark(m, 0x4000)?;
            m.set_u32(STATE_88, value)
        })?,
        _ => {
            if let Some(setter) = states::setter(slot) {
                cx.call(GuestCall::new(setter).args([value])).await?;
            }
        }
    }
    Ok(())
}

fn stage_shadow(stage: u32, slot: u32) -> u32 {
    SAMPLER_SHADOW.wrapping_add(stage.wrapping_mul(0x80)).wrapping_add(slot.wrapping_mul(4))
}

pub(super) async fn stage_state(cx: &Cx, stage: u32, slot: u32, value: u32) -> Result<(), EntryError> {
    let call = match slot {
        0..0x16 => {
            cx.write(|m| {
                let bits = if slot < 0xc { 1u32.wrapping_shl(stage) } else { m.u32(SAMPLER_MASKS + 4 * slot)? };
                mark(m, bits)?;
                m.set_u32(stage_shadow(stage, slot), value)
            })?;
            return Ok(());
        }
        0x16..=0x1b => GuestCall::new(SET_BUMP_ENV).eax(stage).args([slot, value]),
        0x1c => GuestCall::new(SET_TEXCOORD_INDEX).esi(stage).args([value]),
        0x1d => GuestCall::new(SET_BORDER_COLOR).eax(stage).ebx(value),
        0x1e => GuestCall::new(SET_COLOR_KEY).eax(stage).ebx(value),
        _ => return Ok(()),
    };
    cx.call(call).await?;
    Ok(())
}

#[derive(Clone, Copy)]
enum List {
    Render,
    Sampler,
}

struct Write {
    stage: u32,
    slot: u32,
    value: u32,
}

impl List {
    fn evaluate(self, m: &dyn GuestMemory, context: u32, at: u32, fcw: u16)
        -> Result<Option<Write>, EntryError>
    {
        let [first, second, third, parameter] = m.u32(at)?.to_le_bytes();
        let (stage, def, source, table) = match self {
            Self::Render => (0, first, second, DEFS),
            Self::Sampler => (u32::from(first), second, third, SAMPLER_DEFS),
        };
        let def = table + 4 * u32::from(def);
        let slot = u32::from(m.u16(def)?);
        let old = || match self {
            Self::Render => m.u32(SHADOW + 4 * slot),
            Self::Sampler => m.u32(stage_shadow(stage, slot)),
        };
        let n = u32::from(parameter);
        let value = if source == 1 {
            let bits = m.u32(context.wrapping_add(0x5c + 4 * n))?;
            match m.u8(def + 3)? {
                0 => (old()? & 0x00ff_ffff) | alpha_byte(bits, m.u32(ALPHA_SCALE)?, fcw),
                1 => bits,
                2 => states::integer_value(f32::from_bits(bits), fcw),
                _ => return Ok(None),
            }
        } else {
            let p = context.wrapping_add(12 * (n + 0x16));
            let rgb = [m.u32(p)?, m.u32(p.wrapping_add(4))?, m.u32(p.wrapping_add(8))?];
            let packed = engine::glow::pack_rgb(rgb, m.u32(ALPHA_SCALE)?, fcw)
                .map_err(|e| EntryError::IsolatedCall(format!("state colour conversion: {e:?}")))?;
            (old()? & 0xff00_0000) | packed
        };
        Ok(Some(Write { stage, slot, value }))
    }

    async fn apply(self, cx: &Cx, context: u32, span: u32, fcw: u16) -> Result<(), EntryError> {
        let packed = u32::from(cx.read(|m| m.u16(span))?);
        if packed >> 9 == 0 {
            return Ok(());
        }
        let first = cx.read(|m| {
            let definition = m.u32(context.wrapping_add(0x20))?;
            m.u32(definition.wrapping_add(0x3c))
        })?.wrapping_add(4 * (packed & 0x1ff));
        let mut index = 0;
        loop {
            if let Some(w) = cx.read(|m| self.evaluate(m, context, first.wrapping_add(4 * index), fcw))? {
                match self {
                    Self::Render => render_state(cx, w.slot, w.value).await?,
                    Self::Sampler => stage_state(cx, w.stage, w.slot, w.value).await?,
                }
            }
            // A setter can drain an earlier guest callback. The loop keeps
            // its initial base but reads the span length again after it.
            index += 1;
            if index >= u32::from(cx.read(|m| m.u16(span))?) >> 9 {
                return Ok(());
            }
        }
    }
}

pub(super) async fn render(cx: &Cx, context: u32, span: u32, fcw: u16) -> Result<(), EntryError> {
    List::Render.apply(cx, context, span, fcw).await
}

pub(super) async fn sampler(cx: &Cx, context: u32, span: u32, fcw: u16) -> Result<(), EntryError> {
    List::Sampler.apply(cx, context, span, fcw).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::entry::{EntryFuture, Regs, Ret, testing::run};
    use engine::testing::FakeGuest;

    const CONTEXT: u32 = 0x2000;
    const DEFINITION: u32 = 0x3000;
    const ENTRIES: u32 = 0x8000;
    const SPAN: u32 = 0x6012;

    fn scene(first: u16, count: u16) -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(CONTEXT + 0x20, DEFINITION)
            .put_u32(DEFINITION + 0x3c, ENTRIES)
            .put_f32(ALPHA_SCALE, 255.0)
            .put_u16(SPAN, (count << 9) | first)
            .put_u32(DIRTY, 0x8000_0000);
        m
    }

    #[test]
    fn dynamic_sampler_preserves_the_other_colour_lanes_and_stage_bits() {
        let mut m = scene(5, 5);
        let at = ENTRIES + 5 * 4;
        // Stage two, sampler state 3. Update alpha and RGB separately.
        m.put_u16(SAMPLER_DEFS, 3)
            .put_u8(SAMPLER_DEFS + 3, 0)
            .put(at, &[2, 0, 1, 4])
            .put_f32(CONTEXT + 0x5c + 16, 0.5)
            .put(at + 4, &[2, 0, 2, 1])
            .put_f32(CONTEXT + 12 * 23, 1.0)
            .put_f32(CONTEXT + 12 * 23 + 4, 0.5)
            .put_f32(CONTEXT + 12 * 23 + 8, 0.0)
            .put_u32(stage_shadow(2, 3), 0x4455_6677)
            .put_u32(stage_shadow(1, 3), 0x1234_5678);
        // 12..21 use the table's dirty mask, independently of the stage.
        m.put_u16(SAMPLER_DEFS + 4, 19)
            .put_u8(SAMPLER_DEFS + 7, 1)
            .put(at + 8, &[0, 1, 1, 5])
            .put_u32(CONTEXT + 0x5c + 20, 0x7fc1_2345)
            .put_u32(SAMPLER_MASKS + 4 * 19, 0x0010_0000)
            .map(stage_shadow(0, 19), 4);
        // Bump-env and border colour reach two distinct D3D ABIs.
        m.put_u16(SAMPLER_DEFS + 8, 0x17)
            .put_u8(SAMPLER_DEFS + 11, 2)
            .put(at + 12, &[1, 2, 1, 4])
            .put_u16(SAMPLER_DEFS + 12, 0x1d)
            .put_u8(SAMPLER_DEFS + 15, 1)
            .put(at + 16, &[3, 3, 1, 5]);

        let body = |cx: Cx| -> EntryFuture {
            Box::pin(async move {
                sampler(&cx, CONTEXT, SPAN, 0x037f).await?;
                Ok(Ret::new(0, 0))
            })
        };
        let result = run(&body, m, Regs::default(), |_, mem| {
            assert_eq!(mem.u32(stage_shadow(2, 3)), Ok(0x80ff_8000));
            assert_eq!(mem.u32(DIRTY), Ok(0x8010_0004));
            Regs::default()
        });
        assert_eq!(result.result, Ok(Ret::new(0, 0)));
        assert_eq!(result.guest.u32(stage_shadow(0, 19)), Ok(0x7fc1_2345));
        assert_eq!(result.guest.u32(stage_shadow(1, 3)), Ok(0x1234_5678));
        assert_eq!(result.calls, [
            GuestCall::new(SET_BUMP_ENV).eax(1).args([0x17, 0]),
            GuestCall::new(SET_BORDER_COLOR).eax(3).ebx(0x7fc1_2345),
        ]);
    }

    #[test]
    fn render_state_writes_the_mirror_and_dirty_bit_before_a_later_setter() {
        let mut m = scene(0, 3);
        m.map(STATE_88, 4)
            .put_u16(DEFS, 0x88)
            .put_u8(DEFS + 3, 2)
            .put(ENTRIES, &[0, 1, 0, 0])
            .put_f32(CONTEXT + 0x5c, 2.8)
            .put_u16(DEFS + 4, 4)
            .put_u8(DEFS + 7, 1)
            .put_u32(HEADERS + 16, 0x40038)
            .put_u32(SHADOW + 16, 99)
            .put(ENTRIES + 4, &[1, 1, 0, 1])
            .put_u32(CONTEXT + 0x60, 96)
            .put_u16(DEFS + 8, 0x89)
            .put_u8(DEFS + 11, 2)
            .put(ENTRIES + 8, &[2, 1, 0, 0]);
        let body = |cx: Cx| -> EntryFuture {
            Box::pin(async move {
                render(&cx, CONTEXT, SPAN, 0x077f).await?;
                Ok(Ret::new(0, 0))
            })
        };
        let result = run(&body, m, Regs::default(), |call, mem| {
            assert_eq!(mem.u32(STATE_88), Ok(2));
            assert_eq!(mem.u32(DIRTY), Ok(0x8000_4000));
            assert_eq!(mem.u32(SHADOW + 16), Ok(if call.target == SIMPLE { 99 } else { 96 }));
            Regs::default()
        });
        assert_eq!(result.result, Ok(Ret::new(0, 0)));
        assert_eq!(result.calls, [
            GuestCall::new(SIMPLE).ecx(0x40038).edx(96),
            GuestCall::new(states::setter(0x89).unwrap()).args([2]),
        ]);
    }
}
