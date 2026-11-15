//! The `audio::Mixer` on a cpal output device, with pause.
//!
//! Same callback as `audio::output::Output` (try-lock the mixer, silence if
//! the guest holds it), plus `pause`/`resume` so a paused guest does not
//! leave a looping DirectSound buffer droning. The cpal `Stream` is not
//! `Send` on macOS, so an [`AudioOut`] stays on the thread that made it (the
//! window thread); only the mixer handle crosses to the guest.
//!
//! With no device (`audio: false`, no hardware, or a failed open) the mixer
//! still has to be consumed: DirectSound buffers only finish, and stream
//! packets only complete, when the mixer renders them. [`DiscardRenderer`]
//! renders the mixer in real time on a thread of its own and throws the
//! samples away, honouring pause and shutdown like a device would.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audio::Mixer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream};

use crate::lifecycle::Lifecycle;

/// Rate used when no device is available (the DirectSound default).
pub const FALLBACK_RATE: u32 = 48_000;

pub struct AudioOut {
    stream: Option<Stream>,
    discard: Option<DiscardRenderer>,
    mixer: Arc<Mutex<Mixer>>,
    sample_rate: u32,
    device: String,
}

impl AudioOut {
    /// Opens the default output device and starts it. On failure the caller
    /// can fall back to [`AudioOut::silent`], which still gives the guest a
    /// mixer.
    pub fn start_default() -> Result<Self, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no default audio output device")?;
        let name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "default".into());
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let sample_rate = config.sample_rate();
        let mixer = Arc::new(Mutex::new(Mixer::new(sample_rate)));
        let format = config.sample_format();
        let cfg: cpal::StreamConfig = config.into();
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, cfg, &mixer),
            SampleFormat::I16 => build::<i16>(&device, cfg, &mixer),
            SampleFormat::I32 => build::<i32>(&device, cfg, &mixer),
            SampleFormat::U16 => build::<u16>(&device, cfg, &mixer),
            other => Err(format!("unsupported device sample format {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            stream: Some(stream),
            discard: None,
            mixer,
            sample_rate,
            device: name,
        })
    }

    /// A mixer with no device (headless runs, or no audio hardware),
    /// consumed in real time by a [`DiscardRenderer`]. With `life`, the
    /// renderer also pauses with the guest and stops on a stop request.
    pub fn silent(sample_rate: u32, life: Option<Lifecycle>) -> Self {
        let mixer = Arc::new(Mutex::new(Mixer::new(sample_rate)));
        Self {
            stream: None,
            discard: Some(DiscardRenderer::start(
                Arc::clone(&mixer),
                sample_rate,
                life,
            )),
            mixer,
            sample_rate,
            device: "none".into(),
        }
    }

    /// The shared mixer; DirectSound is built on this.
    pub fn mixer(&self) -> &Arc<Mutex<Mixer>> {
        &self.mixer
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device
    }

    pub fn has_device(&self) -> bool {
        self.stream.is_some()
    }

    /// Stop pulling from the mixer (voices keep their positions).
    pub fn pause(&self) {
        if let Some(s) = &self.stream {
            let _ = s.pause();
        }
        if let Some(d) = &self.discard {
            d.pause();
        }
    }

    pub fn resume(&self) {
        if let Some(s) = &self.stream {
            let _ = s.play();
        }
        if let Some(d) = &self.discard {
            d.resume();
        }
    }
}

#[derive(Default)]
struct DiscardShared {
    stop: AtomicBool,
    paused: AtomicBool,
    frames: AtomicU64,
}

/// Renders a mixer in real time and discards the output: the stand-in for
/// an audio device. Stops (and joins its thread) when dropped. `Send`.
pub struct DiscardRenderer {
    shared: Arc<DiscardShared>,
    join: Option<JoinHandle<()>>,
}

/// How often the renderer wakes; also the worst-case pause/stop latency.
const DISCARD_PERIOD: Duration = Duration::from_millis(5);
/// Largest single render call, in frames.
const DISCARD_CHUNK: u64 = 1024;

impl DiscardRenderer {
    /// Starts consuming `mixer` at `sample_rate` frames per second. Paused
    /// while [`pause`](Self::pause)d or while `life` is paused; finished once
    /// dropped or once `life` asks the guest to stop.
    pub fn start(mixer: Arc<Mutex<Mixer>>, sample_rate: u32, life: Option<Lifecycle>) -> Self {
        let shared = Arc::new(DiscardShared::default());
        let sh = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("audio-discard".into())
            .spawn(move || {
                let rate = u128::from(sample_rate.max(1));
                let mut scratch = Vec::new();
                let mut base = Instant::now();
                let mut done = 0u64;
                loop {
                    if sh.stop.load(Ordering::Acquire)
                        || life.as_ref().is_some_and(|l| l.should_stop())
                    {
                        return;
                    }
                    let paused = sh.paused.load(Ordering::Acquire)
                        || life.as_ref().is_some_and(|l| l.is_paused());
                    if paused {
                        // Time spent paused is not owed afterwards.
                        base = Instant::now();
                        done = 0;
                    } else {
                        let due = (base.elapsed().as_nanos() * rate / 1_000_000_000) as u64;
                        while done < due {
                            let n = (due - done).min(DISCARD_CHUNK);
                            scratch.resize(n as usize * 2, 0.0);
                            mixer
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .render(&mut scratch);
                            done += n;
                            sh.frames.fetch_add(n, Ordering::Relaxed);
                        }
                    }
                    std::thread::sleep(DISCARD_PERIOD);
                }
            })
            .expect("spawning the audio discard thread");
        Self {
            shared,
            join: Some(join),
        }
    }

    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Release);
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Release);
    }

    /// Frames rendered so far.
    pub fn frames_rendered(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }

    /// The thread has exited (dropped, or the guest was told to stop).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(|j| j.is_finished())
    }
}

impl Drop for DiscardRenderer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mixer: &Arc<Mutex<Mixer>>,
) -> Result<Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mixer = Arc::clone(mixer);
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let frames = data.len() / channels.max(1);
                scratch.resize(frames * 2, 0.0);
                match mixer.try_lock() {
                    Ok(mut m) => m.render(&mut scratch),
                    Err(_) => scratch.fill(0.0),
                }
                audio::output::write_frames(&scratch, data, channels);
            },
            {
                // Underruns repeat while the guest is slow; report the first
                // of each kind, then every 100th.
                let mut seen: Vec<(String, u64)> = Vec::new();
                move |e| {
                    let msg = e.to_string();
                    let n = match seen.iter_mut().find(|(m, _)| *m == msg) {
                        Some((_, n)) => {
                            *n += 1;
                            *n
                        }
                        None => {
                            seen.push((msg.clone(), 1));
                            1
                        }
                    };
                    if n == 1 || n.is_multiple_of(100) {
                        eprintln!("app: audio stream error ({n}x): {msg}");
                    }
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::dsound::PacketStatus;
    use audio::{SoundBuffer, Stream, WaveFormat};

    const RATE: u32 = 48_000;

    /// 0.1 s of 16-bit mono silence at 22.05 kHz.
    fn clip() -> Vec<u8> {
        vec![0u8; 2205 * 2]
    }

    fn wait_until(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < limit {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    #[test]
    fn silent_output_finishes_buffers_and_completes_packets() {
        let out = AudioOut::silent(RATE, None);
        assert!(!out.has_device());
        let (buf, stream) = {
            let mut m = out.mixer().lock().unwrap();
            let id = m.add_buffer(
                SoundBuffer::from_data(WaveFormat::pcm16(1, 22_050).unwrap(), clip()).unwrap(),
            );
            m.buffer_mut(id).unwrap().play(false);
            let mut s = Stream::new(WaveFormat::pcm16(1, 22_050).unwrap()).unwrap();
            let packet = s.submit(&clip()).unwrap();
            (id, (m.add_stream(s), packet))
        };
        let mixer = out.mixer();
        assert!(
            wait_until(Duration::from_secs(3), || !mixer
                .lock()
                .unwrap()
                .buffer_mut(buf)
                .unwrap()
                .is_playing()),
            "a non-looping buffer finishes"
        );
        let mut completed = Vec::new();
        assert!(wait_until(Duration::from_secs(3), || {
            completed.extend(
                mixer
                    .lock()
                    .unwrap()
                    .stream_mut(stream.0)
                    .unwrap()
                    .take_completed(),
            );
            !completed.is_empty()
        }));
        assert_eq!(completed, [(stream.1, PacketStatus::Success)]);
    }

    #[test]
    fn discard_runs_in_real_time() {
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let d = DiscardRenderer::start(Arc::clone(&mixer), RATE, None);
        std::thread::sleep(Duration::from_millis(200));
        let n = d.frames_rendered();
        // 200 ms is 9600 frames; allow scheduling slack, but never a burst.
        assert!(
            (4_800..=RATE as u64 / 2).contains(&n),
            "{n} frames after 200 ms"
        );
    }

    #[test]
    fn discard_pauses_and_owes_nothing_for_the_pause() {
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let d = DiscardRenderer::start(Arc::clone(&mixer), RATE, None);
        let id = mixer.lock().unwrap().add_buffer(
            SoundBuffer::from_data(WaveFormat::pcm16(1, 22_050).unwrap(), clip()).unwrap(),
        );
        d.pause();
        std::thread::sleep(Duration::from_millis(30));
        mixer.lock().unwrap().buffer_mut(id).unwrap().play(false);
        let before = d.frames_rendered();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(d.frames_rendered(), before, "nothing rendered while paused");
        assert!(
            mixer.lock().unwrap().buffer_mut(id).unwrap().is_playing(),
            "the buffer waits"
        );
        d.resume();
        std::thread::sleep(Duration::from_millis(50));
        // No catch-up burst for the 300 ms pause.
        assert!(
            d.frames_rendered() - before < RATE as u64 / 5,
            "{}",
            d.frames_rendered() - before
        );
        assert!(wait_until(Duration::from_secs(3), || !mixer
            .lock()
            .unwrap()
            .buffer_mut(id)
            .unwrap()
            .is_playing()));
    }

    #[test]
    fn discard_follows_the_guest_lifecycle() {
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let life = Lifecycle::new();
        let d = DiscardRenderer::start(Arc::clone(&mixer), RATE, Some(life.clone()));
        life.set_paused(true);
        std::thread::sleep(Duration::from_millis(20));
        let before = d.frames_rendered();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            d.frames_rendered(),
            before,
            "guest pause pauses the renderer"
        );
        life.set_paused(false);
        life.request_stop();
        assert!(
            wait_until(Duration::from_secs(1), || d.is_finished()),
            "guest stop ends the renderer"
        );
    }

    #[test]
    fn dropping_stops_the_thread_promptly() {
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let d = DiscardRenderer::start(Arc::clone(&mixer), RATE, None);
        let t = Instant::now();
        drop(d);
        assert!(t.elapsed() < Duration::from_millis(500));
    }
}
