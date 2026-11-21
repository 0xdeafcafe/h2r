//! `game.menu`: leave the running campaign or multiplayer game for the
//! main menu the way the title's own departure does, so a scripted run can
//! measure the menu after a level (docs/runtime/agent-api.md, "Returning
//! to the main menu"). The calls are made by the agent's guest calls at
//! the next tick (`agent::sim::calls::Call::QueueGame`): the network
//! sessions are retired, then `request_mainmenu(0)` queues the menu, which
//! is what the return-to-menu picture and the level boot's later requests
//! see (docs/runtime/level-boot.md#returning-to-the-main-menu).

use agent::sim::calls::{self, Arg, Call, GameStep};
use agent::{Access, Agent, GameCtx};
use agent_proto::{RpcError, code, method};
use serde_json::{Value, json};

use crate::layout_glue::ReadGm;
use crate::level_boot::{self, MODE_CAMPAIGN, MODE_MAINMENU, MODE_MULTIPLAYER, active_mode};

/// Ticks `game.menu` waits for the guest calls (one tick makes them).
const CALL_TICKS: u64 = 30;

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

/// The guest calls that leave the game, in order: retire the network
/// sessions (`host_lobby::LEAVE_SESSIONS`, as `join_link` and
/// `host_lobby::retail` do first), then request the main menu.
pub fn steps() -> Vec<GameStep> {
    let call = |target, args| GameStep::Call {
        target,
        args,
        eax: None,
        esi: None,
        report: None,
    };
    vec![
        call(crate::host_lobby::LEAVE_SESSIONS, Vec::new()),
        call(level_boot::REQUEST_MAINMENU, vec![Arg::Value(0)]),
    ]
}

/// Why the menu cannot be requested in `mode` (the active game's options
/// mode, `None` while none is active), or `None` when it can: only a
/// running campaign or multiplayer game leaves for the menu.
pub fn refusal(mode: Option<u32>) -> Option<&'static str> {
    match mode {
        Some(MODE_CAMPAIGN | MODE_MULTIPLAYER) => None,
        Some(MODE_MAINMENU) => Some("already at the main menu"),
        _ => Some("no campaign or multiplayer game is running"),
    }
}

/// Registers `game.menu` on `agent`. `calls` is the agent's guest-call
/// queue; without it the method answers unavailable.
pub fn install(agent: &Agent, calls: Option<calls::Shared>) {
    agent.add_game_method(method::GAME_MENU, Access::Act, move |v, ctx| {
        if !(v.is_null() || v.as_object().is_some_and(|o| o.is_empty())) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "game.menu takes no params",
            ));
        }
        let mut mode = None;
        let read = ctx
            .reader
            .with_guest(&mut |g| mode = active_mode(&ReadGm(&*g)));
        if !read {
            return Err(unavailable("guest memory unavailable"));
        }
        if let Some(why) = refusal(mode) {
            return Err(unavailable(why));
        }
        let q = calls.clone().ok_or_else(|| {
            unavailable(
                "the game's own functions aren't reachable here: the agent-calls feature isn't \
                 installed",
            )
        })?;
        let ticket = calls::submit(&q, Call::QueueGame { steps: steps() }).map_err(unavailable)?;
        eprintln!("game.menu: requesting the main menu (mode {mode:?})");
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.map(|v| {
                    json!({
                        "from": mode,
                        "queued": v["queued"].as_bool().unwrap_or(false),
                        "calls": v["calls"].as_u64().unwrap_or(0),
                    })
                }))
            }),
        );
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_retires_the_sessions_then_requests_the_menu() {
        let targets: Vec<(u32, Vec<Arg>)> = steps()
            .into_iter()
            .map(|s| match s {
                GameStep::Call {
                    target,
                    args,
                    eax: None,
                    esi: None,
                    report: None,
                } => (target, args),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            vec![
                (crate::host_lobby::LEAVE_SESSIONS, vec![]),
                (level_boot::REQUEST_MAINMENU, vec![Arg::Value(0)]),
            ]
        );
    }

    #[test]
    fn only_a_running_game_leaves_for_the_menu() {
        assert_eq!(refusal(Some(MODE_CAMPAIGN)), None);
        assert_eq!(refusal(Some(MODE_MULTIPLAYER)), None);
        assert_eq!(
            refusal(Some(MODE_MAINMENU)),
            Some("already at the main menu")
        );
        assert!(refusal(None).is_some());
        assert!(refusal(Some(9)).is_some());
    }
}
