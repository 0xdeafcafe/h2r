use super::*;
use crate::forge_screens::screen::ForgeScreen;
use crate::forge_screens::sheet::Choice;

fn values(s: &Sheet) -> Vec<(&str, &str)> {
    s.rows
        .iter()
        .map(|r| (r.label.as_str(), r.value.as_deref().unwrap_or("")))
        .collect()
}

#[test]
fn the_rows_are_all_then_each_engine_then_done() {
    let s = sheet(GameSet::of(&[2, 3]));
    assert_eq!(
        values(&s),
        [
            ("All game types", "Off"),
            ("King of the Hill", "Off"),
            ("Capture the Flag", "Off"),
            ("Slayer", "On"),
            ("Oddball", "On"),
            ("Juggernaut", "Off"),
            ("Territories", "Off"),
            ("Assault", "Off"),
            ("Done", ""),
        ]
    );
    assert_eq!(set_of(&s), GameSet::of(&[2, 3]));
    assert_eq!(sheet(GameSet::ALL).rows[0].value.as_deref(), Some("On"));
}

#[test]
fn a_tick_flips_its_engine_and_all_follows() {
    let mut s = sheet(GameSet::of(&[2]));
    tick(&mut s, Tick::Engine(3));
    assert_eq!(set_of(&s), GameSet::of(&[2, 3]));
    tick(&mut s, Tick::Engine(2));
    assert_eq!(set_of(&s), GameSet::of(&[3]));
    tick(&mut s, Tick::All);
    assert_eq!(set_of(&s), GameSet::ALL);
    assert_eq!(s.rows[0].value.as_deref(), Some("On"));
    tick(&mut s, Tick::Engine(4));
    assert_eq!(s.rows[0].value.as_deref(), Some("Off"), "one off, not all");
    tick(&mut s, Tick::All);
    tick(&mut s, Tick::All);
    assert_eq!(set_of(&s), GameSet::NONE, "all ticked, then cleared");
}

#[test]
fn done_takes_a_set_an_entry_holds() {
    let mut s = sheet(GameSet::of(&[2]));
    tick(&mut s, Tick::Engine(1));
    tick(&mut s, Tick::Engine(9));
    assert_eq!(done(&mut s), Ok(GameSet::of(&[1, 2, 9])));
}

#[test]
fn done_refuses_what_the_document_refuses_and_says_why() {
    // Capture the Flag without Assault.
    let mut s = sheet(GameSet::of(&[2]));
    tick(&mut s, Tick::Engine(1));
    let why = done(&mut s).unwrap_err();
    assert_eq!(why, "Capture the Flag and Assault go together.");
    let last = s.rows.last().unwrap();
    assert_eq!(last.description.as_deref(), Some(why.as_str()));
    // Five slots (the flag pair shares one); the five engines without the
    // flags are the all-but-flags group and fit.
    let mut s = sheet(GameSet::of(&[2, 3, 4, 7]));
    tick(&mut s, Tick::Engine(1));
    tick(&mut s, Tick::Engine(9));
    assert_eq!(
        done(&mut s).unwrap_err(),
        "An item can name four game types at most."
    );
    // Nothing ticked.
    let mut s = sheet(GameSet::of(&[2]));
    tick(&mut s, Tick::Engine(2));
    assert_eq!(done(&mut s).unwrap_err(), "An item needs a game type.");
    // Ticking again takes the sentence back.
    tick(&mut s, Tick::Engine(2));
    assert_ne!(
        s.rows.last().unwrap().description.as_deref(),
        Some("An item needs a game type.")
    );
}

#[test]
fn the_screen_ticks_in_place_and_done_sends_one_choice() {
    use crate::list_class::Rows;
    use crate::ui::Chosen;
    let screen = ForgeScreen::new(Kind::Games);
    screen.show(sheet(GameSet::of(&[2]))).unwrap();
    // Slayer's row unticks, Oddball's ticks; a tick sends nothing.
    assert!(matches!(screen.press(3), Chosen::Stepped(_)));
    assert!(matches!(screen.press(4), Chosen::Stepped(_)));
    assert_eq!(screen.sheet().rows[3].value.as_deref(), Some("Off"));
    assert_eq!(screen.sheet().rows[4].value.as_deref(), Some("On"));
    assert_eq!(screen.take_choices(), []);
    // Done: one choice and the dialog closes.
    assert_eq!(screen.press(8), Chosen::Act);
    assert_eq!(screen.take_choices(), [Choice::Games(GameSet::of(&[3]))]);
}

#[test]
fn a_refused_done_stays_up_and_writes_nothing() {
    use crate::list_class::Rows;
    use crate::ui::Chosen;
    let screen = ForgeScreen::new(Kind::Games);
    screen.show(sheet(GameSet::of(&[2]))).unwrap();
    screen.press(2); // Capture the Flag alone
    assert!(matches!(screen.press(8), Chosen::Stepped(_)));
    assert_eq!(screen.take_choices(), []);
    assert_eq!(
        screen.sheet().rows[8].description.as_deref(),
        Some("Capture the Flag and Assault go together.")
    );
}
