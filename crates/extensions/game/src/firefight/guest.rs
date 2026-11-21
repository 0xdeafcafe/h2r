//! The AI switch on the guest: the two AI globals bytes `ai_update`
//! (`0x1c83c0`) gates on (docs/game/ai-networking.md, "Guest boundary").

use engine::{GuestMemory, MemFault, points};

use super::host::AiGlobals;

/// `on` (globals `+0`, the script's `ai` switch) and `initialised` (`+1`).
/// `None` before the globals exist.
pub fn ai_globals(m: &dyn GuestMemory) -> Result<Option<AiGlobals>, MemFault> {
    let g = m.u32(points::AI_GLOBALS.base)?;
    if g == 0 {
        return Ok(None);
    }
    Ok(Some(AiGlobals {
        on: m.u8(g)? != 0,
        initialised: m.u8(g.wrapping_add(1))? != 0,
    }))
}

/// Set the on byte, as the script's `ai` switch does (`0x2a4d31` stores its boolean
/// at `+0`). Writes nothing and returns false when the globals do not
/// exist yet or the byte is already set.
pub fn switch_ai_on(m: &mut dyn GuestMemory) -> Result<bool, MemFault> {
    let g = m.u32(points::AI_GLOBALS.base)?;
    if g == 0 || m.u8(g)? != 0 {
        return Ok(false);
    }
    m.write(g, &[1])?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const GLOBALS: u32 = 0x4000;

    #[test]
    fn the_switch_sets_only_the_on_byte_and_only_when_off() {
        let mut m = FakeGuest::new();
        m.put_u32(points::AI_GLOBALS.base, 0);
        assert_eq!(ai_globals(&m).unwrap(), None);
        assert!(!switch_ai_on(&mut m).unwrap());

        m.put_u32(points::AI_GLOBALS.base, GLOBALS)
            .put_u8(GLOBALS, 0)
            .put_u8(GLOBALS + 1, 1)
            .put_u8(GLOBALS + 2, 7);
        assert_eq!(
            ai_globals(&m).unwrap(),
            Some(AiGlobals {
                on: false,
                initialised: true
            })
        );
        assert!(switch_ai_on(&mut m).unwrap());
        assert_eq!(
            ai_globals(&m).unwrap(),
            Some(AiGlobals {
                on: true,
                initialised: true
            })
        );
        assert_eq!(m.u8(GLOBALS + 2).unwrap(), 7);
        assert!(!switch_ai_on(&mut m).unwrap());
    }
}
