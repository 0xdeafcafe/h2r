//! The Halo 1 static HUD panels' tint in Halo 2's widget combiner.

use std::cell::RefCell;

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{
    HUD_MATERIAL_CONSTANTS, HUD_MATERIAL_PIXEL_SHADER, HUD_MATERIAL_READY, SHADER_DRAW_BEGIN,
};
use hooks::{Builder, Flow};
use h1_contract::hud::{COMPONENT_START, CONTROL_FLAGS};

use super::{controls::widget_control, flash, mission_hud};

const DRAW_SETUP_RETURN: u32 = 0x0022_a7e2;
const WIDGET_DRAW_RETURN: u32 = 0x0022_b695;
const DRAW_READY_RETURN: u32 = 0x0022_a7f4;
const MATERIAL: u32 = 0x0048_5b48;
const C0: u32 = 0x144c;
const C1: u32 = 0x146c;
const DIRTY_C0: u32 = 0x1518;
const DIRTY_C1: u32 = 0x151c;
// Gradient and solid branches of widget_gradient_flash all use these
// colour endpoints. Stage 4 also multiplies the bitmap's alpha by C0.a.
const STAGES: [u32; 3] = [1, 2, 4];
const DIRTY: u32 = (1 << 1) | (1 << 2) | (1 << 4);

#[derive(Default)]
struct Draw {
    colour: Option<u32>,
    original: Option<[u32; 6]>,
}

thread_local! {
    static DRAW: RefCell<Draw> = RefCell::new(Draw::default());
}

fn colour(mem: &dyn GuestMemory, frame: u32, rows: u32) -> Result<Option<u32>, MemFault> {
    if mem.u32(frame + 4)? != WIDGET_DRAW_RETURN {
        return Ok(None);
    }
    let bitmap_frame = mem.u32(frame)?;
    let widget = mem.u32(bitmap_frame + 0x78)?;
    if let Some(colour) = super::weapon::colour(mem, widget, rows, bitmap_frame)? {
        return Ok(Some(colour));
    }
    let Some(tag) = mission_hud(mem)? else {
        return Ok(None);
    };
    if !super::extended(mem, tag)? {
        return Ok(None);
    }
    let Some(entry) = widget_control(mem, tag, mem.u32(widget)?)? else {
        return Ok(None);
    };
    if mem.u32(entry + 8)? == 0 {
        return Ok(None);
    }
    let role = mem.u32(entry + 4)?;
    let at = entry + 0xc;
    let flags = mem.u32(tag + CONTROL_FLAGS)?;
    if flags & (role << 1) == 0 {
        return mem.u32(at).map(Some);
    }
    let tick = mem.u32(tag + COMPONENT_START + 4 * (role.trailing_zeros() / 2))?;
    let mut record = [0; 24];
    mem.read(at, &mut record)?;
    Ok(Some(pack(flash::flash_colour(
        &record,
        Some(flash::elapsed(mem, tick)?),
    ))))
}

fn pack(argb: [f32; 4]) -> u32 {
    u32::from_be_bytes(argb.map(|v| (v * 255.0).round() as u8))
}

fn constants() -> impl Iterator<Item = u32> {
    [C0, C1]
        .into_iter()
        .flat_map(|base| STAGES.map(move |i| MATERIAL + base + i * 4))
}

fn mark_dirty(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    for at in [MATERIAL + DIRTY_C0, MATERIAL + DIRTY_C1] {
        mem.set_u32(at, mem.u32(at)? | DIRTY)?;
    }
    Ok(())
}

fn apply(mem: &mut dyn GuestMemory, material: u32) -> Result<(), MemFault> {
    if material != MATERIAL {
        return Ok(());
    }
    DRAW.with(|draw| {
        let mut draw = draw.borrow_mut();
        let Some(colour) = draw.colour else {
            return Ok(());
        };
        if draw.original.is_none() {
            let mut original = [0; 6];
            for (at, word) in constants().zip(&mut original) {
                *word = mem.u32(at)?;
            }
            draw.original = Some(original);
        }
        for at in constants() {
            mem.set_u32(at, colour)?;
        }
        mark_dirty(mem)
    })
}

fn restore(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let previous = DRAW.with(|draw| std::mem::take(&mut *draw.borrow_mut()));
    if let Some(words) = previous.original {
        for (at, word) in constants().zip(words) {
            mem.set_u32(at, word)?;
        }
        // A following material that reuses the same state must resend the
        // native endpoints, even if none of its own inputs have changed.
        mark_dirty(mem)?;
    }
    Ok(())
}

pub(super) fn register(b: &mut Builder, rows: u32) {
    b.raw()
        .before(&SHADER_DRAW_BEGIN, "select a converted panel tint", move |c| {
            let sp = c.cpu.esp;
            let frame = c.cpu.ebp;
            let mut mem = c.ctx.memory();
            if mem.u32(sp).ok() == Some(DRAW_SETUP_RETURN) {
                let _ = restore(&mut *mem);
                let colour = colour(&*mem, frame, rows).ok().flatten();
                DRAW.with(|draw| draw.borrow_mut().colour = colour);
            }
            Ok(Flow::Continue)
        })
        .context_only();
    b.raw()
        .before(
            &HUD_MATERIAL_CONSTANTS,
            "tint cached HUD combiner constants",
            |c| {
                let sp = c.cpu.esp;
                let mut mem = c.ctx.memory();
                if let Ok(material) = mem.u32(sp + 4) {
                    let _ = apply(&mut *mem, material);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &HUD_MATERIAL_PIXEL_SHADER,
            "tint a newly bound HUD shader",
            |c| {
                let material = c.cpu.esi;
                let _ = apply(&mut *c.ctx.memory(), material);
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &HUD_MATERIAL_READY,
            "restore the renderer's material cache",
            |c| {
                let mut mem = c.ctx.memory();
                if mem.u32(c.cpu.esp).ok() == Some(DRAW_READY_RETURN) {
                    let _ = restore(&mut *mem);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn argb_is_applied_to_both_endpoints_and_the_native_cache_is_restored() {
        let mut mem = FlatMemory::new(MATERIAL, 0x1600);
        for (at, n) in constants().zip(1..=6) {
            mem.set_u32(at, n).unwrap();
        }
        let argb = pack([0.5, 1.0, 0.0, 0.25]);
        DRAW.with(|draw| draw.borrow_mut().colour = Some(argb));
        apply(&mut mem, MATERIAL).unwrap();
        assert_eq!(argb, 0x80ff_0040);
        assert!(constants().all(|at| mem.u32(at).unwrap() == argb));
        restore(&mut mem).unwrap();
        assert!(
            constants()
                .zip(1..=6)
                .all(|(at, n)| mem.u32(at).unwrap() == n)
        );
        assert_eq!(mem.u32(MATERIAL + DIRTY_C0).unwrap(), DIRTY);
    }
}
