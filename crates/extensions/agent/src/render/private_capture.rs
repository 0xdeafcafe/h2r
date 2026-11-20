//! A lighting override draws an immutable scene a second time. Its PNG,
//! tick and hash must all come from that scene, even if more ticks end
//! while the renderer is working.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak, mpsc};

use agent_proto::{CaptureLighting, CaptureReply, RpcError, code};

use super::FrameCapture;

/// A host-owned, sealed frame and its retained render inputs. Drawing
/// enqueues a frame on the render thread and returns without waiting.
/// The worker resolves the request after drawing its pixels.
pub trait PrivateCaptureScene: Send + Sync {
    fn tick(&self) -> u64;
    fn present(&self) -> u64;
    fn scene_hash(&self) -> u64;
    /// The displayed extent of this Swap may still be on its way from
    /// the display worker. Keep the selection while waiting for it.
    fn ready(&self) -> bool;
    fn draw(&self, lighting: CaptureLighting, output: PrivateCaptureWrite) -> Result<(), String>;
}

/// Select the latest sealed scene, or exactly the requested scene tick.
/// Return `Ok(None)` while it can still arrive, or an error if it was
/// skipped, has expired, or the renderer cannot reproduce the frame.
pub type PrivateCaptureSource =
    fn(Option<u64>) -> Result<Option<Arc<dyn PrivateCaptureScene>>, RpcError>;

/// The worker can skip a capture after disconnection, timeout, another
/// request, or a renderer reset. A file is retained only for its reply.
pub struct PrivateCaptureWrite {
    waiting: Weak<()>,
    path: PathBuf,
    done: mpsc::Sender<Result<Written, String>>,
}

impl PrivateCaptureWrite {
    pub fn waiting(&self) -> bool {
        self.waiting.strong_count() != 0
    }

    pub fn finish(self, result: Result<crate::compare::Rgba, String>) {
        if !self.waiting() {
            return;
        }
        let result = result.and_then(|image| {
            let file = Written(Some(self.path));
            crate::compare::write_png(file.0.as_deref().expect("capture path"), &image)
                .map_err(|e| e.to_string())?;
            Ok(file)
        });
        let _ = self.done.send(result);
    }
}

// If the game discards a request while PNG encoding is finishing, the
// last owner of the unclaimed file removes it, on either thread.
struct Written(Option<PathBuf>);

impl Drop for Written {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub(super) struct Lease {
    capture: &'static FrameCapture,
    seq: u64,
}

impl Lease {
    pub(super) fn new(capture: &'static FrameCapture, seq: u64) -> Self {
        Self { capture, seq }
    }
    pub(super) fn finish(&self) -> Option<Result<CaptureReply, RpcError>> {
        self.capture.finish(self.seq)
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.capture.lock();
        if state
            .private
            .active
            .as_ref()
            .is_some_and(|(seq, _)| *seq == self.seq)
        {
            state.private.active = None;
        }
    }
}

/// Register the native frame owner's capture source. Unregister on
/// teardown. Registering again also discards captures from a prior map
/// or snapshot timeline.
pub fn set_private_capture_source(source: Option<PrivateCaptureSource>) {
    super::private_graph::reset();
    super::CAPTURE.set_private_source(source);
}

/// Whether the host has a native renderer registered for private draws.
pub fn private_capture_available() -> bool {
    super::CAPTURE.lock().private.source.is_some()
}

pub(super) struct PrivateState {
    source: Option<PrivateCaptureSource>,
    /// The previous completed capture. An explicit request for its tick
    /// can compare two modes after that scene leaves the renderer's queue.
    pinned: Option<Arc<dyn PrivateCaptureScene>>,
    pub(super) active: Option<(u64, Arc<Mutex<Job>>)>,
}

impl PrivateState {
    pub(super) const fn new() -> Self {
        Self {
            source: None,
            pinned: None,
            active: None,
        }
    }
}

pub(super) struct Job {
    source: PrivateCaptureSource,
    tick: Option<u64>,
    lighting: CaptureLighting,
    scene: Option<Arc<dyn PrivateCaptureScene>>,
    waiting: Arc<()>,
    writing: Option<mpsc::Receiver<Result<Written, String>>>,
    path: PathBuf,
}

impl Job {
    fn poll(&mut self) -> Result<Option<(CaptureReply, Written)>, RpcError> {
        if self.scene.is_none() {
            self.scene = (self.source)(self.tick)?;
        }
        let Some(scene) = &self.scene else {
            return Ok(None);
        };
        if let Some(tick) = self.tick
            && tick != scene.tick()
        {
            return Err(unavailable(format!(
                "requested scene tick {tick}, renderer supplied {}",
                scene.tick()
            )));
        }
        if self.writing.is_none() {
            if !scene.ready() {
                return Ok(None);
            }
            let (done, received) = mpsc::channel();
            scene
                .draw(
                    self.lighting,
                    PrivateCaptureWrite {
                        waiting: Arc::downgrade(&self.waiting),
                        path: self.path.clone(),
                        done,
                    },
                )
                .map_err(unavailable)?;
            self.writing = Some(received);
        }
        let writing = self.writing.as_ref().expect("draw queued");
        let file = match writing.try_recv() {
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(unavailable("private capture renderer stopped"));
            }
            Ok(result) => result.map_err(unavailable)?,
        };
        Ok(Some((
            CaptureReply {
                path: self.path.display().to_string(),
                tick: scene.tick(),
                present: scene.present(),
                pass: None,
                lighting: Some(self.lighting),
                scene_hash: Some(format!("{:016x}", scene.scene_hash())),
            },
            file,
        )))
    }
}

fn unavailable(message: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, message)
}

impl FrameCapture {
    pub(super) fn set_private_source(&self, source: Option<PrivateCaptureSource>) {
        let mut s = self.lock();
        s.private = PrivateState {
            source,
            ..PrivateState::new()
        };
    }

    pub(super) fn arm_private(
        &self,
        lighting: CaptureLighting,
        tick: Option<u64>,
        dir: &Path,
    ) -> Result<u64, RpcError> {
        if tick.is_some_and(|t| t > u64::from(u32::MAX)) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "a native scene tick must fit in 32 bits",
            ));
        }
        let mut s = self.lock();
        let source = s
            .private
            .source
            .ok_or_else(|| unavailable("no private-target scene renderer registered"))?;
        let scene = s
            .private
            .pinned
            .as_ref()
            .filter(|scene| Some(scene.tick()) == tick)
            .cloned();
        let seq = s.next_seq;
        s.next_seq += 1;
        let path = dir.join(format!("capture-{}-{seq}-scene.png", std::process::id()));
        let job = Job {
            source,
            tick,
            lighting,
            scene,
            waiting: Arc::new(()),
            writing: None,
            path,
        };
        s.private.active = Some((seq, Arc::new(Mutex::new(job))));
        s.job = None;
        self.pending
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.want.store(0, std::sync::atomic::Ordering::Relaxed);
        Ok(seq)
    }

    pub(super) fn finish_private(
        &self,
        seq: u64,
        job: Arc<Mutex<Job>>,
    ) -> Option<Result<CaptureReply, RpcError>> {
        // The host callback may use a different mutex. Don't call it with
        // the capture state locked; a source reset can interrupt readback.
        let mut job = job.lock().unwrap_or_else(|e| e.into_inner());
        let result = match job.poll() {
            Ok(None) => return None,
            Ok(Some(reply)) => Ok(reply),
            Err(error) => Err(error),
        };
        let mut s = self.lock();
        if s.private
            .active
            .as_ref()
            .is_none_or(|(active, _)| *active != seq)
        {
            return Some(Err(super::capture_interrupted()));
        }
        s.private.active = None;
        Some(result.map(|(reply, mut file)| {
            s.private.pinned.clone_from(&job.scene);
            file.0.take();
            reply
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{Rgba, compare_files};
    use agent_proto::{CompareParams, FrameCaptureParams};
    use std::sync::Mutex;

    // One installed source for this test. It serves only the latest
    // synthetic scene: the second RPC has to use the first one's pin.
    struct Scene {
        tick: u64,
        draws: Mutex<Vec<CaptureLighting>>,
    }

    impl PrivateCaptureScene for Scene {
        fn tick(&self) -> u64 {
            self.tick
        }
        fn present(&self) -> u64 {
            self.tick + 9
        }
        fn scene_hash(&self) -> u64 {
            self.tick + 3
        }

        fn ready(&self) -> bool {
            true
        }
        fn draw(
            &self,
            lighting: CaptureLighting,
            output: PrivateCaptureWrite,
        ) -> Result<(), String> {
            self.draws.lock().unwrap().push(lighting);
            let mut image = Rgba::filled(4, 3, [1, 2, 3, 255]);
            image.set(1, 2, [self.tick as u8, 33, 22, 255]);
            output.finish(Ok(image));
            Ok(())
        }
    }

    #[test]
    fn abandoned_requests_skip_work_and_release_unclaimed_pngs() {
        let dir = std::env::temp_dir().join(format!("h2-agent-abandoned-{}", std::process::id()));
        let path = dir.join("scene.png");
        let (done, result) = mpsc::channel();
        let waiting = Arc::new(());
        let late = PrivateCaptureWrite {
            waiting: Arc::downgrade(&waiting),
            path: path.clone(),
            done,
        };
        drop(waiting);
        assert!(!late.waiting());
        late.finish(Ok(Rgba::filled(2, 1, [0; 4])));
        assert!(
            !path.exists(),
            "a cancelled request never starts PNG encoding"
        );
        assert!(result.try_recv().is_err());

        let (done, result) = mpsc::channel();
        let waiting = Arc::new(());
        let early = PrivateCaptureWrite {
            waiting: Arc::downgrade(&waiting),
            path: path.clone(),
            done,
        };
        early.finish(Ok(Rgba::filled(2, 1, [0; 4])));
        assert!(path.exists());
        drop(result);
        assert!(
            !path.exists(),
            "disconnect between encoding and reply removes the file"
        );
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn dropping_a_deferred_request_cancels_only_that_private_capture() {
        let c = Box::leak(Box::new(FrameCapture::new(
            &super::super::PROBE,
            &super::super::HOSTED,
        )));
        c.set_private_source(Some(|_| Ok(None)));
        let p = FrameCaptureParams {
            lighting: Some(CaptureLighting::E3),
            ..Default::default()
        };
        let dir = Path::new("unused");
        let a = Lease::new(c, c.arm(&p, dir).unwrap());
        let b = Lease::new(c, c.arm(&p, dir).unwrap());
        drop(a);
        assert!(b.finish().is_none());
        let seq = b.seq;
        drop(b);
        assert!(c.lock().private.active.is_none());
        assert_eq!(c.finish(seq).unwrap().unwrap_err().code, code::INTERRUPTED);
    }

    #[test]
    fn two_private_captures_draw_the_same_pinned_scene_with_each_requested_mode() {
        static LATEST: Mutex<Option<Arc<Scene>>> = Mutex::new(None);
        fn select(tick: Option<u64>) -> Result<Option<Arc<dyn PrivateCaptureScene>>, RpcError> {
            let latest = LATEST.lock().unwrap().as_ref().unwrap().clone();
            if tick.is_some_and(|t| t != latest.tick) {
                return Err(unavailable("scene no longer available"));
            }
            Ok(Some(latest))
        }

        let first = Arc::new(Scene {
            tick: 42,
            draws: Mutex::new(vec![]),
        });
        *LATEST.lock().unwrap() = Some(Arc::clone(&first));
        let c = FrameCapture::new(&super::super::PROBE, &super::super::HOSTED);
        c.set_private_source(Some(select));
        let dir =
            std::env::temp_dir().join(format!("h2-agent-lighting-pair-{}", std::process::id()));
        let capture = |lighting, tick| {
            let p = FrameCaptureParams {
                tick,
                lighting: Some(lighting),
                pass: None,
            };
            let seq = c.arm(&p, &dir).unwrap();
            c.finish(seq).unwrap().unwrap()
        };

        let a = capture(CaptureLighting::Retail, None);
        *LATEST.lock().unwrap() = Some(Arc::new(Scene {
            tick: 43,
            draws: Mutex::new(vec![]),
        }));
        let b = capture(CaptureLighting::E3, Some(a.tick));
        assert_eq!(
            (a.tick, a.present, a.scene_hash.as_deref()),
            (42, 51, Some("000000000000002d"))
        );
        assert_eq!(
            (a.tick, a.present, a.scene_hash),
            (b.tick, b.present, b.scene_hash)
        );
        assert_ne!(a.path, b.path);
        assert_eq!(
            (a.lighting, b.lighting),
            (Some(CaptureLighting::Retail), Some(CaptureLighting::E3))
        );
        assert_eq!(
            *first.draws.lock().unwrap(),
            [CaptureLighting::Retail, CaptureLighting::E3]
        );
        let compared = compare_files(&CompareParams {
            a: a.path.clone(),
            b: b.path.clone(),
            diff: None,
        })
        .unwrap();
        assert_eq!(compared.max_channel, 0);
        assert_eq!(compared.size, [4, 3]);

        c.set_private_source(Some(select));
        let old = FrameCaptureParams {
            tick: Some(42),
            lighting: Some(CaptureLighting::E3),
            pass: None,
        };
        let seq = c.arm(&old, &dir).unwrap();
        assert_eq!(
            c.finish(seq).unwrap().unwrap_err().code,
            code::UNAVAILABLE,
            "reset retired the pin"
        );
        std::fs::remove_file(a.path).unwrap();
        std::fs::remove_file(b.path).unwrap();
        std::fs::remove_dir(dir).unwrap();
        *LATEST.lock().unwrap() = None;
    }
}
