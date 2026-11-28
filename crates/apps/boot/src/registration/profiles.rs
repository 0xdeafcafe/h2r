//! The registry boot hands the plan, per profile, built with no game
//! running: the plan tests' cases (`registration::tests`) and the lift's
//! replaced list ([`replaced_entries`], docs/lifter/lift-pruning.md). Each
//! step mirrors `run_hosted_with_live`; a registration that fails here
//! panics, as the same boot would refuse it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hooks::points::Point;

use super::{AgentSource, CoreInputs, LateInputs, register_core_film, register_late_for_profile};
use crate::lockstep_glue;

/// The function entries in `entries` whose guest bodies no boot profile
/// runs, so the lift may leave them out (docs/lifter/lift-pruning.md, "The
/// replaced list"): [`hooks::Plan::replaced`] in the windowed game, Theater
/// playback and the headless host, each with default switches. One image
/// serves all three, so an entry kept in any of them is kept.
pub fn replaced_entries(entries: &BTreeSet<u32>) -> BTreeMap<u32, &'static Point> {
    hooks::replaced_in_every(&profile_plans(), entries)
}

/// [`replaced_entries`] with each entry's replacing feature, and the guest
/// functions the on features' Rust calls in any profile, which the lift
/// keeps: what boot registers for the build
/// (`recomp::register_lift_replaced`, docs/lifter/lift-pruning.md).
pub fn lift_replaced(entries: &BTreeSet<u32>) -> emit::pruned::Replaced {
    let plans = profile_plans();
    let feature = |addr: u32| {
        plans
            .iter()
            .find_map(|p| p.chains.get(&addr)?.implement.as_ref().map(|h| h.feature))
            .unwrap_or("?")
    };
    let entries = hooks::replaced_in_every(&plans, entries)
        .into_iter()
        .map(|(entry, p)| {
            let root = emit::pruned::Root { entry, point: p.name.to_owned(), feature: feature(entry).to_owned() };
            (entry, root)
        })
        .collect();
    let calls = plans.iter().flat_map(|p| p.calls_guest.keys().copied()).collect();
    emit::pruned::Replaced { entries, calls }
}

/// Boot's check of a pruned module against this boot's plan: every root
/// the lift cut must still be replaced, or the left-out body could be
/// reached. Refuses naming the point and its feature, before any guest
/// code runs (docs/lifter/lift-pruning.md, "Boot safety").
pub fn check_pruned(roots: &[emit::pruned::Root], plan: &hooks::Plan) -> anyhow::Result<()> {
    let entries: BTreeSet<u32> = roots.iter().map(|r| r.entry).collect();
    let live = plan.replaced(&entries);
    let missing: Vec<String> = roots
        .iter()
        .filter(|r| !live.contains_key(&r.entry))
        .map(|r| {
            let why = match plan.features.iter().find(|f| f.id == r.feature) {
                Some(f) => match &f.off {
                    Some(why) => format!("{} is off ({why})", r.feature),
                    None => format!("{} no longer replaces it", r.feature),
                },
                None => format!("{} is not in this boot's plan", r.feature),
            };
            format!("{} ({:#010x}): {why}", r.point, r.entry)
        })
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "the lift left out guest code this boot would run: {}. Turn the feature back on, or rebuild with H2_LIFT_PRUNE=0",
        missing.join("; ")
    )
}

/// The windowed game's, Theater playback's and the headless host's plans,
/// each with default switches.
fn profile_plans() -> Vec<hooks::Plan> {
    let window = Case { coop_live: true, telemetry: false, ..Case::full("") };
    let playback = Case { replaying: true, ..window.clone() };
    let host = Case {
        light: true,
        takeover: false,
        host_lobby: true,
        telemetry: false,
        ..Case::full("")
    };
    [window, playback, host]
        .iter()
        .map(|case| {
            registry(case)
                .build(&hooks::Base)
                .unwrap_or_else(|c| panic!("{case:?}: the plan refused: {} conflicts", c.len()))
        })
        .collect()
}

/// What a boot has, from nothing (a headless run with no settings) to
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
    fn set_fit(&self, _: &mut runtime::Guest, _: Option<gfx::external::UiFit>) {}
}

pub(crate) fn registry_for_profile(case: &Case, profile: film::RenderHooks) -> hooks::Registry {
    registry_with_film(case, profile, None)
}

/// [`registry_for_profile`] for a boot that records `film`.
pub(crate) fn registry_with_film(
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
    // Boot's render stream: every rendering session, lockstep's included.
    if !case.light {
        crate::render_rng::install(&mut features).unwrap();
        crate::render_scratch::install(&mut features).unwrap();
        crate::flare_store::install(&mut features).unwrap();
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
    let mut late = register_late_for_profile(
        &mut features,
        LateInputs {
            #[cfg(feature = "gpu")]
            output: windowed.then(|| std::sync::Arc::new(|| crate::setting_defs::GUEST_FRAME) as crate::run_opts::NativeSize),
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
        features
            .add(&sim_bridge::sim_lightmap::LightmapStore)
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
    // Boot's frame step, after sim-frame: render_frame_and_present as Rust
    // (a light host's sim-frame owns the point).
    #[cfg(feature = "gpu")]
    if !case.light {
        features
            .add(&render_bridge::render_frame::RenderFrame::default())
            .unwrap();
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
