//! A light layer planned from the scene alone (docs/graphics/framescene.md,
//! slice 4, "The light passes and the shadow layers";
//! docs/graphics/native-lights.md, "Gathered groups in the scene"): each
//! of a gathered group's sorted entries ([`frame_scene::LightGroup`])
//! becomes a draw of the native geometry its stream names, with its pose
//! and bindings from the scene, in the sorted order, duplicates kept.
//! Nothing here reads guest memory or needs a guest handler to have run:
//! what the first geometry handler supplied to the D3D-snapshot path
//! (`boot::native_lights_glue::walk`), the scene carries.
//!
//! Each entry's part is looked up in the native geometry ([`Resident`]),
//! which gives its stream and its index range there. Adjacent list
//! entries of one stream, binding, vertex class and pose join into one
//! draw; strips never join. A group with a remainder, an entry whose part
//! the native geometry does not hold, or an object entry without node
//! rows, is refused by name and the layer keeps its handlers.
//! `boot::native_lights_glue` draws a planned group in layers 12 and 13:
//! structure geometry (clusters and placements) from the world's buffers,
//! object entries from the map's model parts built at load
//! ([`crate::light_models`]), a stream per model section.

use std::ops::Range;

use frame_scene::{FrameScene, GroupSite, LightBinding, LightEntry, LightGroup, LightPose, PROJECTION_ROWS, Row, Topology};

/// A resident stream, as the native geometry names it.
pub type StreamId = u32;

/// Where the native geometry holds an entry's part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidentPart {
    pub stream: StreamId,
    /// The part's indices in the stream's index buffer.
    pub indices: Range<u32>,
}

/// The native geometry a group's entries name.
pub trait Resident {
    /// Where `entry`'s part (its owner, model, section and part, with its
    /// shader) is held; `None` when it is not, or not for that shader.
    fn part(&self, entry: &LightEntry) -> Option<ResidentPart>;
}

/// Why a group is not planned: its layer keeps its handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Sorted entries extraction left to the guest.
    Remainder(u32),
    /// An entry's part is not resident.
    Stream,
    /// An object entry's node rows are empty.
    Pose,
}

impl Refusal {
    pub fn name(self) -> &'static str {
        match self {
            Refusal::Remainder(_) => "remainder",
            Refusal::Stream => "stream",
            Refusal::Pose => "pose",
        }
    }
}

/// One draw of a planned group.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub stream: StreamId,
    pub vertex_class: u16,
    /// Indices in the stream's index buffer.
    pub indices: Range<u32>,
    pub topology: Topology,
    /// The first entry's pose; a [`LightPose::Rows`] range is into the
    /// scene's rows.
    pub pose: LightPose,
    pub binding: LightBinding,
    /// The group's sorted entries it draws, by index among the group's.
    pub entries: Range<u32>,
}

/// A group's draws, in the sorted order.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub light: u32,
    pub layer: u8,
    pub site: GroupSite,
    /// `c[40..48]` for layers 10 and 11.
    pub projection: Option<[[u32; 4]; PROJECTION_ROWS]>,
    pub draws: Vec<Draw>,
}

impl Plan {
    /// The sorted entries the plan draws: the handlers it stands in for.
    pub fn entries(&self) -> u32 {
        self.draws.iter().map(|d| d.entries.len() as u32).sum()
    }
}

fn same_pose<'r>(rows: &dyn Fn(&LightEntry) -> &'r [Row], a: &LightEntry, b: &LightEntry) -> bool {
    match (&a.pose, &b.pose) {
        (LightPose::World, LightPose::World) => true,
        (LightPose::Placement(x), LightPose::Placement(y)) => x == y,
        (LightPose::Rows(_), LightPose::Rows(_)) => a.record == b.record && rows(a) == rows(b),
        _ => false,
    }
}

/// The group's draws from the scene, or why not.
pub fn plan(scene: &FrameScene, group: &LightGroup, resident: &dyn Resident) -> Result<Plan, Refusal> {
    if !group.whole() {
        return Err(Refusal::Remainder(group.remainder));
    }
    let draws = plan_entries(scene.entries_of(group), &|e| scene.pose_rows(e), resident)?;
    Ok(Plan { light: group.light, layer: group.layer, site: group.site, projection: group.projection, draws })
}

/// [`plan`]'s draws for a whole group's `entries`, each object entry's
/// node rows from `rows`.
pub fn plan_entries<'r>(
    entries: &[LightEntry],
    rows: &dyn Fn(&LightEntry) -> &'r [Row],
    resident: &dyn Resident,
) -> Result<Vec<Draw>, Refusal> {
    let mut draws: Vec<Draw> = Vec::with_capacity(entries.len());
    let mut last: Option<&LightEntry> = None;
    for (n, e) in entries.iter().enumerate() {
        let part = resident.part(e).ok_or(Refusal::Stream)?;
        if matches!(e.pose, LightPose::Rows(_)) && rows(e).is_empty() {
            return Err(Refusal::Pose);
        }
        let n = n as u32;
        let joins = last.is_some_and(|l| {
            let d = draws.last().expect("a draw for the last entry");
            e.stream.topology == Topology::List
                && d.topology == Topology::List
                && d.stream == part.stream
                && d.vertex_class == e.stream.vertex_class
                && d.binding == e.binding
                && d.indices.end == part.indices.start
                && same_pose(rows, l, e)
        });
        if joins {
            let d = draws.last_mut().expect("a draw for the last entry");
            d.indices.end = part.indices.end;
            d.entries.end = n + 1;
        } else {
            draws.push(Draw {
                stream: part.stream,
                vertex_class: e.stream.vertex_class,
                indices: part.indices,
                topology: e.stream.topology,
                pose: e.pose.clone(),
                binding: e.binding,
                entries: n..n + 1,
            });
        }
        last = Some(e);
    }
    Ok(draws)
}

/// Each group of `scene`'s lights of `view` that plans, or why not, in
/// light and passes' order.
pub fn plan_view<'a>(
    scene: &'a FrameScene,
    view: &'a frame_scene::View,
    resident: &'a dyn Resident,
) -> impl Iterator<Item = (&'a LightGroup, Result<Plan, Refusal>)> + 'a {
    view.lights.clone().flat_map(move |l| scene.groups_of(l).iter().map(move |g| (g, plan(scene, g, resident))))
}

#[cfg(test)]
mod tests {
    use frame_scene::{
        InstanceKey, LightStep, LightStream, LightTarget, SceneBuilder, SceneId, SceneLight, Stamp, ViewKey, ViewKind,
    };

    use super::*;
    use frame_scene::LayerOwner;

    /// Holds every part but cluster 9's and placement 7's, its indices
    /// where the guest's are, each owner kind its own stream.
    struct Held;

    impl Resident for Held {
        fn part(&self, e: &LightEntry) -> Option<ResidentPart> {
            let stream = match e.owner {
                LayerOwner::Cluster(c) if c != 9 => 1000 + u32::from(e.stream.section),
                LayerOwner::Placement(p) if p != 7 => 2000 + u32::from(e.stream.section),
                LayerOwner::Object(_) => e.model ^ u32::from(e.stream.section),
                _ => return None,
            };
            let first = e.stream.first_index;
            Some(ResidentPart { stream, indices: first..first + e.stream.index_count })
        }
    }

    fn camera() -> frame_scene::Camera {
        let mut m = scene_extract::testing::Mem::default();
        scene_extract::testing::camera_record(&mut m, 0x1000);
        scene_extract::layout::camera::read(&m, 0x1000).unwrap()
    }

    fn entry(slot: u16, owner: LayerOwner, pose: LightPose, first: u32, topology: Topology) -> LightEntry {
        LightEntry {
            slot,
            record: 1,
            owner,
            model: u32::MAX,
            stream: LightStream { section: 2, part: slot as u8, vertex_class: 0, first_index: first, index_count: 6, topology },
            pose,
            binding: LightBinding { shader: 0xe000_0010, selector: 0, variant: 0, implementation: 3 },
        }
    }

    fn scene(groups: &[(LightGroup, Vec<(LightEntry, Vec<frame_scene::Row>)>)]) -> std::sync::Arc<FrameScene> {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(ViewKey { kind: ViewKind::Game, player: 0, view: 0 }, camera()).unwrap();
        let light = SceneLight { handle: 1, definition: 2, flags: 7, definition_flags: 0, additional_casters: true, steps: 0..0 };
        b.push_light(light, &[LightStep { layer: 12, stencil: Some(0), target: LightTarget::View, draws: true }]).unwrap();
        for (g, e) in groups {
            b.push_light_group(g.clone(), e).unwrap();
        }
        b.seal(Stamp::default()).unwrap()
    }

    fn group(layer: u8, site: GroupSite) -> LightGroup {
        LightGroup { light: 0, layer, site, projection: None, entries: 0..0, remainder: 0 }
    }

    const W: LightPose = LightPose::World;
    const LIST: Topology = Topology::List;

    #[test]
    fn a_group_draws_alone_in_sorted_order_joining_adjacent_list_ranges_only() {
        let rows = vec![[1.0, 0.0, 0.0, 0.0]; 3];
        let object = LayerOwner::Object(InstanceKey(0xe400_0001));
        let entries = vec![
            (entry(0, LayerOwner::Cluster(2), W, 0, LIST), vec![]),
            (entry(1, LayerOwner::Cluster(2), W, 6, LIST), vec![]),
            // A duplicate: drawn again, not joined backwards.
            (entry(1, LayerOwner::Cluster(2), W, 6, LIST), vec![]),
            (entry(3, LayerOwner::Cluster(2), W, 12, Topology::Strip), vec![]),
            (entry(4, LayerOwner::Cluster(2), W, 18, Topology::Strip), vec![]),
            (entry(5, LayerOwner::Placement(4), LightPose::Placement(4), 24, LIST), vec![]),
            (LightEntry { model: 0xe500_0000, ..entry(6, object, LightPose::Rows(0..0), 30, LIST) }, rows.clone()),
            (LightEntry { model: 0xe500_0000, ..entry(7, object, LightPose::Rows(0..0), 36, LIST) }, rows.clone()),
        ];
        let s = scene(&[(group(12, GroupSite::Light), entries)]);
        let g = &s.groups_of(0)[0];
        let p = plan(&s, g, &Held).unwrap();
        let ranges: Vec<_> = p.draws.iter().map(|d| (d.indices.clone(), d.entries.clone())).collect();
        assert_eq!(ranges, [(0..12, 0..2), (6..12, 2..3), (12..18, 3..4), (18..24, 4..5), (24..30, 5..6), (30..42, 6..8)]);
        assert_eq!(p.entries(), 8, "every handler of the layer");
        assert_eq!(p.draws[0].stream, 1002);
        assert_eq!(p.draws[4].stream, 2002);
        assert_eq!(p.draws[4].pose, LightPose::Placement(4));
        let LightPose::Rows(r) = &p.draws[5].pose else { panic!("an object's rows") };
        assert_eq!(&s.rows[r.start as usize..r.end as usize], rows.as_slice());
    }

    #[test]
    fn a_different_binding_class_or_pose_starts_a_draw() {
        let base = entry(0, LayerOwner::Cluster(2), W, 0, LIST);
        let next = |f: &dyn Fn(&mut LightEntry)| {
            let mut e = entry(1, LayerOwner::Cluster(2), W, 6, LIST);
            f(&mut e);
            e
        };
        for (n, e) in [
            next(&|e| e.binding.implementation = 2),
            next(&|e| e.binding.shader = 0xe000_0011),
            next(&|e| e.stream.vertex_class = 1),
            next(&|e| e.stream.first_index = 7),
            next(&|e| e.stream.section = 3),
        ]
        .into_iter()
        .enumerate()
        {
            let s = scene(&[(group(12, GroupSite::Light), vec![(base.clone(), vec![]), (e, vec![])])]);
            assert_eq!(plan(&s, &s.groups_of(0)[0], &Held).unwrap().draws.len(), 2, "case {n}");
        }
        let object = LayerOwner::Object(InstanceKey(1));
        let a = LightEntry { model: 5, ..entry(0, object, LightPose::Rows(0..0), 0, LIST) };
        let b = LightEntry { record: 2, ..LightEntry { model: 5, ..entry(1, object, LightPose::Rows(0..0), 6, LIST) } };
        let rows = |x| vec![[x, 0.0, 0.0, 0.0]; 3];
        let s = scene(&[(group(12, GroupSite::Light), vec![(a, rows(1.0)), (b, rows(2.0))])]);
        assert_eq!(plan(&s, &s.groups_of(0)[0], &Held).unwrap().draws.len(), 2, "another record's pose");
    }

    #[test]
    fn the_additional_casters_and_shadow_layers_plan_with_their_projection() {
        let projection = Some([[3; 4]; PROJECTION_ROWS]);
        let caster = (entry(0, LayerOwner::Cluster(2), W, 0, LIST), vec![]);
        let s = scene(&[
            (LightGroup { projection, ..group(10, GroupSite::Light) }, vec![caster.clone()]),
            (LightGroup { projection, ..group(10, GroupSite::AdditionalCasters) }, vec![caster.clone()]),
            (LightGroup { projection, ..group(11, GroupSite::AdditionalCasters) }, vec![caster]),
        ]);
        let v = &s.views[0];
        let plans: Vec<_> = plan_view(&s, v, &Held).map(|(_, p)| p.unwrap()).collect();
        assert_eq!(plans.iter().map(|p| (p.layer, p.site)).collect::<Vec<_>>(),
            [(10, GroupSite::Light), (10, GroupSite::AdditionalCasters), (11, GroupSite::AdditionalCasters)]);
        assert!(plans.iter().all(|p| p.projection == projection && p.draws.len() == 1));
    }

    #[test]
    fn a_remainder_a_missing_stream_or_pose_keeps_the_layers_handlers() {
        let s = scene(&[
            (LightGroup { remainder: 2, ..group(12, GroupSite::Light) }, vec![]),
            (group(12, GroupSite::Light), vec![(entry(0, LayerOwner::Cluster(9), W, 0, LIST), vec![])]),
            (group(12, GroupSite::Light), vec![(entry(0, LayerOwner::Placement(7), LightPose::Placement(7), 0, LIST), vec![])]),
            (group(12, GroupSite::Light), vec![(entry(0, LayerOwner::Object(InstanceKey(1)), LightPose::Rows(0..0), 0, LIST), vec![])]),
            (group(12, GroupSite::Light), vec![]),
        ]);
        let got: Vec<_> = s.groups_of(0).iter().map(|g| plan(&s, g, &Held).map(|p| p.draws.len())).collect();
        assert_eq!(got, [Err(Refusal::Remainder(2)), Err(Refusal::Stream), Err(Refusal::Stream), Err(Refusal::Pose), Ok(0)]);
    }
}
