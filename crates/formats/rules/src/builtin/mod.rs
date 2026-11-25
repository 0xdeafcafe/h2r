//! Built-in scripts, compiled into every build from the files of the `core`
//! and `modes` packages (`content/core/rules/`, `content/modes/rules/`):
//! one per retail game type,
//! found by type (a `uses` line) and by engine index (a record with no
//! mode), and the payoff modes as example scripts. See
//! `docs/game/rules-engine.md`, "A record with no mode", "Porting the
//! seven engines" and "Payoff", and
//! `docs/game/game-types-without-carriers.md`.

use crate::types::Type;

mod registry;
pub use registry::{EXAMPLE_KEYS, LISTED_FILES, Registry, Role, Script, Source, package_sources};

/// Slayer (engine index 2): kill, suicide and betrayal scoring.
pub const SLAYER: &str = include_str!("../../../../../content/core/rules/slayer.rules");
/// Oddball (engine index 3): time held scores.
pub const ODDBALL: &str = include_str!("../../../../../content/core/rules/oddball.rules");
/// King of the Hill (engine index 4): time on the hill scores.
pub const KING: &str = include_str!("../../../../../content/core/rules/koth.rules");
/// Juggernaut (engine index 7): the killer of the Juggernaut takes the role.
pub const JUGGERNAUT: &str = include_str!("../../../../../content/core/rules/juggernaut.rules");
/// Territories (engine index 8): held territories score.
pub const TERRITORIES: &str = include_str!("../../../../../content/core/rules/territories.rules");
// The headers `ctf.rules` gets for CTF and Assault (`ctf.head` and
// `assault.head` beside it in the `core` package): macros, so `concat!`
// takes them and [`sources_of`] uses the same text. `const assault` tells
// the shared body which game it plays.
macro_rules! ctf_header {
    () => {
        include_str!("../../../../../content/core/rules/ctf.head")
    };
}
macro_rules! assault_header {
    () => {
        include_str!("../../../../../content/core/rules/assault.head")
    };
}

/// Capture the Flag (engine index 1). `ctf.rules` serves CTF and Assault;
/// each gets its own header, so each is its own type.
pub const CTF: &str = concat!(ctf_header!(), include_str!("../../../../../content/core/rules/ctf.rules"));
/// Assault (engine index 9), from the same body as [`CTF`].
pub const ASSAULT: &str = concat!(assault_header!(), include_str!("../../../../../content/core/rules/ctf.rules"));

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
pub const INFECTION: &str = include_str!("../../../../../content/modes/rules/infection.rules");
/// Gun Game: a weapon ladder, one rung a kill.
pub const GUN_GAME: &str = include_str!("../../../../../content/modes/rules/gun_game.rules");
/// VIP: a VIP per team, worth more to kill, the role passing on.
pub const VIP: &str = include_str!("../../../../../content/modes/rules/vip.rules");
/// Siege: the phased attack-and-defend mode.
pub const SIEGE: &str = include_str!("../../../../../content/modes/rules/siege.rules");
/// Race: numbered checkpoints, a point a lap.
pub const RACE: &str = include_str!("../../../../../content/modes/rules/race.rules");
/// Bounce Hunt: points for superbounces.
pub const BOUNCE_HUNT: &str = include_str!("../../../../../content/modes/rules/bounce_hunt.rules");
/// Superbounce: the superbounce mode's guide, and points for bounces only:
/// a kill scores nothing. Its settings block is the listed variant's.
pub const SUPERBOUNCE: &str = include_str!("../../../../../content/modes/rules/superbounce.rules");

// The listed variants, the community's classics: each a settings script
// (`uses`) except Superbounce. See `LISTED`.
/// Zombies: honour rules, in `about`.
pub const ZOMBIES: &str = include_str!("../../../../../content/modes/rules/zombies.rules");
/// Hide and Seek.
pub const HIDE_AND_SEEK: &str = include_str!("../../../../../content/modes/rules/hide_and_seek.rules");
/// Jousting.
pub const JOUSTING: &str = include_str!("../../../../../content/modes/rules/jousting.rules");
/// SWAT.
pub const SWAT: &str = include_str!("../../../../../content/modes/rules/swat.rules");
/// Duel.
pub const DUEL: &str = include_str!("../../../../../content/modes/rules/duel.rules");
/// Clint Eastwood.
pub const CLINT_EASTWOOD: &str = include_str!("../../../../../content/modes/rules/clint_eastwood.rules");
/// Sun King.
pub const SUN_KING: &str = include_str!("../../../../../content/modes/rules/sun_king.rules");
/// Tower of Power.
pub const TOWER_OF_POWER: &str = include_str!("../../../../../content/modes/rules/tower_of_power.rules");
/// Wheeeeee!
pub const WHEEEEEE: &str = include_str!("../../../../../content/modes/rules/wheeeeee.rules");
/// Cops and Robbers.
pub const COPS_AND_ROBBERS: &str = include_str!("../../../../../content/modes/rules/cops_and_robbers.rules");
/// Bounce Practice: settings only, no score.
pub const BOUNCE_PRACTICE: &str = include_str!("../../../../../content/modes/rules/bounce_practice.rules");
/// Glitch Hunt.
pub const GLITCH_HUNT: &str = include_str!("../../../../../content/modes/rules/glitch_hunt.rules");
/// Bumper Cars.
pub const BUMPER_CARS: &str = include_str!("../../../../../content/modes/rules/bumper_cars.rules");

/// The built-in scripts of the default Live lineup, in lineup order: each a
/// settings script that `uses` its type's built-in game, which scores it. A hopper entry's variant is the record of one
/// (`variants::portable::record_for`), which is mode 2 and names the script
/// by its hash. `key` is the name Live's admin API gives the variant
/// (`live_proto::hopper::defaults::builtin_variants`).
pub const MATCHMAKING: &[Matchmaking] = &[
    Matchmaking { key: "rumble_slayer", engine: 2, source: include_str!("../../../../../content/modes/rules/matchmaking/rumble_slayer.rules") },
    Matchmaking { key: "team_slayer", engine: 2, source: include_str!("../../../../../content/modes/rules/matchmaking/team_slayer.rules") },
    Matchmaking { key: "team_snipers", engine: 2, source: include_str!("../../../../../content/modes/rules/matchmaking/team_snipers.rules") },
    Matchmaking { key: "team_heavies", engine: 2, source: include_str!("../../../../../content/modes/rules/matchmaking/team_heavies.rules") },
    Matchmaking { key: "oddball", engine: 3, source: include_str!("../../../../../content/modes/rules/matchmaking/oddball.rules") },
    Matchmaking { key: "teamball", engine: 3, source: include_str!("../../../../../content/modes/rules/matchmaking/teamball.rules") },
    Matchmaking { key: "king_of_the_hill", engine: 4, source: include_str!("../../../../../content/modes/rules/matchmaking/king_of_the_hill.rules") },
    Matchmaking { key: "team_king", engine: 4, source: include_str!("../../../../../content/modes/rules/matchmaking/team_king.rules") },
    Matchmaking { key: "classic_ctf", engine: 1, source: include_str!("../../../../../content/modes/rules/matchmaking/classic_ctf.rules") },
    Matchmaking { key: "assault", engine: 9, source: include_str!("../../../../../content/modes/rules/matchmaking/assault.rules") },
    Matchmaking { key: "territories", engine: 8, source: include_str!("../../../../../content/modes/rules/matchmaking/territories.rules") },
];

/// One script of [`MATCHMAKING`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Matchmaking {
    pub key: &'static str,
    /// The engine index its record is on (`ir::Header::engine`).
    pub engine: u8,
    pub source: &'static str,
}

/// A built-in script: its name, the engine index its record is on
/// (`ir::Header::engine`) and its source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub name: &'static str,
    pub engine: u8,
    pub source: &'static str,
}

/// The type-area rows of the settings table a built-in takes as an option
/// of its own, by the row's name and the option's. A retail record with no
/// script plays its type's built-in with these options read from its type
/// area (`variants::portable::retail_options`); a script-index record sets
/// the options directly and has a zero type area.
pub const RETAIL_OPTIONS: &[(&str, &str)] = &[
    ("oddball_ball_count", "ball_count"),
    ("ctf_flag_type", "flag_type"),
    ("ctf_reset_time", "reset_time"),
    ("ctf_touch_return", "touch_return"),
    ("ctf_flag_at_home", "flag_at_home"),
    ("assault_arm_time", "arm_time"),
    ("king_moving_hill", "move_time"),
    ("territories_count", "in_play"),
    ("territories_contest_time", "contest_time"),
];

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
/// "Built-in list entries"). `engine` is its record's engine index. The
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

/// The built-in script of a retail type: the game a `uses` line plays.
pub fn of_type(ty: Type) -> Option<&'static str> {
    if ty.is_retail() { builtin(ty.engine()) } else { None }
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
            assert_eq!(ir.header.engine(), index);
            assert_eq!(ir.header.uses, None, "engine {index}");
            assert_eq!(of_type(ir.header.game_type), Some(src), "engine {index}");
            let report = crate::verify::verify(&ir).unwrap();
            assert!(report.fits(), "engine {index}: worst tick {}", report.worst_tick);
            // A port declares what it does itself: a carried objective's
            // script returns it by removing it (`delete`), as no engine
            // slot runs for it, and the objective ports show their state
            // in widgets and waypoints.
            let wants: &[&str] = match index {
                1 | 3 | 9 => &["hud.waypoints", "hud.widgets", "objects.delete"],
                4 | 8 => &["hud.waypoints", "hud.widgets"],
                _ => &[],
            };
            let mut requires = ir.header.requires.clone();
            requires.sort();
            assert_eq!(requires, wants, "engine {index}");
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
    fn every_matchmaking_script_is_a_settings_script_of_its_type() {
        for m in MATCHMAKING {
            let ir = crate::lang::compile(m.source).unwrap_or_else(|e| panic!("{}: {e}", m.key));
            assert_eq!(ir.header.engine(), m.engine, "{}", m.key);
            assert_eq!(ir.header.uses, Some(ir.header.game_type), "{}", m.key);
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
    fn every_listed_and_example_script_has_its_type_and_engine() {
        for b in LISTED {
            let ir = crate::lang::compile(b.source).unwrap_or_else(|e| panic!("{}: {e}", b.name));
            assert_eq!(ir.header.engine(), b.engine, "{}", b.name);
        }
        let ty = |src: &str| crate::lang::compile(src).unwrap().header.game_type;
        assert_eq!(ty(SIEGE), Type::Siege);
        assert_eq!(ty(SUPERBOUNCE), Type::Research);
        assert_eq!(ty(ZOMBIES), Type::Party);
        assert_eq!(ty(SWAT), Type::Slayer);
        let race = crate::lang::compile(RACE).unwrap();
        assert_eq!(race.header.points, "Laps");
        assert_eq!(crate::lang::compile(CTF).unwrap().header.points, "Captures");
        let hide = crate::lang::compile(HIDE_AND_SEEK).unwrap();
        assert_eq!((hide.header.game_type, hide.header.uses), (Type::Party, Some(Type::Juggernaut)));
        for t in crate::types::TABLE {
            assert_eq!(of_type(t.ty).is_some(), t.ty.is_retail(), "{}", t.name);
        }
    }

    /// The option bits a script's record carries (`ir::OPTION_BITS`).
    fn option_bits(ir: &crate::Ir) -> u32 {
        ir.options.iter().map(crate::ir::OptionDef::bits).sum()
    }

    #[test]
    fn each_retail_game_fits_its_type_area_in_the_option_bits() {
        // GT7 of docs/game/game-types-without-carriers.md reads a retail
        // record's type-area settings into the script's options. Each
        // type-area row of the settings table, at the record's own range,
        // and the built-in's options must fit the record's 32 option bits
        // together (docs/game/notes/2026-11-24-game-type-engine-reads.md).
        // A row a built-in has an option for (`RETAIL_OPTIONS`, by the row's name and
        // the option's) is that option's bits, counted once.
        let width = |(lo, hi): (i32, i32)| 64 - ((hi - lo).max(0) as u64).leading_zeros();
        let mut counted = Vec::new();
        for b in ALL {
            let ir = crate::lang::compile(b.source).unwrap();
            let has = |name: &str| (0..ir.options.len()).any(|i| ir.option_name(i) == name);
            let area: u32 = crate::settings::TABLE
                .iter()
                .filter(|r| !r.carriers.is_empty() && r.applies(b.engine))
                .filter(|r| !RETAIL_OPTIONS.iter().any(|&(row, option)| row == r.name && has(option)))
                .map(|r| width(r.kind.range()))
                .sum();
            let total = option_bits(&ir) + area;
            assert!(total <= crate::ir::OPTION_BITS, "{}: {total} bits", b.name);
            counted.push((b.engine, option_bits(&ir), area));
        }
        // (engine, the built-in's options, the type area).
        assert_eq!(
            counted,
            [(1, 20, 0), (2, 4, 0), (3, 8, 3), (4, 9, 0), (7, 5, 6), (8, 12, 0), (9, 20, 0)]
        );
        for b in LISTED.iter().chain(&[Builtin { name: "Infection", engine: 2, source: INFECTION }]) {
            let ir = crate::lang::compile(b.source).unwrap();
            assert!(option_bits(&ir) <= crate::ir::OPTION_BITS, "{}", b.name);
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
    fn sources_of_a_file_are_the_baked_scripts() {
        let ctf = include_str!("../../../../../content/core/rules/ctf.rules");
        assert_eq!(sources_of("ctf.rules", ctf), vec![CTF.to_string(), ASSAULT.to_string()]);
        assert_eq!(sources_of("slayer.rules", SLAYER), vec![SLAYER.to_string()]);
    }

    #[test]
    fn ctf_and_assault_share_a_body_and_differ_by_header() {
        let ctf = crate::lang::compile(CTF).unwrap();
        let assault = crate::lang::compile(ASSAULT).unwrap();
        // One body, so the same handlers in the same order; the header's
        // `const assault` is folded into their code, which is how they differ.
        let events = |ir: &crate::ir::Ir| ir.handlers.iter().map(|h| h.event.clone()).collect::<Vec<_>>();
        assert_eq!(events(&ctf), events(&assault));
        assert_ne!(ctf.handlers, assault.handlers);
        assert_ne!(ctf.id(), assault.id());
        let body = include_str!("../../../../../content/core/rules/ctf.rules");
        assert!(CTF.ends_with(body) && ASSAULT.ends_with(body));
    }
}
