//! `rules.reload`: a new blob into the running game (docs/game/rules-engine.md,
//! "Reloading a running script").
//!
//! The decision is pure ([`decide`]): a game that takes no script edits is
//! refused ([`refusal`]: Theater, a matchmade game, a guest, no
//! multiplayer game, and a blob this host cannot run); a blob that keeps
//! the running one's state shape (`rules::reload::compatible`) goes in
//! place, between ticks ([`super::runtime::Runtime::swap`]); any other is
//! a restart, unless the caller asked for in place only or this game
//! cannot restart ([`restart_refusal`]).
//!
//! A restart replays the active options block, as `map.reload {}` does,
//! with the variant's script reference ([`restart_options`]) naming the
//! new blob: the blob is saved in the game-variant store first, a listed
//! store variant takes the new block (`inject::retarget`), and the
//! agent's guest calls queue the options (`OPTIONS_QUEUE`, [`restart_steps`]).
//! The game then starts again on the same map, players and variant, and
//! the start finds the new blob in the store.

use std::sync::Arc;

use ::rules::Sha256;
use ::rules::ir::Ir;
use agent::rules::Reload;
use agent::sim::calls::{Arg, GameStep};
use agent_proto::rules::{LastReload, ReloadMode, ReloadParams, ReloadResult, Reloaded};
use agent_proto::{RpcError, code};
use guest_layout::GuestMemory;
use variants::ext::{self, Ext, Mode, Script};

use super::feature::Shared;
use super::runtime;
use crate::layout_glue::{Gm, ReadGm};
use crate::variant_ext;

/// Where this machine sits in the game a reload would go into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    /// No networked game: a local game, or a session alone.
    Alone,
    SystemLinkHost,
    SystemLinkGuest,
    /// The host of a Live custom game (a matchmade one is refused first).
    LiveHost,
    LiveGuest,
}

/// What a reload sees of the game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Situation {
    pub seat: Seat,
    /// A multiplayer game is running.
    pub multiplayer: bool,
    /// A Live hopper's game, ranked or not (`Hle::matchmade`).
    pub matchmade: bool,
    /// A film's playback (Theater).
    pub playback: bool,
    /// The active record's engine index.
    pub engine: Option<i32>,
    /// Why this host cannot queue a game, when it cannot.
    pub restart: Option<&'static str>,
}

/// Why this game takes no script edits at all, or `None`.
pub fn refusal(s: &Situation) -> Option<String> {
    let why = if s.playback {
        "a Theater film's playback takes no script edits"
    } else if s.matchmade {
        "a matchmade game (a Live hopper, ranked or not) takes no script edits; script edits are for custom, System Link and local games"
    } else if matches!(s.seat, Seat::SystemLinkGuest | Seat::LiveGuest) {
        "this machine is a guest: only the host runs the script, so only the host reloads it"
    } else if !s.multiplayer {
        "no multiplayer game is running"
    } else {
        return None;
    };
    Some(why.into())
}

/// Why this game cannot restart for a new script, or `None`.
pub fn restart_refusal(s: &Situation, new: &Ir) -> Option<String> {
    if s.seat == Seat::LiveHost {
        return Some(
            "a Live game restarts from its lobby, not from a replayed options block; end the game and start it again there"
                .into(),
        );
    }
    if let Some(why) = s.restart {
        return Some(why.into());
    }
    match s.engine {
        Some(e) if e == i32::from(new.header.engine()) => None,
        Some(e) if e == i32::from(::rules::SCRIPT_INDEX) => {
            // A restart keeps this record and only changes its extension.
            match variants::portable::settings_record(new) {
                Ok(v) if v.game_type() == variants::GameType::Script => None,
                Ok(_) => Some("the script's settings need a different variant record".into()),
                Err(error) => Some(error.to_string()),
            }
        }
        Some(e) => Some(format!(
            "the script rides engine {}, and the variant is engine {e}",
            new.header.engine()
        )),
        None => Some("the active variant record could not be read".into()),
    }
}

/// What to do with a new blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    InPlace,
    /// `reasons`: why not in place (empty when a restart was asked for).
    Restart { reasons: Vec<String> },
    Refuse(String),
}

/// The decision for `new` in `s`, where `old` is the running script's
/// blob (none when no script runs).
pub fn decide(s: &Situation, old: Option<&Ir>, new: &Ir, mode: ReloadMode) -> Decision {
    if let Some(why) = refusal(s) {
        return Decision::Refuse(why);
    }
    if let Some(why) = runtime::host_refusal(new) {
        return Decision::Refuse(format!("this host cannot run the script: {why}"));
    }
    if let Some(why) = s.engine.and_then(|index| runtime::record_refusal(new, index)) {
        return Decision::Refuse(why);
    }
    let shape = match old {
        Some(old) => ::rules::reload::compatible(old, new)
            .map_err(|r| r.iter().map(ToString::to_string).collect::<Vec<_>>()),
        None => Err(vec!["no script runs in this game".to_string()]),
    };
    let reasons = match (mode, shape) {
        (ReloadMode::Restart, _) => Vec::new(),
        (_, Ok(())) => return Decision::InPlace,
        (ReloadMode::InPlace, Err(r)) => {
            return Decision::Refuse(format!("not in place: {}", r.join("; ")));
        }
        (ReloadMode::Auto, Err(r)) => r,
    };
    if let Some(why) = restart_refusal(s, new) {
        return Decision::Refuse(if reasons.is_empty() {
            format!("cannot restart: {why}")
        } else {
            format!("not in place ({}), and cannot restart: {why}", reasons.join("; "))
        });
    }
    Decision::Restart { reasons }
}

/// The record's option bits for the restarted game: the running record's
/// when the options read the same in both blobs, else the new blob's
/// defaults.
pub fn restart_bits(old: Option<&Ir>, new: &Ir, record: Script) -> u32 {
    let same = old.is_some_and(|old| match ::rules::reload::compatible(old, new) {
        Ok(()) => true,
        Err(r) => !r.iter().any(|r| {
            matches!(
                r,
                ::rules::reload::Reason::Option { .. } | ::rules::reload::Reason::OptionCount { .. }
            )
        }),
    });
    if same && record.mode == Mode::Rules {
        record.options
    } else {
        new.default_options()
    }
}

/// The record's block at its place in an options block.
const BLOCK_AT: usize = variant_ext::OPTIONS_VARIANT as usize + ext::AT;

/// The active options block with its variant's script reference naming
/// `hash` with option bits from [`restart_bits`]; the other extras are
/// kept. Returns the block and the new `hx` block.
pub fn restart_options(
    options: &[u8],
    old: Option<&Ir>,
    new: &Ir,
    hash: &Sha256,
) -> Result<(Vec<u8>, [u8; ext::LEN]), String> {
    let Some(bytes) = options.get(BLOCK_AT..BLOCK_AT + ext::LEN) else {
        return Err(format!("the options block is {} bytes, too short for its variant", options.len()));
    };
    let block: [u8; ext::LEN] = bytes.try_into().expect("a block's length");
    let mut e = Ext::in_effect(&block);
    let mut prefix = [0u8; 8];
    prefix.copy_from_slice(&hash.0[..8]);
    e.script = Script {
        mode: Mode::Rules,
        prefix,
        options: restart_bits(old, new, e.script),
    };
    let block = e.encode();
    let mut out = options.to_vec();
    out[BLOCK_AT..BLOCK_AT + ext::LEN].copy_from_slice(&block);
    Ok((out, block))
}

/// The guest calls of a restart: the options into the scratch, then the
/// game's queue with the scratch in EAX (as `map.reload {}`).
pub fn restart_steps(options: Vec<u8>) -> Vec<GameStep> {
    vec![
        GameStep::Write {
            offset: 0,
            bytes: options,
        },
        GameStep::Call {
            target: crate::level_boot::OPTIONS_QUEUE,
            args: vec![],
            eax: Some(Arg::Options(0)),
            esi: None,
            report: None,
        },
    ]
}

/// The game a reload sees, read now.
pub fn situation(
    m: &dyn GuestMemory,
    hle: Option<&live_hle::Hle>,
    playback: bool,
    restart: Option<&'static str>,
) -> Situation {
    let read = ReadGm(m);
    let session = crate::coop_probe::read_session(&read);
    let seat = match &session {
        Some(s) if s.live() && s.peers > 1 => {
            let class = read.u32(s.addr.wrapping_add(variant_ext::SESSION_NETWORK_CLASS));
            match (class == Some(variant_ext::SYSTEM_LINK), s.is_host()) {
                (true, true) => Seat::SystemLinkHost,
                (true, false) => Seat::SystemLinkGuest,
                (false, true) => Seat::LiveHost,
                (false, false) => Seat::LiveGuest,
            }
        }
        _ => Seat::Alone,
    };
    Situation {
        seat,
        multiplayer: bots::guest::game_mode(m) == Some(runtime::MODE_MULTIPLAYER),
        matchmade: hle.is_some_and(|h| h.matchmade().is_some()),
        playback,
        engine: super::guest::engine_index(m).ok().flatten(),
        restart,
    }
}

fn refused(why: impl Into<String>) -> RpcError {
    let why = why.into();
    let mut e = RpcError::new(agent_proto::sim::REFUSED, why.clone());
    e.data = Some(serde_json::json!({ "why": why }));
    e
}

/// The blob from the params: decoded and verified, or compiled from
/// source and verified. Returns it with its bytes.
pub fn blob(p: &ReloadParams) -> Result<(Ir, Vec<u8>), RpcError> {
    if let Some(b64) = &p.ir_b64 {
        let bytes = variants::portable::base64_decode(b64.trim())
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "ir_b64 is not base64"))?;
        let (ir, _) = ::rules::verify::load(&bytes)
            .map_err(|e| refused(format!("the blob does not verify: {e}")))?;
        return Ok((ir, bytes));
    }
    let src = p.source.as_deref().unwrap_or_default();
    let (ir, _) = ::rules::build(src).map_err(|e| refused(format!("the script does not build: {e}")))?;
    let bytes = ir.encode();
    Ok((ir, bytes))
}

/// `rules.reload` over the game at this boundary (`AgentSource`).
/// `restart`: why the agent cannot queue a game. Records the outcome on
/// `shared` for `rules.state`, except a restart's, which
/// [`restart_outcome`] records once its calls are made.
pub fn reload(
    shared: &Shared,
    m: &mut dyn GuestMemory,
    p: ReloadParams,
    restart: Option<&'static str>,
    playback: bool,
) -> Result<Reload, RpcError> {
    let tick = bots::guest::game_tick(&*m).unwrap_or(0);
    let out = run(shared, m, p, restart, playback, tick);
    if let Err(e) = &out {
        shared.set_last_reload(LastReload::refused(None, tick, e.message.clone()));
    }
    out
}

fn run(
    shared: &Shared,
    m: &mut dyn GuestMemory,
    p: ReloadParams,
    restart: Option<&'static str>,
    playback: bool,
    tick: u32,
) -> Result<Reload, RpcError> {
    let (new, bytes) = blob(&p)?;
    let hash = new.id();
    let links = shared.links();
    let hle = links.and_then(|l| l.hle.as_deref());
    let s = situation(&*m, hle, playback, restart);
    let old = shared.lock().running().map(|r| r.vm.ir().clone());
    let decision = decide(&s, old.as_ref(), &new, p.mode);
    match decision {
        Decision::Refuse(why) => Err(refused(why)),
        Decision::InPlace => {
            let hash = shared.lock().swap(new).map_err(refused)?;
            // A Live host's session names the running blob for its guests'
            // display data, as at the start; the call may wait on the
            // network, so it leaves the game thread.
            if s.seat == Seat::LiveHost
                && let Some(h) = links.and_then(|l| l.hle.clone())
            {
                let named = hash.to_string();
                std::thread::spawn(move || h.set_session_rules(Some(named)));
            }
            let result = ReloadResult {
                mode: Reloaded::InPlace,
                hash: hash.to_string(),
                reasons: Vec::new(),
                tick,
            };
            shared.set_last_reload(LastReload::of(&result));
            Ok(Reload::Done(result))
        }
        Decision::Restart { reasons } => {
            let options = agent::sim::active_options(&*m)?;
            let (patched, block) = restart_options(&options, old.as_ref(), &new, &hash).map_err(refused)?;
            let scripts = links
                .map(|l| Arc::clone(&l.scripts))
                .ok_or_else(|| refused("the game-variant store is not open in this run"))?;
            scripts
                .store
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .save_script(bytes)
                .map_err(|e| refused(format!("keeping the script in the game-variant store: {e}")))?;
            let record = &options[variant_ext::OPTIONS_VARIANT as usize..];
            if record.len() >= 0x48 {
                let engine = i32::from_le_bytes(record[0x44..0x48].try_into().expect("four bytes"));
                crate::game_variants::inject::retarget(&record[4..0x44], engine, block);
            }
            Ok(Reload::Restart {
                result: ReloadResult {
                    mode: Reloaded::Restarted,
                    hash: hash.to_string(),
                    reasons,
                    tick,
                },
                steps: restart_steps(patched),
            })
        }
    }
}

/// A restart's guest calls were made, or failed: the record for
/// `rules.state`.
pub fn restart_outcome(shared: &Shared, outcome: Result<&ReloadResult, &str>) {
    shared.set_last_reload(match outcome {
        Ok(r) => LastReload::of(r),
        Err(why) => LastReload::refused(None, 0, format!("the restart's guest calls failed: {why}")),
    });
}

#[cfg(test)]
mod tests;
