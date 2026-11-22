//! Built-in scripts, compiled into every build and found by engine index
//! for a record with no mode, and the payoff modes as example scripts. See
//! `docs/game/rules-engine.md`, "A record with no mode", "Porting the
//! seven engines" and "Payoff".

/// Slayer (engine index 2): kill, suicide and betrayal scoring.
pub const SLAYER: &str = include_str!("slayer.rules");
/// Oddball (engine index 3): time held scores.
pub const ODDBALL: &str = include_str!("oddball.rules");
/// King of the Hill (engine index 4): time on the hill scores.
pub const KING: &str = include_str!("koth.rules");
/// Juggernaut (engine index 7): the killer of the Juggernaut takes the role.
pub const JUGGERNAUT: &str = include_str!("juggernaut.rules");
/// Territories (engine index 8): held territories score.
pub const TERRITORIES: &str = include_str!("territories.rules");
/// Capture the Flag (engine index 1). `ctf.rules` serves CTF and Assault;
/// each gets its own header, so each rides its own engine index.
pub const CTF: &str = concat!(
    "rules \"Capture the Flag\" 1\ncarrier ctf\n",
    include_str!("ctf.rules")
);
/// Assault (engine index 9), from the same body as [`CTF`].
pub const ASSAULT: &str = concat!("rules \"Assault\" 1\ncarrier assault\n", include_str!("ctf.rules"));

/// Infection: a kill switches the victim to the zombies' team.
pub const INFECTION: &str = include_str!("infection.rules");
/// Gun Game: a weapon ladder, one rung a kill.
pub const GUN_GAME: &str = include_str!("gun_game.rules");
/// VIP, with Forge powers granted by role.
pub const VIP: &str = include_str!("vip.rules");
/// Siege: the phased attack-and-defend mode.
pub const SIEGE: &str = include_str!("siege.rules");
/// Race: numbered checkpoints, a point a lap.
pub const RACE: &str = include_str!("race.rules");
/// Bounce Practice: points for superbounces.
pub const BOUNCE_PRACTICE: &str = include_str!("bounce_practice.rules");
/// Superbounce: the superbounce mode's guide, and points for bounces only:
/// a kill scores nothing.
pub const SUPERBOUNCE: &str = include_str!("superbounce.rules");

/// A built-in script: its name, the retail engine index it rides and its
/// source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub engine: u8,
    pub source: &'static str,
}

/// Every built-in script, by engine index.
pub const ALL: &[Builtin] = &[
    Builtin { name: "Capture the Flag", engine: 1, source: CTF },
    Builtin { name: "Slayer", engine: 2, source: SLAYER },
    Builtin { name: "Oddball", engine: 3, source: ODDBALL },
    Builtin { name: "King of the Hill", engine: 4, source: KING },
    Builtin { name: "Juggernaut", engine: 7, source: JUGGERNAUT },
    Builtin { name: "Territories", engine: 8, source: TERRITORIES },
    Builtin { name: "Assault", engine: 9, source: ASSAULT },
];

/// The built-in script source for a retail engine index, for the engines
/// whose script exists.
pub fn builtin(engine_index: u8) -> Option<&'static str> {
    match engine_index {
        1 => Some(CTF),
        2 => Some(SLAYER),
        3 => Some(ODDBALL),
        4 => Some(KING),
        7 => Some(JUGGERNAUT),
        8 => Some(TERRITORIES),
        9 => Some(ASSAULT),
        _ => None,
    }
}

/// The payoff modes, by name: example scripts, not built-ins by engine.
/// A variant names one by its blob's hash.
pub fn examples() -> &'static [(&'static str, &'static str)] {
    &[
        ("infection", INFECTION),
        ("gun_game", GUN_GAME),
        ("vip", VIP),
        ("siege", SIEGE),
        ("race", RACE),
        ("bounce_practice", BOUNCE_PRACTICE),
        ("superbounce", SUPERBOUNCE),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_compiles_verifies_and_rides_its_own_engine() {
        for index in 0..=u8::MAX {
            let Some(src) = builtin(index) else { continue };
            let ir = crate::lang::compile(src).unwrap();
            assert_eq!(ir.header.carrier, index);
            let report = crate::verify::verify(&ir).unwrap();
            assert!(report.fits(), "engine {index}: worst tick {}", report.worst_tick);
            // A port declares nothing: it does what its retail engine does.
            assert!(ir.header.requires.is_empty(), "engine {index}");
        }
        for index in [1, 2, 3, 4, 7, 8, 9] {
            assert!(builtin(index).is_some(), "engine {index}");
        }
        for index in [0, 5, 6, 10] {
            assert!(builtin(index).is_none(), "engine {index}");
        }
        // Slayer needs nothing a host lacks; the other ports wait on the
        // entries their headers name.
        let slayer = crate::lang::compile(SLAYER).unwrap();
        assert_eq!(crate::verify::unhostable(&slayer), None);
        for b in ALL {
            assert_eq!(builtin(b.engine), Some(b.source));
            assert_eq!(crate::lang::compile(b.source).unwrap().header.name, b.name);
        }
    }

    #[test]
    fn compiling_a_builtin_is_deterministic() {
        for src in [SLAYER, CTF, ODDBALL] {
            let a = crate::lang::compile(src).unwrap();
            let b = crate::lang::compile(src).unwrap();
            assert_eq!(a.encode(), b.encode());
            assert_eq!(a.id(), b.id());
        }
    }

    #[test]
    fn ctf_and_assault_share_a_body_and_differ_by_header() {
        let ctf = crate::lang::compile(CTF).unwrap();
        let assault = crate::lang::compile(ASSAULT).unwrap();
        assert_eq!(ctf.handlers, assault.handlers);
        assert_ne!(ctf.id(), assault.id());
    }
}
