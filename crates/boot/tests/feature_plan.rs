//! The plan boot builds with every migrated feature on (Live telemetry, the
//! Video screen's list class, the level boot's loaded mark, our strings, lockstep's tick hash, the Rust
//! locale lookup on by default): it builds
//! with no conflict, and this is its text (the `feature plan:` lines boot
//! logs with `H2_FEATURE_PLAN=1`). Before the registry, telemetry and lockstep could not both hook
//! the tick, and the level boot had to chain onto telemetry by hand.

use boot::telemetry::{TelemetryConfig, TelemetrySinkConfig};

#[test]
fn the_full_plan_builds_and_reads_as_expected() {
    let path = std::env::temp_dir().join(format!("h2-plan-{}.jsonl", std::process::id()));
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let config = TelemetryConfig {
        enabled: true,
        sink: TelemetrySinkConfig::File(path.clone()),
    };
    assert!(boot::telemetry::install(&mut r, &config).unwrap().is_some());
    // The Video screen's list class, as `layout_glue::install` adds it.
    let store = std::sync::Arc::new(settings::Store::defaults(boot::setting_defs::ALL));
    let model = std::sync::Arc::new(
        boot::video_screen::model(store, boot::video_screen::Switches::ALL).unwrap(),
    );
    let video = boot::list_class::ListClass::new(
        "video-screen",
        boot::video_screen::SCREEN,
        std::sync::Arc::clone(&model),
    );
    r.add(&video).unwrap();
    // Its row on the Settings list, as data.
    r.add(&boot::retail_rows::Extension {
        id: "settings-rows",
        title: "Our rows on the Settings list",
        list: boot::ui::List::Settings,
        text: &feature::points::SETTINGS_ROW_TEXT,
        chosen: &feature::points::SETTINGS_ROW_CHOSEN,
        label_only: true,
        model: std::sync::Arc::clone(&model),
    })
    .unwrap();
    r.add(&boot::layout_glue::StringsFeature).unwrap();
    r.add(&boot::level_boot::LoadedMark(None)).unwrap();
    r.add(&lockstep::TickObserver(lockstep::Recorder::new(None)))
        .unwrap();
    r.add(&feature::leaves::EngineLeaves).unwrap();
    // The aim assist's map-load hook and its per-player multiplayer hooks
    // (docs/game/aim-assist.md).
    let settings = settings::Store::defaults(boot::setting_defs::ALL);
    r.add(&boot::aim_assist::AimAssist::new(&settings)).unwrap();
    let plan = r.build(&feature::Base).unwrap();
    let _ = std::fs::remove_file(path);
    assert_eq!(
        plan.text(),
        "\
feature live-telemetry: on
feature video-screen: on
feature settings-rows: on
feature ui-strings: on
feature level-boot: on
feature lockstep: on
feature engine-leaves: on
feature aim-assist: on
0x000d9640 DAMAGE_REPORT: observe_before [live-telemetry:hits] -> original
0x00104150 WEAPON_FIRE: observe_before [live-telemetry:shots] -> original
0x00137fe0 GAME_TICK: original -> observe_after [live-telemetry:sample, lockstep:hash]
0x001389c0 GAME_END: original -> observe_after [live-telemetry:end]
0x0015ce70 PLAYER_KILLED: observe_before [live-telemetry:kill] -> original
0x00185be0 PLAYER_CONTROL_UPDATE: before [aim-assist:touch player's aim assist (open)] -> original -> after [aim-assist:touch player's aim assist (close)]
0x0018f000 GAME_ACTIVATE: before [aim-assist:weapon aim assist] -> original -> observe_after [level-boot:loaded mark, live-telemetry:match start]
0x001a02d0 LOCALE_FIND: implement engine-leaves:locale_string_id_find
0x001a0310 LOCALE_EXISTS: implement engine-leaves:locale_string_id_exists
0x001a50a0 AIM_ASSIST_PARAMS: original -> after [aim-assist:touch player's weapon cone]
0x002b1d6e SETTINGS_ROW_TEXT: before [settings-rows:our rows' text] -> original
0x002b1da9 SETTINGS_ROW_CHOSEN: before [settings-rows:our rows' press] -> original
0xa0f01000 entry video-screen:loader
0xa0f01010 entry video-screen:row text
0xa0f01020 entry video-screen:row chosen
0xa0f01030 entry video-screen:set up pane
0xa0f01040 entry video-screen:loader slot
locale source ui-strings:our strings (served by engine-leaves)
warning: LOCALE_FIND (0x001a02d0) is hot: hooking it leaves the direct-call fast path
"
    );
    let installed = feature::Installed {
        placed: vec![*plan.chains.keys().next().unwrap(), plan.entries[0].addr],
        notes: vec!["aim-assist: off: PLAYER_CONTROL_UPDATE has no generated original".into()],
        off: vec![(
            "aim-assist",
            "PLAYER_CONTROL_UPDATE has no generated original".into(),
        )],
    };
    assert_eq!(
        plan.startup_text(&installed, false),
        "7 features on, 1 off (aim-assist), 1 hooked points, 1 entries; H2_FEATURE_PLAN=1 lists them\n\
         aim-assist: off: PLAYER_CONTROL_UPDATE has no generated original\n"
    );
    assert_eq!(
        plan.startup_text(&installed, true),
        format!("{}{}\n", plan.text(), installed.notes[0])
    );
    // Shadows only where something runs after the original.
    let shadows = plan.chains.values().filter(|c| c.shadow.is_some()).count();
    assert_eq!(shadows, 5);
}

/// With the Rust locale lookup switched off, our strings go back to one
/// bridge hook on the original lookup.
#[test]
fn without_the_rust_lookup_one_bridge_hook_serves_our_strings() {
    let mut r = feature::Registry::new(feature::Switches::parse("impl.engine.locale=off").unwrap());
    r.add(&boot::layout_glue::StringsFeature).unwrap();
    r.add(&feature::leaves::EngineLeaves).unwrap();
    let text = r.build(&feature::Base).unwrap().text();
    assert!(
        text.contains("0x001a02d0 LOCALE_FIND: before [locale:sources] -> original\n"),
        "{text}"
    );
    assert!(text.contains("locale source ui-strings:our strings (served by the locale bridge)\n"));
}

/// The agent API (H2_AGENT) shares the tick with telemetry and lockstep,
/// after both, so a pause holds the tick only once they've seen it; and
/// it watches the cutscene start and stop (docs/runtime/agent-api.md).
#[test]
fn the_agent_observes_the_tick_last_and_the_cutscenes() {
    let path = std::env::temp_dir().join(format!("h2-plan-agent-{}.jsonl", std::process::id()));
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let config = TelemetryConfig {
        enabled: true,
        sink: TelemetrySinkConfig::File(path.clone()),
    };
    // The agent added first still runs last.
    r.add(&agent::AgentFeature(agent::Agent::new())).unwrap();
    assert!(boot::telemetry::install(&mut r, &config).unwrap().is_some());
    r.add(&lockstep::TickObserver(lockstep::Recorder::new(None)))
        .unwrap();
    let text = r.build(&feature::Base).unwrap().text();
    let _ = std::fs::remove_file(path);
    assert!(text.contains("feature agent: on\n"), "{text}");
    assert!(
        text.contains("GAME_TICK: original -> observe_after [live-telemetry:sample, lockstep:hash, agent:serve]\n"),
        "{text}"
    );
    assert!(
        text.contains("CINEMATIC_START: original -> observe_after [agent:cinematic-start]\n"),
        "{text}"
    );
    assert!(
        text.contains("CINEMATIC_STOP: original -> observe_after [agent:cinematic-stop]\n"),
        "{text}"
    );
}

/// `H2_FEATURES=-agent` leaves every hook out.
#[test]
fn the_agent_can_be_switched_off() {
    let mut r = feature::Registry::new(feature::Switches::parse("-agent").unwrap());
    assert!(!r.add(&agent::AgentFeature(agent::Agent::new())).unwrap());
    let plan = r.build(&feature::Base).unwrap();
    let text = plan.text();
    assert_eq!(
        plan.startup_text(&feature::Installed::default(), false),
        "0 features on, 1 off (agent), 0 hooked points, 0 entries; H2_FEATURE_PLAN=1 lists them\n"
    );
    assert!(text.contains("feature agent: off"), "{text}");
    assert!(!text.contains("[agent:"), "{text}");
}

/// The precision item swap (converted Halo 1 maps) and the aim assist both
/// change weapons before GAME_ACTIVATE; the plan orders them instead of
/// refusing to start the game.
#[test]
fn precision_and_aim_assist_share_the_game_activation() {
    let mut r = feature::Registry::new(feature::Switches::defaults());
    let packages = std::env::temp_dir().join(format!("h2-plan-pkgs-{}", std::process::id()));
    assert!(boot::precision::install(&mut r, &packages).unwrap());
    let settings = settings::Store::defaults(boot::setting_defs::ALL);
    r.add(&boot::aim_assist::AimAssist::new(&settings)).unwrap();
    let plan = r.build(&feature::Base).expect("the plan builds");
    let line = plan
        .text()
        .lines()
        .find(|l| l.contains("GAME_ACTIVATE"))
        .unwrap()
        .to_owned();
    assert!(line.contains("h1-precision:precision item swap"), "{line}");
    assert!(line.contains("aim-assist:weapon aim assist"), "{line}");
}
