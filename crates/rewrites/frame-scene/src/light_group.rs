//! A light's gathered groups (docs/graphics/framescene.md, slice 4,
//! "The light passes and the shadow layers"): per layer a light's passes
//! draw, the sorted entries of the light's or the casters' list as the
//! layer build `0x0004cbf0` leaves them, each with what the guest's first
//! geometry handler `0x0004d0b0` would otherwise supply: its streams (the
//! render section, part, vertex class, index range and topology), its
//! pose (world, a placement, or an object's node rows) and its bindings
//! (shader, pass selector, variant and implementation). The additional
//! casters' pair (`0x00030ed0`) is groups of its own site. With these the
//! native light and shadow stages can plan a layer from the scene alone
//! (`native_render::light_scene`), with no guest handler run for it.
//! `scene_extract::light_groups` reads them at each layer build.

use std::ops::Range;

use serde::Serialize;

use crate::layer::LayerOwner;
use crate::scene::{FrameScene, Row};

/// Which call drew a group's layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum GroupSite {
    /// The light passes `0x00031170`: layers 10, 11, 14, 12 and 13.
    Light,
    /// The additional casters' pair `0x00030ed0`: its layers 10 and 11,
    /// on a suffix of the same lists, between the light's 11 and 14.
    AdditionalCasters,
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
}

/// Registers of the shadow projection, `c[40..48]`.
pub const PROJECTION_ROWS: usize = 9;

/// One layer of a light, gathered.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct LightGroup {
    /// Into [`FrameScene::lights`]: the view's light it belongs to.
    pub light: u32,
    pub layer: u8,
    pub site: GroupSite,
    /// The shadow projection block (`0x004c9798`, 36 words for
    /// `c[40..48]`) as the group's layer reads it, for layers 10 and 11;
    /// read at the build, after the shadow setup wrote it.
    pub projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
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

    /// Each group's light in the scene and the groups in light order,
    /// each group's entries and each entry's rows in bounds.
    pub(crate) fn check_light_groups(&self) -> Result<(), String> {
        let mut last = 0;
        for (n, g) in self.light_groups.iter().enumerate() {
            if g.light as usize >= self.lights.len() {
                return Err(format!("light group {n} names light {}, not in the scene", g.light));
            }
            if g.light < last {
                return Err(format!("light group {n} is not in light order"));
            }
            last = g.light;
            if g.entries.start > g.entries.end || g.entries.end as usize > self.light_entries.len() {
                return Err(format!("light group {n} entries {:?} outside 0..{}", g.entries, self.light_entries.len()));
            }
            for (k, e) in self.entries_of(g).iter().enumerate() {
                if let LightPose::Rows(r) = &e.pose
                    && (r.start > r.end || r.end as usize > self.rows.len())
                {
                    return Err(format!("light group {n} entry {k} rows {r:?} outside 0..{}", self.rows.len()));
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
    use crate::camera::tests::camera;
    use crate::light::{LightStep, LightTarget, SceneLight};
    use crate::scene::{InstanceKey, SceneId, Stamp, ViewKey, ViewKind};

    fn light(handle: u32) -> SceneLight {
        SceneLight { handle, definition: 0xe300_0000, flags: 4, definition_flags: 0, additional_casters: true, steps: 0..0 }
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
            binding: LightBinding { shader: 0xe000_0010, selector: 0, variant: 0, implementation: 2 },
        }
    }

    fn group(layer: u8, site: GroupSite) -> LightGroup {
        LightGroup { light: 0, layer, site, projection: None, entries: 0..0, remainder: 0 }
    }

    #[test]
    fn a_lights_groups_keep_their_entries_poses_and_the_additional_casters_in_order() {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        assert_eq!(b.push_light_group(group(10, GroupSite::Light), &[]).unwrap_err(), BuildError::NoView);
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera()).unwrap();
        assert_eq!(b.push_light_group(group(10, GroupSite::Light), &[]).unwrap_err(), BuildError::NoLight);
        let step = |layer| LightStep { layer, stencil: Some(0), target: LightTarget::View, draws: true };
        b.push_light(light(1), &[step(10), step(11), step(12)]).unwrap();
        let rows = vec![[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let projection = Some([[7; 4]; PROJECTION_ROWS]);
        let cluster = entry(0, LayerOwner::Cluster(3), LightPose::World);
        let placement = entry(1, LayerOwner::Placement(4), LightPose::Placement(4));
        let object = entry(2, LayerOwner::Object(InstanceKey(0xe400_0001)), LightPose::Rows(0..0));
        b.push_light_group(LightGroup { projection, ..group(10, GroupSite::Light) },
            &[(cluster.clone(), vec![]), (object.clone(), rows.clone()), (object.clone(), rows.clone())]).unwrap();
        b.push_light_group(group(10, GroupSite::AdditionalCasters), &[(placement.clone(), vec![])]).unwrap();
        b.push_light_group(LightGroup { remainder: 2, ..group(12, GroupSite::Light) }, &[]).unwrap();
        b.push_light(light(2), &[step(12)]).unwrap();
        b.push_light_group(group(12, GroupSite::Light), &[(cluster.clone(), vec![])]).unwrap();
        let s = b.seal(Stamp::default()).unwrap();
        s.check().unwrap();
        let first = s.groups_of(0);
        assert_eq!(first.iter().map(|g| (g.layer, g.site)).collect::<Vec<_>>(),
            [(10, GroupSite::Light), (10, GroupSite::AdditionalCasters), (12, GroupSite::Light)]);
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
}
