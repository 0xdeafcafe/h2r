//! The shot-assist block copied to action +0x34 by the local builder at
//! 0x187074. The outer routine (0x1a4900) indexes a local controller and
//! camera. Its weapon parameters and target search accept explicit inputs.

use engine::entry::{Cx, EntryError, GuestCall};
use engine::handle::DatumHandle;
use engine::memory::MemFault;
use engine::view::objects;

use super::session::MAX;

// One guest workspace per player slot. The search returns a 0x60-byte
// candidate. Its pointers must survive guest calls that use the guest stack.
const STRIDE: u32 = 0xb0;
pub const AIM_WORK_BYTES: u32 = MAX * STRIDE;

pub(super) async fn fill(
    cx: &Cx,
    unit: DatumHandle,
    team: i8,
    player_index: u16,
    workspace: u32,
    out: u32,
) -> Result<(), EntryError> {
    let unit = unit.raw();
    let params = workspace + u32::from(player_index) * STRIDE;
    let eye = params + 0x30;
    let direction = params + 0x40;
    let candidate = params + 0x50;

    // 0x1a4978 uses the controlled weapon unit (which may be a vehicle).
    let shooter = cx.call(GuestCall::new(0xcb6d0).eax(unit)).await?.eax;
    let zoom = cx.read(|m| m.u16(out + 0x24))? as i16;
    if cx
        .call(
            GuestCall::new(0x1a50a0)
                .ecx(shooter)
                .esi(params)
                .args([zoom as u32]),
        )
        .await?
        .eax as u8
        == 0
    {
        return Ok(());
    }

    // The same unit/seat camera origin the barrel asks for at 0x1a4cd9.
    cx.call(GuestCall::new(0x1557c0).eax(unit).args([eye, direction]))
        .await?;
    cx.write(|m| -> Result<(), MemFault> {
        let (sy, cy) = m.f32(out + 4)?.sin_cos();
        let (sp, cp) = m.f32(out + 8)?.sin_cos();
        for (i, v) in [cy * cp, sy * cp, sp].into_iter().enumerate() {
            m.set_f32(direction + i as u32 * 4, v)?;
        }
        Ok(())
    })?;
    // Seat transform and the object to exclude from the cone, as at
    // 0x1a49cf and 0x1a4a40. The game handles team, range and occlusion.
    cx.call(GuestCall::new(0xcb810).eax(unit).args([direction]))
        .await?;
    let root = cx.call(GuestCall::new(0xbaf40).eax(shooter)).await?.eax;
    if cx
        .call(GuestCall::new(0x1a5370).ecx(eye).args([
            params,
            direction,
            root,
            team as u32,
            candidate,
        ]))
        .await?
        .eax as u8
        == 0
    {
        return Ok(());
    }

    // 0x1a4a74 / 0x1a4b95: two hand strengths, object, model target
    // index, and controlling player for unit targets. The action starts
    // with the retail empty sentinels each tick.
    cx.write(|m| -> Result<(), MemFault> {
        let primary = m.f32(candidate + 0x4c)?;
        let secondary = m.f32(candidate + 0x50)?;
        if primary <= 0. && secondary <= 0. && m.u8(candidate + 0x28)? == 0 {
            return Ok(());
        }
        let Some(target) = DatumHandle::from_raw(m.u32(candidate)?) else {
            return Ok(());
        };
        let Some((kind, body)) = objects::get(m, target)? else {
            return Ok(());
        };
        let marker = m.u32(candidate + 4)?;
        m.set_u32(out + 0x34, target.raw())?;
        m.set_u32(out + 0x38, marker)?;
        if matches!(kind, objects::BIPED | objects::VEHICLE) {
            let player = m.u32(body.addr + 0x13c)?;
            m.set_u32(out + 0x3c, player)?;
        }
        m.set_f32(out + 0x50, primary)?;
        m.set_f32(out + 0x54, secondary)
    })?;
    Ok(())
}
