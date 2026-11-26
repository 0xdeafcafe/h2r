//! The feature plan, built from the registration code boot runs.
//!
//! A clash (two features hooking one point the same way with no order
//! between them) makes boot refuse the plan and the game stop at launch.
//! These tests call [`register_core`](super::register_core) and
//! [`register_late`](super::register_late) with test inputs, add the
//! registrations `run_hosted_with_live` makes between and after them (the
//! Theater, the lab takeover, the host metrics, the light host's skips, the
//! return to the menu, the render aspect, Forge) and build the plan, so the
//! clash fails here. [`every_registration_site_in_boot_is_accounted_for`]
//! fails when boot gains a registration this file does not know.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{AgentSource, CoreInputs, LateInputs, register_core_film, register_late_for_profile};
use crate::lockstep_glue;

/// Every switch that adds hooks, on (the native renderer, the engine leaves,
/// Forge, the actor capture, Firefight and the creature probe are on by
/// default; only the diagnostics and `impl.engine=on` are spelled out;
/// `rules`, on by default too, stays named).
pub(crate) const ALL_ON: &str = "impl.engine=on,render-timing,actor-net,firefight,rules";
/// Added to [`ALL_ON`]: the native renderer (on by default) off.
pub(crate) const NATIVE_OFF: &str = "impl.render.native=off";

/// What a test run has, from nothing (a headless run with no settings) to
/// everything a test can stand in for.
#[derive(Clone, Debug)]
pub(crate) struct Case {
    /// `H2_FEATURES`.
    pub switches: String,
    /// A default settings store: skulls, aim assist, the Halo 1 options, the
    /// Clankers page and the multiplayer skulls need one.
    pub settings: bool,
    /// A backend drawing at the native 1x resolution.
    #[cfg(feature = "gpu")]
    pub drawing: bool,
    /// The XAPI layer: bots and the Clankers page.
    pub xapi: bool,
    /// A mounted VFS: map slots, our skulls' map reads.
    pub vfs: bool,
    /// Telemetry to a file.
    pub telemetry: bool,
    /// `H2_LEVEL` / `H2_PERF_FROM=level`: the level-boot hooks.
    pub level: bool,
    /// The debug menu's local agent: our cheats.
    pub agent: bool,
    /// `H2_LOCKSTEP`: the tick observer.
    pub lockstep: bool,
    /// `H2_COOP_PROBE`.
    pub coop_probe: bool,
    /// Signed in to Live: co-op over Live's armed hooks (`coop-live`), as
    /// boot adds them when there is a Live HLE and no `H2_COOP_PROBE`.
    pub coop_live: bool,
    /// The Theater page and film link, installed as boot's `Theater::install`
    /// does (`theater-texts`, `theater-link`).
    pub theater: bool,
    /// Signed in to a Live service (a test Live HLE): the join link, the
    /// browser, the map pack rotation and the rest of Live's registrations.
    /// Boot's own co-op arming then comes from `register_late`, so
    /// `coop_live` adds nothing.
    pub live: bool,
    /// A window: the lab takeover takes requests (`AgentGlue::configure_takeover`,
    /// needs `agent`). Its dialog is planned with `xapi` alone
    /// (`registration::lab_takeover`).
    pub takeover: bool,
    /// The light host: the viewer skips, the native sim, `sim-frame`, no
    /// window (so no drawing, takeover or return to the menu) and no
    /// non-player Live features.
    pub light: bool,
    /// The dedicated lobby (`H2_HOST_LOBBY=1` on a light host with an
    /// agent), added as `host_lobby::install` does.
    pub host_lobby: bool,
    /// Theater's playback child: boot's `replaying`, which decides the
    /// non-player features and Forge with no film or a film's header
    /// (`registration::non_player`, `registration::local_forge`).
    pub replaying: bool,
}

impl Case {
    pub fn none(switches: &str) -> Case {
        Case {
            switches: switches.to_owned(),
            settings: false,
            #[cfg(feature = "gpu")]
            drawing: false,
            xapi: false,
            vfs: false,
            telemetry: false,
            level: false,
            agent: false,
            lockstep: false,
            coop_probe: false,
            coop_live: false,
            theater: false,
            live: false,
            takeover: false,
            light: false,
            host_lobby: false,
            replaying: false,
        }
    }

    /// Everything a test can supply except the runs that exclude each other
    /// (lockstep, the co-op probe and the light host).
    pub fn full(switches: &str) -> Case {
        Case {
            settings: true,
            #[cfg(feature = "gpu")]
            drawing: true,
            xapi: true,
            vfs: true,
            telemetry: true,
            level: true,
            agent: true,
            theater: true,
            live: true,
            takeover: true,
            ..Case::none(switches)
        }
    }

    fn describe(&self) -> String {
        format!("{self:?}")
    }
}

/// The registry boot would hand the plan for `case`, from the same
/// registration functions.
pub(crate) fn registry(case: &Case) -> hooks::Registry {
    registry_for_profile(case, super::compiled_render_hooks())
}

fn live_config() -> live_hle::LiveConfig {
    live_hle::LiveConfig {
        enabled: true,
        server_url: Some("https://live.example".into()),
        ..Default::default()
    }
}

fn no_library(_: &Path) -> Result<Box<dyn crate::theater_ui::adapter::Library>, String> {
    Err("no film library in a test".into())
}

/// A sink for the render aspect's layout, standing in for the device.
#[cfg(feature = "gpu")]
struct NoDevice;

#[cfg(feature = "gpu")]
impl crate::render_aspect::UiLayoutSink for NoDevice {
    fn set_projection_factor(&self, _: f32) {}
    fn set_fit(&self, _: &mut runtime::Guest, _: Option<d3d::backend::UiFit>) {}
}

fn registry_for_profile(case: &Case, profile: film::RenderHooks) -> hooks::Registry {
    registry_with_film(case, profile, None)
}

/// [`registry_for_profile`] for a boot that records `film`.
fn registry_with_film(
    case: &Case,
    profile: film::RenderHooks,
    film: Option<Arc<crate::theater::Session>>,
) -> hooks::Registry {
    let rt = Arc::new(runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::Dispatch::default(),
    ));
    let settings = case
        .settings
        .then(|| Arc::new(settings::Store::defaults(crate::setting_defs::ALL)));
    let mut features = hooks::Registry::new(super::profile_switches(
        hooks::Switches::parse(&case.switches).unwrap(),
        case.light,
    ));
    let nowhere = Path::new("");
    let live = case
        .live
        .then(|| live_hle::Hle::new(live_config()).unwrap());
    // Boot's order: the Theater first, then the core registrations.
    let theater = case.theater.then(|| {
        let theater = crate::theater_ui::Theater::default();
        theater
            .install(
                &mut features,
                nowhere,
                crate::theater::Installation::new(&[], Vec::new(), PathBuf::new()),
                &live.as_ref().map(|_| live_config()).unwrap_or_default(),
                live.clone(),
                no_library,
            )
            .unwrap();
        theater
    });
    // Boot's film decisions, from the same functions (lib.rs).
    let non_player = super::non_player(film.as_deref(), case.replaying, case.light, false);
    let local_forge = super::local_forge(
        features.switches(),
        film.as_deref(),
        case.replaying,
        case.light,
        case.lockstep,
    );
    let telemetry = if case.replaying {
        Ok(crate::telemetry::TelemetryConfig::playback())
    } else if case.telemetry {
        Ok(crate::telemetry::TelemetryConfig::file(
            std::env::temp_dir().join("h2-registration-test.telemetry"),
        ))
    } else {
        Ok(crate::telemetry::TelemetryConfig::default())
    };
    let core = register_core_film(
        &mut features,
        CoreInputs {
            theater: theater.clone(),
            rt: &rt,
            telemetry,
            vfs: case.vfs.then(|| Arc::new(Mutex::new(vfs::Vfs::new()))),
            model: None,
            campaigns: None,
            settings: settings.clone(),
            live: live.clone(),
            data_root: nowhere,
            packages: nowhere,
            level: None,
            perf_level: case.level,
            bots: case.xapi,
        },
        film.clone(),
        non_player,
        None,
    )
    .unwrap();
    // The window's drawing device: the render aspect (a light host has none).
    #[cfg(feature = "gpu")]
    let windowed = case.drawing && !case.light;
    #[cfg(feature = "gpu")]
    if windowed {
        crate::render_aspect::install(
            &mut features,
            Arc::new(|| (640, 480)),
            !case.lockstep,
            Arc::new(|| 0),
            Arc::new(NoDevice),
        )
        .unwrap();
    }
    // Boot's render stream: every rendering session but lockstep's.
    if !case.light && !case.lockstep {
        crate::render_rng::install(&mut features).unwrap();
    }
    let lockstep = lockstep_glue::Config {
        trace: case
            .lockstep
            .then(|| PathBuf::from("h2-registration-test.lockstep")),
        ticks: true,
        record: None,
        replay: None,
    };
    let lockstep_on = lockstep.enabled();
    #[cfg(feature = "gpu")]
    let protected = crate::d3d_glue::protected_readbacks(live.is_some(), film.is_some(), lockstep_on);
    let mut late = register_late_for_profile(
        &mut features,
        LateInputs {
            #[cfg(feature = "gpu")]
            view: windowed.then_some(&crate::d3d_glue::DrawingView { scale_stats: None }),
            #[cfg(feature = "gpu")]
            d3d_display: !protected,
            #[cfg(feature = "gpu")]
            viewer_lod: None,
            #[cfg(feature = "gpu")]
            world_line_present: None,
            rt: &rt,
            lockstep,
            agent: if case.agent {
                AgentSource::Recording
            } else {
                AgentSource::None
            },
            allow_bot_add: !case.light || core.bots_on,
            xa: None,
            coop_probe: case.coop_probe.then(|| {
                crate::coop_probe::parse(|k| (k == "H2_COOP_PROBE").then(|| "1".to_owned()))
                    .unwrap()
                    .unwrap()
            }),
            live: live.clone(),
            settings,
            map_gate: None,
            disc: nowhere,
            data_root: nowhere,
        },
        profile,
    )
    .unwrap();
    // Boot's Live branch (registration.rs): the armed co-op hooks, unless
    // the probe holds them. With a Live HLE `register_late` made them.
    if case.coop_live && !case.coop_probe && live.is_none() {
        crate::coop_probe::install_live(&rt, &mut features, nowhere).unwrap();
    }
    // What `run_hosted_with_live` adds after `register_late`, in its order.
    if local_forge {
        crate::forge_glue::install(
            &mut features,
            late.agent.as_ref(),
            None,
            nowhere,
            None,
            live.clone(),
        )
        .unwrap();
    }
    if let Some(a) = &mut late.agent {
        // Boot's decision, from the same function (lib.rs): pads plan the
        // dialog, and `takeover` is the window that takes requests.
        let window = case.takeover && !case.light;
        let takeover = super::lab_takeover(super::TakeoverInputs {
            window,
            d3d: true,
            xapi: case.xapi,
            lockstep: case.lockstep,
            host_light: case.light,
            sim: false,
            replaying: case.replaying,
        });
        a.configure_takeover(&mut features, window, takeover.plan, takeover.accept)
            .unwrap();
    }
    if let (Some(theater), Some(a)) = (&theater, &late.agent) {
        theater.connect(&a.agent);
    }
    if case.light {
        features
            .add(&sim_bridge::sim_native::SimNative::new(&rt))
            .unwrap();
    }
    super::register_viewer_skips(&mut features, case.light).unwrap();
    if case.light {
        assert!(
            features
                .add(&sim_bridge::sim_frame::SimFrame::new(0))
                .unwrap(),
            "light host requires sim-frame"
        );
    }
    if let Some(a) = &late.agent {
        crate::host_metrics::install(&mut features, &a.agent, case.light).unwrap();
        if case.light && case.host_lobby {
            features
                .add(&crate::host_lobby::planned_feature(live.clone()))
                .unwrap();
        }
    }
    #[cfg(feature = "gpu")]
    if windowed && !lockstep_on {
        features
            .add(&crate::return_to_menu::ReturnToMenu(Arc::new(|_| {})))
            .unwrap();
    }
    // Boot's film branch: the session's observer.
    if let Some(film) = &film {
        assert!(
            features
                .add(&crate::theater::Observer(
                    film.clone(),
                    late.agent.as_ref().map(|a| a.agent.clone()),
                ))
                .unwrap(),
            "theater-film feature is disabled"
        );
    }
    features
}

/// Builds the plan for `case`; the refusal, if any, says which features clash.
fn plan(case: &Case) -> Result<hooks::Plan, String> {
    registry(case).build(&hooks::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        format!(
            "feature plan refused: {}\n  for {}",
            c.join("; "),
            case.describe()
        )
    })
}

fn assert_builds(case: &Case) {
    if let Err(e) = plan(case) {
        panic!("{e}");
    }
}

#[test]
fn the_creature_probe_plan_keeps_admission_on_in_both_arms() {
    use crate::actor_net::creature::FEATURE;

    for (switches, enabled) in [
        (format!("{ALL_ON},-{FEATURE}"), false),
        (ALL_ON.to_owned(), true),
    ] {
        let mut case = Case::full(&switches);
        case.coop_live = true;
        let plan = plan(&case).unwrap();
        let installed = |id| {
            plan.chains
                .values()
                .any(|chain| chain.hooks().any(|hook| hook.feature == id))
        };

        assert!(installed(crate::replication_admission::FEATURE));
        assert_eq!(installed(FEATURE), enabled);
    }
}

#[test]
fn a_desktop_player_registers_only_the_films_render_groups() {
    use film::RenderHooks::{Desktop, Gpu, Slim};
    for profile in [Slim, Desktop, Gpu] {
        if !super::compiled_render_hooks().supports(profile) {
            continue;
        }
        let plan = registry_for_profile(&Case::none(ALL_ON), profile)
            .build(&hooks::Base)
            .unwrap();
        let ids: Vec<_> = plan.features.iter().map(|f| f.id).collect();
        assert_eq!(ids.contains(&"native-map-work"), profile == Gpu);
        assert_eq!(ids.contains(&"native-world"), profile == Gpu);
        assert_eq!(ids.contains(&"native-hud"), profile == Gpu);
        assert_eq!(ids.contains(&"native-scene-lists"), profile == Gpu);
        assert_eq!(ids.contains(&"native-light-walk"), profile == Gpu);
        assert!(ids.contains(&"h1-scope-zoom"));
        if profile == Slim {
            assert!(
                plan.chains
                    .values()
                    .flat_map(|c| c.hooks())
                    .all(|h| { h.feature != "render-loop" && !h.feature.starts_with("native-") })
            );
        }
    }
}

#[test]
fn actor_net_is_on_unless_switched_off_with_the_host_capture_and_the_client_puppets() {
    for (switches, enabled) in [("-actor-net", false), ("", true)] {
        let plan = plan(&Case::full(switches)).unwrap();
        let mut hooks: Vec<_> = plan
            .chains
            .values()
            .flat_map(|chain| {
                chain
                    .hooks()
                    .filter(|hook| hook.feature == "actor-net")
                    .map(move |hook| (chain.point.name, hook.kind().as_str()))
            })
            .collect();
        hooks.sort_unstable();
        if enabled {
            assert_eq!(
                hooks,
                [
                    ("AI_DIALOGUE_ENQUEUE", "observe_after"),
                    ("AI_DIALOGUE_ENQUEUE", "observe_before"),
                    // The host capture's and the client's new-match resets.
                    ("GAME_ACTIVATE", "observe_after"),
                    ("GAME_ACTIVATE", "observe_after"),
                    // The client applies presentation before the tick; the
                    // host captures after it.
                    ("GAME_TICK", "before"),
                    ("GAME_TICK", "observe_after"),
                ]
            );
        } else {
            assert!(hooks.is_empty());
        }
    }
}

#[cfg(feature = "gpu")]
#[test]
fn lod_distance_requires_a_gpu_profile_and_a_drawing_view() {
    use film::RenderHooks::{Desktop, Gpu, Slim};
    for profile in [Slim, Desktop, Gpu] {
        for drawing in [false, true] {
            let mut case = Case::full("");
            case.drawing = drawing;
            let plan = registry_for_profile(&case, profile)
                .build(&hooks::Base)
                .unwrap();
            let enabled = profile == Gpu && drawing;
            assert_eq!(
                plan.features
                    .iter()
                    .any(|feature| feature.id == "lod-distance"),
                enabled
            );
            assert_eq!(
                plan.chains
                    .values()
                    .any(|chain| chain.point.name == "MODEL_LOD"),
                enabled
            );
        }
    }
    // The guest chooser still runs when either set of native replacements
    // is switched off. Disabling the LOD feature disables both adapters.
    for (switches, enabled) in [
        (NATIVE_OFF, true),
        ("-native-scene-lists", true),
        ("-lod-distance", false),
    ] {
        let plan = registry_for_profile(&Case::full(switches), Gpu)
            .build(&hooks::Base)
            .unwrap();
        for point in ["MODEL_LOD", "RESET_SCENE_RECORDS"] {
            assert_eq!(
                plan.chains.values().any(|chain| chain.point.name == point),
                enabled
            );
        }
    }
}

/// Boot with nothing optional: no settings, no XAPI, default switches.
#[test]
fn the_plan_builds_with_default_switches() {
    assert_builds(&Case::none(""));
    assert_builds(&Case::full(""));
}

/// Every switch that adds hooks on (the census in a second build, and the
/// native renderer off in a third), with everything a test can supply.
#[test]
fn the_plan_builds_with_every_hook_switch_on() {
    assert_builds(&Case::none(ALL_ON));
    assert_builds(&Case::full(ALL_ON));
    assert_builds(&Case::full(&format!("{ALL_ON},render-census")));
    assert_builds(&Case::full(&format!("{ALL_ON},{NATIVE_OFF}")));
    // Forge is on by default; `-forge` leaves it out.
    assert_builds(&Case::full(ALL_ON));
    assert_builds(&Case::full(&format!("{ALL_ON},-forge")));
}

/// The biggest plans a test can build, every switch on, leave at least a
/// sixteenth of the guest arena (`hooks::arena`) free, so a few more
/// features do not refuse boot. The default plan leaves more.
#[test]
fn the_biggest_plans_leave_room_in_the_guest_arena() {
    use hooks::arena::ARENA_SIZE;
    let native_off = format!("{ALL_ON},{NATIVE_OFF}");
    let forge = format!("{ALL_ON},forge");
    for switches in [ALL_ON, native_off.as_str(), forge.as_str()] {
        let mut case = Case::full(switches);
        case.coop_live = true;
        let used = plan(&case).unwrap().arena_used();
        assert!(
            used <= ARENA_SIZE / 16 * 15,
            "{used:#x} of the arena's {ARENA_SIZE:#x} bytes used, for {}",
            case.describe()
        );
    }
}

/// The rules runtime is on by default, and `-rules` takes it out of the plan.
#[cfg(feature = "rules")]
#[test]
fn the_rules_runtime_is_planned_unless_turned_off() {
    let has = |switches: &str| {
        plan(&Case::full(switches))
            .unwrap()
            .features
            .iter()
            .any(|f| f.id == crate::rules::feature::FEATURE && f.off.is_none())
    };
    assert!(has(""));
    assert!(has("rules"));
    assert!(has(ALL_ON));
    assert!(!has("-rules"));
}

/// The rules runtime answers no retail engine's scoring or role-making
/// routine, on or off: a script game's engine object calls none of them
/// (`rules::engine_object`), so none is hooked by it.
#[cfg(feature = "rules")]
#[test]
fn the_rules_runtime_hooks_no_retail_scoring_site() {
    const SCORING: [&str; 4] = ["JUGGERNAUT_KILL", "JUGGERNAUT_TICK", "SCORE_ADD", "SLAYER_KILL"];
    for switches in ["rules", ALL_ON, "", "-rules"] {
        let plan = plan(&Case::full(switches)).unwrap();
        let hooked: Vec<_> = plan
            .chains
            .values()
            .filter(|chain| SCORING.contains(&chain.point.name))
            .flat_map(|chain| {
                chain
                    .hooks()
                    .filter(|hook| hook.feature == crate::rules::feature::FEATURE)
                    .map(move |hook| (chain.point.name, hook.kind().as_str()))
            })
            .collect();
        assert!(hooked.is_empty(), "{switches:?}: {hooked:?}");
    }
}

/// With the rules runtime on, the record clean-up, the wire codec, the
/// protocol map and the content category each have the runtime's
/// `before` (`rules::record`), and no other feature's; off, none is hooked.
#[cfg(feature = "rules")]
#[test]
fn the_rules_runtime_answers_the_record_paths_unless_turned_off() {
    const PATHS: [&str; 5] = [
        "VARIANT_CATEGORY",
        "VARIANT_CLEAN_UP",
        "VARIANT_DECODE",
        "VARIANT_ENCODE",
        "VARIANT_PROTOCOL",
    ];
    let hooks = |switches: &str| {
        let plan = plan(&Case::full(switches)).unwrap();
        let mut at: Vec<_> = plan
            .chains
            .values()
            .filter(|chain| PATHS.contains(&chain.point.name))
            .flat_map(|chain| {
                chain
                    .hooks()
                    .map(move |hook| (chain.point.name, hook.feature, hook.kind().as_str()))
            })
            .collect();
        at.sort_unstable();
        at
    };
    let want: Vec<_> = PATHS
        .iter()
        .map(|&p| (p, crate::rules::feature::FEATURE, "before"))
        .collect();
    assert_eq!(hooks("rules"), want);
    assert_eq!(hooks(""), want);
    assert!(hooks("-rules").is_empty());
}

/// With the rules runtime on, every game start puts the script index's
/// engine object in and moves a record that would score beside its engine
/// onto the script index (`GAME_ACTIVATE`, two `before`s in the last class)
/// and the
/// spawn gather's jump on the engine index has the runtime's `before`
/// (`rules::engine_object`); off, neither is hooked by it.
#[cfg(feature = "rules")]
#[test]
fn the_rules_runtime_puts_in_the_script_index_engine_object_unless_turned_off() {
    const POINTS: [&str; 2] = ["GAME_ACTIVATE", "SPAWN_TYPE_MASK"];
    let hooks = |switches: &str| {
        let plan = plan(&Case::full(switches)).unwrap();
        let mut at: Vec<_> = plan
            .chains
            .values()
            .filter(|chain| POINTS.contains(&chain.point.name))
            .flat_map(|chain| {
                chain
                    .hooks()
                    .filter(|hook| hook.feature == crate::rules::feature::FEATURE)
                    .map(move |hook| (chain.point.name, hook.kind().as_str()))
            })
            .collect();
        at.sort_unstable();
        at
    };
    let want = vec![
        ("GAME_ACTIVATE", "before"),
        ("GAME_ACTIVATE", "before"),
        ("GAME_ACTIVATE", "observe_after"),
        ("SPAWN_TYPE_MASK", "before"),
    ];
    assert_eq!(hooks("rules"), want);
    assert_eq!(hooks(""), want);
    assert!(hooks("-rules").is_empty());
    // The spawn gather's site and its resume are image entries.
    let entries: Vec<u32> = crate::image_entries().collect();
    assert!(entries.contains(&engine::points::game_types::SPAWN_TYPE_MASK.base));
    assert!(entries.contains(&engine::points::game_types::SPAWN_TYPE_MASK_SLAYER));
}

/// With the multiplayer skulls on, every AI skull reader's campaign gate
/// (`engine::points::skull_gates`) has the feature's one `before`, and each
/// gate's site and fall-through are image entries; off, no gate is hooked.
#[test]
fn every_ai_skull_gate_has_the_multiplayer_skulls_before_unless_turned_off() {
    use engine::points::skull_gates::GATES;
    let hooked = |switches: &str| {
        let plan = plan(&Case::full(switches)).unwrap();
        let mut at: Vec<u32> = plan
            .chains
            .values()
            .filter(|chain| GATES.iter().any(|g| std::ptr::eq(chain.point, &g.site)))
            .flat_map(|chain| {
                chain
                    .hooks()
                    .filter(|hook| hook.feature == crate::skulls_mp::FEATURE)
                    .map(move |hook| {
                        assert_eq!(hook.kind().as_str(), "before", "{}", chain.point.name);
                        chain.addr
                    })
            })
            .collect();
        at.sort_unstable();
        at
    };
    let mut want: Vec<u32> = GATES.iter().map(|g| g.site.base).collect();
    want.sort_unstable();
    assert_eq!(hooked(""), want);
    assert!(hooked("-skulls-mp").is_empty());
    let entries: Vec<u32> = crate::image_entries().collect();
    for g in &GATES {
        assert!(entries.contains(&g.site.base), "{}", g.site.name);
        assert!(entries.contains(&g.resume), "{}", g.site.name);
    }
}

/// The features `run_hosted_with_live` adds around `register_core` and
/// `register_late`, present in the full case's plan.
#[test]
fn the_full_case_plans_every_feature_boot_adds_outside_the_registration_functions() {
    let full = plan(&Case::full("")).unwrap();
    let ids: Vec<_> = full.features.iter().map(|f| f.id).collect();
    #[allow(unused_mut)]
    let mut expected = vec![
        // `Theater::install`, before the core registrations.
        "theater-texts",
        "theater-link",
        // `join_link::install` with a Live HLE (in `register_core`).
        "live-join-link",
        // `AgentGlue::configure_takeover` with a window and player input.
        "lab-takeover",
        "lab-takeover-dialog",
        "lab-takeover-alongside-dialog",
        "lab-running-dialog",
        // `host_metrics::install` with an agent.
        "host-metrics",
    ];
    #[cfg(feature = "gpu")]
    expected.extend([
        // `render_aspect::install` with a drawing device.
        "render-aspect",
        // `return_to_menu::ReturnToMenu` on a window with no lockstep run.
        crate::return_to_menu::ID,
    ]);
    let missing: Vec<_> = expected
        .iter()
        .copied()
        .filter(|id| !ids.contains(id))
        .collect();
    assert!(
        missing.is_empty(),
        "the full case leaves out {missing:?}; planned: {ids:?}"
    );

    // The light host's registrations, and Forge, on by default.
    let mut light = Case::full("");
    light.light = true;
    let light = plan(&light).unwrap();
    assert!(
        light
            .features
            .iter()
            .any(|f| f.id == sim_bridge::sim_frame::ID)
    );
    let forge = plan(&Case::full("")).unwrap();
    assert!(forge.features.iter().any(|f| f.id == "forge"));
    let no_forge = plan(&Case::full("-forge")).unwrap();
    assert!(!no_forge.features.iter().any(|f| f.id == "forge"));
}

/// The feature set `h2 serve --system-link --host-light` plans with
/// `H2_HOST_LOBBY=1`: the core and late registrations, the light skips and
/// `sim-frame`, the host metrics and the dedicated lobby. A production boot
/// refused this plan (the lobby and the map select both hook the game tick
/// with no order) while every other plan test passed, because none added the
/// lobby. Every switch set, with and without Live, settings, a VFS and bots.
#[test]
fn the_light_host_with_the_dedicated_lobby_plans_with_no_refusal() {
    let mut failures = Vec::new();
    let native_off = format!("{ALL_ON},{NATIVE_OFF}");
    for switches in ["", ALL_ON, native_off.as_str(), "host-light-native"] {
        for bits in 0u32..(1 << 4) {
            let mut case = Case::full(switches);
            case.light = true;
            case.takeover = false;
            case.host_lobby = true;
            case.live = bits & 1 != 0;
            case.settings = bits & 2 != 0;
            case.vfs = bits & 4 != 0;
            case.xapi = bits & 8 != 0;
            if let Err(e) = plan(&case) {
                failures.push(e);
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases refused:\n{}",
        failures.len(),
        dedup(failures).join("\n")
    );
    let mut case = Case::full("");
    case.light = true;
    case.takeover = false;
    case.host_lobby = true;
    let planned = plan(&case).unwrap();
    // map-select installs only once boot prepared the map catalogue, a
    // process-wide set this test does not make: it would leak into the
    // other plan tests, whose small guest arena it fills.
    {
        let id = "host-lobby";
        assert!(
            planned.features.iter().any(|f| f.id == id),
            "the light host plan has no {id}"
        );
    }
}

/// Where boot registers features outside `register_core` and
/// `register_late` (`lib.rs`, `run_hosted_with_live`), with the plan test
/// that covers each, or why none can. Boot's `&mut features` and
/// `features.add(` sites are counted below: a new one fails the count, and
/// the fix is a row here and, if a test can build it without a disc, a
/// step in `registry_for_profile`.
const BOOT_SITES: &[(&str, &str)] = &[
    (
        "campaign_cache::configure",
        "not covered: its limit feature is private and only added when a map outgrows retail's cache slot",
    ),
    ("Theater::install", "Case::theater"),
    (
        "register_core_film",
        "registry_for_profile (a film's session is not covered: it needs a recording)",
    ),
    ("render_aspect::install", "Case::drawing (gpu)"),
    ("render_rng::install", "every case but Case::light and Case::lockstep"),
    ("register_late_for_profile", "registry_for_profile"),
    (
        "forge_glue::install",
        "Case::full, and `-forge` in the full-case plan test",
    ),
    ("AgentGlue::configure_takeover", "Case::takeover"),
    (
        "dsound_glue::add_loop_agent_method",
        "not covered: needs the audio device (agent-audio)",
    ),
    ("register_viewer_skips", "Case::light"),
    ("host_metrics::install", "Case::agent"),
    ("host_lobby::install", "Case::host_lobby"),
    ("Sim::install_with_film", "not covered: needs a sim session"),
    (
        "Attract::install",
        "not covered: its constructor is crate-private to game and needs the bink HLE",
    ),
    ("SimNative", "Case::light"),
    ("SimFrame", "Case::light"),
    ("ReturnToMenu", "Case::drawing (gpu)"),
    (
        "host audit",
        "not covered: needs the generated module and the running stamp",
    ),
    ("theater::Observer", "registry_with_film"),
    ("film camera hooks", "not covered: needs a film session"),
];

/// Counts the registration call sites in boot's `lib.rs`.
#[test]
fn every_registration_site_in_boot_is_accounted_for() {
    let source = include_str!("../lib.rs");
    let code: String = source
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect();
    let sites = code.matches("&mutfeatures").count() + code.matches("features.add(").count();
    assert_eq!(
        sites,
        BOOT_SITES.len(),
        "boot's lib.rs registers features at {sites} places and BOOT_SITES lists {}; \
         add the new one to BOOT_SITES, and to registry_for_profile when a test can build it",
        BOOT_SITES.len()
    );
}

/// Each optional piece alone and in every combination, with the default
/// switches and with every hook switch on: skulls and the Halo 1 options
/// (a settings store), bots and the Clankers page (XAPI), map slots and our
/// skulls' map reads (a VFS), telemetry, the level boot, our cheats (an
/// agent), the lockstep harness and the co-op probe.
///
/// The second group (the Theater, Live, the lab takeover and the light
/// host) is crossed with that in two ways to keep the run short: all four
/// off or all four on over every first-group combination, then every
/// combination of the four over a few first-group bases.
#[test]
fn the_plan_builds_with_every_combination_of_optional_features() {
    let mut failures = Vec::new();
    let native_off = format!("{ALL_ON},{NATIVE_OFF}");
    let forge = format!("{ALL_ON},forge");
    for switches in ["", ALL_ON, native_off.as_str()] {
        for bits in 0u32..(1 << 9) {
            let on = |n: u32| bits & (1 << n) != 0;
            let first = Case {
                switches: switches.to_owned(),
                settings: on(0),
                #[cfg(feature = "gpu")]
                drawing: true,
                xapi: on(1),
                vfs: on(2),
                telemetry: on(3),
                level: on(4),
                agent: on(5),
                lockstep: on(6),
                coop_probe: on(7),
                coop_live: on(8),
                ..Case::none(switches)
            };
            for second in [0u32, 0b0111] {
                if let Err(e) = plan(&with_second_group(&first, second)) {
                    failures.push(e);
                }
            }
        }
    }
    // Every combination of the second group over the bases that matter:
    // nothing else, everything a test can supply, and the probe's and
    // lockstep's runs.
    let bases = |switches: &str| {
        let mut probe = Case::full(switches);
        probe.coop_probe = true;
        let mut lockstep = Case::full(switches);
        lockstep.lockstep = true;
        [Case::none(switches), Case::full(switches), probe, lockstep]
    };
    for switches in ["", ALL_ON, native_off.as_str(), forge.as_str()] {
        for base in bases(switches) {
            for second in 0u32..(1 << 4) {
                if let Err(e) = plan(&with_second_group(&base, second)) {
                    failures.push(e);
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} cases refused:\n{}",
        failures.len(),
        dedup(failures).join("\n")
    );
}

/// `case` with the second group's bits: the Theater, Live, the takeover and
/// the light host.
fn with_second_group(case: &Case, bits: u32) -> Case {
    Case {
        theater: bits & 1 != 0,
        live: bits & 2 != 0,
        takeover: bits & 4 != 0,
        light: bits & 8 != 0,
        ..case.clone()
    }
}

/// The distinct first lines of the refusals, so one clash reads once.
fn dedup(failures: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for f in failures {
        let head = f.lines().next().unwrap_or_default().to_owned();
        if !seen.contains(&head) {
            seen.push(head);
        }
    }
    seen
}

#[test]
fn the_lobby_copy_boost_is_on_for_light_hosts_unless_switched_off() {
    for (light, spec, on) in [
        (false, "", false),
        (true, "", true),
        (true, "-map-copy.lobby", false),
        (true, "map-copy.lobby=off", false),
        (false, "map-copy.lobby", true),
    ] {
        let switches = super::profile_switches(hooks::Switches::parse(spec).unwrap(), light);
        assert_eq!(
            switches.feature(crate::map_copy::LOBBY, false),
            on,
            "{light} {spec}"
        );
    }
}

#[test]
fn light_native_subsystems_follow_the_profile_and_explicit_overrides() {
    use engine::points::{COLLISION_LOCATION, OBJECT_FUNCTION_VALUE, SIM_ANIMATION_STREAM};
    let runtime = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
    let points = [
        &SIM_ANIMATION_STREAM,
        &COLLISION_LOCATION,
        &OBJECT_FUNCTION_VALUE,
    ];
    for (light, spec, expected) in [
        (true, "", [false; 3]),
        (false, "host-light-native", [false; 3]),
        (true, "host-light-native", [true; 3]),
        (
            true,
            "-host-light-native,impl.sim.native.anim",
            [true, false, false],
        ),
        (
            true,
            "host-light-native,-impl.sim.native.collision",
            [true, false, true],
        ),
        (true, "host-light-native,-impl.sim.native", [false; 3]),
        (
            true,
            "host-light-native,-impl.sim,impl.sim.native.objects",
            [false, false, true],
        ),
    ] {
        let switches = super::profile_switches(hooks::Switches::parse(spec).unwrap(), light);
        assert!(!switches.implementation("sim.native_unit_pose", false));
        let mut registry = hooks::Registry::new(switches);
        registry
            .add(&sim_bridge::sim_native::SimNative::new(&runtime))
            .unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        assert_eq!(
            points.map(|p| plan
                .chains
                .get(&p.base)
                .is_some_and(|c| c.implement.is_some())),
            expected,
            "{light} {spec}"
        );
    }
}

#[test]
fn viewer_skips_follow_the_light_profile_and_independent_switches() {
    use engine::points::PREDICT_RESOURCES;
    for (light, spec, predicted) in [
        (false, "", false),
        (false, "impl.sim=on", false),
        (true, "", true),
        (true, "-impl.sim.observer-resources", false),
        (true, "impl.sim.native.seats", false),
        (true, "-impl.sim", false),
    ] {
        let mut registry = hooks::Registry::new(hooks::Switches::parse(spec).unwrap());
        super::register_viewer_skips(&mut registry, light).unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        assert_eq!(plan.chains.contains_key(&PREDICT_RESOURCES.base), predicted, "{light} {spec}");
    }
}

/// A player session's recording: by default (`auto`) into the film
/// library `dir/name`, or with `--film` to the film `dir/name`.
fn recording(dir: &Path, name: &str, auto: bool) -> Arc<crate::theater::Session> {
    let opts = crate::RunOpts {
        disc: PathBuf::new(),
        data_root: dir.to_owned(),
        max_slices: 1,
        xapi: true,
        transfers_per_slice: 5000,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        movies: crate::movies_glue::Movies::Off,
    };
    let facts = crate::theater::Facts {
        stamp: "fixture".into(),
        title_id: 2,
        epoch: 0,
        light: false,
        bots: false,
        address: "127.0.0.1:3074".into(),
        live_service: true,
        telemetry_hooks: live_proto::film::TelemetryHooks::UNIFIED,
        render_hooks: super::compiled_render_hooks(),
        player_features: auto,
    };
    crate::theater::Session::open(Some(dir.join(name)), None, facts, b"xbe", &opts)
        .unwrap()
        .unwrap()
}

fn on_features(registry: hooks::Registry) -> std::collections::BTreeSet<&'static str> {
    registry
        .build(&hooks::Base)
        .unwrap()
        .features
        .iter()
        .filter(|f| f.off.is_none())
        .map(|f| f.id)
        .collect()
}

/// A recording session with the in-game Theater plans exactly one feature
/// with the film observer's id; the Theater's screens once took it too and
/// every autorecorded game failed its plan.
#[test]
fn a_recording_with_the_theater_plans_one_film_observer() {
    let dir = std::env::temp_dir().join(format!("h2-registration-observer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let case = Case::full("");
    assert!(case.theater);
    let profile = super::compiled_render_hooks();
    let auto = recording(&dir, "auto.h2film", true);
    let plan = registry_with_film(&case, profile, Some(auto))
        .build(&hooks::Base)
        .map_err(|conflicts| {
            conflicts
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap();
    let observers: Vec<_> = plan
        .features
        .iter()
        .filter(|f| f.id == crate::theater::OBSERVER_FEATURE)
        .collect();
    assert_eq!(observers.len(), 1);
    assert!(observers[0].off.is_none(), "{:?}", observers[0].off);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A default recording turns nothing off: Live session links, the Custom
/// Game Browser advertisements, co-op over Live and the rest of a signed-in
/// session's features are planned as with no film. A film the player asked
/// for keeps its old rule and plans neither the links nor the
/// advertisements, on recording and playback alike.
#[test]
fn a_default_recording_plans_every_feature_a_session_without_a_film_plans() {
    let dir = std::env::temp_dir().join(format!("h2-registration-film-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let case = Case { coop_live: true, ..Case::full("") };
    let profile = super::compiled_render_hooks();
    let without = on_features(registry_for_profile(&case, profile));
    assert!(without.contains("live-join-link") && without.contains("live-browser-ads"));

    let auto = recording(&dir, "auto.h2film", true);
    assert!(auto.player_features());
    let with_auto = on_features(registry_with_film(&case, profile, Some(auto)));
    let missing: Vec<_> = without.difference(&with_auto).collect();
    assert!(missing.is_empty(), "a default recording turned off {missing:?}");

    let explicit = recording(&dir, "explicit.h2film", false);
    assert!(!explicit.player_features());
    let with_explicit = on_features(registry_with_film(&case, profile, Some(explicit)));
    assert!(!with_explicit.contains("live-join-link"));
    assert!(!with_explicit.contains("live-browser-ads"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What a film's hook check compares (`theater::compatibility`'s hook
/// digest), by name rather than placed address: the features on, their
/// hooks by point and kind, their guest allocations and entries. Features
/// that only draw are left out, as the check leaves them out.
fn simulation_hooks(registry: hooks::Registry) -> std::collections::BTreeSet<String> {
    let plan = registry.build(&hooks::Base).unwrap();
    let counted = |id: &str| {
        plan.features
            .iter()
            .any(|f| f.id == id && f.off.is_none() && !f.presentation_only)
    };
    let mut rows = std::collections::BTreeSet::new();
    for feature in plan.features.iter().filter(|f| counted(f.id)) {
        rows.insert(format!("feature {}", feature.id));
    }
    for chain in plan.chains.values() {
        for hook in chain.hooks().filter(|h| counted(h.feature)) {
            rows.insert(format!(
                "hook {} {} {}",
                chain.point.name,
                hook.feature,
                hook.kind().as_str()
            ));
        }
    }
    for alloc in plan.allocs.iter().filter(|a| !a.presentation_only && counted(a.feature)) {
        rows.insert(format!("allocation {} {}", alloc.feature, alloc.len));
    }
    for entry in plan.entries.iter().filter(|e| counted(e.feature)) {
        rows.insert(format!("entry {}", entry.feature));
    }
    rows
}

/// A default recording's replay installs the recording's hooks, so Theater
/// can play it: boot's film decisions (`non_player` for the Custom Game
/// Browser's filters, `local_forge` for Forge) follow the film's header,
/// never whether this boot records or plays back. The replay side is
/// Theater's playback child (`replaying`) holding a film whose header keeps
/// the player features, as a default recording's replay does
/// (`theater::autorecord`'s tests open one). A film the player named plans
/// neither Forge nor the filters, on either side.
#[test]
fn a_default_recordings_replay_installs_the_recordings_hooks() {
    let dir = std::env::temp_dir().join(format!("h2-registration-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let case = Case { coop_live: true, ..Case::full("") };
    let replay = Case { replaying: true, ..case.clone() };
    let profile = super::compiled_render_hooks();
    for (name, auto) in [("auto.h2film", true), ("explicit.h2film", false)] {
        let film = recording(&dir, name, auto);
        assert_eq!(film.player_features(), auto);
        let recorded = registry_with_film(&case, profile, Some(Arc::clone(&film)));
        let forge = on_features(recorded).contains("forge");
        assert_eq!(forge, auto, "{name}: Forge planned while recording");
        let recorded = simulation_hooks(registry_with_film(&case, profile, Some(Arc::clone(&film))));
        let replayed = simulation_hooks(registry_with_film(&replay, profile, Some(film)));
        let only_recorded: Vec<_> = recorded.difference(&replayed).collect();
        let only_replayed: Vec<_> = replayed.difference(&recorded).collect();
        assert!(
            only_recorded.is_empty() && only_replayed.is_empty(),
            "{name}: recorded but not replayed {only_recorded:?}; replayed but not recorded {only_replayed:?}"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The film's hook digest (`theater::hook_digest`) of `registry`'s plan,
/// every address placed as install places it on a full image.
fn hook_digest(registry: hooks::Registry) -> String {
    let plan = registry.build(&hooks::Base).unwrap();
    let placed = plan
        .chains
        .values()
        .flat_map(|c| [Some(c.addr), c.shadow])
        .flatten()
        .chain(plan.entries.iter().map(|e| e.addr))
        .collect();
    let installed = hooks::Installed { placed, ..Default::default() };
    crate::theater::hook_digest(&plan, &installed)
}

/// `h2 play` records in a window that takes lab requests; `h2 theater
/// play` replays it in Theater's window or with none (`--render-every 0`).
/// Each replay's digest is the recording's, or the replay refuses the
/// film ("Installed simulation hooks differ from the film"): the lab
/// takeover's dialog was planned only in a window that takes requests, so
/// every replay of a windowed recording refused it.
#[test]
fn a_recording_and_its_replays_have_one_hook_digest() {
    let dir = std::env::temp_dir().join(format!("h2-registration-digest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let profile = super::compiled_render_hooks();
    let play = Case { coop_live: true, ..Case::full("") };
    assert!(play.takeover && play.xapi);
    let window = Case { replaying: true, ..play.clone() };
    let no_window = Case { takeover: false, ..window.clone() };
    for (name, auto) in [("auto.h2film", true), ("explicit.h2film", false)] {
        let film = recording(&dir, name, auto);
        let digest = |case: &Case| hook_digest(registry_with_film(case, profile, Some(Arc::clone(&film))));
        let recorded = digest(&play);
        assert_eq!(digest(&window), recorded, "{name}: Theater's window");
        assert_eq!(digest(&no_window), recorded, "{name}: no window");
        assert!(on_features(registry_with_film(&window, profile, Some(Arc::clone(&film)))).contains("lab-takeover"));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Pads plan the lab takeover's dialog; only a player's window that is not
/// a replay takes a lab's request.
#[test]
fn the_lab_takeover_plans_with_pads_and_accepts_in_the_players_window() {
    use super::{Takeover, TakeoverInputs, lab_takeover};
    let play = TakeoverInputs {
        window: true,
        d3d: true,
        xapi: true,
        lockstep: false,
        host_light: false,
        sim: false,
        replaying: false,
    };
    assert_eq!(lab_takeover(play), Takeover { plan: true, accept: true });
    for replay in [
        TakeoverInputs { replaying: true, ..play },
        TakeoverInputs { replaying: true, window: false, ..play },
        TakeoverInputs { window: false, ..play },
    ] {
        assert_eq!(lab_takeover(replay), Takeover { plan: true, accept: false }, "{replay:?}");
    }
    for off in [
        TakeoverInputs { xapi: false, ..play },
        TakeoverInputs { d3d: false, ..play },
        TakeoverInputs { lockstep: true, ..play },
        TakeoverInputs { host_light: true, ..play },
        TakeoverInputs { sim: true, ..play },
    ] {
        assert_eq!(lab_takeover(off), Takeover { plan: false, accept: false }, "{off:?}");
    }
}

#[test]
fn the_films_header_not_the_boot_decides_forge_and_the_non_player_features() {
    let dir = std::env::temp_dir().join(format!("h2-registration-decide-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let on = hooks::Switches::parse("").unwrap();
    let auto = recording(&dir, "auto.h2film", true);
    let named = recording(&dir, "named.h2film", false);
    for replaying in [false, true] {
        assert!(!super::non_player(Some(&*auto), replaying, false, false));
        assert!(super::non_player(Some(&*named), replaying, false, false));
        assert!(super::local_forge(&on, Some(&*auto), replaying, false, false));
        assert!(!super::local_forge(&on, Some(&*named), replaying, false, false));
    }
    // With no film, Theater's child and hosts are the non-player boots.
    assert!(!super::non_player(None, false, false, false));
    assert!(super::non_player(None, true, false, false));
    assert!(super::non_player(Some(&*auto), false, true, false));
    assert!(super::non_player(Some(&*auto), false, false, true));
    assert!(super::local_forge(&on, None, false, false, false));
    assert!(!super::local_forge(&on, None, true, false, false));
    assert!(!super::local_forge(&on, Some(&*auto), false, true, false));
    assert!(!super::local_forge(&on, Some(&*auto), false, false, true));
    let off = hooks::Switches::parse("-forge").unwrap();
    assert!(!super::local_forge(&off, Some(&*auto), false, false, false));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The features with a hook at `point` in `registry`'s plan, switched on.
fn hooked_at(registry: hooks::Registry, point: &str) -> Vec<&'static str> {
    let plan = registry.build(&hooks::Base).unwrap();
    let mut ids: Vec<_> = plan.chains
        .values()
        .filter(|chain| chain.point.name == point)
        .flat_map(|chain| chain.hooks().map(|hook| hook.feature))
        .filter(|id| plan.features.iter().any(|f| f.id == *id && f.off.is_none()))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// A default play (autorecorded, signed in to Live, in a window) fits the
/// guest camera to the window and renders on the private random stream:
/// neither the film nor the Live HLE turns either off. Lockstep keeps the
/// retail camera and the retail stream (docs/graphics/high-resolution.md,
/// "Widescreen projection" and "Determinism").
#[cfg(feature = "gpu")]
#[test]
fn a_default_play_fits_the_guest_camera_and_renders_on_its_own_stream() {
    let dir = std::env::temp_dir().join(format!("h2-registration-lens-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let profile = super::compiled_render_hooks();
    let play = Case { coop_live: true, ..Case::full("") };
    assert!(play.live && play.drawing);
    let auto = recording(&dir, "auto.h2film", true);
    let registry = || registry_with_film(&play, profile, Some(Arc::clone(&auto)));
    assert_eq!(hooked_at(registry(), "VIEW_PROJECTION_TERMS"), ["render-aspect"]);
    assert!(hooked_at(registry(), "RENDER_FRAME").contains(&crate::render_rng::ID));

    let lockstep = Case { lockstep: true, ..play.clone() };
    assert!(hooked_at(registry_for_profile(&lockstep, profile), "VIEW_PROJECTION_TERMS").is_empty());
    assert!(!hooked_at(registry_for_profile(&lockstep, profile), "RENDER_FRAME").contains(&crate::render_rng::ID));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A recording and its replay register the render stream alike, so the
/// film's simulation hooks match whatever window either had.
#[test]
fn a_replay_renders_on_the_stream_its_recording_did() {
    let profile = super::compiled_render_hooks();
    let play = Case { coop_live: true, ..Case::full("") };
    let replay = Case { replaying: true, ..play.clone() };
    for case in [&play, &replay] {
        assert!(hooked_at(registry_for_profile(case, profile), "RENDER_FRAME").contains(&crate::render_rng::ID));
    }
}
