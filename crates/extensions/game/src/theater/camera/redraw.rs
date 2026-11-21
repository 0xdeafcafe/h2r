//! The displayed frame lives in the viewer's frame sink. Leave it there
//! while the player returns to the held tick through a recorded barrier.
use super::super::{Mode, Session, State};
use super::FilmCamera;
use anyhow::{Result, ensure};
use std::sync::{Arc, mpsc};

#[derive(Clone, Copy)]
pub(in super::super) enum PausedRedraw {
    Drawing { tick: u64 },
    Returning { tick: u64, revision: u64 },
}

impl PausedRedraw {
    fn tick(self) -> u64 {
        match self {
            Self::Drawing { tick } | Self::Returning { tick, .. } => tick,
        }
    }
}

/// The render thread sees only whether to publish the guest's next frame.
#[derive(Clone, Copy, Default)]
pub(super) enum Publish {
    #[default]
    Paced,
    Next,
    Sampling {
        revision: u64,
    },
    Held {
        revision: u64,
    },
}

type Pixels = ((u32, u32), Vec<u8>);
type Deliver = Box<dyn FnOnce(Result<Pixels>) + Send>;

pub(super) struct RasterJob {
    size: (u32, u32),
    ticket: Arc<()>,
    /// The scale changes *after* presenting a frame. A first present at
    /// the old size primes it; the same held tick is drawn again.
    primed: bool,
    result: Option<Result<Pixels>>,
    deliver: Option<Deliver>,
}

impl Drop for RasterJob {
    fn drop(&mut self) {
        if let Some(deliver) = self.deliver.take() {
            deliver(Err(anyhow::anyhow!(
                "guest redraw cancelled before returning to the pause"
            )));
        }
    }
}

impl Session {
    /// Queue one scaled, guest-rendered picture at a held tick. The converter
    /// packages the actual viewport dimensions and tightly packed RGBA8 for
    /// the caller. Completion follows the verified return to the held tick.
    pub fn queue_guest_redraw<T: Send + 'static>(
        &self,
        size: (u32, u32),
        tick: u64,
        convert: impl FnOnce((u32, u32), Vec<u8>) -> T + Send + 'static,
    ) -> Result<mpsc::Receiver<Result<T>>> {
        let mut s = self.lock();
        ensure!(
            s.paused
                && s.tick == tick
                && s.seeking.is_none()
                && s.restart.is_none()
                && !s.complete
                && s.failure.is_none(),
            "guest redraw requires the current paused tick"
        );
        ensure!(s.camera_redraw.is_none(), "a guest redraw is in progress");
        ensure!(
            s.can_redraw_from_ring(tick),
            "no warm return point for paused redraw"
        );
        let Mode::Replay(_, replay) = &s.mode else {
            anyhow::bail!("redraw requires film playback")
        };
        let mut v = replay.camera.0.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            v.guest_view_limit > 0,
            "the viewer does not use guest presents"
        );
        ensure!(
            size.0 > 0 && size.1 > 0 && size.0.max(size.1) <= v.guest_view_limit,
            "requested raster exceeds the viewer's dimensions"
        );
        let (tx, rx) = mpsc::channel();
        v.raster_job = Some(RasterJob {
            size,
            ticket: Arc::new(()),
            primed: false,
            result: None,
            deliver: Some(Box::new(move |result| {
                let _ = tx.send(result.map(|(size, rgba)| convert(size, rgba)));
            })),
        });
        v.publish = Publish::Next;
        drop(v);
        s.camera_redraw = Some(PausedRedraw::Drawing { tick });
        Ok(rx)
    }
}

impl FilmCamera {
    /// An independent observer redraws from simulation state while the
    /// machine stays still. Only a guest-drawn window needs this handoff.
    pub fn attach_guest_view(&self, max_dimension: u32) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .guest_view_limit = max_dimension.min(settings::Resolution::MAX_DIMENSION);
    }

    /// This temporary override is read by the display backend at a present.
    /// It never changes the user's resolution setting or guest reference raster.
    pub fn guest_raster(&self, normal: (u32, u32)) -> (u32, u32) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .raster_job
            .as_ref()
            .filter(|job| job.result.is_none())
            .map_or(normal, |job| job.size)
    }

    /// Called on each viewer swap, even when normal playback decimates it.
    pub fn publish_guest_present(&self, due: bool, size: Option<(u32, u32)>) -> bool {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match v.publish {
            Publish::Paced => due,
            Publish::Next => {
                let revision = v.revision;
                if let Some(job) = &mut v.raster_job {
                    if Some(job.size) == size {
                        v.publish = Publish::Sampling { revision };
                        return true;
                    }
                    if job.primed {
                        job.result = Some(Err(anyhow::anyhow!(
                            "requested raster {:?}, guest viewer presented {size:?}",
                            job.size
                        )));
                    }
                    job.primed = true;
                    v.publish = Publish::Held { revision };
                    return false;
                }
                v.publish = Publish::Held { revision };
                true
            }
            Publish::Sampling { .. } | Publish::Held { .. } => false,
        }
    }

    /// The sink reads its exact submitted slot only when it is sampled.
    pub fn sample_guest_present(&self, read: impl FnOnce() -> Result<((u32, u32), Vec<u8>)>) {
        let (ticket, revision) = {
            let v = self.0.lock().unwrap_or_else(|e| e.into_inner());
            let (Publish::Sampling { revision }, Some(job)) = (v.publish, &v.raster_job) else {
                return;
            };
            (job.ticket.clone(), revision)
        };
        let pixels = read();
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = &mut v.raster_job
            && Arc::ptr_eq(&job.ticket, &ticket)
        {
            job.result = Some(pixels);
            v.publish = Publish::Held { revision };
        }
    }

    fn start_redraw(&self) -> bool {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if v.guest_view_limit > 0 {
            v.publish = Publish::Next;
        }
        v.guest_view_limit > 0
    }
}

impl State {
    pub(in super::super) fn request_camera_redraw(&mut self) {
        if self.paused
            && self.restart.is_none()
            && self.seeking.is_none()
            && self.camera_redraw.is_none()
            && self.can_redraw_from_ring(self.tick)
            && let Mode::Replay(_, replay) = &self.mode
            && replay.camera.start_redraw()
        {
            self.camera_redraw = Some(PausedRedraw::Drawing { tick: self.tick });
        }
    }

    pub(in super::super) fn cancel_camera_redraw(&mut self) {
        if self.camera_redraw.take().is_some()
            && let Mode::Replay(_, replay) = &self.mode
        {
            let mut v = replay.camera.0.lock().unwrap_or_else(|e| e.into_inner());
            v.publish = Publish::Paced;
            v.raster_job.take();
        }
    }

    /// Keep the viewer's time at the pause, including while replaying back to it.
    pub(in super::super) fn visible_tick(&self) -> u64 {
        self.camera_redraw.map_or(self.tick, PausedRedraw::tick)
    }

    pub(in super::super) fn visible_pause(&self) -> bool {
        if self.camera_redraw.is_some()
            && let Some((_, hold)) = self.warm_check
        {
            return hold;
        }
        self.paused
    }

    /// Changing transport during a preview changes what happens *at* the
    /// held tick. The replay back to that tick must keep advancing.
    pub(in super::super) fn set_transport_pause(&mut self, paused: bool) {
        if let Some(PausedRedraw::Returning { tick, .. }) = self.camera_redraw
            && self.seeking == Some(tick)
        {
            if let Mode::Replay(_, replay) = &mut self.mode {
                replay.paused_at_target = paused;
            }
            if let Some((_, hold)) = &mut self.warm_check {
                *hold = paused;
            }
        } else {
            self.paused = paused;
        }
    }

    /// Tick hooks may finish their slice, even after publishing the picture.
    /// Preempting on a viewer swap changes later film boundaries.
    pub(in super::super) fn running_camera_redraw(&self) -> bool {
        matches!(self.camera_redraw, Some(PausedRedraw::Drawing { .. }))
    }

    /// Reaching the held tick can block inside a hook, before the next slice.
    /// If the user moved again during the return, draw the latest revision.
    pub(in super::super) fn finish_camera_return(&mut self) {
        let Some(PausedRedraw::Returning { tick, revision }) = self.camera_redraw else {
            return;
        };
        if self.restart.is_some() || self.seeking.is_some() || self.tick != tick {
            return;
        }
        let Mode::Replay(_, replay) = &self.mode else {
            return;
        };
        let mut v = replay.camera.0.lock().unwrap_or_else(|e| e.into_inner());
        let prime = v
            .raster_job
            .as_ref()
            .is_some_and(|job| job.result.is_none());
        if self.paused && (prime || v.revision != revision) && self.can_redraw_from_ring(tick) {
            // A camera control during a screenshot's return requests a new
            // ordinary frame after this picture has been delivered.
            self.camera_redraw = Some(PausedRedraw::Drawing { tick });
            v.publish = Publish::Next;
        } else {
            self.camera_redraw = None;
            v.publish = Publish::Paced;
        }
        if !prime || !self.paused || !self.can_redraw_from_ring(tick) {
            let job = v.raster_job.take();
            drop(v);
            if let Some(mut job) = job
                && let Some(result) = job.result.take()
                && let Some(deliver) = job.deliver.take()
            {
                deliver(result);
            }
        }
    }

    /// End a preview only at a natural scheduler boundary. End-of-film
    /// without another present also returns to the pause.
    pub(in super::super) fn redraw_boundary(&mut self) -> bool {
        self.finish_camera_return();
        let Some(PausedRedraw::Drawing { tick }) = self.camera_redraw else {
            return false;
        };
        let Mode::Replay(_, replay) = &self.mode else {
            return false;
        };
        let mut v = replay.camera.0.lock().unwrap_or_else(|e| e.into_inner());
        let revision = match v.publish {
            Publish::Next | Publish::Sampling { .. } if !self.complete && self.paused => {
                return true;
            }
            Publish::Held { revision } => revision,
            _ => {
                if let Some(job) = &mut v.raster_job {
                    job.result = Some(Err(anyhow::anyhow!(
                        "redraw ended before the requested present"
                    )));
                }
                v.revision
            }
        };
        v.publish = Publish::Held { revision };
        self.camera_redraw = Some(PausedRedraw::Returning { tick, revision });
        self.restart.get_or_insert(tick);
        self.complete = false;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_redraw_publishes_one_present_even_between_paced_frames() {
        let camera = FilmCamera::default();
        assert!(
            !camera.start_redraw(),
            "the independent view needs no guest slice"
        );
        assert!(!camera.publish_guest_present(false, None));
        camera.attach_guest_view(4096);
        assert!(camera.start_redraw());
        assert!(camera.publish_guest_present(false, None));
        assert!(
            !camera.publish_guest_present(true, None),
            "keep this frame during the return"
        );
    }
}
