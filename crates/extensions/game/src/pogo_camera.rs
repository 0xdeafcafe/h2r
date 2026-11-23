//! The Pogo Stick's chase camera (docs/game/pogo-stick.md, "The camera"):
//! while the first local player holds a stick, the world is drawn from
//! behind and above that player, along the player's aim, pulled in by
//! walls.
//!
//! It is the host camera Forge and Theater use
//! ([`crate::spectator_view::draw_world_view`]): the world renderer is
//! handed a private copy of the player's view with the camera moved. The
//! game's director stays in first person, so the aim, the bots and the
//! netcode read what they always do, and nothing in guest memory but the
//! private view changes.
//!
//! - **Each tick** ([`on_tick`], from `superbounce_mode`): whether the
//!   camera is on (a local player holds a stick), and how much of the
//!   boom the level leaves room for ([`pull_in`], line-of-sight tests
//!   against the collision the superbounce scan read), from the body's
//!   position and aim.
//! - **Each frame** (the hook on `RENDER_VIEWS`): the player's own
//!   first-person camera is read from the view the game built
//!   ([`crate::spectator_view::retail_view`]) and moved back along the
//!   boom ([`chase`]), the boom's length eased toward the tick's
//!   ([`Boom`]): in at once, out slowly.
//!
//! The camera stands down for another host camera (Forge's monitor, a
//! Theater camera, the agent's spawn view), which have made the view
//! playerless before this hook runs, and for split screen.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use engine::{GuestMemory, points, render_loop::views::VIEW_STRIDE};
use hooks::Flow;
use superbounce::Vec3;

use crate::spectator_view::{Pose, draw_world_view, retail_view};

/// The boom's length behind the eye, along the aim (world units; 1 is
/// 3.048 m).
pub const BOOM: f32 = 0.95;
/// How far the camera is lifted above the boom's end.
pub const RISE: f32 = 0.22;
/// The eye above a standing biped's origin, for the tick's test.
pub const EYE: f32 = 0.62;
/// The least of the boom the camera keeps, against a wall at the
/// player's back.
pub const NEAREST: f32 = 0.12;
/// The camera stops this share of the way short of what blocks it.
pub const MARGIN: f32 = 0.9;
/// Halvings of the boom in the search for the wall.
pub const SEARCH: u32 = 6;
/// The boom's share a second it grows back by once the wall is gone.
pub const EASE_OUT: f32 = 1.6;

/// The camera's offset from the eye for an aim: back along it by
/// [`BOOM`], and up by [`RISE`].
pub fn offset(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [-cy * cp * BOOM, -sy * cp * BOOM, -sp * BOOM + RISE]
}

/// The share of `offset` from `pivot` the level leaves room for, 0 to 1:
/// all of it when `clear(pivot, pivot + offset)`, else the farthest
/// clear point a halving search finds, held back by [`MARGIN`] and never
/// under [`NEAREST`].
pub fn pull_in(pivot: Vec3, offset: Vec3, clear: &dyn Fn(Vec3, Vec3) -> bool) -> f32 {
    let at = |t: f32| {
        [
            pivot[0] + offset[0] * t,
            pivot[1] + offset[1] * t,
            pivot[2] + offset[2] * t,
        ]
    };
    if !pivot.iter().chain(&offset).all(|v| v.is_finite()) {
        return NEAREST;
    }
    if clear(pivot, at(1.0)) {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..SEARCH {
        let mid = (lo + hi) * 0.5;
        if clear(pivot, at(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo * MARGIN).max(NEAREST)
}

/// The eye's pose moved back by `share` of its boom. The camera looks
/// the way the eye does.
pub(crate) fn chase(eye: Pose, share: f32) -> Pose {
    let o = offset(eye.yaw, eye.pitch);
    let share = if share.is_finite() {
        share.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Pose {
        position: [
            eye.position[0] + o[0] * share,
            eye.position[1] + o[1] * share,
            eye.position[2] + o[2] * share,
        ],
        ..eye
    }
}

/// The boom's share, eased between frames: toward a wall at once, away
/// from it at [`EASE_OUT`] a second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Boom {
    share: f32,
}

impl Boom {
    /// Starts at the eye, so the camera pulls back out of first person.
    pub const fn new() -> Boom {
        Boom { share: 0.0 }
    }

    pub fn share(&self) -> f32 {
        self.share
    }

    /// One frame, `dt` seconds after the last, toward `goal`.
    pub fn step(&mut self, goal: f32, dt: f32) -> f32 {
        let goal = if goal.is_finite() {
            goal.clamp(0.0, 1.0)
        } else {
            self.share
        };
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.25)
        } else {
            0.0
        };
        self.share = if goal < self.share {
            goal
        } else {
            (self.share + EASE_OUT * dt).min(goal)
        };
        self.share
    }
}

impl Default for Boom {
    fn default() -> Self {
        Boom::new()
    }
}

// ---- the game ----

/// A local player holds a stick, as of the last tick.
static ON: AtomicBool = AtomicBool::new(false);
/// The boom's share the level leaves room for, as of the last tick (f32
/// bits).
static ROOM: AtomicU32 = AtomicU32::new(0x3f80_0000);
/// The last frame drew the chase camera.
static DRAWN: AtomicBool = AtomicBool::new(false);
/// The eased boom and when it was last stepped.
static BOOM_NOW: Mutex<(Boom, Option<Instant>)> = Mutex::new((Boom::new(), None));

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What `pogo.state` shows of the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// A local player holds a stick: the camera is asked for.
    pub on: bool,
    /// The last frame was drawn from it (false headless, in split screen
    /// and under another host camera).
    pub drawn: bool,
    /// The boom's share the level leaves room for.
    pub room: f32,
    /// The boom's share now, eased.
    pub share: f32,
}

pub fn view() -> View {
    View {
        on: ON.load(Ordering::Relaxed),
        drawn: DRAWN.load(Ordering::Relaxed),
        room: f32::from_bits(ROOM.load(Ordering::Relaxed)),
        share: lock(&BOOM_NOW).0.share(),
    }
}

/// Stands the camera down: a new game, the game's end, the mode off.
pub fn reset() {
    ON.store(false, Ordering::Relaxed);
    DRAWN.store(false, Ordering::Relaxed);
    ROOM.store(1.0f32.to_bits(), Ordering::Relaxed);
    *lock(&BOOM_NOW) = (Boom::new(), None);
}

/// One tick, from the superbounce mode: `held` is whether the first
/// local player holds a stick, `position` and `aim` that player's body
/// origin and aim, `clear` the level's line of sight.
pub(crate) fn on_tick(
    held: bool,
    position: Option<Vec3>,
    aim: Option<Vec3>,
    clear: &dyn Fn(Vec3, Vec3) -> bool,
) {
    if !held {
        if ON.swap(false, Ordering::Relaxed) {
            DRAWN.store(false, Ordering::Relaxed);
            *lock(&BOOM_NOW) = (Boom::new(), None);
        }
        return;
    }
    let room = match (position, aim) {
        (Some(p), Some(a)) => {
            let pose = Pose::looking([p[0], p[1], p[2] + EYE], a);
            pose.map_or(1.0, |e| pull_in(e.position, offset(e.yaw, e.pitch), clear))
        }
        _ => 1.0,
    };
    ROOM.store(room.to_bits(), Ordering::Relaxed);
    ON.store(true, Ordering::Relaxed);
}

/// The player's own camera among the views the game built: `None` (the
/// game's own view is drawn) unless there is exactly one view, and it is
/// a player's.
fn eye(guest: &dyn GuestMemory, views: &[u32]) -> Option<Pose> {
    let [view] = views else {
        return None;
    };
    // Another host camera has already made the view playerless.
    if guest.u32(view.checked_add(8)?).ok()? == u32::MAX {
        return None;
    }
    retail_view(guest, *view).map(|(eye, _)| eye)
}

/// The frame's camera: the player's [`eye`] moved back by the boom,
/// which this steps.
fn resolve(guest: &dyn GuestMemory, views: &[u32]) -> Option<Pose> {
    let eye = eye(guest, views)?;
    let share = {
        let mut boom = lock(&BOOM_NOW);
        let now = Instant::now();
        let dt = boom.1.map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        boom.1 = Some(now);
        boom.0
            .step(f32::from_bits(ROOM.load(Ordering::Relaxed)), dt)
    };
    Some(chase(eye, share))
}

/// The hook that draws the world from the chase camera.
pub fn install_hooks(b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
    let scratch = b.guest_alloc(VIEW_STRIDE, runtime::guest::Permissions::RW)?;
    b.raw()
        .before(
            &points::RENDER_VIEWS,
            "pogo: the chase camera",
            move |call| {
                if ON.load(Ordering::Relaxed) {
                    let drawn = draw_world_view(call, scratch, |guest, views| {
                        resolve(guest, views).map(|pose| (pose, None))
                    });
                    DRAWN.store(drawn, Ordering::Relaxed);
                }
                Ok(Flow::Continue)
            },
        )
        // After Forge's (first), Theater's and the agent spawn view's, which
        // this one yields to.
        .after_feature("theater-camera")
        .after_feature("agent-frames")
        .context_only();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectator_view::blank_view;
    use engine::testing::FakeGuest;

    fn length(v: Vec3) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    #[test]
    fn the_camera_sits_behind_and_above_along_the_aim() {
        // Looking along +X, level: behind is -X, above is +Z.
        let o = offset(0.0, 0.0);
        assert!((o[0] + BOOM).abs() < 1e-6 && o[1].abs() < 1e-6);
        assert!((o[2] - RISE).abs() < 1e-6);
        // Looking along +Y.
        let o = offset(std::f32::consts::FRAC_PI_2, 0.0);
        assert!(o[0].abs() < 1e-6 && (o[1] + BOOM).abs() < 1e-6);
        // Looking down, the camera rises over the player; looking up it
        // drops behind.
        assert!(offset(0.0, -1.0)[2] > offset(0.0, 0.0)[2]);
        assert!(offset(0.0, 1.0)[2] < 0.0);
        // The boom's own length, whatever the aim.
        for (yaw, pitch) in [(0.3, 0.2), (2.0, -1.2), (-1.0, 1.5)] {
            let o = offset(yaw, pitch);
            assert!((length([o[0], o[1], o[2] - RISE]) - BOOM).abs() < 1e-5);
        }
    }

    #[test]
    fn the_chase_pose_keeps_the_aim_and_moves_by_the_share() {
        let eye = Pose {
            position: [4.0, -2.0, 1.0],
            yaw: 0.7,
            pitch: -0.3,
        };
        let full = chase(eye, 1.0);
        assert_eq!((full.yaw, full.pitch), (eye.yaw, eye.pitch));
        let o = offset(eye.yaw, eye.pitch);
        for i in 0..3 {
            assert!((full.position[i] - eye.position[i] - o[i]).abs() < 1e-6);
        }
        assert_eq!(chase(eye, 0.0).position, eye.position);
        let half = chase(eye, 0.5);
        for i in 0..3 {
            assert!((half.position[i] - eye.position[i] - o[i] * 0.5).abs() < 1e-6);
        }
        // Out of range and not a number: clamped, never past the boom.
        assert_eq!(chase(eye, 7.0).position, full.position);
        assert_eq!(chase(eye, f32::NAN).position, eye.position);
    }

    #[test]
    fn open_ground_gives_the_whole_boom() {
        assert_eq!(pull_in([0.0; 3], offset(0.0, 0.0), &|_, _| true), 1.0);
    }

    #[test]
    fn a_wall_behind_pulls_the_camera_in_short_of_it() {
        // A wall across x = -0.5, the player at the origin looking +X:
        // the boom runs back along -X.
        let wall = -0.5f32;
        let clear = |_: Vec3, to: Vec3| to[0] > wall;
        let o = offset(0.0, 0.0);
        let share = pull_in([0.0; 3], o, &clear);
        let x = o[0] * share;
        assert!(x > wall, "the camera is on the player's side: {x}");
        // Within a step of the search of the wall, held back by the
        // margin.
        let hit = wall / o[0];
        assert!(share <= hit * MARGIN + 1e-6, "{share} {hit}");
        assert!(share >= (hit - 1.0 / (1 << SEARCH) as f32) * MARGIN - 1e-6);
    }

    #[test]
    fn a_wall_at_the_players_back_keeps_the_nearest_boom() {
        assert_eq!(pull_in([0.0; 3], offset(0.0, 0.0), &|_, _| false), NEAREST);
        // Not a number in: the nearest, and no test made.
        let asked = std::cell::Cell::new(false);
        let clear = |_: Vec3, _: Vec3| {
            asked.set(true);
            true
        };
        assert_eq!(pull_in([f32::NAN, 0.0, 0.0], [1.0; 3], &clear), NEAREST);
        assert!(!asked.get());
    }

    #[test]
    fn the_boom_comes_in_at_once_and_eases_back_out() {
        let mut b = Boom::new();
        assert_eq!(b.share(), 0.0);
        // Out of first person, at EASE_OUT a second.
        let a = b.step(1.0, 0.1);
        assert!((a - EASE_OUT * 0.1).abs() < 1e-6);
        for _ in 0..100 {
            b.step(1.0, 1.0 / 60.0);
        }
        assert_eq!(b.share(), 1.0);
        // A wall: in on the frame.
        assert_eq!(b.step(0.3, 1.0 / 60.0), 0.3);
        // Gone: out slowly, never past the goal.
        let next = b.step(1.0, 1.0 / 60.0);
        assert!(next > 0.3 && next < 0.4);
        // A long frame is a quarter of a second at most.
        let mut b = Boom::new();
        assert!((b.step(1.0, 60.0) - EASE_OUT * 0.25).abs() < 1e-6);
        // Not numbers: where it was.
        let was = b.share();
        assert_eq!(b.step(f32::NAN, 0.1), was);
        assert_eq!(b.step(1.0, f32::NAN), was);
    }

    /// A player's view at `at`, looking along +X from (1, 2, 3).
    fn player_view(guest: &mut FakeGuest, at: u32, player: u32) {
        let mut record = blank_view();
        record[8..12].copy_from_slice(&player.to_le_bytes());
        let camera = 0x0c;
        for (i, v) in [1.0f32, 2.0, 3.0, 1.0, 0.0, 0.0].into_iter().enumerate() {
            record[camera + 4 * i..camera + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        guest.put(at, &record);
    }

    #[test]
    fn the_eye_is_the_one_players_view() {
        let mut guest = FakeGuest::new();
        player_view(&mut guest, 0x2000, 0);
        player_view(&mut guest, 0x3000, 1);
        player_view(&mut guest, 0x4000, u32::MAX);
        let e = eye(&guest, &[0x2000]).expect("one player's view");
        assert_eq!(e.position, [1.0, 2.0, 3.0]);
        assert_eq!((e.yaw, e.pitch), (0.0, 0.0));
        // Split screen, and no view at all.
        assert!(eye(&guest, &[0x2000, 0x3000]).is_none());
        assert!(eye(&guest, &[]).is_none());
        // A view another host camera made playerless.
        assert!(eye(&guest, &[0x4000]).is_none());
        // Addresses the guest does not hold.
        assert!(eye(&guest, &[0x9000]).is_none());
        assert!(eye(&guest, &[u32::MAX - 4]).is_none());
    }

    #[test]
    fn the_tick_turns_the_camera_on_and_the_frame_pulls_it_back() {
        // The one test of the statics, so no other can interleave.
        reset();
        assert!(!view().on);
        on_tick(true, Some([0.0; 3]), Some([1.0, 0.0, 0.0]), &|_, to| {
            to[0] > -0.5
        });
        let v = view();
        assert!(v.on);
        assert!(v.room < 1.0 && v.room >= NEAREST);
        // No body to test from: the whole boom.
        on_tick(true, None, None, &|_, _| false);
        assert_eq!(view().room, 1.0);

        // The first frame starts at the eye; the boom then eases out.
        let mut guest = FakeGuest::new();
        player_view(&mut guest, 0x2000, 0);
        let first = resolve(&guest, &[0x2000]).expect("one player's view");
        assert_eq!(first.position, [1.0, 2.0, 3.0]);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let next = resolve(&guest, &[0x2000]).unwrap();
        assert!(next.position[0] < 1.0, "behind: {:?}", next.position);
        assert!(next.position[2] > 3.0, "and above");
        assert_eq!(next.position[1], 2.0);
        assert!(view().share > 0.0);

        // The stick put away: back to the eye for the next time.
        on_tick(false, None, None, &|_, _| true);
        assert!(!view().on);
        assert_eq!(view().share, 0.0);
        reset();
    }
}
