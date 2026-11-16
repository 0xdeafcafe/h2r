//! The plan boot builds with every migrated feature on (Live telemetry, the
//! level boot's loaded mark, our strings, lockstep's tick hash): it builds
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
feature ui-strings: on
feature level-boot: on
feature lockstep: on
feature engine-leaves: on
0x00104150 WEAPON_FIRE: observe_before [live-telemetry:shots] -> original
0x00137fe0 GAME_TICK: original -> observe_after [live-telemetry:sample, lockstep:hash]
0x001389c0 GAME_END: original -> observe_after [live-telemetry:end]
0x0015ce70 PLAYER_KILLED: observe_before [live-telemetry:kill] -> original
0x0018f000 GAME_ACTIVATE: original -> observe_after [level-boot:loaded mark, live-telemetry:match start]
0x001a02d0 LOCALE_FIND: before [ui-strings:our strings] -> original
warning: LOCALE_FIND (0x001a02d0) is hot: hooking it leaves the direct-call fast path
"
    );
    // Shadows only where something runs after the original.
    let shadows = plan.chains.values().filter(|c| c.shadow.is_some()).count();
    assert_eq!(shadows, 3);
}

/// With `impl.engine=on` the Rust locale lookup sits under our strings
/// instead of displacing them (or being displaced).
#[test]
fn the_rust_locale_lookup_goes_under_our_strings() {
    let mut r = feature::Registry::new(feature::Switches::parse("impl.engine=on").unwrap());
    r.add(&boot::layout_glue::StringsFeature).unwrap();
    r.add(&feature::leaves::EngineLeaves).unwrap();
    let plan = r.build(&feature::Base).unwrap();
    assert!(plan.text().contains(
        "0x001a02d0 LOCALE_FIND: before [ui-strings:our strings] -> implement engine-leaves:locale_string_id_find\n"
    ));
}
