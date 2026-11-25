//! Submit stream and buffer mix-bin changes with the same DirectSound calls
//! as the sound driver. Descriptors stay on the guest stack across callbacks.

use crate::points::{Loc, Point, Sig};
use crate::sim_objects::{Body, Cx, Next, Result, frame, sse};
use crate::sim_physics::{arithmetic, byte, logic};
use sim_sound::mix::{BinAction, BinChanges};

mod levels;
mod route;
mod update;
use update::{load_x87, store_x87};

enum Step {
    To(u32),
    Call(Next),
}

const DOC: &str = "docs/audio/dsound.md";
const BINS_SIG: Sig = Sig::Fn {
    args: &[
        ("previous", Loc::Eax),
        ("requested", Loc::Stack(0)),
        ("voice", Loc::Stack(1)),
    ],
    ret: None,
    pop: 8,
};

static STREAM_BINS: Point = Point::new("SOUND_STREAM_MIX_BINS", 0x2215d0, BINS_SIG, DOC);
static BUFFER_BINS: Point = Point::new("SOUND_BUFFER_MIX_BINS", 0x2216f0, BINS_SIG, DOC);
static ADD_SEND: Point = Point::new(
    "SOUND_APPEND_MIX_BIN",
    0x21e230,
    Sig::Fn {
        args: &[
            ("bin", Loc::Ecx),
            ("sends", Loc::Esi),
            ("gain", Loc::Stack(0)),
        ],
        ret: None,
        pop: 4,
    },
    DOC,
);

static UPDATE: Point = Point::new(
    "SOUND_STREAM_PARAMETERS",
    0x21fa80,
    Sig::Fn {
        args: &[
            ("parameters", Loc::Edi),
            ("stream", Loc::Stack(0)),
            ("effects", Loc::Stack(1)),
            ("skip_pitch", Loc::Stack(2)),
        ],
        ret: None,
        pop: 12,
    },
    DOC,
);

pub const BODIES: &[Body] = &[
    Body {
        point: &UPDATE,
        sites: update::SITES,
        run: update::update,
    },
    Body {
        point: &STREAM_BINS,
        sites: &[0x2216b7, 0x2216dc],
        run: stream_bins,
    },
    Body {
        point: &BUFFER_BINS,
        sites: &[0x2217d7, 0x2217fc],
        run: buffer_bins,
    },
    Body {
        point: &ADD_SEND,
        sites: &[0x21e249],
        run: add_send,
    },
];

fn add_send(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let count_at = c.esi.wrapping_add(0x40);
    c.eax = g.u32(count_at)?;
    let row = c.esi.wrapping_add(c.eax.wrapping_mul(8));
    if pc == ADD_SEND.base {
        g.set_u32(row, c.ecx)?;
        c.eax = frame::slot(c, g, 4)?;
        frame::push(c, g, c.eax)?;
        frame::push(c, g, 0)?;
        let floor = g.u32(0x21e23e)?;
        frame::push(c, g, floor)?;
        c.ecx = byte(c.ecx, 1);
        return Ok(Next::Call {
            target: 0x12aff0,
            site: 0x21e249,
        });
    }
    sse::movaps(c, 1, 0);
    sse::load(c, 0, g.u32(0x45e008)?);
    sse::ss_reg(c, sse::Ss::Mul, 1, 0);
    let gain = sse::ss_reg(c, sse::Ss::Sub, 1, 0);
    c.edx = sse::cvttss2si(c, gain);
    g.set_u32(row.wrapping_add(4), c.edx)?;
    let count = g.u32(count_at)?;
    let cf = c.eflags & 1;
    let cf_defined = c.eflags_defined & 1;
    let next = arithmetic(c, count, 1, false);
    c.eflags = (c.eflags & !1) | cf;
    c.eflags_defined = (c.eflags_defined & !1) | cf_defined;
    g.set_u32(count_at, next)?;
    Ok(Next::Ret { pop: 4 })
}

struct BinApi {
    entry: u32,
    set_volumes: u32,
    set_bins: u32,
}

fn stream_bins(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    submit_bins(
        cx,
        pc,
        BinApi {
            entry: STREAM_BINS.base,
            set_volumes: 0x37b81d,
            set_bins: 0x37c70a,
        },
    )
}

fn buffer_bins(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    submit_bins(
        cx,
        pc,
        BinApi {
            entry: BUFFER_BINS.base,
            set_volumes: 0x37b6c3,
            set_bins: 0x37c5e4,
        },
    )
}

fn submit_bins(cx: &mut Cx<'_>, pc: u32, api: BinApi) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == api.entry {
        c.esp = c.esp.wrapping_sub(0xdc);
        for reg in [c.ebx, c.ebp, c.esi, c.edi] {
            frame::push(c, g, reg)?;
        }
        c.edi = 0;
        c.ebx = 0;
        c.ecx = c.esp.wrapping_add(0x2c);
        c.ebp = 0;
        arithmetic(c, c.eax, 0, true);
        frame::set_slot(c, g, 0x18, 0)?;
        frame::set_slot(c, g, 0x1c, c.ecx)?;
        frame::set_slot(c, g, 0x14, 0)?;
        if let Some(full) = compare_bins(cx)? {
            let (c, g) = (&mut *cx.cpu, &mut *cx.g);
            let (target, site) = if full {
                c.edx = frame::slot(c, g, 0xf0)?;
                c.eax = frame::slot(c, g, 0xf4)?;
                c.edx = arithmetic(c, c.edx, 0x40, false);
                frame::push(c, g, c.edx)?;
                frame::push(c, g, c.eax)?;
                (api.set_bins, api.entry + 0x10c)
            } else {
                c.edx = frame::slot(c, g, 0xf4)?;
                c.ecx = c.esp.wrapping_add(0x18);
                frame::push(c, g, c.ecx)?;
                frame::push(c, g, c.edx)?;
                (api.set_volumes, api.entry + 0xe7)
            };
            return Ok(Next::Call { target, site });
        }
    }
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    c.edi = frame::pop(c, g)?;
    c.esi = frame::pop(c, g)?;
    c.ebp = frame::pop(c, g)?;
    c.ebx = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0xdc, false);
    Ok(Next::Ret { pop: 8 })
}

/// Some(true) replaces routing, Some(false) updates levels, None leaves the
/// voice alone. The temporary gain array is addressed exactly as the driver
/// addresses it, including on duplicate send bins.
fn compare_bins(cx: &mut Cx<'_>) -> Result<Option<bool>> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if c.eax == 0 {
        return Ok(Some(true));
    }
    c.esi = frame::slot(c, g, 0xf0)?;
    c.ecx = g.u32(c.eax)?;
    let count = c.ecx;
    frame::set_slot(c, g, 0x28, count)?;
    if count != g.u32(c.esi.wrapping_add(0x40))? {
        return Ok(Some(true));
    }
    c.edx = c.eax.wrapping_add(4);
    frame::set_slot(c, g, 0x20, c.edx)?;
    g.set_u8(c.esp.wrapping_add(0x13), 0)?;
    if (count as i32) <= 0 {
        return Ok(None);
    }
    c.ecx = frame::slot(c, g, 0xf0)?;
    c.eax = c.edx.wrapping_sub(c.ecx);
    c.esi = c.esi.wrapping_add(4);
    frame::set_slot(c, g, 0x24, c.eax)?;
    let mut bins = BinChanges::default();
    loop {
        c.ecx = g.u32(c.esi.wrapping_sub(4))?;
        if g.u32(c.edx.wrapping_add(c.edi.wrapping_mul(8)))? != c.ecx {
            return Ok(Some(true));
        }
        c.edx = g.u32(c.esi)?;
        let changed = g.u32(c.eax.wrapping_add(c.esi))? != c.edx;
        let mask = 1u32.wrapping_shl(c.ecx);
        let agrees = changed
            && bins.seen & mask != 0
            && g.u32(c.esp.wrapping_add(0x6c).wrapping_add(c.edi.wrapping_mul(4)))? == c.edx;
        let action = bins.visit(c.ecx, changed, agrees);
        if action == BinAction::Rebind {
            return Ok(Some(true));
        }
        if changed {
            frame::set_slot(c, g, 0x14, bins.changed)?;
            c.ebx = frame::slot(c, g, 0x18)?;
            g.set_u8(c.esp.wrapping_add(0x13), 1)?;
        }
        if action == BinAction::Append {
            let row = c.esp.wrapping_add(0x2c).wrapping_add(c.ebx.wrapping_mul(8));
            g.set_u32(row.wrapping_add(4), c.edx)?;
            c.ebp = bins.seen;
            g.set_u32(row, c.ecx)?;
            c.ebx = c.ebx.wrapping_add(1);
            g.set_u32(
                c.esp.wrapping_add(0x6c).wrapping_add(c.ecx.wrapping_mul(4)),
                c.edx,
            )?;
            frame::set_slot(c, g, 0x18, c.ebx)?;
        }
        c.eax = frame::slot(c, g, 0x28)?;
        c.edi = c.edi.wrapping_add(1);
        c.esi = c.esi.wrapping_add(8);
        if (c.edi as i32) >= c.eax as i32 {
            break;
        }
        c.eax = frame::slot(c, g, 0x24)?;
        c.edx = frame::slot(c, g, 0x20)?;
    }
    let changed = g.u8(c.esp.wrapping_add(0x13))?;
    c.eax = byte(c.eax, changed);
    logic(c, changed.into(), 0x80);
    Ok((changed != 0).then_some(false))
}
