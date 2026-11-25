use super::{Keep, view_run};
use frame_scene::{GroupSite, LightConstants, LightGroup};
use native_render::{light_graph::Pass, light_passes};

#[test]
fn a_shader_gel_keeps_its_producer_even_with_whole_view_groups() {
    let site = GroupSite::Light;
    let base = LightConstants {
        frame: [0; 36],
        flags: 0,
        colour_a: [0; 3],
        colour_b: [0; 3],
        mode: 0,
        definition_flags: 0,
        definition_8a: 0,
        definition_80: 0,
        falloff_0: 0,
        gel: None,
        animated_gel: false,
    };
    // The constants generate the gel even when no layer uses it this frame.
    for drawn in [false, true] {
        let steps = light_passes::steps(&light_passes::Inputs {
            receivers: u32::from(drawn) << 12,
            ..Default::default()
        });
        let diffuse = LightGroup {
            light: 0,
            site,
            layer: 12,
            entries: 0..1,
            remainder: 0,
            projection: None,
            pass: None,
        };
        let groups: Vec<_> = drawn.then_some(&diffuse).into_iter().collect();
        for animated_gel in [false, true] {
            let constants = LightConstants { animated_gel, ..base };
            let result = view_run(
                Pass::Light {
                    site,
                    constants: &constants,
                    steps: steps.as_slice(),
                    additional: None,
                    scissor: None,
                    projection: None,
                },
                &groups,
            );
            if animated_gel {
                assert!(matches!(result, Err(Keep::Unsupported(_))));
            } else {
                assert_eq!(result.unwrap().groups.len(), usize::from(drawn));
            }
        }
    }
}
