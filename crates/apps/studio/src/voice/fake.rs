//! A voice that says every line as an invented signal, and counts how
//! often it is asked; a clone sidecar that answers in Rust; and invented
//! maps of dialogue for the dataset builder. What Studio uses in its
//! tests: the real service, Python and a disc are never reached from one.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::dataset::{Decoded, DialogueSound, LineSource};
use super::key::KeyError;
use super::sidecar::{Backend, Launcher, Progress, Sidecar};
use super::{Pcm, Request, VoiceError, VoiceSource, audio};

/// The fake: half a second of silence, a second of a 440 Hz tone at 5% of
/// full scale (quiet, so normalising has work to do), half a second of
/// silence.
#[derive(Clone, Debug, Default)]
pub struct Fake {
    /// Lines asked for.
    pub calls: Arc<AtomicUsize>,
    /// The key is missing: every ask answers [`VoiceError::NoKey`].
    pub no_key: bool,
    /// What it says instead of the tone: all silence.
    pub silent: bool,
}

impl Fake {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// The signal, `seconds` of a tone at `amplitude` between the silences.
pub fn signal(amplitude: f64, seconds: f64) -> Vec<i16> {
    let rate = f64::from(audio::RATE);
    let pad = (rate * 0.5) as usize;
    let tone = (rate * seconds) as usize;
    let mut out = vec![0i16; pad];
    out.extend((0..tone).map(|i| {
        let v = amplitude * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin();
        (v * 32767.0).round() as i16
    }));
    out.extend(vec![0i16; pad]);
    out
}

impl VoiceSource for Fake {
    fn say(&mut self, _req: &Request) -> Result<Pcm, VoiceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.no_key {
            return Err(VoiceError::NoKey);
        }
        let samples = if self.silent {
            vec![0; audio::RATE as usize]
        } else {
            signal(0.05, 1.0)
        };
        Ok(Pcm {
            rate: audio::RATE,
            samples,
        })
    }

    fn ready(&mut self) -> Result<(), VoiceError> {
        if self.no_key {
            Err(VoiceError::NoKey)
        } else {
            Ok(())
        }
    }

    fn name(&self) -> &'static str {
        "fake"
    }
}

/// A launcher whose sidecars answer at once, in Rust: no Python, no venv,
/// no network. It records every request; `say` writes a 24 kHz tone,
/// `transcribe` a made-up transcript, `train` an empty weights file.
#[derive(Clone, Debug, Default)]
pub struct FakeLauncher {
    pub installed: Arc<AtomicBool>,
    /// Installs run.
    pub installs: Arc<AtomicUsize>,
    /// Every request: its op and params.
    pub calls: Arc<Mutex<Vec<(String, Value)>>>,
    /// Make `say` fail with this.
    pub fail_say: Option<String>,
    /// While set, `train` waits (a job that is still running).
    pub hold: Arc<AtomicBool>,
}

impl FakeLauncher {
    /// One that is already installed.
    pub fn ready() -> FakeLauncher {
        let f = FakeLauncher::default();
        f.installed.store(true, Ordering::SeqCst);
        f
    }

    /// The requests made with `op`.
    pub fn calls_of(&self, op: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(o, _)| o == op)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

impl Launcher for FakeLauncher {
    fn installed(&self, _b: Backend) -> bool {
        self.installed.load(Ordering::SeqCst)
    }

    fn install(&self, _b: Backend, progress: &mut dyn FnMut(&Progress)) -> Result<(), VoiceError> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        for (i, stage) in ["venv", "packages", "weights"].iter().enumerate() {
            progress(&Progress::new(stage, i as u64, 3));
        }
        self.installed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn launch(&self, b: Backend) -> Result<Box<dyn Sidecar>, VoiceError> {
        if !self.installed(b) {
            return Err(VoiceError::Setup("not installed".into()));
        }
        Ok(Box::new(FakeSidecar {
            launcher: self.clone(),
        }))
    }

    fn dir(&self, b: Backend) -> PathBuf {
        PathBuf::from("/nonexistent/voice-clone").join(b.id())
    }
}

/// [`FakeLauncher`]'s sidecar.
pub struct FakeSidecar {
    launcher: FakeLauncher,
}

/// What the fake says: a 220 Hz tone with silence around it, at 24 kHz.
pub fn clone_signal() -> Vec<i16> {
    let rate = 24_000.0;
    let mut out = vec![0i16; 6_000];
    out.extend((0..36_000u32).map(|i| {
        let v = 0.1 * (2.0 * std::f64::consts::PI * 220.0 * f64::from(i) / rate).sin();
        (v * 32767.0).round() as i16
    }));
    out.extend(vec![0i16; 6_000]);
    out
}

impl Sidecar for FakeSidecar {
    fn call(
        &mut self,
        op: &str,
        params: Value,
        progress: &mut dyn FnMut(&Progress),
    ) -> Result<Value, VoiceError> {
        self.launcher
            .calls
            .lock()
            .unwrap()
            .push((op.to_owned(), params.clone()));
        match op {
            "say" => {
                if let Some(e) = &self.launcher.fail_say {
                    return Err(VoiceError::Model(e.clone()));
                }
                let out = PathBuf::from(params["out"].as_str().unwrap());
                std::fs::write(&out, audio::write_wav(&clone_signal(), 24_000)).unwrap();
                Ok(json!({"out": out.display().to_string(), "rate": 24_000}))
            }
            "transcribe" => {
                let files = params["files"].as_array().cloned().unwrap_or_default();
                let total = files.len() as u64;
                let texts: Vec<Value> = files
                    .iter()
                    .enumerate()
                    .map(|(i, f)| {
                        progress(&Progress::new("transcribe", i as u64 + 1, total));
                        let p = f.as_str().unwrap();
                        let stem = std::path::Path::new(p)
                            .file_stem()
                            .unwrap()
                            .to_string_lossy();
                        // A clip whose id says so is heard as nothing.
                        let text = if stem.contains("mute") {
                            String::new()
                        } else {
                            format!("fake transcript of {stem}")
                        };
                        json!({"path": p, "text": text})
                    })
                    .collect();
                Ok(json!({"texts": texts}))
            }
            "train" => {
                let out = PathBuf::from(params["out"].as_str().unwrap());
                std::fs::create_dir_all(&out).unwrap();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while self.launcher.hold.load(Ordering::SeqCst) && std::time::Instant::now() < until
                {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                for i in 1..=3 {
                    progress(&Progress::new("train", i, 3));
                }
                let w = out.join("model.safetensors");
                std::fs::write(&w, b"").unwrap();
                Ok(json!({"checkpoint": w.display().to_string(), "steps": 3}))
            }
            "hello" | "prepare" => Ok(json!({"backend": params["backend"]})),
            other => Err(VoiceError::Model(format!("no op {other}"))),
        }
    }
}

/// A source of invented maps: dialogue sounds with invented audio.
#[derive(Clone, Debug, Default)]
pub struct FakeLines {
    /// Each map: its sounds (name, permutations, how each sounds) and
    /// its subtitles.
    pub maps: Vec<FakeMap>,
}

#[derive(Clone, Debug, Default)]
pub struct FakeMap {
    pub name: String,
    pub sounds: Vec<(String, Vec<Sound>)>,
    pub subtitles: Vec<(String, String)>,
    /// The map does not open.
    pub broken: bool,
}

/// An invented permutation.
#[derive(Clone, Copy, Debug)]
pub enum Sound {
    /// Speech-like: harmonics of `f0` up to 7 kHz, `seconds` long.
    Voice { f0: f64, seconds: f64 },
    /// The same, band-passed to 400 Hz–3 kHz as a radio would.
    Radio { seconds: f64 },
    /// Nothing.
    Silent,
}

/// `seconds` of a voice-like signal at `rate`: harmonics of `f0`, between
/// `lo` and `hi` Hz, with 0.3 s of silence at each end.
pub fn voice_like(rate: u32, f0: f64, seconds: f64, lo: f64, hi: f64) -> Vec<i16> {
    let r = f64::from(rate);
    let pad = (r * 0.3) as usize;
    let n = (r * seconds) as usize;
    let harmonics: Vec<f64> = (1u32..)
        .map(|k| f0 * f64::from(k))
        .take_while(|f| *f < hi.min(r * 0.45))
        .filter(|f| *f >= lo)
        .collect();
    let mut out = vec![0i16; pad];
    out.extend((0..n).map(|i| {
        let t = i as f64 / r;
        let v: f64 = harmonics
            .iter()
            .map(|f| (2.0 * std::f64::consts::PI * f * t).sin() / (f / f0))
            .sum();
        (0.2 * v / harmonics.len().max(1) as f64 * 32767.0).round() as i16
    }));
    out.extend(vec![0i16; pad]);
    out
}

impl LineSource for FakeLines {
    fn maps(&mut self) -> Vec<String> {
        self.maps.iter().map(|m| m.name.clone()).collect()
    }

    fn dialogue(&mut self, map: &str) -> Result<Vec<DialogueSound>, String> {
        let m = self.map(map)?;
        Ok(m.sounds
            .iter()
            .enumerate()
            .filter(|(_, (name, _))| super::dataset::is_dialogue(name))
            .map(|(i, (name, _))| DialogueSound {
                id: i as u32,
                name: name.clone(),
                class: 7,
            })
            .collect())
    }

    fn subtitles(&mut self, map: &str) -> Result<Vec<(String, String)>, String> {
        Ok(self.map(map)?.subtitles.clone())
    }

    fn permutations(&mut self, map: &str, s: &DialogueSound) -> Result<usize, String> {
        Ok(self.map(map)?.sounds[s.id as usize].1.len())
    }

    fn decode(&mut self, map: &str, s: &DialogueSound, p: usize) -> Result<Decoded, String> {
        let rate = 22_050;
        let pcm = match self.map(map)?.sounds[s.id as usize].1[p] {
            Sound::Voice { f0, seconds } => voice_like(rate, f0, seconds, 0.0, 7_000.0),
            Sound::Radio { seconds } => voice_like(rate, 120.0, seconds, 400.0, 3_000.0),
            Sound::Silent => vec![0; rate as usize],
        };
        Ok(Decoded {
            rate,
            channels: 1,
            pcm,
        })
    }
}

impl FakeLines {
    fn map(&self, name: &str) -> Result<&FakeMap, String> {
        let m = self
            .maps
            .iter()
            .find(|m| m.name == name)
            .ok_or_else(|| format!("no map {name}"))?;
        if m.broken {
            return Err(format!("{name} does not open"));
        }
        Ok(m)
    }

    /// Two maps of Cortana and Johnson lines: subtitled, unsubtitled with
    /// two permutations, too short, radio, silent, one heard as nothing,
    /// one in both maps; and a broken map.
    pub fn sample() -> FakeLines {
        let voice = |s: f64| Sound::Voice {
            f0: 210.0,
            seconds: s,
        };
        FakeLines {
            maps: vec![
                FakeMap {
                    name: "test_a".into(),
                    sounds: vec![
                        (
                            r"sound\dialog\levels\test_level\mission\l01_0010_cor".into(),
                            vec![voice(4.0)],
                        ),
                        (
                            r"sound\dialog\levels\test_level\mission\l01_0020_cor".into(),
                            vec![voice(6.5)],
                        ),
                        (
                            r"sound\dialog\combat\cortana\taunt".into(),
                            vec![voice(2.0), voice(3.0)],
                        ),
                        (
                            r"sound\dialog\levels\test_level\mission\l01_0030_cor".into(),
                            vec![voice(0.4)],
                        ),
                        (
                            r"sound\dialog\levels\test_level\mission\l01_0040_cor".into(),
                            vec![Sound::Radio { seconds: 3.0 }],
                        ),
                        (
                            r"sound\dialog\levels\test_level\mission\l01_0050_cor".into(),
                            vec![Sound::Silent],
                        ),
                        (
                            r"sound\dialog\levels\test_level\mission\l01_mute_cor".into(),
                            vec![voice(2.0)],
                        ),
                        (
                            r"sound\dialog\levels\test_level\mission\l01_0060_jon".into(),
                            vec![Sound::Voice {
                                f0: 110.0,
                                seconds: 3.0,
                            }],
                        ),
                        (r"sound\weapons\rifle\fire".into(), vec![voice(1.5)]),
                    ],
                    subtitles: vec![
                        (
                            "l01_0010_cor".into(),
                            "Cortana: Wake up, [static] Chief.".into(),
                        ),
                        ("l01_0020_cor".into(), "I'll be right here.".into()),
                        ("l01_0060_jon".into(), "Ten-hut!".into()),
                    ],
                    broken: false,
                },
                FakeMap {
                    name: "test_b".into(),
                    sounds: vec![(
                        r"sound\dialog\levels\test_level\mission\l01_0010_cor".into(),
                        vec![voice(4.0)],
                    )],
                    subtitles: Vec::new(),
                    broken: false,
                },
                FakeMap {
                    name: "broken".into(),
                    broken: true,
                    ..FakeMap::default()
                },
            ],
        }
    }
}

/// A key store with a fixed answer.
pub struct FixedKey(pub Result<String, KeyError>);

impl super::key::KeySource for FixedKey {
    fn key(&self) -> Result<super::key::Secret, KeyError> {
        self.0.clone().map(super::key::Secret::new)
    }

    fn present(&self) -> Result<bool, KeyError> {
        match &self.0 {
            Ok(_) => Ok(true),
            Err(KeyError::Missing) => Ok(false),
            Err(e) => Err(e.clone()),
        }
    }
}
