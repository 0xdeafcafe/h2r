//! The tick boundary's async body (docs/game/rules-engine.md, "The host
//! runtime"): the authority check, the calls the handlers made, in order,
//! and the sight traces, then the tick itself.

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::points;

use super::{Call, Work};
use crate::rules::guest;

/// A call the engine refused: the entry's name and the reason, and for a put
/// the place it was refused at.
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub call: String,
    pub error: String,
    pub put: Option<crate::rules::world::objects::Place>,
}

/// Before the tick: the authority check when asked, then each call in
/// order. Returns the answer and the calls the engine refused.
pub async fn make_calls(cx: &Cx, work: &Work) -> (Option<bool>, Vec<Refused>) {
    let mut refused = Vec::new();
    let mut authority = None;
    if work.ask_authority {
        match guest::authoritative(cx).await {
            Ok(a) => authority = Some(a),
            Err(e) => {
                refused.push(Refused {
                    call: "authority".to_string(),
                    error: e.to_string(),
                    put: None,
                });
                authority = Some(false);
            }
        }
        if authority == Some(false) {
            return (authority, refused);
        }
    }
    for call in &work.calls {
        let result = match *call {
            Call::StartLine(ty) => crate::rules::start_line::post(cx, work.tick, ty).await,
            Call::Score { player, delta } => guest::score(cx, player, delta).await,
            Call::SetLives { player, lives } => guest::set_lives(cx, player, lives).await,
            Call::Respawn(player) => crate::firefight::guest::respawn(cx, player).await,
            Call::RoleTraits { player, on } => crate::rules::roles::set(cx, player, on).await,
            Call::Vehicle(action, object) => action.apply(cx, object).await,
            Call::ResetObjective(object) => crate::rules::objectives::reset(cx, object).await,
            Call::BotOrder { player, order } => {
                if crate::rules::world::bots::deliver(player, order) {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!("player {player} is no bot"))
                }
            }
            Call::BotGoal { team, ref goal } => crate::rules::world::bots::set_goal(team, goal.clone()),
            Call::End => crate::firefight::guest::end_game(cx).await,
            Call::NextRound => guest::next_round(cx).await,
            // Messages are made at the boundary by `Runtime::pre_tick`
            // (`crate::rules::world::show::publish`); one reaching this far makes
            // no engine call.
            Call::Message(_) | Call::SoundTo { .. } => Ok(()),
            Call::Play { audience, ref path } => {
                crate::rules::world::audio::play(cx, audience, path).await
            }
            Call::SetLoadout {
                player,
                ref primary,
                ref secondary,
            } => {
                crate::rules::world::loadout::give(
                    cx,
                    player,
                    primary.as_deref(),
                    secondary.as_deref(),
                )
                .await
            }
            Call::Put(place) => crate::rules::world::objects::put(cx, place).await,
            Call::OfferPick(ref ask) => crate::rules::world::pick::open(cx, ask).await,
            Call::OfferChoice(ref ask) => crate::rules::world::choice::open(cx, ask).await,
        };
        if let Err(e) = result {
            // The VM is told a refused put, so a barrier it holds open at a
            // place the world would not take is closed in its state again.
            let put = match call {
                Call::Put(place) => Some(*place),
                _ => None,
            };
            refused.push(Refused {
                call: call.name().to_string(),
                error: e.to_string(),
                put,
            });
        }
    }
    (authority, refused)
}

/// The async body runs the boundary calls, traces sight, then re-enters
/// the tick with `reentry` set so the hook lets it pass.
pub async fn run_work(
    cx: Cx,
    shared: std::sync::Arc<crate::rules::feature::Shared>,
    work: Work,
    sight_scratch: u32,
) -> Result<Ret, EntryError> {
    use std::sync::atomic::Ordering;
    let (authority, refused) = make_calls(&cx, &work).await;
    let mut sight = Vec::new();
    if authority != Some(false) {
        for query in work.sight {
            let (from, to) = query.pair;
            let clear = crate::rules::world::physics::trace_sight(&cx, sight_scratch, query).await;
            sight.push((from, to, clear));
        }
    }
    {
        let mut rt = shared.lock();
        rt.finish(work.tick, authority, refused);
        rt.finish_sight(work.tick, sight);
    }
    shared.reentry.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await;
    shared.reentry.store(false, Ordering::SeqCst);
    Ok(Ret::new(r?.eax, 0))
}
