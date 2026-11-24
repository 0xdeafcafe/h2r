use super::*;
use crate::ui_kit::view::Opener;
use crate::ui::Chosen;
use crate::ui_kit::input::Input::{self, *};
use crate::ui_kit::page::{ListEffect, ListPage, Page, described, named, run};
use crate::ui_kit::picker;

fn store() -> Arc<Store> {
    Arc::new(Store::defaults(&DEFS))
}

/// The page's doc: `content/ui/screens/firefight.yaml`.
fn doc() -> Arc<crate::ui_kit::data::ScreenDoc> {
    crate::ui_kit::data::require(SCREEN).unwrap_or_else(|e| panic!("{e}"))
}

/// The Game Setup row's text and help (`settings.caller`).
fn caller() -> crate::ui_kit::data::CallerDoc {
    doc().settings.clone().and_then(|s| s.caller).expect("a caller")
}

fn screen(kept: &Arc<Store>) -> Arc<KitScreen> {
    Arc::new(page(Arc::clone(kept)))
}

fn list(s: &Arc<KitScreen>) -> ListPage {
    ListPage::new(s.class(), described(s)).with_states(&[0, 2, 3, 5])
}

fn stepped(line: &str) -> ListEffect {
    ListEffect::Stepped(line.into())
}

fn open() -> ListEffect {
    ListEffect::Open(picker::NAME)
}

/// One row of the page's table: the state it starts from, the presses, the
/// effects they give, what the focused row shows after and what the store
/// holds.
struct Case {
    from: &'static str,
    presses: Vec<Input>,
    effects: Vec<ListEffect>,
    after: &'static str,
    held: Vec<(&'static Setting, Value)>,
}

fn case(
    from: &'static str,
    presses: &[Input],
    effects: Vec<ListEffect>,
    after: &'static str,
    held: Vec<(&'static Setting, Value)>,
) -> Case {
    Case {
        from,
        presses: presses.to_vec(),
        effects,
        after,
        held,
    }
}

/// Each case on a fresh page and store: the rows keep their own state in
/// the store, which one case must not hand the next.
fn check(cases: Vec<Case>) {
    for (i, c) in cases.into_iter().enumerate() {
        let kept = store();
        let s = screen(&kept);
        let l = list(&s);
        let mut state = named(&l, c.from);
        let what = format!("case {i}: from {} on {:?}", c.from, c.presses);
        assert_eq!(run(&l, &mut state, c.presses), c.effects, "{what}: effects");
        assert_eq!(l.summary(&state), c.after, "{what}: shows");
        for (def, value) in c.held {
            assert_eq!(kept.get(def.key), Some(value), "{what}: {}", def.key);
        }
    }
}

/// The options page as a table: A opens the picker on every row, left and
/// right step and wrap, up and down stop at the ends, B and Back close.
#[test]
fn the_options_page_steps_and_opens_pickers() {
    check(vec![
        // A opens the value picker, and changes nothing.
        case(
            "focus-0",
            &[A],
            vec![open()],
            "focus 0 Firefight = On",
            vec![(&ENABLED, Value::Bool(true))],
        ),
        case(
            "focus-3",
            &[A],
            vec![open()],
            "focus 3 Difficulty = Normal",
            vec![(&DIFFICULTY, Value::Int(1))],
        ),
        // Right and left step through the choices, saved at once.
        case(
            "focus-0",
            &[Right],
            vec![stepped("firefight: right on row 0: firefight.enabled -> Off")],
            "focus 0 Firefight = Off",
            vec![(&ENABLED, Value::Bool(false))],
        ),
        case(
            "focus-0",
            &[Down, Right],
            vec![
                ListEffect::Focus(1),
                stepped("firefight: right on row 1: firefight.waveset -> Attrition"),
            ],
            "focus 1 Wave set = Attrition",
            vec![(&WAVE_SET, Value::Int(1))],
        ),
        case(
            "focus-0",
            &[Down, Left],
            vec![
                ListEffect::Focus(1),
                stepped("firefight: left on row 1: firefight.waveset -> Crossfire"),
            ],
            "focus 1 Wave set = Crossfire",
            vec![(&WAVE_SET, Value::Int(3))],
        ),
        // The teams start at the wave set's own and wrap both ways.
        case(
            "focus-2",
            &[Right],
            vec![stepped("firefight: right on row 2: firefight.teams -> One team")],
            "focus 2 Teams = One team",
            vec![(&TEAMS, Value::Int(1))],
        ),
        case(
            "focus-2",
            &[Left],
            vec![stepped("firefight: left on row 2: firefight.teams -> Two teams")],
            "focus 2 Teams = Two teams",
            vec![(&TEAMS, Value::Int(2))],
        ),
        case(
            "focus-3",
            &[Left],
            vec![stepped("firefight: left on row 3: firefight.difficulty -> Easy")],
            "focus 3 Difficulty = Easy",
            vec![(&DIFFICULTY, Value::Int(0))],
        ),
        // Right three times from Normal goes Heroic, Legendary, Easy.
        case(
            "focus-3",
            &[Right, Right, Right],
            vec![
                stepped("firefight: right on row 3: firefight.difficulty -> Heroic"),
                stepped("firefight: right on row 3: firefight.difficulty -> Legendary"),
                stepped("firefight: right on row 3: firefight.difficulty -> Easy"),
            ],
            "focus 3 Difficulty = Easy",
            vec![(&DIFFICULTY, Value::Int(0))],
        ),
        // The lives start at the wave set's own and wrap both ways.
        case(
            "focus-3",
            &[Down, Right],
            vec![ListEffect::Focus(4), stepped("firefight: right on row 4: firefight.lives -> 1")],
            "focus 4 Starting lives = 1",
            vec![(&LIVES, Value::Int(1))],
        ),
        case(
            "focus-3",
            &[Down, Left],
            vec![ListEffect::Focus(4), stepped("firefight: left on row 4: firefight.lives -> 30")],
            "focus 4 Starting lives = 30",
            vec![(&LIVES, Value::Int(30))],
        ),
        // The ends: down at the last row and up at the first stay put.
        case(
            "focus-5",
            &[Down],
            vec![ListEffect::None],
            "focus 5 Skulls = On",
            vec![],
        ),
        case(
            "focus-0",
            &[Up],
            vec![ListEffect::None],
            "focus 0 Firefight = On",
            vec![],
        ),
        case(
            "focus-5",
            &[Up, Up, Up, Up, Up, Up],
            vec![
                ListEffect::Focus(4),
                ListEffect::Focus(3),
                ListEffect::Focus(2),
                ListEffect::Focus(1),
                ListEffect::Focus(0),
                ListEffect::None,
            ],
            "focus 0 Firefight = On",
            vec![],
        ),
        case(
            "focus-5",
            &[Right],
            vec![stepped("firefight: right on row 5: firefight.skulls -> Off")],
            "focus 5 Skulls = Off",
            vec![(&SKULLS, Value::Bool(false))],
        ),
        // B and Back close the page.
        case(
            "focus-0",
            &[B],
            vec![ListEffect::Close],
            "focus 0 Firefight = On",
            vec![],
        ),
        case(
            "focus-3",
            &[Down, Back],
            vec![ListEffect::Focus(4), ListEffect::Close],
            "focus 4 Starting lives = Default",
            vec![],
        ),
    ]);
}

/// A on any row opens the value picker for it, over Game Setup, with the
/// row's own choices; no row is a "< All >" arrow row.
#[test]
fn a_opens_the_value_picker_on_every_row() {
    use crate::list_class::Rows;
    let s = screen(&store());
    let expected: [(&str, &[&str]); 6] = [
        ("FIREFIGHT", &["Off", "On"]),
        ("WAVE SET", &["Holdout", "Attrition", "Last Stand", "Crossfire"]),
        ("TEAMS", &["Default", "One team", "Two teams"]),
        ("DIFFICULTY", &["Easy", "Normal", "Heroic", "Legendary"]),
        (
            "STARTING LIVES",
            &["Default", "1", "3", "5", "8", "12", "20", "30"],
        ),
        ("SKULLS", &["Off", "On"]),
    ];
    for (row, (title, values)) in expected.iter().enumerate() {
        assert_eq!(s.press(row as i16), Chosen::Open(picker::NAME), "{title}");
        let pick = picker::current().expect("a pick");
        assert_eq!(pick.title, *title);
        assert_eq!(pick.values, *values);
        assert_eq!(pick.opener, Opener::GAME_SETUP_FIREFIGHT, "{title}");
        assert!(
            pick.values
                .iter()
                .all(|v| !v.contains('<') && !v.contains('>')),
            "{title}"
        );
    }
}

#[test]
fn the_settings_are_kept_with_every_other_setting() {
    for d in DEFS {
        assert!(
            crate::setting_defs::ALL.iter().any(|a| a.key == d.key),
            "{} is not in setting_defs::ALL",
            d.key
        );
        assert!(d.key.starts_with("firefight."), "{}", d.key);
    }
    // The Wave set row is bound instead: its offered values are the lobby
    // map's menu, not the setting's fixed choices.
    let (settings, binds): (Vec<Option<String>>, Vec<Option<String>>) = doc()
        .rows
        .values()
        .map(|r| (r.setting.clone(), r.bind.clone()))
        .unzip();
    assert_eq!(
        binds,
        [
            None,
            Some("waveset".to_owned()),
            None,
            None,
            None,
            None
        ]
    );
    let keys: Vec<String> = settings.into_iter().flatten().collect();
    let defs: Vec<&str> = DEFS
        .iter()
        .map(|d| d.key)
        .filter(|key| *key != WAVE_SET.key)
        .collect();
    assert_eq!(keys, defs, "a row per setting, in the page's order");
}

#[test]
fn the_defaults_are_the_documented_match() {
    assert_eq!(Options::read(&store()), Ok(Options::default()));
    let d = Options::default();
    assert_eq!(
        (d.enabled, d.wave_set, d.teams, d.difficulty, d.lives, d.skulls),
        (true, 0, None, Difficulty::Normal, None, true)
    );
    assert_eq!(d.definition().unwrap(), builtin::definitions()[0]);
    assert_eq!(d.player_teams(), 1);
}

/// The Teams row plays a wave set by one team or two; Default keeps the
/// set's own, so Crossfire is two teams and the others one.
#[test]
fn the_teams_choice_reaches_the_definition() {
    let kept = store();
    let read = |kept: &Arc<Store>| Options::read(kept).unwrap();
    let d = read(&kept).definition().unwrap();
    assert_eq!((d.teams, d.scoring.player_kill), (1, 0));
    kept.set(TEAMS.key, Value::Int(2)).unwrap();
    let o = read(&kept);
    assert_eq!((o.teams, o.player_teams()), (Some(2), 2));
    let d = o.definition().unwrap();
    assert_eq!(d.name, "Holdout");
    assert_eq!(
        (d.teams, d.scoring.player_kill),
        (2, builtin::PLAYER_KILL_POINTS)
    );
    kept.set(WAVE_SET.key, Value::Int(3)).unwrap();
    kept.set(TEAMS.key, Value::Int(DEFAULT_TEAMS)).unwrap();
    let o = read(&kept);
    assert_eq!((o.teams, o.player_teams()), (None, 2));
    assert_eq!(o.definition().unwrap().name, "Crossfire");
    kept.set(TEAMS.key, Value::Int(1)).unwrap();
    let d = read(&kept).definition().unwrap();
    assert_eq!((d.name.as_str(), d.teams, d.scoring.player_kill), ("Crossfire", 1, 0));
    // The store refuses a count the setting does not offer.
    let e = format!("{:?}", kept.set(TEAMS.key, Value::Int(3)).unwrap_err());
    assert!(e.contains("firefight.teams"), "{e}");
}

#[test]
fn the_choices_reach_the_definition_and_the_difficulty() {
    let kept = store();
    kept.set(WAVE_SET.key, Value::Int(2)).unwrap();
    kept.set(LIVES.key, Value::Int(3)).unwrap();
    kept.set(SKULLS.key, Value::Bool(false)).unwrap();
    kept.set(DIFFICULTY.key, Value::Int(3)).unwrap();
    let o = Options::read(&kept).unwrap();
    assert_eq!(
        o,
        Options {
            enabled: true,
            wave_set: 2,
            teams: None,
            difficulty: Difficulty::Legendary,
            lives: Some(3),
            skulls: false,
        }
    );
    let def = o.definition().unwrap();
    assert_eq!(def.name, "Last Stand");
    assert_eq!(def.lives.pool_start, 3);
    assert!(
        def.rounds.iter().all(|r| r.skulls_on.is_empty()),
        "no round switches a skull on"
    );
    // The rest is the wave set's own.
    let own = &builtin::definitions()[2];
    assert_eq!(def.rounds.len(), own.rounds.len());
    assert_eq!(def.caps, own.caps);
    assert_eq!(def.lives.per_round_bonus, own.lives.per_round_bonus);
    // Every difficulty is read as itself.
    for (v, want) in [
        (0, Difficulty::Easy),
        (1, Difficulty::Normal),
        (2, Difficulty::Heroic),
        (3, Difficulty::Legendary),
    ] {
        kept.set(DIFFICULTY.key, Value::Int(v)).unwrap();
        assert_eq!(Options::read(&kept).unwrap().difficulty, want);
    }
}

#[test]
fn every_wave_set_and_lives_choice_makes_a_valid_definition() {
    let limits = ::firefight::Limits::new(["elite".to_string()]);
    for wave_set in 0..builtin::definitions().len() {
        for teams in [None, Some(1), Some(2)] {
            for lives in lives_choices() {
                for skulls in [true, false] {
                    let o = Options {
                        wave_set,
                        teams,
                        lives,
                        skulls,
                        ..Options::default()
                    };
                    let def = o.definition().unwrap();
                    ::firefight::validate(&def, &limits)
                        .unwrap_or_else(|e| panic!("{o:?}: {e}"));
                }
            }
        }
    }
    // The map-specific set, on its own map.
    for teams in [None, Some(1), Some(2)] {
        for lives in lives_choices() {
            for skulls in [true, false] {
                let o = Options {
                    map: Some("beavercreek".into()),
                    wave_set: usize::try_from(builtin::MAP_SET_BASE).unwrap(),
                    teams,
                    lives,
                    skulls,
                    ..Options::default()
                };
                let def = o.definition().unwrap();
                ::firefight::validate(&def, &limits)
                    .unwrap_or_else(|e| panic!("{o:?}: {e}"));
            }
        }
    }
}

/// The lobby's kit map puts its own set first in the Wave set row: the
/// picker offers Basin, then the built-ins; picking Basin keeps its own
/// value, and right from it steps to Holdout.
#[test]
fn the_wave_set_row_offers_the_maps_set_first_on_its_map() {
    use crate::lobby_start;
    use crate::list_class::Rows;
    lobby_start::set_kit_map(Some("beavercreek".into()));
    let kept = store();
    let s = screen(&kept);
    assert_eq!(s.press(1), Chosen::Open(picker::NAME), "Wave set");
    let pick = picker::current().expect("a pick");
    assert_eq!(
        pick.values,
        ["Basin", "Holdout", "Attrition", "Last Stand", "Crossfire"]
    );
    assert_eq!(pick.opener, Opener::GAME_SETUP_FIREFIGHT);
    assert_eq!(
        (pick.set)(0),
        "firefight.waveset -> Basin",
        "the picker keeps the map set's own value"
    );
    assert_eq!(kept.get(WAVE_SET.key), Some(Value::Int(builtin::MAP_SET_BASE)));
    // Right from the map's own set steps to Holdout.
    let l = list(&s);
    let mut state = named(&l, "focus-0");
    assert_eq!(
        run(&l, &mut state, &[Down, Right]),
        vec![
            ListEffect::Focus(1),
            stepped("firefight: right on row 1: firefight.waveset -> Holdout")
        ]
    );
    assert_eq!(kept.get(WAVE_SET.key), Some(Value::Int(0)));
    lobby_start::set_kit_map(None);
}

/// A map-specific set plays on its own map only (`builtin::resolve`);
/// elsewhere the built-ins play, the row says what does, and a value no
/// set names defines nothing.
#[test]
fn a_map_set_value_plays_on_its_map_and_falls_back() {
    use crate::lobby_start;
    let kept = store();
    kept.set(WAVE_SET.key, Value::Int(builtin::MAP_SET_BASE)).unwrap();
    // No map known: the built-ins play.
    let o = Options::read(&kept).unwrap();
    assert_eq!(o.definition().unwrap(), builtin::definitions()[0]);
    assert_eq!(o.wave_set_label(), "Holdout");
    // On Beaver Creek: Basin, one team of its own.
    let o = Options::for_map(&kept, Some("beavercreek")).unwrap();
    assert_eq!(o.definition().unwrap(), builtin::map_sets("beavercreek")[0]);
    assert_eq!(o.wave_set_label(), "Basin");
    assert_eq!(o.player_teams(), 1);
    // Another map: the built-ins again, and the row says so.
    let o = Options::for_map(&kept, Some("coagulation")).unwrap();
    assert_eq!(o.definition().unwrap(), builtin::definitions()[0]);
    assert_eq!(o.wave_set_label(), "Holdout");
    // A value no set names defines nothing.
    let refused = Options {
        wave_set: 99,
        ..Options::default()
    };
    assert!(refused.definition().is_err());
    lobby_start::set_kit_map(None);
}

/// The starting pools the page offers: none (the wave set's own) first.
fn lives_choices() -> Vec<Option<u16>> {
    let Kind::Choice(choices) = LIVES.kind else {
        panic!("lives is a choice");
    };
    choices
        .iter()
        .map(|c| (c.value != DEFAULT_LIVES).then(|| u16::try_from(c.value).unwrap()))
        .collect()
}

#[test]
fn a_store_that_lacks_a_setting_is_an_error_naming_it() {
    let empty = Store::defaults(&[]);
    let e = Options::read(&empty).unwrap_err();
    assert!(e.contains("firefight.enabled"), "{e}");
    let without_skulls = Store::defaults(&[&ENABLED, &WAVE_SET, &TEAMS, &DIFFICULTY, &LIVES]);
    let e = Options::read(&without_skulls).unwrap_err();
    assert!(e.contains("firefight.skulls"), "{e}");
}

#[test]
fn the_page_has_a_row_per_setting_and_a_row_text_of_its_own() {
    use crate::list_class::Rows;
    let s = screen(&store());
    assert_eq!(s.count(), DEFS.len());
    assert!(s.slots() <= IDS.len, "the family holds every text");
    // The Game Setup row says "Firefight Options..."; the page is
    // "FIREFIGHT".
    let row = s.row_label_id();
    assert_ne!(row, s.title());
    assert!(IDS.owns(row) && IDS.owns(s.title()));
    let help = s.row_help_id().expect("a description");
    assert!(IDS.owns(help));
    assert_ne!(help, row);
    assert_eq!(caller().label, "Firefight Options...");
    assert!(caller().help.len() < 120, "a few lines under Game Setup's list");
}

/// The copy is spaced and punctuated as the other pages' is.
#[test]
fn the_copy_is_spaced() {
    let doc = doc();
    let title = doc.text("title", &[]).unwrap();
    let (caller, settings) = (caller(), doc.settings.clone().unwrap());
    let mut all: Vec<&str> = vec![&caller.label, &caller.help, &title, settings.about()];
    all.extend(settings.name.as_deref());
    for r in doc.rows.values() {
        all.extend([r.text.as_str(), r.help.as_deref().unwrap(), r.explain.as_deref().unwrap()]);
    }
    for d in DEFS {
        all.push(d.help);
    }
    for t in all {
        assert_eq!(t, t.trim(), "{t:?}");
        for bad in ["  ", " ,", " .", " :", " ;", "\n"] {
            assert!(!t.contains(bad), "{t:?} has {bad:?}");
        }
        let chars: Vec<char> = t.chars().collect();
        for w in chars.windows(2) {
            assert!(
                !(matches!(w[0], ',' | ':' | ';') && w[1] != ' '),
                "{t:?}: a space after {:?}",
                w[0]
            );
        }
        assert!(!t.is_empty());
    }
    assert!(caller.label.ends_with("..."));
    assert_eq!(title, title.to_uppercase());
}

#[test]
fn only_a_custom_games_list_gets_the_row_once() {
    assert!(wants_row(&[0, 1, 2]));
    assert!(wants_row(&[0, 1, 10, 11]), "beside the other rows of ours");
    assert!(!wants_row(&[1, 2]), "no Change Map: not a custom game");
    assert!(!wants_row(&[0, ROW_ACTION]), "already there");
}

/// Our action is past retail's ten and the other rows of ours.
#[test]
fn the_rows_action_is_its_own() {
    assert!(ROW_ACTION > crate::clankers::ROW_ACTION);
    assert!(ROW_ACTION > crate::clankermatch::SWITCH_ACTION);
    assert!(ROW_ACTION > crate::h1_options_row::OPTIONS_ACTION);
}

/// The row's hooks come after the Halo 1 row's, the Clankers row's and
/// Clankermatch's on the plan, so the plan builds with all of them.
#[test]
fn the_row_goes_after_the_other_rows_on_the_plan() {
    let mut r = hooks::Registry::new(hooks::Switches::defaults());
    r.add(&h1row::SetupRow {
        data_root: std::env::temp_dir(),
        page: crate::h1_options_row::test_page(),
    })
    .unwrap();
    let clanker_store = Arc::new(Store::defaults(&crate::clankers::DEFS));
    let clankers = Arc::new(crate::clankers::page(clanker_store));
    r.add(&KitTexts(Arc::clone(&clankers))).unwrap();
    r.add(&clankers.class()).unwrap();
    r.add(&crate::clankers::SetupRow { page: clankers })
        .unwrap();
    let ours = screen(&store());
    r.add(&KitTexts(Arc::clone(&ours))).unwrap();
    r.add(&ours.class()).unwrap();
    r.add(&SetupRow { page: ours }).unwrap();
    let text = r.build(&hooks::Base).unwrap().text();
    for point in ["SQUAD_SETTINGS_ROW_TEXT", "SQUAD_SETTINGS_ROW_CHOSEN"] {
        let line = text.lines().find(|l| l.contains(point)).unwrap();
        let (h1, clankers, firefight) = (
            line.find("h1-options-row:").unwrap(),
            line.find("clankers-row:").unwrap(),
            line.find("firefight-row:").unwrap(),
        );
        assert!(h1 < clankers && clankers < firefight, "{line}");
    }
}

/// Every row has a help that fits the block under the rows and an
/// explanation for the value picker that says more than the help and is
/// player copy. The header stays whatever has the focus.
#[test]
fn every_row_has_a_help_and_real_picker_copy() {
    use crate::list_class::Rows;
    use crate::ui_kit::dialog::{PICKER_HELP_TEXT, SETTINGS_HELP_TEXT, icon, quick_icon};
    use crate::ui_kit::spec::{Describe, fallback, wrap};
    let room = |b: crate::list_class::TextBlock| {
        (i32::from(b.rect[3]) - i32::from(b.rect[1]), i32::from(b.rect[0]) - i32::from(b.rect[2]))
    };
    let (help_width, help_height) = room(SETTINGS_HELP_TEXT);
    let (width, height) = room(PICKER_HELP_TEXT);
    let s = screen(&store());
    let spec = s.spec();
    assert_eq!(spec.art, [quick_icon(icon::SKULL)]);
    let texts: Vec<&str> = spec.texts.iter().map(|t| t.text.as_str()).collect();
    // The about line is the Game Setup row's help: the two always agree.
    assert_eq!(texts, ["Firefight rules", caller().help.as_str()]);
    assert_eq!(
        caller().help,
        "Fight waves of enemies: set the waves, difficulty and lives."
    );
    for (row, r) in doc().rows.values().enumerate() {
        let h = r.help.as_deref().unwrap_or_default();
        assert!(!h.is_empty() && !h.contains('\n'), "{}", r.text);
        let lines = wrap(h, help_width, &fallback::width).len() as i32;
        assert!(lines * fallback::LINE <= help_height, "{}: {lines} lines", r.text);
        let e = r.explain.as_deref().unwrap_or_default();
        assert!(!e.is_empty() && e != h, "{}", r.text);
        assert!(
            e.encode_utf16().count() <= crate::ui_kit::settings::EXPLAIN_UNITS,
            "{}: {e}",
            r.text
        );
        let lines = wrap(e, width, &fallback::width).len() as i32;
        assert!(lines * fallback::LINE <= height, "{}: {lines} lines", r.text);
        let lower = e.to_lowercase();
        for bad in ["docs/", "todo", "placeholder", "lorem", "tbd", "phantom", "peer", "session"] {
            assert!(!lower.contains(bad), "{}: {e}", r.text);
        }
        // A opens the value picker with that explanation.
        assert_eq!(s.press(row as i16), Chosen::Open(picker::NAME), "{}", r.text);
        assert_eq!(picker::current().expect("a pick").description, e, "{}", r.text);
    }
}
