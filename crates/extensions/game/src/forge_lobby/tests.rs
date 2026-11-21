use super::*;
use crate::lobby_start::STATUS_CHARS;
use crate::ui_kit::input::Input::{self, *};
use crate::ui_kit::page::{ListEffect, ListPage, Page, described, named, run};
use crate::ui_kit::picker;

fn map(id: u32, name: &str) -> Map {
    Map {
        id,
        name: name.into(),
        sort: 0,
        path: format!(r"d:\maps\{}", name.to_lowercase()),
    }
}

/// What the lobby was asked to start: the map's list id and the draft.
type Started = Arc<Mutex<Vec<(u32, Option<String>)>>>;

struct World {
    lobby: Arc<Lobby>,
    started: Started,
}

fn world(maps: Vec<Map>, drafts: Result<Vec<String>, String>, start: Result<(), String>) -> World {
    let started = Started::default();
    let log = Arc::clone(&started);
    let lobby = Lobby::new(
        Arc::new(move || maps.clone()),
        Arc::new(move || drafts.clone()),
        Arc::new(move |map: &Map, draft: Option<&str>| {
            log.lock().unwrap().push((map.id, draft.map(str::to_owned)));
            start.clone()
        }),
    );
    World { lobby, started }
}

fn two_maps() -> Vec<Map> {
    vec![map(10, "Lockout"), map(11, "Ascension")]
}

fn fresh() -> World {
    world(two_maps(), Ok(vec!["alpha".into(), "beta".into()]), Ok(()))
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

/// The lobby as a table. Its named states are the first, middle and last
/// rows, so a row in between is reached by pressing down from one.
#[test]
fn the_lobby_opens_pickers_and_starts() {
    check(
        fresh,
        vec![
            // A on Map and Forge map opens the picker and changes nothing.
            case(
                "focus-0",
                &[A],
                vec![open()],
                "focus 0 Map = Lockout",
                "Lockout: a new map",
            ),
            case(
                "focus-1",
                &[A],
                vec![open()],
                "focus 1 Forge map = New",
                "Lockout: a new map",
            ),
            // Left and right do nothing on a pick row: no arrow rows.
            case(
                "focus-0",
                &[Left, Right],
                vec![ListEffect::None; 2],
                "focus 0 Map = Lockout",
                "Lockout: a new map",
            ),
            case(
                "focus-1",
                &[Right],
                vec![ListEffect::None],
                "focus 1 Forge map = New",
                "Lockout: a new map",
            ),
            // Start begins the game on the map held: the first.
            case(
                "focus-2",
                &[A],
                vec![stepped("forge lobby: starting Lockout with a new map")],
                "focus 2 Start",
                "Lockout: a new map",
            ),
            // The ends.
            case(
                "focus-2",
                &[Down],
                vec![ListEffect::None],
                "focus 2 Start",
                "Lockout: a new map",
            ),
            case(
                "focus-0",
                &[Up],
                vec![ListEffect::None],
                "focus 0 Map = Lockout",
                "Lockout: a new map",
            ),
            case(
                "focus-2",
                &[Up, Up, Up],
                vec![ListEffect::Focus(1), ListEffect::Focus(0), ListEffect::None],
                "focus 0 Map = Lockout",
                "Lockout: a new map",
            ),
            // B and Back close it.
            case(
                "focus-1",
                &[B],
                vec![ListEffect::Close],
                "focus 1 Forge map = New",
                "Lockout: a new map",
            ),
            case(
                "focus-0",
                &[Down, Back],
                vec![ListEffect::Focus(1), ListEffect::Close],
                "focus 1 Forge map = New",
                "Lockout: a new map",
            ),
        ],
    );
}

#[test]
fn the_map_picker_lists_the_original_maps_and_a_choice_is_started_on() {
    let w = fresh();
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-0");
    assert_eq!(run(&l, &mut state, [A]), [open()]);
    let pick = picker::current().expect("the picker holds the Map pick");
    assert_eq!(pick.title, "MAP");
    assert_eq!(pick.values, ["Lockout", "Ascension"]);
    assert_eq!(pick.current, 0);
    assert_eq!((pick.set)(1), "forge lobby: map Ascension");
    assert_eq!(l.summary(&state), "focus 0 Map = Ascension");
    assert_eq!(l.spec(&state).help.as_deref(), Some("Ascension: a new map"));
    let mut state = named(&l, "focus-2");
    assert_eq!(
        run(&l, &mut state, [A]),
        [stepped("forge lobby: starting Ascension with a new map")]
    );
    assert_eq!(*w.started.lock().unwrap(), [(11, None)]);
    // Opened again, it holds the one chosen.
    let mut state = named(&l, "focus-0");
    run(&l, &mut state, [A]);
    assert_eq!(picker::current().unwrap().current, 1);
}

#[test]
fn the_forge_map_picker_offers_new_and_each_saved_map() {
    let w = fresh();
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-1");
    run(&l, &mut state, [A]);
    let pick = picker::current().unwrap();
    assert_eq!(pick.title, "FORGE MAP");
    assert_eq!(pick.values, ["New", "alpha", "beta"]);
    assert_eq!((pick.set)(2), "forge lobby: saved map beta");
    assert_eq!(l.summary(&state), "focus 1 Forge map = beta");
    assert_eq!(
        l.spec(&state).help.as_deref(),
        Some("Lockout: saved map beta")
    );
    let mut start = named(&l, "focus-2");
    run(&l, &mut start, [A]);
    assert_eq!(*w.started.lock().unwrap(), [(10, Some("beta".into()))]);
    // Opened again it holds beta; New puts the draft back.
    run(&l, &mut state, [A]);
    let pick = picker::current().unwrap();
    assert_eq!(pick.current, 2);
    assert_eq!((pick.set)(0), "forge lobby: a new map");
    assert_eq!(l.summary(&state), "focus 1 Forge map = New");
}

#[test]
fn with_no_original_map_the_picker_says_so_and_start_is_refused() {
    let w = world(vec![], Ok(vec![]), Ok(()));
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-0");
    assert_eq!(
        l.summary(&state),
        "focus 0 Map = No original map is installed"
    );
    assert_eq!(run(&l, &mut state, [A]), [open()]);
    let pick = picker::current().unwrap();
    assert_eq!(pick.values, ["No original map is installed"]);
    assert_eq!((pick.set)(0), "nothing to pick");
    let mut state = named(&l, "focus-2");
    assert_eq!(
        run(&l, &mut state, [A]),
        [stepped("forge lobby: No original map is installed")]
    );
    assert!(w.started.lock().unwrap().is_empty(), "nothing was started");
    assert_eq!(
        l.spec(&state).help.as_deref(),
        Some("No original map is installed. Forge needs one.")
    );
}

#[test]
fn a_start_the_game_cannot_make_says_why_on_the_status_line() {
    let w = world(
        two_maps(),
        Ok(vec![]),
        Err("The game's own functions are not reachable.".into()),
    );
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-2");
    assert_eq!(
        run(&l, &mut state, [A]),
        [stepped(
            "forge lobby: not started: The game's own functions are not reachable."
        )]
    );
    let help = l.spec(&state).help.unwrap();
    assert!(help.starts_with("The game's own functions"), "{help}");
    assert!(help.chars().count() <= STATUS_CHARS, "{help}");
    // Choosing again clears it.
    let mut map = named(&l, "focus-0");
    run(&l, &mut map, [A]);
    (picker::current().unwrap().set)(1);
    assert_eq!(l.spec(&state).help.as_deref(), Some("Ascension: a new map"));
}

#[test]
fn saved_maps_that_cannot_be_listed_say_why_and_new_still_works() {
    let w = world(two_maps(), Err("the folder is unreadable".into()), Ok(()));
    let s = Arc::new(w.lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-1");
    assert_eq!(run(&l, &mut state, [A]), [open()]);
    let pick = picker::current().unwrap();
    assert_eq!(pick.values, ["Saved maps cannot be listed"]);
    assert!(pick.description.contains("the folder is unreadable"));
    assert_eq!(l.summary(&state), "focus 1 Forge map = New");
    let mut start = named(&l, "focus-2");
    run(&l, &mut start, [A]);
    assert_eq!(*w.started.lock().unwrap(), [(10, None)]);
}

#[test]
fn a_map_that_goes_away_leaves_the_first_map_held_and_shown_on_its_row() {
    let installed = Arc::new(Mutex::new(two_maps()));
    let from = Arc::clone(&installed);
    let lobby = Lobby::new(
        Arc::new(move || from.lock().unwrap().clone()),
        Arc::new(|| Ok(vec![])),
        Arc::new(|_: &Map, _: Option<&str>| Ok(())),
    );
    let s = Arc::new(lobby.page());
    let l = list(&s);
    let mut state = named(&l, "focus-0");
    run(&l, &mut state, [A]);
    (picker::current().unwrap().set)(1);
    assert_eq!(l.summary(&state), "focus 0 Map = Ascension");
    // The chosen map goes: the first of those left is what Start would use,
    // and the row says which.
    installed.lock().unwrap().retain(|m| m.id != 11);
    assert_eq!(l.summary(&state), "focus 0 Map = Lockout");
}

#[test]
fn every_text_of_the_lobby_fits_its_place_and_reads_plainly() {
    let w = fresh();
    let s = Arc::new(w.lobby.page());
    let spec = s.spec();
    assert_eq!(spec.title, "FORGE");
    let labels: Vec<_> = spec.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, ["Map", "Forge map", "Start"]);
    let mut texts = vec![spec.title.clone(), NO_MAP.to_owned(), NEW_MAP.to_owned()];
    texts.extend(spec.rows.iter().filter_map(|r| r.description.clone()));
    for t in &texts {
        assert!(!t.contains("  "), "two spaces in {t:?}");
        assert_eq!(t.trim(), t, "stray space around {t:?}");
        assert!(
            !t.contains('<') && !t.contains('>'),
            "an arrow row in {t:?}"
        );
    }
    for line in [
        format!("{NO_MAP}. Forge needs one."),
        "Lockout: a new map".to_owned(),
    ] {
        assert!(line.chars().count() <= STATUS_CHARS, "{line}");
    }
}
