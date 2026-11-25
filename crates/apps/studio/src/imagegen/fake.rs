//! A source that draws its own picture: for tests, and for trying the
//! flow with no generator (`H2_IMAGEGEN=fake`).
//!
//! The picture is a disc on a transparent ground, its colour from a hash
//! of the prompt, so the same prompt always makes the same picture. It
//! records every request. [`FakeSource::failing`] answers with an error
//! instead, and [`FakeSource::gated`] holds each generation until the test
//! lets it go, so a test sees a job running.

use std::sync::{Arc, Mutex, mpsc};

use super::{GenError, GenRequest, Generated, ImageSource};

/// The fake picture's side.
pub const SIDE: u32 = 64;

#[derive(Default)]
pub struct FakeSource {
    /// Every request, oldest first.
    pub requests: Arc<Mutex<Vec<GenRequest>>>,
    fail: Option<GenError>,
    gate: Option<Mutex<mpsc::Receiver<()>>>,
}

impl FakeSource {
    pub fn new() -> Self {
        FakeSource::default()
    }

    /// A source whose every generation fails with `e`.
    pub fn failing(e: GenError) -> Self {
        FakeSource {
            fail: Some(e),
            ..FakeSource::default()
        }
    }

    /// A source whose generations each wait for a send on the returned
    /// sender (or for it to drop).
    pub fn gated() -> (Self, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel();
        (
            FakeSource {
                gate: Some(Mutex::new(rx)),
                ..FakeSource::default()
            },
            tx,
        )
    }

    /// The picture made for `prompt`: `SIDE` by `SIDE` RGBA.
    pub fn picture(prompt: &str) -> Vec<u8> {
        let h = prompt
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        let colour = [(h >> 8) as u8 | 0x40, (h >> 16) as u8 | 0x40, (h >> 24) as u8 | 0x40];
        let c = SIDE as f32 / 2.0;
        let r = SIDE as f32 * 0.4;
        let mut out = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
                if d <= r {
                    out.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
                } else {
                    out.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        out
    }
}

impl ImageSource for FakeSource {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn generate(&self, req: &GenRequest) -> Result<Generated, GenError> {
        self.requests
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(req.clone());
        if let Some(gate) = &self.gate {
            let _ = gate.lock().unwrap_or_else(|e| e.into_inner()).recv();
        }
        if let Some(e) = &self.fail {
            return Err(e.clone());
        }
        for r in &req.references {
            if !r.is_file() {
                return Err(GenError::Failed(format!(
                    "reference {} is not a file",
                    r.display()
                )));
            }
        }
        let png = bitmap_edit::encode_png(SIDE, SIDE, &Self::picture(&req.prompt))
            .map_err(|e| GenError::Failed(e.to_string()))?;
        let path = req.work_dir.join("fake.png");
        std::fs::write(&path, png).map_err(|e| GenError::Failed(format!("{}: {e}", path.display())))?;
        Ok(Generated {
            png: path,
            generator: "fake".into(),
            revised_prompt: None,
        })
    }
}
