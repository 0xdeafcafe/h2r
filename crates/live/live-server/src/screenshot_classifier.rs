//! Scores normalized screenshots for the gallery's moderation decision.

use async_trait::async_trait;

/// A classifier for the visual content of a screenshot. The input is the exact
/// normalized WebP that Live would display. Return a score in `0.0..=1.0` for
/// sexually explicit content, or an error if there is no reliable result.
#[async_trait]
pub trait Classifier: Send + Sync {
    async fn classify(&self, image: &[u8]) -> Result<f32, String>;
}

#[cfg(feature = "native")]
mod native {
    use std::net::IpAddr;
    use std::sync::Arc;
    use std::time::Duration;

    use reqwest::{Client, Url, redirect};
    use serde::Deserialize;

    use super::Classifier;

    const MAX_REPLY: usize = 1024;

    struct LocalClassifier {
        client: Client,
        url: Url,
    }

    fn local_url(url: &str) -> Result<Url, &'static str> {
        let invalid =
            "expected an HTTP URL with a literal loopback IP and no credentials or fragment";
        let url = Url::parse(url).map_err(|_| invalid)?;
        let loopback = url
            .host_str()
            .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if url.scheme() == "http"
            && loopback
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
        {
            Ok(url)
        } else {
            Err(invalid)
        }
    }

    impl LocalClassifier {
        fn new(url: &str) -> Result<Self, &'static str> {
            let url = local_url(url)?;
            let _ = rustls::crypto::ring::default_provider().install_default();
            let client = Client::builder()
                .no_proxy()
                .redirect(redirect::Policy::none())
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(8))
                .build()
                .map_err(|_| "could not configure the local classifier client")?;
            Ok(Self { client, url })
        }
    }

    fn score(bytes: &[u8]) -> Result<f32, String> {
        #[derive(Deserialize)]
        struct Reply {
            nsfw: f64,
        }

        let reply: Reply = serde_json::from_slice(bytes)
            .map_err(|_| "invalid response from the local classifier")?;
        if !(0.0..=1.0).contains(&reply.nsfw) {
            return Err("local classifier returned a score outside the unit interval".into());
        }
        Ok(reply.nsfw as f32)
    }

    #[async_trait::async_trait]
    impl Classifier for LocalClassifier {
        async fn classify(&self, image: &[u8]) -> Result<f32, String> {
            let mut reply = self
                .client
                .post(self.url.clone())
                .header(reqwest::header::CONTENT_TYPE, "image/webp")
                .body(image.to_vec())
                .send()
                .await
                .map_err(|_| "could not reach the local classifier")?;
            if !reply.status().is_success() {
                return Err("the local classifier rejected the image".into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = reply
                .chunk()
                .await
                .map_err(|_| "could not read the local classifier response")?
            {
                if chunk.len() > MAX_REPLY - bytes.len() {
                    return Err("the local classifier response is too large".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            score(&bytes)
        }
    }

    pub(crate) fn configure_native(live: &crate::Live) {
        let Ok(url) = std::env::var("H2_LIVE_SCREENSHOT_CLASSIFIER_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        match LocalClassifier::new(&url) {
            Ok(classifier) => live.set_screenshot_classifier(Arc::new(classifier)),
            Err(why) => eprintln!("live: screenshot classifier unavailable ({why})"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn local_classifier_requires_a_literal_loopback_address() {
            for good in ["http://127.0.0.1:8232/score", "http://[::1]/score"] {
                assert!(local_url(good).is_ok(), "{good}");
            }
            for bad in [
                "http://localhost/score",
                "http://127.0.0.1.example.test/score",
                "http://[::]/score",
                "http://127.0.0.1@example.test/score",
                "http://user:secret@127.0.0.1/score",
                "http://127.0.0.1/score#fragment",
                "https://127.0.0.1/score",
            ] {
                assert!(local_url(bad).is_err(), "{bad}");
            }
        }

        #[test]
        fn local_response_requires_a_numeric_unit_score() {
            assert_eq!(score(br#"{"nsfw":0.75}"#).unwrap(), 0.75);
            for bad in [
                br#"{}"#.as_slice(),
                br#"{"nsfw":null}"#,
                br#"{"nsfw":"0"}"#,
                br#"{"nsfw":-0.01}"#,
                br#"{"nsfw":1.000000001}"#,
                br#"{"nsfw":0,"nsfw":1}"#,
            ] {
                assert!(score(bad).is_err(), "{bad:?}");
            }
        }
    }
}

#[cfg(feature = "native")]
pub(crate) use native::configure_native;
