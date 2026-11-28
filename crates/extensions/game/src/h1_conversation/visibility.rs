//! The first cast member scores players through the BSP and a sight ray.

use super::*;

const ACTIVE_BSP: u32 = 0x004e_0348;
pub(super) const COLLISION_TEST_VECTOR: u32 = 0x0016_97c0;
const SIGHT_FLAGS: u32 = 0x27;

pub(super) fn in_pvs(m: &dyn GuestMemory, from: i16, to: i16) -> Result<bool, MemFault> {
    if from < 0 || to < 0 {
        return Ok(true);
    }
    let bsp = m.u32(ACTIVE_BSP)?;
    if bsp == 0 {
        return Ok(true);
    }
    let n = m.u32(bsp + 0x9c)?;
    let (from, to) = (from as u32, to as u32);
    if from >= n || to >= n {
        return Ok(true);
    }
    let words = n.div_ceil(32);
    let rows = m.u32(bsp + 0x58)?;
    if rows == 0 || Some(m.u32(bsp + 0x54)?) != n.checked_mul(words * 4) {
        return Ok(true);
    }
    let word = m.u32(rows + (from * words + to / 32) * 4)?;
    Ok(word & (1 << (to % 32)) != 0)
}

pub(super) async fn head(cx: &Cx, scratch: u32, unit: u32) -> Result<[f32; 3], EntryError> {
    // The object marker transform is 0x70 bytes. Its world translation is
    // at +0x60, as in the stock objects_can_see_object path (0x0029f7a0).
    cx.call(
        GuestCall::new(0x000b_8d30)
            .ebx(scratch)
            .ecx(0)
            .args([unit, HEAD_MARKER, 1]),
    )
    .await?;
    Ok(cx.read(|m| m.vec3(scratch + 0x60))?)
}

pub(super) async fn sight_bonus(
    cx: &Cx,
    scratch: u32,
    from: [f32; 3],
    to: [f32; 3],
    distance: f32,
) -> Result<f32, EntryError> {
    // The source awards the bonus within three world units even when
    // occluded. For other eligible players it requires a clear ray.
    if distance < 3.0 {
        return Ok(0.5);
    }
    cx.write(|m| {
        for i in 0..3 {
            m.set_f32(scratch + i as u32 * 4, from[i])?;
            m.set_f32(scratch + 12 + i as u32 * 4, to[i] - from[i])?;
        }
        Ok::<_, MemFault>(())
    })?;
    let result = cx
        .call(GuestCall::new(COLLISION_TEST_VECTOR).args([
            SIGHT_FLAGS,
            scratch,
            scratch + 12,
            NONE,
            NONE,
            scratch + 0x20,
        ]))
        .await?;
    Ok(if result.eax as u8 == 0 { 0.5 } else { 0.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{EntryFuture, Regs, testing::run};
    use guest_layout::testing::FakeGuest;

    #[test]
    fn casting_reads_a_padded_directional_bsp_row() {
        let mut m = FakeGuest::new();
        let bsp = 0x1000;
        let rows = 0x2000;
        m.put_u32(ACTIVE_BSP, bsp)
            .put_u32(bsp + 0x9c, 33)
            .put_u32(bsp + 0x54, 33 * 8)
            .put_u32(bsp + 0x58, rows)
            .put_u32(rows + 4, 1);
        assert!(in_pvs(&m, 0, 32).unwrap());
        assert!(!in_pvs(&m, 32, 0).unwrap());
        assert!(in_pvs(&m, -1, 32).unwrap());
    }

    #[test]
    fn cast_visibility_bonus_uses_a_clear_ray_or_three_unit_range() {
        const SCRATCH: u32 = 0x1000;
        for (distance, blocked, expected) in
            [(2.0, true, 0.5f32), (5.0, true, 0.0), (5.0, false, 0.5)]
        {
            let mut guest = FakeGuest::new();
            guest.map(SCRATCH, 0x100);
            let entry = move |cx| -> EntryFuture {
                Box::pin(async move {
                    let bonus = sight_bonus(
                        &cx,
                        SCRATCH,
                        [2.0, 3.0, 4.0],
                        [2.0 + distance, 3.0, 4.0],
                        distance,
                    )
                    .await?;
                    Ok(Ret::new(bonus.to_bits(), 0))
                })
            };
            let result = run(&entry, guest, Regs::default(), |call, m| {
                assert_eq!(call.target, COLLISION_TEST_VECTOR);
                assert_eq!(
                    call.args,
                    [
                        SIGHT_FLAGS,
                        SCRATCH,
                        SCRATCH + 12,
                        NONE,
                        NONE,
                        SCRATCH + 0x20
                    ]
                );
                assert_eq!(m.vec3(SCRATCH).unwrap(), [2.0, 3.0, 4.0]);
                assert_eq!(m.vec3(SCRATCH + 12).unwrap(), [distance, 0.0, 0.0]);
                Regs {
                    eax: u32::from(blocked),
                    ..Regs::default()
                }
            });
            assert_eq!(result.result, Ok(Ret::new(expected.to_bits(), 0)));
            assert_eq!(result.calls.len(), usize::from(distance >= 3.0));
        }
    }
}
