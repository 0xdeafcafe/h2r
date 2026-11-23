//! A caster layer planned from its frame-scene group
//! (docs/graphics/native-shadow-layers.md, "The plan and the state").
//! Geometry planning is the light layers': sorted entries and duplicates
//! stay in order, and adjacent ranges of one stream join. The shadow
//! target, projection and raster state must be in the group; a group
//! without them keeps its guest handlers.

use frame_scene::{FrameScene, LightEntry, LightGroup, Row};

use crate::light_scene::{self, Draw, Resident};

/// Why a caster group is not drawn from the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The target, projection or raster state did not read or does not
    /// match layer 10's target choice.
    State,
    /// A geometry refusal of the shared light-group planner.
    Light(light_scene::Refusal),
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Light(light) => light.name(),
        }
    }
}

impl From<light_scene::Refusal> for Refusal {
    fn from(value: light_scene::Refusal) -> Self {
        Self::Light(value)
    }
}

fn validate(group: &LightGroup) -> Result<(), Refusal> {
    let pass = group.pass.ok_or(Refusal::State)?;
    if group.layer != 10
        || group.projection.is_none()
        || pass.size.contains(&0)
        || pass.target != if pass.colour_mask() { 10 } else { 9 }
        || pass.stencil_mode != 0
    {
        return Err(Refusal::State);
    }
    Ok(())
}

/// A whole caster group's draws from the scene alone.
pub fn plan(
    scene: &FrameScene,
    group: &LightGroup,
    resident: &dyn Resident,
) -> Result<light_scene::Plan, Refusal> {
    validate(group)?;
    light_scene::plan(scene, group, resident).map_err(Into::into)
}

/// [`plan`]'s draws for a copied scene group and its entries' node rows.
pub fn plan_entries<'r>(
    group: &LightGroup,
    entries: &[LightEntry],
    rows: &dyn Fn(&LightEntry) -> &'r [Row],
    resident: &dyn Resident,
) -> Result<Vec<Draw>, Refusal> {
    validate(group)?;
    light_scene::plan_entries(entries, rows, resident).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use frame_scene::{GroupSite, ShadowPass};

    use super::*;

    struct Empty;
    impl Resident for Empty {
        fn part(&self, _: &LightEntry) -> Option<light_scene::ResidentPart> {
            None
        }
    }

    fn group() -> LightGroup {
        LightGroup {
            light: 0,
            layer: 10,
            site: GroupSite::Light,
            projection: Some([[0; 4]; frame_scene::PROJECTION_ROWS]),
            pass: Some(ShadowPass {
                target: 9,
                size: [512, 512],
                flags: 0,
                clip_control: 0,
                raster: 0,
                offset_enable: 1,
                offset_constant: 0,
                offset_slope: 0,
                stencil_mode: 0,
                shadow_function: 0,
            }),
            entries: 0..0,
            remainder: 0,
        }
    }

    #[test]
    fn a_caster_group_needs_its_own_target_projection_and_stencil_off() {
        let scene = FrameScene::default();
        let valid = group();
        assert_eq!(plan(&scene, &valid, &Empty).unwrap().draws.len(), 0);
        for change in [
            (|g: &mut LightGroup| g.layer = 11) as &dyn Fn(&mut LightGroup),
            &|g| g.projection = None,
            &|g| g.pass.as_mut().unwrap().target = 10,
            &|g| g.pass.as_mut().unwrap().size[0] = 0,
            &|g| g.pass.as_mut().unwrap().stencil_mode = 1,
            &|g| g.pass = None,
        ] {
            let mut invalid = valid.clone();
            change(&mut invalid);
            assert_eq!(plan(&scene, &invalid, &Empty).unwrap_err(), Refusal::State);
        }
        let mut mask = valid;
        mask.pass.as_mut().unwrap().flags = 1 << 4;
        mask.pass.as_mut().unwrap().target = 10;
        assert_eq!(plan(&scene, &mask, &Empty).unwrap().draws.len(), 0);
    }
}
