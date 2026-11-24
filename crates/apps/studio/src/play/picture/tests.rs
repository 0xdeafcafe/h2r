//! The picture over an invented ring: the test writes the frames the game
//! would, into a file of its own, and a second mapping reads them.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use agent_proto::picture::{Opened, Ring, SlotMeta, Transport};

use super::*;

struct Fake {
    path: PathBuf,
    game: Ring,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        let path = std::env::temp_dir().join(format!("h2s-picture-{tag}-{}.ring", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Fake {
            game: Ring::create(&path, (8, 8)).unwrap(),
            path,
        }
    }

    fn opened(&self) -> Opened {
        Opened {
            id: 1,
            transport: Transport::Ring,
            path: Some(self.path.clone()),
            size: [8, 8],
            format: "rgba8".into(),
            fps: 60,
        }
    }

    fn picture(&self, now: Instant) -> Picture {
        Picture::with_ring(Ring::open(&self.path).unwrap(), self.opened(), now)
    }

    /// Frame `n`, 4x2 pixels of the value `n`, taken at tick `10 * n`.
    fn publish(&self, n: u64) {
        self.game.publish(
            SlotMeta {
                frame: n,
                tick: Some(10 * n),
                present: 100 + n,
                time_us: n * 16_000,
                width: 4,
                height: 2,
            },
            &[n as u8; 4 * 2 * 4],
        );
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn a_frame_is_taken_once_with_its_stamp() {
    let fake = Fake::new("once");
    let t0 = Instant::now();
    let mut picture = fake.picture(t0);
    assert!(!picture.poll(t0), "nothing is published yet");
    assert_eq!(picture.state(t0), State::Open);
    fake.publish(1);
    assert!(picture.poll(t0));
    let frame = picture.frame().unwrap();
    assert_eq!((frame.width, frame.height), (4, 2));
    assert_eq!(frame.rgba, vec![1u8; 32]);
    assert_eq!((frame.number, frame.tick, frame.present), (1, Some(10), 101));
    assert_eq!(frame.rev, 1);
    assert!(!picture.poll(t0), "the same frame is not taken again");
    fake.publish(2);
    assert!(picture.poll(t0));
    assert_eq!(picture.frame().unwrap().rgba, vec![2u8; 32]);
    assert_eq!(picture.frame().unwrap().rev, 2);
    assert_eq!(picture.stats(t0).frames, 2);
    assert_eq!(picture.stats(t0).dropped, 0);
}

#[test]
fn frames_published_between_polls_count_as_dropped() {
    let fake = Fake::new("dropped");
    let t0 = Instant::now();
    let mut picture = fake.picture(t0);
    fake.publish(1);
    assert!(picture.poll(t0));
    fake.publish(2);
    fake.publish(3);
    fake.publish(4);
    assert!(picture.poll(t0));
    // The newest is shown; the two between were never taken.
    assert_eq!(picture.frame().unwrap().number, 4);
    let stats = picture.stats(t0);
    assert_eq!((stats.frames, stats.dropped), (2, 2));
    assert_eq!(stats.tick, Some(40));
}

#[test]
fn a_stalled_ring_shows_its_age() {
    let fake = Fake::new("stalled");
    let t0 = Instant::now();
    let mut picture = fake.picture(t0);
    // No frame at all since the ring opened.
    assert_eq!(picture.state(t0 + Duration::from_secs(1)), State::Open);
    assert_eq!(picture.state(t0 + Duration::from_secs(3)), State::Stalled);
    fake.publish(1);
    let t1 = t0 + Duration::from_secs(4);
    assert!(picture.poll(t1));
    assert_eq!(picture.state(t1), State::Open);
    let later = t1 + Duration::from_millis(2500);
    let stats = picture.stats(later);
    assert_eq!(stats.state, State::Stalled);
    assert_eq!(stats.age, Some(Duration::from_millis(2500)));
    // The last frame stays to be shown, with its age.
    assert_eq!(picture.frame().unwrap().number, 1);
    // A new frame ends the stall.
    fake.publish(2);
    assert!(picture.poll(later));
    assert_eq!(picture.state(later), State::Open);
}

#[test]
fn the_rate_counts_the_last_second() {
    let fake = Fake::new("rate");
    let t0 = Instant::now();
    let mut picture = fake.picture(t0);
    for n in 1..=10 {
        fake.publish(n);
        assert!(picture.poll(t0 + Duration::from_millis(100 * n)));
    }
    let at = t0 + Duration::from_millis(1000);
    assert_eq!(picture.stats(at).fps, 10.0);
    // Half a second on, the frames of the first half second have aged out.
    let at = t0 + Duration::from_millis(1500);
    assert_eq!(picture.stats(at).fps, 6.0);
}

#[test]
fn a_closed_ring_is_closed_and_asks_for_a_new_picture() {
    let fake = Fake::new("closed");
    let t0 = Instant::now();
    let mut picture = fake.picture(t0);
    fake.publish(1);
    picture.poll(t0);
    assert!(!picture.dead());
    fake.game.mark_closed();
    assert_eq!(picture.state(t0), State::Closed);
    assert!(picture.dead());
    // What it showed last is still there to look at.
    assert_eq!(picture.frame().unwrap().number, 1);
}

#[test]
fn a_picture_that_could_not_open_says_why() {
    let picture = Picture::failed("no frames here");
    assert_eq!(picture.state(Instant::now()), State::Failed("no frames here".into()));
    assert!(picture.dead());
    assert_eq!(picture.stats(Instant::now()).state.name(), "failed");
    let tcp = Picture::open(Endpoint::Tcp("127.0.0.1:9".into()));
    assert!(matches!(tcp.state(Instant::now()), State::Failed(why) if why.contains("JPEG")));
}

#[test]
fn an_answer_that_is_not_a_ring_fails_the_picture() {
    let mut picture = Picture::empty(Shared {
        opened: Some(Opened {
            id: 1,
            transport: Transport::Ring,
            path: Some(std::env::temp_dir().join("h2s-picture-missing.ring")),
            size: [8, 8],
            format: "rgba8".into(),
            fps: 60,
        }),
        ..Shared::default()
    });
    let now = Instant::now();
    assert!(!picture.poll(now));
    assert!(matches!(picture.state(now), State::Failed(why) if why.contains("mapping")));
}
