//! The lobby's mode (docs/game/forge-firefight-ui.md, "The lobbies"):
//! Multiplayer, Forge or Firefight, chosen on Game Setup's Mode row
//! ([`crate::lobby_mode`]), and the maps each mode plays.
//!
//! **The mode** is held for the lobby the player is in: [`set_mode`] from
//! the Mode row's picker, [`clear`] when the lobby is left (the party's
//! leave, or the main menu or Xbox Live menu built again), which puts it
//! back to Multiplayer, the mode a lobby opens in. Until the menus are
//! first reached there is no lobby at all: a game the level boot starts
//! ([`crate::level_boot`]) is a Forge session when it is local
//! multiplayer ([`forge_session`]), so an agent can drive one.
//!
//! **Maps.** Forge opens on an original map: the catalogue's Halo 2
//! category ([`forge_maps`]). Firefight needs a map that carries the kit,
//! which `h2 firefight kit` installs as a package named `Firefight – <map>`
//! (`firefight-kit`'s `build_package`): the catalogue lists it by that
//! name ([`is_kit_map`]) in its Firefight category. A map the kit's probe
//! recipe made is named `Firefight Probe – <map>` and is not offered.
//!
//! **The guest-call queue** the Forge session submits its game calls to
//! is found at boot ([`install_queue`]).

use std::sync::Mutex;

use agent::sim::calls;

use crate::map_select::{Catalogue, Category, Map};

/// A kit package's name in the catalogue: `firefight-kit`'s package name
/// for its regular mode (`Firefight – <map>`, with an en dash).
pub const KIT_NAME_PREFIX: &str = "Firefight \u{2013} ";

/// Whether `map` carries the Firefight kit.
pub fn is_kit_map(map: &Map) -> bool {
    map.name.starts_with(KIT_NAME_PREFIX)
}

/// The original maps Forge opens on: the disc's own, in the game's order.
pub fn forge_maps(catalogue: &Catalogue) -> Vec<Map> {
    catalogue.maps(Category::Halo2).to_vec()
}

/// The installed maps that carry the Firefight kit.
pub fn kit_maps(catalogue: &Catalogue) -> Vec<Map> {
    catalogue.all().filter(|m| is_kit_map(m)).cloned().collect()
}

/// What the maps say about a Firefight lobby: whether the lobby's map
/// carries the kit, and whether any installed map does. Without the map
/// list (not read yet) neither is known, and both are false.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KitMaps {
    /// The lobby's map carries the kit.
    pub lobby: bool,
    /// An installed map carries the kit.
    pub installed: bool,
}

impl KitMaps {
    /// For the lobby on map `map` (its list id), with `catalogue` the map
    /// list.
    pub fn of(catalogue: Option<&Catalogue>, map: Option<u32>) -> KitMaps {
        let Some(c) = catalogue else {
            return KitMaps::default();
        };
        KitMaps {
            lobby: map.is_some_and(|id| c.all().any(|m| m.id == id && is_kit_map(m))),
            installed: c.all().any(is_kit_map),
        }
    }

    /// For the lobby as it stands: the map list read so far
    /// (`map_select::current`) and the lobby's map (`LOBBY_MAP`).
    pub fn now(m: &dyn engine::memory::GuestMemory) -> KitMaps {
        let catalogue = crate::map_select::current();
        let map = m.u32(crate::h1_options_row::LOBBY_MAP).ok();
        KitMaps::of(catalogue.as_deref(), map)
    }
}

// ---- the mode ----

/// What a lobby plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The game's own multiplayer: the mode a lobby opens in.
    #[default]
    Multiplayer,
    /// A Forge session: D-pad Up enters the monitor
    /// (docs/game/forge-editing.md, "Controls").
    Forge,
    /// Firefight on a kit map (docs/game/firefight.md).
    Firefight,
}

impl Mode {
    /// Every mode, in the picker's order.
    pub const ALL: [Mode; 3] = [Mode::Multiplayer, Mode::Forge, Mode::Firefight];

    /// Its name in the picker and on the Mode row's help.
    pub const fn label(self) -> &'static str {
        match self {
            Mode::Multiplayer => "Multiplayer",
            Mode::Forge => "Forge",
            Mode::Firefight => "Firefight",
        }
    }

    /// What it plays, one sentence.
    pub const fn description(self) -> &'static str {
        match self {
            Mode::Multiplayer => "The game's own rules.",
            Mode::Forge => "Build and edit the map in a game, then save it.",
            Mode::Firefight => "Fight waves of enemies on a Firefight map.",
        }
    }

    /// What it plays, one sentence that names it, for the picker.
    pub const fn summary(self) -> &'static str {
        match self {
            Mode::Multiplayer => "Multiplayer plays the game's own rules.",
            Mode::Forge => "Forge builds and edits the map in a game.",
            Mode::Firefight => "Firefight fights waves of enemies on a Firefight map.",
        }
    }
}

/// Which modes a lobby offers: Multiplayer always, Forge with the `forge`
/// feature on, Firefight with the `firefight` feature on and a settings
/// store holding its choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    pub forge: bool,
    pub firefight: bool,
}

impl Offer {
    /// The modes on offer, in the picker's order.
    pub fn modes(self) -> Vec<Mode> {
        Mode::ALL
            .into_iter()
            .filter(|m| match m {
                Mode::Multiplayer => true,
                Mode::Forge => self.forge,
                Mode::Firefight => self.firefight,
            })
            .collect()
    }

    /// Whether Game Setup has the Mode row: there is a mode besides
    /// Multiplayer to pick. Without one the lobby is retail's.
    pub fn any(self) -> bool {
        self.forge || self.firefight
    }
}

/// The lobby the player is in, as the mode sees it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The menus have been reached since boot: a game starts from a lobby.
    /// `false` only before them, for the level boot's game.
    pub lobby: bool,
    pub mode: Mode,
    /// The saved Forge map the session loads (`None`: a new map).
    pub forge_map: Option<String>,
    /// The map variant of the lobby's map the match plays (`None`:
    /// Original), set with [`crate::map_variant`].
    pub map_variant: Option<content::MapId>,
    /// The lobby's kit map's base name (`beavercreek`), as the Firefight
    /// page's Wave set row reads it between redraws ([`kit_map`]); the
    /// lobby panel keeps it ([`crate::lobby_mode`]).
    pub kit_map: Option<String>,
}

impl State {
    /// Whether a game started now is a Forge session: the lobby is in
    /// Forge mode, or there is no lobby (the level boot).
    pub fn forge_session(&self) -> bool {
        !self.lobby || self.mode == Mode::Forge
    }
}

#[cfg(not(test))]
static STATE: Mutex<State> = Mutex::new(State {
    lobby: false,
    mode: Mode::Multiplayer,
    forge_map: None,
    map_variant: None,
    kit_map: None,
});

#[cfg(not(test))]
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()))
}

// Tests run side by side, each on its own thread: each has its own lobby.
#[cfg(test)]
thread_local! {
    static STATE: Mutex<State> = const {
        Mutex::new(State {
            lobby: false,
            mode: Mode::Multiplayer,
            forge_map: None,
            map_variant: None,
            kit_map: None,
        })
    };
}

#[cfg(test)]
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.lock().unwrap_or_else(|e| e.into_inner())))
}

/// The lobby as it stands.
pub fn state() -> State {
    with_state(|s| s.clone())
}

/// The lobby's mode (Multiplayer before any lobby).
pub fn mode() -> Mode {
    with_state(|s| s.mode)
}

/// Whether a game started now is a Forge session ([`State::forge_session`]).
pub fn forge_session() -> bool {
    with_state(|s| s.forge_session())
}

/// The Mode row's choice for the lobby the player is in.
pub fn set_mode(mode: Mode) {
    with_state(|s| {
        if s.mode != mode {
            eprintln!("lobby: mode {}", mode.label());
        }
        s.lobby = true;
        s.mode = mode;
    });
}

/// The saved Forge map the next Forge session loads (`None`: a new map).
pub fn forge_map() -> Option<String> {
    with_state(|s| s.forge_map.clone())
}

/// The Forge Map row's choice.
pub fn set_forge_map(name: Option<String>) {
    with_state(|s| {
        s.lobby = true;
        s.forge_map = name;
    });
}

/// The map variant the lobby plays (`None`: Original).
pub fn map_variant() -> Option<content::MapId> {
    with_state(|s| s.map_variant)
}

/// The lobby's map variant ([`crate::map_variant`] sets it with its pick).
pub fn set_map_variant(id: Option<content::MapId>) {
    with_state(|s| s.map_variant = id);
}

/// The lobby's kit map's base name, as it stood at the panel's last draw
/// ([`crate::lobby_mode`]): the Wave set row reads it between redraws.
pub fn kit_map() -> Option<String> {
    with_state(|s| s.kit_map.clone())
}

/// Keeps the lobby's kit map's base name ([`kit_map`]).
pub fn set_kit_map(map: Option<String>) {
    with_state(|s| s.kit_map = map);
}

/// The lobby map's base name now (`beavercreek`), when it carries the
/// kit: the lobby's map in the catalogue, named `Firefight – <map>`
/// ([`KIT_NAME_PREFIX`]). `None` when the list is not read, there is no
/// lobby map, or it is not a kit map (the probe's `Firefight Probe –
/// <map>` is not one).
pub fn kit_map_name(m: &dyn engine::memory::GuestMemory) -> Option<String> {
    let id = m.u32(crate::h1_options_row::LOBBY_MAP).ok()?;
    let catalogue = crate::map_select::current()?;
    let map = catalogue.all().find(|map| map.id == id)?;
    map.name.strip_prefix(KIT_NAME_PREFIX).map(str::to_owned)
}

/// The lobby as a snapshot held it.
pub fn restore(state: State) {
    with_state(|s| *s = state);
}

/// The lobby was left, or the menus reached: the next lobby opens in
/// Multiplayer with a new Forge map.
pub fn clear() {
    with_state(|s| {
        if s.mode != Mode::Multiplayer {
            eprintln!("lobby: left; mode Multiplayer");
        }
        *s = State {
            lobby: true,
            ..State::default()
        };
    });
    crate::map_variant::forget();
}

// ---- the guest-call queue ----

/// Finds the guest-call queue the Forge session submits to: the agent's,
/// else one of our own with the feature that makes its calls.
pub fn install_queue(
    registry: &mut hooks::Registry,
    agent: Option<&crate::agent_glue::AgentGlue>,
) -> anyhow::Result<calls::Shared> {
    if let Some(sim) = agent.and_then(|a| a.sim.as_ref()) {
        return Ok(sim.calls().unwrap_or_else(calls::shared));
    }
    let queue = calls::shared();
    registry
        .add(&calls::CallsFeature(queue.clone()))
        .map_err(anyhow::Error::msg)?;
    Ok(queue)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(id: u32, name: &str, path: &str) -> Map {
        Map {
            id,
            name: name.into(),
            sort: 0,
            path: path.into(),
        }
    }

    #[test]
    fn only_the_kit_packages_carry_the_kit() {
        assert!(is_kit_map(&map(
            1,
            "Firefight \u{2013} Coagulation",
            r"t:\$C\1\coagulation"
        )));
        assert!(!is_kit_map(&map(
            2,
            "Firefight Probe \u{2013} Coagulation",
            ""
        )));
        assert!(!is_kit_map(&map(3, "Coagulation", r"d:\maps\coagulation")));
        assert!(
            !is_kit_map(&map(4, "Firefight - Coagulation", "")),
            "a hyphen is not the kit's dash"
        );
    }

    #[test]
    fn forge_lists_the_discs_maps_and_firefight_the_kit_packages() {
        let cat = Catalogue::new([
            map(10, "Lockout", r"d:\maps\lockout"),
            map(11, "Ascension", r"d:\maps\ascension"),
            map(12, "Firefight \u{2013} Lockout", r"t:\$C\12345678\lockout"),
            map(13, "Some Mod", r"t:\$C\22345678\midship"),
        ]);
        let forge: Vec<_> = forge_maps(&cat).into_iter().map(|m| m.id).collect();
        assert!(forge.contains(&10) && forge.contains(&11));
        assert!(
            !forge.contains(&12) && !forge.contains(&13),
            "a package is not an original"
        );
        let kit: Vec<_> = kit_maps(&cat).into_iter().map(|m| m.id).collect();
        assert_eq!(kit, [12]);
        let firefight: Vec<_> = cat.maps(Category::Firefight).iter().map(|m| m.id).collect();
        assert_eq!(firefight, [12], "the kit maps are the Firefight category");
    }

    #[test]
    fn kit_maps_say_whether_the_lobby_and_the_install_can_play_firefight() {
        let cat = Catalogue::new([
            map(10, "Lockout", r"d:\maps\lockout"),
            map(12, "Firefight \u{2013} Lockout", r"t:\$C\12345678\lockout"),
        ]);
        let bare = Catalogue::new([map(10, "Lockout", r"d:\maps\lockout")]);
        assert_eq!(
            KitMaps::of(Some(&cat), Some(12)),
            KitMaps { lobby: true, installed: true }
        );
        assert_eq!(
            KitMaps::of(Some(&cat), Some(10)),
            KitMaps { lobby: false, installed: true }
        );
        assert_eq!(KitMaps::of(Some(&bare), Some(10)), KitMaps::default());
        assert!(!KitMaps::of(Some(&cat), None).lobby);
        assert_eq!(KitMaps::of(None, Some(12)), KitMaps::default(), "no map list");
    }

    #[test]
    fn mode_lists_only_enabled_features() {
        let cases = [
            (false, false, vec![Mode::Multiplayer]),
            (true, false, vec![Mode::Multiplayer, Mode::Forge]),
            (false, true, vec![Mode::Multiplayer, Mode::Firefight]),
            (true, true, Mode::ALL.to_vec()),
        ];
        for (forge, firefight, want) in cases {
            let offer = Offer { forge, firefight };
            assert_eq!(offer.modes(), want, "forge {forge}, firefight {firefight}");
            assert_eq!(offer.any(), forge || firefight);
        }
    }

    #[test]
    fn leaving_the_lobby_clears_the_mode() {
        set_mode(Mode::Forge);
        set_forge_map(Some("Castle".into()));
        assert_eq!(mode(), Mode::Forge);
        assert_eq!(forge_map().as_deref(), Some("Castle"));
        clear();
        assert_eq!(
            state(),
            State {
                lobby: true,
                mode: Mode::Multiplayer,
                forge_map: None,
                map_variant: None,
                kit_map: None,
            }
        );
    }

    #[test]
    fn the_monitor_needs_forge_mode() {
        assert!(
            forge_session(),
            "no lobby yet: the level boot's game is a Forge session"
        );
        clear();
        assert!(!forge_session(), "a lobby opens in Multiplayer");
        for (m, forge) in [
            (Mode::Forge, true),
            (Mode::Firefight, false),
            (Mode::Multiplayer, false),
        ] {
            set_mode(m);
            assert_eq!(forge_session(), forge, "{m:?}");
        }
    }

    #[test]
    fn each_mode_has_its_copy() {
        for m in Mode::ALL {
            let label = m.label();
            assert!(!label.contains(':') && !label.contains("  "), "{label}");
            assert!(m.description().ends_with('.'), "{m:?}");
            assert!(m.summary().starts_with(label), "{m:?}");
        }
        assert_eq!(Mode::default(), Mode::Multiplayer);
    }
}
