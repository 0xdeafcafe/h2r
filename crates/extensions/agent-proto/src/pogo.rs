//! `pogo.state` and `pogo.test`: Superbounce's Pogo Stick
//! (docs/runtime/agent-api.md, "pogo"; docs/game/pogo-stick.md).
//! Wire types only. Game methods: `session.methods` lists them where the
//! game registered them (`game::pogo_agent::install`).

use serde::{Deserialize, Serialize};

/// The stick's state now ([`State`]; no params).
pub const STATE: &str = "pogo.state";
/// Shows a trick on the local player's stick ([`TestParams`], [`Test`]).
pub const TEST: &str = "pogo.test";

/// Method names.
pub mod method {
    pub const ALL: &[&str] = &[super::STATE, super::TEST];
}

/// A trick in progress.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Trick {
    /// `no_hander`, `can_can`, `bar_spin`, `superman`, `backflip`,
    /// `double_backflip` or `front_flip`.
    pub id: String,
    /// The trick's name, as a person reads it.
    pub name: String,
    /// 0 at its start to 1 at its end; half at the flight's apex.
    pub progress: f64,
}

/// One player holding a stick.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rider {
    /// The player's slot.
    pub player: u8,
    /// A player of this machine.
    pub local: bool,
    /// The biped's and the held stick's object handles (`world.object`).
    pub unit: u32,
    pub weapon: u32,
    /// The stick's look: `unsc`, `covenant` or `forerunner`.
    pub style: String,
    /// `carried`, `mount`, `ride`, `compress`, `spring` or `dismount`.
    pub phase: String,
    /// 0 carried in the hands, 1 on the stick.
    pub mounted: f64,
    /// 0 the piston out, 1 all the way in.
    pub compress: f64,
    /// 1 on the tick of a bounce, fading to 0.
    pub flare: f64,
    /// 0 legs out, 1 knees tucked.
    pub tuck: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trick: Option<Trick>,
    /// The pose is drawn over the game's own (anything but carried with
    /// the piston out).
    pub posed: bool,
}

/// The chase camera.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    /// A local player holds a stick: the camera is asked for.
    pub on: bool,
    /// The last frame was drawn from it (false headless, in split screen
    /// and under Forge's or Theater's camera).
    pub drawn: bool,
    /// The share of the boom the level leaves room for, 0 to 1.
    pub room: f64,
    /// The share of the boom now, eased.
    pub share: f64,
    /// Frames drawn from the body's eye and aim because the game's own
    /// view did not read as a camera, since the game started.
    #[serde(default)]
    pub fallback_frames: u64,
}

/// `pogo.state`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The superbounce mode is on (the game's script asks for it).
    pub active: bool,
    /// The script's `pogo` option.
    pub option: bool,
    /// The script's `pogo_style` option: the look a spawn is given.
    pub style: String,
    /// The loaded map's name; none without a map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// The map carries the kit: the three sticks' weapon tags, in the
    /// order `unsc`, `covenant`, `forerunner`. Empty when it does not
    /// (the `pogo` feature off, or the map refused), and then no stick
    /// is given.
    pub weapons: Vec<u32>,
    /// Every player holding a stick, the local players first.
    pub riders: Vec<Rider>,
    pub camera: Camera,
    /// The tricks `pogo.test` takes.
    pub tricks: Vec<String>,
}

/// `pogo.test`'s params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestParams {
    /// A trick's id.
    pub trick: String,
}

/// `pogo.test`'s result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Test {
    /// The slot of the player it is shown on.
    pub player: u8,
    pub trick: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_takes_a_trick_and_nothing_else() {
        let p: TestParams =
            serde_json::from_value(serde_json::json!({"trick": "bar_spin"})).unwrap();
        assert_eq!(p.trick, "bar_spin");
        assert!(
            serde_json::from_value::<TestParams>(serde_json::json!({"trick": "x", "player": 1}))
                .is_err()
        );
        assert!(serde_json::from_value::<TestParams>(serde_json::json!({})).is_err());
    }

    #[test]
    fn a_rider_with_no_trick_leaves_it_out() {
        let s = State {
            active: true,
            riders: vec![Rider {
                player: 2,
                local: true,
                phase: "carried".into(),
                style: "unsc".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let j = serde_json::to_value(&s).unwrap();
        assert!(j["riders"][0].get("trick").is_none());
        assert!(j.get("map").is_none());
        assert_eq!(j["weapons"], serde_json::json!([]));
        assert_eq!(serde_json::from_value::<State>(j).unwrap(), s);
        assert_eq!(method::ALL, [STATE, TEST]);
    }
}
