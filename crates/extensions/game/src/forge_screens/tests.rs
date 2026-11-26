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
/// focused row.
#[test]
fn page_tables() {
    use Input::{A, B, Down, Left, Right, Up};
    use ListEffect::{Action, Close, Focus, None as Nothing, Open, Stepped};
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
        Case {
            kind: Kind::Options,
            from: "open",
            inputs: &[Down, Down, Down, A],
            effects: vec![Focus(1), Focus(2), Focus(3), Open(Kind::MapLimits.screen())],
            after: "FORGE OPTIONS | focus 3 Map limits",
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
    // The options, then Placed objects, Build AI ground, Test waves and
    // Generate nav.
    assert_eq!(page.rows.len(), sheet::OPTIONS.len() + 4);
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
        ["Scenery", "Crates", "Machines", sheet::AI_PAGE],
        "after the map's groups"
    );
    assert_eq!(s.focus, 0, "the last chosen object keeps the focus");
    let Act::Pick(p) = &s.rows[3].act else {
        panic!("the page opens the picker")
    };
    assert!(p.closes, "choosing a label closes both");
    assert_eq!(p.values.len(), forge::GroundLabel::ALL.len());
    assert_eq!(p.values[0], "Wave drop");
    assert_eq!(p.details[0], "ff_drop. None placed");
    assert_eq!(p.choices[0], Choice::Marker(forge::GroundLabel::Drop));
    assert_eq!(p.choices[5], Choice::Marker(forge::GroundLabel::Line));
    // The page is the same one with the counts the session has.
    let s = sheet::objects_with_ai(&items, 1, &ai_items(3)).unwrap();
    let Act::Pick(p) = &s.rows[3].act else {
        panic!("the page opens the picker")
    };
    assert_eq!(p.details[1], "ff_base. 3 placed");
}

#[test]
fn the_page_is_alone_on_a_map_with_nothing_to_place_and_absent_with_no_labels() {
    let s = sheet::objects_with_ai(&[], 0, &ai_items(0)).unwrap();
    assert_eq!(s.rows.len(), 1);
    assert_eq!(s.rows[0].label, sheet::AI_PAGE);
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
fn the_forge_options_build_the_ground_and_test_waves() {
    let s = sheet::options();
    let rows: Vec<(&str, &Act)> = s.rows.iter().map(|r| (r.label.as_str(), &r.act)).collect();
    assert!(rows.contains(&("Build AI ground", &Act::Choose(Choice::BuildGround))));
    assert!(rows.contains(&("Test waves", &Act::Choose(Choice::TestWaves))));
    assert!(rows.contains(&("Generate nav", &Act::Choose(Choice::GenerateNav))));
    assert!(ForgeScreen::new(Kind::Options).show(s).is_ok());
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
    let idle = rows(&sheet::options_for(&[], &[], false));
    let testing = rows(&sheet::options_for(&[], &[], true));
    let at = idle
        .iter()
        .position(|(label, _)| label == "Test waves")
        .unwrap();
    assert_eq!(testing[at], ("Stop waves".into(), Act::Choose(Choice::StopWaves)));
    assert!(!testing.iter().any(|(label, _)| label == "Test waves"));
    assert_eq!(idle.len(), testing.len(), "one row in the other's place");
    assert_eq!(rows(&sheet::options()), idle);
    assert!(ForgeScreen::new(Kind::Options).show(sheet::options_for(&[], &[], true)).is_ok());
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
    let page = sheet::options_with(&rows);
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
    assert!(ForgeScreen::new(Kind::Options).show(page).is_ok());
    let none = sheet::options();
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
    let page = sheet::options_with_groups(&folders, &rows);
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
    assert!(ForgeScreen::new(Kind::Options).show(page).is_ok());
    // Groups alone are enough to make the row a browser.
    let only = sheet::options_with_groups(&folders, &[]);
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
