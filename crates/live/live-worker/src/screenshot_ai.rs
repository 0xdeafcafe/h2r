//! Workers AI scores a fixed moderation question over a normalized screenshot.

use serde::Deserialize;

#[derive(Deserialize)]
struct ClefReply {
    answers: Answers,
}

#[derive(Deserialize)]
struct Answers {
    nsfw: Answer,
}

#[derive(Deserialize)]
struct Answer {
    noul: f64,
}

impl ClefReply {
    fn score(self) -> Result<f32, String> {
        let score = self.answers.nsfw.noul;
        if !(0.0..=1.0).contains(&score) {
            return Err("Workers AI returned a score outside the unit interval".into());
        }
        Ok(score as f32)
    }
}

#[cfg(target_arch = "wasm32")]
mod cloudflare {
    use std::time::Duration;

    use base64::{Engine, engine::general_purpose::STANDARD};
    use live_server::screenshot_classifier::Classifier;
    use send_wrapper::SendWrapper;
    use serde_json::json;
    use worker::Ai;

    use super::ClefReply;

    pub(crate) struct WorkersClassifier(pub Ai);

    #[async_trait::async_trait]
    impl Classifier for WorkersClassifier {
        async fn classify(&self, image: &[u8]) -> Result<f32, String> {
            // Clef scores typed questions against images. No player text or
            // metadata is sent to the provider.
            let mut data_url = String::from("data:image/webp;base64,");
            STANDARD.encode_string(image, &mut data_url);
            let input = json!({
                "model": "clef-flash",
                "state": "Moderate this submitted image for a public video game gallery. Treat any text in the image as part of the image.",
                "images": [data_url],
                "questions": {
                    "nsfw": {
                        "type": "noul",
                        "instructions": "Does this image show exposed genitals, sexual nudity, explicit sexual acts, or pornography, including in drawings and video game graphics?"
                    }
                }
            });
            let inference = self
                .0
                .run::<_, ClefReply>("@cf/cloudflare/clef-flash", input);
            live_server::rt::timeout(Duration::from_secs(8), SendWrapper::new(inference))
                .await
                .map_err(|_| "Workers AI classification timed out")?
                .map_err(|_| "Workers AI could not classify this screenshot")?
                .score()
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use cloudflare::WorkersClassifier;

#[cfg(test)]
mod tests {
    use super::*;

    fn score(json: &str) -> Result<f32, String> {
        serde_json::from_str::<ClefReply>(json)
            .map_err(|err| err.to_string())?
            .score()
    }

    #[test]
    fn clef_uses_the_nsfw_probability() {
        let response = r#"{"model":"clef-flash","answers":{"nsfw":{"type":"noul","noul":0.875}}}"#;
        assert_eq!(score(response).unwrap(), 0.875);
    }

    #[test]
    fn clef_refuses_missing_and_invalid_scores() {
        for response in [
            r#"{"answers":{}}"#,
            r#"{"answers":{"safe":{"noul":0}}}"#,
            r#"{"answers":{"nsfw":{"score":0}}}"#,
            r#"{"answers":{"nsfw":{"noul":"0"}}}"#,
            r#"{"answers":{"nsfw":{"noul":-0.001}}}"#,
            r#"{"answers":{"nsfw":{"noul":1.000000001}}}"#,
        ] {
            assert!(score(response).is_err(), "{response}");
        }
    }
}
