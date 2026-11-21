//! A pass snapshot belongs to the renderer the worker actually chooses.
//! Keep its original command position while that decision is pending.
use super::{Backend, Worker};

type Pixels = Result<Vec<u8>, String>;

enum Source {
    Target { proxy: u32, width: u32, height: u32 },
    Saved(Pixels),
}

pub(super) struct Read {
    source: Source,
    done: Box<dyn FnOnce(Pixels) + Send>,
}

impl Read {
    pub(super) fn new(
        proxy: u32,
        width: u32,
        height: u32,
        done: Box<dyn FnOnce(Pixels) + Send>,
    ) -> Self {
        Self {
            source: Source::Target {
                proxy,
                width,
                height,
            },
            done,
        }
    }

    /// A barrier before the first retained draw can alter or destroy the
    /// surface without disqualifying this frame. Save its pixels once.
    pub(super) fn preserve<B: Backend>(&mut self, worker: &mut Worker<B>) {
        if let Source::Target {
            proxy,
            width,
            height,
        } = self.source
        {
            let bytes = worker.surface(proxy).and_then(|surface| {
                crate::backend::read_frame_probe(&mut worker.inner, surface, width, height)
            });
            self.source = Source::Saved(bytes);
        }
    }

    pub(super) fn deliver<B: Backend>(self, worker: &mut Worker<B>) {
        match self.source {
            Source::Saved(result) => (self.done)(result),
            Source::Target {
                proxy,
                width,
                height,
            } => match worker.surface(proxy) {
                Ok(surface) => worker
                    .inner
                    .queue_frame_probe_read(surface, width, height, self.done),
                Err(e) => (self.done)(Err(e)),
            },
        }
    }
}
