use super::*;

use engine::GuestMemory as _;
use engine::datum::DatumArray;
use engine::testing::FakeGuest;

use crate::list_class::Rows;
use crate::ui::Chosen;
use crate::ui_kit::input::Input;
use crate::ui_kit::page::{ListEffect, Page, named, run};
use crate::ui_kit::{content, layout, preview, spec};
use pages::SheetPage;
use sheet::{Act, Group, Prop, Tool};

/// Each screen's presses from a named state: the effects, then the
/// focused row (the compositor's tables, `ui_kit/forge/tests.rs`, on the
/// game's screens).
#[test]
fn page_tables() {
    use Input::{A, B, Down, Left, Right, Up};
    use ListEffect::{Action, Close, Focus, None as Nothing, Open};
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
        Case {
            kind: Kind::Tools,
            from: "objects",
            inputs: &[Down, A],
            effects: vec![Focus(1), Action],
            after: "TOOLS | focus 1 Links",
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
        Case {
            kind: Kind::Options,
            from: "open",
            inputs: &[A, Down, A],
            effects: vec![
                Open(Kind::Settings.screen()),
                Focus(1),
                Open(Kind::Controls.screen()),
            ],
            after: "FORGE OPTIONS | focus 1 Controls",
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
    for ask in [Ask::DeleteLinked, Ask::Overwrite, Ask::Leave] {
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
            .show(sheet::options())
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
fn properties_come_from_the_table_by_kind() {
    let labels = |group, kind| {
        sheet::properties(group, kind)
            .map(|d| d.label)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(Group::Scenery, forge::ThingKind::Object),
        ["Coordinates...", "Heading"]
    );
    assert_eq!(
        labels(Group::Spawns, forge::ThingKind::StartingPoint),
        ["Coordinates...", "Heading", "Team"]
    );
    assert_eq!(
        labels(Group::Goals, forge::ThingKind::Goal),
        ["Coordinates...", "Heading", "Team", "Role", "Order"]
    );
    assert_eq!(
        labels(Group::Equipment, forge::ThingKind::ItemSpawner),
        ["Coordinates...", "Heading", "Respawn time"]
    );
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

const LIST: u32 = 0x1000;
const ARRAY: u32 = 0x2000;
const NAME: u32 = 0x3000;

/// A GAME MENU rows' array with `actions` live, room for `capacity`.
fn menu(capacity: u32, actions: &[u16]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(LIST, 0x100).map(ARRAY, 0x1000).map(NAME, 0x20);
    ListHandler::at(LIST).set_rows(&mut g, ARRAY).unwrap();
    let rows = DatumArray::at(ARRAY);
    rows.initialize_contiguous(&mut g, NAME, capacity as i32, 4, 0, 0)
        .unwrap();
    rows.make_valid(&mut g).unwrap();
    for &action in actions {
        let handle = rows.new_datum(&mut g).unwrap().unwrap();
        let datum = rows.try_get(&g, handle).unwrap().unwrap();
        g.set_u16(datum + 2, action).unwrap();
    }
    g
}

fn words(g: &FakeGuest) -> Vec<u16> {
    let rows = DatumArray::at(ARRAY);
    let high = rows
        .header_field(g, engine::datum::layout::HIGH_WATER)
        .unwrap();
    (0..high)
        .filter_map(|i| rows.get_absolute(g, i).unwrap())
        .map(|e| g.u16(e + 2).unwrap())
        .collect()
}

#[test]
fn game_menu_adds_five_rows() {
    let labels: Vec<&str> = game_menu::ROWS.iter().map(|r| r.label()).collect();
    assert_eq!(
        labels,
        ["Save", "Save as", "Load", "Rebuild map", "Forge options..."]
    );
    let retail = [0u16, 1, 2, 3];
    // The room grows by our five, only for GAME MENU's array and only
    // while a session runs.
    let room = game_menu::list_count(game_menu::ARRAY_NAME, game_menu::RETAIL_ROOM, true);
    assert_eq!(room, game_menu::RETAIL_ROOM + 5);
    assert_eq!(game_menu::list_count(game_menu::ARRAY_NAME, 6, false), 6);
    assert_eq!(game_menu::list_count(0x1234, 6, true), 6);
    let mut g = menu(room, &retail);
    let n = game_menu::put_first(&mut g, ARRAY, &game_menu::actions()).unwrap();
    assert_eq!(n, 9);
    let mut want = game_menu::actions();
    want.extend(retail);
    assert_eq!(
        words(&g),
        want,
        "ours first, the retail rows after in order"
    );
    for (i, row) in game_menu::ROWS.iter().enumerate() {
        assert_eq!(MenuRow::of_action(want[i]), Some(*row));
    }
    assert_eq!(MenuRow::of_action(game_menu::LEAVE_GAME), None);
    // Without the room grown, refused by name.
    let mut small = menu(game_menu::RETAIL_ROOM, &retail);
    let err = game_menu::put_first(&mut small, ARRAY, &game_menu::actions()).unwrap_err();
    assert!(err.contains("room was not grown"), "{err}");
    // Rebuild map carries the pending count.
    assert_eq!(MenuRow::Rebuild.text(0), "Rebuild map");
    assert_eq!(MenuRow::Rebuild.text(2), "Rebuild map  2");
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

/// Every Forge options row has its help, player copy that fits the
/// settings dialog's help block in two lines.
#[test]
fn every_forge_option_has_its_help() {
    use crate::ui_kit::dialog::SETTINGS_HELP_TEXT;
    use crate::ui_kit::spec::{fallback, wrap};
    let [top, left, bottom, right] = SETTINGS_HELP_TEXT.rect;
    let (width, height) = (i32::from(right - left), i32::from(top - bottom));
    let page = sheet::options();
    assert_eq!(page.rows.len(), sheet::OPTIONS.len());
    for r in &page.rows {
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
