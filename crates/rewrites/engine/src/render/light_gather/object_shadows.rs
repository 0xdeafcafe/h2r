//! Object shadows (`0x000335e0`), slice L7 of docs/graphics/native-light-gather.md.
//! The gates run above the saved registers. The target setup, the row gather
//! and each caster run below them, so observers and callees see the same row
//! addresses as in the guest. Each nested entry can decline its own plan.

pub mod caster;
pub mod rows;

use super::{Refusal, Result, passes};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use guest_layout::points;

/// The rows occupy the top of the frame; EBX, ESI and EDI are saved below.
const ROW_BYTES: u32 = 0x280;
const FRAME: u32 = ROW_BYTES + 12;
const MAIN_OBJECT_LIST: u32 = super::MAIN_VIS + 0x14;
const SHADOW_MODE: u32 = 0x0048_5a46;
const GATHER_ACTIVE: u32 = 0x0050_9413;
/// The immediate loaded into EDI before sizing target 9.
const SIZE_WORD: u32 = 0x0003_35fc;

pub mod site {
    pub const LIGHTMAP: u32 = 0x0003_35eb;
    pub const OVERRIDE: u32 = 0x0003_35f4;
    pub const SIZE_TARGET: u32 = 0x0003_3614;
    pub const ROWS: u32 = 0x0003_3632;
    pub const CASTER: u32 = 0x0003_364c;
}

#[derive(Clone, Copy, Debug)]
pub struct Plan {
    entry_esp: u32,
}

/// Check the frame, both argument slots of the deepest outgoing call, and
/// each address the list body accesses. The gather and caster preflight
/// their own inputs at the point the guest calls them.
pub fn plan(m: &dyn GuestMemory, entry_esp: u32) -> Result<Plan> {
    let bottom = entry_esp.checked_sub(FRAME + 12).ok_or(Refusal::Layout)?;
    m.bytes(bottom, (FRAME + 12) as usize)?;
    m.u32(entry_esp.checked_add(4).ok_or(Refusal::Layout)?)?;
    m.u32(MAIN_OBJECT_LIST)?;
    m.u32(SIZE_WORD)?;
    m.u8(SHADOW_MODE)?;
    m.u8(GATHER_ACTIVE)?;
    Ok(Plan { entry_esp })
}

/// Run the two gates, set up the target, gather into the frame, and pass
/// each caster its live row words. A caster may change a subsequent row.
pub async fn run(cx: &Cx, p: Plan) -> Result<Ret, EntryError> {
    let esp = p.entry_esp;
    let rows = esp - ROW_BYTES;
    for (callee, site) in [(0x0002_26d0, site::LIGHTMAP), (0x0003_35c0, site::OVERRIDE)] {
        let r = passes::named(cx, esp, ROW_BYTES, site, GuestCall::new(callee)).await?;
        if r.eax as u8 == 0 {
            return Ok(Ret::new(r.eax, 4));
        }
    }

    let size = cx.read(|m| m.u32(SIZE_WORD))?;
    let entry = cx.entry();
    cx.write(|m| {
        m.set_u32(rows - 4, entry.ebx)?;
        m.set_u32(rows - 8, entry.esi)?;
        m.set_u32(rows - 12, entry.edi)?;
        m.set_u8(SHADOW_MODE, 1)
    })?;
    let size_target = GuestCall::new(0x0001_ddd0)
        .ebx((entry.ebx & !0xff) | 1)
        .ecx(9)
        .esi(size)
        .edi(size);
    passes::named(cx, esp, FRAME, site::SIZE_TARGET, size_target).await?;

    let mode = cx.arg(0)?;
    cx.write(|m| m.set_u8(GATHER_ACTIVE, 1))?;
    let list = cx.read(|m| m.u32(MAIN_OBJECT_LIST))?;
    let gather = GuestCall::new(points::GATHER_OBJECT_SHADOWS.base)
        .eax(mode)
        .ecx(rows)
        .ebx(list)
        .esi(size)
        .edi(size)
        .args([rows, mode]);
    let count = passes::named(cx, esp, FRAME, site::ROWS, gather).await?.eax;
    let mut eax = count;
    let mut cursor = rows + 8;
    for remaining in (1..=count as i32).rev() {
        let caster = cx.read(|m| m.u32(cursor))?;
        let fade = cx.read(|m| m.u32(cursor + 4))?;
        let draw = GuestCall::new(points::OBJECT_SHADOW.base)
            .eax(caster)
            .edx(fade)
            .ebx(0)
            .esi(cursor)
            .edi(remaining as u32)
            .args([caster, fade]);
        eax = passes::named(cx, esp, FRAME, site::CASTER, draw).await?.eax;
        cursor = cursor.wrapping_add(0x14);
    }
    cx.write(|m| {
        m.set_u8(GATHER_ACTIVE, 0)?;
        m.set_u8(SHADOW_MODE, 0)
    })?;
    Ok(Ret::new(eax, 4))
}
