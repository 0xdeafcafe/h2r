//! The feature plan, built from the registration code boot runs.
//!
//! A clash (two features hooking one point the same way with no order
//! between them) makes boot refuse the plan and the game stop at launch.
//! These tests call [`register_core`](super::register_core) and
//! [`register_late`](super::register_late) with test inputs and build the
//! plan, so the clash fails here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{AgentSource, CoreInputs, LateInputs, register_core, register_late_for_profile};
use crate::lockstep_glue;

/// Every switch that adds hooks, on (the native renderer is on by default).
pub(crate) const ALL_ON: &str = "impl.engine=on,render-timing";
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
        }
    }

    /// Everything a test can supply except the runs that exclude each other
    /// (lockstep and the co-op probe).
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

fn registry_for_profile(case: &Case, profile: film::RenderHooks) -> hooks::Registry {
    let rt = Arc::new(runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default()));
    let settings = case
        .settings
        .then(|| Arc::new(settings::Store::defaults(crate::setting_defs::ALL)));
    let mut features = hooks::Registry::new(hooks::Switches::parse(&case.switches).unwrap());
    let nowhere = Path::new("");
    let telemetry = if case.telemetry {
        Ok(crate::telemetry::TelemetryConfig::file(std::env::temp_dir().join("h2-registration-test.telemetry")))
    } else {
        Ok(crate::telemetry::TelemetryConfig::default())
    };
    register_core(
        &mut features,
        CoreInputs {
            theater: None,
            rt: &rt,
            telemetry,
            vfs: case.vfs.then(|| Arc::new(Mutex::new(vfs::Vfs::new()))),
            model: None,
            campaigns: None,
            settings: settings.clone(),
            live: None,
            data_root: nowhere,
            packages: nowhere,
            level: None,
            perf_level: case.level,
            bots: case.xapi,
        },
    )
    .unwrap();
    let lockstep = lockstep_glue::Config {
        trace: case.lockstep.then(|| PathBuf::from("h2-registration-test.lockstep")),
        ticks: true,
        record: None,
        replay: None,
    };
    let pipeline = super::pipeline_for(
        render_bridge::pipeline_render::PipelineRender::with(true, render_bridge::pipeline_render::Addrs::HALO2),
        lockstep.enabled(),
        false,
    );
    register_late_for_profile(
        &mut features,
        LateInputs {
            #[cfg(feature = "gpu")]
            view: case.drawing.then_some(&crate::d3d_glue::DrawingView { scale_stats: None }),
            rt: &rt,
            lockstep,
            agent: if case.agent { AgentSource::Local } else { AgentSource::None },
            allow_bot_add: true,
            xa: None,
            pipeline,
            coop_probe: case
                .coop_probe
                .then(|| crate::coop_probe::parse(|k| (k == "H2_COOP_PROBE").then(|| "1".to_owned())).unwrap().unwrap()),
            live: None,
            settings,
            map_gate: None,
            disc: nowhere,
            data_root: nowhere,
        },
        profile,
    )
    .unwrap();
    // Boot's Live branch (registration.rs): the armed co-op hooks, unless
    // the probe holds them.
    if case.coop_live && !case.coop_probe {
        crate::coop_probe::install_live(&rt, &mut features, nowhere).unwrap();
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
}

/// Each optional piece alone and in every combination, with the default
/// switches and with every hook switch on: skulls and the Halo 1 options
/// (a settings store), bots and the Clankers page (XAPI), map slots and our
/// skulls' map reads (a VFS), telemetry, the level boot, our cheats (an
/// agent), the lockstep harness and the co-op probe.
#[test]
fn the_plan_builds_with_every_combination_of_optional_features() {
    let mut failures = Vec::new();
    let native_off = format!("{ALL_ON},{NATIVE_OFF}");
    for switches in ["", ALL_ON, native_off.as_str()] {
        for bits in 0u32..(1 << 9) {
            let on = |n: u32| bits & (1 << n) != 0;
            let case = Case {
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
            };
            if let Err(e) = plan(&case) {
                failures.push(e);
            }
        }
    }
    assert!(failures.is_empty(), "{} cases refused:\n{}", failures.len(), dedup(failures).join("\n"));
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
    /// and so are the native transparents' geometry handler and the native
    /// lights' passes. The native post's colour pass and screen effect are
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
        ] {
            assert!(safe.contains(&p), "{p} runs on the render thread");
        }
    }
}
