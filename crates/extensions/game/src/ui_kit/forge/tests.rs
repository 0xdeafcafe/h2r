use super::{
    dialogs::Effect,
    pages::{DialogPage, HudPage, sample_hud},
    *,
};
use crate::ui_kit::{
    content,
    input::Input,
    layout,
    page::{Page, named, run},
    preview, spec,
    view::Backing,
};

#[test]
fn monitor_hud_in_every_state() {
    let from = preview::From::Synthetic;
    for (name, hud) in HudPage.states() {
        let s = hud.spec();
        let v = from.view(&s, &name).unwrap();
        assert_eq!(v.backing, Backing::None);
        assert_eq!((v.focus.as_deref(), v.list), (None, None));
        assert!(v.widget("reticle").is_some());
        assert_eq!(v.widget("notice").is_some(), hud.notice.is_some());
        assert_eq!(v.widget("target").is_some(), hud.mode != Mode::Idle);
        assert!(spec::check_fallback(&s).is_empty(), "{name}");
        let content = content::check(&s, &v, from.source());
        assert!(content.is_empty(), "{name}: {content:?}");
        let report = layout::check(&v, from.source(), &layout::Rules::for_spec(&s));
        assert!(report.issues.is_empty(), "{name}: {:?}", report.issues);
        let canvas = preview::render_overlay(&v, from.source());
        assert_eq!(canvas.rgba[3], 0, "the world is visible outside the strip");
        assert!(canvas.rgba.chunks_exact(4).any(|p| p[3] != 0));
    }
}

#[test]
fn dialog_page_tables() {
    use Action::*;
    use DialogPage::*;
    use Effect::{Action as Command, Changed, Ignored};
    use Input::{A, B, Down, Left, Right, Up};
    struct Case {
        page: DialogPage,
        from: &'static str,
        inputs: &'static [Input],
        effects: Vec<Effect>,
        after: &'static str,
    }
    for case in [
        Case {
            page: Palette,
            from: "open",
            inputs: &[A, Up, A],
            effects: vec![Changed, Changed, Command(Select(2))],
            after: "DEVICES | focus 0 Button",
        },
        Case {
            page: Palette,
            from: "objects",
            inputs: &[B, Down, A, A],
            effects: vec![Changed, Changed, Changed, Command(Select(1))],
            after: "SCENERY | focus 0 Beacon",
        },
        Case {
            page: Properties,
            from: "open",
            inputs: &[A],
            effects: vec![Changed],
            after: "HEADING | focus 0 17.5 degrees",
        },
        Case {
            page: Properties,
            from: "heading",
            inputs: &[A],
            effects: vec![Changed],
            after: "PROPERTIES | focus 0 Heading = 17.5 degrees",
        },
        Case {
            page: Properties,
            from: "heading",
            inputs: &[Down, Down, A],
            effects: vec![Changed, Changed, Command(Heading(1))],
            after: "HEADING | focus 2 45 degrees",
        },
        Case {
            page: Menu,
            from: "open",
            inputs: &[Down, A],
            effects: vec![Changed, Command(SaveAs)],
            after: "FORGE | focus 1 Save as",
        },
        Case {
            page: Palette,
            from: "scrolled",
            inputs: &[Down, A],
            effects: vec![Ignored, Command(Select(8))],
            after: "SCENERY | focus 6 Tray",
        },
        Case {
            page: Palette,
            from: "empty",
            inputs: &[A, B],
            effects: vec![Ignored, Command(Close)],
            after: "PALETTE | focus 0 (no row)",
        },
        Case {
            page: Properties,
            from: "open",
            inputs: &[Left, Right, Down, A],
            effects: vec![Ignored, Ignored, Changed, Changed],
            after: "ROTATION STEP | focus 2 15 degrees",
        },
        Case {
            page: Properties,
            from: "rotation",
            inputs: &[Up, A],
            effects: vec![Changed, Command(RotationStep(1))],
            after: "ROTATION STEP | focus 1 5 degrees",
        },
        Case {
            page: Properties,
            from: "rotation",
            inputs: &[Down, B],
            effects: vec![Changed, Changed],
            after: "PROPERTIES | focus 1 Rotation step = 15 degrees",
        },
        Case {
            page: Menu,
            from: "open",
            inputs: &[A],
            effects: vec![Command(Save)],
            after: "FORGE | focus 0 Save",
        },
        Case {
            page: Menu,
            from: "open",
            inputs: &[Down, Down, A, Down, A],
            effects: vec![
                Changed,
                Changed,
                Changed,
                Changed,
                Command(Load("Stage".into())),
            ],
            after: "LOAD | focus 1 Stage",
        },
        Case {
            page: Menu,
            from: "load",
            inputs: &[B, B],
            effects: vec![Changed, Command(Close)],
            after: "FORGE | focus 2 Load",
        },
        Case {
            page: Menu,
            from: "no-saves",
            inputs: &[A, B],
            effects: vec![Ignored, Changed],
            after: "FORGE | focus 2 Load",
        },
    ] {
        let mut state = named(&case.page, case.from);
        assert_eq!(
            run(&case.page, &mut state, case.inputs.iter().copied()),
            case.effects
        );
        assert_eq!(case.page.summary(&state), case.after);
    }
}

#[test]
fn mouse_uses_topmost_rows_and_preserves_underlying_focus() {
    let hud = sample_hud(Mode::Aimed);
    let mut dialogs = Dialogs::menu(vec!["First".into(), "Second".into()]);
    assert_eq!(dialogs.click(2), Action::None);
    assert_eq!(dialogs.click(5), Action::None);
    let top = dialogs.spec(&hud);
    let menu = top.underlay.as_ref().unwrap();
    assert_eq!(
        (&*top.name, &*menu.name, menu.focus),
        ("forge.load", MENU, 2)
    );
    assert!(menu.underlay.as_ref().unwrap().hud.is_some());
    assert_eq!(dialogs.click(1), Action::Load("Second".into()));
}
