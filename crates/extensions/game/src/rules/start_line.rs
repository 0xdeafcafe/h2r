//! Round-start announcements. The game's event initializer makes a broadcast
//! event. Its post routine delivers it locally and through the simulation.
//! See docs/game/notes/2026-11-24-game-type-engine-reads.md, "The start line".

use ::rules::types::{StartLine, Type};
use guest_layout::entry::{Cx, GuestCall};
use guest_layout::{GuestMemory, points};
use std::sync::Arc;

/// The broadcast waits until every seated player's machine is in the game,
/// the condition of the retail per-player start slot. Players that have
/// left the round no longer hold the line.
pub fn players_ready(m: &dyn GuestMemory) -> bool {
    let Ok(Some(players)) = guest_layout::view::players::array(m) else {
        return false;
    };
    let Ok(listed) = players.live_iter(m, 16) else {
        return false;
    };
    let mut seated = false;
    for entry in listed {
        let Some(flags) = entry.ok().and_then(|(_, p)| p.flags(m).ok()) else {
            return false;
        };
        if flags & 2 == 0 {
            if flags & 1 == 0 {
                return false;
            }
            seated = true;
        }
    }
    seated
}

/// The authority posts the type's line at the first ready tick of the round.
pub async fn post(cx: &Cx, tick: u32, ty: Type) -> anyhow::Result<()> {
    match ty.row().start {
        StartLine::Retail(kind) => {
            // The broadcast record lives in the tick's stack frame, above both
            // callees' stacks. Zero padding that the event initializer skips.
            const SIZE: u32 = 0x24;
            let event = cx
                .entry()
                .esp
                .checked_sub(SIZE)
                .ok_or_else(|| anyhow::anyhow!("no stack space for the game event"))?;
            cx.write(|m| m.write(event, &[0; SIZE as usize]))
                .map_err(|e| anyhow::anyhow!("{}", guest_layout::entry::EntryError::from(e)))?;
            cx.call(
                GuestCall::new(points::GAME_EVENT_NEW.base)
                    .eax(event)
                    .ecx(u32::from(kind))
                    .edx(0)
                    .reserve(SIZE),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            cx.call(
                GuestCall::new(points::GAME_EVENT_POST.base)
                    .eax(event)
                    .reserve(SIZE),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        StartLine::Ours => super::world::show::start_clip(tick, ty),
        StartLine::None => {}
    }
    Ok(())
}

// The returns of the two posts in each player-ready slot, in the same order
// on both paths: Slayer, CTF/Assault, Oddball, King, Juggernaut, Territories.
// Skipping just these calls lets CTF finish its player setup. Replicated
// events, score events and the broadcast made by GAME_EVENT_POST pass.
const SEND_RETURNS: [u32; 6] = [0x2bbfec, 0x240381, 0x2bc3bc, 0x2bd2aa, 0x19228c, 0x2bf4ca];
const SHOW_RETURNS: [u32; 6] = [0x2bbff5, 0x24038a, 0x2bc3c5, 0x2bd2b3, 0x192295, 0x2bf4d3];

/// Stop a running script's carrier from posting its own line as well.
pub fn replace_carrier(
    b: &mut hooks::Builder,
    active: impl Fn() -> bool + Send + Sync + 'static,
    after: Option<&'static str>,
) {
    use hooks::Flow;
    let active = Arc::new(active);
    for (point, sites) in [
        (&points::GAME_EVENT_SEND, &SEND_RETURNS),
        (&points::GAME_EVENT_SHOW, &SHOW_RETURNS),
    ] {
        let active = Arc::clone(&active);
        let hook = b
            .raw()
            .before(point, "the script owns the round start line", move |c| {
                let caller = c.ctx.runtime().memory().u32(c.cpu.esp).ok();
                if caller.is_some_and(|pc| sites.contains(&pc)) && active() {
                    c.ret(0).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            });
        if let Some(feature) = after {
            hook.after_feature(feature);
        }
    }
}
