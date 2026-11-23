//! `audio.meter`: the level of the host mixer's output bus
//! (docs/runtime/agent-api.md, "audio.meter"; docs/audio/audio.md,
//! "Metering the mixer"). Wire types only; the game registers the method
//! (`game::mixer_meter`).

use serde::{Deserialize, Serialize};

/// `audio.meter`: meter the last `window_ms` of the mixer's output, or
/// start a named host clip and meter it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MeterParams {
    /// How much of the latest output to meter, in ms
    /// ([`MeterParams::WINDOW_MS`]; default [`MeterParams::DEFAULT_WINDOW_MS`]).
    pub window_ms: Option<u32>,
    /// A host clip to start: `pogo/compress`, `pogo/spring`, `pogo/settle`
    /// or `announcer/<medal id>`. The reply meters from the start to its
    /// end. Not with `window_ms`.
    pub play: Option<String>,
}

/// A checked [`MeterParams`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeterRequest {
    Window { ms: u32 },
    Play { clip: String },
}

impl MeterParams {
    pub const DEFAULT_WINDOW_MS: u32 = 1000;
    pub const WINDOW_MS: std::ops::RangeInclusive<u32> = 50..=5000;

    pub fn check(&self) -> Result<MeterRequest, String> {
        match (&self.play, self.window_ms) {
            (Some(_), Some(_)) => Err("play meters the clip's own length; drop window_ms".into()),
            (Some(clip), None) if clip.is_empty() => Err("play names a clip".into()),
            (Some(clip), None) => Ok(MeterRequest::Play { clip: clip.clone() }),
            (None, ms) => {
                let ms = ms.unwrap_or(Self::DEFAULT_WINDOW_MS);
                if Self::WINDOW_MS.contains(&ms) {
                    Ok(MeterRequest::Window { ms })
                } else {
                    Err(format!(
                        "window_ms must be {}..={}",
                        Self::WINDOW_MS.start(),
                        Self::WINDOW_MS.end()
                    ))
                }
            }
        }
    }
}

/// `audio.meter`'s answer. Levels are rounded to 0.1 dB; silence reads -120.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterReply {
    /// The span metered, ms.
    pub window_ms: u64,
    pub frames: u64,
    pub rate: u32,
    pub channels: u16,
    /// Sample peak, dBFS.
    pub peak_dbfs: f64,
    /// RMS over every sample of both channels, dBFS.
    pub rms_dbfs: f64,
    /// Integrated loudness, LUFS (BS.1770-4, gated).
    pub lufs: f64,
    /// The loudest 400 ms block, LUFS.
    pub max_momentary_lufs: f64,
    /// Samples at or past full scale.
    pub clipped: u64,
    /// The clip `play` started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play: Option<MeterPlay>,
}

/// The clip a `play` request started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeterPlay {
    pub clip: String,
    /// The clip's own length, ms.
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> Result<MeterRequest, String> {
        serde_json::from_value::<MeterParams>(v).unwrap().check()
    }

    #[test]
    fn meter_params_are_checked() {
        assert_eq!(parse(json!({})), Ok(MeterRequest::Window { ms: 1000 }));
        assert_eq!(
            parse(json!({"window_ms": 400})),
            Ok(MeterRequest::Window { ms: 400 })
        );
        assert_eq!(
            parse(json!({"play": "pogo/spring"})),
            Ok(MeterRequest::Play {
                clip: "pogo/spring".into()
            })
        );
        for bad in [
            json!({"window_ms": 49}),
            json!({"window_ms": 5001}),
            json!({"play": ""}),
            json!({"play": "pogo/spring", "window_ms": 400}),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
        assert!(serde_json::from_value::<MeterParams>(json!({"window": 400})).is_err());
        assert!(serde_json::from_value::<MeterParams>(json!({"window_ms": -1})).is_err());
    }

    #[test]
    fn a_window_reply_has_no_play() {
        let v = serde_json::to_value(MeterReply::default()).unwrap();
        assert!(v.get("play").is_none());
        assert_eq!(v["peak_dbfs"], json!(0.0));
    }
}
