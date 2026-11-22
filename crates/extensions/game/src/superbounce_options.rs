//! The superbounce HUD's options (docs/game/superbounce.md, "The HUD's
//! options"): which pieces of the HUD a game shows. They are options of
//! the Superbounce rules script
//! (`crates/formats/rules/src/builtin/superbounce.rules`), carried in the
//! variant record's option bits, and the host reads them by name through
//! the path that turns the mode on: the active variant's extension block
//! ([`crate::variant_ext::in_game`]), its script reference, the script's
//! IR ([`crate::rules::header::superbounce_options`]).
//!
//! Every piece is on unless a variant turns it off. A game with the mode
//! on and no script (the `H2_SUPERBOUNCE` switch, `superbounce.set`, the
//! name fallback) takes [`Options::DEFAULT`]; so does a script that
//! declares none of them, or only some.

use std::sync::atomic::{AtomicU16, Ordering};

use engine::GuestMemory;
use ::rules::ir::Ir;

/// The script's option names.
pub const TARGETS: &str = "targets";
pub const VELOCITY: &str = "velocity";
pub const ALIGNMENT: &str = "alignment";
pub const MISS_INFO: &str = "miss_info";
pub const MEDALS: &str = "medals";
pub const SECOND_BOUNCE: &str = "second_bounce";
pub const APPROACH_GUIDE: &str = "approach_guide";
pub const CHARGE_SEGMENTS: &str = "charge_segments";

/// Which seams get a target navpoint, by what is known about them
/// ([`crate::superbounce_targets::Class`]). The script's `Targets` enum,
/// in its order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TargetMode {
    /// The proven and the community-measured ones.
    Common,
    /// Those and the sim's learned lines and the scan's slots.
    #[default]
    Likely,
    /// Every seam the scan found that no game run failed.
    All,
}

impl TargetMode {
    /// The mode for the option's value (the enum's index); `Likely` for a
    /// value the enum does not have.
    pub fn from_value(v: i32) -> TargetMode {
        match v {
            0 => TargetMode::Common,
            2 => TargetMode::All,
            _ => TargetMode::Likely,
        }
    }

    /// The next mode round the cycle common, likely, all: the player's
    /// switch of it in the game (`superbounce_targets::Switch`).
    pub fn next(self) -> TargetMode {
        match self {
            TargetMode::Common => TargetMode::Likely,
            TargetMode::Likely => TargetMode::All,
            TargetMode::All => TargetMode::Common,
        }
    }

    /// The enum value's name in the script.
    pub fn name(self) -> &'static str {
        match self {
            TargetMode::Common => "common",
            TargetMode::Likely => "likely",
            TargetMode::All => "all",
        }
    }
}

/// What the HUD shows this game.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Options {
    pub targets: TargetMode,
    /// The speed and its direction while moving.
    pub velocity: bool,
    /// The fall's speed bar and steer line.
    pub alignment: bool,
    /// Why a landing did not bounce.
    pub miss_info: bool,
    /// The medal lines and their announcer.
    pub medals: bool,
    /// After a bounce, the seams the flight can reach, ranked
    /// ([`crate::superbounce_second`]).
    pub second_bounce: bool,
    /// The approach guide: the pipper and the box on the seam
    /// ([`crate::superbounce_guide`]).
    pub approach_guide: bool,
    /// The charge segments round the motion tracker
    /// (`hud_kit_core::huds::superbounce_segments`).
    pub charge_segments: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options::DEFAULT
    }
}

impl Options {
    /// Every piece on, the likely targets.
    pub const DEFAULT: Options = Options {
        targets: TargetMode::Likely,
        velocity: true,
        alignment: true,
        miss_info: true,
        medals: true,
        second_bounce: true,
        approach_guide: true,
        charge_segments: true,
    };

    /// The options a record's option bits `bits` give for the script
    /// `ir`. An option the script does not declare keeps its default.
    pub fn from_ir(ir: &Ir, bits: u32) -> Options {
        let d = Options::DEFAULT;
        let on = |name: &str, default: bool| ir.option_value(bits, name).map_or(default, |v| v != 0);
        Options {
            targets: ir
                .option_value(bits, TARGETS)
                .map_or(d.targets, TargetMode::from_value),
            velocity: on(VELOCITY, d.velocity),
            alignment: on(ALIGNMENT, d.alignment),
            miss_info: on(MISS_INFO, d.miss_info),
            medals: on(MEDALS, d.medals),
            second_bounce: on(SECOND_BOUNCE, d.second_bounce),
            approach_guide: on(APPROACH_GUIDE, d.approach_guide),
            charge_segments: on(CHARGE_SEGMENTS, d.charge_segments),
        }
    }

    const fn pack(self) -> u16 {
        let t = match self.targets {
            TargetMode::Common => 0,
            TargetMode::Likely => 1,
            TargetMode::All => 2,
        };
        t | (self.velocity as u16) << 2
            | (self.alignment as u16) << 3
            | (self.miss_info as u16) << 4
            | (self.medals as u16) << 5
            | (self.second_bounce as u16) << 6
            | (self.approach_guide as u16) << 7
            | (self.charge_segments as u16) << 8
    }

    fn unpack(b: u16) -> Options {
        Options {
            targets: TargetMode::from_value(i32::from(b & 3)),
            velocity: b & 1 << 2 != 0,
            alignment: b & 1 << 3 != 0,
            miss_info: b & 1 << 4 != 0,
            medals: b & 1 << 5 != 0,
            second_bounce: b & 1 << 6 != 0,
            approach_guide: b & 1 << 7 != 0,
            charge_segments: b & 1 << 8 != 0,
        }
    }
}

/// The running game's options, as the mode last read them.
static CURRENT: AtomicU16 = AtomicU16::new(Options::DEFAULT.pack());

/// The running game's options.
pub fn current() -> Options {
    Options::unpack(CURRENT.load(Ordering::Relaxed))
}

/// Sets the running game's options: the mode's, each time it decides.
pub fn set(o: Options) {
    CURRENT.store(o.pack(), Ordering::Relaxed);
}

/// The options of the game in `m`: the active variant's script's, when
/// this machine applies its block and has the script; else the defaults.
pub fn read(m: &dyn GuestMemory) -> Options {
    crate::variant_ext::in_game(&crate::layout_glue::ReadGm(m))
        .script()
        .map_or(Options::DEFAULT, crate::rules::header::superbounce_options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::ir::OptionValue;

    fn script() -> Ir {
        ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap()
    }

    /// The script's defaults are the host's: every piece on, the likely
    /// targets.
    #[test]
    fn the_scripts_defaults_are_every_piece_on() {
        let ir = script();
        assert_eq!(Options::from_ir(&ir, ir.default_options()), Options::DEFAULT);
        assert!(Options::DEFAULT.velocity && Options::DEFAULT.medals);
        assert!(Options::DEFAULT.approach_guide && Options::DEFAULT.charge_segments);
        assert_eq!(Options::DEFAULT.targets, TargetMode::Likely);
    }

    /// A variant turns each piece off by its name, and picks the targets.
    #[test]
    fn a_variant_turns_each_piece_off() {
        let ir = script();
        let with = |name: &str, v: OptionValue| {
            let bits = ir.pack_options(&[(name.to_string(), v)]).unwrap();
            Options::from_ir(&ir, bits)
        };
        let d = Options::DEFAULT;
        assert_eq!(with(VELOCITY, OptionValue::Bool(false)), Options { velocity: false, ..d });
        assert_eq!(with(ALIGNMENT, OptionValue::Bool(false)), Options { alignment: false, ..d });
        assert_eq!(with(MISS_INFO, OptionValue::Bool(false)), Options { miss_info: false, ..d });
        assert_eq!(with(MEDALS, OptionValue::Bool(false)), Options { medals: false, ..d });
        assert_eq!(
            with(SECOND_BOUNCE, OptionValue::Bool(false)),
            Options { second_bounce: false, ..d }
        );
        assert_eq!(
            with(APPROACH_GUIDE, OptionValue::Bool(false)),
            Options { approach_guide: false, ..d }
        );
        assert_eq!(
            with(CHARGE_SEGMENTS, OptionValue::Bool(false)),
            Options { charge_segments: false, ..d }
        );
        for mode in [TargetMode::Common, TargetMode::Likely, TargetMode::All] {
            assert_eq!(
                with(TARGETS, OptionValue::Name(mode.name().into())),
                Options { targets: mode, ..d }
            );
        }
        // The scoring options do not move a HUD piece.
        assert_eq!(with("bounce_points", OptionValue::Int(5)), d);
    }

    /// A script with none of the options (another script that asks for
    /// the mode) gives the defaults whatever its bits.
    #[test]
    fn a_script_without_the_options_gives_the_defaults() {
        let ir = ::rules::compile(::rules::builtin::SLAYER).unwrap();
        assert_eq!(Options::from_ir(&ir, 0), Options::DEFAULT);
        assert_eq!(Options::from_ir(&ir, u32::MAX), Options::DEFAULT);
    }

    #[test]
    fn the_options_round_trip_through_the_current_slot() {
        for targets in [TargetMode::Common, TargetMode::Likely, TargetMode::All] {
            for bits in 0..128u8 {
                let o = Options {
                    targets,
                    velocity: bits & 1 != 0,
                    alignment: bits & 2 != 0,
                    miss_info: bits & 4 != 0,
                    medals: bits & 8 != 0,
                    second_bounce: bits & 16 != 0,
                    approach_guide: bits & 32 != 0,
                    charge_segments: bits & 64 != 0,
                };
                assert_eq!(Options::unpack(o.pack()), o);
            }
        }
    }
}
