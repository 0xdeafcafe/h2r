//! Publish the unit-control packet. This entry also serves the AI and the
//! recording player. In particular the packet's weapon selectors only
//! replace a matching pair, and the grenade sentinel preserves the unit's
//! previous request.
use super::*;

struct UnitControl {
    flags: u32,
    animation: u8,
    weapon_selection: u32,
    grenade: u16,
    zoom: u8,
    actions: u32,
    throttle: Vector,
    triggers: [u32; 2],
    facing: Vector,
    aim: Vector,
    look: Vector,
    gaze: Vector,
    aim_assist: [u8; 36],
}

impl UnitControl {
    fn gather(m: &impl Memory, packet: u32) -> Result<Self> {
        Ok(Self {
            flags: m.u32(packet)?,
            animation: m.u8(at(packet, 4))?,
            weapon_selection: m.u32(at(packet, 6))?,
            grenade: m.u16(at(packet, 0xa))?,
            zoom: m.u8(at(packet, 0xc))?,
            actions: m.u32(at(packet, 0x10))?,
            throttle: vector(m, at(packet, 0x14))?,
            triggers: [m.u32(at(packet, 0x20))?, m.u32(at(packet, 0x24))?],
            facing: vector(m, at(packet, 0x28))?,
            aim: vector(m, at(packet, 0x34))?,
            look: vector(m, at(packet, 0x40))?,
            gaze: vector(m, at(packet, 0x4c))?,
            aim_assist: m.load(at(packet, 0x58))?,
        })
    }

    fn publish(&self, m: &mut impl Memory, body: u32) -> Result<()> {
        store_vector(m, at(body, Unit::THROTTLE.offset), self.throttle)?;
        m.set_u32(at(body, 0x1c0), self.triggers[0])?;
        m.set_u32(at(body, 0x1c4), self.triggers[1])?;
        m.set_u8(at(body, 0x1bc), self.animation)?;
        if self.weapon_selection as u16 == m.u16(at(body, 0x214))? {
            m.set_u32(at(body, 0x214), self.weapon_selection)?;
        }
        if self.grenade != u16::MAX {
            m.set_u8(at(body, 0x23d), self.grenade as u8)?;
        }
        m.set_u8(at(body, 0x241), self.zoom)?;
        m.set_u32(at(body, 0x148), self.actions)?;
        store_vector(m, at(body, Unit::DESIRED_LOOK.offset), self.look)?;
        store_vector(m, at(body, Unit::DESIRED_AIM.offset), self.aim)?;
        store_vector(m, at(body, Unit::DESIRED_FACING.offset), self.facing)?;
        m.set_u32(at(body, 0x14c), self.flags)?;
        store_vector(m, at(body, 0x1a4), self.gaze)?;
        for (i, word) in self.aim_assist.chunks_exact(4).enumerate() {
            m.write_bytes(at(body, 0x1c8 + i as u32 * 4), word, 1)?;
        }
        Ok(())
    }
}

pub fn apply(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Step {
    if let Some(why) = super::super::guard(c, DIRECTION_FLAG) {
        return decline(pc, why);
    }
    segment::<40, _>(
        c,
        g,
        pc,
        points::UNIT_APPLY_CONTROL.base..0xc6ee2,
        &[],
        |c, m, _| {
            let packet = c.ecx;
            let table = data(m, points::OBJECTS.base)?;
            push(c, m, c.esi)?;
            let body = object(m, table, c.eax)?;
            push(c, m, c.edi)?;
            disjoint(m, (packet, 0x7c), (at(body, 0x148), 0xfa))?;
            let control = UnitControl::gather(m, packet)?;
            control.publish(m, body)?;
            crate::sim_objects::alu::cmp16(c, u32::from(control.grenade), u32::from(u16::MAX));
            c.eax = body;
            c.ecx = 0;
            c.edx = control.gaze[2];
            c.edi = pop(c, m)?;
            c.esi = pop(c, m)?;
            m.ret(c, 0)
        },
    )
}
