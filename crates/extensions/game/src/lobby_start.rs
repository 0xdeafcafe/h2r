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
//! name ([`is_kit_map`]); the map list tags its original map as supporting
//! Firefight ([`crate::map_modes`]). A map the kit's probe
//! recipe made is named `Firefight Probe – <map>` and is not offered.
//!
//! **The guest-call queue** the Forge session submits its game calls to
//! is found at boot ([`install_queue`]).

use std::sync::Mutex;

use agent::sim::calls;

use crate::map_select::{Catalogue, Map};
#[cfg(test)]
use crate::map_select::Category;

/// A kit package's name in the catalogue: `firefight-kit`'s package name
/// for its regular mode (`Firefight – <map>`, with an en dash).
pub const KIT_NAME_PREFIX: &str = "Firefight \u{2013} ";

/// Whether `map` carries the Firefight kit.
pub fn is_kit_map(map: &Map) -> bool {
    map.name.starts_with(KIT_NAME_PREFIX)
}

/// Kit packages on which a variant's director has a wave set. Map Waves
/// ([`::firefight::builtin::MAP_WAVES`]) is on none: a kit package holds
/// no wave set of its own.
pub fn supports_wave(map: &Map, prefix: [u8; 8]) -> bool {
    is_kit_map(map) && ::firefight::builtin::variant(prefix).is_some_and(|wave|
        wave.map.is_none_or(|base| map.short_name().eq_ignore_ascii_case(base)))
}

/// Whether a map variant with AI ground on `map` can play the wave set
/// `wave` (a Firefight game variant's prefix; `None` for the lobby's own
/// choice): a set made for one base map needs that map, as it does of a
/// kit package ([`supports_wave`]). Map Waves plays on any ground pick;
/// whether the pick carries a set is [`map_waves_refusal`]'s.
pub fn supports_ground_wave(map: &Map, wave: Option<[u8; 8]>) -> bool {
    wave.is_none_or(|prefix| {
        prefix == ::firefight::builtin::MAP_WAVES
            || ::firefight::builtin::variant(prefix).is_some_and(|set|
            set.map.is_none_or(|base| map.short_name().eq_ignore_ascii_case(base)))
    })
}

/// Why the Map Waves entry (`wave` is its prefix) cannot start: the lobby's
/// pick carries no wave set of its own (`own`,
/// [`crate::map_variant::waves_on`]). `None` for any other entry, and
/// for Map Waves on a pick with a set.
pub fn map_waves_refusal(wave: Option<[u8; 8]>, own: bool) -> Option<crate::lobby_tabs::Reason> {
    (wave == Some(::firefight::builtin::MAP_WAVES) && !own)
        .then_some(crate::lobby_tabs::Reason::NoOwnWaves)
}

/// The installed multiplayer maps in catalogue order.
pub fn forge_maps(catalogue: &Catalogue) -> Vec<Map> {
    catalogue.all().cloned().collect()
}

/// The installed maps that carry the Firefight kit.
pub fn kit_maps(catalogue: &Catalogue) -> Vec<Map> {
    catalogue.all().filter(|m| is_kit_map(m)).cloned().collect()
}

/// The built-in kit maps' names as the catalogue lists them after
/// [`KIT_NAME_PREFIX`], in the order a Firefight lobby prefers them
/// (`firefight_kit::builtin::MAPS`).
pub const BUILTIN_KIT_MAPS: [&str; 2] = ["Coagulation", "Zanzibar"];

/// The kit map a Firefight lobby lands on: the first built-in map in
/// [`BUILTIN_KIT_MAPS`] order that `kit` has, else its first.
pub fn preferred_kit_map(kit: &[Map]) -> Option<&Map> {
    let rank = |m: &Map| {
        m.name
            .strip_prefix(KIT_NAME_PREFIX)
            .and_then(|n| BUILTIN_KIT_MAPS.iter().position(|b| b.eq_ignore_ascii_case(n)))
            .unwrap_or(BUILTIN_KIT_MAPS.len())
    };
    kit.iter().min_by_key(|m| rank(m))
}

/// The map a Firefight lobby on `current` (its list id) lands on once kit
/// maps are installed: the preferred kit map ([`preferred_kit_map`]), or
/// `None` when the lobby is already on a kit map or none is installed.
pub fn landing_map(catalogue: &Catalogue, current: Option<u32>) -> Option<u32> {
    let kit = kit_maps(catalogue);
    if current.is_some_and(|c| kit.iter().any(|m| m.id == c)) {
        return None;
    }
    preferred_kit_map(&kit).map(|m| m.id)
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
        Self::for_wave(catalogue, map, None)
    }

    pub fn for_wave(catalogue: Option<&Catalogue>, map: Option<u32>, wave: Option<[u8; 8]>) -> Self {
        Self::for_pick(catalogue, map, wave, false)
    }

    /// [`Self::for_wave`] with whether the lobby holds a map variant with
    /// AI ground on its map (`ground`, [`crate::map_variant::ground_on`]):
    /// that map plays Firefight as a kit package does, when the wave set
    /// takes its base map ([`supports_ground_wave`]).
    pub fn for_pick(
        catalogue: Option<&Catalogue>,
        map: Option<u32>,
        wave: Option<[u8; 8]>,
        ground: bool,
    ) -> Self {
        let mut result = Self::default();
        if let Some(catalogue) = catalogue {
            for item in catalogue.all().filter(|m| wave.map_or_else(|| is_kit_map(m), |p| supports_wave(m, p))) {
                result.installed = true;
                if Some(item.id) == map {
                    result.lobby = true;
                    break;
                }
            }
            if ground
                && !result.lobby
                && let Some(item) = catalogue.all().find(|m| Some(m.id) == map)
                && supports_ground_wave(item, wave)
            {
                result.installed = true;
                result.lobby = true;
            }
        }
        result
    }

    /// For the lobby as it stands: the map list read so far
    /// (`map_select::current`) and the lobby's map (`LOBBY_MAP`).
    pub fn now(m: &dyn guest_layout::memory::GuestMemory) -> KitMaps {
        let catalogue = crate::map_select::current();
        let map = m.u32(crate::h1_options_row::LOBBY_MAP).ok();
        KitMaps::for_pick(catalogue.as_deref(), map, None, crate::map_variant::ground_on(map))
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
    /// The classes the lobby's kit map carries besides the authored Elite,
    /// when known ([`kit_classes`]): the Wave set row greys out a set that
    /// needs one the kit lacks. `None`: not known, every set is offered and
    /// the host refuses one the kit cannot play when the match starts.
    pub kit_classes: Option<Vec<String>>,
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
    kit_classes: None,
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
            kit_classes: None,
        })
    };
}

#[cfg(test)]
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.lock().unwrap_or_else(|e| e.into_inner())))
}

// ---- the built-in kits' build ----

/// Where the built-in Firefight kits' build is
/// (`firefight_kit::builtin::ensure`, run by `h2 run`/`h2 serve` at start).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum KitBuild {
    /// No build runs: it is done, was not needed, or never started.
    #[default]
    Idle,
    /// A worker is building; the lobby says it is preparing.
    Building,
    /// The build is done and a kit could not be made: one line for each,
    /// the map and why (`Zanzibar: 6 KB over, sectors`).
    Failed(Vec<String>),
}

#[derive(Default)]
struct Kits {
    build: KitBuild,
    /// A build finished and a Firefight lobby has not been looked at since
    /// ([`take_kit_landing`]).
    landing: bool,
}

#[cfg(not(test))]
static KITS: Mutex<Kits> = Mutex::new(Kits {
    build: KitBuild::Idle,
    landing: false,
});

#[cfg(test)]
thread_local! {
    static KITS: Mutex<Kits> = const {
        Mutex::new(Kits {
            build: KitBuild::Idle,
            landing: false,
        })
    };
}

fn with_kits<T>(f: impl FnOnce(&mut Kits) -> T) -> T {
    #[cfg(not(test))]
    {
        f(&mut KITS.lock().unwrap_or_else(|e| e.into_inner()))
    }
    #[cfg(test)]
    {
        KITS.with(|k| f(&mut k.lock().unwrap_or_else(|e| e.into_inner())))
    }
}

/// The built-in kits' build as it stands.
pub fn kit_build() -> KitBuild {
    with_kits(|k| k.build.clone())
}

/// A worker began building the built-in kits.
pub fn kit_build_started() {
    with_kits(|k| k.build = KitBuild::Building);
}

/// The worker is done; `failures` has a line for each kit it could not
/// make. A Firefight lobby is then looked at once to land on a kit map
/// ([`take_kit_landing`]).
pub fn kit_build_finished(failures: Vec<String>) {
    with_kits(|k| {
        k.build = if failures.is_empty() {
            KitBuild::Idle
        } else {
            KitBuild::Failed(failures)
        };
        k.landing = true;
    });
}

/// Whether a build finished since this was last asked (and forget it).
pub fn take_kit_landing() -> bool {
    with_kits(|k| std::mem::take(&mut k.landing))
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

/// The classes the lobby's kit map carries, when known.
pub fn kit_classes() -> Option<Vec<String>> {
    with_state(|s| s.kit_classes.clone())
}

/// Keeps the lobby's kit map's classes ([`kit_classes`]).
pub fn set_kit_classes(classes: Option<Vec<String>>) {
    with_state(|s| s.kit_classes = classes);
}

/// The classes each kit map was seen to carry, by its base name: a game
/// on the map reads its kit ([`learn_kit_classes`]), and a later lobby on
/// it gates the Wave set row by what was read ([`known_kit_classes`]).
static KITS_SEEN: Mutex<std::collections::BTreeMap<String, Vec<String>>> =
    Mutex::new(std::collections::BTreeMap::new());

/// A game on `map` read a kit with these classes.
pub fn learn_kit_classes(map: &str, classes: Vec<String>) {
    KITS_SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(map.to_owned(), classes);
}

/// The classes `map`'s kit was seen with; `None` until a game has played
/// on it, when every set is offered ([`State::kit_classes`]).
pub fn known_kit_classes(map: &str) -> Option<Vec<String>> {
    KITS_SEEN.lock().unwrap_or_else(|e| e.into_inner()).get(map).cloned()
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
pub fn kit_map_name(m: &dyn guest_layout::memory::GuestMemory) -> Option<String> {
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

    /// A game's kit read is remembered per map: the lobby on that map
    /// knows its classes, a map never played on does not.
    #[test]
    fn a_kit_map_is_known_by_the_classes_a_game_read_from_it() {
        assert_eq!(known_kit_classes("lobbytest-map"), None);
        learn_kit_classes("lobbytest-map", vec!["elite".into(), "marine".into()]);
        assert_eq!(known_kit_classes("lobbytest-map"), Some(vec!["elite".into(), "marine".into()]));
        assert_eq!(known_kit_classes("lobbytest-other"), None);
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
            forge.contains(&12) && forge.contains(&13),
            "installed packages are selectable"
        );
        let kit: Vec<_> = kit_maps(&cat).into_iter().map(|m| m.id).collect();
        assert_eq!(kit, [12]);
        assert_eq!(cat.maps(Category::Mods).iter().map(|m| m.id).collect::<Vec<_>>(), [12, 13],
            "a kit package is listed with the other packages");
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

    fn ground_cat() -> Catalogue {
        Catalogue::new([
            map(10, "Lockout", r"d:\maps\lockout"),
            map(11, "Beaver Creek", r"d:\maps\beavercreek"),
            map(12, "Firefight \u{2013} Lockout", r"t:\$C\12345678\lockout"),
        ])
    }

    #[test]
    fn a_variant_with_ground_is_a_kit_map_for_the_lobby() {
        let cat = ground_cat();
        // The held pick is what makes the base map a kit map.
        assert_eq!(
            KitMaps::for_pick(Some(&cat), Some(10), None, true),
            KitMaps { lobby: true, installed: true }
        );
        // A kit package needs no pick.
        assert!(KitMaps::for_pick(Some(&cat), Some(12), None, false).lobby);
        // A pick on a map the list does not have is no map.
        assert!(!KitMaps::for_pick(Some(&cat), Some(99), None, true).lobby);
        assert_eq!(KitMaps::for_pick(None, Some(10), None, true), KitMaps::default());
    }

    #[test]
    fn a_base_map_without_a_ground_pick_is_refused_by_name() {
        use crate::lobby_mode::refusal;
        use crate::lobby_tabs::Reason;
        let cat = ground_cat();
        let kit = KitMaps::for_pick(Some(&cat), Some(10), None, false);
        assert_eq!(kit, KitMaps { lobby: false, installed: true });
        assert_eq!(refusal(Mode::Firefight, Some(0), kit), Some(Reason::NotKitMap));
        assert_eq!(
            Reason::NotKitMap.text(),
            "Pick a map variant with AI ground, or a Firefight map."
        );
        let picked = KitMaps::for_pick(Some(&cat), Some(10), None, true);
        assert_eq!(refusal(Mode::Firefight, Some(0), picked), None);
    }

    #[test]
    fn a_map_specific_set_still_needs_its_own_base() {
        let cat = ground_cat();
        let prefix = ::firefight::builtin::variants()
            .iter()
            .find(|v| v.map == Some("beavercreek"))
            .expect("a packaged map set")
            .prefix;
        let on = |id| KitMaps::for_pick(Some(&cat), Some(id), Some(prefix), true);
        assert!(on(11).lobby, "its own base, with ground");
        assert!(!on(10).lobby, "another base, even with ground");
        let any = ::firefight::builtin::variants()
            .iter()
            .find(|v| v.map.is_none())
            .unwrap()
            .prefix;
        assert!(KitMaps::for_pick(Some(&cat), Some(10), Some(any), true).lobby);
        assert!(!KitMaps::for_pick(Some(&cat), Some(10), Some(prefix), false).lobby);
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
                kit_classes: None,
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
