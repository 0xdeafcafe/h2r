//! Every native stage is on with the empty switch string (since
//! 2026-11-20), and each turns off with its own `=off`: the comparison
//! against the D3D path is one switch per stage, not a list of `=on`s.
//! `impl.render.native_lights_split` stays off unless named.

use feature::plan::Base;
use feature::{Feature, Registry, Switches};

/// One stage: its switch, a label only its hooks put in the plan text,
/// and the feature that places it. `None` for a stage that places no
/// hooks without its probe environment (`native-residual`).
struct Stage {
    switch: &'static str,
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
            label: "native_world_skip",
            feature: Some(&feature::native_world::NativeWorld),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_world_layers::SWITCH,
            label: "native_world_layers_begin",
            feature: Some(&feature::native_world_layers::NativeWorldLayers),
            off_with: &[
                feature::native_objects::SWITCH,
                feature::native_lights::SWITCH,
                feature::native_residual::SWITCH,
            ],
            compare_env: None,
        },
        Stage {
            switch: feature::native_objects::SWITCH,
            label: "native_objects_skip",
            feature: Some(&feature::native_objects::NativeObjects),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_first_person::SWITCH,
            label: "native_first_person_skip",
            feature: Some(&feature::native_first_person::NativeFirstPerson),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_fog::SWITCH,
            label: "native_fog_skip",
            feature: Some(&feature::native_fog::NativeFog),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_water::SWITCH,
            label: "native_water_skip",
            feature: Some(&feature::native_water::NativeWater),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_transparents::SWITCH,
            label: "native_transparents_entry",
            feature: Some(&feature::native_transparents::NativeTransparents),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_decals::SWITCH,
            label: "native_decals_begin",
            feature: Some(&feature::native_decals::NativeDecals),
            off_with: &[],
            compare_env: Some("H2_NATIVE_DECALS_COMPARE"),
        },
        Stage {
            switch: feature::native_particles::SWITCH,
            label: "native_particles_emit",
            feature: Some(&feature::native_particles::NativeParticles),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_lights::SWITCH,
            label: "native_lights_view",
            feature: Some(&feature::native_lights::NativeLights),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_gels::SWITCH,
            label: "native_gels_template",
            feature: Some(&feature::native_gels::NativeGels),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_shadows::SWITCH,
            label: "shadow layer",
            feature: Some(&feature::native_shadows::NativeShadows),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_post::SWITCH,
            label: "native_post_colour",
            feature: Some(&feature::native_post::NativePost),
            off_with: &[],
            compare_env: None,
        },
        Stage {
            switch: feature::native_hud::SWITCH,
            label: "native_hud_quad",
            feature: Some(&feature::native_hud::NativeHud),
            off_with: &[],
            compare_env: Some("H2_NATIVE_HUD_COMPARE"),
        },
        Stage {
            switch: feature::native_residual::SWITCH,
            label: "residual_frame_scope",
            feature: None,
            off_with: &[],
            compare_env: None,
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
