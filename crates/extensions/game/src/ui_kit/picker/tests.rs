use super::*;
use crate::ui_kit::choice::example::{COUNT, COUNT_DEF, LEVEL, LEVEL_DEF, store};
use crate::ui_kit::dialog::{
    DialogGeometry, LARGE_SCREEN_ID, PICKER_HELP, VALUE_PICKER_SCREEN_ID, dialog_view,
};
use crate::ui_kit::page::{ListEffect, Page, described};
use crate::ui_kit::spec::{Finding, ValueStyle, check_fallback};
use crate::ui_kit::view::{Art, Synthetic};
use settings::Value;

fn noop() -> Arc<dyn Fn(usize) -> String + Send + Sync> {
    Arc::new(|i| format!("set {i}"))
}

/// The title is the row's name in capitals, as retail's ("SCORE TO WIN
/// ROUND"), without a colon or an ellipsis.
#[test]
fn the_title_is_the_settings_name_in_capitals() {
    assert_eq!(title("Score to win round"), "SCORE TO WIN ROUND");
    assert_eq!(title("Count:"), "COUNT");
    assert_eq!(title("Clanker Options..."), "CLANKER OPTIONS");
}

/// Two values open it (retail's On and Off settings do); one, or more
/// than its slots hold, leave A to step the row.
#[test]
fn it_takes_two_to_max_choices() {
    let values = |n: usize| (0..n).map(|i| i.to_string()).collect::<Vec<_>>();
    assert_eq!(offer(Pick::new("One", values(1), 0, "", noop())), None);
    assert_eq!(
        offer(Pick::new("Two", values(2), 1, "", noop())),
        Some(NAME)
    );
    assert_eq!(current().unwrap().title, "TWO");
    assert_eq!(
        offer(Pick::new("Many", values(MAX_CHOICES), 0, "", noop())),
        Some(NAME)
    );
    assert_eq!(
        offer(Pick::new(
            "Too many",
            values(MAX_CHOICES + 1),
            0,
            "",
            noop()
        )),
        None
    );
    assert_eq!(
        current().unwrap().title,
        "MANY",
        "a refused pick changes nothing"
    );
    assert_eq!(
        Pick::new("Held", values(3), 9, "", noop()).current,
        2,
        "the held value is one of them"
    );
    let slots = VALUES as usize + MAX_CHOICES;
    assert_eq!(slots as u32, ids::family(NAME).len);
    assert_eq!(slots as u32, ids::family(GAME_NAME).len);
}

#[test]
fn a_filter_list_reuses_its_text_slots_and_selects_the_scrolled_row() {
    let n = MAX_CHOICES + 2;
    let values = (0..n).map(|i| format!("Map {i}")).collect();
    let chosen = Arc::new(Mutex::new(None));
    let save = chosen.clone();
    let set = Arc::new(move |i| {
        *save.lock().unwrap() = Some(i);
        String::new()
    });
    assert_eq!(offer_list(Pick::new("Map", values, 0, "", set)), Some(NAME));
    let picker = Picker::new(false);
    assert_eq!(picker.count(), n);
    assert_eq!(picker.texts(MAX_CHOICES as i16), picker.texts(0));
    assert_eq!(picker.texts((n - 1) as i16), picker.texts(1));
    assert_eq!(picker.spec().rows[n - 1].label, format!("Map {}", n - 1));
    assert_eq!(picker.press((n - 1) as i16), Chosen::Act);
    assert_eq!(*chosen.lock().unwrap(), Some(n - 1));
}

/// In the main menu it is `low_level_settings`; in a game the large
/// dialog.
#[test]
fn the_game_gets_the_large_dialogs_picker() {
    assert_eq!(screen_for(NAME, false), NAME);
    assert_eq!(screen_for(NAME, true), GAME_NAME);
    assert_eq!(screen_for("clankers", true), "clankers");
    assert_eq!(Picker::new(false).base(), DialogBase::ValuePicker);
    assert_eq!(Picker::new(true).base(), DialogBase::Large);
    assert!(DialogBase::Large.in_game());
    assert!(!DialogBase::ValuePicker.in_game());
}

/// A on a kit screen's value row opens the picker on that row: its name in
/// capitals, every value, the held one, the row's description; choosing
/// a value through the picker saves it to the store and closes.
#[test]
fn a_on_a_value_row_opens_the_picker_and_a_choice_is_saved() {
    let s = store();
    let page = crate::ui_kit::screens::dialog_example(Arc::clone(&s), DialogBase::QuickOptions);
    assert_eq!(page.press(1), Chosen::Open(NAME));
    let p = current().expect("a pick");
    assert_eq!(p.title, "DIFFICULTY");
    assert_eq!(p.values, ["Easy", "Normal", "Heroic", "Legendary"]);
    assert_eq!(p.current, 1);
    assert_eq!(p.description, LEVEL.description);
    assert_eq!(p.opener, Opener::MAIN_MENU, "the page's own opener");

    let picker = Arc::new(Picker::new(false));
    assert_eq!(picker.count(), 4);
    assert_eq!(picker.press(3), Chosen::Act, "the action closes it");
    assert!(picker.chosen.load(Ordering::SeqCst));
    assert_eq!(s.get(LEVEL_DEF.key), Some(Value::Int(3)), "saved");
    assert_eq!(current().unwrap().current, 3);
    assert_eq!(picker.press(9), Chosen::Nothing, "no such value");

    // Count's nine values scroll in the six rows.
    assert_eq!(page.press(0), Chosen::Open(NAME));
    let spec = picker.spec();
    assert_eq!(spec.rows.len(), 9);
    assert!(spec.scrolls);
    assert_eq!(spec.focus, COUNT.index(&s));
    assert_eq!(
        s.get(COUNT_DEF.key),
        Some(Value::Int(4)),
        "opening changes nothing"
    );
}

/// Offline, as the game steps it: A on a value sets it and runs the
/// action (the close), B closes and changes nothing, X and Start do
/// nothing, left and right are not taken.
#[test]
fn the_picker_page_selects_or_cancels() {
    use crate::ui_kit::input::Input::*;
    let s = store();
    let picker = || {
        let held = Arc::clone(&s);
        offer(Pick::new(
            LEVEL.label,
            LEVEL.choices().into_iter().map(|(_, n)| n).collect(),
            LEVEL.index(&s),
            LEVEL.description,
            Arc::new(move |i| LEVEL.set(&held, i)),
        ));
        let p = Arc::new(Picker::new(false));
        crate::ui_kit::page::ListPage::new(p.class(), described(&p))
    };
    let page = picker();
    let mut f = crate::ui_kit::page::Focus(0);
    assert_eq!(page.step(&mut f, B), ListEffect::Close);
    assert_eq!(s.get(LEVEL_DEF.key), Some(Value::Int(1)), "B: unchanged");
    for i in [X, Start, Left, Right] {
        assert_eq!(page.step(&mut f, i), ListEffect::None, "{i:?}");
    }
    assert_eq!(s.get(LEVEL_DEF.key), Some(Value::Int(1)));
    let page = picker();
    let mut f = crate::ui_kit::page::Focus(0);
    assert_eq!(page.step(&mut f, Down), ListEffect::Focus(1));
    assert_eq!(page.step(&mut f, Down), ListEffect::Focus(2));
    assert_eq!(page.step(&mut f, A), ListEffect::Action);
    assert_eq!(s.get(LEVEL_DEF.key), Some(Value::Int(2)), "A: Heroic saved");
}

/// The spec is `low_level_settings` inside: its dialog, its list and
/// skin, a value per row with nothing beside it, the description block
/// on the right, a capital title and `(A) SELECT (B) CANCEL` in the
/// footer; it passes the kit's static checks.
#[test]
fn the_picker_is_low_level_settings_inside() {
    offer(crate::ui_kit::screens::picker_sample());
    for (ingame, base) in [(false, DialogBase::ValuePicker), (true, DialogBase::Large)] {
        let p = Picker::new(ingame);
        let s = p.spec();
        let d = s.dialog.as_ref().expect("a dialog");
        assert_eq!(d.base, base);
        assert_eq!(d.rows, s.rows);
        assert_eq!(d.opener, Opener::GAME_SETUP_CLANKERS);
        assert_eq!(s.title, "COUNT");
        assert_eq!(s.style, ValueStyle::Inline);
        assert_eq!(s.list, ListPlace::from(Some(Picker::layout())));
        assert_eq!(
            s.list,
            DialogGeometry::list_place(PICKER_LIST, PICKER_LIST_ROWS)
        );
        assert_eq!(s.description, Some(PICKER_HELP_TEXT.into()));
        assert_eq!(s.focus, 4);
        let (label, value) = s.rows[4].cells(s.style);
        assert_eq!((label.as_str(), value), ("4", None));
        assert_eq!(s.description_text().as_deref(), Some(COUNT.description));
        assert_eq!(
            s.footer_text().as_deref(),
            Some("\u{e100} SELECT  \u{e101} CANCEL")
        );
        let f: Vec<Finding> = check_fallback(&s)
            .into_iter()
            .filter(|f| !matches!(f, Finding::Wide { .. } | Finding::Description { .. }))
            .collect();
        assert_eq!(f, vec![], "{base:?}");
    }
}

/// `low_level_settings` as the disc has it (held to the disc by
/// `ui_kit::screens::tests::preview_and_check_against_the_disc`): the
/// large dialog's header and panel, its own list and description.
#[test]
fn the_value_picker_geometry_is_the_large_dialogs_with_its_own_list() {
    let large = DialogGeometry::fallback_for(DialogBase::Large);
    let g = DialogGeometry::fallback_for(DialogBase::ValuePicker);
    assert_eq!(g.screen_id, VALUE_PICKER_SCREEN_ID);
    assert_eq!(g.header, large.header);
    assert_eq!(g.footer, large.footer);
    assert_eq!(g.list, PICKER_LIST);
    assert_eq!(g.list.skin, 22);
    assert_eq!(g.list_items, 6);
    assert_eq!(g.panel.len(), 4);
    for (a, b) in g.panel.iter().zip(&large.panel) {
        assert_eq!((a.at, a.size), (b.at, b.size));
        match (&a.art, &b.art) {
            (
                Art::Screen {
                    screen_id: s,
                    index: i,
                    ..
                },
                Art::Screen {
                    screen_id: t,
                    index: j,
                    ..
                },
            ) => {
                assert_eq!((*s, *t), (VALUE_PICKER_SCREEN_ID, LARGE_SCREEN_ID));
                assert_eq!(i, j);
            }
            _ => panic!("panel art is a screen's"),
        }
    }
    assert_eq!(g.rows.skin, 22, "default6");
    assert_eq!(g.rows.item_height, 22);
    // The description is right of the list, inside the panel.
    let area = g.area();
    assert!(PICKER_HELP.left > g.list.anchor[0] + 210);
    assert!(PICKER_HELP.top <= area.top && PICKER_HELP.bottom >= area.bottom);
    assert!(PICKER_HELP.right <= area.right);
}

/// Drawn: the values down the list from its anchor, the focused one lit,
/// the description inside the panel, nothing outside it; the checks find
/// only the large dialog's see-through contrast over its opener.
#[test]
fn the_picker_draws_inside_its_panel() {
    use crate::ui_kit::layout::{Issue, Rules, check};
    use crate::ui_kit::preview::SyntheticSource;
    offer(crate::ui_kit::screens::picker_sample());
    for ingame in [false, true] {
        let p = Picker::new(ingame);
        let s = p.spec();
        let d = s.dialog.clone().unwrap();
        let g = DialogGeometry::fallback_for(p.base());
        let g = DialogGeometry {
            list: PICKER_LIST,
            list_items: PICKER_LIST_ROWS,
            rows: crate::ui_kit::view::Geometry::fallback(s.list),
            ..g
        };
        let v = dialog_view(&s, &d, &g, &Synthetic, "t");
        assert_eq!(v.focus.as_deref(), Some("row.4"));
        assert_eq!(v.widget("row.4.label").unwrap().text(), Some("4"));
        let desc = v.widget("description").expect("the description");
        let r = desc.rect;
        assert_eq!([r[0], r[1]], [PICKER_HELP.top, PICKER_HELP.left]);
        let issues = check(&v, &SyntheticSource, &Rules::for_spec(&s)).issues;
        assert!(
            issues.iter().all(|i| matches!(i, Issue::Contrast { .. })),
            "{ingame}: {issues:?}"
        );
    }
}
