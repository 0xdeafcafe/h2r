//! The announcer for our medals against a fake speaker and a synthetic
//! guest: what is queued, in what order, and when it may start.

use guest_layout::memory::{FlatMemory, GuestMemory};
use hud_kit_core::views::superbounce::{MEDALS, Medal};

use super::*;
use crate::superbounce_chain::{Award, bounce_medals_with};
use crate::superbounce_hud::Sent;
use crate::superbounce_mode::{ScanState, State};

/// A speaker whose lines sound until the test ends them.
#[derive(Default)]
struct Fake {
    started: Vec<&'static str>,
    sounding: bool,
    /// Clips that do not play (a missing WAV).
    broken: Vec<&'static str>,
}

impl Speaker for Fake {
    fn start(&mut self, id: &'static str) -> bool {
        if self.broken.contains(&id) {
            return false;
        }
        self.started.push(id);
        self.sounding = true;
        true
    }
    fn sounding(&mut self) -> bool {
        self.sounding
    }
}

fn on() -> Announcer {
    Announcer {
        on: true,
        ..Announcer::new()
    }
}

/// Steps `a` until its queue is empty and nothing plays, ending each line
/// the tick after it started; the game's announcer idle throughout.
fn play_out(a: &mut Announcer, s: &mut Fake) {
    for _ in 0..64 {
        a.queue.step(s, true);
        s.sounding = false;
        if !a.queue.holds() && a.queue.waiting().next().is_none() {
            return;
        }
    }
    panic!("the queue did not empty");
}

fn ready() -> State {
    State {
        active: true,
        scan: ScanState::Ready,
        bounce: 3,
        ..State::default()
    }
}

fn award(seq: u64, chain: u32, monster: bool) -> Award {
    Award {
        seq,
        slot: 0,
        user: Some(0),
        tick: 30 * seq as u32,
        chain,
        medals: bounce_medals_with(chain, monster),
        spot: Some(1),
        strips: Some(1),
    }
}

/// The lines the HUD sends for `awards`, one award a tick, each spoken.
fn hud_lines(a: &mut Announcer, awards: Vec<Award>) -> Vec<String> {
    let mut sent = Sent::default();
    let mut s = ready();
    assert!(sent.notes(Some(&s), None, true).is_empty(), "the first state is not news");
    let mut lines = Vec::new();
    for aw in awards {
        s.medals.recent.push(aw);
        for (_, text) in sent.notes(Some(&s), None, true) {
            a.announce(&text);
            lines.push(text);
        }
    }
    lines
}

/// A chain of three bounces shows Superbounce, Double bounce and Triple
/// bounce, and the announcer says them in that order, one at a time.
#[test]
fn a_chain_of_three_speaks_its_three_lines_in_order() {
    let mut a = on();
    let lines = hud_lines(&mut a, vec![award(1, 1, false), award(2, 2, false), award(3, 3, false)]);
    assert_eq!(
        lines,
        vec![
            Medal::Superbounce.line(),
            Medal::DoubleBounce.line(),
            Medal::TripleBounce.line()
        ]
    );
    assert_eq!(
        a.queue.waiting().collect::<Vec<_>>(),
        ["superbounce", "double-bounce", "triple-bounce"]
    );

    let mut s = Fake::default();
    assert_eq!(a.queue.step(&mut s, true), Some("superbounce"));
    assert!(a.queue.holds(), "the game's queue is held while ours plays");
    for _ in 0..3 {
        assert_eq!(a.queue.step(&mut s, true), None, "one line at a time");
    }
    s.sounding = false;
    assert_eq!(a.queue.step(&mut s, true), Some("double-bounce"));
    s.sounding = false;
    assert_eq!(a.queue.step(&mut s, true), Some("triple-bounce"));
    s.sounding = false;
    assert_eq!(a.queue.step(&mut s, true), None);
    assert!(!a.queue.holds(), "the hold ends with the last line");
    assert_eq!(s.started, ["superbounce", "double-bounce", "triple-bounce"]);
}

/// With the feature off nothing is queued and nothing plays.
#[test]
fn switched_off_plays_nothing() {
    let mut a = Announcer::new();
    let lines = hud_lines(&mut a, vec![award(1, 1, false), award(2, 2, true)]);
    assert_eq!(lines.len(), 2, "the HUD still shows its lines");
    assert_eq!(a.queue.waiting().count(), 0);
    let mut s = Fake::default();
    play_out(&mut a, &mut s);
    assert!(s.started.is_empty());
    assert!(!a.queue.holds());
}

/// A Monster's line is Monster bounce (added last, so it wins over the
/// bounce's chain medal), and its clip is the one spoken; Bounced has no
/// clip and is silent.
#[test]
fn monster_speaks_its_clip_and_bounced_is_silent() {
    let mut a = on();
    let lines = hud_lines(&mut a, vec![award(1, 2, true)]);
    assert_eq!(lines, vec![Medal::Monster.line()]);
    assert_eq!(a.queue.waiting().collect::<Vec<_>>(), ["monster-bounce"]);

    assert_eq!(a.announce(&Medal::Bounced.line()), None);
    assert_eq!(a.announce("Primed!"), None, "not a medal line");
    let mut s = Fake::default();
    play_out(&mut a, &mut s);
    assert_eq!(s.started, ["monster-bounce"]);
}

/// A Firefight or Siege line that is not a medal (art/announcer/README.md,
/// "Firefight and Siege lines").
fn is_game_line(id: &str) -> bool {
    id.ends_with("-on")
        || matches!(
            id,
            "new-wave" | "wave-complete" | "round-complete" | "set-complete" | "bonus-round"
                | "bonus-round-complete" | "lives-added" | "last-life" | "game-over"
                | "skulls-off" | "all-skulls-on" | "attack" | "defend" | "final-round"
                | "site-captured" | "reinforcements" | "round-over"
        )
}

/// Every medal but Bounced has a clip, named by its id.
#[test]
fn every_medal_but_bounced_has_a_clip() {
    for m in MEDALS {
        let want = (m != Medal::Bounced).then_some(m.id());
        assert_eq!(clip_id(m), want, "{m:?}");
        assert_eq!(clip_for_line(&m.line()), want, "{m:?}");
    }
    for (id, _) in CLIPS {
        let line = matches!(id, "firefight" | "siege") || is_game_line(id);
        assert!(Medal::from_id(id).is_some() || line, "{id}");
    }
}

/// A line of ours waits while the game's announcer is busy, and once it
/// plays it holds the game's queue until it ends.
#[test]
fn waits_for_the_game_announcer() {
    let mut a = on();
    a.announce(&Medal::Superbounce.line());
    let mut s = Fake::default();
    assert_eq!(a.queue.step(&mut s, false), None);
    assert!(!a.queue.holds());
    assert_eq!(a.queue.step(&mut s, true), Some("superbounce"));
    // The game queued a line meanwhile: ours plays on, holding it.
    assert_eq!(a.queue.step(&mut s, false), None);
    assert!(a.queue.holds());
    s.sounding = false;
    assert_eq!(a.queue.step(&mut s, false), None);
    assert!(!a.queue.holds(), "ended: the game's line goes next");
}

/// Past the queue's length the oldest waiting line gives way, so the
/// newest medal is spoken.
#[test]
fn a_full_queue_drops_its_oldest_line() {
    let mut a = on();
    let all = [
        Medal::Superbounce,
        Medal::DoubleBounce,
        Medal::TripleBounce,
        Medal::QuadBounce,
        Medal::ChainBounce,
        Medal::Bouncetacular,
    ];
    for m in all {
        a.announce(&m.line());
    }
    assert_eq!(
        a.queue.waiting().collect::<Vec<_>>(),
        ["triple-bounce", "quad-bounce", "chain-bounce", "bouncetacular"]
    );
    assert_eq!(a.queue.waiting().count(), MAX_WAITING);
}

/// A clip that does not play is skipped for the next.
#[test]
fn a_clip_that_does_not_play_is_skipped() {
    let mut a = on();
    a.announce(&Medal::Superbounce.line());
    a.announce(&Medal::DoubleBounce.line());
    let mut s = Fake {
        broken: vec!["superbounce"],
        ..Fake::default()
    };
    assert_eq!(a.queue.step(&mut s, true), Some("double-bounce"));
}

/// A new game drops the lines waiting; the one playing plays out.
#[test]
fn a_new_game_drops_the_waiting_lines() {
    let mut a = on();
    a.announce(&Medal::Superbounce.line());
    a.announce(&Medal::DoubleBounce.line());
    let mut s = Fake::default();
    a.queue.step(&mut s, true);
    a.queue.clear_waiting();
    assert_eq!(a.queue.speaking(), Some("superbounce"));
    s.sounding = false;
    assert_eq!(a.queue.step(&mut s, true), None);
}

// ---- the game's queue ----

fn guest() -> FlatMemory {
    FlatMemory::new(announcer_queue::QUEUE, 0x100)
}

fn set(m: &mut FlatMemory, count: u32, playing: bool, delay: i16) {
    m.set_u32(announcer_queue::COUNT, count).unwrap();
    m.set_u8(announcer_queue::HEAD_PLAYING, playing as u8).unwrap();
    m.set_u16(announcer_queue::HEAD_DELAY, delay as u16).unwrap();
}

#[test]
fn the_game_announcer_is_idle_only_with_nothing_to_say() {
    let mut m = guest();
    set(&mut m, 0, false, -1);
    assert!(game_idle(&m).unwrap(), "empty");
    set(&mut m, 1, false, -1);
    assert!(game_idle(&m).unwrap(), "the head played and is about to go");
    set(&mut m, 1, true, -1);
    assert!(!game_idle(&m).unwrap(), "the head is playing");
    set(&mut m, 1, false, 12);
    assert!(!game_idle(&m).unwrap(), "the head is counting down");
    set(&mut m, 2, false, -1);
    assert!(!game_idle(&m).unwrap(), "a line waits");
}

// ---- the clips ----

/// A WAV of 16-bit PCM with `frames` samples per channel.
fn wav(channels: u16, rate: u32, frames: usize, extra_chunk: bool) -> Vec<u8> {
    let data: Vec<u8> = (0..frames * usize::from(channels))
        .flat_map(|i| (i as i16 * 7).to_le_bytes())
        .collect();
    let mut body = Vec::new();
    body.extend(b"WAVE");
    body.extend(b"fmt ");
    body.extend(16u32.to_le_bytes());
    body.extend(1u16.to_le_bytes());
    body.extend(channels.to_le_bytes());
    body.extend(rate.to_le_bytes());
    body.extend((rate * 2 * u32::from(channels)).to_le_bytes());
    body.extend((2 * channels).to_le_bytes());
    body.extend(16u16.to_le_bytes());
    if extra_chunk {
        // An odd-length chunk, padded to even.
        body.extend(b"LIST");
        body.extend(3u32.to_le_bytes());
        body.extend([1, 2, 3, 0]);
    }
    body.extend(b"data");
    body.extend((data.len() as u32).to_le_bytes());
    body.extend(&data);
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

#[test]
fn a_wav_reads_as_its_pcm() {
    let c = parse_wav(&wav(1, 44_100, 5, true)).unwrap();
    assert_eq!((c.channels, c.rate, c.data.len()), (1, 44_100, 10));
    assert_eq!(&c.data[2..4], &7i16.to_le_bytes());
    let c = parse_wav(&wav(2, 22_050, 3, false)).unwrap();
    assert_eq!((c.channels, c.rate, c.data.len()), (2, 22_050, 12));
}

#[test]
fn an_lfs_pointer_or_other_file_is_named() {
    let pointer = b"version https://git-lfs.github.com/spec/v1\noid sha256:00\nsize 1\n";
    assert_eq!(parse_wav(pointer), Err(ClipError::LfsPointer));
    assert!(ClipError::LfsPointer.to_string().contains("git lfs pull"));
    assert_eq!(parse_wav(b"OggS...."), Err(ClipError::NotWave));
    let mut eight_bit = wav(1, 8_000, 4, false);
    eight_bit[34] = 8; // bits per sample
    assert!(matches!(parse_wav(&eight_bit), Err(ClipError::Format(_))));
}

/// Each embedded clip is either the WAV (an LFS checkout) or its pointer
/// (`GIT_LFS_SKIP_SMUDGE`), never anything else. The bytes are not
/// asserted on beyond their format.
#[test]
fn every_embedded_clip_is_a_wav_or_its_lfs_pointer() {
    for (id, bytes) in CLIPS {
        match parse_wav(bytes) {
            Ok(c) => assert!(!c.data.is_empty(), "{id}"),
            Err(ClipError::LfsPointer) => {}
            Err(e) => panic!("art/announcer/{id}.wav: {e}"),
        }
    }
}

/// Both round starts contain audible PCM even in a source-only checkout.
#[test]
fn each_custom_type_has_a_playable_start_line() {
    for id in ["firefight", "siege"] {
        let clip = parse_wav(clip_bytes(id).expect("start line")).unwrap();
        let samples = clip.data.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]));
        assert!(samples.map(i16::unsigned_abs).max().unwrap_or(0) > 0, "{id}");
        assert!(clip.data.len() >= clip.rate as usize * usize::from(clip.channels) / 2, "{id}");
    }
    assert_ne!(clip_bytes("firefight"), clip_bytes("siege"));
}

/// The mixer speaker plays a clip as a buffer and reports it ended once
/// the mixer has rendered past it.
#[test]
fn the_mixer_plays_a_clip_to_its_end() {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(44_100)));
    let mut s = MixerSpeaker::new(Arc::clone(&mixer));
    let c = parse_wav(&wav(1, 44_100, 441, false)).unwrap();
    let format = audio::WaveFormat::pcm16(c.channels, c.rate).unwrap();
    s.clips.insert("superbounce", Some((format, c.data)));
    assert!(s.start("superbounce"));
    assert_eq!(lock(&mixer).buffer_count(), 1);
    assert!(s.sounding());
    let mut out = vec![0f32; 2 * 1024];
    lock(&mixer).render(&mut out);
    assert!(!s.sounding(), "rendered past its end");
    assert_eq!(lock(&mixer).buffer_count(), 0, "removed once ended");
    // A clip that does not read is not started.
    s.clips.insert("double-bounce", None);
    assert!(!s.start("double-bounce"));
}

/// A line that never ends lets the game's announcer go after
/// `MAX_HOLD_TICKS`.
#[test]
fn a_stuck_line_releases_the_hold() {
    let mut a = on();
    a.announce(&Medal::Superbounce.line());
    let mut s = Fake::default();
    a.queue.step(&mut s, true);
    for _ in 0..MAX_HOLD_TICKS {
        a.queue.step(&mut s, true);
        assert!(a.queue.holds());
    }
    a.queue.step(&mut s, true);
    assert!(!a.queue.holds(), "released though still sounding");
}
