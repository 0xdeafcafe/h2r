use super::*;

use Avail::{Off, On};

const NONE: Facts = Facts {
    playlists: false,
    live_co_op: false,
    remote_members: false,
};
/// A lobby where every tab can run: playlists listed, co-op over Xbox Live
/// installed, no member on another console.
const ALL_FACTS: Facts = Facts {
    playlists: true,
    live_co_op: true,
    remote_members: false,
};

/// Each mode's parts, the table of docs/game/party-lobby.md, "The modes".
#[test]
fn every_tab_round_trips_through_its_parts() {
    let table: [(Tab, RetailMode, Mode); 5] = [
        (Tab::Multiplayer, RetailMode::Matchmaking, Mode::Multiplayer),
        (Tab::CustomGames, RetailMode::Custom, Mode::Multiplayer),
        (Tab::Forge, RetailMode::Custom, Mode::Forge),
        (Tab::Firefight, RetailMode::Custom, Mode::Firefight),
        (Tab::CoOp, RetailMode::CoOp, Mode::Multiplayer),
    ];
    assert_eq!(table.map(|(t, ..)| t), Tab::ALL, "the modes' order");
    for (tab, retail, mode) in table {
        assert_eq!(tab.parts(), Parts { retail, mode }, "{tab:?}");
        assert_eq!(Tab::of(retail, mode), tab, "{tab:?}");
    }
    // Every lobby state has a tab: our mode counts only in the custom
    // lobby.
    for retail in [
        RetailMode::CoOp,
        RetailMode::Custom,
        RetailMode::Matchmaking,
    ] {
        for mode in Mode::ALL {
            let tab = Tab::of(retail, mode);
            assert_eq!(tab.parts().retail, retail, "{retail:?} {mode:?}");
        }
    }
    assert_eq!(
        Tab::of(RetailMode::Matchmaking, Mode::Forge),
        Tab::Multiplayer
    );
    assert_eq!(Tab::of(RetailMode::CoOp, Mode::Firefight), Tab::CoOp);
    for retail in [
        RetailMode::CoOp,
        RetailMode::Custom,
        RetailMode::Matchmaking,
    ] {
        assert_eq!(RetailMode::from_value(retail.value()), Some(retail));
    }
    assert_eq!(RetailMode::from_value(1), None);
}

/// There is no CLANKERS mode: the Clanker playlists are MULTIPLAYER's, on
/// its one playlist page.
#[test]
fn there_is_no_clankers_tab() {
    let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(
        labels,
        ["MULTIPLAYER", "CUSTOM GAMES", "FORGE", "FIREFIGHT", "CO-OP"]
    );
    assert!(Tab::ALL.iter().all(|t| !t.name().contains("clanker")));
}

/// The table of docs/game/party-lobby.md, "Where each mode runs", cell by
/// cell.
#[test]
fn availability_by_place() {
    use Place::{SplitScreen, SystemLink, XboxLive};
    use Reason::*;
    let table: [(Tab, [Avail; 3]); 5] = [
        (
            Tab::Multiplayer,
            [Off(MatchmakingNeedsLive), Off(MatchmakingNeedsLive), On],
        ),
        (Tab::CustomGames, [On, On, On]),
        (Tab::Forge, [On, On, On]),
        (Tab::Firefight, [On, On, On]),
        (Tab::CoOp, [On, Off(CoOpNotOnSystemLink), On]),
    ];
    for (tab, by_place) in table {
        for (place, want) in [SplitScreen, SystemLink, XboxLive]
            .into_iter()
            .zip(by_place)
        {
            assert_eq!(
                available(tab, place, ALL_FACTS),
                want,
                "{tab:?} on {place:?}"
            );
        }
    }
    // Xbox Live with no playlist listed.
    assert_eq!(
        available(Tab::Multiplayer, XboxLive, NONE),
        Off(NoPlaylists)
    );
    // Xbox Live without co-op over Live installed.
    assert_eq!(available(Tab::CoOp, XboxLive, NONE), Off(CoOpLiveOff));
    assert_eq!(available(Tab::CoOp, SplitScreen, NONE), On, "retail's own");
    // FIREFIGHT is on whatever maps are installed: Start Game says what is
    // missing (`lobby_mode::start_refusal`). FORGE's reason applies in
    // every place.
    for place in [SplitScreen, SystemLink, XboxLive] {
        assert_eq!(available(Tab::Firefight, place, NONE), On);
        let remote = Facts {
            remote_members: true,
            ..ALL_FACTS
        };
        assert_eq!(available(Tab::Forge, place, remote), Off(ForgeRemote));
        assert_eq!(available(Tab::CustomGames, place, NONE), On);
    }
}

#[test]
fn a_tab_that_is_off_has_a_reason() {
    for tab in Tab::ALL {
        for place in [Place::SplitScreen, Place::SystemLink, Place::XboxLive] {
            for facts in [NONE, ALL_FACTS] {
                if let Off(r) = available(tab, place, facts) {
                    let t = r.text();
                    assert!(t.ends_with('.') && t.len() > 10, "{tab:?}: {t:?}");
                }
            }
        }
    }
    assert_eq!(
        Reason::ForgeRemote.text(),
        crate::lobby_mode::REMOTE_REFUSAL
    );
    assert_eq!(
        Reason::MatchmakingNeedsLive.text(),
        "Matchmaking needs Xbox Live."
    );
    // No reason says a switch is not built: every mode that is on switches.
    for r in Reason::ALL {
        assert!(!r.text().contains("not available yet"), "{r:?}");
    }
}

#[test]
fn a_feature_that_is_off_removes_its_tab() {
    assert_eq!(Present::ALL.tabs(), [Tab::Multiplayer, Tab::CustomGames, Tab::Forge, Tab::CoOp]);
    assert_eq!(
        Present::default().tabs(),
        [Tab::Multiplayer, Tab::CustomGames, Tab::CoOp],
        "retail's three are always there"
    );
    let no_forge = Present {
        forge: false,
        ..Present::ALL
    };
    assert!(!no_forge.tabs().contains(&Tab::Forge));
    assert_eq!(no_forge.tabs().len(), 3);
    // Off in a place is refused with its reason, never removed.
    let r = row(Present::ALL, Place::SplitScreen, NONE);
    assert_eq!(r.len(), 4);
    assert_eq!(r[0], (Tab::Multiplayer, Off(Reason::MatchmakingNeedsLive)));
}

/// The user's lobby: Split Screen, no map that carries the Firefight kit,
/// no Live. Every mode but MULTIPLAYER is on, FIREFIGHT and CO-OP
/// included, so the picker reaches each.
#[test]
fn firefight_and_co_op_are_reached_with_no_kit_map_and_no_live() {
    let r = row(Present::ALL, Place::SplitScreen, NONE);
    assert_eq!(r.len(), 4, "every mode present");
    let on: Vec<Tab> = r.iter().filter(|(_, a)| a.is_on()).map(|(t, _)| *t).collect();
    assert_eq!(on, [Tab::CustomGames, Tab::Forge, Tab::CoOp]);
    // With a kit map or without, in every place, FIREFIGHT is on.
    for place in [Place::SplitScreen, Place::SystemLink, Place::XboxLive] {
        for facts in [NONE, ALL_FACTS] {
            assert!(
                available(Tab::Firefight, place, facts).is_on(),
                "{place:?}"
            );
        }
    }
}

#[test]
fn every_label_and_reason_passes_the_content_check() {
    for (id, text) in strings() {
        assert!(IDS.owns(id), "{id:#x}");
        let findings = crate::ui_kit::content::check_text(&format!("{id:#x}"), text, true);
        assert!(findings.is_empty(), "{text:?}: {findings:?}");
    }
    for t in Tab::ALL {
        assert_eq!(t.label(), t.label().to_uppercase(), "a label in capitals");
        assert!(!t.label().contains(':'));
    }
    for r in Reason::ALL {
        assert!(!r.text().contains("  "), "{r:?}");
    }
    // The prompt line: the X glyph, then what it does.
    assert!(CHANGE_MODE_TEXT.starts_with(CHANGE_MODE_GLYPH.char()));
    assert_eq!(CHANGE_MODE_TEXT, "\u{e102} Change Game Mode");
    assert!(strings().contains(&(CHANGE_MODE_ID, CHANGE_MODE_TEXT)));
    for t in Tab::ALL {
        let l = t.line();
        assert!(l.ends_with('.') && !l.contains('\n') && l.len() <= 70, "{t:?}: {l:?}");
    }
    // No id twice.
    let mut ids: Vec<u32> = strings().into_iter().map(|(i, _)| i).collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n);
}

#[test]
fn custom_game_variants_share_the_custom_lobby_header() {
    assert_eq!(seen_mode(true, Mode::Forge), Mode::Forge);
    assert_eq!(seen_mode(false, Mode::Forge), Mode::Multiplayer);
    assert_eq!(seen_mode(false, Mode::Firefight), Mode::Multiplayer);
}

#[test]
fn a_reason_shows_for_four_seconds() {
    let at = Instant::now();
    let n = Notice {
        reason: Reason::MatchmakingNeedsLive,
        at,
    };
    assert_eq!(n.shown(at), Some(Reason::MatchmakingNeedsLive));
    assert_eq!(
        n.shown(at + Duration::from_millis(3999)),
        Some(Reason::MatchmakingNeedsLive)
    );
    assert_eq!(n.shown(at + NOTICE_FOR), None);
    assert_eq!(n.shown(at + Duration::from_secs(60)), None);
}

#[test]
fn each_look_follows_the_selection_and_the_place() {
    assert_eq!(
        look(Tab::Forge, Tab::Forge, Off(Reason::ForgeRemote)),
        Look::Selected
    );
    assert_eq!(look(Tab::Forge, Tab::CustomGames, On), Look::On);
    assert_eq!(
        look(Tab::Forge, Tab::CustomGames, Off(Reason::ForgeRemote)),
        Look::Off
    );
}

/// A Live menu row's switch is run once, while it is still wanted.
#[test]
fn a_requested_switch_is_taken_once_and_expires() {
    let at = Instant::now();
    let r = Request {
        to: Tab::Forge,
        at,
    };
    assert_eq!(r.wanted(at), Some(Tab::Forge));
    assert_eq!(r.wanted(at + REQUEST_FOR), None, "a lobby that never came");
    request(Tab::Firefight);
    assert_eq!(take_request(), Some(Tab::Firefight));
    assert_eq!(take_request(), None, "taken once");
    request(Tab::Forge);
    cancel_request();
    assert_eq!(take_request(), None, "another row dropped it");
}
