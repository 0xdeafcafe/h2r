//! The view's object shadows handed to the host from the scene, in slot 3
//! before layer 7 (docs/graphics/native-shadows.md, "The object shadows
//! from the scene"). Each is the caster's layer 10 and 11 groups with the
//! filter between them, as `0x00032350` made them. The hand-off is held
//! ([`HELD`]) while the guest's `0x00032350` still draws: the cut keeps the
//! guest's tail until the host's draw is proven on the lab.

use std::sync::atomic::{AtomicU64, Ordering};

use engine::memory::GuestMemory;
use frame_scene::{FrameScene, ShadowFilter, View, ViewSlot};

use super::{HOST, SceneGroup, VIEW_TARGET};
use crate::native_map_work::Stage;

/// While true the host is not asked and the guest's tail draws.
pub const HELD: bool = true;

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

static DRAWN: AtomicU64 = AtomicU64::new(0);

/// Object shadows the host drew from the scene.
pub fn drawn() -> u64 {
    DRAWN.load(Ordering::Relaxed)
}

/// Draw the open view's object shadows, called before layer 7's draw.
/// Returns the shadows drawn; none while [`HELD`].
pub fn draw(mem: &mut dyn GuestMemory, passes: &[ObjectShadowPass]) -> usize {
    draw_with(mem, passes, HELD)
}

fn draw_with(mem: &mut dyn GuestMemory, passes: &[ObjectShadowPass], held: bool) -> usize {
    if held || passes.is_empty() || Stage::Lights.standing_down() {
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
        assert_eq!(passes[0].slot, native_render::layer_pass::world_slot(7).unwrap());
        let host = Arc::new(Records(Mutex::new(Vec::new())));
        set_host(host.clone());
        VIEW_TARGET.set(Some([1; 8]));
        let mut m = FlatMemory::new(0, 0x1000);
        assert_eq!(draw(&mut m, &passes), 0, "held: the guest's tail draws");
        assert_eq!(draw_with(&mut m, &passes, false), 1);
        assert_eq!(*host.0.lock().unwrap(), [3]);
    }
}
