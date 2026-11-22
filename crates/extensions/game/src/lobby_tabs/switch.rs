//! What a switch does (docs/game/party-lobby.md, "Switching"), in this
//! order: the start state checked (no switch while the game loads or counts
//! down), our parts set ([`set_ours`]: the lobby's mode, the `firefight`
//! setting held on for FIREFIGHT, the Clanker flag), then the map put
//! inside the tab's category through `LOBBY_SET_MAP` ([`plan`]). A switch
//! among CUSTOM GAMES, FORGE and FIREFIGHT changes no retail state but the
//! map.
//!
//! A switch that needs the retail lobby mode changed (to or from CO-OP,
//! MATCHMAKING or CLANKERS) is refused by name until slices T4 and T5
//! build it. Every refusal goes to the status line ([`super::notice`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{Cx, EntryError, GuestCall};
use engine::points;
use settings::{Store, Value};

use super::{Reason, Tab, notice};
use crate::firefight::options as ff;
use crate::lobby_start::{self, Mode};
use crate::map_select::{Catalogue, Category};

/// What a switch to a tab sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub to: Tab,
    pub mode: Mode,
    /// The `firefight` setting held on for the lobby's games.
    pub firefight: bool,
    pub clanker: bool,
    /// The map `LOBBY_SET_MAP` puts the lobby on, when the lobby's own is
    /// outside the tab's category.
    pub map: Option<u32>,
}

/// The reason a switch to `to` is not built yet.
pub fn not_yet(to: Tab) -> Reason {
    match to {
        Tab::Matchmaking => Reason::MatchmakingNotYet,
        Tab::CoOp => Reason::CoOpNotYet,
        Tab::Clankers => Reason::ClankersNotYet,
        Tab::CustomGames | Tab::Forge | Tab::Firefight => Reason::CustomNotYet,
    }
}

/// The switch from `from` to `to` with the lobby on map `current` (its
/// list id) and `catalogue` the map list, or why there is none.
pub fn plan(
    from: Tab,
    to: Tab,
    current: Option<u32>,
    catalogue: Option<&Catalogue>,
) -> Result<Plan, Reason> {
    let parts = to.parts();
    if from.parts().retail != parts.retail || parts.retail != super::RetailMode::Custom {
        return Err(not_yet(to));
    }
    let map = match to {
        Tab::CustomGames => None,
        Tab::Forge => {
            let c = catalogue.ok_or(Reason::NoMapList)?;
            inside(c.maps(Category::Halo2).iter().map(|m| m.id), current)
                .ok_or(Reason::NoForgeMap)?
        }
        Tab::Firefight => {
            let c = catalogue.ok_or(Reason::NoMapList)?;
            let kit = lobby_start::kit_maps(c);
            inside(kit.iter().map(|m| m.id), current).ok_or(Reason::NoKitMap)?
        }
        Tab::Matchmaking | Tab::CoOp | Tab::Clankers => return Err(not_yet(to)),
    };
    Ok(Plan {
        to,
        mode: parts.mode,
        firefight: to == Tab::Firefight,
        clanker: parts.clanker,
        map,
    })
}

/// The map a category of `ids` puts the lobby on from `current`: none
/// when `current` is in it, else its first; `None` for an empty category.
fn inside(ids: impl Iterator<Item = u32>, current: Option<u32>) -> Option<Option<u32>> {
    let ids: Vec<u32> = ids.collect();
    if current.is_some_and(|c| ids.contains(&c)) {
        return Some(None);
    }
    ids.first().map(|&id| Some(id))
}

/// Why no switch now, from the lobby's start state (`LOBBY_START_STATE`):
/// the map loading (8) or the countdown running (10).
pub fn locked(start_state: u32) -> Option<Reason> {
    matches!(start_state, 8 | 10).then_some(Reason::Starting)
}

/// Sets our parts of `plan`: the `firefight` setting first (in `store`),
/// then the lobby's mode and the Clanker flag.
pub fn set_ours(plan: &Plan, store: Option<&Store>) -> Result<(), Reason> {
    if plan.firefight {
        let store = store.ok_or(Reason::NoSettings)?;
        store.set(ff::ENABLED.key, Value::Bool(true)).map_err(|e| {
            eprintln!("lobby tabs: {}: {e}", ff::ENABLED.key);
            Reason::NoSettings
        })?;
    }
    lobby_start::set_mode(plan.mode);
    crate::clankermatch::set_active(plan.clanker);
    Ok(())
}

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Why a snapshot waits: a switch's guest calls are in flight.
pub fn in_flight() -> Option<String> {
    IN_FLIGHT
        .load(Ordering::SeqCst)
        .then(|| "a mode strip switch's guest calls are in flight".to_owned())
}

/// Runs the switch from `from` to `to` on the leader's console.
pub async fn run(cx: &Cx, from: Tab, to: Tab, store: Option<Arc<Store>>) -> Result<(), EntryError> {
    IN_FLIGHT.store(true, Ordering::SeqCst);
    let out = run_inner(cx, from, to, store).await;
    IN_FLIGHT.store(false, Ordering::SeqCst);
    out
}

async fn run_inner(
    cx: &Cx,
    from: Tab,
    to: Tab,
    store: Option<Arc<Store>>,
) -> Result<(), EntryError> {
    let state = cx
        .call(GuestCall::new(points::LOBBY_START_STATE.base).args([0]))
        .await?
        .eax;
    if let Some(r) = locked(state) {
        notice(r);
        return Ok(());
    }
    let current = cx.read(|m| m.u32(crate::h1_options_row::LOBBY_MAP).ok());
    let catalogue = crate::map_select::current();
    let plan = match plan(from, to, current, catalogue.as_deref()) {
        Ok(p) => p,
        Err(r) => {
            notice(r);
            return Ok(());
        }
    };
    let before = lobby_start::mode();
    if let Err(r) = set_ours(&plan, store.as_deref()) {
        notice(r);
        return Ok(());
    }
    if let Some(map) = plan.map {
        let set = cx
            .call(
                GuestCall::new(points::LOBBY_SET_MAP.base)
                    .esi(map)
                    .args([u32::MAX]),
            )
            .await?
            .eax as u8;
        if set == 0 {
            lobby_start::set_mode(before);
            notice(Reason::MapRefused);
            return Ok(());
        }
    }
    eprintln!("lobby tabs: {} to {}", from.label(), to.label());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map_select::Map;

    fn map(id: u32, name: &str, path: &str) -> Map {
        Map {
            id,
            name: name.into(),
            sort: 0,
            path: path.into(),
        }
    }

    fn catalogue() -> Catalogue {
        Catalogue::new([
            map(10, "Lockout", r"d:\maps\lockout"),
            map(11, "Ascension", r"d:\maps\ascension"),
            map(12, "Firefight \u{2013} Lockout", r"t:\$C\12345678\lockout"),
            map(13, "Some Mod", r"t:\$C\22345678\midship"),
        ])
    }

    #[test]
    fn switching_to_firefight_holds_the_setting_and_picks_a_kit_map() {
        let c = catalogue();
        let p = plan(Tab::CustomGames, Tab::Firefight, Some(10), Some(&c)).unwrap();
        assert_eq!(p.mode, Mode::Firefight);
        assert!(p.firefight);
        assert_eq!(p.map, Some(12), "a kit map in place of Lockout");
        let kept = plan(Tab::Forge, Tab::Firefight, Some(12), Some(&c)).unwrap();
        assert_eq!(kept.map, None, "already on a kit map");
        // The setting is held on.
        let store = Store::defaults(&ff::DEFS);
        store.set(ff::ENABLED.key, Value::Bool(false)).unwrap();
        set_ours(&p, Some(&store)).unwrap();
        assert_eq!(store.get(ff::ENABLED.key), Some(Value::Bool(true)));
        assert_eq!(lobby_start::mode(), Mode::Firefight);
        // Without a store, refused by name and the mode left as it was.
        lobby_start::set_mode(Mode::Multiplayer);
        assert_eq!(set_ours(&p, None), Err(Reason::NoSettings));
        assert_eq!(lobby_start::mode(), Mode::Multiplayer);
        // No kit map installed.
        let bare = Catalogue::new([map(10, "Lockout", r"d:\maps\lockout")]);
        assert_eq!(
            plan(Tab::CustomGames, Tab::Firefight, Some(10), Some(&bare)),
            Err(Reason::NoKitMap)
        );
    }

    #[test]
    fn switching_to_forge_keeps_a_halo_2_map() {
        let c = catalogue();
        let p = plan(Tab::CustomGames, Tab::Forge, Some(11), Some(&c)).unwrap();
        assert_eq!((p.mode, p.map, p.firefight), (Mode::Forge, None, false));
        let from_kit = plan(Tab::Firefight, Tab::Forge, Some(12), Some(&c)).unwrap();
        assert!(
            from_kit.map.is_some_and(|id| id == 10 || id == 11),
            "an original map in place of the kit map: {from_kit:?}"
        );
        let mod_map = plan(Tab::CustomGames, Tab::Forge, Some(13), Some(&c)).unwrap();
        assert!(mod_map.map.is_some(), "a mod is not an original");
        assert_eq!(
            plan(Tab::CustomGames, Tab::Forge, Some(11), None),
            Err(Reason::NoMapList)
        );
        // CUSTOM GAMES takes any map.
        let custom = plan(Tab::Firefight, Tab::CustomGames, Some(12), None).unwrap();
        assert_eq!((custom.mode, custom.map), (Mode::Multiplayer, None));
    }

    #[test]
    fn no_switch_during_the_countdown() {
        assert_eq!(locked(10), Some(Reason::Starting), "the countdown");
        assert_eq!(locked(8), Some(Reason::Starting), "the map loading");
        assert_eq!(locked(9), None, "ready");
        assert_eq!(locked(7), None);
    }

    #[test]
    fn a_switch_to_another_retail_lobby_is_refused_by_name() {
        let c = catalogue();
        for to in [Tab::Matchmaking, Tab::CoOp, Tab::Clankers] {
            let r = plan(Tab::CustomGames, to, Some(10), Some(&c)).unwrap_err();
            assert_eq!(r, not_yet(to));
            assert!(r.text().ends_with("not available yet."), "{r:?}");
        }
        assert_eq!(
            plan(Tab::CoOp, Tab::CustomGames, None, Some(&c)),
            Err(Reason::CustomNotYet)
        );
    }
}
