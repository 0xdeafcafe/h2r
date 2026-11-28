//! A light's gathered groups (docs/graphics/framescene.md, slice 4,
//! "The light passes and the shadow layers"): per layer a light's passes
//! draw, the sorted entries of the light's or the casters' list as the
//! layer build `0x0004cbf0` leaves them, each with what the guest's first
//! geometry handler `0x0004d0b0` would otherwise supply: its streams (the
//! render section, part, vertex class, index range and topology), its
//! pose (world, a placement, or an object's node rows) and its bindings
//! (shader, pass selector, variant and implementation). The additional
//! casters' pair (`0x00030ed0`) is groups of its own site, and each
//! object shadow (`0x00032350`, outside any light) an [`ObjectShadow`]
//! with its layers 10 and 11 as groups. With these the
//! native light and shadow stages can plan a layer from the scene alone
//! (`native_render::light_scene`), with no guest handler run for it.
//! `scene_extract::light_groups` reads them at each layer build.

use std::ops::Range;

use serde::Serialize;

use crate::layer::LayerOwner;
use crate::scene::{FrameScene, InstanceKey, Row};

/// Which call drew a group's layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum GroupSite {
    /// The light passes `0x00031170`: layers 10, 11, 14, 12 and 13.
    Light,
    /// The additional casters' pair `0x00030ed0`: its layers 10 and 11,
    /// on a suffix of the same lists, between the light's 11 and 14.
    AdditionalCasters,
    /// An object shadow `0x00032350`: its layers 10 and 11, into
    /// [`FrameScene::shadow_groups`].
    ObjectShadow,
    /// The first-person lights `0x000404b0`: a light's layers 11, 14, 12
    /// and 13 on its list rebuilt from its slot's records, in the first
    /// person's view (docs/graphics/native-light-gather.md, slice L6).
    FirstPerson,
}

/// How a part's indices draw (the emitter `0x0004daa0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Topology {
    List,
    /// Record bit 9 on a part without the triangle-list flag.
    Strip,
}

/// Where an entry's vertices and indices are: one part of a render
/// section, as the native geometry holds it by the entry's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightStream {
    /// The model's section, the cluster, or the placement's definition.
    pub section: u16,
    /// The part (the item argument's low byte).
    pub part: u8,
    /// The vertex classification the build chose (the sorted entry's
    /// `+8`).
    pub vertex_class: u16,
    /// The part's indices in its section's index buffer (part `+6`,
    /// `+8`).
    pub first_index: u32,
    pub index_count: u32,
    pub topology: Topology,
}

/// What places an entry's vertices in the world.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum LightPose {
    /// A structure cluster's: world space.
    World,
    /// An instanced-geometry placement's transform, by placement index;
    /// the native world holds it.
    Placement(u16),
    /// An object's node rows its record uploads at `c[50..]`, into
    /// [`FrameScene::rows`].
    Rows(Range<u32>),
}

/// What an entry's draw binds of its material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightBinding {
    /// The shader tag datum (item `+8`).
    pub shader: u32,
    /// The pass among the layer's (item `+0x14`).
    pub selector: u8,
    /// The template variant (item `+0x15`).
    pub variant: u8,
    /// The implementation the build chose for the layer (the sorted
    /// entry's `+4`).
    pub implementation: u8,
}

/// What an entry's draw prologue `0x0004d640` sets beyond its bindings,
/// which the native draw makes itself (docs/graphics/native-lights.md,
/// "Entry state"). The handler's epilogue `0x0004d720` puts both back,
/// so the host takes such an entry only while the device holds what the
/// epilogue leaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct EntryState {
    /// Record bit 11, an object's or the structure's: the auxiliary
    /// state toggle `0x00012d50`. The part draws with its view's
    /// [`crate::AuxDepth`].
    pub aux_depth: bool,
    /// The record's fade byte (`+0x1a`) is below `0xff`: the polygon
    /// stipple pattern it selects, which masks nothing while the
    /// device's stipple is off.
    pub fade: bool,
    /// The shader's flag bit 0 (`+0x16`): the draw asks for the stage
    /// bitmaps' residency, which sets no device state. Held where
    /// [`Self::structure`] is held and, with [`Self::uncopied`], no
    /// program of the layer has flag bit 20.
    pub shader_flag: bool,
    /// With [`Self::shader_flag`]: the active structure has a parameter
    /// block (`+0x214` above 0), which the prologue uploads
    /// (`0x0003c650`, `0x0003d000`). The host takes the entry only where
    /// the device and the engine already hold every word of it.
    pub structure: bool,
    /// With [`Self::shader_flag`]: the frame's structure target copy is
    /// not made yet, so a program with flag bit 20 would make it
    /// (`0x0003bea0`), which no native stage does.
    pub uncopied: bool,
}

impl EntryState {
    /// The handler sets nothing beyond the bindings.
    pub fn plain(self) -> bool {
        self == Self::default()
    }
}

/// One sorted entry of a group, in the order the handlers would run.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightEntry {
    /// The item's index in its list (the sorted entry's `+0`).
    pub slot: u16,
    /// The render record.
    pub record: u16,
    pub owner: LayerOwner,
    /// The record's model (`+4`); `u32::MAX` for the structure's.
    pub model: u32,
    pub stream: LightStream,
    pub pose: LightPose,
    pub binding: LightBinding,
    /// What its prologue sets beyond the bindings.
    pub state: EntryState,
    /// The record's visible subparts (bit 6 with a mask): the spans of
    /// the section's source indices the emitter draws, each its own strip
    /// in strip topology (`scene_extract::layout::world_ranges::emissions`);
    /// `None` draws the whole part. Every layer takes them
    /// (docs/graphics/native-lights.md, "Visible subparts").
    pub subparts: Option<Vec<Range<u32>>>,
}

/// Registers of the shadow projection, `c[40..48]`.
pub const PROJECTION_ROWS: usize = 9;

/// The state a shadow layer's begin `0x00044370` and the receiver setup
/// `0x000496f0` leave, read at the layer's build. Raw words stay as the
/// game stored them so the native pass can check the device against them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ShadowPass {
    /// The selected render-target index (`0x004858b4`).
    pub target: u32,
    /// The target's guest size in pixels.
    pub size: [u32; 2],
    /// The light pass flags (`0x004ba014`).
    pub flags: u32,
    pub clip_control: u32,
    pub raster: u32,
    pub offset_enable: u32,
    pub offset_constant: u32,
    pub offset_slope: u32,
    pub stencil_mode: u32,
    pub shadow_function: u32,
}

impl ShadowPass {
    /// Layer 10 writes the small colour mask rather than the depth map.
    pub fn colour_mask(&self) -> bool {
        self.flags & (1 << 4 | 1 << 5) == 1 << 4
    }
}

/// One layer of a light, gathered.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightGroup {
    /// Into [`FrameScene::lights`]: the view's light it belongs to; for
    /// [`GroupSite::ObjectShadow`], into [`FrameScene::object_shadows`].
    pub light: u32,
    pub layer: u8,
    pub site: GroupSite,
    /// The shadow projection block (`0x004c9798`, 36 words for
    /// `c[40..48]`) as the group's layer reads it, for layers 10 and 11;
    /// read at the build, after the shadow setup wrote it.
    pub projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
    /// The layer's target and raster state, for layers 10, 11 and 14.
    pub pass: Option<ShadowPass>,
    /// Into [`FrameScene::light_entries`], in the sorted order, duplicates
    /// kept.
    pub entries: Range<u32>,
    /// Sorted entries extraction could not read: the guest's handlers
    /// still draw them.
    pub remainder: u32,
}

impl LightGroup {
    /// Every sorted entry is in the scene: the layer can draw from it
    /// alone.
    pub fn whole(&self) -> bool {
        self.remainder == 0
    }
}

pub const SHADOW_COMBINER_WORDS: usize = 60;

/// One quad of an object shadow's filter `0x000222a0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ShadowFilterDraw {
    pub source: u32,
    pub dest: u32,
    /// Spread in guest texels, as f32 bits.
    pub spread: u32,
}

fn serialize_words<S: serde::Serializer>(words: &[u32], s: S) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(words, s)
}

/// The object filter's ping-pong quads and final copy, with the combiner
/// and tap signs the native quad shader needs.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ShadowFilter {
    pub draws: Vec<ShadowFilterDraw>,
    pub copy: Option<(u32, u32)>,
    #[serde(serialize_with = "serialize_words")]
    pub combiner: [u32; SHADOW_COMBINER_WORDS],
    pub signs: [u32; 2],
    /// The colour-mask target's guest size (targets 10 and 11).
    pub size: [u32; 2],
}

/// One object shadow `0x00032350` of a view, in the guest's order: the
/// caster gathered by `0x00033400`, its layers' groups after it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct ObjectShadow {
    /// Into [`FrameScene::views`]: the view it was drawn in.
    pub view: u32,
    /// The call's two arguments, the caster entry's `+8` and `+0xc`:
    /// the caster's object handle and its ranked fade.
    pub identity: [u32; 2],
    /// Into [`FrameScene::shadow_groups`].
    pub groups: Range<u32>,
    /// The caster is a creature (object type 12), which takes the
    /// ground-shadow branch `0x00032ce0` and its own geometry, not the
    /// mask branch.
    pub creature: bool,
    /// The light weight the mask branch publishes before its layers
    /// (`0x00032912`): the sample's strength times the caster's two
    /// fades, as f32 bits, read at its layer 11. `None` when that layer
    /// was not gathered or the word did not read.
    pub weight_bits: Option<u32>,
    /// The filter `0x000222a0` between layers 10 and 11.
    pub filter: Option<ShadowFilter>,
}

impl ObjectShadow {
    /// Minimum weight of an ordinary mask draw (0x00032438).
    pub const DRAW_THRESHOLD: f32 = 0.09;

    /// The caster's object handle.
    pub fn caster(&self) -> InstanceKey {
        InstanceKey(self.identity[0])
    }

    /// [`Self::weight_bits`] as a number.
    pub fn weight(&self) -> Option<f32> {
        self.weight_bits.map(f32::from_bits)
    }

    /// The direction towards the light, unnormalized: hardware `c[41].xyz`
    /// of the projection its layer 11 (else layer 10) read (`0x00048f85`,
    /// `engine::shadow_constants`). `None` without a projection.
    pub fn towards_light(&self, scene: &FrameScene) -> Option<[f32; 3]> {
        let groups = scene.shadow_groups_of(self);
        let projection = groups
            .iter()
            .find(|g| g.layer == 11 && g.projection.is_some())
            .or_else(|| {
                groups
                    .iter()
                    .find(|g| g.layer == 10 && g.projection.is_some())
            })?
            .projection?;
        let [x, y, z, _] = projection[1].map(f32::from_bits);
        Some([x, y, z])
    }
}

fn slice<'a, T>(v: &'a [T], r: &Range<u32>) -> &'a [T] {
    v.get(r.start as usize..r.end as usize).unwrap_or(&[])
}

impl FrameScene {
    /// The groups of light `light` (an index into [`Self::lights`]), in
    /// the passes' order.
    pub fn groups_of(&self, light: u32) -> &[LightGroup] {
        let start = self.light_groups.partition_point(|g| g.light < light);
        let end = self.light_groups.partition_point(|g| g.light <= light);
        &self.light_groups[start..end.max(start)]
    }

    /// The object shadows of view `view` (an index into [`Self::views`]).
    pub fn object_shadows_of(&self, view: u32) -> &[ObjectShadow] {
        let start = self.object_shadows.partition_point(|o| o.view < view);
        let end = self.object_shadows.partition_point(|o| o.view <= view);
        &self.object_shadows[start..end.max(start)]
    }

    pub fn shadow_groups_of(&self, shadow: &ObjectShadow) -> &[LightGroup] {
        slice(&self.shadow_groups, &shadow.groups)
    }

    pub fn entries_of(&self, group: &LightGroup) -> &[LightEntry] {
        slice(&self.light_entries, &group.entries)
    }

    /// An entry's node rows; empty for a world or placement pose.
    pub fn pose_rows(&self, entry: &LightEntry) -> &[Row] {
        match &entry.pose {
            LightPose::Rows(r) => slice(&self.rows, r),
            _ => &[],
        }
    }

    /// Each group's light or object shadow in the scene and the groups in
    /// its order, each object shadow's view and groups, each group's
    /// entries and each entry's rows in bounds.
    pub(crate) fn check_light_groups(&self) -> Result<(), String> {
        let mut last = 0;
        for (n, o) in self.object_shadows.iter().enumerate() {
            if o.view as usize >= self.views.len() || o.view < last {
                return Err(format!(
                    "object shadow {n} names view {}, out of the scene or its order",
                    o.view
                ));
            }
            last = o.view;
            if o.groups.start > o.groups.end || o.groups.end as usize > self.shadow_groups.len() {
                return Err(format!(
                    "object shadow {n} groups {:?} outside 0..{}",
                    o.groups,
                    self.shadow_groups.len()
                ));
            }
            if self
                .shadow_groups_of(o)
                .iter()
                .any(|g| g.light as usize != n || g.site != GroupSite::ObjectShadow)
            {
                return Err(format!("object shadow {n} holds another's group"));
            }
        }
        let groups = self
            .light_groups
            .iter()
            .map(|g| (g, self.lights.len(), "light"))
            .chain(
                self.shadow_groups
                    .iter()
                    .map(|g| (g, self.object_shadows.len(), "object shadow")),
            );
        let mut last = (0, "light");
        for (n, (g, owners, what)) in groups.enumerate() {
            if g.light as usize >= owners {
                return Err(format!(
                    "group {n} names {what} {}, not in the scene",
                    g.light
                ));
            }
            if last.1 == what && g.light < last.0 {
                return Err(format!("group {n} is not in {what} order"));
            }
            if (what == "light") == (g.site == GroupSite::ObjectShadow) {
                return Err(format!("group {n} of site {:?} under a {what}", g.site));
            }
            last = (g.light, what);
            if g.entries.start > g.entries.end || g.entries.end as usize > self.light_entries.len()
            {
                return Err(format!(
                    "group {n} entries {:?} outside 0..{}",
                    g.entries,
                    self.light_entries.len()
                ));
            }
            for (k, e) in self.entries_of(g).iter().enumerate() {
                if let LightPose::Rows(r) = &e.pose
                    && (r.start > r.end || r.end as usize > self.rows.len())
                {
                    return Err(format!(
                        "group {n} entry {k} rows {r:?} outside 0..{}",
                        self.rows.len()
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{BuildError, SceneBuilder};
    use crate::light::{LightStep, LightTarget, SceneLight};
    use crate::scene::{InstanceKey, SceneId, Stamp, ViewKey, ViewKind};
    use crate::test_camera::camera;

    fn light(handle: u32) -> SceneLight {
        SceneLight {
            handle,
            definition: 0xe300_0000,
            flags: 4,
            definition_flags: 0,
            additional_casters: true,
            steps: 0..0,
            ..Default::default()
        }
    }

    fn entry(slot: u16, owner: LayerOwner, pose: LightPose) -> LightEntry {
        LightEntry {
            slot,
            record: slot + 1,
            owner,
            model: u32::MAX,
            stream: LightStream {
                section: 2,
                part: slot as u8,
                vertex_class: 0,
                first_index: 6 * u32::from(slot),
                index_count: 6,
                topology: Topology::List,
            },
            pose,
            binding: LightBinding {
                shader: 0xe000_0010,
                selector: 0,
                variant: 0,
                implementation: 2,
            },
            state: Default::default(),
            subparts: None,
        }
    }

    fn group(layer: u8, site: GroupSite) -> LightGroup {
        LightGroup {
            light: 0,
            layer,
            site,
            projection: None,
            pass: None,
            entries: 0..0,
            remainder: 0,
        }
    }

    #[test]
    fn a_lights_groups_keep_their_entries_poses_and_the_additional_casters_in_order() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(
            b.push_light_group(group(10, GroupSite::Light), &[])
                .unwrap_err(),
            BuildError::NoView
        );
        b.open_view(
            ViewKey {
                kind: ViewKind::Game,
                player: 0,
                view: 0,
            },
            camera(),
        )
        .unwrap();
        assert_eq!(
            b.push_light_group(group(10, GroupSite::Light), &[])
                .unwrap_err(),
            BuildError::NoLight
        );
        let step = |layer| LightStep {
            layer,
            stencil: Some(0),
            target: LightTarget::View,
            draws: true,
        };
        b.push_light(light(1), &[step(10), step(11), step(12)])
            .unwrap();
        let rows = vec![
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let projection = Some([[7; 4]; PROJECTION_ROWS]);
        let cluster = entry(0, LayerOwner::Cluster(3), LightPose::World);
        let placement = entry(1, LayerOwner::Placement(4), LightPose::Placement(4));
        let object = entry(
            2,
            LayerOwner::Object(InstanceKey(0xe400_0001)),
            LightPose::Rows(0..0),
        );
        b.push_light_group(
            LightGroup {
                projection,
                ..group(10, GroupSite::Light)
            },
            &[
                (cluster.clone(), vec![]),
                (object.clone(), rows.clone()),
                (object.clone(), rows.clone()),
            ],
        )
        .unwrap();
        b.push_light_group(
            group(10, GroupSite::AdditionalCasters),
            &[(placement.clone(), vec![])],
        )
        .unwrap();
        b.push_light_group(
            LightGroup {
                remainder: 2,
                ..group(12, GroupSite::Light)
            },
            &[],
        )
        .unwrap();
        b.push_light(light(2), &[step(12)]).unwrap();
        b.push_light_group(group(12, GroupSite::Light), &[(cluster.clone(), vec![])])
            .unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let first = s.groups_of(0);
        assert_eq!(
            first.iter().map(|g| (g.layer, g.site)).collect::<Vec<_>>(),
            [
                (10, GroupSite::Light),
                (10, GroupSite::AdditionalCasters),
                (12, GroupSite::Light)
            ]
        );
        assert_eq!(first[0].projection, projection);
        let entries = s.entries_of(&first[0]);
        assert_eq!(entries.len(), 3, "a duplicate entry stays");
        assert_eq!(entries[0], cluster);
        assert_eq!(s.pose_rows(&entries[1]), rows.as_slice());
        assert_eq!(s.pose_rows(&entries[2]), rows.as_slice());
        assert_ne!(entries[1].pose, entries[2].pose, "each entry its own rows");
        assert!(s.pose_rows(&entries[0]).is_empty());
        assert_eq!(s.entries_of(&first[1]), &[placement]);
        assert!(!first[2].whole() && first[0].whole());
        assert_eq!(s.groups_of(1).len(), 1);
        assert_eq!(s.groups_of(1)[0].light, 1);
        assert!(s.groups_of(2).is_empty());
        let mut bad = (*s).clone();
        bad.light_groups[0].entries = 0..100;
        assert!(bad.check().is_err());
        let mut bad = (*s).clone();
        bad.light_groups[0].light = 9;
        assert!(bad.check().is_err());
        let mut bad = (*s).clone();
        bad.light_entries[1].pose = LightPose::Rows(0..1000);
        assert!(bad.check().is_err());
    }

    #[test]
    fn an_object_shadows_layers_are_its_groups_outside_any_light() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey {
                kind: ViewKind::Game,
                player: 0,
                view: 0,
            },
            camera(),
        )
        .unwrap();
        assert_eq!(
            b.push_shadow_group(group(10, GroupSite::ObjectShadow), &[])
                .unwrap_err(),
            BuildError::NoShadow
        );
        let caster = entry(
            0,
            LayerOwner::Object(InstanceKey(0xe400_0001)),
            LightPose::Rows(0..0),
        );
        let rows = vec![[1.0, 2.0, 3.0, 4.0]; 3];
        assert_eq!(b.set_shadow_weight(0.5), Err(BuildError::NoShadow));
        assert_eq!(b.push_object_shadow([0x11, 0x12], false).unwrap(), 0);
        b.push_shadow_group(
            group(10, GroupSite::ObjectShadow),
            &[(caster.clone(), rows.clone())],
        )
        .unwrap();
        b.push_shadow_group(
            group(11, GroupSite::ObjectShadow),
            &[(entry(1, LayerOwner::Cluster(3), LightPose::World), vec![])],
        )
        .unwrap();
        b.open_view(
            ViewKey {
                kind: ViewKind::Game,
                player: 0,
                view: 1,
            },
            camera(),
        )
        .unwrap();
        assert_eq!(
            b.push_shadow_group(group(10, GroupSite::ObjectShadow), &[])
                .unwrap_err(),
            BuildError::NoShadow,
            "the last object shadow is another view's"
        );
        b.push_object_shadow([0x21, 0x22], true).unwrap();
        b.set_shadow_weight(0.25).unwrap();
        b.push_shadow_group(group(10, GroupSite::ObjectShadow), &[])
            .unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        assert!(s.light_groups.is_empty(), "no light holds them");
        let first = s.object_shadows_of(0);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].identity, [0x11, 0x12]);
        let groups = s.shadow_groups_of(&first[0]);
        assert_eq!(groups.iter().map(|g| g.layer).collect::<Vec<_>>(), [10, 11]);
        assert_eq!(s.pose_rows(&s.entries_of(&groups[0])[0]), rows.as_slice());
        assert_eq!(s.object_shadows_of(1)[0].identity, [0x21, 0x22]);
        assert_eq!((first[0].creature, first[0].weight()), (false, None));
        let second = &s.object_shadows_of(1)[0];
        assert_eq!(
            (second.creature, second.weight(), second.caster()),
            (true, Some(0.25), InstanceKey(0x21))
        );
        assert_eq!(s.shadow_groups_of(&s.object_shadows_of(1)[0])[0].light, 1);
        assert_eq!(first[0].towards_light(&s), None, "no projection was read");
        let mut bad = (*s).clone();
        bad.shadow_groups[0].site = GroupSite::Light;
        assert!(bad.check().is_err());
        let mut bad = (*s).clone();
        bad.object_shadows[1].view = 5;
        assert!(bad.check().is_err());
    }

    /// The direction is `c[41].xyz` of layer 11's projection, layer 10's
    /// when 11 read none.
    #[test]
    fn an_object_shadows_light_direction_is_c41_of_its_projection() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey {
                kind: ViewKind::Game,
                player: 0,
                view: 0,
            },
            camera(),
        )
        .unwrap();
        b.push_object_shadow([0x11, 0x12], false).unwrap();
        let block = |c41: [f32; 3]| {
            let mut p = [[0u32; 4]; PROJECTION_ROWS];
            p[1] = [c41[0].to_bits(), c41[1].to_bits(), c41[2].to_bits(), 7];
            Some(p)
        };
        let mut ten = group(10, GroupSite::ObjectShadow);
        ten.projection = block([0.0, 0.0, 1.0]);
        b.push_shadow_group(ten, &[]).unwrap();
        b.push_object_shadow([0x21, 0x22], false).unwrap();
        let mut ten = group(10, GroupSite::ObjectShadow);
        ten.projection = block([0.0, 0.0, 1.0]);
        b.push_shadow_group(ten, &[]).unwrap();
        let mut eleven = group(11, GroupSite::ObjectShadow);
        eleven.projection = block([1.0, -2.0, 3.0]);
        b.push_shadow_group(eleven, &[]).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        let shadows = s.object_shadows_of(0);
        assert_eq!(shadows[0].towards_light(&s), Some([0.0, 0.0, 1.0]));
        assert_eq!(shadows[1].towards_light(&s), Some([1.0, -2.0, 3.0]));
    }
}
