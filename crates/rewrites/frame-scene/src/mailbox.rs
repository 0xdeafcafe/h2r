//! The sim-to-render hand-off (docs/graphics/framescene.md, section 6).
//! Publish a sealed scene and the resources needed to draw it together.
//! A display-paced reader retains the last two **taken** scenes itself.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// Whether the presentation clock or the simulation sets the pace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Play. Replace the unread scene when the simulation runs ahead.
    Paced,
    /// Capture. Deliver every scene, in order, with room for two waiting.
    Locked,
}

/// Counts for this render session. A taken scene may still be drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub published: u64,
    pub taken: u64,
    /// Overwritten before the renderer could take them (paced only).
    pub dropped: u64,
    pub waiting: usize,
}

const LOCKED_CAPACITY: usize = 2;

struct Queue<T> {
    scenes: VecDeque<T>,
    published: u64,
    taken: u64,
    dropped: u64,
    disconnected: bool,
}

struct Shared<T> {
    mode: Mode,
    queue: Mutex<Queue<T>>,
    changed: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, Queue<T>> {
        self.queue.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stats(&self) -> Stats {
        let queue = self.lock();
        Stats {
            published: queue.published,
            taken: queue.taken,
            dropped: queue.dropped,
            waiting: queue.scenes.len(),
        }
    }
}

/// Owned by the sim thread. Dropping it lets the renderer drain and exit.
pub struct Publisher<T>(Arc<Shared<T>>);

/// Owned by the render thread. Dropping it wakes a blocked publisher.
pub struct Receiver<T>(Arc<Shared<T>>);

pub fn channel<T>(mode: Mode) -> (Publisher<T>, Receiver<T>) {
    let shared = Arc::new(Shared {
        mode,
        queue: Mutex::new(Queue {
            scenes: VecDeque::with_capacity(LOCKED_CAPACITY),
            published: 0,
            taken: 0,
            dropped: 0,
            disconnected: false,
        }),
        changed: Condvar::new(),
    });
    (Publisher(Arc::clone(&shared)), Receiver(shared))
}

impl<T> Publisher<T> {
    /// Hand over a scene, or return it if the render thread has exited.
    /// Only a locked publisher waits for space.
    pub fn publish(&self, scene: T) -> Result<(), T> {
        let mut queue = self.0.lock();
        let mut replaced = None;
        match self.0.mode {
            Mode::Paced => {
                replaced = queue.scenes.pop_back();
                queue.dropped += u64::from(replaced.is_some());
            }
            Mode::Locked => {
                queue = self
                    .0
                    .changed
                    .wait_while(queue, |q| !q.disconnected && q.scenes.len() == LOCKED_CAPACITY)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
        if queue.disconnected {
            return Err(scene);
        }
        queue.scenes.push_back(scene);
        queue.published += 1;
        drop(queue);
        self.0.changed.notify_all();
        // A replaced scene can own the last reference to large resources.
        // Release those references outside the queue lock.
        drop(replaced);
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        self.0.stats()
    }
}

impl<T> Drop for Publisher<T> {
    fn drop(&mut self) {
        self.0.lock().disconnected = true;
        self.0.changed.notify_all();
    }
}

impl<T> Receiver<T> {
    /// Wait for a scene. Returns `None` after the producer drops and all
    /// queued scenes have been taken.
    pub fn take(&self) -> Option<T> {
        let queue = self
            .0
            .changed
            .wait_while(self.0.lock(), |q| !q.disconnected && q.scenes.is_empty())
            .unwrap_or_else(|e| e.into_inner());
        self.take_front(queue)
    }

    /// Take a waiting scene, or keep displaying the last one.
    pub fn try_take(&self) -> Option<T> {
        self.take_front(self.0.lock())
    }

    fn take_front(&self, mut queue: MutexGuard<'_, Queue<T>>) -> Option<T> {
        let scene = queue.scenes.pop_front()?;
        queue.taken += 1;
        drop(queue);
        self.0.changed.notify_all();
        Some(scene)
    }

    pub fn stats(&self) -> Stats {
        self.0.stats()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut queue = self.0.lock();
        queue.disconnected = true;
        let unread = std::mem::take(&mut queue.scenes);
        drop(queue);
        self.0.changed.notify_all();
        drop(unread);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn paced_replaces_only_scenes_the_reader_has_not_taken() {
        let (tx, rx) = channel(Mode::Paced);
        assert_eq!(rx.try_take(), None);
        tx.publish(1).unwrap();
        assert_eq!(rx.take(), Some(1));
        for frame in 2..=5 {
            tx.publish(frame).unwrap();
        }
        assert_eq!(tx.stats(), Stats { published: 5, taken: 1, dropped: 3, waiting: 1 });
        drop(tx);
        assert_eq!(rx.try_take(), Some(5));
        assert_eq!(rx.take(), None);
        assert_eq!(rx.stats(), Stats { published: 5, taken: 2, dropped: 3, waiting: 0 });
    }

    #[test]
    fn locked_blocks_at_two_and_delivers_every_scene_in_order() {
        let (tx, rx) = channel(Mode::Locked);
        tx.publish(1).unwrap();
        tx.publish(2).unwrap();
        let (started, ready) = mpsc::sync_channel(0);
        let (completed, done) = mpsc::channel();
        let writer = thread::spawn(move || {
            started.send(()).unwrap();
            tx.publish(3).unwrap();
            completed.send(()).unwrap();
        });
        ready.recv().unwrap();
        assert_eq!(
            done.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(rx.stats().waiting, 2);
        assert_eq!(rx.take(), Some(1));
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        writer.join().unwrap();
        assert_eq!(rx.take(), Some(2));
        assert_eq!(rx.take(), Some(3));
        assert_eq!(rx.take(), None);
        assert_eq!(rx.stats(), Stats { published: 3, taken: 3, dropped: 0, waiting: 0 });
    }

    #[test]
    fn render_thread_exit_returns_a_waiting_scene_to_its_publisher() {
        let (tx, rx) = channel(Mode::Locked);
        tx.publish(1).unwrap();
        tx.publish(2).unwrap();
        let (started, ready) = mpsc::sync_channel(0);
        let (completed, done) = mpsc::channel();
        let writer = thread::spawn(move || {
            started.send(()).unwrap();
            completed.send(tx.publish(3)).unwrap();
        });
        ready.recv().unwrap();
        drop(rx);
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), Err(3));
        writer.join().unwrap();
    }

    #[test]
    fn producer_shutdown_wakes_a_reader_waiting_for_its_first_scene() {
        let (tx, rx) = channel::<u32>(Mode::Paced);
        let (started, ready) = mpsc::sync_channel(0);
        let (completed, done) = mpsc::channel();
        let reader = thread::spawn(move || {
            started.send(()).unwrap();
            completed.send(rx.take()).unwrap();
        });
        ready.recv().unwrap();
        drop(tx);
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        reader.join().unwrap();
    }
}
