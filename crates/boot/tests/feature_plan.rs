//! The plan boot builds with every migrated feature on (Live telemetry, the
//! Video screen's list class, the level boot's loaded mark, our strings, lockstep's tick hash, the Rust
//! locale lookup on by default): it builds
//! with no conflict, and this is its text (the `feature plan:` lines boot
//! logs). Before the registry, telemetry and lockstep could not both hook
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
    let model = boot::video_screen::model(store, boot::video_screen::Switches::ALL).unwrap();
    let video = boot::list_class::ListClass::new(
        "video-screen",
        boot::video_screen::SCREEN,
        std::sync::Arc::new(model),
    );
    r.add(&video).unwrap();
    r.add(&boot::layout_glue::StringsFeature).unwrap();
    r.add(&boot::level_boot::LoadedMark(None)).unwrap();
    r.add(&lockstep::TickObserver(lockstep::Recorder::new(None)))
        .unwrap();
    r.add(&feature::leaves::EngineLeaves).unwrap();
    let plan = r.build(&feature::Base).unwrap();
    let _ = std::fs::remove_file(path);
    assert_eq!(
        plan.text(),
        "\
feature live-telemetry: on
feature video-screen: on
feature ui-strings: on
feature level-boot: on
feature lockstep: on
feature engine-leaves: on
0x000d9640 DAMAGE_REPORT: observe_before [live-telemetry:hits] -> original
0x00104150 WEAPON_FIRE: observe_before [live-telemetry:shots] -> original
0x00137fe0 GAME_TICK: original -> observe_after [live-telemetry:sample, lockstep:hash]
0x001389c0 GAME_END: original -> observe_after [live-telemetry:end]
0x0015ce70 PLAYER_KILLED: observe_before [live-telemetry:kill] -> original
0x0018f000 GAME_ACTIVATE: original -> observe_after [level-boot:loaded mark, live-telemetry:match start]
0x001a02d0 LOCALE_FIND: implement engine-leaves:locale_string_id_find
0x001a0310 LOCALE_EXISTS: implement engine-leaves:locale_string_id_exists
0xa0f01000 entry video-screen:loader
0xa0f01010 entry video-screen:row text
0xa0f01020 entry video-screen:row chosen
0xa0f01030 entry video-screen:set up pane
0xa0f01040 entry video-screen:loader slot
locale source ui-strings:our strings (served by engine-leaves)
warning: LOCALE_FIND (0x001a02d0) is hot: hooking it leaves the direct-call fast path
"
    );
    // Shadows only where something runs after the original.
    let shadows = plan.chains.values().filter(|c| c.shadow.is_some()).count();
    assert_eq!(shadows, 3);
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
