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
