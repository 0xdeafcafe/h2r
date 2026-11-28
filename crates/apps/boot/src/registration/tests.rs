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
use std::sync::Arc;

pub(crate) use super::profiles::{Case, registry};
use super::profiles::{registry_for_profile, registry_with_film};

/// Every switch that adds hooks, on (the sim's sound events, which hooks
/// points other features hook, included; the native renderer, the engine leaves,
/// Forge, the actor capture, Firefight and the creature probe are on by
/// default; only the diagnostics and `impl.engine=on` are spelled out;
/// `rules`, on by default too, stays named).
pub(crate) const ALL_ON: &str = "impl.engine=on,render-timing,actor-net,firefight,rules,impl.sim.sound-events";
/// Added to [`ALL_ON`]: the native renderer (on by default) off.
pub(crate) const NATIVE_OFF: &str = "impl.render.native=off";

impl Case {
    fn describe(&self) -> String {
        format!("{self:?}")
    }
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
    // The sound table in place of the events arm: every entry it answers,
    // the listeners' before first among the bots' map swap.
    assert_builds(&Case::none(&format!("{ALL_ON},impl.sim.sound-events.table")));
    assert_builds(&Case::full(&format!("{ALL_ON},impl.sim.sound-events.table")));
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
        let plan = plan(&case).unwrap();
        // The window-and-agent configuration boot installs after the plan's
        // other features (`registry`'s `configure_takeover`): the lab
        // takeover consent's dialog is in the plan and has its page of
        // guest memory (`lab_takeover::Driver`), so it is counted below.
        assert!(
            plan.features.iter().any(|f| f.id == "lab-takeover" && f.off.is_none()),
            "the lab takeover consent is not in the plan for {}",
            case.describe()
        );
        assert!(
            !plan.mappings(|a| a.feature == "lab-takeover").is_empty(),
            "the lab takeover consent has no guest memory for {}",
            case.describe()
        );
        let used = plan.arena_used();
        // The biggest users first, so a failure names what grew.
        let mut by_feature: Vec<(&str, u32)> = plan
            .features
            .iter()
            .map(|f| {
                let bytes = plan.mappings(|a| a.feature == f.id).iter().map(|m| m.len).sum();
                (f.id, bytes)
            })
            .filter(|&(_, bytes)| bytes > 0)
            .collect();
        by_feature.sort_by_key(|&(_, bytes)| std::cmp::Reverse(bytes));
        by_feature.truncate(12);
        assert!(
            used <= ARENA_SIZE / 16 * 15,
            "{used:#x} of the arena's {ARENA_SIZE:#x} bytes used, for {}; the biggest: {by_feature:x?}",
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

/// With Firefight on, the AI's multiplayer gate in
/// `ai_initialize_for_new_map` (`engine::points::ai_gates`) has the
/// feature's one `before`, so a kit map's AI is initialised and squads can
/// be placed; the gate's site and its target are image entries. Off, the
/// gate is not hooked and a multiplayer map's AI stays off as in retail.
#[test]
fn a_firefight_map_initialises_its_ai_unless_firefight_is_off() {
    use engine::points::ai_gates::{AI_NEW_MAP_INITIALISE, AI_NEW_MAP_MULTIPLAYER};
    let hooked = |switches: &str| {
        let plan = plan(&Case::full(switches)).unwrap();
        plan.chains
            .values()
            // The point is a `const`: each use is its own copy, so it is
            // matched by name and address, not by pointer.
            .filter(|chain| {
                chain.point.name == AI_NEW_MAP_MULTIPLAYER.name
                    && chain.point.base == AI_NEW_MAP_MULTIPLAYER.base
            })
            .flat_map(|chain| {
                chain
                    .hooks()
                    .filter(|hook| hook.feature == crate::firefight::feature::FEATURE)
                    .map(|hook| hook.kind().as_str())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(hooked(""), ["before"]);
    assert!(hooked("-firefight").is_empty());
    let entries: Vec<u32> = crate::image_entries().collect();
    assert!(entries.contains(&AI_NEW_MAP_MULTIPLAYER.base));
    assert!(entries.contains(&AI_NEW_MAP_INITIALISE));
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
        "theater-pause-menu",
        "theater-pause-driver",
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
    for switches in ["", ALL_ON, native_off.as_str()] {
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
    ("render_rng::install", "every case but Case::light"),
    ("render_scratch::install", "every case but Case::light"),
    ("flare_store::install", "every case but Case::light"),
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
    (
        "LightmapStore",
        "not covered: its store reads the running map's lightmap files",
    ),
    ("SimFrame", "Case::light"),
    (
        "RenderFrame",
        "every gpu case that is not Case::light (the registry helper adds it)",
    ),
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

/// The optional pieces a case can supply, as one bit each: the first nine
/// (a settings store, XAPI, a VFS, telemetry, the level boot, an agent, the
/// lockstep harness, the co-op probe, signed-in co-op) then the Theater,
/// Live, the lab takeover and the light host.
const OPTIONAL_PIECES: u32 = 13;

/// How many pieces together a clash may need. Two features clash over a
/// point, and each is made by one piece or a pair of them, so three covers
/// a clash that needs a third piece present or absent as well.
const CLASH_STRENGTH: u32 = 3;

/// Rows of on/off for `pieces` pieces such that every choice of `strength`
/// pieces takes each of its `2^strength` on/off values in some row (a
/// covering array). Greedy, from the all-off row, every candidate row scored
/// per round: `O(rows * 2^pieces * C(pieces, strength))`, deterministic.
fn covering_rows(pieces: u32, strength: u32) -> Vec<u32> {
    let all = 1u32 << pieces;
    let groups: Vec<u32> = (0..all).filter(|m| m.count_ones() == strength).collect();
    // `covered[g][v]`: group g has shown the values `v` (a subset of its mask).
    let mut covered = vec![vec![false; all as usize]; groups.len()];
    let mut missing = groups.len() * (1 << strength);
    let mut rows = Vec::new();
    let mut next = Some(0);
    while missing > 0 {
        let row = next.take().unwrap_or_else(|| {
            (0..all)
                .max_by_key(|row| {
                    groups
                        .iter()
                        .enumerate()
                        .filter(|(g, mask)| !covered[*g][(row & **mask) as usize])
                        .count()
                })
                .expect("at least one candidate row")
        });
        for (g, mask) in groups.iter().enumerate() {
            let seen = &mut covered[g][(row & mask) as usize];
            if !*seen {
                *seen = true;
                missing -= 1;
            }
        }
        rows.push(row);
    }
    rows
}

/// The case for a row of [`OPTIONAL_PIECES`] bits.
fn case_for_row(switches: &str, row: u32) -> Case {
    let on = |n: u32| row & (1 << n) != 0;
    let first = Case {
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
    with_second_group(&first, row >> 9)
}

/// The cases the sweep builds for `switches`: each covering row.
fn sweep_cases_for(switches: &str) -> Vec<Case> {
    covering_rows(OPTIONAL_PIECES, CLASH_STRENGTH)
        .into_iter()
        .map(|row| case_for_row(switches, row))
        .collect()
}

/// The cases the sweep builds: the covering rows under each switch set.
fn sweep_cases() -> Vec<Case> {
    let native_off = format!("{ALL_ON},{NATIVE_OFF}");
    let forge = format!("{ALL_ON},forge");
    ["", ALL_ON, native_off.as_str(), forge.as_str()]
        .into_iter()
        .flat_map(sweep_cases_for)
        .collect()
}

/// Builds each case's plan with `plan`; the distinct refusals.
fn refusals(cases: &[Case], plan: impl Fn(&Case) -> Result<hooks::Plan, String>) -> Vec<String> {
    dedup(cases.iter().filter_map(|case| plan(case).err()).collect())
}

/// Every piece alone, and every three of them together on and off, with the
/// default switches and with every hook switch on: skulls and the Halo 1
/// options (a settings store), bots and the Clankers page (XAPI), map slots
/// and our skulls' map reads (a VFS), telemetry, the level boot, our cheats
/// (an agent), the lockstep harness and the co-op probe, the Theater, Live,
/// the lab takeover and the light host.
///
/// A clash comes from a feature one piece adds meeting a feature another
/// adds, so a covering array over the pieces ([`covering_rows`]) finds what
/// the full product of 2^13 does, in a few dozen builds instead of
/// thousands. [`the_sweep_finds_a_clash_that_needs_two_or_three_pieces`]
/// proves it finds one.
#[test]
fn the_plan_builds_with_every_combination_of_optional_features() {
    let failures = refusals(&sweep_cases(), plan);
    assert!(
        failures.is_empty(),
        "{} distinct refusals:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A feature that hooks a made-up point with a `before` and no order, or
/// names that point's address as another name, so two of them clash.
struct Clasher(&'static str, &'static hooks::points::Point);

static CLASH_POINT: hooks::points::Point = hooks::points::Point::new(
    "TEST_CLASH",
    0x00ff_e000,
    hooks::points::Sig::Fn {
        args: &[],
        ret: None,
        pop: 0,
    },
    "docs/engine/feature-engine.md",
);
static CLASH_ALIAS: hooks::points::Point = hooks::points::Point::new(
    "TEST_CLASH_ALIAS",
    0x00ff_e000,
    hooks::points::Sig::Opaque,
    "docs/engine/feature-engine.md",
);

impl hooks::Feature for Clasher {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(self.0, "test")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.raw()
            .before(self.1, self.0, |_| Ok(hooks::Flow::Continue));
        Ok(())
    }
}

/// `case`'s plan with two [`Clasher`]s added when `when` holds of it.
fn plan_with_clash(
    case: &Case,
    when: impl Fn(&Case) -> bool,
    second: &'static hooks::points::Point,
) -> Result<hooks::Plan, String> {
    let mut features = registry(case);
    if when(case) {
        features.add(&Clasher("clash-a", &CLASH_POINT)).unwrap();
        features.add(&Clasher("clash-b", second)).unwrap();
    }
    features.build(&hooks::Base).map_err(|conflicts| {
        let c: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        format!("feature plan refused: {}", c.join("; "))
    })
}

#[test]
fn the_sweep_finds_a_clash_that_needs_two_or_three_pieces() {
    let cases = sweep_cases_for(ALL_ON);
    // Nothing injected: the sweep is clean (the test above).
    assert!(refusals(&cases, |c| plan_with_clash(c, |_| false, &CLASH_POINT)).is_empty());
    // Two features both hooking a point before, no order, when a VFS and the
    // co-op probe are both present.
    let two = refusals(&cases, |c| {
        plan_with_clash(c, |c| c.vfs && c.coop_probe, &CLASH_POINT)
    });
    assert!(
        two.iter().any(|r| r.contains(
            "clash-a and clash-b both hook it before with no order between them"
        )),
        "{two:?}"
    );
    // Two names for one address, when telemetry, the level boot and a light
    // host are all present (three pieces).
    let three = refusals(&cases, |c| {
        plan_with_clash(c, |c| c.telemetry && c.level && c.light, &CLASH_ALIAS)
    });
    assert!(
        three.iter().any(|r| r.contains("is named both")),
        "{three:?}"
    );
    // And when the pieces are absent together: no XAPI, no agent, no Live.
    let absent = refusals(&cases, |c| {
        plan_with_clash(c, |c| !c.xapi && !c.agent && !c.live && c.theater, &CLASH_POINT)
    });
    assert!(!absent.is_empty(), "a clash needing three absences went unseen");
}

#[test]
fn a_covering_array_covers_every_choice_of_pieces() {
    let rows = covering_rows(8, 3);
    for mask in (0u32..1 << 8).filter(|m| m.count_ones() == 3) {
        let mut seen = std::collections::BTreeSet::new();
        for row in &rows {
            seen.insert(row & mask);
        }
        assert_eq!(seen.len(), 8, "mask {mask:#b} lacks some values");
    }
    assert!(rows.len() < 40, "{} rows for 8 pieces", rows.len());
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
fn native_subsystems_install_on_every_profile() {
    use engine::points::{COLLISION_LOCATION, OBJECT_FUNCTION_VALUE, SIM_ANIMATION_STREAM};
    let runtime = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
    let points = [
        &SIM_ANIMATION_STREAM,
        &COLLISION_LOCATION,
        &OBJECT_FUNCTION_VALUE,
    ];
    // Animation, collision and the object update always run in Rust: no
    // profile or switch removes them.
    for light in [false, true] {
        let switches = super::profile_switches(hooks::Switches::parse("").unwrap(), light);
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
            [true; 3],
            "{light}"
        );
    }
}

#[test]
fn viewer_skips_follow_the_light_profile_and_independent_switches() {
    use engine::points::PREDICT_RESOURCES;
    // The seats' prediction walk owns the point (sim-bridge/src/sim_seats.rs):
    // no viewer skip hooks it.
    for (light, spec, predicted) in [
        (false, "", false),
        (false, "impl.sim=on", false),
        (true, "", false),
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
/// retail camera and takes the private stream (docs/graphics/high-resolution.md,
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
    assert!(hooked_at(registry_for_profile(&lockstep, profile), "RENDER_FRAME").contains(&crate::render_rng::ID));
    // The render's arena scratch is its own copy in every rendering
    // session too (docs/engine/sim-render-split.md, slice 8).
    for case in [&play, &lockstep] {
        assert!(hooked_at(registry_for_profile(case, profile), "RENDER_FRAME").contains(&crate::render_scratch::ID));
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The native renderer replaces the guest's view projection and its
/// visibility producers; `impl.render.native.visibility=off` leaves them
/// to the guest (docs/graphics/native-visibility.md).
#[cfg(feature = "gpu")]
#[test]
fn the_view_projection_is_native_unless_its_switch_is_off() {
    let profile = super::compiled_render_hooks();
    if profile != film::RenderHooks::Gpu {
        return;
    }
    let on = registry_for_profile(&Case::full(""), profile);
    assert_eq!(hooked_at(on, "VIEW_PROJECTION_BUILD"), ["native-scene-lists"]);
    let off = Case::full("impl.render.native.visibility=off");
    let on = registry_for_profile(&Case::full(""), profile);
    assert_eq!(hooked_at(on, "VISIBILITY_SETUP"), ["native-scene-lists"]);
    assert!(hooked_at(registry_for_profile(&off, profile), "VIEW_PROJECTION_BUILD").is_empty());
    // The setup's arm still compares its samples with the switch off: at its
    // own entry and at the entry of each producer the gather calls next.
    for point in ["VISIBILITY_SETUP", "VIEW_SPHERE_REGIONS", "VIEW_PVS_REGIONS"] {
        assert_eq!(hooked_at(registry_for_profile(&off, profile), point), ["native-scene-lists"], "{point}");
    }
}

/// Every rendering session holds the flare state in the render's store,
/// lockstep and replays included, so its bytes in the arena are the
/// simulation's alone (docs/engine/sim-render-split.md, slice 6).
#[test]
fn every_rendering_session_keeps_the_flare_state_out_of_the_arena() {
    let profile = super::compiled_render_hooks();
    let play = Case { coop_live: true, ..Case::full("") };
    let lockstep = Case { lockstep: true, ..play.clone() };
    let replay = Case { replaying: true, ..play.clone() };
    for case in [&play, &lockstep, &replay] {
        assert!(hooked_at(registry_for_profile(case, profile), "RENDER_FRAME").contains(&crate::flare_store::ID));
    }
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

/// The lift's replaced list (docs/lifter/lift-pruning.md): the ported
/// Controls row text replaces its entry in every profile.
#[test]
fn the_controls_row_text_is_replaced_in_every_profile() {
    let entry = engine::points::CONTROLS_ROW_TEXT.base;
    let replaced = super::replaced_entries(&[entry, engine::points::TEXT_SET.base].into_iter().collect());
    assert_eq!(replaced.keys().copied().collect::<Vec<_>>(), [entry]);
    // The build's list names the feature and keeps what the port calls.
    let lift = super::lift_replaced(&[entry, engine::points::TEXT_SET.base].into_iter().collect());
    let root = &lift.entries[&entry];
    assert_eq!((root.point.as_str(), root.feature.as_str()), ("CONTROLS_ROW_TEXT", crate::controls_text::ID));
    assert!(lift.calls.contains(&engine::points::TEXT_SET.base), "{:x?}", lift.calls);
}

/// A pruned module whose cut root's feature is switched off refuses the
/// boot, naming the point, the feature and `H2_LIFT_PRUNE=0`
/// (docs/lifter/lift-pruning.md, "Boot safety").
#[test]
fn a_pruned_root_whose_feature_is_off_refuses_the_boot_by_name() {
    let entry = engine::points::CONTROLS_ROW_TEXT.base;
    let roots = [emit::pruned::Root {
        entry,
        point: "CONTROLS_ROW_TEXT".into(),
        feature: crate::controls_text::ID.into(),
    }];
    super::check_pruned(&roots, &plan(&Case::full("")).unwrap()).unwrap();
    let off = format!("-{}", crate::controls_text::ID);
    let err = super::check_pruned(&roots, &plan(&Case::full(&off)).unwrap()).unwrap_err().to_string();
    for want in ["CONTROLS_ROW_TEXT", crate::controls_text::ID, "H2_LIFT_PRUNE=0"] {
        assert!(err.contains(want), "{want}: {err}");
    }
    // Nothing cut, nothing to check.
    super::check_pruned(&[], &plan(&Case::full(&off)).unwrap()).unwrap();
}
