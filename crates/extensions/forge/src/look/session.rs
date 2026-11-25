//! The monitor looks and poses of a shared session's machines (docs/game/
//! forge-monitor-appearance.md, "Other players", ML7): what a machine can
//! draw as its hello says, the host's table of seat to look — a peer
//! proposes, the host validates against every machine and sends the
//! accepted look to everyone — and a peer's seats, whose poses interpolate
//! on the render clock and fade when stale.
//!
//! The messages ride the session's own channel beside the edit log
//! (`forge::net`'s lanes): they are not log entries, and a look edits no
//! map. Nothing here reads a clock, a guest or a socket: the caller says
//! when a pose was sampled and when it is drawn.
use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

pub use super::body::Pose;
use super::refusal::Refusal;
use super::{MARKER, MonitorLook, VERSION};

/// What a machine can draw, as its session hello says: the look version
/// it speaks, whether the native renderer draws bodies, and the SHA-256
/// of each census source map it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanDraw {
    pub look_version: u32,
    pub renderer: bool,
    /// Lower-case hex SHA-256 of each census source map.
    pub maps: Vec<String>,
}

impl CanDraw {
    /// This build's declaration, for a machine that draws what it holds.
    pub fn ours(renderer: bool, maps: Vec<String>) -> CanDraw {
        CanDraw {
            look_version: VERSION,
            renderer,
            maps,
        }
    }

    /// Whether this machine can draw `look` whose source map hashes
    /// `base`: the marker always; a disc model only with the renderer on
    /// and the source map held. `Err` names the machine.
    pub fn draws(&self, machine: &str, look: &MonitorLook, base: Option<&str>) -> Result<(), Refusal> {
        if look.is_marker() {
            return Ok(());
        }
        if !self.renderer {
            return Err(Refusal::Renderer {
                machine: machine.into(),
            });
        }
        let map = base.unwrap_or_default();
        if !self.maps.iter().any(|held| held == map) {
            return Err(Refusal::SourceMissing {
                machine: machine.into(),
                map: map.into(),
            });
        }
        Ok(())
    }
}

/// One seat of the host's table.
#[derive(Clone, Debug, PartialEq)]
struct Seat {
    name: String,
    can: Option<CanDraw>,
    look: MonitorLook,
    /// The source map the accepted look's model was read from, as the
    /// proposer named it: what a joiner must hold to draw the look.
    source: Option<String>,
}

/// The host's table of seat to look: the one authority. A peer proposes;
/// [`Table::propose`] validates the look against every machine's
/// [`CanDraw`] and, accepted, is the seat's look, to be sent to everyone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    seats: BTreeMap<u8, Seat>,
}

/// A seat the join reset moved back to the marker, with the notice the
/// affected player reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Reset {
    pub seat: u8,
    pub look: MonitorLook,
    pub notice: String,
}

impl Table {
    /// The host's own seat: seat 0 under the host's name, drawing `look`.
    pub fn host(name: &str, look: MonitorLook) -> Table {
        Table {
            seats: BTreeMap::from([(
                0,
                Seat {
                    name: name.into(),
                    can: None,
                    look,
                    source: None,
                },
            )]),
        }
    }

    /// A machine joined as `seat`: remember what it can draw. A look
    /// version that is not this build's refuses the join outright.
    pub fn joined(&mut self, seat: u8, name: &str, can: CanDraw) -> Result<(), Refusal> {
        if can.look_version != VERSION {
            return Err(Refusal::Version {
                theirs: can.look_version,
                ours: VERSION,
            });
        }
        self.seats.insert(
            seat,
            Seat {
                name: name.into(),
                can: Some(can),
                look: MonitorLook::default_for(0),
                source: None,
            },
        );
        Ok(())
    }

    /// The seat left.
    pub fn left(&mut self, seat: u8) {
        self.seats.remove(&seat);
    }

    /// The table as it stands: each seat and its accepted look.
    pub fn looks(&self) -> impl Iterator<Item = (u8, &MonitorLook)> {
        self.seats.iter().map(|(seat, s)| (*seat, &s.look))
    }

    /// The look `seat` would draw, before any proposal.
    pub fn look(&self, seat: u8) -> Option<&MonitorLook> {
        self.seats.get(&seat).map(|s| &s.look)
    }

    /// A peer's proposal, validated: the bounds
    /// ([`MonitorLook::validate`]), then every machine's [`CanDraw`].
    /// Accepted, it is the seat's look — the caller sends it to everyone.
    /// The host's own seat may propose too.
    pub fn propose(
        &mut self,
        seat: u8,
        look: MonitorLook,
        base: Option<&str>,
    ) -> Result<MonitorLook, Refusal> {
        look.validate()?;
        if !look.is_marker() && base.is_none() {
            return Err(Refusal::invalid(
                "model",
                "is not in this machine's catalogue of models",
            ));
        }
        for (other, s) in &self.seats {
            if *other == seat {
                continue;
            }
            if let Some(can) = &s.can {
                can.draws(&s.name, &look, base)?;
            }
        }
        let source = base.map(str::to_owned);
        self.seats.entry(seat).and_modify(|s| {
            s.look = look.clone();
            s.source = source;
        });
        Ok(look)
    }

    /// A joiner who cannot draw a look in use does not join into a
    /// session that silently differs: each affected seat is reset to the
    /// marker with its colours kept, and the affected player is told.
    /// The caller sends each reset look to everyone.
    pub fn join_reset(&mut self, joiner: u8, name: &str, can: &CanDraw) -> Vec<Reset> {
        let mut resets = Vec::new();
        for (seat, s) in &mut self.seats {
            if *seat == joiner {
                continue;
            }
            if s.look.is_marker() {
                // The marker needs neither the renderer nor a source map.
                continue;
            }
            if let Err(refusal) = can.draws(name, &s.look, s.source.as_deref()) {
                let map = match &refusal {
                    Refusal::SourceMissing { map, .. } => map.clone(),
                    _ => s.look.model.clone(),
                };
                let mut look = s.look.clone();
                look.model = MARKER.into();
                look.variant = None;
                look.tint = 0;
                look.light = 0;
                s.look = look.clone();
                s.source = None;
                resets.push(Reset {
                    seat: *seat,
                    look,
                    notice: format!(
                        "Monitor look reset: {name} does not have {map}."
                    ),
                });
            }
        }
        resets
    }
}

/// How long a pose is held without a sample before the body fades, and
/// how long the fade takes: seconds of the render clock.
pub const STALE_AFTER: f32 = 2.0;
pub const FADE_FOR: f32 = 1.0;

/// The poses one sample window holds.
const SAMPLES: usize = 8;

/// The pose `t` of the way from the sample at `a` to the one at `b`.
fn between(a: &(f32, Pose), b: &(f32, Pose), now: f32) -> Pose {
    let span = (b.0 - a.0).max(1e-6);
    let t = ((now - a.0) / span).clamp(0., 1.);
    let mix = |(a, b): (f32, f32)| a + (b - a) * t;
    Pose {
        position: [
            mix((a.1.position[0], b.1.position[0])),
            mix((a.1.position[1], b.1.position[1])),
            mix((a.1.position[2], b.1.position[2])),
        ],
        yaw: mix((a.1.yaw, b.1.yaw)),
        pitch: mix((a.1.pitch, b.1.pitch)),
    }
}

/// One seat's pose track.
#[derive(Clone, Debug, PartialEq)]
struct Track {
    monitor: bool,
    /// In order of their own `at`.
    samples: VecDeque<(f32, Pose)>,
    /// The newest sample's `at`.
    newest: f32,
}

impl Track {
    fn new() -> Track {
        Track {
            monitor: false,
            samples: VecDeque::new(),
            newest: f32::NEG_INFINITY,
        }
    }

    fn push(&mut self, at: f32, pose: Pose) {
        if at >= self.newest {
            self.newest = at;
            self.samples.push_back((at, pose));
            while self.samples.len() > SAMPLES {
                self.samples.pop_front();
            }
            return;
        }
        // A sample out of order refines the past it falls in; the drawn
        // body, ahead of it, does not move.
        let at_insert = self
            .samples
            .iter()
            .position(|(held, _)| *held > at)
            .unwrap_or(self.samples.len());
        self.samples.insert(at_insert, (at, pose));
    }

    /// The pose at `now`, interpolated between the samples around it and
    /// held at the newest while none fresher has come.
    fn at(&self, now: f32) -> Option<Pose> {
        let mut previous = None;
        for (held, pose) in &self.samples {
            if *held > now {
                return Some(match previous {
                    // Before the first sample: hold it.
                    None => *pose,
                    Some(a) => between(&a, &(*held, *pose), now),
                });
            }
            previous = Some((*held, *pose));
        }
        previous.map(|(_, p)| p)
    }
}

/// A peer's view of the session's seats: the looks the host sent and each
/// seat's poses. A machine draws only looks the host sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seats {
    looks: BTreeMap<u8, MonitorLook>,
    poses: BTreeMap<u8, Track>,
}

impl Seats {
    /// The host sent `seat`'s accepted look.
    pub fn look(&mut self, seat: u8, look: MonitorLook) {
        self.looks.insert(seat, look);
    }

    /// A pose sample for `seat`, taken at `at` of its machine's render
    /// clock. `monitor: false` (the player is back in their biped) hides
    /// the body at once.
    pub fn pose(&mut self, seat: u8, at: f32, pose: Pose, monitor: bool) {
        let track = self.poses.entry(seat).or_insert_with(Track::new);
        if !monitor {
            *track = Track::new();
            return;
        }
        track.monitor = true;
        track.push(at, pose);
    }

    /// The seat's look, as the host sent it.
    pub fn look_of(&self, seat: u8) -> Option<&MonitorLook> {
        self.looks.get(&seat)
    }

    /// The seats with a look, as the host sent them.
    pub fn looks(&self) -> impl Iterator<Item = (u8, &MonitorLook)> {
        self.looks.iter().map(|(seat, look)| (*seat, look))
    }

    /// The seat's drawn pose at `now` of this machine's render clock, and
    /// how faded it is (1 drawn, 0 gone): held between the samples around
    /// `now`, gone once no sample is fresh — stale by [`STALE_AFTER`]
    /// seconds, faded over the [`FADE_FOR`] after — and while the seat
    /// is not a monitor at all.
    pub fn body(&self, seat: u8, now: f32) -> Option<(Pose, f32)> {
        let look = self.looks.get(&seat)?;
        if look.is_marker() {
            // A marker seat is drawn by the compositor's ring, not a body.
            return None;
        }
        self.pose_at(seat, now)
    }

    /// The seat's monitor pose at `now` and how faded it is, whatever its
    /// look: a marker seat has a pose too, which a camera may follow.
    /// `None` while the seat is not a monitor or its samples are gone.
    pub fn pose_at(&self, seat: u8, now: f32) -> Option<(Pose, f32)> {
        let track = self.poses.get(&seat)?;
        if !track.monitor {
            return None;
        }
        let pose = track.at(now)?;
        let age = (now - track.newest).max(0.);
        if age > STALE_AFTER + FADE_FOR {
            return None;
        }
        let fade = if age <= STALE_AFTER {
            1.
        } else {
            1. - (age - STALE_AFTER) / FADE_FOR
        };
        Some((pose, fade))
    }

    /// Whether the seat is a monitor a sample has spoken of, however
    /// stale: `forge.status`'s "not seen" is a body gone.
    pub fn is_monitor(&self, seat: u8) -> bool {
        self.poses.get(&seat).is_some_and(|t| t.monitor)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MonitorLook, VERSION};
    use super::*;

    fn model_look() -> MonitorLook {
        MonitorLook {
            version: VERSION,
            model: "sentinel".into(),
            variant: None,
            primary: 3,
            secondary: 3,
            tint: 1,
            light: 1,
            light_colour: 3,
        }
    }

    fn can(maps: &[&str]) -> CanDraw {
        CanDraw::ours(true, maps.iter().map(|m| m.to_string()).collect())
    }

    fn pose(x: f32) -> Pose {
        Pose {
            position: [x, 0., 0.],
            yaw: 0.,
            pitch: 0.,
        }
    }

    #[test]
    fn the_host_refuses_a_look_one_machine_cannot_draw_naming_it() {
        let mut table = Table::host("Host", MonitorLook::default_for(0));
        table.joined(1, "Alpha", can(&["abc"])).unwrap();
        table.joined(2, "Bravo", can(&[])).unwrap();
        // Bravo holds no source map: refused naming Bravo and the map.
        let refusal = table.propose(1, model_look(), Some("abc")).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::SourceMissing {
                machine: "Bravo".into(),
                map: "abc".into(),
            }
        );
        // Without the renderer, refused naming the machine.
        let mut no_renderer = can(&["abc"]);
        no_renderer.renderer = false;
        let mut table = Table::host("Host", MonitorLook::default_for(0));
        table.joined(1, "Alpha", no_renderer).unwrap();
        assert_eq!(
            table.propose(0, model_look(), Some("abc")).unwrap_err(),
            Refusal::Renderer {
                machine: "Alpha".into()
            }
        );
        // The marker needs none of it, and a model without a source is
        // the catalogue's refusal.
        assert!(table
            .propose(1, MonitorLook::default_for(0), None)
            .is_ok());
        assert_eq!(
            table.propose(1, model_look(), None).unwrap_err().code(),
            "monitor_look_invalid"
        );
        // A field out of bounds is the bounds' refusal.
        let mut bound = model_look();
        bound.tint = 9;
        assert_eq!(
            table.propose(1, bound, Some("abc")).unwrap_err().code(),
            "monitor_look_invalid"
        );
    }

    #[test]
    fn an_accepted_look_is_the_seats_and_the_table_lists_it() {
        let mut table = Table::host("Host", MonitorLook::default_for(2));
        table.joined(1, "Alpha", can(&["abc"])).unwrap();
        let accepted = table.propose(1, model_look(), Some("abc")).unwrap();
        assert_eq!(table.look(1), Some(&accepted));
        assert_eq!(table.looks().count(), 2);
        table.left(1);
        assert_eq!(table.looks().count(), 1);
        // A joiner whose look version differs is refused outright.
        let mut old = can(&[]);
        old.look_version = 0;
        assert_eq!(
            table.joined(2, "Old", old).unwrap_err().code(),
            "monitor_look_version"
        );
    }

    #[test]
    fn a_joiner_without_a_source_resets_the_seat_with_the_notice() {
        let mut table = Table::host("Host", MonitorLook::default_for(0));
        let mut look = model_look();
        look.primary = 5;
        table.joined(1, "Alpha", can(&["abc"])).unwrap();
        table.propose(1, look.clone(), Some("abc")).unwrap();
        // Bravo joins with nothing: Alpha's seat resets to the marker
        // with its colours kept, and Alpha is told.
        let resets = table.join_reset(2, "Bravo", &can(&[]));
        assert_eq!(resets.len(), 1, "the host's own marker needs no reset");
        assert_eq!(resets[0].seat, 1);
        let reset = &resets[0].look;
        assert!(reset.is_marker());
        assert_eq!((reset.primary, reset.secondary), (5, 3));
        assert_eq!(reset.tint, 0);
        assert_eq!(resets[0].notice, "Monitor look reset: Bravo does not have abc.");
        assert_eq!(table.look(1), Some(reset));
        // A joiner that holds the map resets nothing.
        assert!(table.join_reset(3, "Charlie", &can(&["abc"])).is_empty());
    }

    #[test]
    fn poses_interpolate_on_the_clock_and_fade_when_stale() {
        let mut seats = Seats::default();
        seats.look(1, model_look());
        seats.pose(1, 0., pose(0.), true);
        seats.pose(1, 1., pose(10.), true);
        let (held, fade) = seats.body(1, 0.5).unwrap();
        assert_eq!(held.position, [5., 0., 0.]);
        assert_eq!(fade, 1., "fresh");
        // Held at the newest sample while none fresher has come.
        assert_eq!(seats.body(1, 1.5).unwrap().0.position[0], 10.);
        // Stale by two seconds after the newest sample (at 1), faded
        // over the second after that.
        let (_, fresh) = seats.body(1, 2.5).unwrap();
        assert_eq!(fresh, 1., "two seconds stale is still whole");
        let (_, fading) = seats.body(1, 3.5).unwrap();
        assert!((fading - 0.5).abs() < 1e-6);
        assert!(seats.body(1, 4.01).is_none(), "gone");
        // monitor: false hides the body at once.
        seats.pose(1, 4.5, pose(0.), false);
        assert!(seats.body(1, 4.5).is_none());
        assert!(!seats.is_monitor(1));
    }

    #[test]
    fn reordered_poses_never_move_a_body_backwards() {
        let mut seats = Seats::default();
        seats.look(1, model_look());
        seats.pose(1, 0., pose(0.), true);
        seats.pose(1, 2., pose(20.), true);
        let forward = seats.body(1, 1.).unwrap().0;
        // The sample for 1 second arrives after the one for 2: the body
        // drawn at 1 is the same, and the one at 2 does not go back.
        let mut reordered = Seats::default();
        reordered.look(1, model_look());
        reordered.pose(1, 0., pose(0.), true);
        reordered.pose(1, 2., pose(20.), true);
        reordered.pose(1, 1., pose(10.), true);
        assert_eq!(reordered.body(1, 1.).unwrap().0, forward);
        assert_eq!(reordered.body(1, 2.).unwrap().0.position[0], 20.);
        // A sample older than everything held changes nothing drawn.
        reordered.pose(1, 0.5, pose(99.), true);
        assert_eq!(reordered.body(1, 2.).unwrap().0.position[0], 20.);
    }

    #[test]
    fn a_marker_seat_draws_no_body_and_the_looks_the_host_sent() {
        let mut seats = Seats::default();
        seats.look(1, MonitorLook::default_for(4));
        seats.pose(1, 0., pose(0.), true);
        assert!(seats.body(1, 0.).is_none(), "the ring draws instead");
        assert_eq!(seats.looks().count(), 1);
        assert_eq!(seats.look_of(1).unwrap().primary, 4);
        // A seat with no look draws nothing at all.
        seats.pose(2, 0., pose(0.), true);
        assert!(seats.body(2, 0.).is_none());
    }

    // ---- the channel over lanes (the game's glue drives exactly this) ----

    /// One end of an in-memory lane pair, as `forge::net`'s tests use.
    /// One end of an in-memory lane pair: whole messages at once, in
    /// order; the beat each end reports is the one the other set.
    #[derive(Clone, Default)]
    struct End {
        /// The queue this end sends into (its peer drains it).
        out: std::rc::Rc<std::cell::RefCell<VecDeque<u8>>>,
        /// The queue this end drains (its peer sends into it).
        inn: std::rc::Rc<std::cell::RefCell<VecDeque<u8>>>,
        /// The beat this end reports (its peer reads it).
        beat_out: std::rc::Rc<std::cell::RefCell<Option<crate::net::Beat>>>,
        /// The beat this end reads (its peer reports it).
        beat_in: std::rc::Rc<std::cell::RefCell<Option<crate::net::Beat>>>,
    }

    fn pipe() -> (Lane, Lane) {
        let ab = End::default();
        let ba = End::default();
        // A sends into ab's queue and reports ab's beat slot; B sends into
        // ba's and reports ba's, so each drains and reads the other's.
        let a = End {
            out: ab.out.clone(),
            inn: ba.out.clone(),
            beat_out: ab.beat_out.clone(),
            beat_in: ba.beat_out.clone(),
        };
        let b = End {
            out: ba.out.clone(),
            inn: ab.out.clone(),
            beat_out: ba.beat_out.clone(),
            beat_in: ab.beat_out.clone(),
        };
        (Box::new(a), Box::new(b))
    }

    impl crate::net::Lane for End {
        fn send(&mut self, bytes: &[u8]) -> bool {
            self.out.borrow_mut().extend(bytes.iter().copied());
            true
        }
        fn recv(&mut self) -> Vec<u8> {
            self.inn.borrow_mut().drain(..).collect()
        }
        fn set_beat(&mut self, beat: Option<crate::net::Beat>) {
            *self.beat_out.borrow_mut() = beat;
        }
        fn beat(&self) -> Option<crate::net::Beat> {
            *self.beat_in.borrow()
        }
    }

    type Lane = Box<dyn crate::net::Lane>;

    fn doc() -> forge_doc::ForgeDoc {
        forge_doc::ForgeDoc {
            format: forge_doc::FORMAT,
            builder: forge_doc::BuilderVersions {
                forge_build: 1,
                pathfind: 1,
                bots: 1,
            },
            base: forge_doc::MapRef {
                map: "lookroom".into(),
                sha256: forge_doc::Sha256::of(b"a room"),
            },
            sources: vec![],
            title: "Room".into(),
            author: "Host".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![],
            expect: None,
        }
    }

    fn limits() -> forge_doc::Limits {
        forge_doc::Limits::default()
    }

    /// Drives the host the game's `monitor_tick` does: hellos join the
    /// table, proposals answer, poses relay.
    fn host_tick(
        host: &mut crate::net::Node<crate::net::DocOnly, Lane>,
        table: &mut Table,
    ) -> Vec<crate::net::Monitor> {
        use crate::net::Monitor;
        let _ = host.tick(0);
        let mut seen = Vec::new();
        for message in host.take_monitors() {
            match &message {
                Monitor::Hello { seat, name, can } => {
                    table.joined(seat.0, name, can.clone()).unwrap();
                    // The joiner learns the table, and a look it cannot
                    // draw resets its seat with the notice.
                    for (other, look) in table.looks().collect::<Vec<_>>() {
                        host.send_look_to(*seat, other, look.clone());
                    }
                    for reset in table.join_reset(seat.0, name, can) {
                        host.send_look(reset.seat, reset.look);
                        host.send_notice(*seat, reset.notice);
                    }
                }
                Monitor::Look { seat, look, base } => {
                    match table.propose(seat.0, look.clone(), base.as_deref()) {
                        Ok(accepted) => host.send_look(seat.0, accepted),
                        Err(refusal) => host.send_look_refused(*seat, refusal.clone()),
                    }
                }
                Monitor::Pose {
                    seat,
                    at,
                    pose,
                    monitor,
                    ..
                } => host.send_pose(*seat, *at, *pose, *monitor, None, Some(*seat)),
                _ => {}
            }
            seen.push(message);
        }
        seen
    }

    fn guest_tick(guest: &mut crate::net::Node<crate::net::DocOnly, Lane>) -> Vec<crate::net::Monitor> {
        let _ = guest.tick(0);
        guest.take_monitors()
    }

    #[test]
    fn a_fake_host_and_two_peers_carry_the_looks_and_poses() {
        use crate::net::{HOST_SEAT, Monitor, Node};
        let mut host = Node::host(doc(), limits(), crate::net::DocOnly, "Host", 0, 1)
            .unwrap();
        let (a_to_host, a_to_alpha) = pipe();
        let (b_to_host, b_to_bravo) = pipe();
        let mut alpha = Node::<crate::net::DocOnly, _>::guest(limits(), "Alpha", can(&["abc"]), a_to_alpha)
            .unwrap();
        let mut bravo = Node::<crate::net::DocOnly, _>::guest(limits(), "Bravo", can(&[]), b_to_bravo).unwrap();
        host.accept(crate::net::Seat(1), a_to_host, crate::net::Role::Editor)
            .unwrap();
        host.accept(crate::net::Seat(2), b_to_host, crate::net::Role::Editor)
            .unwrap();
        let mut table = Table::host("Host", MonitorLook::default_for(0));
        // The hellos join the table.
        for _ in 0..4 {
            host_tick(&mut host, &mut table);
            guest_tick(&mut alpha);
            guest_tick(&mut bravo);
        }
        assert_eq!(table.looks().count(), 3);

        // A look one peer cannot draw is refused, naming the peer.
        alpha.send_look_up(model_look(), Some("abc".into()));
        host_tick(&mut host, &mut table);
        let seen = guest_tick(&mut alpha);
        assert_eq!(
            seen,
            vec![Monitor::LookRefused {
                seat: HOST_SEAT,
                refusal: Refusal::SourceMissing {
                    machine: "Bravo".into(),
                    map: "abc".into(),
                },
            }],
            "the refusal names Bravo and the map"
        );
        assert_eq!(table.look(1).unwrap().model, MARKER, "unchanged");

        // The marker look everyone draws: accepted, to everyone.
        let mut marker = MonitorLook::default_for(3);
        marker.light = 0;
        alpha.send_look_up(marker.clone(), None);
        host_tick(&mut host, &mut table);
        let seen = guest_tick(&mut bravo);
        assert_eq!(
            seen,
            vec![Monitor::Accepted {
                seat: crate::net::Seat(1),
                look: marker,
            }]
        );

        // A pose relays peer to peer.
        alpha.send_pose_up(
            1.,
            Pose {
                position: [7., 0., 0.],
                yaw: 0.,
                pitch: 0.,
            },
            true,
            None,
        );
        host_tick(&mut host, &mut table);
        let mut seats = Seats::default();
        for message in guest_tick(&mut bravo) {
            if let Monitor::Pose {
                seat,
                at,
                pose,
                monitor,
                ..
            } = message
            {
                seats.pose(seat.0, at, pose, monitor);
            }
        }
        seats.look(1, model_look());
        assert_eq!(
            seats.body(1, 1.).map(|(pose, _)| pose.position),
            Some([7., 0., 0.]),
            "Bravo draws Alpha's monitor where it said"
        );
    }

    #[test]
    fn a_joiner_without_a_source_resets_the_seat_over_the_channel() {
        use crate::net::{Monitor, Node, Seat};
        let mut host = Node::host(doc(), limits(), crate::net::DocOnly, "Host", 0, 1)
            .unwrap();
        let (a_to_host, a_to_alpha) = pipe();
        let mut alpha = Node::<crate::net::DocOnly, _>::guest(limits(), "Alpha", can(&["abc"]), a_to_alpha)
            .unwrap();
        host.accept(Seat(1), a_to_host, crate::net::Role::Editor).unwrap();
        let mut table = Table::host("Host", MonitorLook::default_for(0));
        for _ in 0..4 {
            host_tick(&mut host, &mut table);
            guest_tick(&mut alpha);
        }
        // Alpha's model look stands.
        alpha.send_look_up(model_look(), Some("abc".into()));
        host_tick(&mut host, &mut table);
        guest_tick(&mut alpha);
        assert_eq!(table.look(1).unwrap().model, "sentinel");

        // A joiner with nothing joins: the seat resets, everyone is told,
        // and the joiner learns the table.
        let (c_to_host, c_to_charlie) = pipe();
        let mut charlie = Node::<crate::net::DocOnly, _>::guest(limits(), "Charlie", can(&[]), c_to_charlie)
            .unwrap();
        host.accept(Seat(2), c_to_host, crate::net::Role::Editor).unwrap();
        for _ in 0..2 {
            host_tick(&mut host, &mut table);
        }
        let mut seen = guest_tick(&mut charlie);
        assert_eq!(table.look(1).unwrap().model, MARKER);
        assert!(
            seen.iter().any(|m| matches!(m, Monitor::Notice { text, .. }
                if text == "Monitor look reset: Charlie does not have abc.")),
            "the notice travels: {seen:?}"
        );
        assert!(
            seen.iter().any(|m| matches!(m, Monitor::Accepted { seat, .. } if *seat == Seat(1))),
            "the joiner learns the table"
        );
        // Alpha sees the reset too.
        seen = guest_tick(&mut alpha);
        assert!(
            seen.iter().any(|m| matches!(m, Monitor::Accepted { seat, look, .. }
                if *seat == Seat(1) && look.is_marker()))
        );
    }
}
