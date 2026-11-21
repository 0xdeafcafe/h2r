use super::*;
use crate::lobby_start::STATUS_CHARS;
use crate::ui_kit::input::Input::{self, *};
use crate::ui_kit::page::{ListEffect, ListPage, Page, described, named, run};
use crate::ui_kit::picker;

fn kit(id: u32, base: &str) -> Map {
    Map {
        id,
        name: format!("{KIT_NAME_PREFIX}{base}"),
        sort: 0,
        path: format!(r"t:\$C\{id:08x}\{}", base.to_lowercase()),
    }
}

/// What the lobby was asked to start: the maps' list ids.
type Started = Arc<Mutex<Vec<u32>>>;

struct World {
    lobby: Arc<Lobby>,
    started: Started,
}

fn world(
    maps: Vec<Map>,
    read: Result<options::Options, String>,
    start: Result<(), String>,
) -> World {
    let started = Started::default();
    let log = Arc::clone(&started);
    let lobby = Lobby::new(
        Arc::new(move || maps.clone()),
        Arc::new(move || read.clone()),
        Arc::new(move |map: &Map| {
            log.lock().unwrap().push(map.id);
            start.clone()
        }),
    );
    World { lobby, started }
}

fn two_kits() -> Vec<Map> {
    vec![kit(20, "Coagulation"), kit(21, "Lockout")]
}

fn fresh() -> World {
    world(two_kits(), Ok(options::Options::default()), Ok(()))
}

fn list(s: &Arc<KitScreen>) -> ListPage {
    ListPage::new(s.class(), described(s))
}

fn stepped(line: &str) -> ListEffect {
    ListEffect::Stepped(line.into())
}

fn open() -> ListEffect {
    ListEffect::Open(picker::NAME)
}

/// One row of the table: the state it starts from, the presses, the
/// effects, what the focused row shows after and the status line.
struct Case {
    from: &'static str,
    presses: Vec<Input>,
    effects: Vec<ListEffect>,
    after: &'static str,
    status: &'static str,
}

fn case(
    from: &'static str,
    presses: &[Input],
    effects: Vec<ListEffect>,
    after: &'static str,
    status: &'static str,
) -> Case {
    Case {
        from,
        presses: presses.to_vec(),
        effects,
        after,
        status,
    }
}

fn check(make: impl Fn() -> World, cases: Vec<Case>) {
    for (i, c) in cases.into_iter().enumerate() {
        let w = make();
        let s = Arc::new(w.lobby.page());
        let l = list(&s);
        let mut state = named(&l, c.from);
        let what = format!("case {i}: from {} on {:?}", c.from, c.presses);
        assert_eq!(run(&l, &mut state, c.presses), c.effects, "{what}: effects");
        assert_eq!(l.summary(&state), c.after, "{what}: shows");
        assert_eq!(
            l.spec(&state).help.as_deref(),
            Some(c.status),
            "{what}: status"
        );
    }
}

const READY: &str = "Start: Firefight on Coagulation";

/// The lobby as a table. Its named states are the first, middle and last
/// rows.
#[test]
fn the_lobby_opens_the_picker_and_the_options_and_starts() {
    check(
        fresh,
        vec![
            // A on Map opens the picker; on Options... the options page.
            case(
                "focus-0",
                &[A],
                vec![open()],
                "focus 0 Map = Coagulation",
                READY,
            ),
            case(
                "focus-1",
                &[A],
                vec![ListEffect::Open(options::SCREEN)],
                "focus 1 Options...",
                READY,
            ),
            // Left and right do nothing: no arrow rows.
            case(
                "focus-0",
                &[Left, Right],
                vec![ListEffect::None; 2],
                "focus 0 Map = Coagulation",
                READY,
            ),
            case(
                "focus-1",
                &[Right],
                vec![ListEffect::None],
                "focus 1 Options...",
                READY,
            ),
            // Start begins the game on the map held: the first.
            case(
                "focus-2",
                &[A],
                vec![stepped("firefight lobby: starting on Coagulation")],
                "focus 2 Start",
                READY,
            ),
            // The ends.
            case(
                "focus-2",
                &[Down],
                vec![ListEffect::None],
                "focus 2 Start",
                READY,
            ),
            case(
                "focus-0",
                &[Up],
                vec![ListEffect::None],
                "focus 0 Map = Coagulation",
                READY,
            ),
            case(
                "focus-2",
                &[Up, Up, Up],
                vec![ListEffect::Focus(1), ListEffect::Focus(0), ListEffect::None],
                "focus 0 Map = Coagulation",
                READY,
            ),
            // B and Back close it.
            case(
                "focus-1",
                &[B],
                vec![ListEffect::Close],
                "focus 1 Options...",
                READY,
            ),
            case(
                "focus-0",
                &[Down, Down, Back],
                vec![
                    ListEffect::Focus(1),
                    ListEffect::Focus(2),
                    ListEffect::Close,
                ],
                "focus 2 Start",
                READY,
            ),
        ],
    );
}

#[test]
fn the_picker_lists_the_kit_maps_by_their_base_and_a_choice_is_started_on() {
    let w = fresh();
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-0");
    assert_eq!(run(&l, &mut state, [A]), [open()]);
    let pick = picker::current().expect("the picker holds the Map pick");
    assert_eq!(pick.title, "MAP");
    assert_eq!(pick.values, ["Coagulation", "Lockout"]);
    assert_eq!((pick.set)(1), "firefight lobby: map Lockout");
    assert_eq!(l.summary(&state), "focus 0 Map = Lockout");
    assert_eq!(
        l.spec(&state).help.as_deref(),
        Some("Start: Firefight on Lockout")
    );
    let mut go = named(&l, "focus-2");
    assert_eq!(
        run(&l, &mut go, [A]),
        [stepped("firefight lobby: starting on Lockout")]
    );
    assert_eq!(*w.started.lock().unwrap(), [21]);
    run(&l, &mut state, [A]);
    assert_eq!(
        picker::current().unwrap().current,
        1,
        "it holds the one chosen"
    );
}

#[test]
fn with_no_kit_map_the_picker_says_so_and_start_is_refused_with_the_reason() {
    let w = world(vec![], Ok(options::Options::default()), Ok(()));
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-0");
    assert_eq!(
        l.summary(&state),
        "focus 0 Map = No installed map carries the Firefight kit"
    );
    assert_eq!(run(&l, &mut state, [A]), [open()]);
    let pick = picker::current().unwrap();
    assert_eq!(pick.values, [NO_MAP]);
    assert!(
        pick.description.contains("h2 firefight kit MAP"),
        "{}",
        pick.description
    );
    assert_eq!((pick.set)(0), "nothing to pick");
    let mut go = named(&l, "focus-2");
    assert_eq!(
        run(&l, &mut go, [A]),
        [stepped(
            "firefight lobby: not started: No installed map carries the Firefight kit."
        )]
    );
    assert!(w.started.lock().unwrap().is_empty());
    assert_eq!(
        l.spec(&go).help.as_deref(),
        Some("No installed map carries the Firefight kit.")
    );
}

#[test]
fn start_is_refused_while_firefight_is_off_or_the_options_cannot_be_read() {
    let off = options::Options {
        enabled: false,
        ..options::Options::default()
    };
    let w = world(two_kits(), Ok(off), Ok(()));
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut go = named(&l, "focus-2");
    assert_eq!(
        l.spec(&go).help.as_deref(),
        Some(OFF),
        "said before it is pressed"
    );
    assert_eq!(
        run(&l, &mut go, [A]),
        [stepped(&format!("firefight lobby: not started: {OFF}"))]
    );
    assert!(w.started.lock().unwrap().is_empty());

    let w = world(
        two_kits(),
        Err("firefight.waveset names no built-in wave set".into()),
        Ok(()),
    );
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut go = named(&l, "focus-2");
    let effects = run(&l, &mut go, [A]);
    let ListEffect::Stepped(line) = &effects[0] else {
        panic!("a refusal is a stepped line: {effects:?}");
    };
    assert!(
        line.contains("firefight.waveset names no built-in wave set"),
        "{line}"
    );
    assert!(w.started.lock().unwrap().is_empty());
    let help = l.spec(&go).help.unwrap();
    assert!(help.starts_with("Options: firefight.waveset"), "{help}");
    assert!(help.chars().count() <= STATUS_CHARS, "{help}");
}

#[test]
fn a_start_the_game_cannot_make_says_why_and_a_new_choice_clears_it() {
    let w = world(
        two_kits(),
        Ok(options::Options::default()),
        Err("The game did not queue the match.".into()),
    );
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut go = named(&l, "focus-2");
    assert_eq!(
        run(&l, &mut go, [A]),
        [stepped(
            "firefight lobby: not started: The game did not queue the match."
        )]
    );
    assert_eq!(
        l.spec(&go).help.as_deref(),
        Some("The game did not queue the match.")
    );
    let mut map = named(&l, "focus-0");
    run(&l, &mut map, [A]);
    (picker::current().unwrap().set)(1);
    assert_eq!(
        l.spec(&go).help.as_deref(),
        Some("Start: Firefight on Lockout")
    );
}

#[test]
fn every_text_of_the_lobby_fits_its_place_and_reads_plainly() {
    let w = fresh();
    let s = Arc::new(w.lobby.page());
    let spec = s.spec();
    assert_eq!(spec.title, "FIREFIGHT");
    let labels: Vec<_> = spec.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Map", "Options...", "Start"]);
    let mut texts = vec![
        spec.title.clone(),
        NO_MAP.to_owned(),
        HOW_TO.to_owned(),
        OFF.to_owned(),
    ];
    texts.extend(spec.rows.iter().filter_map(|r| r.description.clone()));
    for t in &texts {
        assert!(!t.contains("  "), "two spaces in {t:?}");
        assert_eq!(t.trim(), t, "stray space around {t:?}");
        assert!(
            !t.contains('<') && !t.contains('>'),
            "an arrow row in {t:?}"
        );
    }
    for line in [format!("{NO_MAP}."), OFF.to_owned(), READY.to_owned()] {
        assert!(line.chars().count() <= STATUS_CHARS, "{line}");
    }
}

#[test]
fn a_kit_map_is_named_by_its_base() {
    assert_eq!(title(&kit(1, "Coagulation")), "Coagulation");
    let bare = Map {
        id: 2,
        name: "Midship".into(),
        sort: 0,
        path: String::new(),
    };
    assert_eq!(title(&bare), "Midship");
}
