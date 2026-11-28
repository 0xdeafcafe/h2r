//! A shadow layer planned from its frame-scene group
//! (docs/graphics/native-shadow-layers.md, "The plan and the state").
//! Geometry planning is the light layers': sorted entries and duplicates
//! stay in order, and adjacent ranges of one stream join. The shadow
//! target, projection and raster state must be in the group; a group
//! without them keeps its guest handlers.

use frame_scene::{FrameScene, LightEntry, LightGroup, Row};

use crate::light_scene::{self, Draw, Resident};

/// Why a shadow group is not drawn from the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The group's shadow pass state did not read.
    NoPass,
    /// The pass's target size is zero in a dimension.
    EmptySize,
    /// The pass draws with stencil mode `n` (the guest's stencil-tested
    /// shadow layers; the native pass has no stencil).
    Stencil(u32),
    /// A caster or receiver layer whose projection did not read.
    NoProjection,
    /// A caster whose target is not its map's (9 for depth, 10 for the
    /// colour mask), or an opaque layer not on target 1.
    Target,
    /// The group's layer is not one of 10, 11 or 14.
    Layer,
    /// A geometry refusal of the shared light-group planner.
    Light(light_scene::Refusal),
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Self::NoPass => "state: no shadow pass",
            Self::EmptySize => "state: an empty shadow target",
            Self::Stencil(_) => "state: a stencil-tested shadow pass",
            Self::NoProjection => "state: no shadow projection",
            Self::Target => "state: the layer's target is not its map's",
            Self::Layer => "state: not a shadow layer",
            Self::Light(light) => light.name(),
        }
    }
}

impl From<light_scene::Refusal> for Refusal {
    fn from(value: light_scene::Refusal) -> Self {
        Self::Light(value)
    }
}

pub(crate) fn validate(group: &LightGroup) -> Result<(), Refusal> {
    let pass = group.pass.ok_or(Refusal::NoPass)?;
    if pass.size.contains(&0) {
        return Err(Refusal::EmptySize);
    }
    if pass.stencil_mode != 0 {
        return Err(Refusal::Stencil(pass.stencil_mode));
    }
    let projected = || group.projection.map(|_| ()).ok_or(Refusal::NoProjection);
    match group.layer {
        10 => {
            projected()?;
            let map = if pass.colour_mask() { 10 } else { 9 };
            if pass.target == map { Ok(()) } else { Err(Refusal::Target) }
        }
        11 => projected(),
        14 if pass.target == 1 => Ok(()),
        14 => Err(Refusal::Target),
        _ => Err(Refusal::Layer),
    }
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
        let changes: [(&dyn Fn(&mut LightGroup), Refusal); 6] = [
            (&|g| g.layer = 12, Refusal::Layer),
            (&|g| g.projection = None, Refusal::NoProjection),
            (&|g| g.pass.as_mut().unwrap().target = 10, Refusal::Target),
            (&|g| g.pass.as_mut().unwrap().size[0] = 0, Refusal::EmptySize),
            (&|g| g.pass.as_mut().unwrap().stencil_mode = 1, Refusal::Stencil(1)),
            (&|g| g.pass = None, Refusal::NoPass),
        ];
        for (change, why) in changes {
            let mut invalid = valid.clone();
            change(&mut invalid);
            assert_eq!(plan(&scene, &invalid, &Empty).unwrap_err(), why);
        }
        let mut mask = valid;
        mask.pass.as_mut().unwrap().flags = 1 << 4;
        mask.pass.as_mut().unwrap().target = 10;
        assert_eq!(plan(&scene, &mask, &Empty).unwrap().draws.len(), 0);
        let mut receiver = mask;
        receiver.layer = 11;
        assert_eq!(plan(&scene, &receiver, &Empty).unwrap().draws.len(), 0);
        let mut opaque = receiver;
        opaque.layer = 14;
        opaque.projection = None;
        opaque.pass.as_mut().unwrap().target = 1;
        assert_eq!(plan(&scene, &opaque, &Empty).unwrap().draws.len(), 0);
    }

    #[test]
    fn a_scaled_view_draws_its_shadow_layers() {
        // The receiver layers draw into the view's target, at any render
        // scale: the plan checks the pass's own state, never the scale.
        let scene = FrameScene::default();
        for size in [[1280, 720], [2560, 1440], [3840, 2160]] {
            let mut receiver = group();
            receiver.layer = 11;
            receiver.pass.as_mut().unwrap().size = size;
            assert_eq!(plan(&scene, &receiver, &Empty).unwrap().draws.len(), 0);
            let mut opaque = receiver;
            opaque.layer = 14;
            opaque.projection = None;
            opaque.pass.as_mut().unwrap().target = 1;
            assert_eq!(plan(&scene, &opaque, &Empty).unwrap().draws.len(), 0);
        }
    }

    #[test]
    fn each_shadow_state_refusal_names_its_cause() {
        let names = [
            Refusal::NoPass,
            Refusal::EmptySize,
            Refusal::Stencil(2),
            Refusal::NoProjection,
            Refusal::Target,
            Refusal::Layer,
        ]
        .map(Refusal::name);
        for (i, a) in names.iter().enumerate() {
            assert!(a.starts_with("state: "), "{a}");
            assert!(names[i + 1..].iter().all(|b| a != b), "{a} named twice");
        }
        let mut opaque = group();
        opaque.layer = 14;
        assert_eq!(validate(&opaque), Err(Refusal::Target), "layer 14 off target 1");
    }
}
