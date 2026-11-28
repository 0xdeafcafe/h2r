//! Every native stage is on with the empty switch string.
#![cfg(feature = "native-render")]

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
            on: render_bridge::native_world::on,
            label: "native_world_skip",
            feature: &render_bridge::native_world::NativeWorld,
        },
        Stage {
            name: "world layers",
            on: render_bridge::native_world_layers::on,
            label: "native_world_layers_begin",
            feature: &render_bridge::native_world_layers::NativeWorldLayers,
        },
        Stage {
            name: "objects",
            on: render_bridge::native_objects::on,
            label: "native_objects_skip",
            feature: &render_bridge::native_objects::NativeObjects,
        },
        Stage {
            name: "first person",
            on: render_bridge::native_first_person::on,
            label: "native_first_person_skip",
            feature: &render_bridge::native_first_person::NativeFirstPerson,
        },
        Stage {
            name: "fog",
            on: render_bridge::native_fog::on,
            label: "native_fog_skip",
            feature: &render_bridge::native_fog::NativeFog,
        },
        Stage {
            name: "water",
            on: render_bridge::native_water::on,
            label: "native_water_skip",
            feature: &render_bridge::native_water::NativeWater,
        },
        Stage {
            name: "transparents",
            on: render_bridge::native_transparents::on,
            label: "native_transparents_entry",
            feature: &render_bridge::native_transparents::NativeTransparents,
        },
        Stage {
            name: "decals",
            on: render_bridge::native_decals::on,
            label: "native_decals_begin",
            feature: &render_bridge::native_decals::NativeDecals,
        },
        Stage {
            name: "decal decorators",
            on: render_bridge::native_decal_decorators::on,
            label: "native_decal_decorators",
            feature: &render_bridge::native_decal_decorators::NativeDecalDecorators,
        },
        Stage {
            name: "particles",
            on: render_bridge::native_particles::on,
            label: "native_particles_walk_emit",
            feature: &render_bridge::native_particles::NativeParticles,
        },
        Stage {
            name: "lights",
            on: render_bridge::native_lights::on,
            label: "native_lights_view",
            feature: &render_bridge::native_lights::NativeLights,
        },
        Stage {
            name: "light walk",
            on: render_bridge::native_lights::walk::on,
            label: "walk gathered light geometry",
            feature: &render_bridge::native_lights::walk::NativeLightWalk,
        },
        Stage {
            name: "gels",
            on: render_bridge::native_gels::on,
            label: "native_gels_template",
            feature: &render_bridge::native_gels::NativeGels,
        },
        Stage {
            name: "shadows",
            on: render_bridge::native_shadows::on,
            label: "shadow layer",
            feature: &render_bridge::native_shadows::NativeShadows,
        },
        Stage {
            name: "post",
            on: render_bridge::native_post::on,
            label: "native_post_colour",
            feature: &render_bridge::native_post::NativePost,
        },
        Stage {
            name: "ui",
            on: render_bridge::native_hud::on,
            label: "native_ui_fade_fan",
            feature: &render_bridge::native_hud::NativeHud,
        },
    ]
}

fn placed(feature: &dyn Feature, switches: &str) -> String {
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(feature).unwrap());
    r.build(&Base).unwrap().text()
}

/// The empty switch string places every stage.
#[test]
fn every_stage_is_on_by_default() {
    for stage in stages() {
        assert!((stage.on)(&Switches::defaults()), "{}", stage.name);
        let on = placed(stage.feature, "");
        assert!(on.contains(stage.label), "{}: {on}", stage.name);
    }
}

/// The old per-stage keys are gone: `H2_FEATURES` refuses them, and set
/// directly they change nothing, so no stage can be turned off alone.
#[test]
fn the_old_stage_keys_reach_nothing() {
    for spec in [
        "impl.render.native_world=on",
        "impl.render.native_ui=off",
        "impl.render.native_lights_split=on",
    ] {
        assert!(Switches::parse(spec).is_err(), "{spec} parses");
    }
    let stage_keys = [("impl.render.native_world", false), ("impl.render.native_ui", false)];
    let mut switches = Switches::defaults();
    for (key, on) in stage_keys {
        switches = switches.with(key, on);
    }
    for stage in stages() {
        assert!((stage.on)(&switches), "{stage_keys:?}: {}", stage.name);
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
/// what `switches.rs` tests.
#[test]
fn the_native_master_switch_gates_every_stage_and_walk() {
    let enabled = |spec| {
        let switches = Switches::parse(spec).unwrap();
        [
            render_bridge::native_world::on(&switches),
            render_bridge::native_transparents::on(&switches),
            render_bridge::native_decals::on(&switches),
            render_bridge::native_particles::on(&switches),
            render_bridge::native_first_person::on(&switches),
            render_bridge::native_hud::on(&switches),
            render_bridge::native_post::on(&switches),
        ]
    };
    assert_eq!(enabled(""), [true; 7]);
    assert_eq!(enabled("impl.render=off"), [true; 7]);
}
