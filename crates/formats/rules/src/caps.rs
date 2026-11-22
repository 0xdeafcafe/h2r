//! The capability list and the engine API entry table, fixed per API
//! version. See `docs/game/rules-engine.md`, "The engine API" and "Roles,
//! powers and capabilities".

use crate::ty::Ty;

/// The API version this build implements. A blob naming another is refused.
pub const API_VERSION: u16 = 1;

/// A named group of API actions a script must declare in `requires`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    pub name: &'static str,
    /// The line the lobby and the variant screen show. A script supplies no
    /// text about its own powers.
    pub words: &'static str,
    /// Whether a script may `grant` it to a player.
    pub grantable: bool,
    /// The API version that added it.
    pub since: u16,
}

const fn cap(name: &'static str, words: &'static str, grantable: bool) -> Capability {
    Capability {
        name,
        words,
        grantable,
        since: 1,
    }
}

/// Every capability of [`API_VERSION`].
pub const CAPABILITIES: &[Capability] = &[
    cap("game.traits", "changes gravity, damage, shields and speed", false),
    cap("player.traits", "changes gravity, damage, shields and speed", false),
    cap("player.team", "moves players between teams", false),
    cap("player.spawn", "chooses where players spawn", false),
    cap("player.weapons", "gives and takes weapons", false),
    cap("player.disguise", "hides or changes a player's body", false),
    cap("player.camera", "changes the camera", false),
    cap("objects.create", "makes objects", false),
    cap("objects.delete", "removes objects", false),
    cap("objects.move", "moves objects", false),
    cap("ai.squads", "places and commands AI", false),
    cap("hud.message", "shows messages", false),
    cap(
        "hud.superbounce",
        "shows the superbounce guide: a priming HUD, marks over the map and bounce medals",
        false,
    ),
    cap("firefight.director", "runs Firefight rounds, waves, lives and scores", false),
    cap("forge.move", "lets players move objects during the match", true),
    cap("forge.delete", "lets players remove objects during the match", true),
    cap("forge.place", "lets players place objects during the match", true),
    cap("forge.scale", "lets players resize objects during the match", true),
];

/// The capability named `name` in `api`, if that version has it.
pub fn capability(name: &str, api: u16) -> Option<&'static Capability> {
    CAPABILITIES
        .iter()
        .find(|c| c.name == name && c.since <= api)
}

/// Whether an entry is a read, an action, or a VM built-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Read,
    Action,
    /// Reaches no engine entry; the VM answers it.
    VmOnly,
}

/// When an action's call is made ("Order").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// At once, where the handler runs.
    Now,
    /// In program order by the `before` on the next `GAME_TICK`.
    Queued,
}

/// How far the entry behind a row is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// A point or layout in `guest-layout` or `engine`.
    Named,
    /// An address in a doc, no named point yet.
    Traced,
    /// No known entry.
    Untraced,
    /// The game-side part (labels, zones, detector) is not built.
    NotBuilt,
    /// Answered by the VM.
    VmOnly,
}

impl Status {
    /// Whether a host can make the call. A host refuses a script that calls
    /// any other entry, naming it; nothing runs with a call stubbed out.
    pub fn hostable(self) -> bool {
        matches!(self, Status::Named | Status::VmOnly)
    }
}

/// One numbered API entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The number a `Call` names; equal to the entry's index in [`ENTRIES`].
    pub id: u16,
    pub name: &'static str,
    pub args: &'static [Ty],
    pub result: Option<Ty>,
    /// The capability a script must declare; `None` is `core`.
    pub cap: Option<&'static str>,
    pub kind: Kind,
    pub order: Order,
    pub status: Status,
    /// The API version that added it.
    pub since: u16,
}

const fn read(id: u16, name: &'static str, args: &'static [Ty], result: Ty, status: Status) -> Entry {
    Entry {
        id,
        name,
        args,
        result: Some(result),
        cap: None,
        kind: Kind::Read,
        order: Order::Now,
        status,
        since: 1,
    }
}

const fn act(
    id: u16,
    name: &'static str,
    args: &'static [Ty],
    cap: Option<&'static str>,
    order: Order,
    status: Status,
) -> Entry {
    Entry {
        id,
        name,
        args,
        result: None,
        cap,
        kind: Kind::Action,
        order,
        status,
        since: 1,
    }
}

const fn vm(id: u16, name: &'static str, args: &'static [Ty], result: Ty) -> Entry {
    Entry {
        id,
        name,
        args,
        result: Some(result),
        cap: None,
        kind: Kind::VmOnly,
        order: Order::Now,
        status: Status::VmOnly,
        since: 1,
    }
}

use self::Order::{Now, Queued};
use self::Status::{Named, NotBuilt, Traced, Untraced};
use crate::ty::Ty::{Bool, Class, Fixed, Int, Label, Object, Player, Squad, Tag, Team, Text};

pub mod id {
    //! Entry numbers the compiler and VM name directly.
    pub const PLAYER_SCORE: u16 = 0;
    pub const TEAM_SCORE: u16 = 1;
    pub const KILLS: u16 = 2;
    pub const DEATHS: u16 = 3;
    pub const ALIVE: u16 = 4;
    pub const TEAM_OF: u16 = 5;
    pub const LIVES: u16 = 6;
    pub const ROUND_TIME_LEFT: u16 = 7;
    pub const IS_OBJECTIVE: u16 = 8;
    pub const LABEL_NUMBER: u16 = 9;
    pub const LABEL_OF: u16 = 10;
    pub const ZONE_ID: u16 = 11;
    pub const TICKS_HELD: u16 = 12;
    pub const SQUAD_ALIVE: u16 = 13;
    pub const SCORE: u16 = 14;
    pub const END_ROUND: u16 = 15;
    pub const SET_LIVES: u16 = 16;
    pub const RESPAWN: u16 = 17;
    pub const SET_GRAVITY: u16 = 18;
    pub const SET_TEAM: u16 = 19;
    pub const SET_LOADOUT: u16 = 20;
    pub const GIVE: u16 = 21;
    pub const TAKE: u16 = 22;
    pub const CREATE: u16 = 23;
    pub const DELETE: u16 = 24;
    pub const REGISTER_OBJECTIVE: u16 = 25;
    pub const PLACE_SQUAD: u16 = 26;
    pub const HUNT: u16 = 27;
    pub const ERASE_SQUAD: u16 = 28;
    pub const SPAWN_AREAS: u16 = 29;
    pub const MESSAGE: u16 = 30;
    pub const FORGE_MOVE: u16 = 31;
    pub const FORGE_DELETE: u16 = 32;
    pub const FORGE_PLACE: u16 = 33;
    pub const FORGE_SCALE: u16 = 34;
    pub const CLAMP: u16 = 35;
    pub const MIN: u16 = 36;
    pub const MAX: u16 = 37;
    pub const SQUAD: u16 = 38;
    pub const CARRIER_OF: u16 = 39;
    pub const OBJECT_TEAM: u16 = 40;
    pub const INSIDE: u16 = 41;
    pub const PLAYER_OF: u16 = 42;
    pub const SPAWN_OBJECTIVE: u16 = 43;
    pub const RESET_OBJECTIVE: u16 = 44;
    pub const ROLE_TRAITS: u16 = 45;
    pub const NOW: u16 = 46;
    pub const RANDOM: u16 = 47;
    pub const ZONE_SEEN: u16 = 48;
    pub const ZONE_DISTANCE: u16 = 49;
    pub const PLACE_SQUAD_AT: u16 = 50;
    pub const FF_ROUND: u16 = 51;
    pub const FF_WAVE: u16 = 52;
    pub const FF_SKULL: u16 = 53;
    pub const FF_LIVES: u16 = 54;
    pub const FF_SCORE: u16 = 55;
    pub const FF_RESPAWN: u16 = 56;
    pub const FF_FINISH: u16 = 57;
    pub const MESSAGE_TEAM: u16 = 58;
    pub const MESSAGE_ALL: u16 = 59;
}

/// Every entry of [`API_VERSION`], indexed by number.
pub const ENTRIES: &[Entry] = &[
    // Reads.
    read(id::PLAYER_SCORE, "player_score", &[Player], Int, Named),
    read(id::TEAM_SCORE, "team_score", &[Team], Int, Named),
    read(id::KILLS, "kills", &[Player], Int, Named),
    read(id::DEATHS, "deaths", &[Player], Int, Named),
    read(id::ALIVE, "alive", &[Player], Bool, Named),
    read(id::TEAM_OF, "team_of", &[Player], Team, Named),
    read(id::LIVES, "lives", &[Player], Int, Named),
    read(id::ROUND_TIME_LEFT, "round_time_left", &[], Int, Named),
    read(id::IS_OBJECTIVE, "is_objective", &[Object], Bool, Named),
    read(id::LABEL_NUMBER, "label_number", &[Object], Int, NotBuilt),
    read(id::LABEL_OF, "label_of", &[Object], Label, NotBuilt),
    read(id::ZONE_ID, "zone_id", &[Object], Int, NotBuilt),
    // Answered by the VM from the zone test (`Game::zone_holder`), which
    // no host has yet: the VM's own, over a game-side part not built.
    Entry {
        status: NotBuilt,
        ..vm(id::TICKS_HELD, "ticks_held", &[Object, Team], Int)
    },
    read(id::SQUAD_ALIVE, "squad_alive", &[Squad], Int, Named),
    // Actions.
    act(id::SCORE, "score", &[Player, Int], None, Now, Named),
    // GAME_END is named; advancing a round and naming the winner are not
    // traced (rules-engine.md, "Round end and the winner").
    act(id::END_ROUND, "end_round", &[Team], None, Now, Named),
    act(id::SET_LIVES, "set_lives", &[Player, Int], None, Now, Named),
    act(id::RESPAWN, "respawn", &[Player], None, Now, Named),
    act(id::SET_GRAVITY, "set_gravity", &[Fixed], Some("game.traits"), Now, Named),
    act(id::SET_TEAM, "set_team", &[Player, Team], Some("player.team"), Now, Untraced),
    act(id::SET_LOADOUT, "set_loadout", &[Player, Tag, Tag], Some("player.weapons"), Now, Traced),
    act(id::GIVE, "give", &[Player, Tag], Some("player.weapons"), Now, Untraced),
    act(id::TAKE, "take", &[Player, Object], Some("player.weapons"), Now, Traced),
    act(id::CREATE, "create", &[Tag, Object], Some("objects.create"), Queued, Traced),
    act(id::DELETE, "delete", &[Object], Some("objects.delete"), Queued, Named),
    act(id::REGISTER_OBJECTIVE, "register_objective", &[Object, Team], None, Now, Traced),
    act(id::PLACE_SQUAD, "place_squad", &[Squad, Class, Int, Label], Some("ai.squads"), Queued, Named),
    act(id::HUNT, "hunt", &[Squad], Some("ai.squads"), Queued, Named),
    act(id::ERASE_SQUAD, "erase_squad", &[Squad], Some("ai.squads"), Queued, Named),
    act(id::SPAWN_AREAS, "spawn_areas", &[Team, Label, Int], Some("player.spawn"), Now, Untraced),
    // `message(to, text)`: the language picks this entry for a player,
    // `message_team` for a team and `message_all` for `all` ([`spelled`]).
    act(id::MESSAGE, "message", &[Player, Text], Some("hud.message"), Now, Untraced),
    act(id::FORGE_MOVE, "forge_move", &[Object, Object], Some("forge.move"), Queued, Named),
    act(id::FORGE_DELETE, "forge_delete", &[Object], Some("forge.delete"), Queued, Named),
    act(id::FORGE_PLACE, "forge_place", &[Tag, Object], Some("forge.place"), Queued, Named),
    act(id::FORGE_SCALE, "forge_scale", &[Object, Fixed], Some("forge.scale"), Queued, Named),
    // VM built-ins.
    vm(id::CLAMP, "clamp", &[Int, Int, Int], Int),
    vm(id::MIN, "min", &[Int, Int], Int),
    vm(id::MAX, "max", &[Int, Int], Int),
    vm(id::SQUAD, "squad", &[Int], Squad),
    // The objective entries the engine ports need (rules-engine.md,
    // "Porting the seven engines").
    // An objective record's carrying player, `+0xc` (`engine::objectives`).
    read(id::CARRIER_OF, "carrier_of", &[Object], Player, Named),
    // An objective record's team (`+4`), or a netgame flag placement's team
    // through its designator (`E+0x10`); neutral (8) reads as none.
    read(id::OBJECT_TEAM, "object_team", &[Object], Team, Traced),
    // The player's living unit inside a zone object: a labelled volume, a
    // territory marker's capture volume, or a hill corner's hill.
    read(id::INSIDE, "inside", &[Player, Object], Bool, NotBuilt),
    // The player whose unit an object is (unit `+0x13c`).
    read(id::PLAYER_OF, "player_of", &[Object], Player, Traced),
    // Create an objective at a placement and register it with a team, as
    // the CTF/Assault flag spawn `0x2422d0` and Oddball's `0x2bc1f0` do.
    act(id::SPAWN_OBJECTIVE, "spawn_objective", &[Tag, Object, Team], None, Queued, Traced),
    // Return an objective to where it spawned; the lifted engines' return
    // path is not traced.
    act(id::RESET_OBJECTIVE, "reset_objective", &[Object], None, Queued, Untraced),
    // Apply (or clear) the carrier engine's role traits from the variant's
    // type area: ball or flag carrier, the Juggernaut (with its bit of the
    // role mask `[0x510c9c]+0`). Each engine answers traits through its
    // vtable slot `+0x8c` (docs/engine/notes/2026-11-21-engine-vtables.md);
    // which traits each role gets is not read.
    act(id::ROLE_TRAITS, "role_traits", &[Player, Bool], None, Now, Untraced),
    // The tick the host began (`Vm::begin_tick`).
    vm(id::NOW, "now", &[], Int),
    // The VM generator's next draw modulo `n`; 0, with no draw, for n <= 0.
    vm(id::RANDOM, "random", &[Int], Int),
    // Firefight's drop zones, as the objects labelled `ff_drop` in kit
    // order: the host's `firefight::director::ZoneSeen` per zone.
    read(id::ZONE_SEEN, "zone_seen", &[Object], Bool, Named),
    read(id::ZONE_DISTANCE, "zone_distance", &[Object], Int, Named),
    // `firefight::Command::PlaceSquad`: a kit zone index and a weapon tier.
    act(id::PLACE_SQUAD_AT, "place_squad_at", &[Squad, Class, Int, Int, Int], Some("ai.squads"), Queued, Named),
    // The Firefight director's host log entries (`firefight::Command`),
    // made at the tick boundary in program order with the squad entries.
    act(id::FF_ROUND, "ff_round", &[Int], Some("firefight.director"), Queued, Named),
    act(id::FF_WAVE, "ff_wave", &[Int], Some("firefight.director"), Queued, Named),
    act(id::FF_SKULL, "ff_skull", &[Int, Bool], Some("firefight.director"), Queued, Named),
    act(id::FF_LIVES, "ff_lives", &[Player, Int], Some("firefight.director"), Queued, Named),
    act(id::FF_SCORE, "ff_score", &[Player, Int, Bool], Some("firefight.director"), Queued, Named),
    act(id::FF_RESPAWN, "ff_respawn", &[Player], Some("firefight.director"), Queued, Named),
    act(id::FF_FINISH, "ff_finish", &[Bool], Some("firefight.director"), Queued, Named),
    // The team and everyone forms of `message(to, text)`.
    act(id::MESSAGE_TEAM, "message_team", &[Team, Text], Some("hud.message"), Now, Untraced),
    act(id::MESSAGE_ALL, "message_all", &[Text], Some("hud.message"), Now, Untraced),
];

/// The word `message(to, text)` takes for everyone.
pub const EVERYONE: &str = "all";

/// The name the language spells an entry with: `message` for each of the
/// three forms of `message(to, text)`, its own name for the rest.
pub fn spelled(e: &Entry) -> &'static str {
    match e.id {
        id::MESSAGE_TEAM | id::MESSAGE_ALL => "message",
        _ => e.name,
    }
}

/// An entry a script names only through another's spelling (the team and
/// everyone forms of `message`). Completion and the API listing leave it
/// out, and calling it by its own name is an unknown function.
pub fn is_form(e: &Entry) -> bool {
    spelled(e) != e.name
}

/// The parameters an entry's signature shows, as the language spells them.
pub fn params(e: &Entry) -> Vec<String> {
    match e.id {
        id::MESSAGE | id::MESSAGE_TEAM | id::MESSAGE_ALL => {
            vec![format!("to: player | team | {EVERYONE}"), "text: text".into()]
        }
        _ => e.args.iter().map(|t| t.name().to_string()).collect(),
    }
}

/// The entry numbered `id` in `api`.
pub fn entry(id: u16, api: u16) -> Option<&'static Entry> {
    ENTRIES.get(id as usize).filter(|e| e.since <= api)
}

/// The entry the language calls `name`. For `message` it is the player
/// form; the compiler picks the form by the first argument.
pub fn entry_named(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.name == name && !is_form(e))
}

/// Retail record fields a script reads as `variant.<name>`, with the type
/// it reads them as. The host maps each onto `variants::record`. New
/// fields are appended: a blob names a field by its index.
///
/// The per-engine fields read the type area (`+0xf0..+0x10c`), whose
/// meaning depends on the engine; each names its `record::SettingId` and
/// offset. Meanings marked inferred come from the editor's page labels
/// against `record::field` (docs/editor/variants.md), not from a reader.
pub const VARIANT_FIELDS: &[(&str, Ty)] = &[
    ("team_play", Ty::Bool),
    ("score_limit", Ty::Int),
    ("time_limit", Ty::Int),
    ("lives", Ty::Int),
    ("respawn_time", Ty::Int),
    ("friendly_fire", Ty::Bool),
    ("rounds", Ty::Int),
    // The engine index, `+0x44` (a CTF and an Assault game share a script).
    ("engine", Ty::Int),
    // Oddball: setting 43, i16 `+0xf4`, 0..3 (inferred: Ball Count).
    ("oddball_ball_count", Ty::Int),
    // King: setting 40, i16 `+0xf4`, seconds until the hill moves, 0 never
    // (the KotH globals' `+0x1a4` counts it, bots.md 3.7).
    ("king_moving_hill", Ty::Int),
    // CTF and Assault: settings 50 and 59, i32 `+0x104`, 0..2; 1 is one
    // flag or one bomb (`G+0x240 == 1`, bots.md 3.7).
    ("ctf_flag_type", Ty::Int),
    // CTF and Assault: settings 54 and 61, i32 `+0xf4`, seconds
    // (inferred: Flag Reset Time, Bomb Reset Time).
    ("ctf_reset_time", Ty::Int),
    // CTF and Assault: settings 53 and 62, type-area bit 3 (inferred: Flag
    // Touch Return, Bomb Touch Return).
    ("ctf_touch_return", Ty::Bool),
    // CTF: setting 52, type-area bit 2 (inferred: Flag At Home To Score).
    ("ctf_flag_at_home", Ty::Bool),
    // Assault: setting 63, i16 `+0x108`, seconds to arm (`G+0x244`,
    // bots.md 3.7).
    ("assault_arm_time", Ty::Int),
    // Territories: setting 74, i16 `+0xf0`, 1..8 (Territory Count).
    ("territories_count", Ty::Int),
    // Territories: setting 98, i16 `+0xf2`, seconds (inferred: the
    // contest time).
    ("territories_contest_time", Ty::Int),
];

/// The script's carrier engine: retail engine indices
/// (`variants::record::GameType`).
pub const CARRIERS: &[(&str, u8)] = &[
    ("ctf", 1),
    ("slayer", 2),
    ("oddball", 3),
    ("king", 4),
    ("juggernaut", 7),
    ("territories", 8),
    ("assault", 9),
];

/// The tick rate `seconds(n)` multiplies by.
pub const TICKS_PER_SECOND: i32 = 30;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_numbered_by_index() {
        for (index, entry) in ENTRIES.iter().enumerate() {
            assert_eq!(entry.id as usize, index, "{}", entry.name);
        }
    }

    #[test]
    fn names_are_unique_and_caps_exist() {
        for (i, a) in ENTRIES.iter().enumerate() {
            for b in &ENTRIES[i + 1..] {
                assert_ne!(a.name, b.name);
            }
            if let Some(cap) = a.cap {
                assert!(capability(cap, API_VERSION).is_some(), "{cap}");
            }
            // A VM built-in is the VM's own, unless its game-side part is
            // not built (`ticks_held` over the zone test).
            if a.status == Status::VmOnly {
                assert_eq!(a.kind, Kind::VmOnly, "{}", a.name);
            }
            if a.kind == Kind::VmOnly {
                assert!(matches!(a.status, Status::VmOnly | Status::NotBuilt), "{}", a.name);
            }
            assert!(a.kind != Kind::Read || a.cap.is_none());
        }
        for (i, a) in CAPABILITIES.iter().enumerate() {
            for b in &CAPABILITIES[i + 1..] {
                assert_ne!(a.name, b.name);
            }
            assert_eq!(a.grantable, a.name.starts_with("forge."));
        }
    }

    /// `hud.superbounce` gates no entry: a host turns its superbounce mode
    /// on for a script that declares it, and every kind of game takes it.
    #[test]
    fn hud_superbounce_is_a_declared_host_feature() {
        let c = capability("hud.superbounce", API_VERSION).unwrap();
        assert!(!c.grantable);
        assert!(ENTRIES.iter().all(|e| e.cap != Some(c.name)));
        let req = vec![c.name.to_string()];
        assert_eq!(
            crate::policy::eligible(&req, API_VERSION),
            ["custom", "hopper", "ranked"]
        );
    }

    #[test]
    fn unknown_names_and_versions_are_absent() {
        assert!(capability("net.socket", API_VERSION).is_none());
        assert!(capability("forge.move", 0).is_none());
        assert!(entry(ENTRIES.len() as u16, API_VERSION).is_none());
        assert!(entry(id::SCORE, 0).is_none());
        assert!(!entry(id::SET_TEAM, 1).unwrap().status.hostable());
        assert!(entry(id::SCORE, 1).unwrap().status.hostable());
    }

    #[test]
    fn message_has_three_forms_under_one_name() {
        for f in [id::MESSAGE, id::MESSAGE_TEAM, id::MESSAGE_ALL] {
            let e = entry(f, API_VERSION).unwrap();
            assert_eq!(spelled(e), "message");
            assert_eq!(e.cap, Some("hud.message"));
            assert_eq!(params(e), ["to: player | team | all", "text: text"]);
        }
        assert_eq!(entry_named("message").unwrap().id, id::MESSAGE);
        assert_eq!(entry_named("message").unwrap().args, &[Player, Text]);
        assert!(entry_named("message_team").is_none());
        assert!(entry_named("message_all").is_none());
        assert!(!is_form(entry(id::SCORE, API_VERSION).unwrap()));
    }

    #[test]
    fn ticks_held_is_the_vms_over_a_zone_test_not_built() {
        let e = entry(id::TICKS_HELD, API_VERSION).unwrap();
        assert_eq!(e.kind, Kind::VmOnly);
        assert_eq!(e.status, Status::NotBuilt);
        assert!(!e.status.hostable());
        assert_eq!(e.args, &[Object, Team]);
    }
}
