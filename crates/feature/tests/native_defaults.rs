//! Every native stage is on with the empty switch string (since
//! 2026-11-20). The master default can be overridden for each stage.
//! `impl.render.native_lights_split` stays off unless named.

use feature::plan::Base;
use feature::{Feature, Registry, Switches};

/// One stage: its switch, a label only its hooks put in the plan text,
/// and the feature that places it. The world programs use the world's hooks.
struct Stage {
    switch: &'static str,
    on: fn(&Switches) -> bool,
    label: &'static str,
    feature: Option<&'static dyn Feature>,
    /// What else must be off for the stage's hooks to go: the shared
    /// layer context stays while any consumer is on.
    off_with: &'static [&'static str],
    /// The environment that places the stage even with the switch off.
    compare_env: Option<&'static str>,
}

fn stages() -> Vec<Stage> {
    vec![
        Stage {
            switch: feature::native_world::SWITCH,
            on: feature::native_world::on,
            label: "native_world_skip",
            feature: Some(&feature::native_world::NativeWorld),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_world::PROGRAMS_SWITCH,
            on: feature::native_world::programs_on,
            label: "",
            feature: None,
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_world_layers::SWITCH,
            on: feature::native_world_layers::on,
            label: "native_world_layers_begin",
            feature: Some(&feature::native_world_layers::NativeWorldLayers),
            off_with: &[
                feature::native_objects::SWITCH,
                feature::native_lights::SWITCH,
                feature::native_residual::SWITCH,
            ],
            compare_env: Some("H2_NATIVE_WORLD_LAYERS_COMPARE"),
        },
        Stage {
            switch: feature::native_objects::SWITCH,
            on: feature::native_objects::on,
            label: "native_objects_skip",
            feature: Some(&feature::native_objects::NativeObjects),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_first_person::SWITCH,
            on: feature::native_first_person::on,
            label: "native_first_person_skip",
            feature: Some(&feature::native_first_person::NativeFirstPerson),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_fog::SWITCH,
            on: feature::native_fog::on,
            label: "native_fog_skip",
            feature: Some(&feature::native_fog::NativeFog),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_water::SWITCH,
            on: feature::native_water::on,
            label: "native_water_skip",
            feature: Some(&feature::native_water::NativeWater),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_transparents::SWITCH,
            on: feature::native_transparents::on,
            label: "native_transparents_entry",
            feature: Some(&feature::native_transparents::NativeTransparents),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_decals::SWITCH,
            on: feature::native_decals::on,
            label: "native_decals_begin",
            feature: Some(&feature::native_decals::NativeDecals),
            off_with: &[],
            compare_env: Some("H2_NATIVE_DECALS_COMPARE"),
        },
        Stage {
            switch: feature::native_particles::SWITCH,
            on: feature::native_particles::on,
            label: "native_particles_walk_emit",
            feature: Some(&feature::native_particles::NativeParticles),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_lights::SWITCH,
            on: feature::native_lights::on,
            label: "native_lights_view",
            feature: Some(&feature::native_lights::NativeLights),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_gels::SWITCH,
            on: feature::native_gels::on,
            label: "native_gels_template",
            feature: Some(&feature::native_gels::NativeGels),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_shadows::SWITCH,
            on: feature::native_shadows::on,
            label: "shadow layer",
            feature: Some(&feature::native_shadows::NativeShadows),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_post::SWITCH,
            on: feature::native_post::on,
            label: "native_post_colour",
            feature: Some(&feature::native_post::NativePost),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_hud::UI_SWITCH,
            on: feature::native_hud::on,
            label: if feature::native_hud::compare_spec().is_some() {
                "native_hud_quad"
            } else {
                "native_ui_screen_fan"
            },
            feature: Some(&feature::native_hud::NativeHud),
            off_with: &[],
            compare_env: Some("H2_NATIVE_HUD_COMPARE"),
        },
        Stage {
            switch: feature::native_residual::SWITCH,
            on: feature::native_residual::on,
            label: "residual_frame_scope",
            feature: Some(&feature::native_residual::NativeResidual),
            off_with: &[],
            compare_env: Some("H2_NATIVE_RESIDUAL_PROBE"),
        },
    ]
}

/// The empty switch string places every stage that places hooks without
/// a probe environment, and each `=off` alone removes exactly that one
/// (a stage's comparison environment also places it, so those are
/// skipped when it is set).
#[test]
fn every_stage_is_on_with_the_empty_switch_string_and_off_with_equals_off() {
    for stage in stages() {
        assert!((stage.on)(&Switches::defaults()), "{}", stage.switch);
        if stage.switch == feature::native_residual::SWITCH
            && feature::native_residual::probe_frames().is_empty()
        {
            continue;
        }
        let Some(feature) = stage.feature else {
            continue;
        };
        let mut r = Registry::new(Switches::parse("").unwrap());
        assert!(r.add(feature).unwrap());
        let placed = r.build(&Base).unwrap().text();
        assert!(placed.contains(stage.label), "{}: {placed}", stage.switch);

        if stage
            .compare_env
            .is_some_and(|env| std::env::var_os(env).is_some())
        {
            continue;
        }
        let off_switches = format!(
            "{}=off{}",
            stage.switch,
            stage
                .off_with
                .iter()
                .map(|s| format!(",{s}=off"))
                .collect::<String>()
        );
        let mut r = Registry::new(Switches::parse(&off_switches).unwrap());
        assert!(r.add(feature).unwrap());
        let off = r.build(&Base).unwrap().text();
        assert!(!off.contains(stage.label), "{}: {off}", stage.switch);
    }
}

/// The stages that place no hooks without their probe environment, and
/// the sub-switches that stay off: `native-residual`'s helper, the
/// world's material programs (on with the world) and the lights' split.
#[test]
fn the_residual_helper_defaults_on_and_the_subswitches_hold() {
    let empty = Switches::parse("").unwrap();
    assert!(feature::native_residual::on(&empty));
    let off = Switches::parse(&format!("{}=off", feature::native_residual::SWITCH)).unwrap();
    assert!(!feature::native_residual::on(&off));
    assert!(
        feature::native_world::programs_on(&empty),
        "programs on with the world"
    );
    assert!(
        !feature::native_lights::split_on(&empty),
        "the split stays off"
    );
}

fn plan(switches: Switches) -> feature::Plan {
    let mut registry = Registry::new(switches);
    assert!(registry.add(&feature::native_map_work::NativeMapWork).unwrap());
    for feature in stages().into_iter().filter_map(|stage| stage.feature) {
        assert!(registry.add(feature).unwrap());
    }
    registry.build(&Base).unwrap()
}

#[test]
fn the_native_master_switch_removes_every_stage_hook() {
    let off = Switches::parse("impl.render.native=off").unwrap();
    for stage in stages() {
        assert!(!(stage.on)(&off), "{}", stage.switch);
    }
    let plan = plan(off);
    assert!(
        plan.chains.is_empty() && plan.entries.is_empty(),
        "{}",
        plan.text(),
    );
}

#[test]
fn the_world_override_restores_only_the_world_and_its_programs() {
    for spec in [
        "impl.render.native=off,impl.render.native_world=on",
        "impl.render.native_world=on,impl.render.native=off",
    ] {
        let switches = Switches::parse(spec).unwrap();
        let plan = plan(switches.clone());
        let placed = plan.text();
        for stage in stages() {
            let world = stage.switch == feature::native_world::SWITCH;
            let programs = stage.switch == feature::native_world::PROGRAMS_SWITCH;
            assert_eq!((stage.on)(&switches), world || programs, "{}", stage.switch);
            if stage.feature.is_some() {
                assert_eq!(placed.contains(stage.label), world, "{placed}");
            }
        }
        assert!(placed.contains("native_bsp_load"), "{placed}");
        assert!(
            plan.chains
                .values()
                .flat_map(|chain| chain.hooks())
                .all(|hook| [feature::native_world::ID, feature::native_map_work::ID].contains(&hook.feature)),
            "{placed}"
        );
    }
}

#[test]
fn the_render_group_and_native_stages_are_independent() {
    let ordinary_render_off = Switches::parse("impl.render=off").unwrap();
    assert!(!ordinary_render_off.implementation("render.layer_draw", true));
    for stage in stages() {
        assert!((stage.on)(&ordinary_render_off), "{}", stage.switch);
    }

    let switches = Switches::parse("impl.render.native=on,impl.render.native_world=off").unwrap();
    assert!(!feature::native_world::on(&switches));
    assert!(!feature::native_world::programs_on(&switches));
    assert!(feature::native_lights::on(&switches));

    let switches = Switches::parse("impl.render.native=off,impl.render.native_lights=on").unwrap();
    assert!(feature::native_lights::on(&switches));
    assert!(!feature::native_lights::split_on(&switches));
}
