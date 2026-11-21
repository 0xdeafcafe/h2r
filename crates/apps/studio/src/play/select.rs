//! Picking uses the displayed sample, including salted handles and parent transforms.
use std::time::{Duration, Instant};

use agent_proto::live::Object;
use agent_proto::sim::objects::handle;
use live_mirror::Sample;
use map::TagId;
use scenario_edit::geometry::{self, Ray, Tri, Vec3, add, dot, scale, sub};
use scenario_edit::{BlockKind, Session};
use serde_json::{Value, json};

use crate::app::{Action, App, NavAction};
use crate::viewport::mirror::WorldPoses;

/// Eye above the unit origin in world units until the stream supplies its camera.
pub(crate) const EYE_HEIGHT: f32 = 0.6;
pub(crate) const AIM_DWELL: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hit {
    pub object: u32,
    pub placement: Option<(BlockKind, usize)>,
    pub tag: Option<u32>,
    pub point: Vec3,
    pub distance: f32,
}

impl Hit {
    pub(crate) fn json(&self) -> Value {
        json!({
            "object": handle(self.object),
            "placement": self.placement.map(|(kind,index)| json!({
                "block": crate::api::block_name(kind), "index": index
            })),
            "tag": self.tag.map(handle), "point": self.point, "distance": self.distance
        })
    }
}

/// The local documents and stream header a cached map match describes.
#[derive(Debug)]
struct Identity {
    world: u64,
    epoch: u64,
    stream_epoch: Option<u64>,
    checksum: Option<u64>,
    copy: Option<(u64, u32)>,
    name: Option<String>,
    matches: bool,
}

impl Identity {
    fn describes(&self, app: &App, sample: &Sample) -> bool {
        let bridge = &app.pairing;
        self.world == app.session.world_rev()
            && self.epoch == sample.epoch
            && self.stream_epoch == bridge.stream_map["epoch"].as_u64()
            && self.checksum == bridge.stream_map["checksum"].as_u64()
            && self.copy == bridge.play_copy
            && self.name.as_deref() == bridge.stream_map["name"].as_str()
    }

    fn read(app: &App, sample: &Sample) -> Self {
        let bridge = &app.pairing;
        Self {
            world: app.session.world_rev(),
            epoch: sample.epoch,
            stream_epoch: bridge.stream_map["epoch"].as_u64(),
            checksum: bridge.stream_map["checksum"].as_u64(),
            copy: bridge.play_copy,
            name: bridge.stream_map["name"].as_str().map(str::to_owned),
            matches: bridge.matches_sample(app, sample),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct LiveSelection {
    pub enabled: bool,
    pub candidate: Option<Hit>,
    identity: Option<Identity>,
    pub(crate) bounds: std::cell::RefCell<crate::viewport::mirror::ModelBounds>,
    dwell: Dwell,
    tick: Option<u64>,
    marker_revision: u64,
    // A loose live object remains distinct from a tag selected in the tree.
    selected: Option<(u32, (u64, u64, u64))>,
}

#[derive(Debug, Default)]
struct Dwell {
    held: Option<(u32, Instant)>,
    taken: bool,
}

impl Dwell {
    fn observe(&mut self, object: Option<u32>, now: Instant) -> bool {
        let Some(id) = object else {
            *self = Self::default();
            return false;
        };
        if let Some((prior, since)) = self.held
            && prior == id
        {
            if !self.taken && now.saturating_duration_since(since) >= AIM_DWELL {
                self.taken = true;
                return true;
            }
        } else {
            self.held = Some((id, now));
            self.taken = false;
        }
        false
    }
}

pub(crate) fn matching(app: &App) -> bool {
    let Some(sample) = &app.pairing.shown else {
        return false;
    };
    app.pairing
        .selection
        .identity
        .as_ref()
        .filter(|id| id.describes(app, sample))
        .map_or_else(|| app.pairing.matches_sample(app, sample), |id| id.matches)
}

pub(crate) fn local_pose(app: &App) -> Option<(u16, Vec3, Vec3)> {
    if !matching(app) {
        return None;
    }
    let (player, unit) = app.pairing.player()?;
    player.local_user?;
    let pos = WorldPoses::unit_position(app.pairing.shown.as_ref()?, unit)?;
    pos.into_iter()
        .all(f32::is_finite)
        .then_some((player.index, pos, player.aim))
}

pub(crate) fn player_ray(app: &App) -> Option<Ray> {
    let (_, pos, dir) = local_pose(app)?;
    unit_ray(add(pos, [0.0, 0.0, EYE_HEIGHT]), dir)
}

pub(crate) fn unit_ray(origin: Vec3, dir: Vec3) -> Option<Ray> {
    let length = dot(dir, dir).sqrt();
    if length <= 0.0 || !length.is_finite() || !origin.into_iter().all(f32::is_finite) {
        return None;
    }
    Some(Ray {
        origin,
        dir: dir.map(|d| d / length),
    })
}

pub(crate) fn placement(session: &Session, object: &Object) -> Option<(BlockKind, usize)> {
    let index = usize::from(object.placement?);
    let marker = session.markers().iter().find(|m| {
        m.index == index
            && scenario_edit::layout::block(m.block).object_type == Some(i16::from(object.kind))
    })?;
    Some((marker.block, index))
}

/// Nearest intersection in front of the level. Resolve the placement only for the winner.
fn aim_pick(
    session: &Session,
    sample: &Sample,
    ray: &Ray,
    wall: &[Tri],
    omit: [Option<u32>; 2],
    local_sphere: impl Fn(&Object) -> (Vec3, f32),
) -> Option<Hit> {
    let ray = unit_ray(ray.origin, ray.dir)?;
    let poses = WorldPoses::new(sample);
    let (distance, object) = sample
        .objects
        .iter()
        .filter(|o| !omit.contains(&Some(o.id)))
        .filter_map(|o| {
            let (offset, radius) = local_sphere(o);
            let (center, radius) = poses.sphere(o.id, offset, radius)?;
            let t = ray.sphere(center, radius)?;
            t.is_finite().then_some((t, o))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)))?;
    if geometry::surface(wall, &ray).is_some_and(|p| dot(sub(p, ray.origin), ray.dir) <= distance) {
        return None;
    }
    Some(Hit {
        object: object.id,
        tag: object.tag,
        placement: placement(session, object),
        point: add(ray.origin, scale(ray.dir, distance)),
        distance,
    })
}

fn pick_sample(app: &App, ray: &Ray, wall: &[Tri], omit: [Option<u32>; 2]) -> Option<Hit> {
    let sample = app.pairing.shown.as_ref()?;
    if !matching(app) {
        return None;
    }
    let mut bounds = app.pairing.selection.bounds.borrow_mut();
    bounds.prepare(&app.session, sample);
    let mut hit = aim_pick(&app.session, sample, ray, wall, omit, |o| {
        bounds.sphere(o.tag)
    })?;
    hit.placement = hit
        .placement
        .filter(|(kind, _)| app.pairing.placement_matches(&app.session, *kind));
    Some(hit)
}

pub(crate) fn pick(app: &App, ray: &Ray) -> Option<Hit> {
    pick_sample(app, ray, app.session.world().0, [None; 2])
}

pub(crate) fn viewport_pick(app: &App, ray: &Ray) -> Option<Hit> {
    use scenario_edit::layers::LayerId;
    if !app.layers.visible(LayerId::Live) {
        return None;
    }
    let wall = if app.layers.visible(LayerId::Structure) {
        app.session.world().0
    } else {
        &[]
    };
    pick_sample(app, ray, wall, [None; 2])
}

pub(crate) fn from_aim(app: &App) -> Option<Hit> {
    let ray = player_ray(app)?;
    let (player, _) = app.pairing.player()?;
    pick_sample(app, &ray, app.session.world().0, [player.unit, player.held])
}

pub(crate) fn selected_object(app: &App) -> Option<u32> {
    let (id, revision) = app.pairing.selection.selected?;
    (revision == app.selection_revision() && matching(app)).then_some(id)
}

pub(crate) fn apply(app: &mut App, hit: Hit, extend: bool) {
    if let Some(entry) = hit
        .placement
        .filter(|(kind, _)| app.pairing.placement_matches(&app.session, *kind))
    {
        app.pairing.selection.selected = None;
        app.dispatch(if extend {
            Action::ToggleSelect(entry)
        } else {
            Action::Select(Some(entry))
        });
    } else {
        app.sync_tags();
        let tag = hit.tag.map(TagId).filter(|&t| app.tags.tag(t).is_some());
        if let Some(tag) = tag {
            app.dispatch(Action::Navigate(NavAction::SelectTag(tag)));
        } else {
            app.dispatch(Action::Select(None));
        }
        app.pairing.selection.selected = Some((hit.object, app.selection_revision()));
    }
}

pub(crate) fn set_enabled(app: &mut App, enabled: bool) {
    if enabled == app.pairing.selection.enabled {
        return;
    }
    let selection = &mut app.pairing.selection;
    selection.enabled = enabled;
    selection.tick = None;
    selection.candidate = None;
    selection.dwell = Dwell::default();
    if enabled {
        update(app, Instant::now());
    }
}

pub(crate) fn update(app: &mut App, now: Instant) {
    let Some(sample) = app.pairing.shown.as_ref() else {
        if app.pairing.selection.identity.is_some() {
            app.pairing.selection = LiveSelection::default();
        }
        return;
    };
    if !app
        .pairing
        .selection
        .identity
        .as_ref()
        .is_some_and(|id| id.describes(app, sample))
    {
        let id = Identity::read(app, sample);
        if app.pairing.selection.identity.is_some() {
            app.pairing.selection = LiveSelection::default();
        }
        app.pairing.selection.identity = Some(id);
    }
    let state = &app.pairing.selection;
    if !state.identity.as_ref().is_some_and(|id| id.matches) {
        return;
    }
    let tick = sample.tick;
    let fresh = state.tick != Some(tick) || state.marker_revision != app.session.markers_rev();
    if let Some((id, rev)) = state.selected
        && (rev != app.selection_revision() || fresh && !sample.objects.iter().any(|o| o.id == id))
    {
        app.pairing.selection.selected = None;
    }
    if !app.pairing.selection.enabled || sample.stale {
        let state = &mut app.pairing.selection;
        state.tick = (!sample.stale).then_some(tick);
        state.marker_revision = app.session.markers_rev();
        state.candidate = None;
        state.dwell = Dwell::default();
        return;
    }
    if fresh {
        app.pairing.selection.marker_revision = app.session.markers_rev();
        let hit = from_aim(app);
        app.pairing.selection.candidate = hit;
        app.pairing.selection.tick = Some(tick);
    }
    let state = &mut app.pairing.selection;
    let hit = state.candidate;
    if state.dwell.observe(hit.map(|h| h.object), now)
        && let Some(hit) = hit
    {
        apply(app, hit, false);
    }
}

impl super::Play {
    pub(crate) fn can_aim_select(app: &App) -> bool {
        app.pairing.selection.enabled || matching(app)
    }

    pub(crate) fn set_aim_select(app: &mut App, enabled: bool) -> Result<(), &'static str> {
        if enabled && !matching(app) {
            return Err("aim select needs a live sample for the open map");
        }
        set_enabled(app, enabled);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(id: u32, x: f32) -> Object {
        Object {
            id,
            position: [x, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            scale: 1.0,
            ..Default::default()
        }
    }

    fn sample(objects: Vec<Object>) -> Sample {
        Sample {
            epoch: 1,
            tick: 1,
            stale: false,
            objects,
            players: vec![],
        }
    }

    #[test]
    fn aim_pick_finds_nearest_sphere_before_wall_and_none_behind_it() {
        let session = Session::new();
        let ray = Ray {
            origin: [0.0; 3],
            dir: [2.0, 0.0, 0.0],
        };
        let world = sample(vec![object(3, 10.0), object(2, 4.0), object(1, 2.0)]);
        let wall = |x| {
            [Tri {
                p: [[x, -2.0, -2.0], [x, 2.0, -2.0], [x, 0.0, 2.0]],
                n: [-1.0, 0.0, 0.0],
            }]
        };
        let pick = |x, omit| aim_pick(&session, &world, &ray, &wall(x), omit, |_| ([0.0; 3], 0.5));
        let hit = pick(8.0, [None; 2]).unwrap();
        assert_eq!(
            (hit.object, hit.point, hit.distance),
            (1, [1.5, 0.0, 0.0], 1.5)
        );
        assert!(pick(1.0, [None; 2]).is_none());
        // The aiming unit and its held object are not cursor targets.
        assert!(pick(8.0, [Some(1), Some(2)]).is_none());
    }

    #[test]
    fn aim_dwell_commits_once_after_a_continuous_hold() {
        let start = Instant::now();
        let mut dwell = Dwell::default();
        let short = AIM_DWELL - Duration::from_millis(1);
        assert!(!dwell.observe(Some(1), start));
        assert!(!dwell.observe(Some(1), start + short));
        assert!(!dwell.observe(Some(2), start + AIM_DWELL));
        assert!(!dwell.observe(None, start + 2 * AIM_DWELL));
        let restart = start + 3 * AIM_DWELL;
        assert!(!dwell.observe(Some(2), restart));
        assert!(!dwell.observe(Some(2), restart + short));
        assert!(dwell.observe(Some(2), restart + AIM_DWELL));
        assert!(!dwell.observe(Some(2), restart + 2 * AIM_DWELL));
    }

    #[test]
    fn parent_rotation_and_scale_place_model_sphere_in_world_space() {
        let parent = Object {
            forward: [0.0, 1.0, 0.0],
            scale: 2.0,
            ..object(1, 3.0)
        };
        let child = Object {
            parent: Some(parent.id),
            ..object(2, 2.0)
        };
        let sample = sample(vec![child, parent]);
        let ray = Ray {
            origin: [3.0, 0.0, 0.0],
            dir: [0.0, 1.0, 0.0],
        };
        let hit = aim_pick(&Session::new(), &sample, &ray, &[], [Some(1), None], |_| {
            ([1.0, 0.0, 0.0], 0.5)
        })
        .unwrap();
        assert_eq!((hit.object, hit.point), (2, [3.0, 5.0, 0.0]));
        assert_eq!(
            WorldPoses::unit_position(&sample, &child),
            Some([3.0, 4.0, 0.0])
        );
    }
}
