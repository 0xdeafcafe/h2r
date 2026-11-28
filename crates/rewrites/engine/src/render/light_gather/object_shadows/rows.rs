//! The object-shadow candidates, `0x00033400`. The eligibility and
//! local-player predicates run at their guest call sites. The candidates
//! use the lights' merge and rank, with a separate history per view.

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use crate::light_constants::Fpu;
use guest_layout::memory::{GuestMemory, MemFault};
use crate::render::light_gather::{
    History, HistoryRow, Refusal, Result, VisibleLight, Writes, guest, history, passes, visible,
};
use visible::VISIBLE_BYTES;

pub mod call {
    pub const ELIGIBLE: u32 = 0x0003_d7f0;
    pub const LOCAL_OWNER: u32 = 0x0003_e9c0;
    pub const MERGE: u32 = 0x0004_a7c0;
    pub const RANK: u32 = 0x0004_ac30;
}

pub mod site {
    pub const ELIGIBLE: u32 = 0x0003_348b;
    pub const LOCAL_OWNER: u32 = 0x0003_355b;
    pub const MERGE: u32 = 0x0003_35a0;
    pub const RANK: u32 = 0x0003_35b2;
}

/// Locals and the three saved registers while the list is being walked.
pub const FRAME: u32 = 0x2c;
const FADE: u32 = 0x14;
const KIND: u32 = 0x1d;
const HISTORY_FRAME: u32 = 0x24;
const OUTPUT_BYTES: usize = VISIBLE_BYTES * history::ROWS;
const VIEW_INDEX: u32 = 0x004b_9ed4;
const RENDER_FRAME: u32 = 0x004b_a034;
const OVERRIDE: u32 = 0x0051_0c50;
/// The immediates pushed by the caller of the ranking function.
const STEP: u32 = 0x0003_35a5;
const LIMIT: u32 = 0x0003_35aa;

#[derive(Debug)]
pub struct Plan {
    esp: u32,
    list: u32,
    output: u32,
    count: u32,
    view: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rows {
    pub view: u32,
    pub history: History,
    /// Ranked rows in the caller's buffer, with the history's faded weight.
    pub rows: Vec<VisibleLight>,
}

fn history_at(view: u32) -> u32 {
    history::OBJECT_SHADOWS + view * history::STRIDE
}

/// The guest merge and rank operate in place. A buffer covering its own
/// history needs those instructions' interleaved reads and writes.
fn separate_output(output: u32, addr: u32) -> bool {
    output
        .checked_add(OUTPUT_BYTES as u32 - 1)
        .is_some_and(|end| end < addr || output >= addr + history::STRIDE)
}

fn frozen(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    let state = m.u32(OVERRIDE)?;
    Ok(state != 0 && m.u8(state.wrapping_add(5))? != 0)
}

fn fits_mode(flags: u16, mode: u32) -> bool {
    match mode {
        0 => true,
        1 => flags & (1 << 14) != 0,
        2 => flags & (1 << 15) != 0,
        _ => false,
    }
}

struct Object {
    datum: u32,
    flags: u16,
    flags_column: u32,
}

fn object(m: &dyn GuestMemory, list: u32, index: u32) -> Result<Object, MemFault> {
    let handles = m.u32(list.wrapping_add(0xc))?;
    let flags_column = m.u32(list.wrapping_add(8))?;
    Ok(Object {
        datum: m.u32(handles.wrapping_add(index * 4))?,
        flags: m.u16(flags_column.wrapping_add(index * 2))?,
        flags_column,
    })
}

struct Projection {
    body: u32,
    centre: [f32; 3],
    radius: f32,
    depth: f32,
    size: f32,
}

fn projection(m: &dyn GuestMemory, datum: u32) -> Result<Projection, MemFault> {
    let pool = m.u32(guest::OBJECTS)?;
    let headers = m.u32(pool.wrapping_add(0x44))?;
    let body = m.u32(headers.wrapping_add((datum & 0xffff) * 12 + 8))?;
    let centre = m.vec3(body.wrapping_add(0x30))?;
    let radius = m.f32(body.wrapping_add(0x3c))?;
    let [x, y, z, w] = visible::PROJECTION_DEPTH;
    let signed =
        ((m.f32(z)? * centre[2] + m.f32(x)? * centre[0]) + m.f32(y)? * centre[1]) + m.f32(w)?;
    // COMISS/JAE keeps a negative zero; the unordered case subtracts.
    let distance = if signed >= 0.0 { signed } else { 0.0 - signed };
    let minimum = m.f32(visible::MIN_DEPTH)?;
    let depth = if distance > minimum {
        distance
    } else {
        minimum
    };
    let size = ((m.f32(visible::PIXEL_SCALE)? / depth) * radius) * m.f32(visible::SIZE_FACTOR)?;
    Ok(Projection {
        body,
        centre,
        radius,
        depth,
        size,
    })
}

/// Checks the entry, both predicate outputs, the columns and each possible
/// caster, the row buffer, and the history. The view and signed list count
/// are sampled at entry. The columns and sphere are read again in the
/// loop, on their respective sides of the guest eligibility call.
pub fn plan(m: &dyn GuestMemory, entry_esp: u32, list: u32, fpu: Fpu) -> Result<Plan> {
    visible::fp_supported(fpu)?;
    // The deepest call here has three stack arguments plus its return.
    let depth = FRAME + 16;
    m.bytes(
        entry_esp.checked_sub(depth).ok_or(Refusal::Layout)?,
        depth as usize,
    )?;
    let output = m.u32(guest::at(entry_esp, 4)?)?;
    let mode = m.u32(guest::at(entry_esp, 8)?)?;
    let view = m.i32(VIEW_INDEX)?.clamp(0, 3) as u32;
    let history_at = history_at(view);
    if !separate_output(output, history_at) {
        return Err(Refusal::Layout);
    }
    m.bytes(output, OUTPUT_BYTES)?;
    m.bytes(history_at, history::STRIDE as usize)?;
    if m.u32(history_at + history::COUNT)? > history::ROWS as u32 {
        return Err(Refusal::Layout);
    }
    let count = (m.u16(guest::at(list, 4)?)? as i16).max(0) as u32;
    for i in 0..count {
        let row = object(m, list, i)?;
        if fits_mode(row.flags, mode) {
            projection(m, row.datum)?;
        }
    }
    m.u32(RENDER_FRAME)?;
    frozen(m)?;
    // A push of a negative signed byte is not a usable rank limit.
    if (m.u8(LIMIT)? as i8) < 0 || !m.f32(STEP)?.is_finite() {
        return Err(Refusal::Unsupported);
    }
    Ok(Plan {
        esp: entry_esp,
        list,
        output,
        count,
        view,
    })
}

fn finish_candidate(m: &mut dyn GuestMemory, row: u32, local: u8) -> Result<(), MemFault> {
    m.set_u8(row, local)?;
    m.set_u8(row + 2, 0)?;
    m.set_u8(row + 1, 0)
}

fn read_rows(
    m: &dyn GuestMemory,
    output: u32,
    count: usize,
) -> Result<Vec<VisibleLight>, MemFault> {
    let bytes = m.bytes(output, count * VISIBLE_BYTES)?;
    Ok(bytes
        .chunks_exact(VISIBLE_BYTES)
        .map(|b| VisibleLight::decode(b.try_into().unwrap()))
        .collect())
}

async fn guest_rank(cx: &Cx, p: &Plan, history: u32) -> Result<u32, EntryError> {
    let output = cx.arg(0)?;
    let step = cx.read(|m| m.u32(STEP))?;
    let limit = cx.read(|m| m.u8(LIMIT))? as i8 as i32 as u32;
    let request = GuestCall::new(call::RANK)
        .ebx(p.list)
        .eax(history)
        .ecx(output)
        .args([history, output, limit, step]);
    Ok(passes::named(cx, p.esp, HISTORY_FRAME, site::RANK, request)
        .await?
        .eax)
}

/// The ranked history and rows accompany the guest return when Rust takes
/// the merge. If a predicate invalidates the layout, the two original
/// history functions finish the call and only their return is available.
pub async fn run(cx: &Cx, p: Plan) -> Result<(Ret, Option<Rows>), EntryError> {
    let mut n = 0;
    for i in 0..p.count {
        let object = cx.read(|m| object(m, p.list, i))?;
        if !fits_mode(object.flags, cx.arg(1)?) {
            continue;
        }
        let row = p.output.wrapping_add(n as u32 * VISIBLE_BYTES as u32);
        let cursor = row.wrapping_add(8);
        let fade = p.esp - FADE;
        let kind = p.esp - KIND;
        let regs = |target| {
            GuestCall::new(target)
                .ebx(p.list)
                .esi(object.datum)
                .edi(cursor)
        };
        let request = regs(call::ELIGIBLE)
            .eax(kind)
            .ecx(fade)
            .edx(object.flags_column)
            .args([object.datum, fade, kind]);
        let answer = passes::named(cx, p.esp, FRAME, site::ELIGIBLE, request).await?;
        // 0x00033493 checks the cap *after* the call.
        if answer.eax as u8 == 0 || n == history::ROWS {
            continue;
        }
        let projected = cx.write(|m| -> Result<_, MemFault> {
            let weight = m.u32(fade)?;
            let projected = projection(m, object.datum)?;
            m.set_u32(row + 12, weight)?;
            m.set_u32(row + 4, object.datum)?;
            m.set_u32(row + 8, object.datum)?;
            m.set_f32(row + 16, projected.size)?;
            Ok(projected)
        })?;
        let request = regs(call::LOCAL_OWNER)
            .eax(projected.body)
            .ecx(projected.centre[1].to_bits())
            .edx(projected.centre[2].to_bits())
            .xmm0(u128::from(projected.depth.to_bits()))
            .xmm1(u128::from(projected.size.to_bits()))
            .xmm2(u128::from(projected.radius.to_bits()));
        let local = passes::named(cx, p.esp, FRAME, site::LOCAL_OWNER, request)
            .await?
            .eax as u8;
        cx.write(|m| finish_candidate(m, row, local))?;
        n += 1;
    }

    // Both the history and the output argument are read at the merge,
    // after every predicate. Only the view index is an entry-time value.
    let output = cx.arg(0)?;
    let addr = history_at(p.view);
    let decoded = cx.read(|m| History::decode(m, addr));
    let mut history = match decoded {
        Ok(h) if separate_output(output, addr) => h,
        _ => {
            let request = GuestCall::new(call::MERGE)
                .ebx(p.list)
                .eax(addr)
                .ecx(n as u32)
                .edx(output)
                .args([output, n as u32]);
            let result = passes::named(cx, p.esp, HISTORY_FRAME, site::MERGE, request).await?;
            return Ok((Ret::new(guest_rank(cx, &p, result.eax).await?, 8), None));
        }
    };
    let candidates = cx.read(|m| read_rows(m, output, n))?;
    let frame = cx.read(|m| m.u32(RENDER_FRAME))?;
    history::merge(&mut history, &candidates, frame);
    let freeze = history.count == 0 || cx.read(frozen)?;
    let limit = cx.read(|m| m.u8(LIMIT))?;
    let step = cx.read(|m| m.u32(STEP))?;

    let mut writes = Writes::new();
    let rows = if freeze || history.finite() {
        if !freeze {
            history::rank_and_fade(
                history.rows_mut(),
                frame,
                limit.into(),
                f32::from_bits(step),
            );
        }
        history.encode(addr, &mut writes);
        let rows: Vec<_> = history
            .rows()
            .iter()
            .map(|r| VisibleLight {
                weight: r.weight,
                ..r.visible
            })
            .collect();
        writes.array(output, rows.iter().flat_map(VisibleLight::encode).collect());
        cx.write(|m| writes.apply(m))?;
        rows
    } else {
        // A predicate can return a non-finite fade after publication. The
        // merge is already valid; leave just its floating-point ranking to
        // 0x0004ac30 with the original arguments.
        history.encode(addr, &mut writes);
        cx.write(|m| writes.apply(m))?;
        guest_rank(cx, &p, addr).await?;
        for (i, r) in history.rows_mut().iter_mut().enumerate() {
            let mut bytes = [0; history::ROW_BYTES];
            cx.read(|m| m.read(addr + (i * history::ROW_BYTES) as u32, &mut bytes))?;
            *r = HistoryRow::decode(&bytes);
        }
        cx.read(|m| read_rows(m, output, history.count))?
    };
    Ok((
        Ret::new(history.count as u32, 8),
        Some(Rows {
            view: p.view,
            history,
            rows,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_shadow_row_keeps_the_buffers_stale_keep_byte() {
        let base = 0x2000;
        let mut m = guest_layout::FlatMemory::new(base, VISIBLE_BYTES);
        for stale in 0..=u8::MAX {
            m.write(base, &[0xff, 0xff, 0xff, stale]).unwrap();
            finish_candidate(&mut m, base, 1).unwrap();
            let rows = read_rows(&m, base, 1).unwrap();
            assert_eq!(&rows[0].encode()[..4], &[1, 0, 0, stale]);
        }
    }
}
