//! The view's object shadows handed to the host from the scene, in slot 3
//! before layer 7 (docs/graphics/native-shadows.md, "The object shadows
//! from the scene"). Each is the caster's layer 10 and 11 groups with the
//! filter between them. Under the scene path the caster `0x00032350`
//! stops after its item lists ([`cut`]): its layers, filter and tail do
//! not run, and the groups are planned from those lists as the cut light
//! passes plan theirs. The host draws only what the cut planned
//! ([`draw`]), so a shadow is drawn once.

use std::sync::atomic::{AtomicU64, Ordering};

use engine::light_constants::Fpu;
use engine::light_passes as lp;
use engine::memory::GuestMemory;
use frame_scene::{FrameScene, GroupSite, LightStep, LightTarget, ShadowFilter, View, ViewSlot};
use native_render::shadows::{CASTERS, RECEIVERS};
use scene_extract::light_groups;

use super::typed_groups::GroupRefusal;
use super::{HOST, Ro, SceneGroup, VIEW_TARGET, typed_groups};
use crate::native_map_work::Stage;

/// The slot the object shadows draw in; layer 7 follows them.
pub const SLOT: ViewSlot = ViewSlot::ShadowsAndLayer7;

/// One object shadow, owned: its groups in the guest's order (layer 10,
/// then 11) and the filter between them.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectShadowPass {
    pub slot: ViewSlot,
    pub identity: [u32; 2],
    pub creature: bool,
    pub weight_bits: Option<u32>,
    pub groups: Vec<SceneGroup>,
    pub filter: Option<ShadowFilter>,
}

/// What the host is asked to draw.
pub struct ObjectShadowDraw<'a> {
    pub passes: &'a [ObjectShadowPass],
    pub view_target: [u32; 8],
}

/// `view`'s object shadows, owned, in the guest's order. `view` is one of
/// `scene.views`.
pub fn gather(scene: &FrameScene, view: &View) -> Vec<ObjectShadowPass> {
    let Some(index) = scene.views.iter().position(|v| std::ptr::eq(v, view)) else {
        return Vec::new();
    };
    scene
        .object_shadows_of(index as u32)
        .iter()
        .map(|o| ObjectShadowPass {
            slot: SLOT,
            identity: o.identity,
            creature: o.creature,
            weight_bits: o.weight_bits,
            groups: scene
                .shadow_groups_of(o)
                .iter()
                .map(|g| {
                    let entries = scene.entries_of(g).to_vec();
                    let rows = entries.iter().map(|e| scene.pose_rows(e).to_vec()).collect();
                    SceneGroup { group: g.clone(), entries, rows }
                })
                .collect(),
            filter: o.filter.clone(),
        })
        .collect()
}

/// The steps of the mask branch's layers (`0x000329e3`'s caller): layer 10
/// into the colour mask, then 11 on the view's target. The branch draws
/// them only when the casters hold layer 10 and the receivers layer 11.
const STEPS: [LightStep; 2] = [
    LightStep { layer: 10, target: LightTarget::ShadowMap, stencil: None, draws: true },
    LightStep { layer: 11, target: LightTarget::View, stencil: Some(0), draws: true },
];

/// What the cut caster puts into its object shadow.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub groups: Vec<SceneGroup>,
    pub weight: Option<f32>,
    pub filter: Option<ShadowFilter>,
}

/// The object shadow's groups from the lists the mask branch's item body
/// just made, before its layer 10 begins, with the pass flags and
/// definition its layer builds would read and the projection its shadow
/// setup stored. Empty when the branch draws no layer; `None` when a
/// group does not read. Reads only.
pub fn groups(mem: &dyn GuestMemory) -> Option<Vec<SceneGroup>> {
    groups_or_refusal(mem).ok()
}

/// Why the object shadow's groups were not made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The lists' masks, the pass flags or the definition did not read.
    Unread,
    /// The shadow projection block did not read.
    Projection,
    /// A group did not read whole.
    Group(GroupRefusal),
    /// The filter `0x000222a0` would draw did not compute.
    Filter,
}

impl Refused {
    /// The cause as the panic names it (`group entry_state`).
    pub fn message(self) -> String {
        match self {
            Refused::Unread => "unread".to_owned(),
            Refused::Projection => "projection".to_owned(),
            Refused::Group(r) => format!("group {}", r.name()),
            Refused::Filter => "filter".to_owned(),
        }
    }
}

/// [`groups`], or why not.
pub fn groups_or_refusal(mem: &dyn GuestMemory) -> Result<Vec<SceneGroup>, Refused> {
    let word = |at| mem.u32(at).map_err(|_| Refused::Unread);
    let casters = word(CASTERS + 8)?;
    let receivers = word(RECEIVERS + 8)?;
    if casters & (1 << 10) == 0 || receivers & (1 << 11) == 0 {
        return Ok(Vec::new());
    }
    let flags = word(lp::global::PASS_FLAGS)?;
    let definition_flags = word(word(lp::global::DEFINITION)?)?;
    let projection = light_groups::projection(&Ro(mem)).ok_or(Refused::Projection)?;
    typed_groups::plan_or_refusal(
        mem,
        GroupSite::ObjectShadow,
        &STEPS,
        flags,
        definition_flags,
        Some(projection),
    )
    .map_err(Refused::Group)
}

/// [`groups`], the weight the branch published and the filter
/// `0x000222a0` would draw between its layers.
pub fn plan(mem: &dyn GuestMemory, fpu: Fpu) -> Result<Planned, Refused> {
    let groups = groups_or_refusal(mem)?;
    if groups.is_empty() {
        return Ok(Planned { groups, weight: None, filter: None });
    }
    let weight = mem.u32(light_groups::OBJECT_SHADOW_WEIGHT).ok().map(f32::from_bits);
    let filter = Some(light_groups::shadow_filter(mem, fpu.mxcsr, fpu.x87_control).ok_or(Refused::Filter)?);
    Ok(Planned { groups, weight, filter })
}

static CUT: AtomicU64 = AtomicU64::new(0);

/// Object shadows the cut caster put into the scene.
pub fn cut_count() -> u64 {
    CUT.load(Ordering::Relaxed)
}

/// The cut caster's item lists are made: its groups into the open object
/// shadow. A shadow that does not plan, or that the scene refuses, panics
/// (`lights: object_shadow plan <cause>`, `lights: object_shadow scene`).
pub fn cut(mem: &dyn GuestMemory, fpu: Fpu) {
    let p = match plan(mem, fpu) {
        Ok(p) => p,
        Err(why) => unimplemented!("lights: object_shadow plan {}", why.message()),
    };
    if !crate::scene_step::object_shadow_groups(p.groups, p.weight, p.filter) {
        unimplemented!("lights: object_shadow scene");
    }
    CUT.fetch_add(1, Ordering::Relaxed);
}

static DRAWN: AtomicU64 = AtomicU64::new(0);

/// Object shadows the host drew from the scene.
pub fn drawn() -> u64 {
    DRAWN.load(Ordering::Relaxed)
}

/// Draw the open view's object shadows, called before layer 7's draw.
/// Returns the shadows drawn.
pub fn draw(mem: &mut dyn GuestMemory, passes: &[ObjectShadowPass]) -> usize {
    if passes.is_empty() || Stage::Lights.standing_down() {
        return 0;
    }
    let Some(view_target) = VIEW_TARGET.get() else { return 0 };
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return 0;
    };
    let drawn = host.draw_object_shadows(mem, ObjectShadowDraw { passes, view_target });
    DRAWN.fetch_add(drawn as u64, Ordering::Relaxed);
    drawn
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use engine::FlatMemory;
    use frame_scene::{GroupSite, LightGroup, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};

    use super::*;
    use crate::native_lights::{Dispatch, Host, set_host};

    struct Records(Mutex<Vec<u8>>);
    impl Host for Records {
        fn submit(&self, _: &mut dyn GuestMemory, _: Dispatch) -> bool {
            false
        }
        fn clear_admission(&self) {}
        fn checkpoint_gel(&self, _: u64) {}
        fn target(&self) -> Option<[u32; 8]> {
            None
        }
        fn draw_object_shadows(&self, _: &mut dyn GuestMemory, d: ObjectShadowDraw<'_>) -> usize {
            self.0.lock().unwrap().extend(d.passes.iter().filter_map(|p| p.slot.number()));
            d.passes.len()
        }
    }

    fn scene() -> std::sync::Arc<FrameScene> {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey { kind: ViewKind::Game, player: 0, view: 0 },
            frame_scene::RenderCamera {
                position: [0.0; 3],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                fov: 1.0,
                projection_xy: None,
                viewport: [0, 0, 480, 640],
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        b.push_object_shadow([7, 8], false).unwrap();
        for layer in [10, 11] {
            b.push_shadow_group(
                LightGroup {
                    light: 0,
                    layer,
                    site: GroupSite::ObjectShadow,
                    projection: None,
                    pass: None,
                    entries: 0..0,
                    remainder: 0,
                },
                &[],
            )
            .unwrap();
        }
        b.seal(Stamp::default()).unwrap()
    }

    #[test]
    fn object_shadows_draw_before_layer_7() {
        let scene = scene();
        let passes = gather(&scene, &scene.views[0]);
        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].identity, [7, 8]);
        assert_eq!(
            passes[0].groups.iter().map(|g| g.group.layer).collect::<Vec<_>>(),
            [10, 11],
            "the caster's layer, then the receiver's"
        );
        // Slot 3 holds the shadows and then layer 7: the shadows are
        // handed over at layer 7's draw, ahead of it.
        assert_eq!(passes[0].slot, native_render::layer_items::world_slot(7).unwrap());
        let host = Arc::new(Records(Mutex::new(Vec::new())));
        set_host(host.clone());
        VIEW_TARGET.set(Some([1; 8]));
        let mut m = FlatMemory::new(0, 0x1000);
        assert_eq!(draw(&mut m, &passes), 1);
        assert_eq!(*host.0.lock().unwrap(), [3]);
    }

    #[test]
    fn a_shadow_that_does_not_plan_names_why() {
        use scene_extract::light_groups::Cause;
        let fpu = Fpu { mxcsr: 0x1f80, x87_control: 0x027f };
        let small = FlatMemory::new(0, 0x1000);
        assert_eq!(plan(&small, fpu), Err(Refused::Unread), "the lists' masks do not read");
        let mut m = FlatMemory::new(0, 0x0060_0000);
        assert_eq!(plan(&m, fpu).unwrap().groups, Vec::new(), "no layer 10 or 11: nothing to draw");
        m.set_u32(CASTERS + 8, 1 << 10).unwrap();
        m.set_u32(RECEIVERS + 8, 1 << 11).unwrap();
        m.set_u32(lp::global::DEFINITION, 0x0100_0000).unwrap();
        assert_eq!(plan(&m, fpu), Err(Refused::Unread), "the definition does not read");
        assert_eq!(Refused::Group(GroupRefusal::Entry(Cause::State)).message(), "group entry_state");
        assert_eq!(Refused::Filter.message(), "filter");
    }
}
