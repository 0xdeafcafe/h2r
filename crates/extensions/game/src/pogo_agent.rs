//! `pogo.state` and `pogo.test` (docs/runtime/agent-api.md, "pogo";
//! docs/game/pogo-stick.md, "Interfaces"): the Pogo Stick as an agent
//! sees it and drives it.
//!
//! `pogo.state` reads what the last tick published: the script's options,
//! whether the loaded map carries the kit, every player holding a stick
//! with its rider's phase and trick ([`crate::pogo_ride`]), and the chase
//! camera ([`crate::pogo_camera`]). `pogo.test` shows a trick on the first
//! local player's stick ([`crate::pogo_ride::test`]): presentation only,
//! nothing is filed, sent or recorded.

use agent_proto::pogo as wire;

use crate::pogo::Trick;
use crate::pogo_ride::Ridden;

/// One rider on the wire.
pub fn rider_reply(r: &Ridden) -> wire::Rider {
    let s = &r.sample;
    wire::Rider {
        player: r.slot,
        local: r.local,
        unit: r.unit,
        weapon: r.weapon,
        style: s.style.id().into(),
        phase: s.phase.id().into(),
        mounted: f64::from(s.mounted),
        compress: f64::from(s.compress),
        flare: f64::from(s.flare),
        tuck: f64::from(s.tuck),
        trick: s.trick.map(|(t, p)| wire::Trick {
            id: t.id().into(),
            name: t.name().into(),
            progress: f64::from(p),
        }),
        posed: s.posed(),
    }
}

/// `pogo.state`'s result, from what the last tick published.
pub fn state_reply() -> wire::State {
    let active = crate::superbounce_mode::active();
    let options = crate::superbounce_options::current();
    let map = crate::superbounce_mode::state().map;
    let weapons = map
        .as_deref()
        .and_then(pogo_kit::placed::get)
        .map(|p| p.weapons.to_vec())
        .unwrap_or_default();
    let camera = crate::pogo_camera::view();
    wire::State {
        active,
        option: options.pogo,
        style: options.pogo_style.id().into(),
        map,
        weapons,
        riders: crate::pogo_ride::ridden().iter().map(rider_reply).collect(),
        camera: wire::Camera {
            on: camera.on,
            drawn: camera.drawn,
            room: f64::from(camera.room),
            share: f64::from(camera.share),
        },
        tricks: Trick::ALL.iter().map(|t| t.id().to_owned()).collect(),
    }
}

/// `pogo.test`: the trick named `id` on the first local player's stick.
pub fn test(id: &str) -> Result<wire::Test, String> {
    let Some(trick) = Trick::from_id(id) else {
        let known: Vec<_> = Trick::ALL.iter().map(|t| t.id()).collect();
        return Err(format!("no trick {id:?}: one of {}", known.join(", ")));
    };
    let player = crate::pogo_ride::test(trick)?;
    Ok(wire::Test {
        player,
        trick: trick.id().into(),
        name: trick.name().into(),
    })
}

/// Adds `pogo.state` and `pogo.test` to the agent.
pub fn install(agent: &agent::Agent) {
    use agent::Access;
    use agent_proto::{RpcError, code};
    agent.add_game_method(wire::STATE, Access::Read, |params, _ctx| {
        crate::superbounce_mode::no_params(&params)
            .map_err(|_| RpcError::new(code::INVALID_PARAMS, "pogo.state takes no parameters"))?;
        Ok(serde_json::to_value(state_reply()).expect("pogo.state serialises"))
    });
    agent.add_game_method(wire::TEST, Access::Act, |params, _ctx| {
        let p: wire::TestParams = serde_json::from_value(params)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let r = test(&p.trick).map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
        Ok(serde_json::to_value(r).expect("pogo.test serialises"))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pogo::{Phase, Sample};
    use pogo_kit::Style;

    fn ridden(trick: Option<(Trick, f32)>) -> Ridden {
        Ridden {
            slot: 3,
            local: true,
            unit: 0xe001_0007,
            weapon: 0xe002_0009,
            sample: Sample {
                style: Style::Forerunner,
                phase: Phase::Ride,
                mounted: 1.0,
                compress: 0.0,
                flare: 0.25,
                tuck: 1.0,
                trick,
            },
        }
    }

    #[test]
    fn a_rider_reads_as_its_sample() {
        let r = rider_reply(&ridden(Some((Trick::Superman, 0.5))));
        assert_eq!((r.player, r.local), (3, true));
        assert_eq!((r.unit, r.weapon), (0xe001_0007, 0xe002_0009));
        assert_eq!(r.style, "forerunner");
        assert_eq!(r.phase, "ride");
        assert_eq!(
            (r.mounted, r.compress, r.flare, r.tuck),
            (1.0, 0.0, 0.25, 1.0)
        );
        let t = r.trick.unwrap();
        assert_eq!(
            (t.id.as_str(), t.name.as_str(), t.progress),
            ("superman", "Superman", 0.5)
        );
        assert!(r.posed);
        assert_eq!(rider_reply(&ridden(None)).trick, None);
    }

    #[test]
    fn a_trick_that_is_not_one_is_refused_with_the_list() {
        let e = test("flourish").unwrap_err();
        assert!(e.contains("flourish"), "{e}");
        for t in Trick::ALL {
            assert!(e.contains(t.id()), "{e}");
        }
    }

    #[test]
    fn the_state_lists_every_trick_the_test_takes() {
        let s = state_reply();
        let ids: Vec<_> = Trick::ALL.iter().map(|t| t.id().to_owned()).collect();
        assert_eq!(s.tricks, ids);
        assert_eq!(s.riders.len(), crate::pogo_ride::ridden().len());
    }
}
