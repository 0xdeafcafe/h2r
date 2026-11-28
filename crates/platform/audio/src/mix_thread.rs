//! The thread that runs the sample half of a render ([`crate::Synth`]).
//!
//! The game thread prepares [`MixJob`]s in order with the guest's own calls
//! ([`crate::Mixer::prepare_job`]) and [`submit`](MixThread::submit)s them.
//! One worker takes them first in, first out, mixes each and hands the
//! samples to the `deliver` closure, so blocks leave in the order they were
//! prepared. The queue holds a bounded number of blocks; a producer that gets
//! ahead of the worker waits for room, so nothing is dropped here. The worker
//! never touches the mixer, guest memory or any lock the game thread holds.
//!
//! Work that was rendered inline (a mixer with taps, [`Work::Pcm`]) goes
//! through the same queue so it cannot overtake an earlier job.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::mixer::{MixJob, Synth};

/// Blocks the queue holds before `submit` waits: 64 quanta of 256 frames,
/// about 340 ms at 48 kHz.
pub const DEPTH: usize = 64;

/// One block for the worker, in submission order.
pub enum Work {
    /// A prepared block to mix.
    Job(Box<MixJob>),
    /// A block already mixed on the submitting thread.
    Pcm { main: Vec<f32>, side: Option<Vec<f32>> },
}

/// Gets each block's samples: interleaved stereo for the main output and, if
/// the block had one, the side output.
pub type Deliver = Box<dyn FnMut(&[f32], Option<&[f32]>) + Send>;

struct Progress {
    done: Mutex<u64>,
    cv: Condvar,
    unrouted: AtomicU64,
}

/// A worker thread mixing [`Work`] in order. Dropping it mixes what is
/// queued, then stops and joins the thread.
pub struct MixThread {
    tx: Option<SyncSender<Work>>,
    join: Option<JoinHandle<()>>,
    submitted: AtomicU64,
    progress: Arc<Progress>,
}

impl MixThread {
    /// Starts the worker with a queue of `depth` blocks (at least one).
    pub fn spawn(depth: usize, mut deliver: Deliver) -> Self {
        let (tx, rx) = sync_channel::<Work>(depth.max(1));
        let progress = Arc::new(Progress {
            done: Mutex::new(0),
            cv: Condvar::new(),
            unrouted: AtomicU64::new(0),
        });
        let p = Arc::clone(&progress);
        let join = std::thread::Builder::new()
            .name("audio-mix".into())
            .spawn(move || {
                let mut synth = Synth::new();
                let (mut main, mut side) = (Vec::new(), Vec::new());
                while let Ok(work) = rx.recv() {
                    match work {
                        Work::Job(job) => {
                            let n = job.frames() * 2;
                            let with_side = job.wants_side();
                            main.resize(n, 0.0);
                            side.resize(if with_side { n } else { 0 }, 0.0);
                            synth.mix(*job, &mut main, with_side.then_some(&mut side[..]));
                            deliver(&main, with_side.then_some(&side[..]));
                            p.unrouted.store(synth.unrouted_frames(), Ordering::Release);
                        }
                        Work::Pcm { main, side } => deliver(&main, side.as_deref()),
                    }
                    *p.done.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                    p.cv.notify_all();
                }
            })
            .expect("spawning the audio mix thread");
        Self {
            tx: Some(tx),
            join: Some(join),
            submitted: AtomicU64::new(0),
            progress,
        }
    }

    /// Queues `work` behind everything submitted before it, waiting for room
    /// if the queue is full. Returns false if the worker has stopped.
    pub fn submit(&self, work: Work) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        self.submitted.fetch_add(1, Ordering::AcqRel);
        if tx.send(work).is_err() {
            self.submitted.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }

    /// Blocks queued or being mixed.
    pub fn pending(&self) -> u64 {
        let done = *self.progress.done.lock().unwrap_or_else(|e| e.into_inner());
        self.submitted.load(Ordering::Acquire).saturating_sub(done)
    }

    /// Waits until everything submitted so far has been mixed and delivered.
    /// Returns early if the worker has died.
    pub fn flush(&self) {
        let target = self.submitted.load(Ordering::Acquire);
        let mut done = self.progress.done.lock().unwrap_or_else(|e| e.into_inner());
        while *done < target {
            if self.join.as_ref().is_none_or(|j| j.is_finished()) {
                return;
            }
            done = self
                .progress
                .cv
                .wait_timeout(done, Duration::from_millis(10))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Frames of stream voices whose routing reached nothing, as of the last
    /// block mixed (see [`Synth::unrouted_frames`]).
    pub fn unrouted_frames(&self) -> u64 {
        self.progress.unrouted.load(Ordering::Acquire)
    }
}

impl Drop for MixThread {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsound::SoundBuffer;
    use crate::format::WaveFormat;
    use crate::mixer::Mixer;

    const RATE: u32 = 8000;
    type Seen = Arc<Mutex<Vec<(Vec<f32>, Option<Vec<f32>>)>>>;

    fn recorder() -> (Seen, Deliver) {
        let seen: Seen = Arc::default();
        let s = Arc::clone(&seen);
        let deliver: Deliver = Box::new(move |main, side| {
            s.lock()
                .unwrap()
                .push((main.to_vec(), side.map(<[f32]>::to_vec)));
        });
        (seen, deliver)
    }

    fn voice() -> SoundBuffer {
        let pcm = (0..500)
            .flat_map(|i| (((i % 50) as i16 - 25) * 400).to_le_bytes())
            .collect();
        SoundBuffer::from_data(WaveFormat::pcm16(1, RATE).unwrap(), pcm).unwrap()
    }

    #[test]
    fn blocks_leave_in_submission_order_whatever_their_kind() {
        let (seen, deliver) = recorder();
        let t = MixThread::spawn(2, deliver);
        for i in 0..200 {
            let v = i as f32;
            // A depth of two makes the producer wait for room many times.
            assert!(t.submit(Work::Pcm {
                main: vec![v; 4],
                side: (i % 3 == 0).then(|| vec![-v; 4]),
            }));
        }
        t.flush();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 200);
        for (i, (main, side)) in seen.iter().enumerate() {
            assert_eq!(main, &vec![i as f32; 4], "block {i}");
            assert_eq!(side.is_some(), i % 3 == 0, "side of block {i}");
        }
        assert_eq!(t.pending(), 0);
    }

    #[test]
    fn a_job_is_mixed_after_the_blocks_before_it_and_matches_the_inline_render() {
        let mut inline = Mixer::new(RATE);
        let mut split = Mixer::new(RATE);
        let mut ids = Vec::new();
        for m in [&mut inline, &mut split] {
            let id = m.add_buffer(voice());
            m.buffer_mut(id).unwrap().set_pan(-1200);
            m.buffer_mut(id).unwrap().play(true);
            ids.push(id);
        }
        let (seen, deliver) = recorder();
        let t = MixThread::spawn(DEPTH, deliver);
        let mut want = Vec::new();
        for block in 0..30 {
            if block == 12 {
                // The guest moves the play cursor between blocks, and reads
                // it straight back without waiting for the mix.
                inline.buffer_mut(ids[0]).unwrap().set_current_position(100);
                split.buffer_mut(ids[1]).unwrap().set_current_position(100);
                assert_eq!(split.buffer(ids[1]).unwrap().current_position(), 100);
            }
            let mut out = vec![0.0; 128];
            inline.render(&mut out);
            want.push(out);
            let job = split.prepare_job(64, false);
            assert!(t.submit(Work::Job(Box::new(job))));
            assert_eq!(
                inline.buffer(ids[0]).unwrap().current_position(),
                split.buffer(ids[1]).unwrap().current_position(),
                "position after block {block}"
            );
        }
        t.flush();
        let got: Vec<_> = seen.lock().unwrap().iter().map(|(m, _)| m.clone()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn dropping_mixes_what_is_queued() {
        let (seen, deliver) = recorder();
        let t = MixThread::spawn(DEPTH, deliver);
        for i in 0..10 {
            t.submit(Work::Pcm { main: vec![i as f32], side: None });
        }
        drop(t);
        assert_eq!(seen.lock().unwrap().len(), 10);
    }
}
