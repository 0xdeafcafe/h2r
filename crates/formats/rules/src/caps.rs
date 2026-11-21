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
    read(id::TICKS_HELD, "ticks_held", &[Object, Team], Int, NotBuilt),
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
    act(id::MESSAGE, "message", &[Text], Some("hud.message"), Now, Untraced),
    act(id::FORGE_MOVE, "forge_move", &[Object, Object], Some("forge.move"), Queued, Named),
    act(id::FORGE_DELETE, "forge_delete", &[Object], Some("forge.delete"), Queued, Named),
    act(id::FORGE_PLACE, "forge_place", &[Tag, Object], Some("forge.place"), Queued, Named),
    act(id::FORGE_SCALE, "forge_scale", &[Object, Fixed], Some("forge.scale"), Queued, Named),
    // VM built-ins.
    vm(id::CLAMP, "clamp", &[Int, Int, Int], Int),
    vm(id::MIN, "min", &[Int, Int], Int),
    vm(id::MAX, "max", &[Int, Int], Int),
    vm(id::SQUAD, "squad", &[Int], Squad),
];

/// The entry numbered `id` in `api`.
pub fn entry(id: u16, api: u16) -> Option<&'static Entry> {
    ENTRIES.get(id as usize).filter(|e| e.since <= api)
}

/// The entry the language calls `name`.
pub fn entry_named(name: &str) -> Option<&'static Entry> {
    ENTRIES.iter().find(|e| e.name == name)
}

/// Retail record fields a script reads as `variant.<name>`, with the type
/// it reads them as. The host maps each onto `variants::record`.
pub const VARIANT_FIELDS: &[(&str, Ty)] = &[
    ("team_play", Ty::Bool),
    ("score_limit", Ty::Int),
    ("time_limit", Ty::Int),
    ("lives", Ty::Int),
    ("respawn_time", Ty::Int),
    ("friendly_fire", Ty::Bool),
    ("rounds", Ty::Int),
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
            assert_eq!(a.kind == Kind::VmOnly, a.status == Status::VmOnly);
            assert!(a.kind != Kind::Read || a.cap.is_none());
        }
        for (i, a) in CAPABILITIES.iter().enumerate() {
            for b in &CAPABILITIES[i + 1..] {
                assert_ne!(a.name, b.name);
            }
            assert_eq!(a.grantable, a.name.starts_with("forge."));
        }
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
}
