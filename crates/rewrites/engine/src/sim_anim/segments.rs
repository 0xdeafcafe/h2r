//! The unit aim and first-person filter updates, resumed at their original
//! call sites. Both keep their frames in guest memory across callouts.
use super::filter;
use crate::sim_native::{SegmentMemory, plan::segment};
use guest_layout::points;
use crate::sim_physics;
use runtime::{CpuAbi, GuestFault, TransferAbi};

pub const UNIT_AIM_SITES: &[u32] = sim_physics::unit_aim::SITES;
pub const FIRST_PERSON_FILTER_SITES: &[u32] = sim_physics::first_person::FILTER_SITES;

/// Joint-space aim, including its x87 angle and velocity calculations.
pub fn unit_aim(
    cpu: &mut CpuAbi,
    mem: &mut impl SegmentMemory,
    pc: u32,
) -> Result<Option<TransferAbi>, GuestFault> {
    segment::<128, _>(
        cpu,
        mem,
        pc,
        points::UNIT_AIM_UPDATE.base..0xc81f3,
        UNIT_AIM_SITES,
        |cpu, plan, site| sim_physics::unit_aim::aim(cpu, plan, site),
    )
}

/// Seat filters and both weapon slots. Space for the spills of four
/// adjacent filter kernels is needed when all four calls run inline.
pub fn first_person_filters(
    cpu: &mut CpuAbi,
    mem: &mut impl SegmentMemory,
    pc: u32,
) -> Result<Option<TransferAbi>, GuestFault> {
    let mut counts = filter::Stats::default();
    let transfer = segment::<128, _>(
        cpu,
        mem,
        pc,
        points::FIRST_PERSON_FILTER_UPDATE.base..0x16760c,
        FIRST_PERSON_FILTER_SITES,
        |cpu, plan, site| {
            if filter::enabled() {
                plan.protect_inline_text(filter::FILTER, filter::FILTER_END);
            }
            sim_physics::first_person::filters_counted(cpu, plan, site, &mut counts)
        },
    )?;
    if transfer.is_some() {
        counts.record();
    }
    Ok(transfer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};

    #[test]
    fn directed_aim_transcendental_discards_the_projected_directions() {
        let mut ram = Guest::new();
        ram.map(0, 0x500000, 0, Permissions::RW).unwrap();
        let sp = 0x20080;
        let current = 0x22000;
        let target = 0x23000;
        ram.set_u32(current, 1.0f32.to_bits()).unwrap();
        ram.set_u32(target + 4, 1.0f32.to_bits()).unwrap();
        for (offset, value) in [(4, 0.25f32.to_bits()), (8, 0x24000), (12, 0x25000)] {
            ram.set_u32(sp + offset, value).unwrap();
        }
        let untouched = [0x75; 160];
        ram.write_bytes(sp - 160, &untouched, 1).unwrap();
        let mut cpu = CpuAbi {
            esp: sp,
            edi: current,
            eax: target,
            ..Default::default()
        };
        cpu.x87.control |= 0x400;
        let before = cpu;
        assert_eq!(
            unit_aim(&mut cpu, &mut ram, points::UNIT_AIM_UPDATE.base).unwrap(),
            None
        );
        assert_eq!(cpu, before);
        let mut frame = [0; 160];
        ram.read_bytes(sp - 160, &mut frame, 1).unwrap();
        assert_eq!(frame, untouched);
        // The same mapped inputs reach a transfer with supported rounding.
        cpu.x87.control &= !0xc00;
        assert!(
            unit_aim(&mut cpu, &mut ram, points::UNIT_AIM_UPDATE.base)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_late_fault_discards_the_aim_arguments_and_the_filter_prologue() {
        type Body = fn(&mut CpuAbi, &mut Guest, u32) -> Result<Option<TransferAbi>, GuestFault>;
        // Aim pushes two arguments above the mapping boundary. The filter
        // frame fits, then its read of the seat-array global faults.
        for (body, text, site, sp) in [
            (
                unit_aim as Body,
                points::UNIT_AIM_UPDATE.base..0xc81f3,
                0xc7e2d,
                0x20008,
            ),
            (
                first_person_filters as Body,
                points::FIRST_PERSON_FILTER_UPDATE.base..0x16760c,
                points::FIRST_PERSON_FILTER_UPDATE.base,
                0x20040,
            ),
        ] {
            let mut ram = Guest::new();
            ram.map(text.start, text.end - text.start, 0x4000, Permissions::RX)
                .unwrap();
            ram.map(0x20000, 0x100, 0, Permissions::RW).unwrap();
            let stack = [0xa7; 0x100];
            ram.write_bytes(0x20000, &stack, 1).unwrap();
            let mut cpu = CpuAbi {
                esp: sp,
                ebx: 0x12345678,
                ebp: 0x87654321,
                ..Default::default()
            };
            let before = cpu;
            assert_eq!(body(&mut cpu, &mut ram, site).unwrap(), None);
            assert_eq!(cpu, before);
            let mut after = [0; 0x100];
            ram.read_bytes(0x20000, &mut after, 1).unwrap();
            assert_eq!(after, stack);
        }
    }
}
