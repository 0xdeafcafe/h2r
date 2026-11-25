//! The Superbounce script beside the Rust chain: every chain scenario of
//! `superbounce_chain_tests.rs` steps a [`Chain`] of this module, which
//! steps the Rust chain, hands what it saw of the player's physics
//! (`Chain::physics`) to the built-in Superbounce script as the host does
//! (`rules::world::physics::input`, then `on bounced` for a Bounced), and
//! checks after every tick that the script agrees with the chain: its
//! `chain` is the chain's count, its `flying` is the chain's `live`, and
//! it scored exactly what the chain's award is worth (a bounce
//! `min(chain, 6)`, doubled for a Monster; a Bounced -1).
//! docs/game/rules-world-api.md, "Physics".

use std::sync::OnceLock;

use ::rules::fake::FakeGame;
use ::rules::ir::{Ir, OptionValue};
use ::rules::vm::{Arg, Input, Vm};
use superbounce::Spot;

use crate::superbounce_chain::{self as chain, Event, Physics, PlayerTick};

/// The player the scenarios step.
const SLOT: u8 = 0;

/// The built-in script, compiled once, and its options: the defaults
/// (bounce points 1, the Monster double and the Bounced penalty on) with
/// the suicide penalty off, so a Bounced scores its own point.
fn script() -> &'static (Ir, u32) {
    static IR: OnceLock<(Ir, u32)> = OnceLock::new();
    IR.get_or_init(|| {
        let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).expect("Superbounce compiles");
        let options = ir
            .pack_options(&[("suicide_costs".into(), OptionValue::Bool(false))])
            .expect("the option packs");
        (ir, options)
    })
}

/// The Rust chain and the script, stepped together.
#[derive(Clone, Debug)]
pub struct Chain {
    rust: chain::Chain,
    vm: Vm,
    game: FakeGame,
    tick: u32,
}

impl Default for Chain {
    fn default() -> Self {
        let (ir, options) = script().clone();
        Chain {
            rust: chain::Chain::default(),
            vm: Vm::new(ir, options, 0).expect("the script starts"),
            game: FakeGame::with_players(1),
            tick: 0,
        }
    }
}

impl Chain {
    pub fn live(&self) -> bool {
        self.rust.live()
    }

    pub fn armed(&self) -> bool {
        self.rust.armed()
    }

    pub fn physics(&self) -> &[Physics] {
        self.rust.physics()
    }

    /// One tick of both, checking the script against the chain.
    pub fn step(&mut self, t: Option<&PlayerTick>, spots: &[Spot]) -> Option<Event> {
        let event = self.rust.step(t, spots);
        self.tick += 1;
        self.vm.begin_tick(self.tick);
        let before = self.game.calls_to("score").len();
        let mut inputs: Vec<Input> = self
            .rust
            .physics()
            .iter()
            .map(|p| crate::rules::world::physics::input(SLOT, *p))
            .collect();
        if let Some(Event::Bounced { .. }) = event {
            inputs.push(Input::Bounced(SLOT));
        }
        for input in inputs {
            self.game.apply(&input);
            self.vm.fire(&mut self.game, input);
        }
        let scored: Vec<i32> = self.game.calls_to("score")[before..]
            .iter()
            .map(|args| match args[..] {
                [Arg::Player(Some(SLOT)), Arg::Int(n)] => n,
                ref other => panic!("score({other:?})"),
            })
            .collect();
        let worth: Vec<i32> = match event {
            Some(Event::Bounce { chain, monster, .. }) => {
                vec![chain.min(6) as i32 * if monster { 2 } else { 1 }]
            }
            Some(Event::Bounced { .. }) => vec![-1],
            None => Vec::new(),
        };
        let at = (self.tick, event, self.rust.physics().to_vec());
        assert_eq!(scored, worth, "the script's points at {at:?}");
        assert_eq!(
            self.vm.player_var(SLOT, "chain"),
            Some(self.rust.count() as i32),
            "the script's chain at {at:?}"
        );
        assert_eq!(
            self.vm.player_var(SLOT, "flying"),
            Some(i32::from(self.rust.live())),
            "the script's run at {at:?}"
        );
        event
    }
}
