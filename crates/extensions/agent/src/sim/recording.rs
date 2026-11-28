//! `recording.play`'s reads of the loaded scenario: a recorded animation
//! found by name in the block at `scnr +0x110` and the player's flags for
//! how it ends (docs/runtime/agent-api.md, "recording.play"; the player
//! and its arguments are in [`super::calls::addr::RECORDING_PLAYER`]).

use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;

/// The block in the scenario: its count, then its elements' address.
const BLOCK: u32 = 0x110;
/// One element: `defs/recorded_animation.txt`.
const ELEMENT: u32 = 0x34;
/// The name, a 32-byte string at the element's start.
const NAME: usize = 32;
/// A sanity bound on the count, not the engine's.
const MAX: u32 = 0x8000;

fn fault(e: guest_layout::MemFault) -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        format!("guest memory at {:#x} faulted", e.addr),
    )
}

/// The play flags the three script calls pass: `recording_play` 0,
/// `recording_play_and_delete` 8, `recording_play_and_hover` `0x10`.
pub fn flags(mode: Option<&str>) -> Result<u32, RpcError> {
    match mode {
        None | Some("play") => Ok(0),
        Some("delete") => Ok(8),
        Some("hover") => Ok(0x10),
        Some(other) => Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("mode: play, delete or hover, not {other:?}"),
        )),
    }
}

/// The recording called `name` (without case): its index in the block and
/// its name as the scenario spells it. A refusal lists what the loaded
/// scenario has.
pub fn find(m: &dyn GuestMemory, name: &str) -> Result<(u16, String), RpcError> {
    let scenario = m.u32(guest_layout::points::SCENARIO.base).map_err(fault)?;
    if scenario == 0 {
        return Err(RpcError::new(code::UNAVAILABLE, "no scenario is loaded"));
    }
    let count = m.u32(scenario + BLOCK).map_err(fault)?;
    let at = m.u32(scenario + BLOCK + 4).map_err(fault)?;
    if count > MAX || (count > 0 && at == 0) {
        return Err(RpcError::new(
            code::UNAVAILABLE,
            "the scenario's recorded animations block is not valid",
        ));
    }
    let mut names = Vec::new();
    for i in 0..count {
        let mut bytes = [0u8; NAME];
        m.read(at.wrapping_add(i * ELEMENT), &mut bytes)
            .map_err(fault)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(NAME);
        let n = String::from_utf8_lossy(&bytes[..end]).into_owned();
        if n.eq_ignore_ascii_case(name) {
            let index = u16::try_from(i)
                .map_err(|_| RpcError::new(code::UNAVAILABLE, "recording index past 16 bits"))?;
            return Ok((index, n));
        }
        names.push(n);
    }
    let have = if names.is_empty() {
        "the loaded scenario has no recorded animations".to_owned()
    } else {
        format!("it has: {}", names.join(", "))
    };
    Err(RpcError::new(
        code::INVALID_PARAMS,
        format!("recording: no recording {name:?} in the loaded scenario ({have})"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    fn scenario(names: &[&str]) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_u32(guest_layout::points::SCENARIO.base, 0x1000);
        g.put_u32(0x1000 + BLOCK, names.len() as u32);
        g.put_u32(0x1000 + BLOCK + 4, 0x2000);
        g.map(0x2000, 0x400);
        for (i, n) in names.iter().enumerate() {
            g.put(0x2000 + i as u32 * ELEMENT, n.as_bytes());
        }
        g
    }

    #[test]
    fn a_recording_is_found_by_name_without_case_and_gives_its_index() {
        let g = scenario(&["arrive", "Leave"]);
        assert_eq!(find(&g, "LEAVE").unwrap(), (1, "Leave".to_owned()));
        assert_eq!(find(&g, "arrive").unwrap().0, 0);
    }

    #[test]
    fn an_unknown_recording_lists_what_the_scenario_has() {
        let e = find(&scenario(&["arrive"]), "depart").unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
        assert!(e.message.contains("it has: arrive"), "{}", e.message);
        let e = find(&scenario(&[]), "depart").unwrap_err();
        assert!(e.message.contains("no recorded animations"), "{}", e.message);
    }

    #[test]
    fn the_modes_are_the_flags_the_three_script_calls_push() {
        assert_eq!(flags(None).unwrap(), 0);
        assert_eq!(flags(Some("delete")).unwrap(), 8);
        assert_eq!(flags(Some("hover")).unwrap(), 0x10);
        assert!(flags(Some("kill")).is_err());
    }
}
