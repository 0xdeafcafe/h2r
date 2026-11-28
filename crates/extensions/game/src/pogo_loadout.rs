//! The Pogo Stick as Superbounce's secondary weapon
//! (docs/game/pogo-stick.md, "The loadout").
//!
//! The spawn's weapon pass (`0x19ef40`) maps the variant's primary and
//! secondary weapon bytes to tags through one routine
//! ([`points::STARTING_WEAPON_PICK`], `0x19eda0`), then makes an object of
//! each tag and adds it to the unit (`0xb7930`, `0xb7b40`, then
//! `unit_add_weapon` `0xcd0c0` in mode 1). The byte can only name a slot of
//! the multiplayer globals' weapon list, and the stick is a tag of ours the
//! map rewrite adds (`content_kit`), so the hook here answers the routine's
//! two calls for the secondary with the stick's tag, whatever the byte
//! says. The pass then treats it as any weapon.
//!
//! The choice is the simulation's: every machine in the game, and a film's
//! playback, must make the same one. It is read from guest memory (the
//! variant's extension block and the script's option bits, which a guest
//! takes from its host) and from the map's bytes (the tag's datum, which
//! the rewrite places the same way everywhere), and from nothing else. So
//! the feature is not a presentation one.

use guest_layout::GuestMemory;
use guest_layout::handle::DatumHandle;
use guest_layout::points;
use hooks::Flow;
use pogo::Style;
use pogo::placed::Placed;

use crate::superbounce_options::Options;

/// Where the weapon pass's two calls for the secondary weapon return to:
/// the one made first when the primary is random (value 2), and the one
/// made after the primary otherwise. The primary's are `0x19f056` and
/// `0x19f063`; the routine's own random retry returns to `0x19ee18`.
pub const SECONDARY_RETURNS: [u32; 2] = [0x0019_f042, 0x0019_f07b];
/// The argument bytes the routine pops (`ret 0xc`): the profile's weapon,
/// the variant's value, the other weapon's tag.
pub const ARG_BYTES: u32 = 12;
/// "No weapon": the caller skips it (`0x19f085`).
pub const NO_TAG: u32 = u32::MAX;

/// Whether a call of the routine returning to `ret` picks the secondary
/// weapon.
pub fn picks_secondary(ret: u32) -> bool {
    SECONDARY_RETURNS.contains(&ret)
}

/// The tag the secondary weapon is, when the stick takes it: the call
/// returns to a secondary site, the game is Superbounce, the variant has
/// the stick on and the map carries the kit. `None` leaves the routine to
/// answer as retail does.
pub fn secondary(
    ret: u32,
    superbounce: bool,
    opts: Options,
    placed: Option<Placed>,
) -> Option<u32> {
    if !picks_secondary(ret) || !superbounce || !opts.pogo {
        return None;
    }
    let tag = *placed?.weapons.get(opts.pogo_style.index())?;
    (tag != NO_TAG).then_some(tag)
}

/// [`secondary`] for the call whose stack is at `esp`, from the game in
/// `m`. A call for the primary reads the return address and nothing more.
pub fn decide(m: &dyn GuestMemory, esp: u32) -> Option<u32> {
    let ret = m.u32(esp).ok()?;
    if !picks_secondary(ret) {
        return None;
    }
    if !crate::variant_ext::in_game(&crate::layout_glue::ReadGm(m)).superbounce() {
        return None;
    }
    let map = crate::health::loaded_map_name(m).ok()?;
    secondary(
        ret,
        true,
        crate::superbounce_options::read(m),
        pogo::placed::get(&map),
    )
}

/// Which stick a weapon's definition tag is, if it is one of the map's.
pub fn stick_style(definition: u32, placed: &Placed) -> Option<Style> {
    if definition == NO_TAG {
        return None;
    }
    Style::ALL
        .into_iter()
        .find(|s| placed.weapons.get(s.index()) == Some(&definition))
}

/// The stick the unit at `unit_body` holds now: its held weapon's object,
/// that object's definition, and which of the map's sticks it is. `None`
/// when the unit holds another weapon, or none.
pub fn held_style(m: &dyn GuestMemory, unit_body: u32, placed: &Placed) -> Option<Style> {
    let weapon = guest_layout::Unit::at(unit_body).held_weapon(m).ok()??;
    let (kind, body) = guest_layout::view::objects::get(m, DatumHandle(weapon.raw())).ok()??;
    if kind != guest_layout::view::objects::WEAPON {
        return None;
    }
    let definition = body.definition(m).ok()??;
    stick_style(definition.raw(), placed)
}

/// The hook on the routine: a secondary pick in a Superbounce game with
/// the stick on returns the stick's tag in EAX in place of the routine.
pub fn install_hooks(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    b.raw().before(
        &points::STARTING_WEAPON_PICK,
        "pogo: the stick is the secondary weapon",
        |c| {
            let tag = {
                let guest = c.ctx.runtime().memory();
                let m: &dyn GuestMemory = &*guest;
                decide(m, c.cpu.esp)
            };
            let Some(tag) = tag else {
                return Ok(Flow::Continue);
            };
            c.cpu.eax = tag;
            c.ret(ARG_BYTES).map(Flow::Return)
        },
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::{fake, objects};
    use guest_layout::{ObjectHeader, Unit};

    const STICKS: [u32; 3] = [0xe1a2_0b01, 0xe1a3_0b02, 0xe1a4_0b03];

    fn placed() -> Placed {
        Placed { weapons: STICKS }
    }

    fn opts(pogo: bool, pogo_style: Style) -> Options {
        Options {
            pogo,
            pogo_style,
            ..Options::DEFAULT
        }
    }

    /// The stick answers the two secondary calls only, in a Superbounce
    /// game with the option on, on a map that has the kit, as the style
    /// the variant picks.
    #[test]
    fn the_stick_takes_the_secondary_slot_only() {
        for ret in SECONDARY_RETURNS {
            for style in Style::ALL {
                assert_eq!(
                    secondary(ret, true, opts(true, style), Some(placed())),
                    Some(STICKS[style.index()])
                );
            }
            assert_eq!(
                secondary(ret, true, Options::DEFAULT, Some(placed())),
                Some(STICKS[0])
            );
            // Another game type, the option off, a map without the kit.
            assert_eq!(
                secondary(ret, false, Options::DEFAULT, Some(placed())),
                None
            );
            assert_eq!(
                secondary(ret, true, opts(false, Style::Unsc), Some(placed())),
                None
            );
            assert_eq!(secondary(ret, true, Options::DEFAULT, None), None);
        }
        // The primary's two calls, the random retry, and any other caller.
        for ret in [0x0019_f056, 0x0019_f063, 0x0019_ee18, 0, 0x0019_f043] {
            assert!(!picks_secondary(ret));
            assert_eq!(secondary(ret, true, Options::DEFAULT, Some(placed())), None);
        }
    }

    /// A style the map's kit did not place is no weapon of ours: retail
    /// answers.
    #[test]
    fn a_style_the_map_lacks_leaves_retail() {
        let p = Placed {
            weapons: [STICKS[0], NO_TAG, STICKS[2]],
        };
        assert_eq!(
            secondary(
                SECONDARY_RETURNS[0],
                true,
                opts(true, Style::Covenant),
                Some(p)
            ),
            None
        );
        let p = Placed {
            weapons: [STICKS[0], NO_TAG, STICKS[2]],
        };
        assert_eq!(
            secondary(
                SECONDARY_RETURNS[0],
                true,
                opts(true, Style::Forerunner),
                Some(p)
            ),
            Some(STICKS[2])
        );
    }

    /// A call for the primary, or with an unreadable stack, is decided on
    /// the return address alone: nothing else of the game is read.
    #[test]
    fn a_primary_call_reads_only_the_return_address() {
        const STACK: u32 = 0x1000;
        let mut m = FakeGuest::new();
        m.put_u32(STACK, 0x0019_f063);
        assert_eq!(decide(&m, STACK), None);
        m.put_u32(STACK, 0x0019_ee18);
        assert_eq!(decide(&m, STACK), None);
        assert_eq!(decide(&FakeGuest::new(), STACK), None);
        // A secondary call in a guest with no game in it is retail's.
        m.put_u32(STACK, SECONDARY_RETURNS[1]);
        assert_eq!(decide(&m, STACK), None);
    }

    #[test]
    fn a_definition_names_its_style() {
        let p = placed();
        for style in Style::ALL {
            assert_eq!(stick_style(STICKS[style.index()], &p), Some(style));
        }
        assert_eq!(stick_style(0xe1a2_0b00, &p), None);
        assert_eq!(stick_style(NO_TAG, &p), None);
        let none = Placed {
            weapons: [NO_TAG; 3],
        };
        assert_eq!(stick_style(NO_TAG, &none), None);
    }

    /// The held weapon's definition decides it: the stick in the held
    /// slot, not one carried in another slot, and not with no weapon held.
    #[test]
    fn the_held_weapon_is_the_stick_or_not() {
        const POOL: u32 = 0x5000;
        const UNIT: u32 = 0x7000;
        const MAGNUM_BODY: u32 = 0x8000;
        const STICK_BODY: u32 = 0x9000;
        const MAGNUM_TAG: u32 = 0xe000_0123;
        let mut m = FakeGuest::new();
        let pool = fake::datums::<objects::Header>(&mut m, POOL, 2);
        let (magnum, magnum_header) = fake::push(&mut m, &pool);
        let (stick, stick_header) = fake::push(&mut m, &pool);
        m.put_u32(points::OBJECTS.base, POOL)
            .put_u8(
                magnum_header.addr + ObjectHeader::KIND.offset,
                objects::WEAPON,
            )
            .put_u32(magnum_header.addr + ObjectHeader::BODY.offset, MAGNUM_BODY)
            .put_u32(MAGNUM_BODY, MAGNUM_TAG)
            .put_u8(
                stick_header.addr + ObjectHeader::KIND.offset,
                objects::WEAPON,
            )
            .put_u32(stick_header.addr + ObjectHeader::BODY.offset, STICK_BODY)
            .put_u32(STICK_BODY, STICKS[1])
            .put_u32(UNIT + Unit::WEAPONS.offset, magnum.raw())
            .put_u32(UNIT + Unit::WEAPONS.offset + 4, stick.raw())
            .put_u32(UNIT + Unit::WEAPONS.offset + 8, u32::MAX)
            .put_u32(UNIT + Unit::WEAPONS.offset + 12, u32::MAX);
        let p = placed();
        m.put_u8(UNIT + Unit::WEAPON_INDEX.offset, 0);
        assert_eq!(held_style(&m, UNIT, &p), None);
        m.put_u8(UNIT + Unit::WEAPON_INDEX.offset, 1);
        assert_eq!(held_style(&m, UNIT, &p), Some(Style::Covenant));
        // No weapon held, an empty slot, and a slot outside the four.
        m.put_u8(UNIT + Unit::WEAPON_INDEX.offset, 0xff);
        assert_eq!(held_style(&m, UNIT, &p), None);
        m.put_u8(UNIT + Unit::WEAPON_INDEX.offset, 2);
        assert_eq!(held_style(&m, UNIT, &p), None);
        m.put_u8(UNIT + Unit::WEAPON_INDEX.offset, 7);
        assert_eq!(held_style(&m, UNIT, &p), None);
        // A held object that is not a weapon, and one that has gone.
        m.put_u8(UNIT + Unit::WEAPON_INDEX.offset, 1).put_u8(
            stick_header.addr + ObjectHeader::KIND.offset,
            objects::BIPED,
        );
        assert_eq!(held_style(&m, UNIT, &p), None);
        m.put_u8(
            stick_header.addr + ObjectHeader::KIND.offset,
            objects::WEAPON,
        )
        .put_u16(stick_header.addr + ObjectHeader::SALT.offset, 0x7777);
        assert_eq!(held_style(&m, UNIT, &p), None);
        // An unreadable unit.
        assert_eq!(held_style(&FakeGuest::new(), UNIT, &p), None);
    }
}
