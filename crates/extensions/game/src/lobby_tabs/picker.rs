//! The GAME MODE picker (docs/game/party-lobby.md, "Changing mode"): X in
//! the pregame lobby, for the party leader, opens the kit's value picker
//! (`crate::ui_kit::picker`) over the lobby, titled GAME MODE, its values
//! the modes of [`MODES`] present, in title case, focused on the lobby's
//! mode (CUSTOM GAMES's when the lobby is in CO-OP, which the picker does
//! not list). The right side says what the focused mode plays
//! ([`Tab::line`]), or, for a mode that is off here, why ([`Reason`]).
//!
//! A sets the value and closes the picker; then the pick's action runs
//! the switch to that mode from the lobby as it stands
//! ([`super::switch::run_to`]): the same checks, refusals and retail
//! routines whatever the way in. B closes it and changes nothing.

use std::sync::Arc;

use settings::Store;

use super::header::Shown;
use super::{Avail, Present, Tab};
use crate::list_class::Action;
use crate::ui_kit::picker::Pick;
use crate::ui_kit::view::Opener;

/// The modes the picker lists, in its order. CO-OP is a lobby mode the
/// header names, but not a value of the picker.
pub const MODES: [Tab; 4] = [Tab::Multiplayer, Tab::CustomGames, Tab::Forge, Tab::Firefight];
/// The picker's title, as a row's label ([`crate::ui_kit::picker::title`]
/// puts it in capitals).
pub const TITLE: &str = "Game Mode";
/// The picker's right side with no mode focused.
pub const DESCRIPTION: &str = "Choose what this lobby plays.";

/// The modes the picker lists with `present`'s features.
pub fn modes(present: Present) -> Vec<Tab> {
    MODES.into_iter().filter(|&t| present.has(t)).collect()
}

/// The value the picker opens on in the lobby `shown`: its mode, else
/// CUSTOM GAMES (the lobby is in CO-OP), else the first.
pub fn held(tabs: &[Tab], shown: &Shown) -> usize {
    tabs.iter()
        .position(|&t| t == shown.selected)
        .or_else(|| tabs.iter().position(|&t| t == Tab::CustomGames))
        .unwrap_or(0)
}

/// What the right side says while `tab` has the focus in the lobby
/// `shown`: why it is off here, else what it plays.
pub fn line(tab: Tab, shown: &Shown) -> &'static str {
    match shown.avail(tab) {
        Some(Avail::Off(r)) => r.text(),
        _ => tab.line(),
    }
}

/// The picker for the lobby `shown` with `present`'s modes. Its setter
/// only logs: the switch is the action ([`after`]), which runs once the
/// picker closes.
pub fn pick(shown: &Shown, present: Present) -> Pick {
    let tabs = modes(present);
    let values = tabs.iter().map(|t| t.title_case().to_owned()).collect();
    let details = tabs.iter().map(|&t| line(t, shown).to_owned()).collect();
    let at = held(&tabs, shown);
    let names = tabs.clone();
    Pick::new(
        TITLE,
        values,
        at,
        DESCRIPTION,
        Arc::new(move |i: usize| match names.get(i) {
            Some(t) => format!("lobby tabs: {} picked", t.label()),
            None => format!("lobby tabs: mode {i} is not in the list"),
        }),
    )
    .details(details)
    .over(Opener::LOBBY)
}

/// The mode value `i` of the picker names with `present`'s modes.
pub fn picked(present: Present, i: usize) -> Option<Tab> {
    modes(present).get(i).copied()
}

/// The picker's action once a value is picked and it has closed: the
/// switch to the picked mode, with `store` the settings Firefight's
/// choice is held in.
pub fn after(present: Present, store: Option<Arc<Store>>) -> Action {
    Arc::new(move |cx, _screen| {
        let store = store.clone();
        Box::pin(async move {
            let at = crate::ui_kit::picker::current().map(|p| p.current);
            let Some(to) = at.and_then(|i| picked(present, i)) else {
                return Ok(());
            };
            super::switch::run_to(&cx, to, present, store).await
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lobby_tabs::{Facts, Place, Reason, row};

    fn lobby(place: Place, leader: bool, selected: Tab, facts: Facts) -> Shown {
        Shown {
            place,
            leader,
            selected,
            row: row(Present::ALL, place, facts),
            clanker: None,
        }
    }

    const LIVE: Facts = Facts {
        playlists: true,
        live_co_op: true,
        remote_members: false,
    };

    /// The four modes in title case, focused on the lobby's, with what each
    /// plays on the right; over the lobby.
    #[test]
    fn the_picker_lists_the_four_modes_and_opens_on_the_lobbys() {
        let s = lobby(Place::XboxLive, true, Tab::Forge, LIVE);
        let p = pick(&s, Present::ALL);
        assert_eq!(p.title, "GAME MODE");
        assert_eq!(p.values, ["Multiplayer", "Custom Games", "Forge", "Firefight"]);
        assert_eq!(p.values[p.current], "Forge", "the held mode");
        assert_eq!(p.opener, Opener::LOBBY);
        assert_eq!(p.description, DESCRIPTION);
        for (i, t) in MODES.iter().enumerate() {
            assert_eq!(p.line(i), t.line(), "{t:?}");
        }
        // In CO-OP, which it does not list: CUSTOM GAMES focused.
        let co_op = lobby(Place::XboxLive, true, Tab::CoOp, LIVE);
        let p = pick(&co_op, Present::ALL);
        assert_eq!(p.values[p.current], "Custom Games");
        assert!(!p.values.iter().any(|v| v.contains("Co-Op")));
    }

    /// A mode that is off here says why on the right; a mode switched off
    /// is not listed.
    #[test]
    fn a_mode_that_is_off_says_why_and_an_absent_one_is_not_listed() {
        let s = lobby(Place::SplitScreen, true, Tab::CustomGames, Facts::default());
        let p = pick(&s, Present::ALL);
        assert_eq!(p.line(0), Reason::MatchmakingNeedsLive.text());
        assert_eq!(p.line(1), Tab::CustomGames.line());
        let no_forge = Present {
            forge: false,
            ..Present::ALL
        };
        let p = pick(&s, no_forge);
        assert_eq!(p.values, ["Multiplayer", "Custom Games", "Firefight"]);
        assert_eq!(picked(no_forge, 2), Some(Tab::Firefight));
        assert_eq!(picked(no_forge, 3), None);
        assert_eq!(picked(Present::ALL, 2), Some(Tab::Forge));
    }

    /// Every mode the picker lists is a value whose setter writes nothing:
    /// the switch is the action.
    #[test]
    fn the_setter_writes_nothing() {
        let s = lobby(Place::XboxLive, true, Tab::CustomGames, LIVE);
        let p = pick(&s, Present::ALL);
        let before = crate::lobby_start::mode();
        for i in 0..p.values.len() {
            assert!((p.set)(i).contains(MODES[i].label()));
        }
        assert!((p.set)(9).contains("not in the list"));
        assert_eq!(crate::lobby_start::mode(), before);
    }
}
