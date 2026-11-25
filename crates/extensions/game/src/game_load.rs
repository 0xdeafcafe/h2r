//! `game.load` and `game.levels` (docs/runtime/agent-api.md, "Loading a
//! level"): queue any campaign mission or multiplayer map on the disc as
//! the next game, from the main menu or a running game.
//!
//! The level is resolved as `h2 play --level` resolves it
//! (`level_boot::resolve_in`: the disc's maps, then the campaigns installed
//! under the data root), and its options are built with the level boot's
//! steps (`level_boot::game_steps`), made by the agent's guest calls at the
//! next tick (`agent::sim::calls::Call::QueueGame`), the path
//! `maps.test.load` and `game.menu` take. A multiplayer map gets a local
//! game on the named game type's default variant (`VARIANT_DEFAULTS`'s
//! second argument, docs/game/community-gametypes.md).

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use agent::sim::calls::{self, Arg, Call, GameStep};
use agent::{Access, Agent, GameCtx};
use agent_proto::game_load::{self as wire, Level, Levels, LoadParams, Loaded, method};
use agent_proto::{RpcError, code};
use serde_json::{Value, json};

use crate::level_boot::{self, Difficulty, DiscLevel, Kind, Request};

/// Ticks `game.load` waits for the guest calls (one tick makes them).
const CALL_TICKS: u64 = 30;

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

/// Why a game can't be queued in a session of simulation mode `sim_mode`
/// (`cheats::sim_mode`): a co-op or networked session runs one game for
/// every machine, so only this one would leave it.
pub fn session_refusal(sim_mode: Option<u32>) -> Option<&'static str> {
    match sim_mode {
        Some(2..=5) => Some(
            "a co-op or networked session: every machine plays its game; leave it first \
             (game.menu)",
        ),
        _ => None,
    }
}

/// `p` checked and resolved by `resolve` (`level_boot::resolve_in` in the
/// game): the request, and the game type for a multiplayer map.
pub fn plan(
    p: &LoadParams,
    resolve: impl FnOnce(&str, Difficulty) -> anyhow::Result<Request>,
) -> Result<(Request, Option<u32>), RpcError> {
    if p.scenario.trim().is_empty() {
        return Err(invalid("scenario: a map name or scenario path (game.levels lists them)"));
    }
    let difficulty = p
        .difficulty
        .as_deref()
        .map(Difficulty::parse)
        .transpose()
        .map_err(|e| invalid(format!("difficulty: {e}")))?
        .unwrap_or_default();
    let game_type = p
        .variant
        .as_deref()
        .map(|v| {
            wire::game_type(v).ok_or_else(|| {
                invalid(format!("variant {v:?}: one of {}", wire::GAME_TYPES.join(", ")))
            })
        })
        .transpose()?;
    let req = resolve(&p.scenario, difficulty).map_err(|e| invalid(format!("{e:#}")))?;
    match req.kind {
        Kind::Campaign if game_type.is_some() => Err(invalid(format!(
            "{} is a campaign mission: variant is for multiplayer maps",
            req.name
        ))),
        Kind::Multiplayer if p.difficulty.is_some() => Err(invalid(format!(
            "{} is a multiplayer map: difficulty is for campaign missions",
            req.name
        ))),
        Kind::Campaign => Ok((req, None)),
        Kind::Multiplayer => Ok((req, Some(game_type.unwrap_or(level_boot::GAME_TYPE_SLAYER)))),
    }
}

/// The guest-call steps for `req`: the level boot's, with a multiplayer
/// map's default variant built for `game_type`. `None` when the options
/// can't hold the request.
pub fn steps(req: &Request, game_type: Option<u32>) -> Option<Vec<GameStep>> {
    let mut steps = level_boot::game_steps(req)?;
    if let Some(t) = game_type {
        for s in &mut steps {
            if let GameStep::Call { target, args, .. } = s
                && *target == level_boot::VARIANT_DEFAULTS
                && let Some(a) = args.get_mut(1)
            {
                *a = Arg::Value(t);
            }
        }
    }
    Some(steps)
}

/// The disc's levels as `game.levels` lists them.
pub fn levels(disc: &[DiscLevel]) -> Levels {
    let of = |k: Kind| {
        let mut v: Vec<Level> = disc
            .iter()
            .filter(|l| l.kind == k)
            .map(|l| Level {
                name: l.name.clone(),
                scenario: l.scenario.clone(),
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    };
    Levels {
        campaign: of(Kind::Campaign),
        multiplayer: of(Kind::Multiplayer),
        difficulties: wire::DIFFICULTIES.iter().map(|s| (*s).into()).collect(),
        variants: wire::GAME_TYPES.iter().map(|s| (*s).into()).collect(),
    }
}

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| invalid(e.to_string()))
}

/// Registers `game.load` and `game.levels` on `agent`. `calls` is the
/// agent's guest-call queue; without it `game.load` answers unavailable.
pub fn install(agent: &Agent, calls: Option<calls::Shared>, disc: &Path, data_root: &Path) {
    let disc: Arc<PathBuf> = Arc::new(disc.to_owned());
    let data_root: Arc<PathBuf> = Arc::new(data_root.to_owned());

    // The disc doesn't change while the game runs: read it once.
    let cache: Arc<OnceLock<Result<Levels, String>>> = Arc::default();
    let d = Arc::clone(&disc);
    agent.add_method(method::LEVELS, Access::Read, move |_| {
        cache
            .get_or_init(|| {
                level_boot::disc_levels(&d)
                    .map(|l| levels(&l))
                    .map_err(|e| format!("{e:#}"))
            })
            .clone()
            .map(|l| serde_json::to_value(l).expect("serialises"))
            .map_err(unavailable)
    });

    agent.add_game_method(method::LOAD, Access::Act, move |v, ctx| {
        let p: LoadParams = params(v)?;
        let mut sim_mode = None;
        if !ctx
            .reader
            .with_guest(&mut |g| sim_mode = crate::cheats::sim_mode(&*g))
        {
            return Err(unavailable("guest memory unavailable"));
        }
        if let Some(why) = session_refusal(sim_mode) {
            return Err(unavailable(why));
        }
        let q = calls.clone().ok_or_else(|| {
            unavailable(
                "the game's own functions aren't reachable here: the agent-calls feature isn't \
                 installed",
            )
        })?;
        let (req, game_type) = plan(&p, |name, difficulty| {
            level_boot::resolve_in(disc.as_path(), Some(data_root.as_path()), name, difficulty)
        })?;
        let steps = steps(&req, game_type).ok_or_else(|| {
            invalid(format!("{}: the path is longer than the options hold", req.scenario))
        })?;
        let ticket = calls::submit(&q, Call::QueueGame { steps }).map_err(unavailable)?;
        eprintln!("game.load: queueing {} ({}, {})", req.name, req.scenario, req.game());
        let (kind, difficulty, variant) = match req.kind {
            Kind::Campaign => ("campaign", Some(req.difficulty.to_string()), None),
            Kind::Multiplayer => (
                "multiplayer",
                None,
                game_type.and_then(|t| wire::GAME_TYPES.get(t as usize)).map(|s| (*s).to_owned()),
            ),
        };
        let (name, scenario) = (req.name.clone(), req.scenario.clone());
        ctx.defer(
            CALL_TICKS,
            Box::new(move |_: &mut GameCtx<'_>| {
                let out = ticket.outcome()?;
                Some(out.map(|v| {
                    json!(Loaded {
                        name: name.clone(),
                        scenario: scenario.clone(),
                        kind: kind.into(),
                        difficulty: difficulty.clone(),
                        variant: variant.clone(),
                        check: v["check"].as_bool().unwrap_or(false),
                        queued: v["queued"].as_bool().unwrap_or(false),
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

    fn lockout() -> Request {
        Request {
            name: "lockout".into(),
            scenario: "scenarios\\multi\\lockout\\lockout".into(),
            campaign: u32::MAX,
            map: 3,
            difficulty: Difficulty::default(),
            kind: Kind::Multiplayer,
            variant: None,
            options: None,
        }
    }

    fn mombasa(d: Difficulty) -> Request {
        Request {
            name: "03a_oldmombasa".into(),
            scenario: "scenarios\\solo\\03a_oldmombasa\\03a_oldmombasa".into(),
            campaign: 0,
            map: 3,
            difficulty: d,
            kind: Kind::Campaign,
            variant: None,
            options: None,
        }
    }

    fn load(scenario: &str, difficulty: Option<&str>, variant: Option<&str>) -> LoadParams {
        LoadParams {
            scenario: scenario.into(),
            difficulty: difficulty.map(Into::into),
            variant: variant.map(Into::into),
        }
    }

    #[test]
    fn a_campaign_mission_takes_a_difficulty_and_a_map_a_variant() {
        let (r, t) = plan(&load("03a_oldmombasa", Some("Legendary"), None), |n, d| {
            assert_eq!(n, "03a_oldmombasa");
            Ok(mombasa(d))
        })
        .unwrap();
        assert_eq!((r.difficulty, t), (Difficulty::Legendary, None));
        let (_, t) = plan(&load("lockout", None, None), |_, _| Ok(lockout())).unwrap();
        assert_eq!(t, Some(level_boot::GAME_TYPE_SLAYER));
        let (_, t) = plan(&load("lockout", None, Some("CTF")), |_, _| Ok(lockout())).unwrap();
        assert_eq!(t, Some(4));
    }

    #[test]
    fn mismatched_or_unknown_params_are_refused_before_anything_is_queued() {
        let e = |p: LoadParams, r: Request| plan(&p, |_, _| Ok(r)).unwrap_err().code;
        assert_eq!(e(load("lockout", Some("heroic"), None), lockout()), code::INVALID_PARAMS);
        assert_eq!(
            e(load("03a_oldmombasa", None, Some("slayer")), mombasa(Difficulty::Normal)),
            code::INVALID_PARAMS
        );
        assert_eq!(e(load("lockout", Some("mythic"), None), lockout()), code::INVALID_PARAMS);
        assert_eq!(e(load("lockout", None, Some("race")), lockout()), code::INVALID_PARAMS);
        assert_eq!(e(load(" ", None, None), lockout()), code::INVALID_PARAMS);
        let err = plan(&load("nowhere", None, None), |_, _| {
            Err(anyhow::anyhow!("no level \"nowhere\"; the disc has: ..."))
        })
        .unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS);
        assert!(err.message.contains("the disc has"), "{err:?}");
    }

    #[test]
    fn a_maps_steps_build_the_named_game_types_variant() {
        let variant_arg = |steps: &[GameStep]| {
            steps.iter().find_map(|s| match s {
                GameStep::Call { target, args, .. } if *target == level_boot::VARIANT_DEFAULTS => {
                    Some(args.clone())
                }
                _ => None,
            })
        };
        let base = level_boot::game_steps(&lockout()).unwrap();
        let s = steps(&lockout(), Some(4)).unwrap();
        assert_eq!(s.len(), base.len());
        assert_eq!(
            variant_arg(&s).unwrap(),
            vec![Arg::Options(level_boot::OPT_VARIANT), Arg::Value(4)]
        );
        assert_eq!(steps(&lockout(), Some(0)).unwrap(), base);
        // A campaign mission has no variant step; its steps are the boot's.
        let m = mombasa(Difficulty::Heroic);
        assert_eq!(steps(&m, None).unwrap(), level_boot::game_steps(&m).unwrap());
        assert!(variant_arg(&steps(&m, None).unwrap()).is_none());
    }

    #[test]
    fn co_op_and_networked_sessions_refuse_a_load() {
        for m in [None, Some(0), Some(1)] {
            assert_eq!(session_refusal(m), None, "{m:?}");
        }
        for m in 2..=5 {
            assert!(session_refusal(Some(m)).is_some(), "{m}");
        }
    }

    #[test]
    fn levels_are_listed_by_kind_and_name() {
        let disc = [
            DiscLevel { name: "zanzibar".into(), scenario: "z".into(), kind: Kind::Multiplayer },
            DiscLevel { name: "01a_tutorial".into(), scenario: "t".into(), kind: Kind::Campaign },
            DiscLevel { name: "ascension".into(), scenario: "a".into(), kind: Kind::Multiplayer },
        ];
        let l = levels(&disc);
        let names = |v: &[Level]| v.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&l.campaign), ["01a_tutorial"]);
        assert_eq!(names(&l.multiplayer), ["ascension", "zanzibar"]);
        assert_eq!(l.variants[4], "ctf");
        assert_eq!(l.difficulties, ["easy", "normal", "heroic", "legendary"]);
    }
}
