//! Captures host overlays on the render owner for the image it draws.
#![cfg(feature = "gpu")]

use std::sync::Arc;

use agent::editor::EditorLink;
use engine::GuestMemory;
use gfx::world_lines::ImageSamples;
pub use gfx::world_lines::{ImageMarks, ImageValidity, LineStyle, WorldLine};
use render_bridge::scene_step::{self, PresentObserver, PresentPayload};

use crate::d3d_glue::DeviceSnapshot;
use crate::editor_glue;

/// A host producer of world-space geometry for the window overlay.
pub trait WorldLineSource: Send + Sync {
    /// Cheap enough to poll at render entry: one atomic load.
    fn active(&self) -> bool;

    /// Called only when active, on the serial render owner or the pipelined
    /// worker with its own guest view. Read the supplied guest and shared
    /// host data. Write only to the sample being built, which owns its
    /// geometry and can outlive this render on the window thread.
    fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>>;
}

struct CaptureLines {
    editor: Option<EditorLink>,
    sources: Vec<Arc<dyn WorldLineSource>>,
    device: Arc<dyn DeviceSnapshot>,
}

fn capture_samples(
    editor: Option<&EditorLink>,
    sources: &[Arc<dyn WorldLineSource>],
    guest: &dyn GuestMemory,
) -> Option<ImageSamples> {
    let editor = editor.and_then(|link| editor_glue::capture(link, guest));
    let mut captured = sources
        .iter()
        .filter(|source| source.active())
        .filter_map(|source| source.capture(guest));
    let first = captured.next();
    if editor.is_none() && first.is_none() {
        return None;
    }
    let mut samples = ImageSamples { editor, ..Default::default() };
    samples.sources.extend(first.into_iter().chain(captured));
    Some(samples)
}

impl PresentObserver for CaptureLines {
    fn capture(&self, guest: &dyn GuestMemory) -> Option<PresentPayload> {
        let samples = capture_samples(self.editor.as_ref(), &self.sources, guest)?;
        Some(Arc::new(samples))
    }

    fn horizontal_factor(&self) -> f32 {
        self.device.mark_projection_factor()
    }
}

fn take_mark_image() -> Option<d3d::present_camera::Image> {
    let image = scene_step::take_present_image()?;
    Some(d3d::present_camera::Image {
        views: image.views.map(|view| {
            let view = view?;
            Some(d3d::present_camera::View {
                camera: view.camera,
                horizontal_factor: view.horizontal_factor,
            })
        }),
        payload: image.payload,
    })
}

pub fn present_observer(
    editor: Option<EditorLink>,
    sources: Vec<Arc<dyn WorldLineSource>>,
    device: Arc<dyn DeviceSnapshot>,
) -> Arc<dyn PresentObserver> {
    device.set_mark_image_source(take_mark_image);
    Arc::new(CaptureLines {
        editor,
        sources,
        device,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    #[derive(Debug)]
    struct Line;

    impl ImageMarks for Line {
        fn lines(&self, visit: &mut dyn FnMut(WorldLine<'_>)) {
            visit(WorldLine {
                edges: &[[[1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]],
                anchor: [1.0, 0.0, 0.0],
                color: [1.0, 0.0, 0.0],
                dashed: false,
                label: None,
                style: Default::default(),
            });
        }
    }

    struct Source {
        enabled: AtomicBool,
        captures: AtomicUsize,
    }

    impl WorldLineSource for Source {
        fn active(&self) -> bool {
            self.enabled.load(Ordering::Relaxed)
        }

        fn capture(&self, _: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            self.captures.fetch_add(1, Ordering::Relaxed);
            Some(Arc::new(Line))
        }
    }

    #[test]
    fn editor_expiry_leaves_the_other_samples_on_the_same_image() {
        struct Game;

        impl agent::Reader for Game {
            fn status(&mut self) -> agent::GameStatus {
                agent::GameStatus::default()
            }

            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(vec![])
            }
        }

        let editor = EditorLink::default();
        let agent = agent::Agent::new();
        agent::editor::install_scene(&agent, Some(editor.clone()));
        let (send, recv) = std::sync::mpsc::sync_channel(1);
        let connection = agent.connect(send);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "editor.marks",
            "id": 1,
            "params": {"marks": [{"point": [2, 0, 0], "color": [1, 0, 0]}]}
        });
        agent.submit(connection, &request.to_string());
        agent.on_tick(&mut Game);
        let reply: serde_json::Value = serde_json::from_str(&recv.try_recv().unwrap()).unwrap();
        assert_eq!(reply["result"]["held"], 1);

        let sources: [Arc<dyn WorldLineSource>; 1] = [Arc::new(Source {
            enabled: AtomicBool::new(true),
            captures: AtomicUsize::new(0),
        })];
        let guest = engine::FlatMemory::new(0, 0);
        let image = capture_samples(Some(&editor), &sources, &guest).unwrap();
        let visit = |now| {
            let mut anchors = Vec::new();
            let shown = image.visit(&|| now, &mut |line| anchors.push(line.anchor));
            (anchors, shown)
        };
        let (anchors, shown) = visit(Instant::now());
        assert_eq!(anchors, [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        assert!(shown.editor_generation.is_some());

        let (anchors, shown) = visit(shown.expires.unwrap());
        assert_eq!(anchors, [[1.0, 0.0, 0.0]]);
        assert_eq!(shown.editor_generation, None);
        assert_eq!(shown.expires, None);
    }

    #[test]
    fn inactive_world_line_sources_skip_capture() {
        let first = Arc::new(Source {
            enabled: AtomicBool::new(true),
            captures: AtomicUsize::new(0),
        });
        let second = Arc::new(Source {
            enabled: AtomicBool::new(false),
            captures: AtomicUsize::new(0),
        });
        let sources: [Arc<dyn WorldLineSource>; 2] = [first.clone(), second.clone()];
        let guest = engine::FlatMemory::new(0, 0);
        assert!(capture_samples(None, &[], &guest).is_none());

        let image = capture_samples(None, &sources, &guest).unwrap();
        assert_eq!(image.sources.len(), 1);
        assert_eq!(first.captures.load(Ordering::Relaxed), 1);
        assert_eq!(second.captures.load(Ordering::Relaxed), 0);

        second.enabled.store(true, Ordering::Relaxed);
        let image = capture_samples(None, &sources, &guest).unwrap();
        assert_eq!(image.sources.len(), 2);
        assert!(!image.sources.spilled());

        first.enabled.store(false, Ordering::Relaxed);
        second.enabled.store(false, Ordering::Relaxed);
        assert!(capture_samples(None, &sources, &guest).is_none());
        assert_eq!(first.captures.load(Ordering::Relaxed), 2);
        assert_eq!(second.captures.load(Ordering::Relaxed), 1);
    }
}
