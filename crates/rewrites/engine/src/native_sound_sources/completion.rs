//! Completion is synchronous. Disconnect the stream, release the source's
//! resources, notify its owner, and only then free the instance. The owner's
//! callback may itself finish another sound.
use super::*;

pub(super) const FINISH: Body = Body {
    point: &points::SOUND_FINISH_INSTANCE,
    sites: &[0x127350, 0x127360, 0x12737a, 0x12737f],
    run: finish,
};
pub(super) const FREE: Body = Body {
    point: &points::SOUND_FREE_INSTANCE,
    sites: &[0x1273e7],
    run: free,
};

fn finish(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x127320 => {
            frame::push(c, g, c.esi)?;
            frame::push(c, g, c.edi)?;
            c.edi = c.eax;
            c.eax = g.u32(points::SOUND_INSTANCES.base)?;
            c.edx = g.u32(c.eax.wrapping_add(RECORDS))?;
            c.esi = c.edx.wrapping_add((c.edi & 0xffff) * SoundInstance::SIZE);
            c.eax = u32::from(g.u16(c.esi.wrapping_add(0xac))?);
            alu::cmp16(c, c.eax, u16::MAX.into());
            if c.eax != u32::from(u16::MAX) {
                return call(0x12a5d0, 0x127350);
            }
            c.ecx = c.esi;
            call(0x126960, 0x127360)
        }
        0x127350 => {
            g.set_u16(c.esi.wrapping_add(0xac), u16::MAX)?;
            c.ecx = c.esi;
            call(0x126960, 0x127360)
        }
        0x127360 => {
            c.eax = g.u32(c.esi.wrapping_add(0x14))?;
            logic(c, c.eax, 1 << 31);
            if c.eax != 0 {
                c.eax = g.u32(c.eax.wrapping_add(0x10))?;
                logic(c, c.eax, 1 << 31);
                if c.eax != 0 {
                    c.ecx = frame::slot(c, g, 12)?;
                    c.edx = g.u32(c.esi.wrapping_add(0x10))?;
                    frame::push(c, g, c.ecx)?;
                    frame::push(c, g, c.edi)?;
                    frame::push(c, g, c.edx)?;
                    return call(c.eax, 0x12737a);
                }
            }
            call(points::SOUND_FREE_INSTANCE.base, 0x12737f)
        }
        0x12737a => call(points::SOUND_FREE_INSTANCE.base, 0x12737f),
        0x12737f => {
            c.edi = frame::pop(c, g)?;
            c.esi = frame::pop(c, g)?;
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(invalid_site(pc)),
    }
}

fn free(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x127390 => {
            c.ecx = (c.edi & 0xffff) * SoundInstance::SIZE;
            frame::push(c, g, c.ebx)?;
            c.ebx = g.u32(points::SOUND_INSTANCES.base)?;
            c.ecx = c.ecx.wrapping_add(g.u32(c.ebx.wrapping_add(RECORDS))?);
            c.eax = g.u32(c.ecx.wrapping_add(0xa8))?;
            c.edx = u32::MAX;
            arithmetic(c, c.eax, c.edx, true);
            if c.eax != u32::MAX {
                c.eax = (c.eax & 0xffff) * 0x1c;
                frame::push(c, g, c.esi)?;
                c.esi = g.u32(LOOP_COMPANIONS)?;
                c.eax = c.eax.wrapping_add(g.u32(c.esi.wrapping_add(RECORDS))?);
                let layer = g.u8(c.ecx.wrapping_add(3))?;
                instances::cmp_byte(c, layer, 0xff);
                if layer == 0xff {
                    let owner = g.u32(c.eax.wrapping_add(4))?;
                    arithmetic(c, owner, c.edx, true);
                    if owner != u32::MAX {
                        let count = g.u8(c.eax.wrapping_add(3))?.wrapping_sub(1) & 0x7f;
                        c.edx = byte(c.edx, count);
                        g.set_u8(c.eax.wrapping_add(3), count)?;
                        logic(c, count.into(), 0x80);
                    }
                }
                c.esi = g.u32(c.ecx.wrapping_add(0xa8))?;
                return call(0x219c80, 0x1273e7);
            }
        }
        0x1273e7 => c.esi = frame::pop(c, g)?,
        _ => return Err(invalid_site(pc)),
    }
    c.edx = c.ebx;
    c.eax = c.edi;
    c.ebx = frame::pop(c, g)?;
    Ok(Next::Tail {
        target: DATUM_DELETE,
    })
}
