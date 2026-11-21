use super::*;
use crate::ui::Chosen;
use crate::ui_kit::input::Input::{self, *};
use crate::ui_kit::page::{ListEffect, ListPage, Page, described, named, run};
use crate::ui_kit::picker;

fn store() -> Arc<Store> {
    Arc::new(Store::defaults(&DEFS))
}

fn screen(kept: &Arc<Store>) -> Arc<KitScreen> {
    Arc::new(page(Arc::clone(kept)))
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
            "focus-2",
            &[A],
            vec![open()],
            "focus 2 Difficulty = Normal",
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
                stepped("firefight: left on row 1: firefight.waveset -> Last Stand"),
            ],
            "focus 1 Wave set = Last Stand",
            vec![(&WAVE_SET, Value::Int(2))],
        ),
        case(
            "focus-2",
            &[Left],
            vec![stepped("firefight: left on row 2: firefight.difficulty -> Easy")],
            "focus 2 Difficulty = Easy",
            vec![(&DIFFICULTY, Value::Int(0))],
        ),
        // Right three times from Normal goes Heroic, Legendary, Easy.
        case(
            "focus-2",
            &[Right, Right, Right],
            vec![
                stepped("firefight: right on row 2: firefight.difficulty -> Heroic"),
                stepped("firefight: right on row 2: firefight.difficulty -> Legendary"),
                stepped("firefight: right on row 2: firefight.difficulty -> Easy"),
            ],
            "focus 2 Difficulty = Easy",
            vec![(&DIFFICULTY, Value::Int(0))],
        ),
        // The lives start at the wave set's own and wrap both ways.
        case(
            "focus-2",
            &[Down, Right],
            vec![ListEffect::Focus(3), stepped("firefight: right on row 3: firefight.lives -> 1")],
            "focus 3 Starting lives = 1",
            vec![(&LIVES, Value::Int(1))],
        ),
        case(
            "focus-2",
            &[Down, Left],
            vec![ListEffect::Focus(3), stepped("firefight: left on row 3: firefight.lives -> 30")],
            "focus 3 Starting lives = 30",
            vec![(&LIVES, Value::Int(30))],
        ),
        // The ends: down at the last row and up at the first stay put.
        case(
            "focus-4",
            &[Down],
            vec![ListEffect::None],
            "focus 4 Skulls = On",
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
            "focus-4",
            &[Up, Up, Up, Up, Up],
            vec![
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
            "focus-4",
            &[Right],
            vec![stepped("firefight: right on row 4: firefight.skulls -> Off")],
            "focus 4 Skulls = Off",
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
            "focus-2",
            &[Down, Back],
            vec![ListEffect::Focus(3), ListEffect::Close],
            "focus 3 Starting lives = Default",
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
    let expected: [(&str, &[&str]); 5] = [
        ("FIREFIGHT", &["Off", "On"]),
        ("WAVE SET", &["Holdout", "Attrition", "Last Stand"]),
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
    assert_eq!(DEFS.len(), ROWS.len(), "a row per setting");
}

#[test]
fn the_defaults_are_the_documented_match() {
    assert_eq!(Options::read(&store()), Ok(Options::default()));
    let d = Options::default();
    assert_eq!(
        (d.enabled, d.wave_set, d.difficulty, d.lives, d.skulls),
        (true, 0, Difficulty::Normal, None, true)
    );
    assert_eq!(d.definition().unwrap(), builtin::definitions()[0]);
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
        for lives in lives_choices() {
            for skulls in [true, false] {
                let o = Options {
                    wave_set,
                    lives,
                    skulls,
                    ..Options::default()
                };
                let def = o.definition().unwrap();
                ::firefight::validate(&def, &limits).unwrap_or_else(|e| panic!("{o:?}: {e}"));
            }
        }
    }
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
    let without_skulls = Store::defaults(&[&ENABLED, &WAVE_SET, &DIFFICULTY, &LIVES]);
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
    assert_eq!(ROW_LABEL, "Firefight Options...");
    assert!(ROW_HELP.len() < 120, "a few lines under Game Setup's list");
}

/// The copy is spaced and punctuated as the other pages' is.
#[test]
fn the_copy_is_spaced() {
    let mut all: Vec<&str> = vec![ROW_LABEL, ROW_HELP, TITLE];
    for r in &ROWS {
        all.extend([r.label, r.description, r.explain, r.def.help]);
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
    assert!(ROW_LABEL.ends_with("..."));
    assert_eq!(TITLE, TITLE.to_uppercase());
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

/// Every row has an icon of Quick Options' picture, a one-line help and an
/// explanation for the value picker that says more than the help and is
/// player copy.
#[test]
fn every_row_has_an_icon_a_line_and_real_picker_copy() {
    use crate::list_class::Rows;
    use crate::ui_kit::dialog::{PICKER_HELP_TEXT, icon};
    use crate::ui_kit::spec::{fallback, wrap};
    let width = i32::from(PICKER_HELP_TEXT.rect[3]) - i32::from(PICKER_HELP_TEXT.rect[1]);
    let height = i32::from(PICKER_HELP_TEXT.rect[0]) - i32::from(PICKER_HELP_TEXT.rect[2]);
    let s = screen(&store());
    let mut icons = Vec::new();
    for (row, r) in ROWS.iter().enumerate() {
        let i = r.icon.unwrap_or_else(|| panic!("{}: an icon", r.label));
        assert!(i < icon::COUNT, "{}", r.label);
        icons.push(i);
        assert!(!r.description.is_empty() && !r.description.contains('\n'));
        assert!(
            r.description.encode_utf16().count() <= crate::h1_options::HELP_UNITS,
            "{}: one line",
            r.label
        );
        let e = r.explain;
        assert!(!e.is_empty() && e != r.description, "{}", r.label);
        assert!(
            e.encode_utf16().count() <= crate::h1_options::EXPLAIN_UNITS,
            "{}: {e}",
            r.label
        );
        let lines = wrap(e, width, &fallback::width).len() as i32;
        assert!(
            lines * fallback::LINE <= height,
            "{}: {lines} lines",
            r.label
        );
        let lower = e.to_lowercase();
        for bad in [
            "docs/",
            "todo",
            "placeholder",
            "lorem",
            "tbd",
            "phantom",
            "peer",
            "session",
        ] {
            assert!(!lower.contains(bad), "{}: {e}", r.label);
        }
        // A opens the value picker with that explanation.
        assert_eq!(
            s.press(row as i16),
            Chosen::Open(picker::NAME),
            "{}",
            r.label
        );
        assert_eq!(
            picker::current().expect("a pick").description,
            e,
            "{}",
            r.label
        );
    }
    icons.sort();
    icons.dedup();
    assert_eq!(icons.len(), ROWS.len(), "each row its own icon");
}
