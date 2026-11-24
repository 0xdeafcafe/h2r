//! One scorer in a script game (docs/game/rules-engine.md, "The host
//! runtime", "One scorer").
//!
//! A mode 2 game runs on its lifted carrier engine, whose own rules would
//! score beside the script's. While a script runs on this machine and the
//! engine has said this machine is its authority ([`script_scores`]), the
//! feature answers the carrier's rule-making routines in place:
//!
//! - `SLAYER_KILL` and `JUGGERNAUT_KILL`, the kill callbacks (vtable slot
//!   `+0x78`): returned at once, popping their 16 bytes. Slayer's is all
//!   scoring. Juggernaut's also moves the role; the script moves it with
//!   `role_traits` ([`super::roles`]).
//! - `JUGGERNAUT_TICK` (slot `+0x48`): returned at once. It drops the role
//!   from a Juggernaut who quit or is out of lives and draws a new one when
//!   none is left, which would crown a second Juggernaut beside the
//!   script's.
//! - `SCORE_ADD` from one of the carriers' own call sites
//!   ([`CARRIER_SCORE_SITES`], by the return address on the stack):
//!   returned at once with AL 0, popping its 4 bytes, as a refused score.
//!   Oddball, King, Territories and CTF score from their per-tick and
//!   per-player slots, which also run the ball, the hill, the territories
//!   and the flags; only their points are dropped. The script's own
//!   `score` calls come from no such site and pass.
//!
//! Everything else the engine does runs: `PLAYER_KILLED`'s kill feed,
//! medals and statistics, the objective engines' kill callbacks (they post
//! events only), their objects and their traits. A refused or idle script,
//! a guest and a film's replayed non-authority leave every routine to the
//! engine. A `uses` script's game runs the built-in script it uses, which
//! is the one scorer as any script is.
//! The evidence is in
//! docs/engine/notes/2026-11-22-script-scoring-and-roles.md.

use std::sync::Arc;

use engine::GuestMemory;
use engine::points;

use super::feature::Shared;
use super::runtime::{Runtime, Status};

/// The kill callbacks' stack arguments (attacker, victim, flag, kind).
pub const KILL_POP: u32 = 16;
/// `SCORE_ADD`'s stack argument (the player).
pub const SCORE_POP: u32 = 4;
/// The length of a `call rel32`: a call site's return address is the site
/// plus this.
pub const CALL_LEN: u32 = 5;

/// Every call of `SCORE_ADD` in the image (a byte search of `.text` for
/// `call 0x15b7c0`), each a carrier engine's own scoring, with its engine.
pub const CARRIER_SCORE_SITES: &[(u32, &str)] = &[
    (0x002b_c05d, "slayer"),
    (0x002b_c077, "slayer"),
    (0x002b_c0f7, "slayer"),
    (0x0019_238c, "juggernaut"),
    (0x0019_23db, "juggernaut"),
    (0x002b_ceb2, "oddball"),
    (0x002b_f1e7, "king"),
    (0x002b_f2d2, "king"),
    (0x002c_0b32, "territories"),
    (0x0024_2e0e, "ctf"),
];

/// The carrier whose scoring `SCORE_ADD` returns to `ret` from, if any.
pub fn carrier_site(ret: u32) -> Option<&'static str> {
    CARRIER_SCORE_SITES
        .iter()
        .find(|&&(at, _)| at.wrapping_add(CALL_LEN) == ret)
        .map(|&(_, carrier)| carrier)
}

/// Whether the script makes this game's rules here: its VM runs, and
/// `GAME_ENGINE_AUTHORITATIVE` has answered true for this machine. Before
/// the first tick boundary asks, the engine keeps its rules. A `scoring
/// carrier` script's game is a running one: its carrier's built-in
/// scores (docs/game/rules-variants.md, "A script that keeps its
/// carrier's rules").
pub fn script_scores(rt: &Runtime) -> bool {
    match &rt.status {
        Status::Running(r) => r.authority == Some(true),
        Status::Idle(_) | Status::Refused { .. } | Status::Blocked { .. } => false,
    }
}

/// Whether `SCORE_ADD`, returning to `ret`, is answered with no score.
pub fn drops_score(rt: &Runtime, ret: u32) -> bool {
    carrier_site(ret).is_some() && script_scores(rt)
}

/// The hooks: a `before` on each routine above. No other feature hooks
/// these points, so each takes the default class.
pub fn register(b: &mut hooks::Builder, shared: &Arc<Shared>) {
    use hooks::Flow;
    let s = Arc::clone(shared);
    b.raw().before(
        &points::SLAYER_KILL,
        "rules: the script scores the kill, not Slayer",
        move |c| {
            if !script_scores(&s.lock()) {
                return Ok(Flow::Continue);
            }
            c.ret(KILL_POP).map(Flow::Return)
        },
    );
    let s = Arc::clone(shared);
    b.raw().before(
        &points::JUGGERNAUT_KILL,
        "rules: the script scores the kill and moves the role, not Juggernaut",
        move |c| {
            if !script_scores(&s.lock()) {
                return Ok(Flow::Continue);
            }
            c.ret(KILL_POP).map(Flow::Return)
        },
    );
    let s = Arc::clone(shared);
    b.raw().before(
        &points::JUGGERNAUT_TICK,
        "rules: the script keeps the Juggernaut role, not the engine's tick",
        move |c| {
            if !script_scores(&s.lock()) {
                return Ok(Flow::Continue);
            }
            c.ret(0).map(Flow::Return)
        },
    );
    let s = Arc::clone(shared);
    b.raw().before(
        &points::SCORE_ADD,
        "rules: the script is the game's one scorer",
        move |c| {
            let ret = {
                let rt = Arc::clone(c.ctx.runtime());
                let mem = rt.memory();
                let m: &dyn GuestMemory = &*mem;
                m.u32(c.cpu.esp).ok()
            };
            if !ret.is_some_and(|ret| drops_score(&s.lock(), ret)) {
                return Ok(Flow::Continue);
            }
            c.cpu.eax &= !0xff;
            c.ret(SCORE_POP).map(Flow::Return)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_carrier_site_is_known_by_its_return_address() {
        for &(at, carrier) in CARRIER_SCORE_SITES {
            assert_eq!(carrier_site(at + CALL_LEN), Some(carrier), "{at:#x}");
            assert_eq!(carrier_site(at), None, "{at:#x}");
        }
        assert_eq!(CARRIER_SCORE_SITES.len(), 10);
        // Every carrier but Assault, which shares CTF's code, scores here.
        for &(name, _) in ::rules::caps::CARRIERS {
            let present = CARRIER_SCORE_SITES.iter().any(|&(_, c)| c == name);
            assert_eq!(present, name != "assault", "{name}");
        }
        assert_eq!(carrier_site(0), None);
    }
}
