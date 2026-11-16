//! App state and commands without a window, on the scenario crate's
//! invented cache (no game data).
use super::*;
use scenario_edit::{Command, Value, fixture};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2-studio-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn map(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let p = self.0.join(format!("{name}.map"));
        std::fs::write(&p, bytes).unwrap();
        p
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app_with(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app
}

fn move_flag(app: &mut App, x: f32) {
    app.session
        .apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "position",
            Value::Vec3([x, 3.0, 3.0]),
        ))
        .unwrap();
}

fn flag_x(app: &App) -> f32 {
    app.session
        .scenario()
        .unwrap()
        .get(BlockKind::NetgameFlags, 0, "position")
        .unwrap()
        .as_vec3()
        .unwrap()[0]
}

#[test]
fn a_loose_map_opens_straight_into_the_scenario_tool() {
    let t = Temp::new("loose");
    let p = t.map("example", &fixture::fixture());
    let mut app = app_with(&t);
    assert_eq!(app.title(), "h2-studio");
    app.dispatch(Action::OpenSource(p));
    assert_eq!(app.maps(), ["example".to_string()]);
    assert_eq!(app.loaded_map(), Some("example"));
    assert!(app.session.is_open() && !app.is_dirty());
    assert_eq!(app.title(), "example - h2-studio");
    assert!(matches!(app.status(), Some(Status::Info(m)) if m.starts_with("opened example")));
    move_flag(&mut app, 9.0);
    assert_eq!(app.title(), "example * - h2-studio");
}

#[test]
fn a_folder_lists_its_maps_and_opens_the_picked_one() {
    let t = Temp::new("folder");
    t.map("beta", &fixture::fixture());
    t.map("alpha", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(t.0.clone()));
    assert_eq!(app.maps(), ["alpha".to_string(), "beta".to_string()]);
    assert_eq!(app.loaded_map(), None, "nothing is picked for the user");
    app.dispatch(Action::SelectMap("beta".into()));
    assert_eq!(app.loaded_map(), Some("beta"));
    app.dispatch(Action::SelectMap("nope".into()));
    assert!(matches!(app.status(), Some(Status::Error(_))));
    assert_eq!(app.loaded_map(), Some("beta"));
}

#[test]
fn a_bad_source_is_an_error_in_the_log() {
    let t = Temp::new("bad");
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(t.0.join("missing.iso")));
    assert!(matches!(app.status(), Some(Status::Error(_))));
    assert!(app.source().is_none());
    app.dispatch(Action::SelectMap("x".into()));
    assert!(matches!(app.status(), Some(Status::Error(m)) if m.contains("open a disc")));
}

#[test]
fn unsaved_edits_hold_switching_until_answered() {
    let t = Temp::new("guard");
    t.map("alpha", &fixture::fixture());
    t.map("beta", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(t.0.clone()));
    app.dispatch(Action::SelectMap("alpha".into()));
    move_flag(&mut app, 9.0);

    // Re-picking the open map is not a switch.
    app.dispatch(Action::SelectMap("alpha".into()));
    assert!(app.pending().is_none());

    app.dispatch(Action::SelectMap("beta".into()));
    assert_eq!(app.pending(), Some(&Action::SelectMap("beta".into())));
    app.resolve(Answer::Cancel);
    assert!(app.pending().is_none());
    assert_eq!(app.loaded_map(), Some("alpha"));
    assert!(app.is_dirty());

    app.dispatch(Action::SelectMap("beta".into()));
    app.resolve(Answer::Discard);
    assert_eq!(app.loaded_map(), Some("beta"));
    assert!(!app.is_dirty());

    // Quitting is guarded the same way; without edits it just quits.
    move_flag(&mut app, 4.0);
    app.dispatch(Action::Quit);
    assert!(!app.should_quit());
    app.resolve(Answer::Discard);
    assert!(app.should_quit());
    let mut clean = app_with(&t);
    clean.dispatch(Action::Quit);
    assert!(clean.should_quit());
}

#[test]
fn saving_from_the_prompt_writes_the_map_then_goes_on() {
    let t = Temp::new("saveprompt");
    t.map("alpha", &fixture::fixture());
    t.map("beta", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(t.0.clone()));
    app.dispatch(Action::SelectMap("alpha".into()));
    move_flag(&mut app, 9.0);
    app.dispatch(Action::SelectMap("beta".into()));
    let out = t.0.join("out").join("edited.map");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();

    // A failing save keeps the question open.
    app.resolve(Answer::SaveAs(t.0.join("no-such-dir").join("x.map")));
    assert!(app.pending().is_some());
    assert_eq!(app.loaded_map(), Some("alpha"));

    app.resolve(Answer::SaveAs(out.clone()));
    assert!(app.pending().is_none());
    assert!(out.is_file());
    assert_eq!(app.loaded_map(), Some("beta"));

    // The saved file reads back with the edit.
    let mut check = app_with(&t);
    check.dispatch(Action::OpenSource(out));
    assert!((flag_x(&check) - 9.0).abs() < 1e-6);
}

#[test]
fn undo_and_redo_go_through_actions() {
    let t = Temp::new("undo");
    let p = t.map("example", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(p));
    let x0 = flag_x(&app);
    app.dispatch(Action::Undo);
    assert!(matches!(app.status(), Some(Status::Info(m)) if m == "nothing to undo"));
    move_flag(&mut app, x0 + 5.0);
    app.dispatch(Action::Undo);
    assert_eq!(flag_x(&app), x0);
    assert!(!app.is_dirty());
    app.dispatch(Action::Redo);
    assert_eq!(flag_x(&app), x0 + 5.0);
    app.dispatch(Action::Redo);
    assert!(matches!(app.status(), Some(Status::Info(m)) if m == "nothing to redo"));
}

#[test]
fn delete_duplicate_and_add_act_on_the_selection() {
    let t = Temp::new("dupdel");
    let p = t.map("example", &fixture::fixture_at_end());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(p));
    app.dispatch(Action::Delete);
    assert!(matches!(app.status(), Some(Status::Error(m)) if m.contains("select")));

    let weapons = |app: &App| {
        app.session
            .scenario()
            .unwrap()
            .block(BlockKind::Weapons)
            .len()
    };
    let n = weapons(&app);
    app.dispatch(Action::Select(Some((BlockKind::Weapons, 0))));
    // Full block, growth off: refused with the reason.
    app.dispatch(Action::Duplicate);
    assert!(matches!(app.status(), Some(Status::Error(_))));
    assert_eq!(weapons(&app), n);

    app.dispatch(Action::Delete);
    assert_eq!(weapons(&app), n - 1);
    app.dispatch(Action::Select(Some((BlockKind::Weapons, 0))));
    app.dispatch(Action::Duplicate);
    assert_eq!(weapons(&app), n);
    assert_eq!(app.session.selected(), Some((BlockKind::Weapons, n - 1)));

    app.dispatch(Action::SetAllowGrowth(true));
    app.dispatch(Action::Add(BlockKind::Weapons));
    assert_eq!(weapons(&app), n + 1);
    // Each was one step.
    for _ in 0..3 {
        app.dispatch(Action::Undo);
    }
    assert_eq!(weapons(&app), n);
    assert!(!app.is_dirty());
}

#[test]
fn a_gizmo_drag_is_one_undo_step() {
    let t = Temp::new("gizmo");
    let p = t.map("example", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(p));
    app.dispatch(Action::Select(Some((BlockKind::NetgameFlags, 0))));
    let start = app.session.selected_transform().unwrap();
    for k in 1..=5 {
        let mut pos = start.position;
        pos[0] += k as f32;
        let mut ypr = start.ypr();
        ypr[0] += 0.1 * k as f32;
        app.session.gesture_set(pos, ypr).unwrap();
    }
    app.session.end_gesture();
    let moved = app.session.selected_transform().unwrap();
    assert!((moved.position[0] - (start.position[0] + 5.0)).abs() < 1e-5);
    assert!((moved.ypr()[0] - (start.ypr()[0] + 0.5)).abs() < 1e-5);
    app.dispatch(Action::Undo);
    assert_eq!(app.session.selected_transform().unwrap(), start);
    assert!(!app.session.can_undo(), "the whole drag was one step");
    // A second gesture is a second step.
    app.dispatch(Action::Redo);
    let mut pos = moved.position;
    pos[1] += 1.0;
    app.session.gesture_set(pos, moved.ypr()).unwrap();
    app.session.end_gesture();
    app.dispatch(Action::Undo);
    assert_eq!(app.session.selected_transform().unwrap(), moved);
}

#[test]
fn packages_save_and_open_through_actions() {
    let t = Temp::new("pkg");
    let p = t.map("example", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(p));
    move_flag(&mut app, 7.0);
    app.open_package_form();
    let mut form = app.package_form.clone().unwrap();
    assert_eq!(form.dir, "example-edit");
    form.name = "Studio test".into();
    form.dir = "example-studio".into();
    app.dispatch(Action::SavePackage(form));
    assert!(app.package_form.is_none(), "the form closes on success");
    assert!(!app.is_dirty());
    assert!(t.0.join("data/maps/example-studio").is_dir());
    move_flag(&mut app, 1.0);
    app.dispatch(Action::Undo);
    app.dispatch(Action::OpenPackage("example-studio".into()));
    assert!(matches!(app.status(), Some(Status::Info(m)) if m.contains("opened package")));
    assert!((flag_x(&app) - 7.0).abs() < 1e-6);
}

#[test]
fn the_log_is_bounded_and_suggests_save_names() {
    let mut app = App::new();
    for i in 0..LOG_LIMIT + 10 {
        app.info(format!("line {i}"));
    }
    assert_eq!(app.log().len(), LOG_LIMIT);
    assert_eq!(app.log()[0].status, Status::Info("line 10".into()));
    assert_eq!(app.suggested_save_name(), "untitled.map");
    assert!(ToolKind::Scenario.available() && !ToolKind::Playlists.available());
}
