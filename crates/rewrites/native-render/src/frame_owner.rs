//! The render thread (docs/graphics/native-renderer.md, "How a frame
//! flows"). [`FrameOwner`] takes each sealed [`Frame`] from the
//! `frame_scene::mailbox`, draws it through the [`Renderer`] (or the blend
//! toward it the display-rate loop asks for) and presents it on the
//! window's [`gfx::Presenter`]. While nothing is queued it draws the
//! display frames between ticks ([`Display::idle`]). Captures (screenshots,
//! film thumbnails, `render.capture`) are drawn from the newest frame on
//! the same thread.

use std::sync::mpsc;
use std::time::Instant;

use frame_scene::mailbox::Receiver;
use gfx::present_camera::{self, Present};

use crate::frame_graph::display_rate::{Display, IdleTurn, Kept};
use crate::gpu::Image;
use crate::renderer::Renderer;
use crate::stage::Frame;

/// A capture of the newest frame at `size`, answered on `reply`; `None`
/// before the first frame.
pub struct Capture {
    pub size: (u32, u32),
    pub reply: mpsc::Sender<Option<Image>>,
}

pub struct FrameOwner {
    renderer: Renderer,
    presenter: gfx::Presenter,
    frames: Receiver<Frame>,
    display: Display,
    captures: mpsc::Receiver<Capture>,
    capture_tx: mpsc::Sender<Capture>,
    newest: Option<Kept>,
}

impl FrameOwner {
    pub fn new(renderer: Renderer, presenter: gfx::Presenter, frames: Receiver<Frame>, display: Display) -> Self {
        let (capture_tx, captures) = mpsc::channel();
        Self { renderer, presenter, frames, display, captures, capture_tx, newest: None }
    }

    /// Where other threads send captures.
    pub fn captures(&self) -> mpsc::Sender<Capture> {
        self.capture_tx.clone()
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    /// The window's new size.
    pub fn resize(&mut self, size: (u32, u32)) {
        self.presenter.resize(self.renderer.context(), size);
        self.renderer.resize(size);
    }

    /// Runs on its own thread until the frame step's publisher drops.
    pub fn spawn(self) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("render".into())
            .spawn(move || self.run())
            .unwrap_or_else(|e| panic!("frame owner: render thread: {e}"))
    }

    pub fn run(mut self) {
        while self.turn() {}
    }

    /// One turn: a queued frame first, else a due display frame, else a
    /// wait. `false` once the publisher has dropped and nothing is queued.
    pub fn turn(&mut self) -> bool {
        self.serve_captures();
        if let Some(frame) = self.frames.try_take() {
            self.swap(frame);
            return true;
        }
        match self.display.idle(&mut self.renderer, Instant::now()) {
            IdleTurn::Drawn(marks) => self.show(marks),
            IdleTurn::Wait(at) => std::thread::sleep(at.saturating_duration_since(Instant::now())),
            IdleTurn::Stop => match self.frames.take() {
                Some(frame) => self.swap(frame),
                None => return false,
            },
        }
        true
    }

    fn swap(&mut self, frame: Frame) {
        let kept = Kept::new(frame);
        self.newest = Some(kept.clone());
        let marks = self.display.at_swap(&mut self.renderer, kept);
        self.show(marks);
    }

    fn show(&mut self, marks: Present) {
        let (renderer, presenter) = (&mut self.renderer, &mut self.presenter);
        present_camera::with(Some(marks), || renderer.present(presenter));
    }

    fn serve_captures(&mut self) {
        while let Ok(Capture { size, reply }) = self.captures.try_recv() {
            let image = self.newest.as_ref().map(|kept| {
                let mut frame = kept.frame.lock().unwrap_or_else(|e| e.into_inner());
                self.renderer.capture(&mut frame, size)
            });
            let _ = reply.send(image);
        }
    }
}
