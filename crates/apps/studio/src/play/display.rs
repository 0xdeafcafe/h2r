//! The exact or interpolated records presented by the editor this frame.
use std::time::Instant;

use super::stream::Captures;
use agent_proto::live::{Object, Player};
use live_mirror::Sample;

fn same_object(a: &Object, b: &Object) -> bool {
    a.id == b.id
        && a.tag == b.tag
        && a.kind == b.kind
        && a.flags == b.flags
        && a.parent == b.parent
        && a.placement == b.placement
        && a.position.map(f32::to_bits) == b.position.map(f32::to_bits)
        && a.forward.map(f32::to_bits) == b.forward.map(f32::to_bits)
        && a.up.map(f32::to_bits) == b.up.map(f32::to_bits)
        && a.scale.to_bits() == b.scale.to_bits()
        && a.velocity.map(f32::to_bits) == b.velocity.map(f32::to_bits)
        && a.body.to_bits() == b.body.to_bits()
        && a.shield.to_bits() == b.shield.to_bits()
}

fn same_player(a: &Player, b: &Player) -> bool {
    a.index == b.index
        && a.name == b.name
        && a.team == b.team
        && a.unit == b.unit
        && a.held == b.held
        && a.local_user == b.local_user
        && a.aim.map(f32::to_bits) == b.aim.map(f32::to_bits)
}

/// Wire floats include signed zero and NaN payloads. Equal payloads have
/// the same displayed revision, including for a headless viewport sample.
pub(crate) fn same_records(sample: &Sample, objects: &[Object], players: &[Player]) -> bool {
    sample.objects.len() == objects.len()
        && sample.players.len() == players.len()
        && sample
            .objects
            .iter()
            .zip(objects)
            .all(|(a, b)| same_object(a, b))
        && sample
            .players
            .iter()
            .zip(players)
            .all(|(a, b)| same_player(a, b))
}

fn same_bindings(a: &Sample, b: &Sample) -> bool {
    let objects = |o: &Object| (o.id, o.kind, o.placement, o.parent.is_some());
    let held = |p: &Player| p.held;
    a.objects.iter().map(objects).eq(b.objects.iter().map(objects))
        && a.players.iter().filter_map(held).eq(b.players.iter().filter_map(held))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StablePose {
    Exact(u64),
    Extrapolated(u64),
}

#[derive(Debug, Default)]
pub(crate) struct DisplayState {
    segment: Option<u64>,
    revision: u64,
    binding_revision: u64,
    stable: Option<StablePose>,
}

impl DisplayState {
    pub(crate) fn segment(&self) -> Option<u64> {
        self.segment
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    /// Interpolated poses can change many times for the same set of live placement bindings.
    pub(super) fn binding_revision(&self) -> u64 {
        self.binding_revision
    }

    pub(super) fn clear(&mut self) {
        self.segment = None;
        self.stable = None;
    }

    /// Reuse exact and fully extrapolated records. During interpolation a
    /// displayed pose can change even when its source tick stays constant.
    pub(super) fn refresh(
        &mut self,
        store: Option<&Captures>,
        now: Instant,
        latest: bool,
        shown: &mut Option<Sample>,
    ) {
        let Some((store, (segment, header))) =
            store.and_then(|store| Some((store, store.latest_header()?)))
        else {
            *shown = None;
            self.clear();
            return;
        };
        let cut = self.segment != Some(segment);
        let age = store.age_ticks(now);
        let stale = store.stale(header, now);
        let stable = if header.paused || latest {
            Some(StablePose::Exact(header.seq))
        } else if stale && (header.tick as f64 + age - 2.0) - header.tick as f64 >= 1.0 {
            Some(StablePose::Extrapolated(header.seq))
        } else {
            None
        };
        if !cut
            && stable.is_some()
            && stable == self.stable
            && let Some(sample) = shown
        {
            sample.stale = stale;
            return;
        }
        let next = store.sample(now, latest).expect("snapshot has a capture");
        // Interpolation changes poses and aim. Membership, placement, attachments,
        // and held handles are discrete records from the source tick.
        if cut || shown.as_ref().is_none_or(|prior| {
            prior.tick != next.tick && !same_bindings(prior, &next)
        }) {
            self.binding_revision = self.binding_revision.wrapping_add(1);
        }
        if cut
            || shown.as_ref().is_none_or(|prior| {
                !same_records(&next, &prior.objects, &prior.players)
                    || prior.cameras != next.cameras
            })
        {
            self.revision = self.revision.wrapping_add(1);
        }
        self.segment = Some(segment);
        self.stable = stable;
        *shown = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::live::{Frame, Header, Kind, Layers, World};
    use std::time::Duration;

    #[test]
    fn paused_display_reuses_exact_records_and_distinguishes_nan_payloads() {
        let mut store = Captures::new(30.0).unwrap();
        let now = Instant::now();
        let mut world = World {
            objects: vec![Object {
                id: 2,
                body: f32::from_bits(0x7fc0_1234),
                ..Default::default()
            }],
            players: vec![Player {
                index: 1,
                aim: [f32::from_bits(0x7fc0_4321); 3],
                ..Default::default()
            }],
            cameras: Vec::new(),
        };
        let frame = |seq, world: &World| {
            Frame::between(
                Header {
                    seq,
                    tick: seq + 7,
                    epoch: 2,
                    paused: true,
                    cut: false,
                    kind: Kind::Key,
                },
                Layers {
                    objects: true,
                    players: true,
                    camera: false,
                },
                None,
                world,
            )
        };
        let mut display = DisplayState::default();
        let mut shown = None;
        store.apply(&frame(1, &world), now).unwrap();
        display.refresh(Some(&store), now, false, &mut shown);
        let revision = display.revision();
        let ptr = shown.as_ref().unwrap().objects.as_ptr();
        display.refresh(
            Some(&store),
            now + Duration::from_secs(3),
            false,
            &mut shown,
        );
        assert_eq!(shown.as_ref().unwrap().objects.as_ptr(), ptr);
        // A new capture can have the same NaN payloads at a different tick.
        store.apply(&frame(2, &world), now).unwrap();
        display.refresh(Some(&store), now, false, &mut shown);
        assert_eq!(display.revision(), revision);
        world.players[0].aim[2] = f32::from_bits(0x7fc0_4322);
        store.apply(&frame(3, &world), now).unwrap();
        display.refresh(Some(&store), now, false, &mut shown);
        assert_ne!(display.revision(), revision);
        assert_eq!(shown.unwrap().players[0].aim[2].to_bits(), 0x7fc0_4322);
    }

    #[test]
    fn latest_and_clamped_samples_reuse_vectors_and_update_freshness() {
        let start = Instant::now();
        let mut store = Captures::new(10.0).unwrap();
        let world = World {
            objects: vec![Object {
                id: 4,
                position: [2.0, 0.0, 0.0],
                velocity: [10.0, 0.0, 0.0],
                ..Default::default()
            }],
            players: vec![],
            cameras: vec![],
        };
        let frame = Frame::between(
            Header {
                seq: 1,
                tick: 5,
                epoch: 1,
                paused: false,
                cut: false,
                kind: Kind::Key,
            },
            Layers {
                objects: true,
                players: true,
                camera: false,
            },
            None,
            &world,
        );
        store.apply(&frame, start).unwrap();
        let mut display = DisplayState::default();
        let mut shown = None;
        display.refresh(Some(&store), start, true, &mut shown);
        let exact = shown.as_ref().unwrap().objects.as_ptr();
        let revision = display.revision();
        assert!(!shown.as_ref().unwrap().stale);
        let late = start + Duration::from_secs(2);
        display.refresh(Some(&store), late, true, &mut shown);
        assert!(shown.as_ref().unwrap().stale);
        assert_eq!(shown.as_ref().unwrap().objects.as_ptr(), exact);
        assert_eq!(display.revision(), revision);
        display.refresh(Some(&store), late, false, &mut shown);
        let clamped = shown.as_ref().unwrap().objects.as_ptr();
        assert_eq!(shown.as_ref().unwrap().objects[0].position[0], 3.0);
        assert_ne!(display.revision(), revision);
        display.refresh(
            Some(&store),
            late + Duration::from_secs(5),
            false,
            &mut shown,
        );
        assert_eq!(shown.unwrap().objects.as_ptr(), clamped);
    }
    #[test]
    fn placement_binding_revision_ignores_poses_and_changes_for_a_held_or_replaced_handle() {
        let mut store = Captures::new(30.0).unwrap();
        let mut display = DisplayState::default();
        let mut shown = None;
        let now = Instant::now();
        let mut world = World {
            objects: vec![Object { id: 0x10001, kind: 2, placement: Some(1), ..Default::default() }],
            players: vec![Player { index: 0, ..Default::default() }],
            cameras: vec![],
        };
        let mut observe = |seq, world: &World| {
            let f = Frame::between(
                Header { seq, tick: seq, epoch: 1, paused: false, cut: false, kind: Kind::Key },
                Layers { objects: true, players: true, camera: false },
                None,
                world,
            );
            store.apply(&f, now).unwrap();
            display.refresh(Some(&store), now, true, &mut shown);
            display.binding_revision()
        };
        let initial = observe(1, &world);
        world.objects[0].position[0] = 5.0;
        assert_eq!(observe(2, &world), initial);
        world.players[0].held = Some(world.objects[0].id);
        let held = observe(3, &world);
        assert_ne!(held, initial);
        world.objects[0].id = 0x20001;
        assert_ne!(observe(4, &world), held);
    }
}
