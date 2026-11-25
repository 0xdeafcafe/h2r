//! The per-frame random advance for the sound datum pool. The first slot
//! lookup goes through normal dispatch; subsequent slots are searched a
//! bitmap word at a time. Read the two LCG operands from the loaded code.

use super::*;
use crate::datum::layout;
use crate::points::{Point, Sig};
use sim_sound::scheduling::next_in_word;

pub(super) static POINT: Point = Point::new(
    "SOUND_FRAME_SEEDS",
    0x219cc0,
    Sig::Fn { args: &[], ret: None, pop: 0 },
    "docs/engine/sim-rewrite.md",
);

pub(super) const BODY: Body = Body {
    point: &POINT,
    sites: &[0x219cd0],
    run: advance,
};

const POOL: u32 = 0x0051_ebd8;
const DEFAULT_RECORD: u32 = 0x0051_ebdc;
const FIRST_OCCUPIED: u32 = 0x0016_bc00;
const MULTIPLIER: u32 = 0x0021_9d14;
const INCREMENT: u32 = 0x0021_9d1a;
const NONE: u32 = u32::MAX;

fn advance(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == POINT.base {
        push(c, g, c.ebx)?;
        c.ebx = g.u32(POOL)?;
        c.edx = 0;
        logic(c, 0, 1 << 31);
        c.ecx = c.ebx;
        return call(FIRST_OCCUPIED, 0x219cd0);
    }

    let pool = c.ebx;
    c.ecx = NONE;
    if c.eax != NONE {
        let index = c.eax;
        let stride = g.u32(pool.wrapping_add(layout::ELEMENT_SIZE))?;
        c.edx = g.u32(pool.wrapping_add(layout::DATA))?;
        c.ecx = handle(g, index, c.edx, stride)?;
    }
    if c.ecx != NONE {
        push(c, g, c.ebp)?;
        push(c, g, c.esi)?;
        push(c, g, c.edi)?;
        let multiplier = g.u32(MULTIPLIER)?;
        let increment = g.u32(INCREMENT)?;
        while c.ecx != NONE {
            // The frame looks up the default pointer even on a live row.
            g.u32(DEFAULT_RECORD)?;
            let index = c.ecx & 0xffff;
            let data = g.u32(pool.wrapping_add(layout::DATA))?;
            let seed = data.wrapping_add(index * 0x1c + 8);
            c.edx = g.u32(seed)?.wrapping_mul(multiplier).wrapping_add(increment);
            g.set_u32(seed, c.edx)?;

            c.eax = index + 1;
            let limit = g.u32(pool.wrapping_add(layout::HIGH_WATER))?;
            c.ecx = NONE;
            if (c.eax as i32) < (limit as i32) {
                c.edx = g.u32(pool.wrapping_add(layout::BITMAP))?;
                let bitmap = c.edx;
                while c.eax < limit {
                    let word = g.u32(bitmap.wrapping_add((c.eax / 32) * 4))?;
                    if let Some(next) = next_in_word(word, c.eax, limit) {
                        let stride = g.u32(pool.wrapping_add(layout::ELEMENT_SIZE))?;
                        c.eax = g.u32(pool.wrapping_add(layout::DATA))?;
                        c.edx = stride.wrapping_mul(next);
                        c.ecx = handle(g, next, c.eax, stride)?;
                        break;
                    }
                    c.eax = ((c.eax & !31) + 32).min(limit);
                }
            }
        }
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        c.ebp = pop(c, g)?;
    }
    arithmetic(c, c.ecx, NONE, true);
    c.ebx = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn handle(g: &Guest, index: u32, data: u32, stride: u32) -> Result<u32> {
    let record = data.wrapping_add(index.wrapping_mul(stride));
    Ok((u32::from(g.u16(record)?) << 16) | index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn updates_live_seeds_in_a_sparse_pool() {
        let mut g = Guest::new();
        for page in [0x1000, 0x2000, MULTIPLIER & !4095, POOL & !4095] {
            g.map_zeroed(page, 0x1000, Permissions::RW).unwrap();
        }
        let pool = 0x1000;
        let data = 0x2000;
        let bitmap = 0x1200;
        g.set_u32(POOL, pool).unwrap();
        g.set_u32(MULTIPLIER, 7).unwrap();
        g.set_u32(INCREMENT, 3).unwrap();
        g.set_u32(pool + layout::ELEMENT_SIZE, 0x1c).unwrap();
        g.set_u32(pool + layout::DATA, data).unwrap();
        g.set_u32(pool + layout::HIGH_WATER, 36).unwrap();
        g.set_u32(pool + layout::BITMAP, bitmap).unwrap();
        g.set_u32(bitmap, 2).unwrap();
        g.set_u32(bitmap + 4, (1 << 2) | (1 << 11)).unwrap();
        for index in 0..50 {
            g.set_u32(data + 0x1c * index + 8, index).unwrap();
        }
        let mut cpu = CpuAbi { esp: 0x1900, ebx: 42, ..Default::default() };
        let mut cx = Cx { cpu: &mut cpu, g: &mut g };
        assert_eq!(advance(&mut cx, POINT.base).unwrap(), Next::Call { target: FIRST_OCCUPIED, site: 0x219cd0 });
        cx.cpu.eax = 1;
        assert_eq!(advance(&mut cx, 0x219cd0).unwrap(), Next::Ret { pop: 0 });
        assert_eq!((cx.cpu.ebx, cx.cpu.esp, cx.cpu.ecx, cx.cpu.eax), (42, 0x1900, NONE, 36));
        for index in 0..50 {
            let expected = if index == 1 || index == 34 { 7 * index + 3 } else { index };
            assert_eq!(cx.g.u32(data + 0x1c * index + 8).unwrap(), expected);
        }
    }
}
