//! Captures host overlays on the render owner for the image it draws.
#![cfg(feature = "gpu")]

use std::sync::Arc;

use agent::editor::EditorLink;
use guest_layout::GuestMemory;
use gfx::world_lines::ImageSamples;
pub use gfx::world_lines::{ImageMarks, ImageValidity, LineStyle, WorldLine};
use render_bridge::scene_step::{self, PresentObserver, PresentPayload};

use crate::editor_glue;

/// A host producer of world-space geometry for the window overlay.
pub trait WorldLineSource: Send + Sync {
    /// Cheap enough to poll at render entry: one atomic load.
    fn active(&self) -> bool;

    /// Called only when active, on the serial render owner. Read the supplied guest and shared
    /// host data. Write only to the sample being built, which owns its
    /// geometry and can outlive this render on the window thread.
    fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>>;
}

struct CaptureLines {
    editor: Option<EditorLink>,
    sources: Vec<Arc<dyn WorldLineSource>>,
    /// The display's horizontal projection widening over the guest's.
    horizontal_factor: Arc<dyn Fn() -> f32 + Send + Sync>,
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
        (self.horizontal_factor)()
    }
}

/// The marks image the frame step attaches to the present it seals.
pub fn take_mark_image() -> Option<gfx::present_camera::Image> {
    let image = scene_step::take_present_image()?;
    Some(gfx::present_camera::Image {
        views: image.views.map(|view| {
            let view = view?;
            Some(gfx::present_camera::View {
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
    horizontal_factor: Arc<dyn Fn() -> f32 + Send + Sync>,
) -> Arc<dyn PresentObserver> {
    Arc::new(CaptureLines { editor, sources, horizontal_factor })
}

/// `render.marks` (docs/runtime/agent-api.md, "render.marks"): where the
/// window drew the world marks on its last presents. The first call arms
/// the window's probe ([`gfx::world_lines::probe`]); every call returns
/// the presents kept since.
pub fn add_marks_method(agent: &agent::Agent) {
    use agent_proto::{RpcError, code, method};
    agent.add_method(method::RENDER_MARKS, agent::Access::Read, |v| {
        if !(v.is_null() || v.as_object().is_some_and(|o| o.is_empty())) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "render.marks takes no params",
            ));
        }
        Ok(marks_json(
            &gfx::world_lines::probe::PRESENTED,
            std::time::Instant::now(),
        ))
    });
}

/// `render.marks`' answer from `probe` at `now`.
fn marks_json(
    probe: &gfx::world_lines::probe::Probe,
    now: std::time::Instant,
) -> serde_json::Value {
    let (presents, armed) = probe.read();
    let presents: Vec<serde_json::Value> = presents
        .iter()
        .map(|p| {
            serde_json::json!({
                "seq": p.seq,
                "age_ms": now.saturating_duration_since(p.at).as_secs_f64() * 1000.0,
                "frame": p.kind.name(),
                "fraction": p.fraction,
                "shifted": p.shifted,
                "anchors": p.anchors,
            })
        })
        .collect();
    serde_json::json!({"armed": armed, "presents": presents})
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
        let guest = guest_layout::FlatMemory::new(0, 0);
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
    fn render_marks_arms_the_probe_then_reports_each_present() {
        use gfx::world_lines::probe::{Kind, Presented, Probe};
        let probe = Probe::new();
        let now = Instant::now();
        let first = marks_json(&probe, now);
        assert_eq!(first, serde_json::json!({"armed": false, "presents": []}));
        probe.record(Presented {
            seq: 0,
            at: now,
            kind: Kind::Synthetic,
            fraction: Some(0.5),
            shifted: true,
            anchors: vec![Some([10.0, 20.0]), None],
        });
        let r = marks_json(&probe, now);
        assert_eq!(r["armed"], true);
        let p = &r["presents"][0];
        assert_eq!(p["frame"], "synthetic");
        assert_eq!(p["fraction"], 0.5);
        assert_eq!(p["shifted"], true);
        assert_eq!(p["anchors"], serde_json::json!([[10.0, 20.0], null]));
        assert_eq!(p["age_ms"], 0.0);
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
        let guest = guest_layout::FlatMemory::new(0, 0);
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
