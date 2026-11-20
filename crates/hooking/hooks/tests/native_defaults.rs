//! Every native stage is on with the empty switch string, and
//! `impl.render.native=off` is the only way to turn them off: one switch
//! for the whole native renderer, looked up exactly.

use hooks::plan::Base;
use hooks::{Feature, Registry, Switches};

/// One stage: its name, whether it is on, a label only its hooks put in
/// the plan text, and the feature that places it.
struct Stage {
    name: &'static str,
    on: fn(&Switches) -> bool,
    label: &'static str,
    feature: &'static dyn Feature,
}

fn stages() -> Vec<Stage> {
    vec![
        Stage {
            name: "world",
            on: hooks::native_world::on,
            label: "native_world_skip",
            feature: &hooks::native_world::NativeWorld,
        },
        Stage {
            name: "world layers",
            on: hooks::native_world_layers::on,
            label: "native_world_layers_begin",
            feature: &hooks::native_world_layers::NativeWorldLayers,
        },
        Stage {
            name: "objects",
            on: hooks::native_objects::on,
            label: "native_objects_skip",
            feature: &hooks::native_objects::NativeObjects,
        },
        Stage {
            name: "first person",
            on: hooks::native_first_person::on,
            label: "native_first_person_skip",
            feature: &hooks::native_first_person::NativeFirstPerson,
        },
        Stage {
            name: "fog",
            on: hooks::native_fog::on,
            label: "native_fog_skip",
            feature: &hooks::native_fog::NativeFog,
        },
        Stage {
            name: "water",
            on: hooks::native_water::on,
            label: "native_water_skip",
            feature: &hooks::native_water::NativeWater,
        },
        Stage {
            name: "transparents",
            on: hooks::native_transparents::on,
            label: "native_transparents_entry",
            feature: &hooks::native_transparents::NativeTransparents,
        },
        Stage {
            name: "decals",
            on: hooks::native_decals::on,
            label: "native_decals_begin",
            feature: &hooks::native_decals::NativeDecals,
        },
        Stage {
            name: "particles",
            on: hooks::native_particles::on,
            label: "native_particles_walk_emit",
            feature: &hooks::native_particles::NativeParticles,
        },
        Stage {
            name: "lights",
            on: hooks::native_lights::on,
            label: "native_lights_view",
            feature: &hooks::native_lights::NativeLights,
        },
        Stage {
            name: "light walk",
            on: hooks::native_lights::walk::on,
            label: "walk gathered light geometry",
            feature: &hooks::native_lights::walk::NativeLightWalk,
        },
        Stage {
            name: "gels",
            on: hooks::native_gels::on,
            label: "native_gels_template",
            feature: &hooks::native_gels::NativeGels,
        },
        Stage {
            name: "shadows",
            on: hooks::native_shadows::on,
            label: "shadow layer",
            feature: &hooks::native_shadows::NativeShadows,
        },
        Stage {
            name: "post",
            on: hooks::native_post::on,
            label: "native_post_colour",
            feature: &hooks::native_post::NativePost,
        },
        Stage {
            name: "ui",
            on: hooks::native_hud::on,
            label: "native_ui_screen_fan",
            feature: &hooks::native_hud::NativeHud,
        },
    ]
}

fn placed(feature: &dyn Feature, switches: &str) -> String {
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(feature).unwrap());
    r.build(&Base).unwrap().text()
}

/// The empty switch string places every stage; the master switch off
/// removes each one's hooks.
#[test]
fn every_stage_is_on_by_default_and_off_with_the_master_switch() {
    let off = Switches::parse("impl.render.native=off").unwrap();
    for stage in stages() {
        assert!((stage.on)(&Switches::defaults()), "{}", stage.name);
        assert!(!(stage.on)(&off), "{}", stage.name);
        let on = placed(stage.feature, "");
        assert!(on.contains(stage.label), "{}: {on}", stage.name);
        let off = placed(stage.feature, "impl.render.native=off");
        assert!(!off.contains(stage.label), "{}: {off}", stage.name);
    }
}

fn plan(switches: Switches) -> hooks::Plan {
    let mut registry = Registry::new(switches);
    assert!(registry.add(&hooks::native_map_work::NativeMapWork).unwrap());
    assert!(registry.add(&hooks::native_scene_lists::NativeSceneLists).unwrap());
    for stage in stages() {
        assert!(registry.add(stage.feature).unwrap());
    }
    registry.build(&Base).unwrap()
}

#[test]
fn the_native_master_switch_removes_every_native_hook() {
    let plan = plan(Switches::parse("impl.render.native=off").unwrap());
    assert!(
        plan.chains.is_empty() && plan.entries.is_empty(),
        "{}",
        plan.text(),
    );
}

/// The old per-stage keys are gone: `H2_FEATURES` refuses them, and set
/// directly they change nothing, so no stage can be turned on alone with
/// the master off, or off alone with it on.
#[test]
fn the_old_stage_keys_reach_nothing() {
    for spec in [
        "impl.render.native_world=on",
        "impl.render.native_ui=off",
        "impl.render.native_lights_split=on",
    ] {
        assert!(Switches::parse(spec).is_err(), "{spec} parses");
    }
    for (master, stage_keys) in [
        (false, [("impl.render.native_world", true), ("impl.render.native_lights", true)]),
        (true, [("impl.render.native_world", false), ("impl.render.native_ui", false)]),
    ] {
        let mut switches = Switches::defaults().with("impl.render.native", master);
        for (key, on) in stage_keys {
            switches = switches.with(key, on);
        }
        for stage in stages() {
            assert_eq!((stage.on)(&switches), master, "{stage_keys:?}: {}", stage.name);
        }
    }
}

#[test]
fn the_render_group_and_the_native_renderer_are_independent() {
    let ordinary_render_off = Switches::parse("impl.render=off").unwrap();
    for stage in stages() {
        assert!((stage.on)(&ordinary_render_off), "{}", stage.name);
    }
}

/// The master switch and each stage's own `on` agree; the stages' half of
/// what `switches.rs` tests for [`hooks::switches::native_render_on`].
#[test]
fn the_native_master_switch_gates_every_stage_and_walk() {
    let enabled = |spec| {
        let switches = Switches::parse(spec).unwrap();
        [
            hooks::switches::native_render_on(&switches),
            hooks::native_world::on(&switches),
            hooks::native_transparents::on(&switches),
            hooks::native_decals::on(&switches),
            hooks::native_particles::on(&switches),
            hooks::native_first_person::on(&switches),
            hooks::native_hud::on(&switches),
            hooks::native_post::on(&switches),
        ]
    };
    assert_eq!(enabled(""), [true; 8]);
    assert_eq!(enabled("impl.render=off"), [true; 8]);
    assert_eq!(enabled("impl.render.native=off"), [false; 8]);
}
