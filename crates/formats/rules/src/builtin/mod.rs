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
// The headers `ctf.rules` gets for CTF and Assault: macros, so `concat!`
// takes them and [`sources_of`] uses the same text.
macro_rules! ctf_header {
    () => {
        "rules \"Capture the Flag\" 1\ncarrier ctf\n"
    };
}
macro_rules! assault_header {
    () => {
        "rules \"Assault\" 1\ncarrier assault\n"
    };
}

/// Capture the Flag (engine index 1). `ctf.rules` serves CTF and Assault;
/// each gets its own header, so each rides its own engine index.
pub const CTF: &str = concat!(ctf_header!(), include_str!("ctf.rules"));
/// Assault (engine index 9), from the same body as [`CTF`].
pub const ASSAULT: &str = concat!(assault_header!(), include_str!("ctf.rules"));

/// The script sources a file of this directory makes, given its name and
/// its text now (an edited checkout's, not the baked one): `ctf.rules`
/// makes [`CTF`] and [`ASSAULT`], any other file itself. `h2 play --watch`
/// reloads these into a running game (docs/runtime/app.md, "Watching the
/// checkout").
pub fn sources_of(file_name: &str, text: &str) -> Vec<String> {
    if file_name == "ctf.rules" {
        vec![format!("{}{text}", ctf_header!()), format!("{}{text}", assault_header!())]
    } else {
        vec![text.to_string()]
    }
}

/// Infection: a kill switches the victim to the zombies' team.
pub const INFECTION: &str = include_str!("infection.rules");
/// Gun Game: a weapon ladder, one rung a kill.
pub const GUN_GAME: &str = include_str!("gun_game.rules");
/// VIP: a VIP per team, worth more to kill, the role passing on.
pub const VIP: &str = include_str!("vip.rules");
/// Siege: the phased attack-and-defend mode.
pub const SIEGE: &str = include_str!("siege.rules");
/// Race: numbered checkpoints, a point a lap.
pub const RACE: &str = include_str!("race.rules");
/// Bounce Hunt: points for superbounces.
pub const BOUNCE_HUNT: &str = include_str!("bounce_hunt.rules");
/// Superbounce: the superbounce mode's guide, and points for bounces only:
/// a kill scores nothing. Its settings block is the listed variant's.
pub const SUPERBOUNCE: &str = include_str!("superbounce.rules");

// The listed variants, the community's classics: each a settings script
// (`scoring carrier`) except Superbounce. See `LISTED`.
/// Zombies: honour rules, in `about`.
pub const ZOMBIES: &str = include_str!("zombies.rules");
/// Hide and Seek.
pub const HIDE_AND_SEEK: &str = include_str!("hide_and_seek.rules");
/// Jousting.
pub const JOUSTING: &str = include_str!("jousting.rules");
/// SWAT.
pub const SWAT: &str = include_str!("swat.rules");
/// Duel.
pub const DUEL: &str = include_str!("duel.rules");
/// Clint Eastwood.
pub const CLINT_EASTWOOD: &str = include_str!("clint_eastwood.rules");
/// Sun King.
pub const SUN_KING: &str = include_str!("sun_king.rules");
/// Tower of Power.
pub const TOWER_OF_POWER: &str = include_str!("tower_of_power.rules");
/// Wheeeeee!
pub const WHEEEEEE: &str = include_str!("wheeeeee.rules");
/// Cops and Robbers.
pub const COPS_AND_ROBBERS: &str = include_str!("cops_and_robbers.rules");
/// Bounce Practice: settings only, no score.
pub const BOUNCE_PRACTICE: &str = include_str!("bounce_practice.rules");
/// Glitch Hunt.
pub const GLITCH_HUNT: &str = include_str!("glitch_hunt.rules");
/// Bumper Cars.
pub const BUMPER_CARS: &str = include_str!("bumper_cars.rules");

/// The built-in scripts of the default Live lineup, in lineup order: each a
/// settings script (`scoring carrier`) of its engine, so the carrier's own
/// rules score the game. A hopper entry's variant is the record of one
/// (`variants::portable::record_for`), which is mode 2 and names the script
/// by its hash. `key` is the name Live's admin API gives the variant
/// (`live_proto::hopper::defaults::builtin_variants`).
pub const MATCHMAKING: &[Matchmaking] = &[
    Matchmaking { key: "rumble_slayer", engine: 2, source: include_str!("matchmaking/rumble_slayer.rules") },
    Matchmaking { key: "team_slayer", engine: 2, source: include_str!("matchmaking/team_slayer.rules") },
    Matchmaking { key: "team_snipers", engine: 2, source: include_str!("matchmaking/team_snipers.rules") },
    Matchmaking { key: "oddball", engine: 3, source: include_str!("matchmaking/oddball.rules") },
    Matchmaking { key: "teamball", engine: 3, source: include_str!("matchmaking/teamball.rules") },
    Matchmaking { key: "king_of_the_hill", engine: 4, source: include_str!("matchmaking/king_of_the_hill.rules") },
    Matchmaking { key: "team_king", engine: 4, source: include_str!("matchmaking/team_king.rules") },
    Matchmaking { key: "classic_ctf", engine: 1, source: include_str!("matchmaking/classic_ctf.rules") },
    Matchmaking { key: "assault", engine: 9, source: include_str!("matchmaking/assault.rules") },
    Matchmaking { key: "territories", engine: 8, source: include_str!("matchmaking/territories.rules") },
];

/// One script of [`MATCHMAKING`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Matchmaking {
    pub key: &'static str,
    /// The carrier's engine index.
    pub engine: u8,
    pub source: &'static str,
}

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

/// The scripts that are built-in variants, in list order: the entries the
/// game's variant list gets at load (`docs/game/rules-variants.md`,
/// "Built-in list entries"). `engine` is the carrier's engine index. The
/// scripted modes after the settings scripts (Superbounce, Siege, Gun Game, Race, VIP) are
/// examples too, so `examples()` names them by key.
pub const LISTED: &[Builtin] = &[
    Builtin { name: "Zombies", engine: 2, source: ZOMBIES },
    Builtin { name: "Hide and Seek", engine: 7, source: HIDE_AND_SEEK },
    Builtin { name: "Jousting", engine: 2, source: JOUSTING },
    Builtin { name: "SWAT", engine: 2, source: SWAT },
    Builtin { name: "Duel", engine: 2, source: DUEL },
    Builtin { name: "Clint Eastwood", engine: 7, source: CLINT_EASTWOOD },
    Builtin { name: "Sun King", engine: 4, source: SUN_KING },
    Builtin { name: "Tower of Power", engine: 2, source: TOWER_OF_POWER },
    Builtin { name: "Wheeeeee!", engine: 3, source: WHEEEEEE },
    Builtin { name: "Cops and Robbers", engine: 2, source: COPS_AND_ROBBERS },
    Builtin { name: "Bounce Practice", engine: 2, source: BOUNCE_PRACTICE },
    Builtin { name: "Glitch Hunt", engine: 2, source: GLITCH_HUNT },
    Builtin { name: "Bumper Cars", engine: 2, source: BUMPER_CARS },
    Builtin { name: "Superbounce", engine: 2, source: SUPERBOUNCE },
    Builtin { name: "Siege", engine: 2, source: SIEGE },
    Builtin { name: "Gun Game", engine: 2, source: GUN_GAME },
    Builtin { name: "Race", engine: 2, source: RACE },
    Builtin { name: "VIP", engine: 2, source: VIP },
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
        ("bounce_hunt", BOUNCE_HUNT),
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
    fn every_matchmaking_script_is_a_settings_script_of_its_engine() {
        for m in MATCHMAKING {
            let ir = crate::lang::compile(m.source).unwrap_or_else(|e| panic!("{}: {e}", m.key));
            assert_eq!(ir.header.carrier, m.engine, "{}", m.key);
            assert_eq!(ir.header.scoring, crate::ir::Scoring::Carrier, "{}", m.key);
            assert!(crate::verify::verify(&ir).unwrap().fits(), "{}", m.key);
            // The key is the name's snake case.
            assert_eq!(ir.header.name.to_lowercase().replace(' ', "_"), m.key);
            // Every one clears the hopper defaults writer's three settings.
            for name in ["setting_twelve", "setting_thirteen", "join_in_progress"] {
                let (at, _) = crate::settings::named(name).unwrap();
                assert!(
                    ir.settings.iter().any(|s| s.index == at && s.value == 0),
                    "{}: {name}",
                    m.key
                );
            }
        }
        let mut hashes: Vec<_> = MATCHMAKING
            .iter()
            .map(|m| crate::lang::compile(m.source).unwrap().id().0)
            .collect();
        hashes.sort();
        hashes.dedup();
        assert_eq!(hashes.len(), MATCHMAKING.len(), "two scripts are one blob");
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
    fn sources_of_a_file_are_the_baked_scripts() {
        let ctf = include_str!("ctf.rules");
        assert_eq!(sources_of("ctf.rules", ctf), vec![CTF.to_string(), ASSAULT.to_string()]);
        assert_eq!(sources_of("slayer.rules", SLAYER), vec![SLAYER.to_string()]);
    }

    #[test]
    fn ctf_and_assault_share_a_body_and_differ_by_header() {
        let ctf = crate::lang::compile(CTF).unwrap();
        let assault = crate::lang::compile(ASSAULT).unwrap();
        assert_eq!(ctf.handlers, assault.handlers);
        assert_ne!(ctf.id(), assault.id());
    }
}
