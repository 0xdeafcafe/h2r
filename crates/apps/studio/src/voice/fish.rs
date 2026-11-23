//! Fish Audio's text to speech (https://docs.fish.audio, "Text to
//! Speech"): `POST https://api.fish.audio/v1/tts`, a bearer key, a JSON
//! body with the line and the voice's `reference_id`.
//!
//! Lines are asked for as `pcm` at 44.1 kHz: raw 16-bit mono, streamed
//! chunked, so there is no WAV header whose length a stream could get
//! wrong. The `model` header is sent only when a model is asked for; the
//! service's default stands otherwise. The docs state no rate limits: a
//! 429 or 503 is reported by status and not retried.

use std::io::Read;
use std::time::Duration;

use serde_json::json;

use super::key::{KeyError, KeySource, Secret};
use super::{Pcm, Request, VoiceError, VoiceSource, audio};

pub const ENDPOINT: &str = "https://api.fish.audio/v1/tts";

/// Most bytes of audio read back (a 500-character line is a few MiB).
const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The client: the service behind [`VoiceSource`].
pub struct Fish {
    keys: Box<dyn KeySource>,
    endpoint: String,
}

impl Fish {
    pub fn new(keys: Box<dyn KeySource>) -> Fish {
        Fish {
            keys,
            endpoint: ENDPOINT.to_owned(),
        }
    }
}

/// The JSON body of a request.
pub fn body(req: &Request) -> serde_json::Value {
    let mut b = json!({
        "text": req.text,
        "reference_id": req.voice,
        "format": "pcm",
        "sample_rate": audio::RATE,
        "latency": "normal",
        "normalize": true,
    });
    if (req.speed - 1.0).abs() > f32::EPSILON {
        b["prosody"] = json!({"speed": req.speed});
    }
    b
}

fn key_error(e: KeyError) -> VoiceError {
    match e {
        KeyError::Missing => VoiceError::NoKey,
        KeyError::Unreadable(s) => VoiceError::Key(s),
    }
}

/// The service's own error text (its JSON `message`), short, with the key
/// taken out.
fn api_message(key: &Secret, body: &str) -> String {
    let text = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| body.to_owned());
    let text: String = key.scrub(&text).chars().take(200).collect();
    text.trim().to_owned()
}

impl VoiceSource for Fish {
    fn say(&mut self, req: &Request) -> Result<Pcm, VoiceError> {
        let key = self.keys.key().map_err(key_error)?;
        let mut call = ureq::post(&self.endpoint)
            .timeout(Duration::from_secs(90))
            .set("Authorization", &format!("Bearer {}", key.expose()))
            .set("Content-Type", "application/json");
        if let Some(m) = &req.model {
            call = call.set("model", m);
        }
        let resp = match call.send_string(&body(req).to_string()) {
            Ok(r) => r,
            Err(ureq::Error::Status(status, r)) => {
                let text = r.into_string().unwrap_or_default();
                return Err(VoiceError::Api {
                    status,
                    message: api_message(&key, &text),
                });
            }
            Err(ureq::Error::Transport(t)) => {
                return Err(VoiceError::Network(
                    key.scrub(&format!("Fish Audio could not be reached: {t}")),
                ));
            }
        };
        let mut bytes = Vec::new();
        resp.into_reader()
            .take(MAX_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|e| VoiceError::Network(format!("the audio stopped coming: {e}")))?;
        audio::from_response(&bytes)
    }

    fn ready(&mut self) -> Result<(), VoiceError> {
        match self.keys.present() {
            Ok(true) => Ok(()),
            Ok(false) => Err(VoiceError::NoKey),
            Err(e) => Err(key_error(e)),
        }
    }

    fn name(&self) -> &'static str {
        "Fish Audio"
    }
}
