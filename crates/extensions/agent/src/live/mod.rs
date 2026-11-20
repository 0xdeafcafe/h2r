//! The tick thread owns registration and two reusable world buffers. The
//! encoder owns its baselines. A blocked writer only backs up its own queue.

pub(crate) mod capture;

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use agent_proto::live::{Frame, Header, Kind, Layers, World};
use serde_json::{Value, json};

use crate::{ConnId, Reader};

const BUFFERS: usize = 2;
const QUEUE: usize = 8;
const SAMPLES: usize = 150;
pub(crate) const CLOSE_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    bytes: AtomicU64,
    queue: AtomicU64,
    dropped: AtomicU64,
    skipped: AtomicU64,
    closed: AtomicBool,
}

pub(crate) struct Frames {
    rx: Receiver<Vec<u8>>,
    counts: Arc<Counters>,
}

impl Frames {
    /// A write timeout lets live.close interrupt even a peer that stopped
    /// reading halfway through a frame. Keep the partial write's offset.
    pub fn write_to(self, w: &mut impl Write) -> io::Result<()> {
        while !self.counts.closed.load(Relaxed) {
            let frame = match self.rx.recv_timeout(CLOSE_INTERVAL) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            self.counts.queue.fetch_sub(1, Relaxed);
            let mut remaining = frame.as_slice();
            while !remaining.is_empty() {
                if self.counts.closed.load(Relaxed) {
                    return Ok(());
                }
                match w.write(remaining) {
                    Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                    Ok(n) => remaining = &remaining[n..],
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock
                                | io::ErrorKind::TimedOut
                                | io::ErrorKind::Interrupted
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            self.counts.bytes.fetch_add(frame.len() as u64, Relaxed);
            self.counts.frames.fetch_add(1, Relaxed);
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Stream {
    id: ConnId,
    layers: Layers,
    every: u32,
    key_every: u32,
    start: u64,
    tx: SyncSender<Vec<u8>>,
    counts: Arc<Counters>,
}

#[derive(Clone, Copy)]
pub(crate) struct Stamp {
    /// Monotonic GAME_TICK count, including across a restore.
    pub clock: u64,
    /// The game's own tick, as stamped on sim.objects.
    pub tick: u64,
    pub epoch: u64,
    pub paused: bool,
    pub stepping: bool,
}

impl Stream {
    fn due(&self, at: Stamp, only: Option<ConnId>) -> bool {
        if let Some(id) = only {
            return id == self.id;
        }
        at.stepping || (at.clock - self.start).is_multiple_of(u64::from(self.every))
    }
}

struct Sample {
    at: Stamp,
    only: Option<ConnId>,
    gap: u64,
    streams: Arc<Vec<Stream>>,
    world: World,
}

struct Pipeline {
    free: Receiver<World>,
    jobs: SyncSender<Sample>,
    /// Failed or unavailable capture: reuse its partial buffer next time.
    spare: Option<World>,
    gap: u64,
}

impl Pipeline {
    fn new() -> io::Result<Self> {
        let (recycle, free) = mpsc::sync_channel(BUFFERS);
        let (jobs, rx) = mpsc::sync_channel::<Sample>(BUFFERS);
        for _ in 0..BUFFERS {
            recycle
                .try_send(World::default())
                .expect("pool has two slots");
        }
        thread::Builder::new()
            .name("live-encode".into())
            .spawn(move || {
                let mut encoders: BTreeMap<ConnId, Encoder> = BTreeMap::new();
                for sample in rx {
                    encoders.retain(|id, _| sample.streams.iter().any(|s| s.id == *id));
                    for s in sample.streams.iter() {
                        if s.due(sample.at, sample.only) && !s.counts.closed.load(Relaxed) {
                            encoders.entry(s.id).or_default().send(s, &sample);
                        }
                    }
                    if recycle.try_send(sample.world).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            free,
            jobs,
            spare: None,
            gap: 0,
        })
    }
}

#[derive(Default)]
struct Encoder {
    base: Option<World>,
    seq: u64,
    last_key: u64,
    last_tick: Option<u64>,
    epoch: Option<u64>,
    gap: u64,
    force_key: bool,
}

impl Encoder {
    fn send(&mut self, s: &Stream, sample: &Sample) {
        let at = sample.at;
        let cut = self.epoch.is_some_and(|e| e != at.epoch)
            || self.last_tick.is_some_and(|tick| at.tick <= tick);
        let key = self.base.is_none()
            || self.force_key
            || self.gap != sample.gap
            || cut
            || at.clock - self.last_key >= u64::from(s.key_every);
        let frame = Frame::between(
            Header {
                kind: if key { Kind::Key } else { Kind::Delta },
                seq: self.seq + 1,
                tick: at.tick,
                epoch: at.epoch,
                paused: at.paused,
                cut,
            },
            s.layers,
            self.base.as_ref(),
            &sample.world,
        );
        let Ok(bytes) = frame.encode() else {
            self.dropped(s);
            return;
        };
        // Increment before publishing: a fast writer can pop immediately.
        s.counts.queue.fetch_add(1, Relaxed);
        if s.tx.try_send(bytes).is_err() {
            s.counts.queue.fetch_sub(1, Relaxed);
            self.dropped(s);
            return;
        }
        if frame.header.kind == Kind::Key {
            self.last_key = at.clock;
        }
        self.seq += 1;
        self.epoch = Some(at.epoch);
        self.last_tick = Some(at.tick);
        self.gap = sample.gap;
        self.force_key = false;
        self.base
            .get_or_insert_with(World::default)
            .clone_from(&sample.world);
    }

    fn dropped(&mut self, s: &Stream) {
        s.counts.dropped.fetch_add(1, Relaxed);
        self.force_key = true;
    }
}

#[derive(Default)]
pub(crate) struct Mirror {
    // State's existing game-boundary lock owns membership. The worker
    // sees the exact subscribers for that capture, without locking it.
    streams: Arc<Vec<Stream>>,
    pipeline: Option<Pipeline>,
    times: VecDeque<u64>,
}

impl Mirror {
    pub fn active(&self) -> bool {
        self.pipeline.is_some()
    }

    pub fn subscribe(
        &mut self,
        id: ConnId,
        layers: Layers,
        every: u32,
        key_every: u32,
        clock: u64,
    ) -> io::Result<Frames> {
        if self.pipeline.is_none() {
            self.pipeline = Some(Pipeline::new()?);
            self.times = VecDeque::with_capacity(SAMPLES);
        }
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let counts = Arc::new(Counters::default());
        Arc::make_mut(&mut self.streams).push(Stream {
            id,
            layers,
            every,
            key_every,
            start: clock,
            tx,
            counts: counts.clone(),
        });
        Ok(Frames { rx, counts })
    }

    pub fn close(&mut self, id: ConnId) -> bool {
        let Some(i) = self.streams.iter().position(|s| s.id == id) else {
            return false;
        };
        self.streams[i].counts.closed.store(true, Relaxed);
        Arc::make_mut(&mut self.streams).remove(i);
        if self.streams.is_empty() {
            self.pipeline = None;
        }
        true
    }

    pub fn close_all(&mut self) {
        for s in self.streams.iter() {
            s.counts.closed.store(true, Relaxed);
        }
        self.streams = Arc::default();
        self.pipeline = None;
    }

    pub fn capture(&mut self, reader: &mut dyn Reader, at: Stamp, only: Option<ConnId>) {
        let Some(p) = &mut self.pipeline else { return };
        let mut due = self.streams.iter().filter(|s| s.due(at, only)).peekable();
        if due.peek().is_none() {
            return;
        }
        let mut layers = Layers::default();
        for stream in due.clone() {
            layers.objects |= stream.layers.objects;
            layers.players |= stream.layers.players;
        }
        let start = Instant::now();
        let next = p.spare.take().or_else(|| p.free.try_recv().ok());
        if let Some(mut world) = next {
            if reader.live_capture(layers, &mut world) {
                let sample = Sample {
                    at,
                    only,
                    gap: p.gap,
                    streams: self.streams.clone(),
                    world,
                };
                if let Err(err) = p.jobs.try_send(sample) {
                    p.spare = Some(match err {
                        mpsc::TrySendError::Full(s) | mpsc::TrySendError::Disconnected(s) => {
                            s.world
                        }
                    });
                } else {
                    if self.times.len() == SAMPLES {
                        self.times.pop_front();
                    }
                    self.times.push_back(start.elapsed().as_micros() as u64);
                    return;
                }
            } else {
                p.spare = Some(world);
            }
        }
        p.gap += 1;
        for stream in due {
            stream.counts.skipped.fetch_add(1, Relaxed);
        }
    }

    pub fn status(&self) -> Value {
        let streams: Vec<_> = self.streams.iter().map(|s| {
            let c = &s.counts;
            json!({"stream": s.id, "layers": s.layers.names(), "every": s.every,
                   "frames": c.frames.load(Relaxed), "bytes": c.bytes.load(Relaxed),
                   "queue": c.queue.load(Relaxed).min(QUEUE as u64), "skipped": c.skipped.load(Relaxed),
                   "dropped": c.dropped.load(Relaxed)})
        }).collect();
        let mut us: Vec<_> = self.times.iter().copied().collect();
        us.sort_unstable();
        let percentile = |p: usize| {
            us.get((us.len() * p).div_ceil(100).saturating_sub(1))
                .copied()
        };
        json!({"streams": streams, "capture_us": {"p50": percentile(50usize),
               "p99": percentile(99usize), "budget": 150}, "shed": []})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::live::Object;

    struct Pool {
        x: u32,
    }

    impl Reader for Pool {
        fn status(&mut self) -> crate::GameStatus {
            crate::GameStatus::default()
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            unreachable!("world.query does not feed the stream")
        }
        fn live_capture(&mut self, _: Layers, world: &mut World) -> bool {
            self.x += 1;
            world.objects.clear();
            world.objects.push(Object {
                id: 1,
                position: [self.x as f32, 0.0, 0.0],
                ..Object::default()
            });
            true
        }
    }

    #[test]
    fn saturated_writer_drops_and_rekeys_and_busy_encoder_skips_without_reading() {
        // Drive both consumers explicitly: the encoder first runs against
        // a stalled socket writer, then holds both capture buffers.
        let (recycle, free) = mpsc::sync_channel(BUFFERS);
        let (jobs, work) = mpsc::sync_channel(BUFFERS);
        for _ in 0..BUFFERS {
            recycle.try_send(World::default()).unwrap();
        }
        let mut mirror = Mirror {
            pipeline: Some(Pipeline {
                jobs,
                free,
                gap: 0,
                spare: None,
            }),
            ..Default::default()
        };
        let reader = mirror
            .subscribe(
                1,
                Layers {
                    objects: true,
                    players: false,
                },
                1,
                150,
                0,
            )
            .unwrap();
        let mut enc = Encoder::default();
        let mut pool = Pool { x: 0 };
        let capture = |mirror: &mut Mirror, pool: &mut Pool, tick| {
            mirror.capture(
                pool,
                Stamp {
                    clock: tick,
                    tick,
                    epoch: 1,
                    paused: false,
                    stepping: false,
                },
                None,
            );
        };
        let encode = |enc: &mut Encoder| {
            let sample = work.try_recv().unwrap();
            enc.send(&sample.streams[0], &sample);
            recycle.try_send(sample.world).unwrap();
        };
        let pop = || {
            let bytes = reader.rx.try_recv().unwrap();
            reader.counts.queue.fetch_sub(1, Relaxed);
            Frame::read(&mut bytes.as_slice()).unwrap()
        };
        for n in 1..=QUEUE as u64 + 1 {
            capture(&mut mirror, &mut pool, n);
            encode(&mut enc);
        }
        assert_eq!(reader.counts.queue.load(Relaxed), QUEUE as u64);
        assert_eq!(mirror.status()["streams"][0]["dropped"], 1);
        let mut world = World::default();
        for tick in 1..=QUEUE as u64 {
            let frame = pop();
            assert_eq!(frame.header.tick, tick);
            assert_eq!(
                frame.header.kind,
                if tick == 1 { Kind::Key } else { Kind::Delta }
            );
            frame.apply(&mut world).unwrap();
        }

        capture(&mut mirror, &mut pool, 10);
        encode(&mut enc);
        let after_drop = pop();
        assert_eq!(after_drop.header.kind, Kind::Key);
        after_drop.apply(&mut world).unwrap();
        assert_eq!(world.objects[0].position[0], 10.0);

        capture(&mut mirror, &mut pool, 11);
        capture(&mut mirror, &mut pool, 12);
        capture(&mut mirror, &mut pool, 13);
        assert_eq!(pool.x, 12, "no read without a free buffer");
        assert_eq!(mirror.status()["streams"][0]["skipped"], 1);
        for _ in 0..2 {
            encode(&mut enc);
            assert_eq!(pop().header.kind, Kind::Delta);
        }
        capture(&mut mirror, &mut pool, 14);
        encode(&mut enc);
        assert_eq!(pop().header.kind, Kind::Key);
    }
}
