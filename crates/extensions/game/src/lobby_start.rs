//! What the FORGE and FIREFIGHT main-menu lobbies share
//! ([`crate::forge_lobby`], [`crate::firefight::lobby`]): the maps their
//! pickers list, the pregame lobby's skin, and starting a local game.
//!
//! **Maps.** Forge opens on an original map: the catalogue's Halo 2
//! category ([`forge_maps`]). Firefight needs a map that carries the kit,
//! which `h2 firefight kit` installs as a package named `Firefight – <map>`
//! (`firefight-kit`'s `build_package`): the catalogue lists it by that
//! name ([`is_kit_map`]). A map the kit's probe recipe made is named
//! `Firefight Probe – <map>` and is not offered.
//!
//! **The skin.** The pregame lobby's layout, as a mod's page has it
//! ([`crate::mods_screen::page_layout`]): the rows where the lobby's
//! buttons are, the lobby's panels behind them and one status line
//! ([`STATUS`], at most [`STATUS_CHARS`] characters) under the rows.
//!
//! **Starting.** [`start`] queues a local multiplayer game on a map the way
//! the level boot starts one ([`crate::level_boot::game_steps`]): the
//! agent's guest calls ([`agent::sim::calls`]) make the game's own option
//! builders and queue them at the next tick, and the game loads at its next
//! transition, as the main menu's own start does. The queue is set at boot
//! ([`set_queue`]). The guest's answer comes back through [`failure`]: a
//! refusal is shown on the lobby's status line, never dropped.

use std::sync::{Arc, Mutex, MutexGuard};

use agent::sim::calls::{self, Call, PendingCall};

use crate::level_boot::{self, Kind, Request};
use crate::list_class::{ListLayout, Panel, TextBlock};
use crate::map_select::{Catalogue, Category, Map};
use crate::mods_screen::page_layout as lobby;
use crate::ui_kit::picker::Pick;

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

/// The maps `from` lists now, or none before the catalogue is read.
pub fn maps_from(
    from: impl Fn(&Catalogue) -> Vec<Map> + Send + Sync + 'static,
) -> Arc<dyn Fn() -> Vec<Map> + Send + Sync> {
    Arc::new(move || {
        crate::map_select::current()
            .map(|c| from(&c))
            .unwrap_or_default()
    })
}

/// The local game a lobby starts on `map`: one local player, the default
/// Slayer, as the level boot starts a disc map.
pub fn request(map: &Map) -> Request {
    Request {
        name: map.name.clone(),
        scenario: map.path.clone(),
        campaign: u32::MAX,
        map: map.id,
        difficulty: level_boot::Difficulty::default(),
        kind: Kind::Multiplayer,
    }
}

// ---- the skin ----

/// The rows of a lobby: the pregame lobby's list, three rows tall.
pub const LAYOUT: ListLayout = ListLayout {
    visible: 3,
    ..lobby::LIST
};
/// The status line, under the rows.
pub const STATUS: TextBlock = lobby::STATUS;
/// What fits on the status line (about 300 units of conduit-12).
pub const STATUS_CHARS: usize = 46;

/// The lobby's backings, behind the rows and the status.
pub fn panel() -> Panel {
    Panel {
        art: lobby::ART.to_vec(),
        texts: Vec::new(),
    }
}

/// `text` as the status line holds it.
pub fn fit(text: &str) -> String {
    if text.chars().count() <= STATUS_CHARS {
        return text.into();
    }
    let head: String = text.chars().take(STATUS_CHARS - 3).collect();
    format!("{}...", head.trim_end())
}

/// A pick of `maps`, each listed as `name` gives it, the one at `current`
/// held. `set` is told the map chosen.
pub fn map_pick(
    maps: &[Map],
    name: fn(&Map) -> String,
    current: usize,
    description: &str,
    set: impl Fn(&Map) -> String + Send + Sync + 'static,
) -> Pick {
    let held: Vec<Map> = maps.to_vec();
    Pick::new(
        "Map",
        maps.iter().map(name).collect(),
        current,
        description,
        Arc::new(move |i: usize| match held.get(i) {
            Some(map) => set(map),
            None => format!("map {i} is not in the list"),
        }),
    )
}

/// A pick that only says why there is nothing to pick: one entry, the
/// reason, and `description` to explain it. Choosing it changes nothing.
pub fn nothing_to_pick(label: &str, reason: &str, description: &str) -> Pick {
    Pick::new(
        label,
        vec![reason.into()],
        0,
        description,
        Arc::new(|_: usize| "nothing to pick".into()),
    )
}

// ---- starting ----

static QUEUE: Mutex<Option<calls::Shared>> = Mutex::new(None);
static PENDING: Mutex<Option<PendingCall>> = Mutex::new(None);
static FAILED: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The guest-call queue [`start`] submits to.
pub fn set_queue(queue: calls::Shared) {
    *lock(&QUEUE) = Some(queue);
}

/// Finds the guest-call queue the lobbies start through: the agent's, else
/// one of our own with the feature that makes its calls. It becomes
/// [`start`]'s ([`set_queue`]) and is returned.
pub fn install_queue(
    registry: &mut hooks::Registry,
    agent: Option<&crate::agent_glue::AgentGlue>,
) -> anyhow::Result<calls::Shared> {
    let queue = if let Some(sim) = agent.and_then(|a| a.sim.as_ref()) {
        sim.calls().unwrap_or_else(calls::shared)
    } else {
        let queue = calls::shared();
        registry
            .add(&calls::CallsFeature(queue.clone()))
            .map_err(anyhow::Error::msg)?;
        queue
    };
    set_queue(queue.clone());
    Ok(queue)
}

/// Queues a local game on `req`. An error says why it was not queued; the
/// guest's own refusal comes later, through [`failure`].
pub fn start(req: &Request) -> Result<(), String> {
    let steps = level_boot::game_steps(req)
        .ok_or_else(|| format!("{}: its path does not fit the game's options", req.name))?;
    let queue = lock(&QUEUE).clone().ok_or_else(|| {
        "The game's own functions are not reachable here (agent-calls is off).".to_owned()
    })?;
    let ticket = calls::submit(&queue, Call::QueueGame { steps })?;
    eprintln!(
        "lobby: queueing {} ({}, map {:#x})",
        req.name, req.scenario, req.map
    );
    *lock(&FAILED) = None;
    *lock(&PENDING) = Some(ticket);
    Ok(())
}

/// Why the last [`start`] did not go through in the guest, once the guest
/// has answered; `None` while it waits or after it queued the game.
pub fn failure() -> Option<String> {
    let mut pending = lock(&PENDING);
    if let Some(outcome) = pending.as_ref().and_then(PendingCall::outcome) {
        *pending = None;
        *lock(&FAILED) = refusal(outcome.map_err(|e| e.message).map(|v| {
            let queued = v["queued"].as_bool().unwrap_or(false);
            let checked = v["check"].as_bool().unwrap_or(true);
            (queued, checked)
        }));
    }
    lock(&FAILED).clone()
}

/// The reason an answer from the guest is a refusal: the game's options
/// check failed, or the calls did not queue the game.
pub fn refusal(answer: Result<(bool, bool), String>) -> Option<String> {
    match answer {
        Err(why) => Some(format!("The game refused to start: {why}")),
        Ok((_, false)) => Some("The game's options check refused this map.".into()),
        Ok((false, _)) => Some("The game did not queue the match.".into()),
        Ok((true, true)) => None,
    }
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
    }

    #[test]
    fn a_lobby_starts_a_one_player_multiplayer_game_on_the_maps_path() {
        let m = map(0x1234, "Lockout", r"d:\maps\lockout");
        let r = request(&m);
        assert_eq!(r.kind, Kind::Multiplayer);
        assert_eq!(
            (r.scenario.as_str(), r.map, r.campaign),
            (r"d:\maps\lockout", 0x1234, u32::MAX)
        );
        assert!(level_boot::game_steps(&r).is_some());
        let long = map(1, "x", &"p".repeat(0x200));
        assert!(
            level_boot::game_steps(&request(&long)).is_none(),
            "a path the options cannot hold"
        );
    }

    #[test]
    fn the_status_line_keeps_what_fits_and_ends_a_long_one_with_dots() {
        assert_eq!(fit("Lockout: a new Forge map"), "Lockout: a new Forge map");
        let long = fit(&"w".repeat(80));
        assert_eq!(long.chars().count(), STATUS_CHARS);
        assert!(long.ends_with("..."));
        assert_eq!(fit(&"w".repeat(STATUS_CHARS)).chars().count(), STATUS_CHARS);
    }

    #[test]
    fn the_guests_answer_is_a_refusal_unless_it_queued_the_game() {
        assert_eq!(refusal(Ok((true, true))), None);
        assert!(
            refusal(Ok((true, false)))
                .unwrap()
                .contains("options check")
        );
        assert!(
            refusal(Ok((false, true)))
                .unwrap()
                .contains("did not queue")
        );
        assert!(
            refusal(Err("no player".into()))
                .unwrap()
                .ends_with("no player")
        );
    }

    #[test]
    fn a_pick_names_the_maps_and_tells_the_choice() {
        let maps = [map(1, "A", "a"), map(2, "B", "b")];
        let pick = map_pick(
            &maps,
            |m: &Map| m.name.clone(),
            1,
            "x",
            |m: &Map| format!("chose {}", m.name),
        );
        assert_eq!(pick.values, ["A", "B"]);
        assert_eq!(pick.current, 1);
        assert_eq!((pick.set)(0), "chose A");
        assert!((pick.set)(5).contains("not in the list"));
        let none = nothing_to_pick("Map", "None installed", "why");
        assert_eq!(none.values, ["None installed"]);
        assert_eq!((none.set)(0), "nothing to pick");
    }

    #[test]
    fn the_lobby_rows_are_the_lobbys_list_and_every_status_fits() {
        assert_eq!(LAYOUT.visible, 3);
        assert_eq!(LAYOUT.anchor, lobby::LIST.anchor);
        assert_eq!(panel().art.len(), 2);
        assert!(
            STATUS_CHARS <= 50,
            "the status block is one line of conduit-12"
        );
    }
}
