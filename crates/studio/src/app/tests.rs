//! App state and commands without a window, on the scenario crate's
//! invented cache (no game data).
use super::*;
use scenario_edit::{Command, Value, fixture};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2-editor-{tag}-{}", std::process::id()));
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
    assert_eq!(app.title(), "h2-editor");
    app.dispatch(Action::OpenSource(p));
    assert_eq!(app.maps(), ["example".to_string()]);
    assert_eq!(app.loaded_map(), Some("example"));
    assert!(app.session.is_open() && !app.is_dirty());
    assert_eq!(app.title(), "example - h2-editor");
    assert!(matches!(app.status(), Some(Status::Info(m)) if m.starts_with("opened example")));
    move_flag(&mut app, 9.0);
    assert_eq!(app.title(), "example * - h2-editor");
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
}

// ---------- game variants ----------

use variants::save::{CONTENT_TYPES, SaveStore, XbeNames};

/// A variant editor over the fallback schema with a save store under the
/// temp dir and made-up XBE names (so new variants can be saved).
fn with_variant_store(app: &mut App, t: &Temp) {
    let names = XbeNames {
        file_names: (0..CONTENT_TYPES).map(|i| format!("t{i}")).collect(),
        labels: (0..CONTENT_TYPES).map(|i| format!("L{i}")).collect(),
    };
    let store = SaveStore::new(t.0.join("saves"), Some(names));
    app.variants = Some(VariantEditor::new(
        variants::Schema::fallback(),
        Some(store),
    ));
    app.variants_for = Some(None);
}

#[test]
fn the_variant_tool_builds_from_the_fallback_and_undoes_through_actions() {
    let t = Temp::new("variants");
    let mut app = app_with(&t);
    app.dispatch(Action::SetTool(ToolKind::Variants));
    assert_eq!(app.tool, ToolKind::Variants);
    assert!(app.variants.is_some());
    assert!(
        app.log()
            .iter()
            .any(|l| l.status.text().contains("built-in setting model"))
    );
    app.dispatch(Action::NewVariant(GameType::Slayer));
    assert!(
        app.tool_dirty(ToolKind::Variants),
        "a never-saved variant is unsaved"
    );
    let start = app.variants.as_ref().unwrap().current().unwrap().name();
    // Typing a name is one undo step.
    for n in ["M", "My", "Mine"] {
        app.dispatch(Action::SetVariantName(n.into()));
    }
    assert_eq!(app.title(), "Mine * - h2-editor");
    assert!(app.can_undo());
    app.dispatch(Action::Undo);
    assert_eq!(
        app.variants.as_ref().unwrap().current().unwrap().name(),
        start
    );
    assert!(!app.can_undo() && app.can_redo());
    app.dispatch(Action::Redo);
    assert_eq!(
        app.variants.as_ref().unwrap().current().unwrap().name(),
        "Mine"
    );
    // A setting change through its action, then revert.
    let e = app.variants.as_ref().unwrap();
    let v = e.current().unwrap();
    // An on/off setting on the type's pages, toggled.
    let (id, was, other) = e
        .schema()
        .pages_for(v.game_type())
        .iter()
        .flat_map(|p| p.settings.clone())
        .find_map(|id| {
            use variants::record::Kind;
            let kind = variants::record::field(id)?.kind;
            let on_off = matches!(kind, Kind::Bit16(_) | Kind::Bit32(_) | Kind::Bool8);
            let was = v.get(id)?;
            on_off.then_some((id, was, 1 - was))
        })
        .expect("an on/off setting");
    app.dispatch(Action::SetSetting(id, other));
    assert_eq!(
        app.variants.as_ref().unwrap().current().unwrap().get(id),
        Some(other)
    );
    app.dispatch(Action::Undo);
    assert_eq!(
        app.variants.as_ref().unwrap().current().unwrap().get(id),
        Some(was)
    );
}

#[test]
fn unsaved_variants_hold_a_switch_and_save_from_the_prompt() {
    let t = Temp::new("variants-guard");
    let mut app = app_with(&t);
    with_variant_store(&mut app, &t);
    app.dispatch(Action::SetTool(ToolKind::Variants));
    app.dispatch(Action::NewVariant(GameType::Slayer));
    app.dispatch(Action::SetVariantName("Keep me".into()));
    // Another new variant would drop it: held.
    app.dispatch(Action::NewVariant(GameType::Ctf));
    assert_eq!(app.pending(), Some(&Action::NewVariant(GameType::Ctf)));
    assert_eq!(app.pending_tool(), Some(ToolKind::Variants));
    app.resolve(Answer::Cancel);
    assert!(app.pending().is_none());
    app.dispatch(Action::NewVariant(GameType::Ctf));
    app.resolve(Answer::Save);
    assert!(app.pending().is_none());
    let e = app.variants.as_ref().unwrap();
    assert_eq!(e.saved().len(), 1, "saved first");
    assert_eq!(e.saved()[0].contents.variant.name(), "Keep me");
    assert_eq!(
        e.current().unwrap().game_type(),
        GameType::Ctf,
        "then went on"
    );
    // Discarding drops the new one.
    app.dispatch(Action::OpenVariant(Origin::Saved(e.saved()[0].dir.clone())));
    assert!(app.pending().is_some());
    app.resolve(Answer::Discard);
    let e = app.variants.as_ref().unwrap();
    assert_eq!(e.current().unwrap().name(), "Keep me");
    assert!(!app.tool_dirty(ToolKind::Variants));
    // Delete the save.
    app.dispatch(Action::DeleteVariant);
    assert!(app.variants.as_ref().unwrap().saved().is_empty());
}

#[test]
fn variants_export_and_import_as_portable_files() {
    let t = Temp::new("variants-portable");
    let mut app = app_with(&t);
    with_variant_store(&mut app, &t);
    app.dispatch(Action::NewVariant(GameType::King));
    app.dispatch(Action::SetVariantName("Shared".into()));
    let file = t.0.join("shared.h2variant");
    app.dispatch(Action::ExportVariant(file.clone()));
    assert!(file.is_file());
    app.dispatch(Action::SaveVariant);
    assert!(!app.tool_dirty(ToolKind::Variants));
    app.dispatch(Action::ImportVariantFile(file));
    let e = app.variants.as_ref().unwrap();
    assert_eq!(e.origin(), Some(&Origin::New));
    assert_eq!(e.current().unwrap().name(), "Shared");
    assert_eq!(e.current().unwrap().game_type(), GameType::King);
    // Another import would drop the unsaved one: held; garbage is then an
    // error.
    app.dispatch(Action::ImportVariantText("not a variant".into()));
    assert_eq!(app.pending_tool(), Some(ToolKind::Variants));
    app.resolve(Answer::Discard);
    assert!(app.status().unwrap().is_error());
}

// ---------- playlists ----------

fn hopper_file(t: &Temp) -> PathBuf {
    let p = t.0.join("multiplayer_hopper.dat");
    std::fs::write(&p, hopper_edit::fixture::sample().build()).unwrap();
    p
}

#[test]
fn playlists_open_edit_undo_and_save_through_actions() {
    let t = Temp::new("playlists");
    let mut app = app_with(&t);
    let path = hopper_file(&t);
    app.dispatch(Action::OpenHopper(path.clone()));
    assert_eq!(app.tool, ToolKind::Playlists);
    assert!(app.hoppers.is_open() && !app.is_dirty());
    assert_eq!(app.title(), "multiplayer_hopper.dat - h2-editor");
    app.hoppers.edit(|h| h[0].title = "Edited".into());
    assert!(app.tool_dirty(ToolKind::Playlists));
    app.dispatch(Action::Undo);
    assert!(!app.is_dirty());
    app.dispatch(Action::Redo);
    app.dispatch(Action::SaveHopper);
    assert!(!app.is_dirty());
    let (back, _) = hopper_edit::read_file(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(back.text(0).unwrap().title, "Edited");

    // A new file over unsaved edits asks; Save as writes them first.
    app.hoppers.edit(|h| h[1].title = "Second".into());
    app.dispatch(Action::NewHopper);
    assert_eq!(app.pending_tool(), Some(ToolKind::Playlists));
    let other = t.0.join("other.dat");
    app.resolve(Answer::SaveAs(other.clone()));
    assert!(app.pending().is_none());
    let (back, _) = hopper_edit::read_file(&std::fs::read(&other).unwrap()).unwrap();
    assert_eq!(back.text(1).unwrap().title, "Second");
    assert!(
        app.hoppers
            .hoppers()
            .unwrap()
            .iter()
            .all(|h| h.records.is_empty()),
        "then new"
    );
    // Saving a file with no path yet is refused with a reason.
    app.dispatch(Action::SaveHopper);
    assert!(app.status().unwrap().is_error());
}

#[test]
fn playlist_entries_take_and_hand_over_variants() {
    let t = Temp::new("playlists-variants");
    let mut app = app_with(&t);
    with_variant_store(&mut app, &t);
    app.dispatch(Action::OpenHopper(hopper_file(&t)));
    let v = hopper_edit::fixture::good_variant("Linked");
    app.dispatch(Action::SetEntryVariant(0, 1, Box::new(v.clone())));
    assert_eq!(app.hoppers.entry_variant(0, 1).unwrap(), v);
    app.dispatch(Action::Undo);
    assert_ne!(app.hoppers.entry_variant(0, 1).unwrap(), v, "one undo step");
    app.dispatch(Action::Redo);
    // A variant the game would reject is refused and logged.
    let bad = GameVariant::blank(GameType::Territories);
    app.dispatch(Action::SetEntryVariant(0, 0, Box::new(bad)));
    assert!(app.status().unwrap().is_error());
    // Edit in the variant tool: a new, unsaved copy there.
    app.dispatch(Action::EditEntryVariant(0, 1));
    assert_eq!(app.tool, ToolKind::Variants);
    let e = app.variants.as_ref().unwrap();
    assert_eq!(e.origin(), Some(&Origin::New));
    assert_eq!(e.current().unwrap().name(), "Linked");
    assert!(!e.current().unwrap().builtin_flag());
}

#[test]
fn quitting_asks_about_each_tool_with_unsaved_edits() {
    let t = Temp::new("quit-all");
    let mut app = app_with(&t);
    with_variant_store(&mut app, &t);
    app.dispatch(Action::NewDefaultHopper);
    assert_eq!(app.tool, ToolKind::Playlists);
    app.hoppers.edit(|h| h[0].title = "Changed".into());
    app.dispatch(Action::NewVariant(GameType::Slayer));
    app.dispatch(Action::Quit);
    assert_eq!(app.pending_tool(), Some(ToolKind::Variants));
    app.resolve(Answer::Discard);
    assert_eq!(
        app.pending_tool(),
        Some(ToolKind::Playlists),
        "then the next"
    );
    assert!(!app.should_quit());
    app.resolve(Answer::Discard);
    assert!(app.pending().is_none() && app.should_quit());
}

// ---------- packages and Halo 1 ----------

/// A converted Halo 1 map as the import writes it: one package holding
/// both sandbox variants (each a whole map, here the invented cache), and
/// an `[origin]` per variant.
fn converted_package(t: &Temp, source_sha256: &str) {
    let dir = t.0.join("data/maps/h1_bloodgulch");
    std::fs::create_dir_all(dir.join("h1")).unwrap();
    std::fs::create_dir_all(dir.join("h2")).unwrap();
    std::fs::write(dir.join("h1/h1_bloodgulch.map"), fixture::fixture()).unwrap();
    std::fs::write(dir.join("h2/h1_bloodgulch.map"), fixture::fixture()).unwrap();
    std::fs::write(
        dir.join("manifest.toml"),
        format!(
            r#"[package]
format = 1
name = "Blood Gulch"

[base]
map = "h1_bloodgulch"
derived = true

[[variant]]
file = "h1/h1_bloodgulch.map"
origin = {{ game = "halo1", map = "bloodgulch", sandbox = "h1", source_sha256 = "{source_sha256}" }}

[[variant]]
file = "h2/h1_bloodgulch.map"
origin = {{ game = "halo1", map = "bloodgulch", sandbox = "h2", source_sha256 = "{source_sha256}" }}
"#
        ),
    )
    .unwrap();
}

fn labels(app: &App) -> Vec<String> {
    app.packages().iter().map(|e| e.label()).collect()
}

#[test]
fn the_maps_list_groups_user_packages_and_both_halo1_variants() {
    let t = Temp::new("pkglist");
    let p = t.map("example", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(p));
    move_flag(&mut app, 7.0);
    let mut form = app.session.form_defaults();
    form.name = "Flag moved".into();
    app.dispatch(Action::SavePackage(form));
    converted_package(&t, "00");
    app.refresh_packages();
    assert_eq!(
        labels(&app),
        [
            "Flag moved",
            "Blood Gulch (H1 sandbox)",
            "Blood Gulch (H2 sandbox)"
        ]
    );
    let own: Vec<_> = app
        .packages()
        .iter()
        .filter(|e| e.halo1.is_none())
        .collect();
    assert_eq!(own.len(), 1);
    assert!(!own[0].derived);
}

#[test]
fn a_user_package_opens_over_its_disc_map_from_the_list() {
    let t = Temp::new("pkgopen");
    let p = t.map("example", &fixture::fixture());
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(p));
    move_flag(&mut app, 7.0);
    let form = app.session.form_defaults();
    app.dispatch(Action::SavePackage(form));
    assert_eq!(app.opened_package(), Some("example-edit"));
    // Back to the disc's map, then the package from the list.
    app.dispatch(Action::SelectMap("example".into()));
    assert_eq!(app.opened_package(), None);
    assert!((flag_x(&app) - 7.0).abs() > 1e-6, "the disc map, unedited");
    app.dispatch(Action::OpenPackageEntry("example-edit".into()));
    assert_eq!(app.opened_package(), Some("example-edit"));
    assert_eq!(app.session.package_dir(), Some("example-edit"));
    assert!((flag_x(&app) - 7.0).abs() < 1e-6);
}

#[test]
fn a_converted_variant_opens_and_saves_as_a_new_derived_package() {
    let t = Temp::new("h1pkg");
    let mut app = app_with(&t);
    converted_package(&t, "00");
    app.refresh_packages();
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H2".into()));
    assert_eq!(app.loaded_map(), Some("h1_bloodgulch"));
    assert!(app.session.is_derived());
    // Its Halo 1 source is not at hand: the overlay says why.
    let c = app.compare().unwrap();
    assert_eq!(c.origin.map, "bloodgulch");
    assert!(c.view.is_err());
    assert!(app.overlay().is_none());

    move_flag(&mut app, 9.0);
    let form = app.session.form_defaults();
    assert_eq!(
        form.dir, "h1_bloodgulch-edit",
        "not the import's own package"
    );
    app.dispatch(Action::SavePackage(form));
    assert!(!app.is_dirty());
    // The new package is a derived map the resolver reads: the whole
    // converted map, then the edit.
    let pkgs = scenario_edit::package::packages_for(&app.session.maps_dir(), "h1_bloodgulch");
    let saved = pkgs
        .iter()
        .find(|p| p.dir_name() == "h1_bloodgulch-edit")
        .unwrap();
    assert!(saved.manifest.base.derived);
    assert_eq!(saved.manifest.patches.len(), 2);
    // The import's package is untouched.
    let import = std::fs::read_to_string(t.0.join("data/maps/h1_bloodgulch/manifest.toml"));
    assert!(import.unwrap().contains("[[variant]]"));
    // Reopened from the list, the edit is there.
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H1".into()));
    assert!((flag_x(&app) - 9.0).abs() > 1e-6);
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch-edit".into()));
    assert!((flag_x(&app) - 9.0).abs() < 1e-6);
    assert_eq!(app.session.package_dir(), Some("h1_bloodgulch-edit"));
}

#[test]
fn unsaved_edits_hold_opening_a_package() {
    let t = Temp::new("pkgguard");
    let mut app = app_with(&t);
    converted_package(&t, "00");
    app.refresh_packages();
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H1".into()));
    move_flag(&mut app, 4.0);
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H2".into()));
    assert_eq!(app.pending_tool(), Some(ToolKind::Scenario));
    app.resolve(Answer::Cancel);
    assert_eq!(app.opened_package(), Some("h1_bloodgulch#H1"));
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H2".into()));
    app.resolve(Answer::Discard);
    assert_eq!(app.opened_package(), Some("h1_bloodgulch#H2"));
    assert!(!app.is_dirty());
}

/// A folder holding one invented Halo 1 map, `bloodgulch`.
fn halo1_folder(t: &Temp) -> (PathBuf, String) {
    let dir = t.0.join("h1maps");
    std::fs::create_dir_all(&dir).unwrap();
    let map = h1map::testing::tiny_map();
    std::fs::write(dir.join("bloodgulch.map"), &map).unwrap();
    (dir, content::Sha256::of(&map).to_string())
}

#[test]
fn a_halo1_source_lists_its_maps_and_shows_one_read_only() {
    let t = Temp::new("h1src");
    let iso = t.0.join("halo1.iso");
    std::fs::write(&iso, h1map::testing::campaign_disc()).unwrap();
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(iso));
    assert!(app.halo1_maps().contains(&"bloodgulch".to_string()));
    assert!(
        app.source().is_none(),
        "a Halo 1 disc is not a Halo 2 source"
    );
    app.dispatch(Action::SelectHalo1Map("bloodgulch".into()));
    assert_eq!(app.loaded_halo1(), Some("bloodgulch"));
    assert!(app.halo1_view().is_some());
    assert!(app.session.reference().is_some());
    assert!(!app.session.is_open(), "nothing to edit");
    assert!(!app.is_dirty());
    // Opening a Halo 2 map leaves it.
    let p = t.map("example", &fixture::fixture());
    app.dispatch(Action::OpenSource(p));
    assert!(app.halo1_view().is_none());
    assert!(app.session.reference().is_none());
    assert_eq!(app.halo1_maps().len(), 12, "the Halo 1 source stays listed");
}

#[test]
fn halo1_placements_become_markers_in_their_blocks() {
    use editor_core::halo1::{H1Kind, H1Placement, H1Tri, H1View};
    let v = H1View {
        name: "x".into(),
        tris: vec![H1Tri {
            p: [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            n: [0.0, 0.0, 1.0],
        }],
        placements: vec![H1Placement {
            kind: H1Kind::NetgameFlag,
            index: 2,
            position: [1.0, 2.0, 3.0],
            facing: std::f32::consts::FRAC_PI_2,
            label: "CTF flag 2".into(),
            fields: Vec::new(),
        }],
        notes: Vec::new(),
    };
    let m = library_markers(&v);
    assert_eq!(m[0].block, BlockKind::NetgameFlags);
    assert_eq!(m[0].index, 2);
    assert!((m[0].axes[0][1] - 1.0).abs() < 1e-6, "facing turns forward");
    assert_eq!(kind_of(BlockKind::NetgameFlags), Some(H1Kind::NetgameFlag));
    assert_eq!(kind_of(BlockKind::Scenery), None);
    let mut app = App::new();
    app.session.open_reference(scenario_edit::Reference {
        name: v.name.clone(),
        world: library_tris(&v),
        markers: m,
    });
    app.dispatch(Action::Select(Some((BlockKind::NetgameFlags, 2))));
    app.dispatch(Action::FocusSelected);
    assert_eq!(app.session.camera().target, [1.0, 2.0, 3.0]);
    assert!(app.session.selected_transform().is_none(), "no gizmo");
}

#[test]
fn a_converted_map_finds_its_halo1_source_by_hash_for_the_overlay() {
    let t = Temp::new("h1cmp");
    let (dir, sha) = halo1_folder(&t);
    let mut app = app_with(&t);
    app.dispatch(Action::OpenSource(dir));
    converted_package(&t, &sha);
    app.refresh_packages();
    let rev = app.overlay_rev();
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H1".into()));
    let c = app.compare().unwrap();
    assert!(c.view.is_ok(), "{:?}", c.view);
    assert!(app.overlay().is_some());
    assert!(app.overlay_rev() > rev);
    app.dispatch(Action::SetOverlay(false));
    assert!(app.overlay().is_none());
    // A different source hash is not this map's source.
    converted_package(&t, "0000");
    app.refresh_packages();
    app.dispatch(Action::OpenPackageEntry("h1_bloodgulch#H2".into()));
    let c = app.compare().unwrap();
    assert!(c.view.as_ref().unwrap_err().contains("another SHA-256"));
}
