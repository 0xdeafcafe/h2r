//! Feed the mapped action/reload button to Halo 2's script action test.

use guest_layout::points;
use engine::hs_actions::{ACTION, CAPTURE, LATCHED, PLAYER_CONTROL, UNTIL_RELEASE};
use hooks::{Builder, Flow};
use runtime::{Guest, GuestFault};

use super::converted_campaign;

const INTERACTION: u32 = 0x20;
const BUTTON_DOWN: u8 = 1;
const SCRIPT_BUTTON: u8 = 4;

fn sample(m: &mut Guest, packet: u32) -> Result<(), GuestFault> {
    let buttons = m.u8(packet + INTERACTION)?;
    if buttons & BUTTON_DOWN == 0 || !converted_campaign(m) {
        return Ok(());
    }
    let control = m.u32(PLAYER_CONTROL)?;
    // The retail evaluator (488) reads this sticky bit even on its first
    // call, before it has claimed the button. Only the reset (479) clears it.
    let flags = m.u32(control + LATCHED)?;
    m.set_u32(control + LATCHED, flags | ACTION)?;
    if (m.u32(control + CAPTURE)? | m.u32(control + UNTIL_RELEASE)?) & ACTION != 0 {
        // While claimed, route the press into the bit retail consumes at
        // 0x187cc0. Retail clears its hold threshold and marks it handled.
        // After a reset it watches this same bit until the button comes up.
        m.set_u8(
            packet + INTERACTION,
            (buttons & !BUTTON_DOWN) | SCRIPT_BUTTON,
        )?;
    }
    Ok(())
}

pub(super) fn install(b: &mut Builder) {
    // The gathered packet and the checkpointed guest control words are the
    // entire state. The original routine processes every other mapped input.
    b.raw()
        .before(
            &points::PLAYER_ACTION_TEST_UPDATE,
            "sample the mapped Halo 1 action button",
            |c| {
                sample(&mut c.ctx.memory(), c.cpu.ecx)?;
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn action_is_latched_before_poll_and_consumed_through_release() {
        let mut m = Guest::new();
        let control = 0x10000;
        let packet = 0x11000;
        for address in [
            control,
            packet,
            PLAYER_CONTROL,
            crate::health::CACHE_MAP_NAME,
        ] {
            m.map_zeroed(address & !0xfff, 0x1000, Permissions::RW)
                .unwrap();
        }
        m.set_u32(PLAYER_CONTROL, control).unwrap();
        m.write_bytes(crate::health::CACHE_MAP_NAME, b"a10\0", 1)
            .unwrap();
        m.set_u32(control + LATCHED, 0x400).unwrap();

        // The hold threshold alone isn't a press. A tap survives the next
        // neutral packet, and ordinary interaction still receives the tap.
        for (buttons, expected) in [(0x12, 0x400), (0x11, 0x401), (0x10, 0x401)] {
            m.set_u8(packet + INTERACTION, buttons).unwrap();
            sample(&mut m, packet).unwrap();
            assert_eq!(m.u32(control + LATCHED).unwrap(), expected);
            assert_eq!(m.u8(packet + INTERACTION).unwrap(), buttons);
        }

        // Values written by the action evaluator, then by the reset
        // evaluator while X is still held, then by retail on button-up.
        for (capture, held, buttons, expected) in [
            (1, 1, 0x13, 0x16),
            (0, 1, 0x13, 0x16),
            (0, 1, 0x10, 0x10),
            (0, 0, 0x11, 0x11),
        ] {
            m.set_u32(control + LATCHED, 0x400).unwrap();
            m.set_u32(control + CAPTURE, capture).unwrap();
            m.set_u32(control + UNTIL_RELEASE, held).unwrap();
            m.set_u8(packet + INTERACTION, buttons).unwrap();
            sample(&mut m, packet).unwrap();
            assert_eq!(m.u8(packet + INTERACTION).unwrap(), expected);
            assert_eq!(
                m.u32(control + LATCHED).unwrap(),
                0x400 | u32::from(buttons & BUTTON_DOWN)
            );
        }

        m.write_bytes(crate::health::CACHE_MAP_NAME, b"retail\0", 1)
            .unwrap();
        m.set_u32(control + CAPTURE, ACTION).unwrap();
        m.set_u32(control + LATCHED, 0).unwrap();
        sample(&mut m, packet).unwrap();
        assert_eq!(m.u32(control + LATCHED).unwrap(), 0);
        assert_eq!(m.u8(packet + INTERACTION).unwrap(), 0x11);
    }
}
