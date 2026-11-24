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

impl Entry {
    /// Whether a host can make the call in a script whose record is on the
    /// engine `carrier` (`ir::Header::engine`).
    /// A host refuses a script that calls an entry it cannot, naming the
    /// entry; nothing runs with a call stubbed out.
    pub fn hostable_on(&self, carrier: u8) -> bool {
        self.status.hostable() && self.hosts.is_none_or(|h| h.contains(&carrier))
    }
}

/// The name of the retail engine `engine` ([`CARRIERS`]), or `None` for an
/// index no script's record is on.
pub fn carrier_name(engine: u8) -> Option<&'static str> {
    CARRIERS.iter().find(|&&(_, e)| e == engine).map(|&(n, _)| n)
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
    cap("bots.orders", "gives bots orders", false),
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
    /// The carriers (retail engine indices, [`CARRIERS`]) a host makes the
    /// call on; `None` for every carrier. The engine routine behind such an
    /// entry is one engine's own (`reset_objective` returns an Oddball).
    pub hosts: Option<&'static [u8]>,
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
        hosts: None,
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
        hosts: None,
    }
}

/// A message hole's value ("Messages" in
/// `docs/game/rules-language-types.md`): a VM built-in with no result
/// that queues its argument for the next `message`.
const fn fmt(id: u16, name: &'static str, args: &'static [Ty]) -> Entry {
    Entry {
        id,
        name,
        args,
        result: None,
        cap: None,
        kind: Kind::VmOnly,
        order: Order::Now,
        status: Status::VmOnly,
        since: 1,
        hosts: None,
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
        hosts: None,
    }
}

use self::Order::{Now, Queued};

/// The carriers of the entries one engine's routine makes.
const ON_ODDBALL: &[u8] = &[3];

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
    pub const FMT_PLAYER: u16 = 60;
    pub const FMT_TEAM: u16 = 61;
    pub const FMT_INT: u16 = 62;
    pub const FMT_TIME: u16 = 63;
    pub const FMT_ENUM: u16 = 64;
    pub const BOT_GO: u16 = 65;
    pub const BOT_HOLD: u16 = 66;
    pub const BOT_FOLLOW: u16 = 67;
    pub const BOT_ATTACK: u16 = 68;
    pub const BOT_FREE: u16 = 69;
    pub const ZONE_TEAM_DISTANCE: u16 = 70;
    pub const FF_TARGET: u16 = 71;
    pub const FF_TEAM_LIVES: u16 = 72;
    pub const FF_PLAYER_SCORE: u16 = 73;
    pub const FF_TEAM_FINISH: u16 = 74;
    pub const SPAWN_NEAR: u16 = 75;
    pub const SET_BODY: u16 = 76;
    pub const SHOW_AREA: u16 = 77;
    pub const HIDE_AREA: u16 = 78;
    pub const OBJECT_HEALTH: u16 = 79;
    pub const OBJECT_SHIELD: u16 = 80;
    pub const TAG_INT: u16 = 81;
    pub const TAG_FIXED: u16 = 82;
    pub const SQUAD_FIGHTING: u16 = 83;
    pub const SQUAD_STRENGTH: u16 = 84;
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
    // A labelled thing's number, label and placement id: the host's table
    // of the map's labelled things (the netgame flags in guest memory and
    // the session's Forge document; `game::rules::census`).
    read(id::LABEL_NUMBER, "label_number", &[Object], Int, Named),
    read(id::LABEL_OF, "label_of", &[Object], Label, Named),
    read(id::ZONE_ID, "zone_id", &[Object], Int, Named),
    // Answered by the VM from the zone test (`Game::zone_holder`); the
    // host's is `game::rules::world::zones`, over the labelled flags.
    vm(id::TICKS_HELD, "ticks_held", &[Object, Team], Int),
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
    // A weapon of the multiplayer weapon list (`crate::weapons`): its tag
    // through `STARTING_WEAPON_PICK` (`0x19eda0`) by its setting value, then
    // made and added to the unit as `sim.give` does (`0xb7930`, `0xb7b40`,
    // `0xcd0c0`; `game::rules::world::loadout`).
    act(id::SET_LOADOUT, "set_loadout", &[Player, Tag, Tag], Some("player.weapons"), Now, Named),
    act(id::GIVE, "give", &[Player, Tag], Some("player.weapons"), Now, Untraced),
    act(id::TAKE, "take", &[Player, Object], Some("player.weapons"), Now, Traced),
    act(id::CREATE, "create", &[Tag, Object], Some("objects.create"), Queued, Traced),
    act(id::DELETE, "delete", &[Object], Some("objects.delete"), Queued, Named),
    act(id::REGISTER_OBJECTIVE, "register_objective", &[Object, Team], None, Now, Traced),
    act(id::PLACE_SQUAD, "place_squad", &[Squad, Class, Int, Label], Some("ai.squads"), Queued, Named),
    act(id::HUNT, "hunt", &[Squad], Some("ai.squads"), Queued, Named),
    act(id::ERASE_SQUAD, "erase_squad", &[Squad], Some("ai.squads"), Queued, Named),
    // Which starting locations a team's spawns may use: those of the
    // label numbered `n`. Kept in the VM's state (`vm::Spawns`); the host
    // answers the spawn chooser's free-spot test `SPAWN_LOCATION_FREE`
    // (`0x23bb70`, called only from `PLAYER_SPAWN_LOCATION` `0x14ef00`)
    // with "blocked" for every other location, so the chooser's own
    // weighing picks among the live ones (`game::rules::world::spawns`;
    // docs/game/notes/2026-11-24-siege-spawns-and-bodies.md).
    act(id::SPAWN_AREAS, "spawn_areas", &[Team, Label, Int], Some("player.spawn"), Now, Named),
    // `message(to, text)`: the language picks this entry for a player,
    // `message_team` for a team and `message_all` for `all` ([`spelled`]).
    // The host shows it to its local users through the notification insert
    // (`hud_kit::NOTIFY`, E `0x24cbee`) and sends it to clients as a show
    // cue (docs/game/rules-world-api.md, "The show channel").
    act(id::MESSAGE, "message", &[Player, Text], Some("hud.message"), Now, Named),
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
    // through its designator (`E+0x10`, `engine::objectives::DESIGNATOR_TEAMS`);
    // neutral (8) reads as none.
    read(id::OBJECT_TEAM, "object_team", &[Object], Team, Named),
    // The player's living unit inside a zone object: the host's zone test
    // (`game::rules::world::zones`): the flag's area, else the hull of a
    // numbered label's corners, else a cylinder.
    read(id::INSIDE, "inside", &[Player, Object], Bool, Named),
    // The player whose unit an object is: the roster's units (the player
    // datum's unit), so no unit field is read.
    read(id::PLAYER_OF, "player_of", &[Object], Player, Named),
    // Create an objective at a placement and register it with a team, as
    // the CTF/Assault flag spawn `0x2422d0` and Oddball's `0x2bc1f0` do.
    act(id::SPAWN_OBJECTIVE, "spawn_objective", &[Tag, Object, Team], None, Queued, Traced),
    // Return an objective to where it spawned. A host makes it on Oddball,
    // through the engine's own ball return `ODDBALL_BALL_RETURN` `0x2bc340`
    // (the ball's spawn flag, then the shared objective return `0x15e360`);
    // the other engines' return paths are not named
    // (docs/engine/notes/2026-11-22-script-scoring-and-roles.md).
    Entry {
        hosts: Some(ON_ODDBALL),
        ..act(id::RESET_OBJECTIVE, "reset_objective", &[Object], None, Queued, Named)
    },
    // The VM records membership. The host answers the trait query for role holders.
    act(id::ROLE_TRAITS, "role_traits", &[Player, Bool], None, Now, Named),
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
    act(id::MESSAGE_TEAM, "message_team", &[Team, Text], Some("hud.message"), Now, Named),
    act(id::MESSAGE_ALL, "message_all", &[Text], Some("hud.message"), Now, Named),
    // The values of a message's holes, in hole order, directly before the
    // `message` call that shows them (the message rule, `verify.rs`). The
    // VM queues each and hands them to the message after its own
    // arguments. `fmt_time` takes ticks. `fmt_enum` takes a register of
    // any enum type (its `Enum(0)` stands for every enum, and the verifier
    // checks it apart) and hands the variant's name.
    fmt(id::FMT_PLAYER, "fmt_player", &[Player]),
    fmt(id::FMT_TEAM, "fmt_team", &[Team]),
    fmt(id::FMT_INT, "fmt_int", &[Int]),
    fmt(id::FMT_TIME, "fmt_time", &[Int]),
    fmt(id::FMT_ENUM, "fmt_enum", &[Ty::Enum(0)]),
    // Bot orders (docs/game/rules-world-api.md, "Bots"): a hint to a
    // phantom's brain on the host, delivered through the phantom registry
    // (`game::rules::world::bots`). The whole path is ours — a phantom's
    // pad is a player's input — so there is no guest point to name; named
    // means the game-side part is built here. `bot_go`'s and `bot_hold`'s
    // second argument is a labelled thing's handle (a zone fits an object);
    // the `vec` form of `at` comes with the world API's objects slice.
    act(id::BOT_GO, "bot_go", &[Player, Object], Some("bots.orders"), Queued, Named),
    act(id::BOT_HOLD, "bot_hold", &[Player, Object], Some("bots.orders"), Queued, Named),
    act(id::BOT_FOLLOW, "bot_follow", &[Player, Player], Some("bots.orders"), Queued, Named),
    act(id::BOT_ATTACK, "bot_attack", &[Player, Player], Some("bots.orders"), Queued, Named),
    act(id::BOT_FREE, "bot_free", &[Player], Some("bots.orders"), Queued, Named),
    // Firefight with two player teams (docs/game/firefight.md, "Two
    // teams"): a drop zone's distance from one team's players
    // (`ZoneSeen::team_distance`), the team a placed squad is sent at
    // (`Command::Target`), a team's pool (`Command::TeamLives`), a player
    // kill across the teams (`ScoreReason::PlayerKill`), and the end with a
    // winning team, `none` for a draw (`Outcome::TeamWon`, `Outcome::Draw`).
    read(id::ZONE_TEAM_DISTANCE, "zone_team_distance", &[Object, Team], Int, Named),
    act(id::FF_TARGET, "ff_target", &[Squad, Team], Some("firefight.director"), Queued, Named),
    act(id::FF_TEAM_LIVES, "ff_team_lives", &[Team, Int], Some("firefight.director"), Queued, Named),
    act(id::FF_PLAYER_SCORE, "ff_player_score", &[Player, Int], Some("firefight.director"), Queued, Named),
    act(id::FF_TEAM_FINISH, "ff_team_finish", &[Team], Some("firefight.director"), Queued, Named),
    // A team's spawns use only the `n` starting locations nearest a zone
    // (`n` 0 lifts it): `spawn_areas` for a map with no labelled starting
    // locations, through the same answer of `SPAWN_LOCATION_FREE`.
    act(id::SPAWN_NEAR, "spawn_near", &[Team, Object, Int], Some("player.spawn"), Now, Named),
    // A team's players spawn in a body of the list (`crate::bodies`), or
    // their own (`none`). Kept in the VM's state (`vm::RulesExt::bodies`);
    // the host writes the body's model byte (player `+0x88`) for the
    // length of `PLAYER_SPAWN` (`0x14f2e0`), whose representation pick
    // `0x14f040` and unit handover `0x14cad0` read it
    // (`game::rules::world::body`).
    act(id::SET_BODY, "set_body", &[Team, Tag], Some("player.disguise"), Now, Named),
    // An area's wall drawn in play, in a team's colour (`none`: neutral),
    // and taken off: at most eight at once, kept in the VM's state
    // (`vm::RulesExt::shown`) so every machine and a film draw the same
    // (`game::rules::world::walls`). A display only, so no capability.
    act(id::SHOW_AREA, "show_area", &[Object, Team], None, Now, Named),
    act(id::HIDE_AREA, "hide_area", &[Object], None, Now, Named),
    // Advanced data (docs/game/rules-world-api.md, "Advanced data"). A
    // unit's body and shield as a fraction of full, 0 to 1, from the unit
    // datum (`game::rules::world::data`); an object that is no unit, or is
    // gone, reads 0.
    read(id::OBJECT_HEALTH, "object_health", &[Object], Fixed, Named),
    read(id::OBJECT_SHIELD, "object_shield", &[Object], Fixed, Named),
    // A field of a tag the map holds: both arguments are written as
    // constants, and the field path is resolved at load against our typed
    // tag definitions (`fields`), so a field they lack is a load error
    // naming it. A float field is rounded once to `fixed`. The host's
    // read of a tag instance from the loaded map is not built, so a host
    // refuses a script that calls either by name.
    read(id::TAG_INT, "tag_int", &[Tag, Text], Int, Named),
    read(id::TAG_FIXED, "tag_fixed", &[Tag, Text], Fixed, Named),
    // Actor state beyond what the agent API's `world.ai` snapshot reads is
    // not traced, so a host refuses a script that reads either by name.
    read(id::SQUAD_FIGHTING, "squad_fighting", &[Squad], Bool, Untraced),
    read(id::SQUAD_STRENGTH, "squad_strength", &[Squad], Fixed, Untraced),
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

/// An entry a script names only through another's spelling: the team and
/// everyone forms of `message`, and the `fmt_*` entries of its holes.
/// Completion leaves it out, and calling it by its own name is an unknown
/// function.
pub fn is_form(e: &Entry) -> bool {
    spelled(e) != e.name || Hole::of_entry(e.id).is_some()
}

/// Whether `entry` is one of the three forms of `message(to, text)`.
pub fn is_message(entry: u16) -> bool {
    matches!(entry, id::MESSAGE | id::MESSAGE_TEAM | id::MESSAGE_ALL)
}

/// The values one message shows at most.
pub const MAX_HOLES: usize = 4;

/// The type of a message hole. A message template stores each hole as its
/// word (`{player} scored, {int} to go`); `{{` and `}}` are a brace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Hole {
    Player,
    Team,
    Int,
    /// Ticks, shown as minutes and seconds ([`show_time`]).
    Time,
    /// An enum variant, shown by its name.
    Enum,
}

impl Hole {
    pub const ALL: [Hole; 5] = [Hole::Player, Hole::Team, Hole::Int, Hole::Time, Hole::Enum];

    /// The word a template stores the hole as.
    pub fn word(self) -> &'static str {
        match self {
            Hole::Player => "player",
            Hole::Team => "team",
            Hole::Int => "int",
            Hole::Time => "time",
            Hole::Enum => "enum",
        }
    }

    pub fn from_word(word: &str) -> Option<Hole> {
        Hole::ALL.into_iter().find(|h| h.word() == word)
    }

    /// The `fmt_*` entry that queues a value for the hole.
    pub fn entry(self) -> u16 {
        match self {
            Hole::Player => id::FMT_PLAYER,
            Hole::Team => id::FMT_TEAM,
            Hole::Int => id::FMT_INT,
            Hole::Time => id::FMT_TIME,
            Hole::Enum => id::FMT_ENUM,
        }
    }

    /// The hole a `fmt_*` entry queues a value for.
    pub fn of_entry(entry: u16) -> Option<Hole> {
        Hole::ALL.into_iter().find(|h| h.entry() == entry)
    }
}

/// A piece of a message's text: literal text (braces unescaped), or a hole
/// with the text inside its braces and the char offset of its `{`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    Lit(String),
    Hole { inner: &'a str, at: usize },
}

/// Why a message's text does not split into pieces: a `{` with no `}`, or
/// a `}` that closes nothing, at a char offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceError {
    Open(usize),
    Close(usize),
}

/// Splits a message's text at its holes: `{x}` is a hole, and `{{` and
/// `}}` are a brace.
pub fn pieces(text: &str) -> Result<Vec<Piece<'_>>, PieceError> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < chars.len() {
        let (byte, c) = chars[i];
        let next = chars.get(i + 1).map(|&(_, n)| n);
        match c {
            '{' | '}' if next == Some(c) => {
                lit.push(c);
                i += 2;
            }
            '{' => {
                let Some(close) = chars[i + 1..].iter().position(|&(_, n)| n == '{' || n == '}') else {
                    return Err(PieceError::Open(i));
                };
                let end = i + 1 + close;
                if chars[end].1 != '}' {
                    return Err(PieceError::Open(i));
                }
                if !lit.is_empty() {
                    out.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                out.push(Piece::Hole {
                    inner: &text[byte + 1..chars[end].0],
                    at: i,
                });
                i = end + 1;
            }
            '}' => return Err(PieceError::Close(i)),
            c => {
                lit.push(c);
                i += 1;
            }
        }
    }
    if !lit.is_empty() {
        out.push(Piece::Lit(lit));
    }
    Ok(out)
}

/// Literal text as a template writes it: each brace doubled.
pub fn escape(text: &str) -> String {
    text.replace('{', "{{").replace('}', "}}")
}

/// The holes of a stored template, in order; `None` when a brace is
/// unpaired, a hole's word is no [`Hole`], or it has more than
/// [`MAX_HOLES`].
pub fn holes(template: &str) -> Option<Vec<Hole>> {
    let mut out = Vec::new();
    for p in pieces(template).ok()? {
        if let Piece::Hole { inner, .. } = p {
            out.push(Hole::from_word(inner)?);
        }
    }
    (out.len() <= MAX_HOLES).then_some(out)
}

/// A template with its holes filled by `values`, in order. A hole with no
/// value shows its word in braces; a text that is no template shows as
/// it is.
pub fn fill(template: &str, values: &[String]) -> String {
    let Ok(list) = pieces(template) else {
        return template.to_string();
    };
    let mut out = String::new();
    let mut next = values.iter();
    for p in list {
        match p {
            Piece::Lit(s) => out.push_str(&s),
            Piece::Hole { inner, .. } => match next.next() {
                Some(v) => out.push_str(v),
                None => {
                    out.push('{');
                    out.push_str(inner);
                    out.push('}');
                }
            },
        }
    }
    out
}

/// Ticks as a message shows a time: whole seconds, rounded down, as
/// `m:ss`; a negative time shows as `0:00`.
pub fn show_time(ticks: i32) -> String {
    let s = ticks.max(0) / TICKS_PER_SECOND;
    format!("{}:{:02}", s / 60, s % 60)
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

/// The retail engines by name and index (`variants::record::GameType`):
/// the engine a script's record is on (`ir::Header::engine`) and the
/// engines an entry is made on (`Entry::hosts`). No script names one; see
/// `docs/game/game-types-without-carriers.md`.
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
            // not built.
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
    fn hole_entries_are_vm_built_ins_reached_through_message() {
        for h in Hole::ALL {
            let e = entry(h.entry(), API_VERSION).unwrap();
            assert_eq!(e.kind, Kind::VmOnly, "{}", e.name);
            assert_eq!(e.result, None, "{}", e.name);
            assert_eq!(e.cap, None, "{}", e.name);
            assert!(e.status.hostable(), "{}", e.name);
            assert!(is_form(e), "{}", e.name);
            assert_eq!(spelled(e), e.name);
            assert!(entry_named(e.name).is_none(), "{}", e.name);
            assert_eq!(Hole::of_entry(e.id), Some(h));
            assert_eq!(Hole::from_word(h.word()), Some(h));
        }
        assert_eq!(Hole::of_entry(id::MESSAGE), None);
        assert!(is_message(id::MESSAGE_ALL) && !is_message(id::FMT_INT));
    }

    #[test]
    fn templates_split_at_holes_and_doubled_braces() {
        assert_eq!(holes("{player} scored, {int} to go"), Some(vec![Hole::Player, Hole::Int]));
        assert_eq!(holes("no holes {{here}}"), Some(vec![]));
        assert_eq!(holes("{object}"), None);
        assert_eq!(holes("{int}{int}{int}{int}{int}"), None);
        assert_eq!(holes("open {player"), None);
        assert_eq!(pieces("a {b"), Err(PieceError::Open(2)));
        assert_eq!(pieces("a } b"), Err(PieceError::Close(2)));
        assert_eq!(pieces("{p {q}"), Err(PieceError::Open(0)));
        assert_eq!(
            pieces("é{p} x"),
            Ok(vec![
                Piece::Lit("é".into()),
                Piece::Hole { inner: "p", at: 1 },
                Piece::Lit(" x".into())
            ])
        );
        assert_eq!(escape("a{b}"), "a{{b}}");
        let filled = fill("{player} scored {{1}}, {int} to go", &["Ann".into(), "3".into()]);
        assert_eq!(filled, "Ann scored {1}, 3 to go");
        assert_eq!(fill("{player} and {team}", &["Ann".into()]), "Ann and {team}");
        assert_eq!(show_time(65 * TICKS_PER_SECOND + 29), "1:05");
        assert_eq!(show_time(-4), "0:00");
    }

    #[test]
    fn an_engine_routines_entry_is_hostable_on_its_carrier_only() {
        let roles = entry(id::ROLE_TRAITS, API_VERSION).unwrap();
        assert!(roles.status.hostable());
        assert!(roles.hostable_on(crate::SCRIPT_INDEX));
        assert!(CARRIERS.iter().all(|&(_, e)| roles.hostable_on(e)));
        let reset = entry(id::RESET_OBJECTIVE, API_VERSION).unwrap();
        assert!(reset.hostable_on(3));
        assert!(!reset.hostable_on(1) && !reset.hostable_on(2));
        // An entry with no carriers named is hostable on every one.
        let score = entry(id::SCORE, API_VERSION).unwrap();
        assert!(CARRIERS.iter().all(|&(_, e)| score.hostable_on(e)));
        assert!(!entry(id::SET_TEAM, API_VERSION).unwrap().hostable_on(2));
        assert_eq!(carrier_name(7), Some("juggernaut"));
        assert_eq!(carrier_name(5), None);
    }

    #[test]
    fn bot_orders_are_queued_hostable_and_gated() {
        for name in ["bot_go", "bot_hold", "bot_follow", "bot_attack", "bot_free"] {
            let e = entry_named(name).unwrap();
            assert_eq!(e.kind, Kind::Action, "{name}");
            assert_eq!(e.order, Order::Queued, "{name}");
            assert_eq!(e.cap, Some("bots.orders"), "{name}");
            assert!(e.status.hostable(), "{name}");
        }
        assert_eq!(entry(id::BOT_HOLD, API_VERSION).unwrap().args, &[Player, Object]);
        assert_eq!(entry(id::BOT_FOLLOW, API_VERSION).unwrap().args, &[Player, Player]);
        assert_eq!(entry(id::BOT_FREE, API_VERSION).unwrap().args, &[Player]);
        let c = capability("bots.orders", API_VERSION).unwrap();
        assert!(!c.grantable);
        assert_eq!(
            crate::policy::eligible(&[c.name.to_string()], API_VERSION),
            ["custom", "hopper", "ranked"]
        );
    }

    #[test]
    fn ticks_held_is_the_vms_over_the_hosts_zone_test() {
        let e = entry(id::TICKS_HELD, API_VERSION).unwrap();
        assert_eq!(e.kind, Kind::VmOnly);
        assert_eq!(e.status, Status::VmOnly);
        assert!(e.status.hostable());
        assert_eq!(e.args, &[Object, Team]);
        // The zone reads and the loadout are made by a host too.
        for name in ["inside", "player_of", "set_loadout"] {
            assert!(entry_named(name).unwrap().status.hostable(), "{name}");
        }
    }

    /// The two-team Firefight entries hold 70 to 74, after the bot orders
    /// (65 to 69), and no other entry shares a number or a name with them:
    /// a compiled Firefight blob names them by number.
    #[test]
    fn two_team_firefight_entries_keep_70_to_74() {
        let ours = [
            (id::ZONE_TEAM_DISTANCE, "zone_team_distance"),
            (id::FF_TARGET, "ff_target"),
            (id::FF_TEAM_LIVES, "ff_team_lives"),
            (id::FF_PLAYER_SCORE, "ff_player_score"),
            (id::FF_TEAM_FINISH, "ff_team_finish"),
        ];
        for (n, (number, name)) in (70u16..).zip(ours) {
            assert_eq!(number, n, "{name}");
            let e = entry(number, API_VERSION).unwrap();
            assert_eq!((e.id, e.name), (n, name));
            let same: Vec<_> = ENTRIES
                .iter()
                .filter(|o| o.id == number || o.name == name)
                .map(|o| o.name)
                .collect();
            assert_eq!(same, [name], "entry {n} is {name} alone");
        }
        assert_eq!(id::BOT_FREE, 69, "the bot orders end just before");
        for name in [
            "ff_target",
            "ff_team_lives",
            "ff_player_score",
            "ff_team_finish",
        ] {
            assert_eq!(
                entry_named(name).unwrap().cap,
                Some("firefight.director"),
                "{name}"
            );
        }
    }
}
