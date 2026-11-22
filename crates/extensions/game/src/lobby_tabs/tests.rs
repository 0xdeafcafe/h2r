use super::*;

use Avail::{Off, On};

const NONE: Facts = Facts {
    playlists: false,
    kit_maps: false,
    remote_members: false,
};
/// A lobby where every tab can run: playlists listed, a kit map, no
/// member on another console.
const ALL_FACTS: Facts = Facts {
    playlists: true,
    kit_maps: true,
    remote_members: false,
};

/// Each tab's parts, the table of docs/game/party-lobby.md, "The tabs".
#[test]
fn every_tab_round_trips_through_its_parts() {
    let table: [(Tab, RetailMode, Mode); 5] = [
        (Tab::Multiplayer, RetailMode::Matchmaking, Mode::Multiplayer),
        (Tab::CustomGames, RetailMode::Custom, Mode::Multiplayer),
        (Tab::Forge, RetailMode::Custom, Mode::Forge),
        (Tab::Firefight, RetailMode::Custom, Mode::Firefight),
        (Tab::CoOp, RetailMode::CoOp, Mode::Multiplayer),
    ];
    assert_eq!(table.map(|(t, ..)| t), Tab::ALL, "the strip's order");
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

/// The strip has no CLANKERS tab: the Clanker playlists are MULTIPLAYER's,
/// on its one playlist page.
#[test]
fn there_is_no_clankers_tab() {
    let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(
        labels,
        ["MULTIPLAYER", "CUSTOM GAMES", "FORGE", "FIREFIGHT", "CO-OP"]
    );
    assert!(Tab::ALL.iter().all(|t| !t.name().contains("clanker")));
}

/// The table of docs/game/party-lobby.md, "Where each tab runs", cell by
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
    // The two reasons in every place.
    for place in [SplitScreen, SystemLink, XboxLive] {
        assert_eq!(available(Tab::Firefight, place, NONE), Off(NoKitMap));
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
    // No reason says a switch is not built: every tab that is on switches.
    for r in Reason::ALL {
        assert!(!r.text().contains("not available yet"), "{r:?}");
    }
}

#[test]
fn a_feature_that_is_off_removes_its_tab() {
    assert_eq!(Present::ALL.tabs(), Tab::ALL);
    assert_eq!(
        Present::default().tabs(),
        [Tab::Multiplayer, Tab::CustomGames],
        "retail's two are always there"
    );
    let no_forge = Present {
        forge: false,
        ..Present::ALL
    };
    assert!(!no_forge.tabs().contains(&Tab::Forge));
    assert_eq!(no_forge.tabs().len(), 4);
    // Off in a place is dimmed, never removed.
    let r = row(Present::ALL, Place::SplitScreen, NONE);
    assert_eq!(r.len(), 5);
    assert_eq!(r[0], (Tab::Multiplayer, Off(Reason::MatchmakingNeedsLive)));
}

#[test]
fn step_skips_tabs_that_are_off_and_names_the_first() {
    // Split Screen with no kit map: MULTIPLAYER and FIREFIGHT off.
    let r = row(Present::ALL, Place::SplitScreen, NONE);
    assert_eq!(
        step(&r, Tab::CustomGames, Dir::Next),
        Step::Move {
            to: Tab::Forge,
            skipped: None
        }
    );
    assert_eq!(
        step(&r, Tab::Forge, Dir::Next),
        Step::Move {
            to: Tab::CoOp,
            skipped: Some(Reason::NoKitMap)
        },
        "FIREFIGHT skipped, its reason shown"
    );
    assert_eq!(
        step(&r, Tab::CoOp, Dir::Next),
        Step::Stay(None),
        "the end of the strip"
    );
    assert_eq!(
        step(&r, Tab::CustomGames, Dir::Prev),
        Step::Stay(Some(Reason::MatchmakingNeedsLive)),
        "only an off tab that way: stay, and say why"
    );
    assert_eq!(
        step(&r, Tab::CoOp, Dir::Prev),
        Step::Move {
            to: Tab::Forge,
            skipped: Some(Reason::NoKitMap)
        }
    );
    assert!(glyph_on(&r, Tab::CustomGames, Dir::Next));
    assert!(!glyph_on(&r, Tab::CustomGames, Dir::Prev), "White dimmed");
    assert!(!glyph_on(&r, Tab::CoOp, Dir::Next), "Black dimmed");
}

#[test]
fn the_ends_do_not_wrap() {
    let r = row(Present::ALL, Place::XboxLive, ALL_FACTS);
    assert_eq!(step(&r, Tab::Multiplayer, Dir::Prev), Step::Stay(None));
    assert_eq!(step(&r, Tab::CoOp, Dir::Next), Step::Stay(None));
    assert_eq!(
        step(&r, Tab::CoOp, Dir::Prev),
        Step::Move {
            to: Tab::Firefight,
            skipped: None
        }
    );
    assert_eq!(
        step(&r, Tab::CustomGames, Dir::Prev),
        Step::Move {
            to: Tab::Multiplayer,
            skipped: None
        }
    );
    // A tab that is not on the strip goes nowhere.
    let short = row(Present::default(), Place::XboxLive, ALL_FACTS);
    assert_eq!(step(&short, Tab::Forge, Dir::Next), Step::Stay(None));
}

#[test]
fn cells_stay_in_the_band_and_do_not_overlap() {
    use crate::ui_kit::layout::SAFE;
    let all = cells(&Tab::ALL);
    let mut pieces: Vec<[i16; 4]> = vec![PREV];
    pieces.extend(all.iter().map(|(_, c)| *c));
    pieces.push(next_glyph(&Tab::ALL));
    for c in &pieces {
        let [top, left, bottom, right] = *c;
        assert!(
            top <= BAND_TOP && bottom >= BAND_BOTTOM && top > bottom,
            "{c:?}"
        );
        assert!(
            top <= SAFE.top && left >= SAFE.left && right <= SAFE.right,
            "{c:?}"
        );
        assert!(left < right, "{c:?}");
    }
    for w in pieces.windows(2) {
        assert!(w[0][3] < w[1][1], "{:?} meets {:?}", w[0], w[1]);
    }
    // The design's table, cell by cell.
    let want = [
        (Tab::Multiplayer, -244, -160),
        (Tab::CustomGames, -155, -63),
        (Tab::Forge, -58, -18),
        (Tab::Firefight, -13, 57),
        (Tab::CoOp, 62, 102),
    ];
    for ((tab, cell), (t, l, r)) in all.iter().zip(want) {
        assert_eq!((*tab, cell[1], cell[3]), (t, l, r));
    }
    assert_eq!(next_glyph(&Tab::ALL), [TOP, 107, BOTTOM, 129]);
    // The player count's narrowed rectangle (`[199 200 179 280]`) starts
    // past Black's cell.
    assert!(next_glyph(&Tab::ALL)[3] < 200);
}

#[test]
fn cells_close_up_without_a_tab() {
    let tabs = Present {
        forge: false,
        ..Present::ALL
    }
    .tabs();
    let c = cells(&tabs);
    assert_eq!(c.len(), 4);
    assert_eq!(c[0].1[1], RUN_LEFT, "the run stays left-aligned");
    assert_eq!(c[1].0, Tab::CustomGames);
    assert_eq!(
        c[2],
        (Tab::Firefight, [TOP, -58, BOTTOM, 12]),
        "FIREFIGHT moves up into FORGE's place"
    );
    for w in c.windows(2) {
        assert_eq!(w[1].1[1] - w[0].1[3], GAP);
    }
    // Black's glyph follows the last cell.
    assert_eq!(next_glyph(&tabs)[1], c.last().unwrap().1[3] + GAP);
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
    assert_eq!(PREV_TEXT, Glyph::White.char().to_string());
    assert_eq!(NEXT_TEXT, Glyph::Black.char().to_string());
    // No id twice.
    let mut ids: Vec<u32> = strings().into_iter().map(|(i, _)| i).collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n);
}

#[test]
fn a_member_reads_firefight_from_the_map_and_never_forge() {
    assert_eq!(seen_mode(true, Mode::Forge, false), Mode::Forge);
    assert_eq!(seen_mode(false, Mode::Forge, false), Mode::Multiplayer);
    assert_eq!(seen_mode(false, Mode::Multiplayer, true), Mode::Firefight);
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
