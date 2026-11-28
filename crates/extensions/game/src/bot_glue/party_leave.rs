//! Retail owns leave and leader-transfer UI. Give its leave checks the solo
//! answer for a party with just one human, and its picker human candidates.

use bots::{
    lobby,
    phantom::{Phantoms, session},
};
use guest_layout::{memory::GuestMemory, points::{self, party_leave as at}};
use hooks::{Builder, Flow};

fn squad(m: &dyn GuestMemory) -> Option<u32> {
    if m.u8(lobby::NETWORKING).ok()? == 0 {
        return None;
    }
    m.u32(lobby::SESSION).ok().filter(|&s| s != 0)
}

/// Matchmaking has a target session beside the party. Check both before
/// changing the leave path, so a human in either lobby keeps retail behavior.
fn solo_sessions(m: &dyn GuestMemory) -> Option<(u32, Option<u32>)> {
    let squad = squad(m)?;
    use session::at::{LOCAL_PEER, PEER_50, STATE};
    if !session::solo_host(m, squad).ok()?
        || m.u32(squad + PEER_50).ok()? != m.u32(squad + LOCAL_PEER).ok()?
    {
        return None;
    }
    if !lobby::matchmaking_cycle(m) {
        return Some((squad, None));
    }
    let target = m.u32(lobby::TARGET_SESSION).ok()?;
    if target == 0 || target == squad || m.u32(target + STATE).ok()? == 0 {
        return Some((squad, None));
    }
    session::solo_host(m, target)
        .ok()?
        .then_some((squad, Some(target)))
}

fn leave_peer_count(m: &dyn GuestMemory, caller: u32) -> Option<u32> {
    if !matches!(caller, at::LOBBY_BACK_COUNT | at::PAUSE_LEAVE_COUNT) {
        return None;
    }
    let (s, _) = solo_sessions(m)?;
    (m.u32(s + session::at::PEER_COUNT).ok()? > 1).then_some(1)
}

fn reject_candidate(m: &dyn GuestMemory, caller: u32, player: u32) -> bool {
    matches!(
        caller,
        at::LEADER_ROW_LABEL
            | at::LEADER_ROW_BUILD
            | at::LEADER_ROW_PRESS
            | at::LEADER_AUTO_SELECT
            | at::LEADER_APPOINT
    ) && squad(m).is_some_and(|s| session::phantom_player(m, s, player).unwrap_or(false))
}

fn retire(m: &mut dyn GuestMemory, phantoms: &Phantoms, target_only: bool) {
    let Some((squad, target)) = solo_sessions(m) else {
        return;
    };
    // The matchmaking Back confirmation cancels the target search and
    // returns to the same party. A party leave tears down both sessions.
    let squad = (!target_only).then_some(squad);
    for s in [squad, target].into_iter().flatten() {
        if let Err(e) = phantoms.retire_solo_session(m, s) {
            eprintln!(
                "bots: could not retire phantoms from session {s:#x}, fault at {:#x}",
                e.addr
            );
        }
    }
}

pub(super) fn install(b: &mut Builder, phantoms: &Phantoms) {
    b.raw()
        .before(
            &points::PARTY_PEER_COUNT,
            "leave a party with only Clankers as solo",
            |c| {
                let count = {
                    let m = c.ctx.memory();
                    m.u32(c.cpu.esp)
                        .ok()
                        .and_then(|caller| leave_peer_count(&*m, caller))
                };
                if let Some(count) = count {
                    c.cpu.eax = count;
                    return c.ret(0).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only()
        .optional();
    b.raw()
        .before(
            &points::PARTY_PLAYER_VALID,
            "exclude Clankers from party leadership",
            |c| {
                let reject = {
                    let m = c.ctx.memory();
                    m.u32(c.cpu.esp)
                        .is_ok_and(|caller| reject_candidate(&*m, caller, c.cpu.eax))
                };
                if reject {
                    c.cpu.eax &= !0xff;
                    return c.ret(0).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only()
        .optional();
    for (point, target_only) in [
        (&points::PARTY_LEAVE, false),
        (&points::MATCHMAKING_CANCEL_CONFIRMED, true),
    ] {
        let phantoms = phantoms.clone();
        b.raw()
            .before(
                point,
                "retire Clankers from the departing lobby",
                move |c| {
                    retire(&mut *c.ctx.memory(), &phantoms, target_only);
                    Ok(Flow::Continue)
                },
            )
            .optional();
    }
}

#[cfg(test)]
mod tests;
