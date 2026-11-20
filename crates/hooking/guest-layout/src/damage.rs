//! The damage report (docs/live/live-gamestate.md section 7, "Damage-aware
//! alternative"): the record the damage report function `0x000d9640`
//! applies to its victim, once per damage instance (custom ABI: `EDI` the
//! report, the victim object datum on the stack). It adds [`DamageReport::BODY`]
//! and [`DamageReport::SHIELD`] to the victim's recent-damage accumulators
//! (body `+0x100` and `+0xf8`).

use crate::field::Datum;

crate::layout! {
    /// A damage report, as `0x000d9640` reads it at entry.
    pub struct DamageReport: 0x50 {
        /// The packed damage code: the low 6 bits the damage reporting
        /// type, the high 2 a modifier (confirmed).
        CODE code @ 0x00: u8,
        /// Report flags; bit 0 ([`DEPLETING`]) is the body-depleted
        /// transition (confirmed).
        FLAGS flags @ 0x04: u32,
        /// The damage effect (`jpt!`) tag datum (confirmed). It names the
        /// effect, not the weapon.
        EFFECT effect @ 0x08: Datum,
        /// The player the damage is credited to, or none (confirmed).
        OWNER_PLAYER owner_player @ 0x0c: Datum,
        /// The object the damage is credited to, or none (confirmed).
        OWNER_OBJECT owner_object @ 0x10: Datum,
        /// The owner's team (confirmed).
        OWNER_TEAM owner_team @ 0x14: i16,
        /// The damage's direction (confirmed).
        DIRECTION direction @ 0x18: [f32; 3],
        /// Where the damage came from (confirmed).
        ORIGIN origin @ 0x24: [f32; 3],
        /// likely: the body damage, a fraction of body vitality.
        BODY body @ 0x44: f32,
        /// likely: the shield damage, a fraction of shield vitality.
        SHIELD shield @ 0x48: f32,
        /// The distance from the origin (confirmed).
        DISTANCE distance @ 0x4c: f32,
    }
}

/// [`DamageReport::FLAGS`] bit 0: this damage depleted the body.
pub const DEPLETING: u32 = 1;

/// [`DamageReport::CODE`]'s damage reporting type (its low 6 bits).
pub const fn reporting_type(code: u8) -> u8 {
    code & 0x3f
}

/// [`DamageReport::CODE`]'s modifier (its high 2 bits).
pub const fn modifier(code: u8) -> u8 {
    code >> 6
}

/// Damage which depleted the victim. The credited killer can differ from
/// the owner when recent damage contributed to the death.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KillingDamage {
    pub code: u8,
    pub owner_player: Datum,
}

/// At PLAYER_KILLED entry, recover the depleting report from the active
/// DAMAGE_REPORT call (docs/live/live-gamestate.md section 7). For a
/// direct call outside the attribution routine, use the supplied argument.
pub fn killing_damage(
    memory: &dyn crate::GuestMemory,
    esp: u32,
    direct: Option<KillingDamage>,
) -> Option<KillingDamage> {
    use crate::points::kill_credit::{RETURN_TO_ATTRIBUTION, RETURN_TO_DAMAGE_REPORT};

    if memory.u32(esp).ok()? != RETURN_TO_ATTRIBUTION {
        return direct;
    }
    // Four kill arguments + return PC, four saved registers and 0x50
    // bytes of locals separate us from attribution's return PC.
    if memory.u32(esp.checked_add(0x74)?).ok()? != RETURN_TO_DAMAGE_REPORT {
        return None;
    }
    // The attribution prologue saved the damage reporter's EDI here.
    // Its incoming damage argument doubles as an assist-loop counter.
    let addr = memory.u32(esp.checked_add(0x1c)?).ok()?;
    if addr == 0 {
        return None;
    }
    addr.checked_add(DamageReport::OWNER_PLAYER.offset + 3)?;
    let report = DamageReport::at(addr);
    if report.flags(memory).ok()? & DEPLETING == 0 {
        return None;
    }
    Some(KillingDamage {
        code: report.code(memory).ok()?,
        owner_player: report.owner_player(memory).ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::kill_credit::{RETURN_TO_ATTRIBUTION, RETURN_TO_DAMAGE_REPORT};
    use crate::{FlatMemory, GuestMemory};

    #[test]
    fn kill_reads_the_depleting_report_even_when_recent_damage_earned_credit() {
        let esp = 0x8100;
        let report = 0x8300;
        let mut mem = FlatMemory::new(esp, 0x300);
        let rifle = Some(KillingDamage { code: 0x40 | 10, owner_player: Datum(0x18000) });
        mem.set_u32(esp, RETURN_TO_ATTRIBUTION).unwrap();
        mem.set_u32(esp + 0x74, RETURN_TO_DAMAGE_REPORT).unwrap();
        mem.set_u32(esp + 0x1c, report).unwrap();
        mem.set_u32(report + DamageReport::FLAGS.offset, DEPLETING).unwrap();
        mem.set_u8(report + DamageReport::CODE.offset, 1).unwrap();
        mem.set_u32(report + DamageReport::OWNER_PLAYER.offset, u32::MAX).unwrap();
        assert_eq!(
            killing_damage(&mem, esp, rifle),
            Some(KillingDamage { code: 1, owner_player: Datum::NONE }),
        );
        mem.set_u32(report + DamageReport::FLAGS.offset, 0).unwrap();
        assert_eq!(killing_damage(&mem, esp, rifle), None);
        mem.set_u32(esp + 0x1c, u32::MAX).unwrap();
        assert_eq!(killing_damage(&mem, esp, rifle), None);
        mem.set_u32(esp + 0x74, 0x1111).unwrap();
        mem.set_u32(esp + 0x1c, report).unwrap();
        mem.set_u32(report + DamageReport::FLAGS.offset, DEPLETING).unwrap();
        assert_eq!(killing_damage(&mem, esp, rifle), None);
        mem.set_u32(esp, 0x2222).unwrap();
        assert_eq!(killing_damage(&mem, esp, rifle), rifle);
        assert_eq!(killing_damage(&mem, esp, None), None);
    }
}
