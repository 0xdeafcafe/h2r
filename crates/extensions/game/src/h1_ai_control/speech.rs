//! Order speech uses the same event system as native AI vocalizations.

use guest_layout::{entry::{Cx, EntryError, GuestCall}, memory::MemFault};

use super::{NONE, actors};

const VOCALIZE: u32 = 0x0020_ba60;
const ADVANCE: u32 = 111;
const RETREAT: u32 = 114;

pub(super) async fn maneuver(cx: &Cx, unit: u32, advance: bool) -> Result<(), EntryError> {
    say(cx, unit, if advance { ADVANCE } else { RETREAT }).await
}

async fn say(cx: &Cx, unit: u32, event: u32) -> Result<(), EntryError> {
    if unit != NONE {
        cx.call(GuestCall::new(VOCALIZE).args([event, unit, NONE, NONE, NONE, 0]))
            .await?;
    }
    Ok(())
}

fn event(utterance: &[u8]) -> u32 {
    if utterance.eq_ignore_ascii_case(b"advance") {
        ADVANCE
    } else {
        RETREAT
    }
}

pub(super) async fn migrate(cx: &Cx, from: u32, to: u32, utterance: u32) -> Result<(), EntryError> {
    let voice = cx.read(|m| {
        if matches!(utterance, 0 | NONE) {
            RETREAT
        } else {
            // A longer unmatched string takes the retreat branch too.
            event(&m.cstr(utterance, 7).unwrap_or_default())
        }
    });
    let moves = cx.read(|m| {
        let mut moves = Vec::new();
        for actor in actors::selected(m, from)? {
            if let Some(squad) = actors::migrate_to(m, actor, to)?
                && squad != actor.squad
            {
                moves.push((actor, squad));
            }
        }
        Ok::<_, MemFault>(moves)
    })?;
    for (actor, squad) in moves {
        actors::move_actor(cx, actor, squad).await?;
        say(cx, actor.unit, voice).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_selects_the_matching_order_vocalization() {
        assert_eq!(event(b"advance"), ADVANCE);
        assert_eq!(event(b"Advance"), ADVANCE);
        assert_eq!(event(b"retreat"), RETREAT);
    }
}
