use super::*;

use guest_layout::testing::FakeGuest;

use crate::list_class::Rows;
use crate::ui::Chosen;
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{ListEffect, Page, named, run};
use crate::ui_kit::picker;
use crate::ui_kit::{content, layout, preview, spec};
use menu::Tab;
use pages::SheetPage;
use sheet::{Act, Group, Prop, Tool};

/// Each screen's presses from a named state: the effects, then the
/// focused row.
#[test]
fn page_tables() {
    use Input::{A, B, Down, Left, Right, Up};
    use ListEffect::{Action, Close, Focus, Hook, None as Nothing, Open, Stepped};
    struct Case {
        kind: Kind,
        from: &'static str,
        inputs: &'static [Input],
        effects: Vec<ListEffect>,
        after: &'static str,
    }
    let picker = picker::NAME;
    for case in [
        // Groups in the table's order, absent ones left out: Scenery,
        // Crates, Machines. The last chosen (the Beacon) is focused.
        Case {
            kind: Kind::Objects,
            from: "open",
            inputs: &[Up, Down, Down, A],
            effects: vec![Nothing, Focus(1), Focus(2), Open(picker)],
            after: "OBJECTS | focus 2 Machines",
        },
        Case {
            kind: Kind::Objects,
            from: "first",
            inputs: &[A, B],
            effects: vec![Open(picker), Close],
            after: "OBJECTS | focus 2 Machines",
        },
        // No row draws arrows: a sideways step does nothing, A opens the
        // picker on the second row.
        Case {
            kind: Kind::Properties,
            from: "beacon",
            inputs: &[Left, Right, Down, A],
            effects: vec![Nothing, Nothing, Focus(1), Open(picker)],
            after: "BEACON | focus 1 Heading = 17.5 degrees",
        },
        // Structure and Natural follow the map's groups.
        Case {
            kind: Kind::Objects,
            from: "blockout",
            inputs: &[Down, Down, Down, Down],
            effects: vec![Focus(1), Focus(2), Focus(3), Focus(4)],
            after: "OBJECTS | focus 4 Natural",
        },
        // The Size page: a row a size, no arrows.
        Case {
            kind: Kind::Properties,
            from: "size",
            inputs: &[Down],
            effects: vec![Focus(1)],
            after: "SIZE | focus 1 Depth = 96",
        },
        // The Material page: All faces first, then each role.
        Case {
            kind: Kind::Properties,
            from: "material",
            inputs: &[Down, A],
            effects: vec![Focus(1), Open(picker)],
            after: "MATERIAL | focus 1 Floor = Dev",
        },
        Case {
            kind: Kind::Tools,
            from: "objects",
            inputs: &[Down, A],
            effects: vec![Focus(1), Action],
            after: "TOOLS | focus 1 Links",
        },
        Case {
            kind: Kind::Tools,
            from: "ai",
            inputs: &[Up, Down, A],
            effects: vec![Focus(2), Focus(3), Action],
            after: "TOOLS | focus 3 AI",
        },
        Case {
            kind: Kind::Settings,
            from: "open",
            inputs: &[A, B],
            effects: vec![Open(picker), Close],
            after: "TOOL SETTINGS | focus 0 Rotation snap = 15 degrees",
        },
        Case {
            kind: Kind::Link,
            from: "teleporter",
            inputs: &[Up, A],
            effects: vec![Focus(0), Action],
            after: "LINK | focus 0 Remove link",
        },
        Case {
            kind: Kind::Link,
            from: "device",
            inputs: &[A],
            effects: vec![Action],
            after: "LINK | focus 1 Done",
        },
        Case {
            kind: Kind::Question,
            from: "leave",
            inputs: &[Down, Up, A],
            effects: vec![Nothing, Focus(0), Action],
            after: "LEAVE GAME | focus 0 Yes",
        },
        // The Forge menu's rows open their pages above it.
        Case {
            kind: Kind::Menu,
            from: "options",
            inputs: &[Down, A, Down, A],
            effects: vec![
                Focus(1),
                Open(Kind::Settings.screen()),
                Focus(2),
                Open(Kind::Controls.screen()),
            ],
            after: "FORGE | focus 2 Controls",
        },
        // Right and Left step its tabs, each from its first row; a row of
        // the tab then opens its page.
        Case {
            kind: Kind::Menu,
            from: "options",
            inputs: &[Down, Right, Right, Left, Down, Down, Down, A],
            effects: vec![
                Focus(1),
                Hook,
                Hook,
                Hook,
                Focus(1),
                Focus(2),
                Focus(3),
                Open(Kind::Variants.screen()),
            ],
            after: "FORGE | focus 3 Map variants...",
        },
        Case {
            kind: Kind::MapLimits,
            from: "open",
            inputs: &[A, Down, A, B],
            effects: vec![Nothing, Focus(1), Nothing, Close],
            after: "MAP LIMITS | focus 1 Scenery = 4 / 16",
        },
        // A variant's row opens its dialog.
        Case {
            kind: Kind::Variants,
            from: "open",
            inputs: &[A],
            effects: vec![Open(Kind::Variant.screen())],
            after: "MAP VARIANTS | focus 0 Arena = Saved",
        },
        // Rename types a name; Delete is the last row of a variant that
        // fails a check.
        Case {
            kind: Kind::Variant,
            from: "unusable",
            inputs: &[A, Down, A],
            effects: vec![Action, Focus(1), Action],
            after: "MAP VARIANT | focus 1 Delete",
        },
        // A tick row acts and a Down moves on; Done is the last row.
        Case {
            kind: Kind::Games,
            from: "open",
            inputs: &[A, Down, A, Up],
            effects: vec![
                Stepped("All game types: On".into()),
                Focus(1),
                Stepped("King of the Hill: On".into()),
                Focus(0),
            ],
            after: "GAME TYPES | focus 0 All game types = Off",
        },
        // The Controls page only lists: A does nothing.
        Case {
            kind: Kind::Controls,
            from: "open",
            inputs: &[A, Down, B],
            effects: vec![Nothing, Focus(1), Close],
            after: "CONTROLS | focus 1 Fly = Left stick",
        },
    ] {
        let page = SheetPage::new(case.kind);
        let mut state = named(&page, case.from);
        assert_eq!(
            run(&page, &mut state, case.inputs.iter().copied()),
            case.effects,
            "{:?} from {}",
            case.kind,
            case.from
        );
        assert_eq!(page.summary(&state), case.after, "{:?}", case.kind);
    }
}

#[test]
fn no_is_focused_on_every_question() {
    for ask in [Ask::DeleteLinked, Ask::Overwrite, Ask::Leave, Ask::Load] {
        let s = sheet::question(ask, "Courtyard");
        assert_eq!(s.rows[s.focus].label, "No", "{ask:?}");
        assert_eq!(
            s.rows[s.focus].act,
            Act::Choose(Choice::Answer { ask, yes: false })
        );
    }
    for (_, s) in pages::samples(Kind::Question) {
        assert_eq!(s.rows[s.focus].label, "No");
    }
}

/// Every Forge screen in every state passes the kit's checks that need
/// no disc art: the spec's fallbacks, the content checks and the layout.
#[test]
fn layout_and_content_in_every_state() {
    let from = preview::From::Synthetic;
    for kind in Kind::ALL {
        let page = SheetPage::new(kind);
        let states = page.states();
        assert!(!states.is_empty(), "{kind:?}");
        for (name, state) in states {
            let s = page.spec(&state);
            assert_eq!(s.name, kind.screen());
            let issues = spec::check_fallback(&s);
            assert!(issues.is_empty(), "{kind:?}/{name}: {issues:?}");
            let v = from.view(&s, &name).unwrap();
            let found = content::check(&s, &v, from.source());
            assert!(found.is_empty(), "{kind:?}/{name}: {found:?}");
            let report = layout::check(&v, from.source(), &layout::Rules::for_spec(&s));
            let issues: Vec<_> = report
                .issues
                .iter()
                .filter(|i| !matches!(i, layout::Issue::Contrast { id, .. } if id.starts_with("row.") && kind.look() != Look::List))
                // A floating menu has the game behind it: no backdrop to
                // measure its texts against.
                .filter(|i| !(kind.look() == Look::Floating && matches!(i, layout::Issue::Contrast { .. })))
                .collect();
            assert!(issues.is_empty(), "{kind:?}/{name}: {issues:?}");
            for r in &s.rows {
                assert!(!r.label.contains("  "), "{kind:?}/{name}: {:?}", r.label);
                assert!(!r.label.ends_with(':'), "{kind:?}/{name}: {:?}", r.label);
            }
        }
    }
}

/// Each screen's sheet fits its text slots and, for the list pages, the
/// in-game list's lines.
#[test]
fn every_sample_fits_its_screen() {
    for kind in Kind::ALL {
        let screen = ForgeScreen::new(kind);
        for (name, s) in pages::samples(kind) {
            assert!(screen.show(s).is_ok(), "{kind:?}/{name}");
        }
    }
    let wrong = ForgeScreen::new(Kind::Tools);
    assert!(
        wrong
            .show(menu::sheet(Tab::Options, &Facts::default()))
            .unwrap_err()
            .contains("not this screen's")
    );
}

#[test]
fn the_object_menu_lists_groups_by_the_table_and_leaves_empty_ones_out() {
    let items = pages::sample_items();
    let s = sheet::objects(&items, 1).unwrap();
    let labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Scenery", "Crates", "Machines"]);
    assert_eq!(s.focus, 0, "the Beacon's group");
    let Act::Pick(p) = &s.rows[0].act else {
        panic!("a group opens the picker")
    };
    assert!(p.closes, "choosing an object closes both");
    assert_eq!(p.values[0], "Beacon", "by name");
    assert_eq!(p.values[p.current], "Beacon");
    assert_eq!(p.details[0], "Scenery. 2 placed, 12 more fit.");
    let tray = p.values.iter().position(|v| v == "Tray").unwrap();
    assert_eq!(p.details[tray], "Scenery. None placed, no more fit.");
    assert_eq!(p.choices[0], Choice::Select(1));
    assert!(sheet::objects(&[], 0).is_none(), "nothing to place");
    // Every block of a palette has a group in the table.
    for g in [
        Group::Weapons,
        Group::Vehicles,
        Group::Equipment,
        Group::Scenery,
        Group::Crates,
        Group::Machines,
        Group::Teleporters,
        Group::Spawns,
        Group::Goals,
    ] {
        assert_eq!(g.def().group, g);
    }
}

#[test]
fn properties_sheet_is_the_docs_rows_titled_by_the_object() {
    let thing = pages::sample_thing(forge::BlockKind::NetgameFlags, forge::ThingKind::Goal);
    let s = sheet::properties_sheet("Beacon", &thing, 90.).unwrap();
    assert_eq!(s.title, "BEACON");
    assert!(matches!(s.rows[0].act, Act::Open(Kind::Coordinates)));
    assert_eq!(s.rows[1].value.as_deref(), Some("90 degrees"));
    let Act::Pick(p) = &s.rows[1].act else {
        panic!("a value row opens the picker")
    };
    assert_eq!(p.choices[p.current], Choice::Set(Prop::Heading, 2));
    assert!(!p.closes, "Properties stays up after a value is chosen");
    assert_eq!(s.rows[2].value.as_deref(), Some("Red"));
    assert_eq!(s.rows[3].value.as_deref(), Some("CTF flag spawn"));
    assert_eq!(s.rows[4].value.as_deref(), Some("0"));
}

#[test]
fn coordinates_nudge_and_type_numbers() {
    let s = ForgeScreen::new(Kind::Coordinates);
    s.show(sheet::coordinates_sheet(
        "Beacon",
        forge::Frame {
            position: [1.25, -2.5, 3.75],
            rotation: [0.5, 0.25, -0.5],
        },
    ))
    .unwrap();
    publish(Facts {
        active: true,
        movement_snap: 2,
        rotation_snap: 3,
        ..Facts::default()
    });
    assert_eq!(s.sideways(0, true).as_deref(), Some("X: 1.35"));
    assert_eq!(
        s.take_choices(),
        [Choice::Nudge(sheet::Coordinate::X, true)]
    );
    assert_eq!(s.sheet().rows[0].value.as_deref(), Some("1.35"));
    // Yaw 0.5 rad is 28.65 degrees; the 45 degree snap takes it to -16.35.
    let yaw: f32 = s
        .sideways(3, false)
        .and_then(|v| v.strip_prefix("Yaw: ").map(str::to_owned))
        .unwrap()
        .parse()
        .unwrap();
    assert!((yaw + 16.35).abs() < 0.01, "{yaw}");
    assert_eq!(
        s.take_choices(),
        [Choice::Nudge(sheet::Coordinate::Yaw, false)]
    );

    let spec = sheet::TypeSpec {
        field: sheet::NumberField::Coordinate(sheet::Coordinate::Z),
        initial: "3.75".into(),
    };
    assert_eq!(ask_keyboard(spec.clone()), Chosen::Act);
    let mut guest = FakeGuest::new();
    guest.map(0x5000, 0x100);
    // The keyboard channel, empty: no keyboard is open, so the typed
    // text is read.
    guest.map(crate::keyboard_glue::CHANNEL, 0x10);
    assert!(matches!(step(&guest), Some(Step::Keyboard(_))));
    let buffer = 0x5000;
    hub().keyboard.store(buffer, Ordering::SeqCst);
    *lock(&hub().keyboard_field) = Some(spec.field);
    hub().typing.store(true, Ordering::SeqCst);
    guest
        .write(buffer, &crate::signin_keyboard::units("4.25"))
        .unwrap();
    poll_keyboard(&guest).unwrap();
    assert_eq!(
        take_choices(),
        [Choice::Number(
            sheet::NumberField::Coordinate(sheet::Coordinate::Z),
            "4.25".into()
        )]
    );
    publish(Facts::default());
}

/// A on a group in a game: the value picker opens on the game's path
/// (`kit.picker.game`, the large dialog) with the group's objects, the last
/// chosen held, and its choice reaches the session and closes the object
/// menu.
#[test]
fn a_picker_opens_with_its_pick_in_a_game() {
    let s = Arc::new(ForgeScreen::new(Kind::Objects));
    s.show(sheet::objects(&pages::sample_items(), 4).unwrap())
        .unwrap();
    assert_eq!(s.press(0), Chosen::Open(picker::NAME));
    assert_eq!(
        picker::screen_for(picker::NAME, true),
        picker::GAME_NAME,
        "in a game the picker is the large dialog's"
    );
    let pick = picker::current().expect("the group's pick");
    assert_eq!(pick.title, "SCENERY");
    assert_eq!(pick.values[pick.current], "Marker");
    assert_eq!(pick.line(0), "Scenery. 2 placed, 12 more fit.");
    assert!(!s.take_close_after_pick());
    (pick.set)(pick.current);
    assert_eq!(s.take_choices(), [Choice::Select(4)]);
    assert!(s.take_close_after_pick(), "the object menu closes too");
}

#[test]
fn the_screens_built_here_follow_the_sessions_facts() {
    publish(Facts {
        active: true,
        rotation_snap: 3,
        tool: Some(Tool::Links),
        ..Facts::default()
    });
    request(Kind::Tools).unwrap();
    assert_eq!(requested(), Some(Kind::Tools));
    let tools = screen(Kind::Tools).sheet();
    assert_eq!(tools.rows[tools.focus].label, "Links");
    assert_eq!(
        request(Kind::Settings),
        Err("A Forge screen is up.".into()),
        "one at a time"
    );
    take_request();
    request(Kind::Settings).unwrap();
    assert_eq!(
        screen(Kind::Settings).sheet().rows[0].value.as_deref(),
        Some("45 degrees")
    );
    take_request();
    assert!(
        request(Kind::Objects)
            .unwrap_err()
            .contains("the session opens it")
    );
    publish(Facts::default());
}

#[test]
fn yes_to_leave_is_kept_here() {
    publish(Facts {
        active: true,
        ..Facts::default()
    });
    send(Choice::Save);
    send(Choice::Answer {
        ask: Ask::Leave,
        yes: true,
    });
    send(Choice::Answer {
        ask: Ask::Overwrite,
        yes: false,
    });
    assert_eq!(
        take_choices(),
        [
            Choice::Save,
            Choice::Answer {
                ask: Ask::Overwrite,
                yes: false
            }
        ]
    );
    assert!(matches!(step(&FakeGuest::new()), Some(Step::Leave)));
    publish(Facts::default());
}

/// A Forge session's facts while the player flies the monitor.
fn flying() -> Facts {
    Facts {
        active: true,
        monitor: true,
        ..Facts::default()
    }
}

fn labels(s: &Sheet) -> Vec<&str> {
    s.rows.iter().map(|r| r.label.as_str()).collect()
}

/// GAME MENU is the game's in a Forge game: the driver adds no row to it
/// and names none, and of its presses takes only Leave Game's while edits
/// are not saved.
#[test]
fn game_menu_has_the_retail_rows_in_a_forge_game() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    install(&mut registry).unwrap();
    let plan = registry.build(&hooks::Base).unwrap().text();
    let ours = |point: &str| {
        plan.lines()
            .filter(|l| l.contains(&format!(" {point}: ")) && l.contains("forge-screens:"))
            .count()
    };
    assert_eq!(ours("MP_MENU_LIST_CTOR"), 0, "no row is added\n{plan}");
    assert_eq!(ours("MP_MENU_ROW_TEXT"), 0, "no row is named\n{plan}");
    assert_eq!(ours("MP_MENU_ROW_CHOSEN"), 1, "Leave Game asks first\n{plan}");
    let dirty = Facts {
        dirty: true,
        ..flying()
    };
    // Retail's action words: 0 Leave Game, 1 Settings, up to 5 End Game.
    for word in 0..=5u16 {
        assert_eq!(
            asks_before_leaving(Some(word), &dirty),
            word == LEAVE_GAME,
            "row {word}"
        );
        assert!(!asks_before_leaving(Some(word), &flying()), "saved: row {word}");
        let ended = Facts {
            active: false,
            ..dirty.clone()
        };
        assert!(!asks_before_leaving(Some(word), &ended), "no session: row {word}");
    }
    assert!(!asks_before_leaving(None, &dirty));
}

/// Back's request opens the Forge menu only while the player flies the
/// monitor; in their own body it is refused by name.
#[test]
fn back_opens_the_forge_menu_only_in_the_monitor() {
    publish(Facts {
        monitor: false,
        ..flying()
    });
    let err = request(Kind::Menu).unwrap_err();
    assert!(err.contains("monitor"), "{err}");
    assert_eq!(requested(), None);
    publish(flying());
    request(Kind::Menu).unwrap();
    assert_eq!(requested(), Some(Kind::Menu));
    let shown = screen(Kind::Menu).sheet();
    assert_eq!(shown.title, "FORGE");
    assert_eq!(menu::tab_of(&shown), Tab::Options, "its first tab");
    take_request();
    publish(Facts::default());
}

/// A swap back to the player's body drops a Forge menu asked for and
/// closes one that is up.
#[test]
fn swapping_to_the_biped_closes_the_forge_menu() {
    let body = || Facts {
        monitor: false,
        ..flying()
    };
    publish(flying());
    request(Kind::Menu).unwrap();
    publish(body());
    assert_eq!(requested(), None, "one asked for never opens");
    // Up: the class asks its rows as the screen opens.
    publish(flying());
    request(Kind::Menu).unwrap();
    take_request();
    let menu = screen(Kind::Menu);
    menu.describe(menu.count());
    assert!(menu.is_up());
    publish(flying());
    assert!(!menu.take_close_after_pick(), "it stays up in the monitor");
    publish(body());
    assert!(menu.take_close_after_pick(), "it closes at the next UI update");
    publish(Facts::default());
}

/// Another screen asked for is not the body swap's to drop.
#[test]
fn the_body_swap_leaves_other_screens_alone() {
    publish(flying());
    request(Kind::Tools).unwrap();
    publish(Facts {
        monitor: false,
        ..flying()
    });
    assert_eq!(requested(), Some(Kind::Tools));
    take_request();
    publish(Facts::default());
}

/// The Forge menu holds every row GAME MENU and Forge options had for
/// Forge, a tab at a time.
#[test]
fn the_forge_menu_lists_the_old_forge_rows() {
    let f = menu::sample_facts();
    assert_eq!(
        labels(&menu::sheet(Tab::Options, &f)),
        ["Rebuild map", "Tool settings...", "Controls", "Generate nav"]
    );
    assert_eq!(
        labels(&menu::sheet(Tab::Variants, &f)),
        ["Save", "Save as", "Load", "Map variants..."]
    );
    assert_eq!(
        labels(&menu::sheet(Tab::Waves, &f)),
        ["Test waves", "Waves...", "Enemy types...", "Build AI ground"]
    );
    assert_eq!(labels(&menu::sheet(Tab::Placed, &f)), ["Placed objects"]);
    assert_eq!(
        menu::sheet(Tab::Limits, &f).rows,
        map_limits::sheet(&f.map_limits).rows,
        "the Map limits page's rows"
    );
    let all: Vec<String> = Tab::ALL
        .into_iter()
        .flat_map(|t| menu::sheet(t, &f).rows)
        .map(|r| r.label)
        .collect();
    for old in [
        // GAME MENU's.
        "Save",
        "Save as",
        "Load",
        "Rebuild map",
        // Forge options'.
        "Tool settings...",
        "Controls",
        "Map variants...",
        "Placed objects",
        "Waves...",
        "Enemy types...",
        "Build AI ground",
        "Test waves",
        "Generate nav",
    ] {
        assert!(all.iter().any(|l| l == old), "{old} is on no tab");
    }
    // Rebuild map carries the pending count as its value.
    let rebuild = |pending_rebuild| {
        let f = Facts {
            pending_rebuild,
            ..flying()
        };
        menu::sheet(Tab::Options, &f).rows[0].clone()
    };
    assert_eq!(rebuild(2).value.as_deref(), Some("2"));
    assert_eq!(rebuild(0).value, None);
    assert_eq!(rebuild(0).act, Act::Choose(Choice::Rebuild));
    // Every tab fits the menu's screen, and names its tab.
    for tab in Tab::ALL {
        let s = menu::sheet(tab, &f);
        assert_eq!(s.title, "FORGE");
        assert_eq!(menu::tab_of(&s), tab);
        assert!(!s.rows.is_empty(), "{tab:?}");
        assert!(ForgeScreen::new(Kind::Menu).show(s).is_ok(), "{tab:?}");
    }
}

/// LB and RB (the game's White and Black) step the tabs, round the ends,
/// as Left and Right do; the rows change in place and the list is sized
/// again at the next draw.
#[test]
fn lb_and_rb_move_between_tabs() {
    use crate::list_class::{BUTTON_EVENT, Button, DOWN_EVENT, LEFT_EVENT, RIGHT_EVENT};
    const A: u32 = 0;
    const BLACK: u32 = 4;
    const WHITE: u32 = 5;
    assert_eq!(menu::turn_of(BUTTON_EVENT, WHITE), Some(false), "LB");
    assert_eq!(menu::turn_of(BUTTON_EVENT, BLACK), Some(true), "RB");
    assert_eq!(menu::turn_of(LEFT_EVENT, 0), Some(false));
    assert_eq!(menu::turn_of(RIGHT_EVENT, 0), Some(true));
    assert_eq!(menu::turn_of(BUTTON_EVENT, A), None, "A is the list's");
    assert_eq!(menu::turn_of(DOWN_EVENT, 0), None);
    assert_eq!(Tab::Options.next(false), Tab::Limits, "round the left end");
    assert_eq!(Tab::Limits.next(true), Tab::Options, "round the right end");
    let mut seen = vec![Tab::Options];
    while seen.len() < Tab::ALL.len() {
        seen.push(seen[seen.len() - 1].next(true));
    }
    assert_eq!(seen, Tab::ALL);

    publish(menu::sample_facts());
    request(Kind::Menu).unwrap();
    take_request();
    let screen = screen(Kind::Menu);
    let class = screen.class();
    let hook = class.behaviour().hook.expect("the menu takes buttons");
    assert_eq!(screen.growing_capacity(), Some(screen.max_rows()));
    assert!(screen.shrink_growing_rows());
    assert!(!screen.live_text().redraw, "nothing changed yet");
    assert_eq!(hook(BUTTON_EVENT, BLACK), Button::Handled);
    let shown = screen.sheet();
    assert_eq!(menu::tab_of(&shown), Tab::Variants);
    assert_eq!(labels(&shown)[0], "Save");
    assert_eq!(screen.count(), shown.rows.len());
    assert!(screen.live_text().redraw, "the list is sized again");
    assert!(!screen.live_text().redraw, "once");
    assert_eq!(hook(BUTTON_EVENT, WHITE), Button::Handled);
    assert_eq!(hook(BUTTON_EVENT, WHITE), Button::Handled);
    assert_eq!(menu::tab_of(&screen.sheet()), Tab::Limits);
    assert_eq!(hook(RIGHT_EVENT, 0), Button::Handled);
    assert_eq!(menu::tab_of(&screen.sheet()), Tab::Options);
    assert_eq!(hook(BUTTON_EVENT, A), Button::Pass);
    assert_eq!(hook(DOWN_EVENT, 0), Button::Pass);
    // The next opening shows the tab it was left on.
    assert_eq!(hook(BUTTON_EVENT, BLACK), Button::Handled);
    request(Kind::Menu).unwrap();
    assert_eq!(menu::tab_of(&screen.sheet()), Tab::Variants);
    take_request();
    // No other screen takes the bumpers.
    let settings = super::screen(Kind::Settings).class();
    assert!(settings.behaviour().hook.is_none());
    publish(Facts::default());
}

/// The strip shows the open tab bright and in capitals, the others muted,
/// with the bumpers at its ends; every name fits its cell and no cell
/// lies on another.
#[test]
fn the_strip_shows_the_open_tab_bright() {
    use crate::ui_kit::spec::fallback;
    let panel = menu::panel();
    assert_eq!(panel.texts.len(), menu::STRIP_TEXTS);
    assert!(panel.texts.len() <= crate::list_class::MAX_PANEL_TEXTS);
    for tab in Tab::ALL {
        let strip = menu::strip(tab);
        assert_eq!(strip.len(), menu::STRIP_TEXTS);
        for t in Tab::ALL {
            let (bright, muted) = (&strip[2 * t.index()], &strip[2 * t.index() + 1]);
            if t == tab {
                assert_eq!(*bright, t.label().to_uppercase());
                assert!(muted.is_empty(), "{t:?}");
            } else {
                assert!(bright.is_empty(), "{t:?}");
                assert_eq!(muted, t.label());
            }
        }
        assert_eq!(strip[menu::STRIP_TEXTS - 2], "\u{e105}", "LB");
        assert_eq!(strip[menu::STRIP_TEXTS - 1], "\u{e104}", "RB");
        let texts = menu::pane_texts(tab);
        assert_eq!(texts.len(), strip.len());
        for (t, text) in texts.iter().zip(&strip) {
            let [top, left, bottom, right] = t.rect;
            assert!(top > bottom && right > left);
            assert!(
                fallback::width(text) <= i32::from(right - left),
                "{text:?} is wider than its cell"
            );
            assert!(!text.contains("  ") && text.trim() == text, "{text:?}");
        }
    }
    // A tab's two texts share a cell; cells of different tabs, and the
    // bumpers', are apart, left to right.
    let cells: Vec<[i16; 4]> = panel.texts.iter().map(|t| t.block.rect).collect();
    for i in 0..Tab::ALL.len() {
        assert_eq!(cells[2 * i], cells[2 * i + 1]);
    }
    let mut order = vec![cells[menu::STRIP_TEXTS - 2]];
    order.extend((0..Tab::ALL.len()).map(|i| cells[2 * i]));
    order.push(cells[menu::STRIP_TEXTS - 1]);
    for pair in order.windows(2) {
        assert!(pair[0][3] <= pair[1][1], "{pair:?} overlap");
    }
    // The strip is a screen text of the menu alone.
    let spec = |kind| {
        let screen = ForgeScreen::new(kind);
        if kind == Kind::Menu {
            screen
                .show(menu::sheet(Tab::Waves, &menu::sample_facts()))
                .unwrap();
        }
        crate::ui_kit::spec::Describe::spec(&screen)
    };
    let texts = spec(Kind::Menu).texts;
    assert_eq!(texts[2 * Tab::Waves.index()].text, "WAVES");
    assert!(spec(Kind::Settings).texts.is_empty());
}

/// Save writes a named session's variant and asks an unnamed one's name
/// on the game's keyboard, as Save as does; either closes the menu.
#[test]
fn save_asks_for_a_name_until_the_session_has_one() {
    let row = |f: &Facts, label: &str| {
        menu::sheet(Tab::Variants, f)
            .rows
            .into_iter()
            .find(|r| r.label == label)
            .unwrap()
    };
    let named = Facts {
        filename: Some("Courtyard".into()),
        ..flying()
    };
    assert_eq!(row(&named, "Save").act, Act::Choose(Choice::Save));
    assert_eq!(
        row(&named, "Save").description.as_deref(),
        Some("Save Courtyard.")
    );
    assert_eq!(row(&named, "Save as").act, Act::SaveAs);
    assert_eq!(row(&flying(), "Save").act, Act::SaveAs);

    publish(flying());
    request(Kind::Menu).unwrap();
    take_request();
    let screen = screen(Kind::Menu);
    while menu::tab_of(&screen.sheet()) != Tab::Variants {
        screen.turn_tab(true);
    }
    assert_eq!(labels(&screen.sheet())[1], "Save as");
    assert_eq!(screen.press(1), Chosen::Act, "the menu's action closes it");
    assert!(matches!(
        step(&FakeGuest::new()),
        Some(Step::Keyboard(Keyboard::Name))
    ));
    assert!(
        screen.take_choices().is_empty(),
        "the name comes from the keyboard"
    );
    // While a keyboard is open another is refused by name.
    hub().typing.store(true, Ordering::SeqCst);
    assert_eq!(
        screen.press(1),
        Chosen::Stepped("A keyboard is already open.".into())
    );
    hub().typing.store(false, Ordering::SeqCst);
    publish(Facts::default());
}

/// Load picks one of this map's variants that load and closes the menu
/// with the pick; with none it is a fact.
#[test]
fn load_picks_a_variant_and_closes_the_menu() {
    let f = menu::sample_facts();
    let load = menu::sheet(Tab::Variants, &f).rows.remove(2);
    let Act::Pick(p) = &load.act else {
        panic!("Load picks");
    };
    assert_eq!(p.title, "Load");
    assert!(p.closes, "its question needs no screen of ours up");
    assert_eq!(
        p.values,
        ["Arena", "Courtyard"],
        "the ones that load: not the Xbox Live one (Two Towers) nor the one that fails a check (Old Arena)"
    );
    assert_eq!(p.values[p.current], "Courtyard", "the open one");
    assert_eq!(p.choices[0], Choice::Load("Arena".into()));
    assert_eq!(p.choices.len(), p.values.len());
    let none = menu::sheet(Tab::Variants, &flying()).rows.remove(2);
    assert_eq!(none.act, Act::Info);
    assert_eq!(none.value.as_deref(), Some("None"));
    assert_eq!(none.description.as_deref(), Some(variants::NONE_SAVED));
}

/// Each screen's retail base resolves in `shared.map`, where a game's UI
/// finds its screens: the large dialog (240) and the in-game Controller
/// Settings (200).
#[test]
#[ignore = "reads the disc named by H2_DISC"]
fn each_base_resolves_in_shared() {
    let disc = std::path::PathBuf::from(std::env::var("H2_DISC").expect("H2_DISC"));
    let shared = preview::shared_screen_ids(&disc).unwrap();
    for kind in Kind::ALL {
        let id = kind.look().ingame_screen_id();
        assert!(
            shared.binary_search(&id).is_ok(),
            "{}: screen {id} is not in shared.map",
            kind.screen()
        );
    }
}

/// Every row of the Forge menu that acts has its help, player copy that
/// fits the settings dialog's help block in two lines.
#[test]
fn every_forge_menu_row_has_its_help() {
    use crate::ui_kit::dialog::SETTINGS_HELP_TEXT;
    use crate::ui_kit::spec::{fallback, wrap};
    let [top, left, bottom, right] = SETTINGS_HELP_TEXT.rect;
    let (width, height) = (i32::from(right - left), i32::from(top - bottom));
    let f = menu::sample_facts();
    let rows: Vec<sheet::SheetRow> = [Tab::Options, Tab::Variants, Tab::Waves, Tab::Placed]
        .into_iter()
        .flat_map(|t| menu::sheet(t, &f).rows)
        .chain(menu::sheet(Tab::Variants, &flying()).rows)
        .chain(menu::sheet(Tab::Placed, &flying()).rows)
        .collect();
    for r in &rows {
        let help = r.description.as_deref().unwrap_or_default();
        assert!(!help.trim().is_empty(), "{}: no help", r.label);
        let lines = wrap(help, width, &fallback::width).len() as i32;
        assert!(
            lines <= 2 && lines * fallback::LINE <= height,
            "{}: {lines} lines",
            r.label
        );
        let lower = help.to_lowercase();
        for bad in ["docs/", "todo", "placeholder", "tbd"] {
            assert!(!lower.contains(bad), "{}: {help}", r.label);
        }
    }
}

fn ai_items(placed: usize) -> Vec<sheet::AiItem> {
    forge::GroundLabel::ALL
        .into_iter()
        .map(|label| sheet::AiItem { label, placed })
        .collect()
}

#[test]
fn the_ai_and_objectives_page_follows_the_groups_and_places_markers() {
    let items = pages::sample_items();
    let ai = ai_items(0);
    let s = sheet::objects_with_ai(&items, 1, &ai).unwrap();
    let labels: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Scenery", "Crates", "Machines", sheet::AI_PAGE, sheet::SIEGE_PAGE],
        "after the map's groups"
    );
    assert_eq!(s.focus, 0, "the last chosen object keeps the focus");
    let Act::Pick(p) = &s.rows[3].act else {
        panic!("the page opens the picker")
    };
    assert!(p.closes, "choosing a label closes both");
    assert_eq!(p.values, ["Wave drop", "Base", "Cover"]);
    assert_eq!(p.details[0], "ff_drop. None placed");
    assert_eq!(p.choices[0], Choice::Marker(forge::GroundLabel::Drop));
    // The page is the same one with the counts the session has.
    let s = sheet::objects_with_ai(&items, 1, &ai_items(3)).unwrap();
    let Act::Pick(p) = &s.rows[3].act else {
        panic!("the page opens the picker")
    };
    assert_eq!(p.details[1], "ff_base. 3 placed");
}

/// The Siege page is the layout recipe: Site, Flag stand, Spawn line and
/// Barrier, each writing the label `siege.rules` reads.
#[test]
fn the_siege_page_offers_the_layout_recipes_four_labels() {
    let s = sheet::objects_with_ai(&pages::sample_items(), 1, &ai_items(0)).unwrap();
    let Act::Pick(p) = &s.rows[4].act else {
        panic!("the page opens the picker")
    };
    assert_eq!(s.rows[4].label, sheet::SIEGE_PAGE);
    assert!(p.closes);
    assert_eq!(p.values, ["Site", "Flag stand", "Spawn line", "Barrier"]);
    assert_eq!(
        p.details,
        [
            "siege_site. None placed",
            "siege_goal. None placed",
            "siege_line. None placed",
            "siege_barrier. None placed"
        ]
    );
    assert_eq!(
        p.choices,
        forge::GroundLabel::SIEGE.map(Choice::Marker).to_vec()
    );
    // A Siege label alone gives the Siege page alone.
    let one = [sheet::AiItem { label: forge::GroundLabel::Goal, placed: 1 }];
    let s = sheet::objects_with_ai(&[], 0, &one).unwrap();
    let rows: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(rows, [sheet::SIEGE_PAGE]);
}

#[test]
fn the_page_is_alone_on_a_map_with_nothing_to_place_and_absent_with_no_labels() {
    let s = sheet::objects_with_ai(&[], 0, &ai_items(0)).unwrap();
    let rows: Vec<&str> = s.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(rows, [sheet::AI_PAGE, sheet::SIEGE_PAGE]);
    assert!(sheet::objects_with_ai(&[], 0, &[]).is_none());
    let items = pages::sample_items();
    assert_eq!(
        sheet::objects_with_ai(&items, 1, &[]).unwrap().rows.len(),
        sheet::objects(&items, 1).unwrap().rows.len()
    );
    // It fits the object menu's screen.
    let screen = ForgeScreen::new(Kind::Objects);
    assert!(
        screen
            .show(sheet::objects_with_ai(&items, 1, &ai_items(0)).unwrap())
            .is_ok()
    );
}

#[test]
fn the_forge_menu_builds_the_ground_and_tests_waves() {
    let waves = menu::sheet(Tab::Waves, &flying());
    let first = menu::sheet(Tab::Options, &flying());
    let rows: Vec<(&str, &Act)> = waves
        .rows
        .iter()
        .chain(&first.rows)
        .map(|r| (r.label.as_str(), &r.act))
        .collect();
    assert!(rows.contains(&("Build AI ground", &Act::Choose(Choice::BuildGround))));
    assert!(rows.contains(&("Test waves", &Act::Choose(Choice::TestWaves))));
    assert!(rows.contains(&("Generate nav", &Act::Choose(Choice::GenerateNav))));
    assert!(ForgeScreen::new(Kind::Menu).show(waves.clone()).is_ok());
}

/// While a wave test runs (or is starting) the Test waves row reads Stop
/// waves in its place.
#[test]
fn the_row_reads_stop_waves_while_a_test_runs() {
    let rows = |s: &Sheet| {
        s.rows
            .iter()
            .map(|r| (r.label.clone(), r.act.clone()))
            .collect::<Vec<_>>()
    };
    let on = |testing| {
        let f = Facts {
            testing,
            ..flying()
        };
        menu::sheet(Tab::Waves, &f)
    };
    let idle = rows(&on(false));
    let testing = rows(&on(true));
    let at = idle
        .iter()
        .position(|(label, _)| label == "Test waves")
        .unwrap();
    assert_eq!(testing[at], ("Stop waves".into(), Act::Choose(Choice::StopWaves)));
    assert!(!testing.iter().any(|(label, _)| label == "Test waves"));
    assert_eq!(idle.len(), testing.len(), "one row in the other's place");
    assert!(ForgeScreen::new(Kind::Menu).show(on(true)).is_ok());
}

/// The Overlays page has a row for each overlay of the table, showing On
/// or Off, and its picker sends the overlay with the value's index.
#[test]
fn the_overlays_page_has_a_row_for_each_overlay() {
    use forge::tool::{Overlay, Overlays};
    let mut on = Overlays::default();
    on.set(Overlay::Monitors, false);
    let page = sheet::overlays(on);
    assert_eq!(page.title, "OVERLAYS");
    assert_eq!(page.rows.len(), sheet::OVERLAYS.len());
    assert_eq!(sheet::OVERLAYS.len(), Overlay::ALL.len());
    let values: Vec<_> = page.rows.iter().map(|r| r.value.as_deref()).collect();
    assert_eq!(
        values,
        [Some("On"), Some("Off"), Some("On"), Some("On"), Some("On")]
    );
    let Act::Pick(p) = &page.rows[1].act else {
        panic!("a value row opens a picker");
    };
    assert_eq!(p.values, ["On", "Off"]);
    assert_eq!(p.current, 1);
    assert_eq!(
        p.choices,
        [
            Choice::Set(Prop::Overlay(Overlay::Monitors), 0),
            Choice::Set(Prop::Overlay(Overlay::Monitors), 1)
        ]
    );
}

fn placed_sample() -> Vec<sheet::Placed> {
    use forge_doc::OpId;
    sheet::placed_rows(&[
        (
            OpId(7),
            sheet::Group::Scenery,
            "Beacon".into(),
            [1., 2., 3.],
        ),
        (
            OpId(2),
            sheet::Group::Crates,
            "Barrel".into(),
            [0., 0., 0.5],
        ),
        (
            OpId(5),
            sheet::Group::Scenery,
            "Beacon".into(),
            [-4.25, 0., 0.],
        ),
        (OpId(9), sheet::Group::Scenery, "Arch".into(), [0., 0., 0.]),
    ])
}

/// The browser lists groups in the object menu's order, then names, and
/// numbers an object the map holds more than once in the order placed.
#[test]
fn the_placed_objects_are_grouped_named_and_numbered() {
    let rows = placed_sample();
    let names: Vec<&str> = rows.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Arch", "Beacon 1", "Beacon 2", "Barrel"]);
    let ids: Vec<u32> = rows.iter().map(|p| p.id.0).collect();
    assert_eq!(ids, [9, 7, 5, 2], "numbered in the order placed");
    assert_eq!(rows[2].line, "Scenery. At -4.25, 0, 0.");
    assert_eq!(rows[3].line, "Crate. At 0, 0, 0.5.");
}

/// The Placed objects row opens a picker whose rows fly to their object;
/// with nothing placed it is a fact, and the page still fits its screen.
#[test]
fn the_placed_objects_row_picks_an_object_to_fly_to() {
    let rows = placed_sample();
    let placed = Facts {
        placed: rows.clone(),
        ..flying()
    };
    let page = menu::sheet(Tab::Placed, &placed);
    let row = page
        .rows
        .iter()
        .find(|r| r.label == "Placed objects")
        .unwrap();
    assert_eq!(row.value.as_deref(), Some("4"));
    let Act::Pick(p) = &row.act else {
        panic!("a browser picks");
    };
    assert_eq!(p.values, ["Arch", "Beacon 1", "Beacon 2", "Barrel"]);
    assert_eq!(p.details.len(), 4);
    assert_eq!(p.choices[1], Choice::FlyTo(forge_doc::OpId(7)));
    assert!(ForgeScreen::new(Kind::Menu).show(page.clone()).is_ok());
    let none = menu::sheet(Tab::Placed, &flying());
    let row = none
        .rows
        .iter()
        .find(|r| r.label == "Placed objects")
        .unwrap();
    assert_eq!(row.act, Act::Info);
    assert_eq!(row.value.as_deref(), Some("None"));
}

fn group_info(id: u32, name: &str, members: u32, locked: bool) -> forge::GroupInfo {
    forge::GroupInfo {
        id: forge_doc::OpId(id),
        name: name.into(),
        locked,
        pivot_first: false,
        members: (0..members)
            .map(|i| forge::Endpoint::Added(forge_doc::OpId(100 + i)))
            .collect(),
    }
}

/// The browser lists the groups before the objects, each flying to its
/// centre, and the page still fits its screen.
#[test]
fn the_browser_lists_groups_first() {
    let folders = sheet::folder_rows(&[
        group_info(3, "Roof", 3, false),
        group_info(8, "Wall", 2, true),
    ]);
    assert_eq!(folders[0].line, "Group. 3 objects.");
    assert_eq!(folders[1].line, "Group. 2 objects, locked.");
    let rows = placed_sample();
    let both = Facts {
        folders: folders.clone(),
        placed: rows.clone(),
        ..flying()
    };
    let page = menu::sheet(Tab::Placed, &both);
    let row = page
        .rows
        .iter()
        .find(|r| r.label == "Placed objects")
        .unwrap();
    assert_eq!(row.value.as_deref(), Some("6"));
    let Act::Pick(p) = &row.act else {
        panic!("a browser picks");
    };
    assert_eq!(&p.values[..3], ["Roof", "Wall", "Arch"]);
    assert_eq!(p.choices[0], Choice::FlyToGroup(forge_doc::OpId(3)));
    assert_eq!(p.choices[2], Choice::FlyTo(forge_doc::OpId(9)));
    assert_eq!(p.details.len(), p.values.len());
    assert!(ForgeScreen::new(Kind::Menu).show(page.clone()).is_ok());
    // Groups alone are enough to make the row a browser.
    let groups = Facts {
        folders,
        ..flying()
    };
    let only = menu::sheet(Tab::Placed, &groups);
    let row = only
        .rows
        .iter()
        .find(|r| r.label == "Placed objects")
        .unwrap();
    assert!(matches!(row.act, Act::Pick(_)));
}

/// The Properties page offers Select to a free thing, Group selection
/// with two selected, and the group's rows to a member.
#[test]
fn the_properties_page_has_selection_and_group_rows() {
    let thing = forge::Endpoint::Added(forge_doc::OpId(4));
    let facts = |selected, selection, group| sheet::GroupFacts {
        thing,
        selected,
        selection,
        group,
        pivot_editable: true,
    };
    let labels = |f: &sheet::GroupFacts| -> Vec<String> {
        sheet::group_rows(f).into_iter().map(|r| r.label).collect()
    };
    assert_eq!(labels(&facts(false, 0, None)), ["Select"]);
    assert_eq!(labels(&facts(true, 1, None)), ["Deselect"]);
    assert_eq!(
        labels(&facts(true, 2, None)),
        ["Deselect", "Group selection"]
    );
    let rows = sheet::group_rows(&facts(
        false,
        0,
        Some(group_info(2, "Roof", 2, false)),
    ));
    assert_eq!(
        rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
        ["Group", "Lock group", "Pivot", "Ungroup"]
    );
    assert_eq!(rows[0].value.as_deref(), Some("Roof"));
    assert_eq!(
        rows[1].act,
        Act::Choose(Choice::LockGroup(forge_doc::OpId(2), true))
    );
    assert_eq!(
        rows[3].act,
        Act::Choose(Choice::Ungroup(forge_doc::OpId(2)))
    );
    let locked = sheet::group_rows(&facts(
        false,
        0,
        Some(group_info(2, "Roof", 2, true)),
    ));
    assert_eq!(locked[1].label, "Unlock group");
    assert_eq!(
        locked[1].act,
        Act::Choose(Choice::LockGroup(forge_doc::OpId(2), false))
    );
    for r in rows.iter().chain(&locked) {
        assert!(
            r.description
                .as_deref()
                .is_some_and(|d| !d.trim().is_empty())
        );
    }
}

#[test]
fn the_pivot_picker_opens_on_the_current_value_and_names_the_group() {
    let mut group = group_info(2, "Roof", 2, false);
    for (first, value) in [(false, "Centre"), (true, "First member")] {
        group.pivot_first = first;
        let mut facts = sheet::GroupFacts {
            thing: group.members[0],
            selected: false,
            selection: 0,
            group: Some(group.clone()),
            pivot_editable: true,
        };
        let rows = sheet::group_rows(&facts);
        let row = rows.iter().find(|r| r.label == "Pivot").unwrap();
        assert_eq!(row.value.as_deref(), Some(value));
        let Act::Pick(pick) = &row.act else {
            panic!("the pivot opens a picker");
        };
        assert_eq!(pick.values, ["Centre", "First member"]);
        assert_eq!(pick.values[pick.current], value);
        assert_eq!(
            pick.choices,
            [
                Choice::SetGroupPivot(group.id, false),
                Choice::SetGroupPivot(group.id, true)
            ]
        );
        assert!(pick.closes);
        facts.pivot_editable = false;
        assert!(sheet::group_rows(&facts).iter().all(|r| r.label != "Pivot"));
    }
}

#[test]
fn a_kit_session_adds_spawns_teleporters_and_item_spawners_to_the_object_menu() {
    let items = [sheet::Item {
        group: Group::Weapons,
        name: "Rifle".into(),
        placed: 0,
        left: 5,
        hidden: false,
    }];
    let kit = sheet::KitMenu {
        items: vec![
            sheet::KitItem {
                index: 0,
                group: Group::Weapons,
                name: "Rifles".into(),
            },
            sheet::KitItem {
                index: 1,
                group: Group::Vehicles,
                name: "Hogs".into(),
            },
        ],
        items_left: 1,
        spawns_left: 2,
        flags_left: 0,
    };
    let s = sheet::with_kit(sheet::objects(&items, 0), &kit);
    let labels: Vec<&str> = s.rows.iter().map(|r| &*r.label).collect();
    // The groups are in the table's order, the map's own among them.
    assert_eq!(labels, ["Weapons", "Vehicles", "Teleporters", "Spawns"]);
    let pick = |label: &str| {
        let row = s.rows.iter().find(|r| &*r.label == label).unwrap();
        let Act::Pick(p) = &row.act else {
            panic!("a group opens the picker")
        };
        p.clone()
    };
    let weapons = pick("Weapons");
    assert_eq!(weapons.values, ["Rifle", "Rifles spawner"]);
    assert_eq!(weapons.choices, [Choice::Select(0), Choice::Item(0)]);
    assert_eq!(weapons.details[1], "Item spawner. 1 more fits.");
    assert_eq!(pick("Vehicles").choices, [Choice::Item(1)]);
    let spawns = pick("Spawns");
    assert_eq!(spawns.values[0], "Red start");
    assert_eq!(spawns.choices[1], Choice::Spawn(1));
    assert_eq!(spawns.details[0], "Starting point. 2 more fit.");
    let teleporters = pick("Teleporters");
    assert_eq!(teleporters.values, ["Sender", "Receiver"]);
    assert_eq!(
        teleporters.choices,
        [Choice::Teleporter(true), Choice::Teleporter(false)]
    );
    assert_eq!(teleporters.details[0], "Teleporter. no more fit.");
    assert!(spawns.closes && teleporters.closes);
}

#[test]
fn the_goals_group_lists_the_goals_a_game_type_reads() {
    let all = sheet::with_goals(None);
    let Act::Pick(p) = &all.rows[0].act else {
        panic!("Goals opens its picker")
    };
    assert_eq!(p.values, ["Hill", "Territory", "Siege site"]);
    let some = sheet::with_goals_for(None, &[forge::Goal::Hill, forge::Goal::Site]);
    let Act::Pick(p) = &some.rows[0].act else {
        panic!("Goals opens its picker")
    };
    assert_eq!(p.values, ["Hill", "Siege site"]);
    assert_eq!(p.choices[0], Choice::Goal(forge::Goal::Hill));
    // A game type that reads none has no Goals group.
    assert!(sheet::with_goals_for(None, &[]).rows.is_empty());
}

#[test]
fn the_object_menu_offers_every_shape_and_each_start_is_in_the_starter_set() {
    use forge::piece::{self, NATURAL, SEEDS, STRUCTURE};
    let menu = blockout::with_groups(sheet::objects(&pages::sample_items(), 1));
    let labels: Vec<&str> = menu.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Scenery", "Crates", "Machines", "Structure", "Natural"]);
    let Act::Pick(structure) = &menu.rows[3].act else {
        panic!("a group opens the picker")
    };
    assert_eq!(structure.values.len(), STRUCTURE.len());
    assert_eq!(structure.choices[0], Choice::Piece(forge::Shape::Block, 0));
    assert!(structure.closes, "choosing a shape closes both");
    let Act::Pick(natural) = &menu.rows[4].act else {
        panic!("a group opens the picker")
    };
    assert_eq!(natural.values.len(), NATURAL.len() * usize::from(SEEDS));
    assert_eq!(natural.values[0], "Boulder 1");
    let starter: Vec<String> = forge_shapes::starter().iter().map(|d| d.name()).collect();
    for shape in STRUCTURE {
        assert!(starter.contains(&piece::start(shape, 0).name()), "{shape}");
    }
}

#[test]
fn a_hidden_palette_row_is_left_out_of_the_menu() {
    let mut items = pages::sample_items();
    for item in &mut items {
        item.hidden = item.group == Group::Crates;
    }
    let menu = sheet::objects(&items, 0).unwrap();
    assert!(menu.rows.iter().all(|r| r.label != "Crates"));
}

#[test]
fn a_pieces_rows_are_resize_size_material_and_for_a_rock_reroll() {
    let facts = blockout::sample();
    let labels = |facts: &blockout::PieceFacts| -> Vec<String> {
        blockout::piece_rows(facts).into_iter().map(|r| r.label).collect()
    };
    assert_eq!(labels(&facts), ["Resize", "Size...", "Material...", "Reroll"]);
    let block = blockout::PieceFacts {
        def: forge::piece::start(forge::Shape::Block, 0),
        ..facts
    };
    assert_eq!(labels(&block), ["Resize", "Size...", "Material..."]);
    let size = blockout::size_sheet(&block.def);
    let rows: Vec<&str> = size.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(rows, ["Width", "Depth", "Height"]);
    let stairs = blockout::size_sheet(&forge::piece::start(forge::Shape::Stairs, 0));
    assert_eq!(stairs.rows.last().unwrap().label, "Steps");
}

#[test]
fn the_tool_settings_have_the_block_out_grid_and_the_overlays_a_dev_view() {
    let s = sheet::settings_with(2, 0, false, 6);
    let grid = s.rows.iter().find(|r| r.label == "Block-out grid").unwrap();
    assert_eq!(grid.value.as_deref(), Some("64"));
    let o = sheet::overlays_with(forge::tool::Overlays::default(), true);
    assert_eq!(o.rows.len(), sheet::OVERLAYS.len() + 1);
    assert_eq!(o.rows.last().unwrap().label, "Dev view");
    assert_eq!(o.rows.last().unwrap().value.as_deref(), Some("On"));
}
