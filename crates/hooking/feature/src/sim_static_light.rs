//! Skip a placed light's tick update when that update would rewrite the same
//! values and leave the light linked. See docs/perf/sim-budget.md.

use engine::points;
use runtime::guest::Access;
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

use crate::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};

const TICK_RETURN: u32 = 0x000c_1489;

pub struct StaticLight;

impl Feature for StaticLight {
    fn manifest(&self) -> Manifest {
        Manifest::new("sim-static-light", "Skip unchanged placed light updates")
            .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        if b.switches().implementation("sim.static-light", true) {
            b.raw()
                .before(
                    &points::LIGHT_RECORD_UPDATE,
                    "skip unchanged placed light at the tick call",
                    |c| {
                        Ok(match skip(&c.ctx.memory(), c.cpu) {
                            Ok(Some(done)) => Flow::Return(done),
                            _ => Flow::Continue,
                        })
                    },
                )
                .last()
                .optional()
                .context_only();
        }
        Ok(())
    }
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

// Restrict to exact positive zero and positive normal inputs. This makes
// the fixed-point check independent of signed-zero, DAZ/FTZ and NaN rules.
fn nonnegative_normal_or_zero(bits: u32) -> bool {
    bits == 0 || (f32::MIN_POSITIVE.to_bits()..=f32::MAX.to_bits()).contains(&bits)
}

fn unchanged(light: &[u8; 0xf0], tag: &[u8; 0xd8]) -> bool {
    let flags = light[2];
    if flags & 0x17 != 0x07 // placed, live, active, untimed
        || word(light, 0x4c) != u32::MAX // object-function source
        || word(light, 0x58) != u32::MAX // marker parent
        || light[0x54..0x56] != u16::MAX.to_le_bytes() // no function
        || word(light, 0xcc) != 1.0f32.to_bits() // base scale
        || word(light, 0xd0) != 1.0f32.to_bits() // output intensity
        || word(light, 0xc8) != 0 // no parent fade
        || [0xc4, 0xcc, 0xd4].iter().any(|&offset| word(tag, offset) != 0)
    {
        return false;
    }

    // Radius = (1 - intensity) * lower + intensity * upper. With a
    // positive upper radius and a non-black colour the light stays active.
    let radius = word(tag, 0x0c);
    if !nonnegative_normal_or_zero(word(tag, 0x08))
        || radius == 0
        || !nonnegative_normal_or_zero(radius)
        || word(light, 0xc4) != radius
    {
        return false;
    }

    // The unmodulated branch copies tag colours, clamps to [0,1], and
    // clamps twice the upper brightness to [0,4]. Admit in-range values.
    let brightness = word(tag, 0x74);
    if !nonnegative_normal_or_zero(brightness)
        || brightness > 2.0f32.to_bits()
        || word(light, 0xec) != (f32::from_bits(brightness) * 2.0).to_bits()
    {
        return false;
    }
    let mut lit = false;
    for (i, tag_offset) in [0x64, 0x68, 0x6c, 0x4c, 0x50, 0x54].into_iter().enumerate() {
        let colour = word(tag, tag_offset);
        if !nonnegative_normal_or_zero(colour)
            || colour > 1.0f32.to_bits()
            || word(light, 0xd4 + 4 * i) != colour
        {
            return false;
        }
        lit |= colour != 0;
    }
    lit
}

fn skip(mem: &Guest, cpu: &mut CpuAbi) -> Result<Option<TransferAbi>, GuestFault> {
    // At this single call ESI is the light and ECX its current definition.
    // The two constructors of a light call the same body with a different ABI.
    if mem.u32(cpu.esp)? != TICK_RETURN {
        return Ok(None);
    }
    let light = mem.read::<0xf0>(cpu.esi)?;
    let tag = mem.read::<0xd8>(cpu.ecx)?;
    if !unchanged(&light, &tag) {
        return Ok(None);
    }
    // With three empty modulation blocks c0b00 computes and discards the
    // phase. Its per-instance path would fault for a zero tick rate.
    let clock = mem.read::<12>(mem.u32(points::GAME_TIME.base)?)?;
    if i16::from_le_bytes([clock[2], clock[3]]) <= 0 {
        return Ok(None);
    }
    // Leave any protection/stack faults to the original instructions.
    mem.check(cpu.esi.wrapping_add(0xc4), 0x2c, Access::Write, 1)?;
    mem.check(cpu.esp.wrapping_sub(0x8c), 0x90, Access::Write, 1)?;
    mem.u32(cpu.esp.wrapping_add(4))?;

    // RET 4. c1489 tests [esi+2] immediately; its next AL read follows a
    // new EAX load. The untimed, unowned path consumes no FP result.
    Ok(Some(mem.ret(cpu, 4)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Base, Registry, Switches};
    use runtime::guest::Permissions;

    #[test]
    fn registered_light_feature_places_one_switchable_hook() {
        for (switch, present) in [
            ("", true),
            ("-impl.sim.static-light", false),
            ("impl.sim.static-light", true),
        ] {
            let mut registry = Registry::new(Switches::parse(switch).unwrap());
            registry.add(&StaticLight).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert_eq!(
                plan.chains.contains_key(&points::LIGHT_RECORD_UPDATE.base),
                present
            );
        }
    }

    fn fixed_point() -> ([u8; 0xf0], [u8; 0xd8]) {
        let mut light = [0; 0xf0];
        let mut tag = [0; 0xd8];
        light[2] = 0x0f;
        light[0x4c..0x50].fill(0xff);
        light[0x54..0x56].fill(0xff);
        light[0x58..0x5c].fill(0xff);
        for offset in [0xc4, 0xcc, 0xd0, 0xd4, 0xec] {
            light[offset..offset + 4].copy_from_slice(&1.0f32.to_le_bytes());
        }
        for offset in [0x08, 0x0c, 0x64] {
            tag[offset..offset + 4].copy_from_slice(&1.0f32.to_le_bytes());
        }
        tag[0x74..0x78].copy_from_slice(&0.5f32.to_le_bytes());
        (light, tag)
    }

    #[test]
    fn require_all_program_inputs_and_outputs_at_a_fixed_point() {
        let (light, tag) = fixed_point();
        assert!(unchanged(&light, &tag));
        for offset in [
            2, 0x4c, 0x54, 0x58, 0xc4, 0xc8, 0xcc, 0xd0, 0xd4, 0xd8, 0xdc, 0xe0, 0xe4, 0xe8, 0xec,
        ] {
            let mut changed = light;
            changed[offset] ^= 1;
            assert!(!unchanged(&changed, &tag), "light +{offset:x}");
        }
        for bit in [2, 4, 0x10] {
            let mut changed = light;
            changed[2] ^= bit;
            assert!(!unchanged(&changed, &tag));
        }
        for offset in [0x0c, 0x4c, 0x64, 0x74, 0xc4, 0xcc, 0xd4] {
            let mut changed = tag;
            changed[offset] ^= 1;
            assert!(!unchanged(&light, &changed), "tag +{offset:x}");
        }
        let mut dark = light;
        dark[0xd4..0xd8].fill(0);
        let mut dark_tag = tag;
        dark_tag[0x64..0x68].fill(0);
        assert!(!unchanged(&dark, &dark_tag));
        for value in [f32::NAN, -0.0, f32::from_bits(1), f32::INFINITY] {
            let mut invalid = tag;
            invalid[8..12].copy_from_slice(&value.to_le_bytes());
            assert!(!unchanged(&light, &invalid));
        }
    }

    #[test]
    fn tick_returns_over_one_argument_and_other_calls_and_faults_fall_back() {
        let mut mem = Guest::new();
        let mut cpu = CpuAbi {
            esp: 0x1800,
            esi: 0x2000,
            ecx: 0x3000,
            ..Default::default()
        };
        mem.map_zeroed(0x1000, 0x4000, Permissions::RW).unwrap();
        mem.map_zeroed(points::GAME_TIME.base & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        let (light, tag) = fixed_point();
        mem.write(cpu.esi, light).unwrap();
        mem.write(cpu.ecx, tag).unwrap();

        mem.set_u32(points::GAME_TIME.base, 0x4000).unwrap();
        mem.set_u16(0x4002, 30).unwrap();
        mem.set_u32(cpu.esp, 0x000c_04f8).unwrap(); // constructor
        assert!(skip(&mem, &mut cpu).unwrap().is_none());
        mem.set_u32(cpu.esp, TICK_RETURN).unwrap();
        mem.set_u16(0x4002, 0).unwrap();
        assert!(skip(&mem, &mut cpu).unwrap().is_none());
        mem.set_u32(points::GAME_TIME.base, 0x5000).unwrap(); // unmapped
        assert!(skip(&mem, &mut cpu).is_err());
        assert_eq!(cpu.esp, 0x1800);

        mem.set_u32(points::GAME_TIME.base, 0x4000).unwrap();
        mem.set_u16(0x4002, 30).unwrap();
        assert_eq!(
            skip(&mem, &mut cpu).unwrap().unwrap().target_pc,
            TICK_RETURN
        );
        assert_eq!(cpu.esp, 0x1808);
        assert_eq!(mem.read::<0xf0>(cpu.esi).unwrap(), light);
    }
}
