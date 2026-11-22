use super::*;
use crate::map_select::Map;
use engine::entry::{EntryFuture, Regs, Ret, testing};
use engine::memory::GuestMemory;
use engine::testing::FakeGuest;

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

/// Where the tests' difficulty read writes.
const SCRATCH_AT: u32 = 0x0010_0000;

/// A lobby's memory: the start-state and search globals, the co-op gate
/// and the switch's scratch page.
fn guest(gate: u8) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(0x0051_0000, 0x1000)
        .map(0x0052_7000, 0x1000)
        .map(0x0054_d000, 0x2000)
        .map(SCRATCH_AT, 0x1000);
    g.put_u8(bots::lobby::NETWORKING, 0)
        .put_u32(bots::lobby::SESSION_TYPE, 1)
        .put_u32(crate::h1_options_row::LOBBY_MAP, 10)
        .put_u8(CO_OP_GATE, gate)
        .put_u8(CO_OP_CLEARED, 1);
    g
}

fn targets(calls: &[GuestCall]) -> Vec<u32> {
    calls.iter().map(|c| c.target).collect()
}

#[test]
fn switching_to_firefight_holds_the_setting_and_picks_a_kit_map() {
    let c = catalogue();
    let p = plan(Tab::CustomGames, Tab::Firefight);
    assert_eq!(p.mode, Mode::Firefight);
    assert!(p.firefight);
    assert_eq!(p.retail, None, "inside the custom lobby");
    assert_eq!(
        map_for(Tab::Firefight, Some(10), Some(&c)),
        Ok(Some(12)),
        "a kit map in place of Lockout"
    );
    assert_eq!(
        map_for(Tab::Firefight, Some(12), Some(&c)),
        Ok(None),
        "already on a kit map"
    );
    // The setting is held on.
    let store = Store::defaults(&ff::DEFS);
    store.set(ff::ENABLED.key, Value::Bool(false)).unwrap();
    set_ours(&p, Some(&store)).unwrap();
    assert_eq!(store.get(ff::ENABLED.key), Some(Value::Bool(true)));
    // Without a store, refused by name.
    assert_eq!(set_ours(&p, None), Err(Reason::NoSettings));
    // No kit map installed: the switch goes ahead and the lobby keeps its
    // map; Start Game says why it cannot start.
    let bare = Catalogue::new([map(10, "Lockout", r"d:\maps\lockout")]);
    assert_eq!(map_for(Tab::Firefight, Some(10), Some(&bare)), Ok(None));
    assert_eq!(map_for(Tab::Firefight, None, Some(&bare)), Ok(None));
    assert_eq!(
        map_for(Tab::Firefight, Some(10), None),
        Ok(None),
        "the map list not read yet"
    );
}

#[test]
fn switching_to_forge_keeps_a_halo_2_map() {
    let c = catalogue();
    let p = plan(Tab::CustomGames, Tab::Forge);
    assert_eq!((p.mode, p.firefight, p.retail), (Mode::Forge, false, None));
    assert_eq!(map_for(Tab::Forge, Some(11), Some(&c)), Ok(None));
    let from_kit = map_for(Tab::Forge, Some(12), Some(&c)).unwrap();
    assert!(
        from_kit.is_some_and(|id| id == 10 || id == 11),
        "an original map in place of the kit map: {from_kit:?}"
    );
    assert!(
        map_for(Tab::Forge, Some(13), Some(&c)).unwrap().is_some(),
        "a mod is not an original"
    );
    assert_eq!(map_for(Tab::Forge, Some(11), None), Err(Reason::NoMapList));
    // CUSTOM GAMES, MULTIPLAYER and CO-OP take any map.
    for to in [Tab::CustomGames, Tab::Multiplayer, Tab::CoOp] {
        assert_eq!(map_for(to, Some(12), None), Ok(None), "{to:?}");
    }
}

#[test]
fn no_switch_during_the_countdown() {
    assert_eq!(locked(10, false), Some(Reason::Starting), "the countdown");
    assert_eq!(locked(8, false), Some(Reason::Starting), "the map loading");
    assert_eq!(locked(9, false), None, "ready");
    assert_eq!(locked(7, false), None);
}

#[test]
fn no_switch_during_a_search() {
    assert_eq!(locked(9, true), Some(Reason::Searching));
    assert_eq!(
        locked(10, true),
        Some(Reason::Starting),
        "the start says more"
    );
    assert_eq!(
        Reason::Searching.text(),
        "Cancel the search to change mode."
    );
}

/// The retail mode each switch sets: only when it is not the lobby's own,
/// tab by tab.
#[test]
fn the_plan_sets_the_retail_mode_only_when_it_differs() {
    use RetailMode::{CoOp, Custom, Matchmaking};
    for from in Tab::ALL {
        for to in Tab::ALL {
            let p = plan(from, to);
            assert_eq!(p.to, to);
            assert_eq!(p.mode, to.parts().mode, "{from:?} to {to:?}");
            let want = (from.parts().retail != to.parts().retail).then_some(to.parts().retail);
            assert_eq!(p.retail, want, "{from:?} to {to:?}");
        }
    }
    assert_eq!(plan(Tab::Forge, Tab::CoOp).retail, Some(CoOp));
    assert_eq!(plan(Tab::CoOp, Tab::Firefight).retail, Some(Custom));
    assert_eq!(
        plan(Tab::CustomGames, Tab::Multiplayer).retail,
        Some(Matchmaking)
    );
    assert_eq!(plan(Tab::Multiplayer, Tab::CoOp).retail, Some(CoOp));
    assert_eq!(plan(Tab::Forge, Tab::Firefight).retail, None);
}

/// CO-OP runs Switch To: Co-Op Game's press after its gate: the choices
/// cleared, mode 0, the profile's difficulty held and set, the byte
/// cleared.
#[test]
fn co_op_runs_the_co_op_switch() {
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            to_co_op(&cx, SCRATCH_AT).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(&entry, guest(0), Regs::default(), |call, m| {
        let mut out = Regs::default();
        match call.target {
            CAMPAIGN_DIFFICULTY_READ => {
                m.set_u32(call.eax.unwrap(), 2).unwrap();
            }
            LOBBY_DIFFICULTY_HOLD => out.eax = call.eax.unwrap(),
            _ => {}
        }
        out
    });
    run.result.unwrap();
    assert_eq!(
        targets(&run.calls),
        [
            LOBBY_CHOICES_CLEAR,
            points::LOBBY_MODE_SET.base,
            CAMPAIGN_DIFFICULTY_READ,
            LOBBY_DIFFICULTY_HOLD,
            points::LOBBY_SET_DIFFICULTY.base,
        ]
    );
    assert_eq!(run.calls[1].args, [0], "LOBBY_MODE_SET(0)");
    assert_eq!(run.calls[2].eax, Some(SCRATCH_AT));
    assert_eq!(
        run.calls[3].eax,
        Some(2),
        "the difficulty the profile holds"
    );
    assert_eq!(run.calls[4].eax, Some(2));
    assert_eq!(run.guest.u8(CO_OP_CLEARED).unwrap(), 0);
}

/// Leaving CO-OP (or MULTIPLAYER) for the custom lobby runs Switch To:
/// Custom Game's press: the choices cleared, mode 2, the custom setup.
#[test]
fn leaving_co_op_puts_the_custom_mode_back() {
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            to_custom(&cx).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(&entry, guest(0), Regs::default(), |_, _| Regs::default());
    run.result.unwrap();
    assert_eq!(
        targets(&run.calls),
        [
            LOBBY_CHOICES_CLEAR,
            points::LOBBY_MODE_SET.base,
            CUSTOM_SETUP
        ]
    );
    assert_eq!(run.calls[1].args, [2], "LOBBY_MODE_SET(2)");
    assert_eq!(
        plan(Tab::CoOp, Tab::CustomGames).retail,
        Some(RetailMode::Custom)
    );
}

/// The whole switch from FORGE to CO-OP: the start state read, the gate
/// passed, then the co-op press; no map is set (the campaign setup picks
/// the level).
#[test]
fn co_op_from_forge_runs_after_the_checks() {
    install(SCRATCH_AT);
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let r = run_inner(&cx, Tab::Forge, Tab::CoOp, Place::SplitScreen, None).await?;
            Ok(Ret::new(u32::from(r.is_ok()), 0))
        })
    };
    let run = testing::run(&entry, guest(0), Regs::default(), |call, m| {
        let mut out = Regs::default();
        match call.target {
            t if t == points::LOBBY_START_STATE.base => out.eax = 9,
            CAMPAIGN_DIFFICULTY_READ => m.set_u32(call.eax.unwrap(), 1).unwrap(),
            LOBBY_DIFFICULTY_HOLD => out.eax = call.eax.unwrap(),
            _ => {}
        }
        out
    });
    assert_eq!(run.result.unwrap().eax, 1, "switched");
    let t = targets(&run.calls);
    assert_eq!(t[0], points::LOBBY_START_STATE.base);
    assert_eq!(&t[1..3], [LOBBY_CHOICES_CLEAR, points::LOBBY_MODE_SET.base]);
    assert!(!t.contains(&points::LOBBY_SET_MAP.base), "no map of ours");
}

#[test]
fn co_op_behind_the_retail_gate_is_refused_by_name() {
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let r = run_inner(&cx, Tab::CustomGames, Tab::CoOp, Place::SplitScreen, None).await?;
            Ok(Ret::new(u32::from(r == Err(Reason::CoOpRefused)), 0))
        })
    };
    let run = testing::run(&entry, guest(1), Regs::default(), |call, _| Regs {
        eax: if call.target == points::LOBBY_START_STATE.base {
            9
        } else {
            0
        },
        ..Regs::default()
    });
    assert_eq!(run.result.unwrap().eax, 1, "CoOpRefused");
    assert_eq!(
        targets(&run.calls),
        [points::LOBBY_START_STATE.base],
        "nothing changed"
    );
    assert!(Reason::CoOpRefused.text().ends_with('.'));
}

/// MULTIPLAYER runs the playlist press for its hopper: the choices
/// cleared, mode 3, the hopper held and published, the choice saved for
/// the leader.
#[test]
fn multiplayer_runs_the_playlist_press_for_its_hopper() {
    let entry = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            to_matchmaking(&cx, 5).await?;
            Ok(Ret::new(0, 0))
        })
    };
    let run = testing::run(&entry, guest(0), Regs::default(), |call, _| Regs {
        eax: if call.target == LOBBY_HOPPER_SET {
            0x77
        } else {
            0
        },
        ..Regs::default()
    });
    run.result.unwrap();
    assert_eq!(
        targets(&run.calls),
        [
            LOBBY_CHOICES_CLEAR,
            points::LOBBY_MODE_SET.base,
            MATCHMAKING_COMMIT,
            LOBBY_HOPPER_SET,
            LOBBY_CHOICES_PUBLISH,
            points::PLAYLIST_REMEMBER.base,
        ]
    );
    assert_eq!(run.calls[1].args, [3], "LOBBY_MODE_SET(3)");
    assert_eq!(run.calls[3].eax, Some(5), "the hopper");
    assert_eq!(
        run.calls[4].eax,
        Some(0x77),
        "EAX as the hopper set left it"
    );
    assert_eq!(run.calls[5].args, [LEADER_PLAYER, 5]);
}

#[test]
fn the_tab_opens_on_the_last_playlist_when_listed() {
    assert_eq!(opening_hopper(4, &[1, 4, 9]), Some(4));
}

#[test]
fn else_on_the_first_listed() {
    assert_eq!(opening_hopper(7, &[1, 4, 9]), Some(1), "saved, not listed");
    assert_eq!(
        opening_hopper(clankermatch::NO_HOPPER, &[4, 9]),
        Some(4),
        "none saved (or a Clanker one, which Quickmatch's getter reads as none)"
    );
}

/// The playlists listed on one page, both kinds, and with none listed the
/// tab turns off with its reason until the party leaves.
#[test]
fn no_listed_playlist_turns_the_tab_off() {
    assert_eq!(opening_hopper(4, &[]), None);
    let listed_at = |listed: &'static [u32]| {
        let entry = |cx: Cx| -> EntryFuture {
            Box::pin(async move {
                let r = opening(&cx).await?;
                Ok(Ret::new(r.unwrap_or(u32::MAX), 0))
            })
        };
        testing::run(&entry, guest(0), Regs::default(), move |call, _| {
            let eax = if call.target == points::HOPPER_LISTED.base {
                u32::from(listed.contains(&call.ecx.unwrap()))
            } else if call.target == points::QUICKMATCH_HOPPER.base {
                9
            } else {
                0
            };
            Regs {
                eax,
                ..Regs::default()
            }
        })
    };
    let none = listed_at(&[]);
    assert_eq!(none.result.unwrap().eax, u32::MAX, "Reason::NoPlaylists");
    assert!(no_playlists_seen());
    assert!(
        !super::super::available(
            Tab::Multiplayer,
            Place::XboxLive,
            super::super::Facts {
                playlists: !no_playlists_seen(),
                ..Default::default()
            }
        )
        .is_on()
    );
    let some = listed_at(&[2, 9, 12]);
    assert_eq!(some.result.unwrap().eax, 9, "the saved one, listed");
    assert!(!no_playlists_seen());
    let asked: Vec<u32> = some
        .calls
        .iter()
        .filter(|c| c.target == points::HOPPER_LISTED.base)
        .map(|c| c.ecx.unwrap())
        .collect();
    assert_eq!(
        asked,
        (0..HOPPERS).collect::<Vec<_>>(),
        "every hopper asked"
    );
    assert_eq!(clankermatch::scope(), Scope::All, "one page of both kinds");
    forget_playlists();
    assert!(!no_playlists_seen());
}
