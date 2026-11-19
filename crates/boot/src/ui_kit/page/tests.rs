//! Every native screen's behaviour as tables: from a named state, the
//! events in turn, the effects they give and what the screen shows after.

use super::*;
use crate::signin_keyboard::Field;
use crate::signin_screen::{Effect, SigninEvent, SigninPage};
use crate::ui_kit::screens::{NAMES, live};
use Input::*;
use live_hle::signin::Phase;

/// One row of a table: the state it starts from, the events, the effects
/// they give, and the summary after.
struct Case<P: Page> {
    from: &'static str,
    events: Vec<P::Event>,
    effects: Vec<P::Effect>,
    after: &'static str,
}

fn case<P: Page>(
    from: &'static str,
    events: impl IntoIterator<Item = P::Event>,
    effects: impl IntoIterator<Item = P::Effect>,
    after: &'static str,
) -> Case<P> {
    Case {
        from,
        events: events.into_iter().collect(),
        effects: effects.into_iter().collect(),
        after,
    }
}

/// Each case on a fresh page from `make`: a list page's rows keep their
/// own state (the store), which one case must not hand the next.
fn check<P: Page>(make: impl Fn() -> P, cases: Vec<Case<P>>) {
    for (i, c) in cases.into_iter().enumerate() {
        let page = &make();
        let mut s = named(page, c.from);
        let what = format!("case {i}: from {} on {:?}", c.from, c.events);
        let effects = run(page, &mut s, c.events);
        assert_eq!(effects, c.effects, "{what}: effects");
        assert_eq!(page.summary(&s), c.after, "{what}: shows");
    }
}

fn press(inputs: &[Input]) -> Vec<SigninEvent> {
    inputs.iter().map(|&i| SigninEvent::Press(i)).collect()
}

fn typed(text: Option<&str>) -> SigninEvent {
    SigninEvent::Typed(text.map(String::from))
}

fn answer(r: Result<(), SetupError>) -> SigninEvent {
    SigninEvent::Answer(r.map(|()| None))
}

/// The code page: digits wrap, the cursor stops at either end, A submits.
#[test]
fn signin_digits_cursor_and_submit() {
    let r = Effect::Redraw;
    check(
        || SigninPage,
        vec![
            case(
                "code",
                press(&[Up]),
                [r.clone()],
                "Sign in as Chief | [1] 0 0 - 0 0 0 | -",
            ),
            case(
                "code",
                press(&[Down]),
                [r.clone()],
                "Sign in as Chief | [9] 0 0 - 0 0 0 | -",
            ),
            case(
                "code",
                press(&[Up; 10]),
                vec![r.clone(); 10],
                "Sign in as Chief | [0] 0 0 - 0 0 0 | -",
            ),
            // The cursor stops at the first digit and at the last.
            case(
                "code",
                press(&[Left]),
                [Effect::None],
                "Sign in as Chief | [0] 0 0 - 0 0 0 | -",
            ),
            case(
                "code",
                press(&[Right; 6]),
                [vec![r.clone(); 5], vec![Effect::None]].concat(),
                "Sign in as Chief | 0 0 0 - 0 0 [0] | -",
            ),
            // Each digit is its own.
            case(
                "code",
                press(&[Up, Right, Up, Up, Right, Right, Down]),
                vec![r.clone(); 7],
                "Sign in as Chief | 1 2 0 - [9] 0 0 | -",
            ),
            // A submits the code as typed; the page checks it.
            case(
                "code-typed",
                press(&[A]),
                [Effect::Submit("290000".into())],
                "Checking... | 2 9 [0] - 0 0 0 | -",
            ),
            // Start submits as A does, as the legend says.
            case(
                "code",
                press(&[Start]),
                [Effect::Submit("000000".into())],
                "Checking... | [0] 0 0 - 0 0 0 | -",
            ),
            // X never submits: a stray X must not send a half-typed code.
            case(
                "code-typed",
                press(&[X]),
                [Effect::None],
                "Sign in as Chief | 2 9 [0] - 0 0 0 | -",
            ),
            case(
                "code",
                press(&[Y]),
                [Effect::None],
                "Sign in as Chief | [0] 0 0 - 0 0 0 | -",
            ),
            // The server's answers.
            case(
                "checking",
                [answer(Ok(()))],
                [r.clone()],
                "Signed in. A: done | 2 9 [0] - 0 0 0 | -",
            ),
            case(
                "checking",
                [answer(Err(SetupError::NoSuchGamertag))],
                [r.clone()],
                "Make the account | 2 9 [0] - 0 0 0 | -",
            ),
            case(
                "done",
                press(&[A]),
                [Effect::Close],
                "Signed in. A: done | [0] 0 0 - 0 0 0 | -",
            ),
        ],
    );
}

/// An error shows, and clears on the next edit; the next answer shows
/// again.
#[test]
fn signin_errors_show_and_clear_on_the_next_edit() {
    let left = |n| SetupError::WrongCodeLeft { left: n };
    check(
        || SigninPage,
        vec![
            case(
                "code-wrong",
                [],
                [],
                "Sign in as Chief | 2 9 [0] - 0 0 0 | Wrong code. 2 tries left.",
            ),
            case(
                "code-wrong",
                press(&[Up]),
                [Effect::Redraw],
                "Sign in as Chief | 2 9 [1] - 0 0 0 | -",
            ),
            case(
                "code-wrong",
                press(&[Left]),
                [Effect::Redraw],
                "Sign in as Chief | 2 [9] 0 - 0 0 0 | -",
            ),
            case(
                "code-wrong",
                [
                    SigninEvent::Press(Up),
                    SigninEvent::Press(A),
                    answer(Err(left(1))),
                ],
                [
                    Effect::Redraw,
                    Effect::Submit("291000".into()),
                    Effect::Redraw,
                ],
                "Sign in as Chief | 2 9 [1] - 0 0 0 | Wrong code. 1 try left.",
            ),
            // The same error again shows again: it is a new answer.
            case(
                "code-wrong",
                [SigninEvent::Press(A), answer(Err(left(2)))],
                [Effect::Submit("290000".into()), Effect::Redraw],
                "Sign in as Chief | 2 9 [0] - 0 0 0 | Wrong code. 2 tries left.",
            ),
            case(
                "code-unreachable",
                press(&[Down]),
                [Effect::Redraw],
                "Sign in as Chief | 2 9 [9] - 0 0 0 | -",
            ),
            case(
                "gamertag-bad",
                [],
                [],
                "New Xbox Live account | [0] 0 0 - 0 0 0 | That gamertag can't be used.",
            ),
        ],
    );
}

/// Checking takes no edits and no A; B still cancels, and an answer after
/// that is dropped.
#[test]
fn signin_checking_ignores_edits() {
    let shows = "Checking... | 2 9 [0] - 0 0 0 | -";
    check(
        || SigninPage,
        vec![
            case(
                "checking",
                press(&[Up, Down, Left, Right, A, X, Start]),
                vec![Effect::None; 7],
                shows,
            ),
            case(
                "checking",
                [SigninEvent::Press(B), answer(Ok(()))],
                [Effect::Cancel, Effect::None],
                "Cancelled | 2 9 [0] - 0 0 0 | -",
            ),
        ],
    );
}

/// B cancels from every page still asking; on a finished page it closes.
/// Back is B.
#[test]
fn signin_b_cancels_from_every_page() {
    for (name, state) in SigninPage.states() {
        for input in [B, Back] {
            let mut s = state.clone();
            let effect = SigninPage.step(&mut s, input.into());
            if state.prompt.phase.finished() {
                assert_eq!(effect, Effect::Close, "{name} {input:?}");
                assert_eq!(s.prompt.phase, state.prompt.phase, "{name}");
            } else {
                assert_eq!(effect, Effect::Cancel, "{name} {input:?}");
                assert_eq!(s.prompt.phase, Phase::Cancelled, "{name}");
            }
        }
    }
}

/// The new-account pages hand off to the keyboard; what is typed goes to
/// the server; a taken gamertag becomes the code page for it.
#[test]
fn signin_new_account_pages() {
    let r = Effect::Redraw;
    check(
        || SigninPage,
        vec![
            case(
                "gamertag",
                press(&[A]),
                [Effect::Keyboard(Field::Gamertag)],
                "New Xbox Live account | [0] 0 0 - 0 0 0 | -",
            ),
            case(
                "gamertag",
                press(&[Up, Left, Right, Down]),
                vec![Effect::None; 4],
                "New Xbox Live account | [0] 0 0 - 0 0 0 | -",
            ),
            case(
                "invite",
                press(&[Start]),
                [Effect::Keyboard(Field::Invite)],
                "Invite code | [0] 0 0 - 0 0 0 | -",
            ),
            // A taken gamertag: the code page for it.
            case(
                "gamertag",
                [
                    SigninEvent::Press(A),
                    typed(Some(" Arbiter ")),
                    answer(Err(SetupError::GamertagTaken)),
                ],
                [Effect::Keyboard(Field::Gamertag), r.clone(), r.clone()],
                "Sign in as Arbiter | [0] 0 0 - 0 0 0 | -",
            ),
            // A free one on a server that wants an invite: the invite page,
            // then made with it.
            case(
                "gamertag",
                [
                    typed(Some("Arbiter")),
                    answer(Err(SetupError::NeedsInvite)),
                    SigninEvent::Press(A),
                    typed(Some("INVITE-1")),
                    answer(Ok(())),
                ],
                [
                    r.clone(),
                    r.clone(),
                    Effect::Keyboard(Field::Invite),
                    r.clone(),
                    r.clone(),
                ],
                "Signed in. A: done | [0] 0 0 - 0 0 0 | -",
            ),
            // A bad gamertag stays on the page with its error; the
            // keyboard backed out of leaves it as it was.
            case(
                "gamertag",
                [typed(Some("no/slash")), typed(None)],
                [r.clone(), Effect::None],
                "New Xbox Live account | [0] 0 0 - 0 0 0 | That gamertag can't be used.",
            ),
            // Backing out of the invite keyboard goes back to the
            // gamertag.
            case(
                "invite",
                [typed(None)],
                [r.clone()],
                "New Xbox Live account | [0] 0 0 - 0 0 0 | -",
            ),
            // The gamertag isn't on the server: A makes it.
            case(
                "create",
                [SigninEvent::Press(A), answer(Ok(()))],
                [Effect::Create, r.clone()],
                "Signed in. A: done | [0] 0 0 - 0 0 0 | -",
            ),
            // Text typed where none is asked for does nothing.
            case(
                "code",
                [typed(Some("123456"))],
                [Effect::None],
                "Sign in as Chief | [0] 0 0 - 0 0 0 | -",
            ),
        ],
    );
}

/// A list screen's table, from its first state.
fn list_case(
    from: &'static str,
    inputs: &[Input],
    effects: Vec<ListEffect>,
    after: &'static str,
) -> Case<ListPage> {
    case(from, inputs.to_vec(), effects, after)
}

/// The page of list screen `name`, fresh (its own store or lobby).
fn list_page(name: &str) -> ListPage {
    crate::ui_kit::screens::list_page(name).unwrap_or_else(|| panic!("{name} is not a list"))
}

/// The Game Options page: Start Level first and A on it goes on through the
/// action; left, right and A step a skull; locked skulls stay locked; X and
/// Start are not the page's; the focus stops at either end.
#[test]
fn game_options_page() {
    use ListEffect as E;
    let locked = "skulls menu: Thunderstorm is locked for Chief: not collected yet \
                  (found on Cairo Station, Legendary only)";
    let p = list_page("skulls");
    let last = p.count() - 1;
    let make = || list_page("skulls");
    let cases = vec![
        list_case("focus-0", &[A], vec![E::Action], "focus 0 Start Level"),
        list_case(
            "focus-0",
            &[X, Start, Y, Left],
            vec![E::None; 4],
            "focus 0 Start Level",
        ),
        list_case("focus-0", &[Up], vec![E::None], "focus 0 Start Level"),
        list_case(
            "focus-0",
            &[Down, Right],
            vec![
                E::Focus(1),
                E::Stepped("skulls page: right on row 1: skulls: Assassins off for Chief".into()),
            ],
            "focus 1 Assassins = < Off >",
        ),
        list_case(
            "focus-0",
            &[Down, A, A],
            vec![
                E::Focus(1),
                E::Stepped("skulls: Assassins off for Chief".into()),
                E::Stepped("skulls: Assassins on for Chief".into()),
            ],
            "focus 1 Assassins = < On >",
        ),
        list_case(
            "focus-0",
            &[Down, Down, A],
            vec![E::Focus(1), E::Focus(2), E::Stepped(locked.into())],
            "focus 2 Thunderstorm = Locked",
        ),
        list_case("focus-0", &[B], vec![E::Close], "focus 0 Start Level"),
        list_case("focus-0", &[Back], vec![E::Close], "focus 0 Start Level"),
    ];
    check(make, cases);
    // The last row: down goes no further.
    let mut s = Focus(last);
    assert_eq!(p.step(&mut s, Down), E::None);
    assert_eq!(s, Focus(last));
}

/// The Halo 1 campaign's page: its Assets and HUD rows step both ways.
#[test]
fn halo1_game_options_rows() {
    use ListEffect as E;
    check(
        || list_page("skulls.h1"),
        vec![list_case(
            "focus-0",
            &[Down, Down, Left, Right, A],
            vec![
                E::Focus(1),
                E::Focus(2),
                E::Stepped("skulls page: left on row 2: h1 options: halo1.hud -> Halo 2".into()),
                E::Stepped("skulls page: right on row 2: h1 options: halo1.hud -> Halo 1".into()),
                E::Stepped("h1 options: halo1.hud -> Halo 2".into()),
            ],
            "focus 2 HUD = < Halo 2 >",
        )],
    );
}

/// The multiplayer skulls screen: A toggles, no sideways steps, no action.
#[test]
fn multiplayer_skulls_page() {
    use ListEffect as E;
    check(
        || list_page("skulls.mp"),
        vec![
            list_case(
                "focus-0",
                &[Left, Right, X, Start],
                vec![E::None; 4],
                "focus 0 Famine: Off",
            ),
            list_case(
                "focus-0",
                &[A],
                vec![E::Stepped(
                    "skulls: Famine on in multiplayer for Chief".into(),
                )],
                "focus 0 Famine: On",
            ),
            list_case("focus-0", &[B], vec![E::Close], "focus 0 Famine: Off"),
        ],
    );
}

/// The Halo 1 options screen: two rows, both step; A opens the value
/// picker, as retail's two-value settings do.
#[test]
fn halo1_options_page() {
    use ListEffect as E;
    check(
        || list_page("h1-options"),
        vec![
            list_case(
                "focus-0",
                &[A],
                vec![E::Open(crate::ui_kit::picker::NAME)],
                "focus 0 Sandbox = < Halo 1 >",
            ),
            list_case(
                "focus-0",
                &[Down, Down],
                vec![E::Focus(1), E::None],
                "focus 1 HUD = < Halo 1 >",
            ),
            list_case(
                "focus-0",
                &[Right],
                vec![E::Stepped(
                    "h1 options: right on row 0: h1 options: multiplayer.halo1_sandbox -> Halo 2"
                        .into(),
                )],
                "focus 0 Sandbox = < Halo 2 >",
            ),
        ],
    );
}

/// The co-op screens: Start (and X) start or say ready, Invite a Friend
/// opens the friends list, which has no action.
#[test]
fn coop_pages() {
    use ListEffect as E;
    check(
        || list_page("coop"),
        vec![
            list_case(
                "focus-0",
                &[Start],
                vec![E::Action],
                "focus 0 Level = Outskirts",
            ),
            list_case(
                "focus-0",
                &[Down, Down, A],
                vec![E::Focus(1), E::Focus(2), E::Open("coop.friends")],
                "focus 2 Invite a Friend",
            ),
            list_case("focus-0", &[B], vec![E::Close], "focus 0 Level = Outskirts"),
        ],
    );
    check(
        || list_page("coop.friends"),
        vec![list_case(
            "focus-0",
            &[Start, X, Down],
            vec![E::None; 3],
            "focus 0 No friends yet",
        )],
    );
    check(
        || list_page("coop.join"),
        vec![
            list_case("focus-0", &[Start], vec![E::Action], "focus 0 Host = -"),
            list_case("focus-0", &[A], vec![E::None], "focus 0 Host = -"),
        ],
    );
}

/// The account menu: up and down wrap (its own model), A on a row asks for
/// the gamertag, B from the menu closes.
#[test]
fn account_page() {
    use crate::account_screen::{Effect as E, Field as F};
    let p = AccountPage;
    let mut s = named(&p, "focus-0");
    assert_eq!(p.step(&mut s, Up.into()), E::Redraw);
    assert_eq!(p.summary(&s), "Live account | focus 2 Cancel");
    let mut s = named(&p, "focus-0");
    match p.step(&mut s, A.into()) {
        E::Keyboard(k) => assert_eq!(k.field, F::Gamertag),
        e => panic!("{e:?}"),
    }
    let mut s = named(&p, "focus-0");
    assert_eq!(p.step(&mut s, B.into()), E::Close { signed_up: false });
    let mut s = named(&p, "focus-2");
    assert_eq!(p.step(&mut s, A.into()), E::Close { signed_up: false });
}

/// The kit's example: X, Start and A on an action row run the action.
#[test]
fn kit_example_page() {
    use ListEffect as E;
    check(
        || list_page("kit.example"),
        vec![list_case(
            "focus-0",
            &[X, Start, Y],
            vec![E::Action, E::Action, E::None],
            "focus 0 Grunts = < On >",
        )],
    );
}

/// The kit's choice rows: left and right step through every choice and
/// wrap, A opens the value picker, up and down move the focus, B closes. X and Start
/// run the kit's action as on every kit screen; with no action row there
/// is nothing pending, so they leave the value alone.
#[test]
fn kit_choices_page() {
    use ListEffect as E;
    let step = |s: &str| E::Stepped(format!("kit.choices: {s}"));
    check(
        || list_page("kit.choices"),
        vec![
            list_case(
                "focus-0",
                &[Right, Right, Right, Right, Right],
                vec![
                    step("right on row 0: kit.example_count -> 5"),
                    step("right on row 0: kit.example_count -> 6"),
                    step("right on row 0: kit.example_count -> 7"),
                    step("right on row 0: kit.example_count -> 8"),
                    step("right on row 0: kit.example_count -> 0"),
                ],
                "focus 0 Count = < 0 >",
            ),
            list_case(
                "focus-1",
                &[Left, Left, A],
                vec![
                    step("left on row 1: kit.example_level -> Easy"),
                    step("left on row 1: kit.example_level -> Legendary"),
                    E::Open(crate::ui_kit::picker::NAME),
                ],
                "focus 1 Difficulty = < Legendary >",
            ),
            list_case(
                "focus-0",
                &[Down, Down, Up, X, Start],
                vec![E::Focus(1), E::None, E::Focus(0), E::Action, E::Action],
                "focus 0 Count = < 4 >",
            ),
            list_case(
                "focus-1",
                &[B],
                vec![E::Close],
                "focus 1 Difficulty = < Normal >",
            ),
        ],
    );
}

/// Every screen steps through every input from every state without a
/// panic, and its spec after passes the kit's static checks.
#[test]
fn every_screen_takes_every_input_in_every_state() {
    use crate::ui_kit::spec::{Finding, check_fallback};
    let all = [Up, Down, Left, Right, A, B, X, Y, Start, Back];
    for name in NAMES {
        let names = live(name).unwrap().state_names();
        assert!(!names.is_empty(), "{name}");
        for state in &names {
            for input in all {
                let mut l = live(name).unwrap();
                assert!(l.go(state));
                l.press(input);
                let f: Vec<Finding> = check_fallback(&l.spec())
                    .into_iter()
                    .filter(|f| !matches!(f, Finding::Wide { .. } | Finding::Description { .. }))
                    .collect();
                assert_eq!(f, vec![], "{name}/{state} after {input:?}");
            }
        }
    }
}
