use std::collections::HashMap;

use sim_sound::feed::Ear;
use sim_sound::impulse::Selection;
use sound_clips::Clip;

use super::*;

const TAG: TagIndex = TagIndex(0xe100_0001);
const RATE: u32 = 48_000;

#[derive(Clone)]
enum Answer {
    Ready(Arc<Clip>),
    Pending,
    Refused,
    /// A stream of these runs.
    Chunks(Vec<Vec<i16>>),
}

/// Clips by key, which a test can change after the host holds them.
#[derive(Clone, Default)]
struct Fake {
    answers: Arc<Mutex<HashMap<ClipKey, Answer>>>,
    asked: Arc<Mutex<Vec<ClipKey>>>,
}

impl Fake {
    fn set(&self, key: ClipKey, a: Answer) {
        self.answers.lock().unwrap().insert(key, a);
    }
}

struct Runs(VecDeque<Vec<i16>>);

impl Chunks for Runs {
    fn channels(&self) -> u16 {
        1
    }
    fn rate(&self) -> u32 {
        RATE
    }
    fn next_chunk(&mut self) -> Result<Option<Vec<i16>>, ClipError> {
        Ok(self.0.pop_front())
    }
}

impl Clips for Fake {
    fn get(&mut self, key: ClipKey) -> Result<Got, ClipError> {
        self.asked.lock().unwrap().push(key);
        match self.answers.lock().unwrap().get(&key).cloned() {
            Some(Answer::Ready(c)) => Ok(Got::Ready(c)),
            Some(Answer::Pending) => Ok(Got::Pending),
            Some(Answer::Refused) | None => Err(ClipError::Refused("refused".into())),
            Some(Answer::Chunks(_)) => Err(ClipError::Failed("a stream".into())),
        }
    }
    fn stream(&mut self, key: ClipKey) -> Result<Box<dyn Chunks>, ClipError> {
        self.asked.lock().unwrap().push(key);
        match self.answers.lock().unwrap().get(&key).cloned() {
            Some(Answer::Chunks(runs)) => Ok(Box::new(Runs(runs.into()))),
            _ => Err(ClipError::Refused("no stream".into())),
        }
    }
}

/// `ms` of mono at [`RATE`].
fn clip(ms: u32) -> Arc<Clip> {
    let frames = (RATE * ms / 1000) as usize;
    Arc::new(Clip { rate: RATE, channels: 1, pcm: vec![1000i16; frames].into() })
}

fn host() -> (MixerHost<Fake>, Fake, Arc<Mutex<audio::Mixer>>) {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(RATE)));
    let fake = Fake::default();
    (MixerHost::new(Arc::clone(&mixer), fake.clone()), fake, mixer)
}

fn handle(n: u32) -> Handle {
    Handle::from_raw(0xef73_0000 | n)
}

fn started(h: Handle, selection: (i8, i8), tick: u32) -> TableEvent {
    TableEvent::Started {
        handle: h,
        tag: TAG,
        selection: Selection { range: selection.0, permutation: selection.1, chunk: 0 },
        gain: 0.0,
        pitch: 0.0,
        source: SourceParams::default(),
        looping: false,
        detail: false,
        level: 0.0,
        distance: [1.0, 100.0],
        track: None,
        follows: None,
        tick,
    }
}

fn on_track(e: TableEvent, follows: Option<Handle>) -> TableEvent {
    match e {
        TableEvent::Started { handle: h, tag, selection, gain, pitch, source, detail, level, distance, tick, .. } => TableEvent::Started {
            handle: h,
            tag,
            selection,
            gain,
            pitch,
            source,
            looping: true,
            detail,
            level,
            distance,
            track: Some((handle(90), 0)),
            follows,
            tick,
        },
        other => other,
    }
}

fn scene(tick: u32) -> Scene {
    Scene { tick, ear: None, sources: Vec::new() }
}

fn render(mixer: &Mutex<audio::Mixer>, ms: u32) {
    let mut out = vec![0.0f32; (RATE * ms / 1000) as usize * 2];
    lock(mixer).render(&mut out);
}

#[test]
fn a_start_plays_the_drawn_permutation() {
    let (mut h, fake, mixer) = host();
    let key = clip_key(TAG, 1, 2);
    fake.set(key, Answer::Ready(clip(100)));
    let mut e = started(handle(1), (1, 2), 0);
    if let TableEvent::Started { gain, level, pitch, .. } = &mut e {
        (*gain, *level, *pitch) = (-4.0, -2.0, 1200.0);
    }
    h.event(&e);
    h.frame(&scene(0));
    assert_eq!(*fake.asked.lock().unwrap(), [key], "the selection the table drew, nothing else");
    let v = h.voice(handle(1)).unwrap();
    assert_eq!((v.frequency, v.volume_mb, v.streamed, v.position), (2 * RATE, -600, false, None));
    assert_eq!(lock(&mixer).buffer_count(), 1);
    assert_eq!(h.feed_counters().started, 1);
}

#[test]
fn a_stop_ramps_the_buffer_down_over_the_fade() {
    let (mut h, fake, mixer) = host();
    fake.set(clip_key(TAG, 0, 0), Answer::Ready(clip(1000)));
    h.event(&started(handle(1), (0, 0), 0));
    h.event(&TableEvent::Stopped { handle: handle(1), fade_ticks: 6, tick: 0 });
    h.frame(&scene(0));
    assert_eq!(h.voice(handle(1)).unwrap().volume_mb, 0);
    h.frame(&scene(3));
    assert_eq!(h.voice(handle(1)).unwrap().volume_mb, -602, "half way: 20 log 0.5");
    h.frame(&scene(6));
    assert_eq!(h.voice(handle(1)).unwrap().volume_mb, audio::dsound::VOLUME_MIN_MB);
    h.event(&TableEvent::Ended { handle: handle(1), reason: Reason::Faded, tick: 6 });
    assert_eq!((h.held(), lock(&mixer).buffer_count()), (0, 0), "a faded sound goes at once");
}

#[test]
fn a_played_out_buffer_finishes_then_is_swept() {
    let (mut h, fake, mixer) = host();
    fake.set(clip_key(TAG, 0, 0), Answer::Ready(clip(100)));
    h.event(&started(handle(1), (0, 0), 0));
    h.frame(&scene(0));
    h.event(&TableEvent::Ended { handle: handle(1), reason: Reason::PlayedOut, tick: 3 });
    h.frame(&scene(3));
    assert_eq!(lock(&mixer).buffer_count(), 1, "it plays to its own end");
    render(&mixer, 150);
    h.frame(&scene(4));
    assert_eq!((h.held(), lock(&mixer).buffer_count()), (0, 0));
}

#[test]
fn a_culled_voice_changes_no_sim_state() {
    let (mut h, fake, mixer) = host();
    fake.set(clip_key(TAG, 0, 0), Answer::Ready(clip(100)));
    for n in 0..=MAX_VOICES as u32 {
        h.event(&started(handle(n), (0, 0), 0));
    }
    let culled = handle(MAX_VOICES as u32);
    // Its stop and end come all the same, and touch nothing.
    h.event(&TableEvent::Stopped { handle: culled, fade_ticks: 3, tick: 0 });
    h.event(&TableEvent::Ended { handle: culled, reason: Reason::Faded, tick: 3 });
    h.frame(&scene(0));
    assert_eq!((h.feed_counters().started, h.feed_counters().culled), (MAX_VOICES as u64, 1));
    assert!(h.voice(culled).is_none());
    assert_eq!(lock(&mixer).buffer_count(), MAX_VOICES);
}

#[test]
fn a_late_clip_starts_at_the_elapsed_offset() {
    let (mut h, fake, _) = host();
    let key = clip_key(TAG, 0, 0);
    fake.set(key, Answer::Pending);
    h.event(&started(handle(1), (0, 0), 0));
    h.frame(&scene(0));
    assert!(h.voice(handle(1)).unwrap().waiting);
    fake.set(key, Answer::Ready(clip(1000)));
    // Three ticks: 100 ms in.
    h.frame(&scene(3));
    let v = h.voice(handle(1)).unwrap();
    assert!(!v.waiting);
    assert_eq!(v.cursor, f64::from(RATE / 10));
}

#[test]
fn a_clip_later_than_the_limit_is_dropped_and_counted() {
    let (mut h, fake, mixer) = host();
    let key = clip_key(TAG, 0, 0);
    fake.set(key, Answer::Pending);
    h.event(&started(handle(1), (0, 0), 0));
    h.frame(&scene(7));
    assert!(h.voice(handle(1)).is_some(), "233 ms: still in time");
    fake.set(key, Answer::Ready(clip(1000)));
    h.frame(&scene(8));
    assert!(h.voice(handle(1)).is_none(), "267 ms: past the limit");
    assert_eq!(h.feed_counters().clip_pending_dropped, 1);
    assert_eq!(lock(&mixer).buffer_count(), 0);
}

#[test]
fn a_refused_clip_is_counted_and_nothing_else_plays() {
    let (mut h, fake, mixer) = host();
    fake.set(clip_key(TAG, 0, 0), Answer::Refused);
    fake.set(clip_key(TAG, 0, 1), Answer::Ready(clip(100)));
    h.event(&started(handle(1), (0, 0), 0));
    h.frame(&scene(0));
    assert_eq!(h.feed_counters().clip_refused, 1);
    assert_eq!((h.held(), lock(&mixer).buffer_count()), (0, 0));
    assert_eq!(*fake.asked.lock().unwrap(), [clip_key(TAG, 0, 0)], "no other permutation is tried");
}

#[test]
fn a_moving_source_follows_its_marker() {
    let (mut h, fake, _) = host();
    fake.set(clip_key(TAG, 0, 0), Answer::Ready(clip(1000)));
    let at = |x: f32| {
        let mut s = SourceParams::default();
        s.0[0] = 1 << 24;
        s.0[3] = x.to_bits();
        s
    };
    h.event(&started(handle(1), (0, 0), 0));
    let ear = Ear { position: [0.0; 3], forward: [1.0, 0.0, 0.0], left: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0] };
    let mut s = Scene { tick: 0, ear: Some(ear), sources: vec![(handle(1), at(5.0))] };
    h.frame(&s);
    assert_eq!(h.voice(handle(1)).unwrap().position, Some([0.0, 0.0, 5.0]), "5 ahead");
    s.sources = vec![(handle(1), at(-8.0))];
    h.frame(&s);
    assert_eq!(h.voice(handle(1)).unwrap().position, Some([0.0, 0.0, -8.0]), "behind now");
    // Past the maximum distance it is silent.
    s.sources = vec![(handle(1), at(150.0))];
    h.frame(&s);
    assert_eq!(h.voice(handle(1)).unwrap().volume_mb, audio::dsound::VOLUME_MIN_MB);
}

#[test]
fn a_music_track_streams_from_chunks() {
    let (mut h, fake, mixer) = host();
    // Ten runs of 100 ms.
    let run = vec![500i16; (RATE / 10) as usize];
    fake.set(clip_key(TAG, 0, 0), Answer::Chunks(vec![run; 10]));
    h.event(&on_track(started(handle(1), (0, 0), 0), None));
    h.frame(&scene(0));
    assert!(h.voice(handle(1)).unwrap().streamed);
    assert_eq!(lock(&mixer).buffer_count(), 0);
    assert_eq!(lock(&mixer).stream_count(), 1);
    assert_eq!(h.feed_counters().streamed_chunks, 5, "500 ms ahead");
    render(&mixer, 300);
    h.frame(&scene(9));
    assert_eq!(h.feed_counters().streamed_chunks, 8, "kept 500 ms ahead of the cursor");
    render(&mixer, 1000);
    h.frame(&scene(40));
    assert_eq!(h.feed_counters().streamed_chunks, 10);
}

#[test]
fn a_following_segment_queues_on_its_predecessors_stream() {
    let (mut h, fake, mixer) = host();
    let run = vec![500i16; (RATE / 10) as usize];
    fake.set(clip_key(TAG, 0, 0), Answer::Chunks(vec![run.clone(); 2]));
    fake.set(clip_key(TAG, 0, 1), Answer::Chunks(vec![run.clone(); 2]));
    fake.set(clip_key(TAG, 0, 2), Answer::Chunks(vec![run; 2]));
    h.event(&on_track(started(handle(1), (0, 0), 0), None));
    h.frame(&scene(0));
    render(&mixer, 180);
    // The table's order on the seam tick: the next segment, then the end.
    h.event(&on_track(started(handle(2), (0, 1), 6), Some(handle(1))));
    h.event(&TableEvent::Ended { handle: handle(1), reason: Reason::PlayedOut, tick: 6 });
    h.frame(&scene(6));
    assert_eq!(lock(&mixer).stream_count(), 1, "one stream: no gap at the seam");
    assert!(h.voice(handle(2)).unwrap().streamed);
    assert_eq!(h.feed_counters().seam_cuts, 0);
    // An overlay on the same track follows nothing: a stream of its own.
    h.event(&on_track(started(handle(3), (0, 2), 6), None));
    h.frame(&scene(6));
    assert_eq!(lock(&mixer).stream_count(), 2);
    // The last member's stop takes the stream with it.
    h.event(&TableEvent::Ended { handle: handle(2), reason: Reason::Faded, tick: 9 });
    h.event(&TableEvent::Ended { handle: handle(3), reason: Reason::Faded, tick: 9 });
    assert_eq!((h.held(), lock(&mixer).stream_count()), (0, 0));
}

#[test]
fn a_segment_the_sim_starts_early_is_cut_in_by_the_excess_lead() {
    let (mut h, fake, mixer) = host();
    let run = vec![500i16; (RATE / 10) as usize];
    fake.set(clip_key(TAG, 0, 0), Answer::Chunks(vec![run.clone(); 3]));
    fake.set(clip_key(TAG, 0, 1), Answer::Chunks(vec![run; 3]));
    h.event(&on_track(started(handle(1), (0, 0), 0), None));
    h.frame(&scene(0));
    // 300 ms queued, none played: the next segment comes 234 ms early.
    h.event(&on_track(started(handle(2), (0, 1), 2), Some(handle(1))));
    h.frame(&scene(2));
    assert_eq!(h.feed_counters().seam_cuts, 1);
    assert_eq!(lock(&mixer).stream_count(), 1);
}

#[test]
fn clear_takes_every_voice_at_once() {
    let (mut h, fake, mixer) = host();
    fake.set(clip_key(TAG, 0, 0), Answer::Ready(clip(100)));
    fake.set(clip_key(TAG, 0, 1), Answer::Chunks(vec![vec![0; 100]]));
    h.event(&started(handle(1), (0, 0), 0));
    h.event(&on_track(started(handle(2), (0, 1), 0), None));
    h.frame(&scene(0));
    h.clear();
    // One guard at a time: two in a statement would deadlock on the mutex.
    let buffers = lock(&mixer).buffer_count();
    let streams = lock(&mixer).stream_count();
    assert_eq!((h.held(), buffers, streams), (0, 0, 0));
}
