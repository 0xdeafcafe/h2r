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

/// Every switch that adds hooks, on (the native renderer is on by default).
pub(crate) const ALL_ON: &str = "impl.engine=on,render-timing,actor-net,firefight";
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
    /// `H2_LOCKSTEP`: the tick observer, and no pipelined render.
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
    /// The windowed game with player input: the lab takeover consent and
    /// its dialog (`AgentGlue::configure_takeover`, needs `agent`).
    pub takeover: bool,
    /// The light host: the viewer skips, the native sim, `sim-frame`, no
    /// window (so no drawing, takeover or return to the menu) and no
    /// non-player Live features.
    pub light: bool,
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
    let rt = Arc::new(runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default()));
    let settings = case
        .settings
        .then(|| Arc::new(settings::Store::defaults(crate::setting_defs::ALL)));
    let mut features = hooks::Registry::new(super::profile_switches(
        hooks::Switches::parse(&case.switches).unwrap(),
        case.light,
    ));
    let nowhere = Path::new("");
    let live = case.live.then(|| live_hle::Hle::new(live_config()).unwrap());
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
    let telemetry = if case.telemetry {
        Ok(crate::telemetry::TelemetryConfig::file(std::env::temp_dir().join("h2-registration-test.telemetry")))
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
        None,
        case.light,
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
            Arc::new(|| 0),
            Arc::new(NoDevice),
        )
        .unwrap();
    }
    let lockstep = lockstep_glue::Config {
        trace: case.lockstep.then(|| PathBuf::from("h2-registration-test.lockstep")),
        ticks: true,
        record: None,
        replay: None,
    };
    let lockstep_on = lockstep.enabled();
    let pipeline = super::pipeline_for(
        render_bridge::pipeline_render::PipelineRender::with(true, render_bridge::pipeline_render::Addrs::HALO2),
        lockstep_on,
        case.light,
    );
    let mut late = register_late_for_profile(
        &mut features,
        LateInputs {
            #[cfg(feature = "gpu")]
            view: windowed.then_some(&crate::d3d_glue::DrawingView { scale_stats: None }),
            #[cfg(feature = "gpu")]
            fixed_lod: None,
            #[cfg(feature = "gpu")]
            world_line_present: None,
            rt: &rt,
            lockstep,
            agent: if case.agent { AgentSource::Local } else { AgentSource::None },
            allow_bot_add: !case.light || core.bots_on,
            xa: None,
            pipeline,
            coop_probe: case
                .coop_probe
                .then(|| crate::coop_probe::parse(|k| (k == "H2_COOP_PROBE").then(|| "1".to_owned())).unwrap().unwrap()),
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
    if features.switches().feature("forge", false) && !case.light && !lockstep_on {
        crate::forge_glue::install(&mut features, late.agent.as_ref(), None, nowhere, None).unwrap();
    }
    if let Some(a) = &mut late.agent {
        a.configure_takeover(&mut features, case.takeover && !case.light, case.takeover && !case.light)
            .unwrap();
    }
    if let (Some(theater), Some(a)) = (&theater, &late.agent) {
        theater.connect(&a.agent);
    }
    if case.light {
        features.add(&sim_bridge::sim_native::SimNative::new(&rt)).unwrap();
    }
    super::register_viewer_skips(&mut features, case.light).unwrap();
    if case.light {
        assert!(features.add(&sim_bridge::sim_frame::SimFrame::new(0)).unwrap(), "light host requires sim-frame");
    }
    if let Some(a) = &late.agent {
        crate::host_metrics::install(&mut features, &a.agent, case.light).unwrap();
    }
    #[cfg(feature = "gpu")]
    if windowed && !lockstep_on {
        features.add(&crate::return_to_menu::ReturnToMenu(Arc::new(|_| {}))).unwrap();
    }
    features
}

/// Builds the plan for `case`; the refusal, if any, says which features clash.
fn plan(case: &Case) -> Result<hooks::Plan, String> {
    registry(case).build(&hooks::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        format!("feature plan refused: {}\n  for {}", c.join("; "), case.describe())
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
        (ALL_ON.to_owned(), false),
        (format!("{ALL_ON},{FEATURE}"), true),
    ] {
        let mut case = Case::full(&switches);
        case.coop_live = true;
        let plan = plan(&case).unwrap();
        let installed = |id| {
            plan.chains.values().any(|chain| chain.hooks().any(|hook| hook.feature == id))
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
            assert!(plan.chains.values().flat_map(|c| c.hooks()).all(|h| {
                h.feature != "render-loop" && !h.feature.starts_with("native-")
            }));
        }
    }
}

#[test]
fn actor_capture_is_opt_in_and_observes_the_host_tick_and_dialogue() {
    for (switches, enabled) in [("", false), (ALL_ON, true)] {
        let plan = plan(&Case::full(switches)).unwrap();
        let mut hooks: Vec<_> = plan.chains.values().flat_map(|chain| {
            chain.hooks().filter(|hook| hook.feature == "actor-net")
                .map(move |hook| (chain.point.name, hook.kind().as_str()))
        }).collect();
        hooks.sort_unstable();
        if enabled {
            assert_eq!(hooks, [
                ("AI_DIALOGUE_ENQUEUE", "observe_after"),
                ("AI_DIALOGUE_ENQUEUE", "observe_before"),
                ("GAME_ACTIVATE", "observe_after"),
                ("GAME_TICK", "observe_after"),
            ]);
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
            let plan = registry_for_profile(&case, profile).build(&hooks::Base).unwrap();
            let enabled = profile == Gpu && drawing;
            assert_eq!(plan.features.iter().any(|feature| feature.id == "lod-distance"), enabled);
            assert_eq!(plan.chains.values().any(|chain| chain.point.name == "MODEL_LOD"), enabled);
        }
    }
    // The guest chooser still runs when either set of native replacements
    // is switched off. Disabling the LOD feature disables both adapters.
    for (switches, enabled) in [
        (NATIVE_OFF, true),
        ("-native-scene-lists", true),
        ("-lod-distance", false),
    ] {
        let plan = registry_for_profile(&Case::full(switches), Gpu).build(&hooks::Base).unwrap();
        for point in ["MODEL_LOD", "RESET_SCENE_RECORDS"] {
            assert_eq!(plan.chains.values().any(|chain| chain.point.name == point), enabled);
        }
    }
}

#[test]
fn full_profile_allows_explicit_pipelining_for_tick_hashes() {
    use render_bridge::pipeline_render::{Addrs, PipelineRender, SWITCH};

    for (switches, lockstep, light, expected) in [
        ("", false, false, false),
        ("impl.render.native=on", false, false, false),
        (SWITCH, false, false, true),
        (
            "impl.render.native=on,impl.render.pipelined_frame=on",
            false,
            false,
            true,
        ),
        (SWITCH, false, true, false),
        (SWITCH, true, false, false),
    ] {
        let mut features = hooks::Registry::new(hooks::Switches::parse(switches).unwrap());
        let pipeline = super::pipeline_for(
            PipelineRender::with(false, Addrs::HALO2),
            lockstep,
            light,
        );
        features.add(&pipeline).unwrap();
        let plan = features.build(&hooks::Base).unwrap();
        assert_eq!(
            plan.chains.contains_key(&hooks::points::RENDER_FRAME.base),
            expected,
            "{switches}, lockstep={lockstep}, light={light}"
        );
    }
}

/// Boot with nothing optional: no settings, no XAPI, default switches.
#[test]
fn the_plan_builds_with_default_switches() {
    assert_builds(&Case::none(""));
    assert_builds(&Case::full(""));
}

/// Every switch that adds hooks on (the census in a second build, since
/// pipelining is off with it, and the native renderer off in a third), with
/// everything a test can supply.
#[test]
fn the_plan_builds_with_every_hook_switch_on() {
    assert_builds(&Case::none(ALL_ON));
    assert_builds(&Case::full(ALL_ON));
    assert_builds(&Case::full(&format!("{ALL_ON},render-census")));
    assert_builds(&Case::full(&format!("{ALL_ON},{NATIVE_OFF}")));
    // Forge is off by default; boot installs it with its switch on.
    assert_builds(&Case::full(&format!("{ALL_ON},forge")));
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
    let missing: Vec<_> = expected.iter().copied().filter(|id| !ids.contains(id)).collect();
    assert!(missing.is_empty(), "the full case leaves out {missing:?}; planned: {ids:?}");

    // The light host's registrations, and Forge with its switch.
    let mut light = Case::full("");
    light.light = true;
    let light = plan(&light).unwrap();
    assert!(light.features.iter().any(|f| f.id == sim_bridge::sim_frame::ID));
    let forge = plan(&Case::full("forge")).unwrap();
    assert!(forge.features.iter().any(|f| f.id == "forge"));
}

/// Where boot registers features outside `register_core` and
/// `register_late` (`lib.rs`, `run_hosted_with_live`), with the plan test
/// that covers each, or why none can. Boot's `&mut features` and
/// `features.add(` sites are counted below: a new one fails the count, and
/// the fix is a row here and, if a test can build it without a disc, a
/// step in `registry_for_profile`.
const BOOT_SITES: &[(&str, &str)] = &[
    ("campaign_cache::configure", "not covered: its limit feature is private and only added when a map outgrows retail's cache slot"),
    ("Theater::install", "Case::theater"),
    ("register_core_film", "registry_for_profile (a film's session is not covered: it needs a recording)"),
    ("render_aspect::install", "Case::drawing (gpu)"),
    ("register_late_for_profile", "registry_for_profile"),
    ("forge_glue::install", "the forge switch"),
    ("AgentGlue::configure_takeover", "Case::takeover"),
    ("dsound_glue::add_loop_agent_method", "not covered: needs the audio device (agent-audio)"),
    ("register_viewer_skips", "Case::light"),
    ("host_metrics::install", "Case::agent"),
    ("host_lobby::install", "not covered: env-only (H2_HOST_LOBBY) and needs the run options"),
    ("Sim::install_with_film", "not covered: needs a sim session"),
    ("Attract::install", "not covered: its constructor is crate-private to game and needs the bink HLE"),
    ("SimNative", "Case::light"),
    ("SimFrame", "Case::light"),
    ("ReturnToMenu", "Case::drawing (gpu)"),
    ("host audit", "not covered: needs the generated module and the running stamp"),
    ("theater::Observer", "not covered: needs a film session"),
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
    assert!(failures.is_empty(), "{} cases refused:\n{}", failures.len(), dedup(failures).join("\n"));
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
fn light_native_subsystems_follow_the_profile_and_explicit_overrides() {
    use engine::points::{COLLISION_LOCATION, OBJECT_FUNCTION_VALUE, SIM_ANIMATION_STREAM};
    let runtime = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
    let points = [&SIM_ANIMATION_STREAM, &COLLISION_LOCATION, &OBJECT_FUNCTION_VALUE];
    for (light, spec, expected) in [
        (true, "", [false; 3]),
        (false, "host-light-native", [false; 3]),
        (true, "host-light-native", [true; 3]),
        (true, "-host-light-native,impl.sim.native.anim", [true, false, false]),
        (true, "host-light-native,-impl.sim.native.collision", [true, false, true]),
        (true, "host-light-native,-impl.sim.native", [false; 3]),
        (true, "host-light-native,-impl.sim,impl.sim.native.objects", [false, false, true]),
    ] {
        let switches = super::profile_switches(hooks::Switches::parse(spec).unwrap(), light);
        assert!(!switches.implementation("sim.native_unit_pose", false));
        let mut registry = hooks::Registry::new(switches);
        registry.add(&sim_bridge::sim_native::SimNative::new(&runtime)).unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        assert_eq!(
            points.map(|p| plan.chains.get(&p.base).is_some_and(|c| c.implement.is_some())),
            expected,
            "{light} {spec}"
        );
    }
}

#[test]
fn viewer_skips_follow_the_light_profile_and_independent_switches() {
    use engine::points::{PREDICT_RESOURCES, SOUND_SUBMIX_FX};
    let points = [&PREDICT_RESOURCES, &SOUND_SUBMIX_FX];
    for (light, spec, expected) in [
        (false, "", [false; 2]),
        (false, "impl.sim=on", [false; 2]),
        (true, "", [true; 2]),
        (true, "-impl.sim.observer-resources", [false, true]),
        (true, "-impl.sim.submix-fx", [true, false]),
        (true, "-impl.sim", [false; 2]),
    ] {
        let mut registry = hooks::Registry::new(hooks::Switches::parse(spec).unwrap());
        super::register_viewer_skips(&mut registry, light).unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        assert_eq!(
            points.map(|p| plan.chains.contains_key(&p.base)),
            expected,
            "{light} {spec}"
        );
    }
}

/// The pipelined render thread's refusals (`render_thread::REFUSED`),
/// checked against the whole plan this module builds.
#[cfg(feature = "desktop-runtime")]
mod render_thread_plan {
    use std::sync::Arc;

    use super::{ALL_ON, Case, NATIVE_OFF, registry};
    use crate::render_thread::REFUSED;

    /// The campaign picker's state over the two fixture campaigns, as
    /// `campaign_menu`'s own tests build it.
    fn campaign_state() -> crate::campaigns::State {
        use content::campaign_list::{self as cl, CampaignManifest, FIXTURE, HALO1};
        let ms = [CampaignManifest::parse(HALO1).unwrap(), CampaignManifest::parse(FIXTURE).unwrap()];
        crate::campaigns::State {
            built: cl::build(&ms, &[1], &[0x65]).unwrap(),
            selection: crate::campaigns::Selection::default(),
            progress: crate::campaigns::Progress::memory(),
            cards: Default::default(),
        }
    }

    fn refused(feature: &str, point: &str) -> Option<usize> {
        REFUSED
            .iter()
            .position(|&(f, p, _)| f == feature && (p == "*" || p == point))
    }

    /// Every hook boot's features place, with every switch that adds one
    /// on (and the census in a second build, since pipelining is off with
    /// it), either runs on the pipelined render thread or is listed in
    /// [`REFUSED`] with its reason, and every listed entry is still used.
    /// The ones the render path meets in a default run (the locale lookups,
    /// Blind, the native world's and objects' layer draw) are context-only,
    /// and so are the native transparents' geometry handler, the native
    /// lights' passes, and the dynamic lights' call (the frame step reads
    /// the light candidates before the decal mode after it, not on the
    /// lights' return). The native post's colour pass and screen effect are
    /// not: their return hooks commit or replay the held draws.
    #[test]
    fn every_feature_hook_runs_on_the_render_thread_or_says_why_not() {
        let mut unlisted = Vec::new();
        let mut used = vec![false; REFUSED.len()];
        let mut safe = Vec::new();
        // The empty switch string places every native stage (on by default
        // since 2026-11-20): the native world and objects at the layer draw,
        // and the transparents' handler, with nothing else there, are
        // render-thread safe together, and so are the lights' skip and view
        // observer (placed only without render-timing) and the HUD's
        // screen-quad and HUD-quad befores.
        for switches in [
            String::new(),
            format!("{ALL_ON},{NATIVE_OFF}"),
            ALL_ON.to_owned(),
            format!("{ALL_ON},render-census"),
        ] {
            let mut features = registry(&Case::full(&switches));
            features.add(&crate::campaign_menu::Campaigns {
                state: Arc::new(campaign_state()),
                picker: Arc::new(|| 0),
            }).unwrap();
            let plan = features.build(&hooks::Base).unwrap();
            for c in plan.chains.values() {
                match c.render_thread() {
                    Ok(()) => safe.push(c.point.name),
                    Err(hooks) => {
                        for (h, why) in hooks {
                            match refused(h.feature, c.point.name) {
                                Some(i) => used[i] = true,
                                None => unlisted.push(format!(
                                    "{} {} {}:{} ({why})",
                                    c.point.name,
                                    h.kind().as_str(),
                                    h.feature,
                                    h.label
                                )),
                            }
                        }
                    }
                }
            }
        }
        assert!(
            unlisted.is_empty(),
            "hooks the pipelined render would fall back on; mark them context_only or list them in REFUSED:\n{}",
            unlisted.join("\n")
        );
        let stale: Vec<_> = REFUSED
            .iter()
            .zip(&used)
            .filter(|(_, u)| !**u)
            // Boot adds agent-audio only with the agent and live audio
            // (dsound_glue::add_loop_agent_method), which this plan omits.
            .filter(|((f, _, _), _)| *f != "agent-audio")
            .map(|(r, _)| r)
            .collect();
        assert!(stale.is_empty(), "listed but not placed: {stale:?}");
        for p in [
            "LOCALE_FIND",
            "LOCALE_EXISTS",
            "HUD_SHOWN",
            "FIRST_PERSON_RENDER",
            "UI_QUAD_TO_SCREEN",
        ] {
            assert!(safe.contains(&p), "{p} runs on the render thread");
        }
        #[cfg(feature = "gpu")]
        for p in [
            "LAYER_DRAW",
            "TRANSPARENT_GEOMETRY",
            "SCENE_TRANSPARENTS",
            "SCENE_TRANSPARENTS_END",
            "DECAL_RENDER",
            "DECAL_RENDER_END",
            "RENDER_LIGHT_PASSES",
            "LIGHT_GEOMETRY_DRAW",
            "RENDER_DYNAMIC_LIGHTS",
            "RENDER_DECAL_MODE",
        ] {
            assert!(safe.contains(&p), "{p} runs on the render thread");
        }
    }
}
