//! `126ec0`: select a sound promotion rule and decide whether its current
//! cooldown suppresses this start. Both callers in the impulse path use the
//! returned rule index to start the voice if the result is zero.

use super::{at, compare_small};
use crate::points::{self, Loc, Point, Sig};
use crate::sim_objects::{Body, Cx, Next, Result, alu, sse};
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::Guest;
use sim_sound::scheduling::{Admission, PromotionCredit, admission, decay_counter};

const SELECT: Point = Point::new(
    "SOUND_PROMOTION_SELECT",
    0x126ec0,
    Sig::Fn {
        args: &[("sound", Loc::Ecx), ("selected_rule", Loc::Stack(0))],
        ret: Some(Loc::Eax),
        pop: 4,
    },
    super::DOC,
);
pub(super) const BODY: Body = Body {
    point: &SELECT,
    sites: &[],
    run: select,
};

const GESTALT: u32 = 0x0051_ebd4;
const TIME_SCALE: u32 = 0x0045_dccc;

#[derive(Clone, Copy)]
struct Promotion(u32);

impl Promotion {
    fn counter(self, g: &Guest, index: i32) -> Result<u32> {
        Ok(at(g.u32(at(self.0, 0xc))?, (index as u32).wrapping_mul(4)))
    }

    fn rule(self, g: &Guest, index: i32) -> Result<u32> {
        Ok(at(g.u32(at(self.0, 4))?, (index as u32).wrapping_mul(16)))
    }

    fn decay(self, g: &mut Guest, elapsed: i32) -> Result<()> {
        let mut index = 0;
        while index < g.u32(at(self.0, 8))? as i32 {
            let at = self.counter(g, index)?;
            let remaining = decay_counter(g.u32(at)? as i32, elapsed);
            g.set_u32(at, remaining as u32)?;
            index += 1;
        }
        Ok(())
    }

    fn next_rule(self, g: &mut Guest) -> Result<i32> {
        let mut index = 0;
        while index < g.u32(self.0)? as i32 {
            let counter = self.counter(g, index)?;
            let row = self.rule(g, index)?;
            let increment = g.u32(at(row, 12))? as i32;
            let credit = PromotionCredit::accrue(g.u32(counter)? as i32, increment);
            g.set_u32(counter, credit.0 as u32)?;
            // A caller may point the counters into a rule. The gate sees the
            // accrued value, and a disabled gate never touches the threshold.
            if g.u16(at(row, 2))? as i16 <= 0
                || !credit.reached(g.u32(at(row, 8))? as i32)
                || index >= (g.u32(self.0)? as i32).wrapping_sub(1)
            {
                break;
            }
            g.set_u32(counter, 0)?;
            index += 1;
        }
        Ok(index)
    }
}

fn select(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.edx = g.u32(points::TAG_ENTRIES.base)?;
    c.ecx = g.u32(at(c.edx, (c.ecx & 0xffff) * 16 + 8))?;
    let index = g.u8(at(c.ecx, 12))?;
    push(c, g, c.esi)?;
    c.esi = 0;
    c.eax = 0;
    compare_small(c, index.into(), 255, 0x80);
    push(c, g, c.edi)?;

    let promotion = if index == 255 {
        None
    } else {
        c.edx = g.u32(GESTALT)?;
        c.edi = g.u32(at(c.edx, 0x4c))?;
        c.ecx = arithmetic(c, (index as i8 as u32).wrapping_mul(28), c.edi, false);
        (c.ecx != 0).then_some(Promotion(c.ecx))
    };
    if let Some(promotion) = promotion {
        push(c, g, c.ebx)?;
        c.ebx = g.u32(points::SOUND_GLOBALS.base)?;
        let clock_address = at(c.ebx, 0x84);
        if g.u32(at(promotion.0, 24))? as i32 <= g.u32(clock_address)? as i32 {
            g.set_u32(at(promotion.0, 16), u32::MAX)?;
            g.set_u32(at(promotion.0, 24), 0)?;
        }
        let elapsed = g
            .u32(clock_address)?
            .wrapping_sub(g.u32(at(promotion.0, 20))?);
        push(c, g, c.ebp)?;
        promotion.decay(g, elapsed as i32)?;
        let chosen = promotion.next_rule(g)?;
        let out = g.u32(at(c.esp, 20))?;
        g.set_u32(out, chosen as u32)?;
        let active = g.u32(at(promotion.0, 16))?;
        let now = g.u32(clock_address)?;
        let until = g.u32(at(promotion.0, 24))?;
        let action = admission(chosen, active as i32, now as i32, until as i32);
        c.ebp = pop(c, g)?;
        c.eax = u32::from(action == Admission::Suppress);
        arithmetic(c, chosen as u32, active, true);
        if chosen >= active as i32 {
            logic(c, c.eax, 1 << 31);
        }
        if action == Admission::AdmitAndArm {
            let rules = g.u32(at(promotion.0, 4))?;
            g.set_u32(at(promotion.0, 16), chosen as u32)?;
            sse::cvtsi2ss(c, 1, g.u32(clock_address)?);
            let offset = alu::shl(c, chosen as u32, 4, 32);
            sse::load(c, 0, g.u32(at(at(rules, offset), 4))?);
            sse::ss(c, sse::Ss::Mul, 0, g.u32(TIME_SCALE)?);
            sse::ss_reg(c, sse::Ss::Add, 0, 1);
            let deadline = sse::cvttss2si(c, sse::lane0(c, 0));
            g.set_u32(at(promotion.0, 24), deadline)?;
        }
        c.edx = g.u32(clock_address)?;
        c.ebx = pop(c, g)?;
        c.edi = pop(c, g)?;
        g.set_u32(at(promotion.0, 20), c.edx)?;
    } else {
        c.ecx = g.u32(at(c.esp, 12))?;
        c.edi = pop(c, g)?;
        g.set_u32(c.ecx, u32::MAX)?;
    }
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}
