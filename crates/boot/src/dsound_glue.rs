//! Installs crates/dsound through `dsound::register` (docs/audio/dsound.md "Boot
//! integration") with a headless mixer. Completion events go through the
//! kernel, pinned with `Kernel::reference_dispatcher` so a closed handle
//! still gets its signal. The game's WMA codec runs natively
//! (`dsound::xwma`). Audio time is logical (docs/audio/dsound.md "Boot
//! integration"): an output thread with `Pacing::Logical` into a
//! `NullSink`, which the runner drives with [`advance`] between slices, so
//! a run stays deterministic. When every guest thread waits and only
//! audio is still due to finish something, [`wake_idle`] moves logical
//! time on to the next audio quantum instead of reporting a deadlock.

use std::sync::{Arc, Mutex};

use dsound::{DirectSound, Hle, Layout};

/// Read-only routing view for comparing one emitter at several distances.
pub(crate) fn add_agent_methods(agent: &agent::Agent, mixer: Arc<Mutex<audio::Mixer>>) {
    agent.add_method(
        agent_proto::method::AUDIO_VOICES,
        agent::Access::Read,
        move |params| voice_levels(&mixer, params),
    );
}

fn voice_levels(
    mixer: &Mutex<audio::Mixer>,
    params: serde_json::Value,
) -> Result<serde_json::Value, agent_proto::RpcError> {
    use serde_json::json;
    if !(params.is_null() || params.as_object().is_some_and(|p| p.is_empty())) {
        return Err(agent_proto::RpcError::new(
            agent_proto::code::INVALID_PARAMS,
            "audio.voices takes no params",
        ));
    }
    let (rows, distance_reverb, reverb_active) = {
        let m = mixer.lock().unwrap_or_else(|e| e.into_inner());
        (
            m.voice_levels(),
            m.distance_reverb(),
            m.reverb().is_some_and(|e| !e.is_silent()),
        )
    };
    let voices = rows.into_iter().map(|v| {
        let bins = v.bins.map(|pairs| pairs.into_iter().map(|(bin, mb)| {
            let path_gain = if bin == audio::route::I3DL2 { v.room_gain } else { v.direct_gain };
            json!({ "bin": bin, "volume_mb": mb, "gain": audio::dsound::mb_to_gain(mb) * path_gain })
        }).collect::<Vec<_>>());
        json!({
            "id": v.id, "kind": v.kind, "input_bin": v.input_bin,
            "distance_units": v.distance_units, "volume_mb": v.volume_mb, "bins": bins,
            "direct_gain": v.direct_gain, "room_gain": v.room_gain,
            "direct_hf_db": v.direct_hf_db, "room_hf_db": v.room_hf_db,
            "reference_hz": v.reference_hz
        })
    }).collect::<Vec<_>>();
    Ok(
        json!({ "distance_reverb": distance_reverb, "reverb_active": reverb_active, "voices": voices }),
    )
}

struct KernelEvents(Arc<kernel::Kernel>);

/// A pinned kernel event; dropping it releases the pin.
struct Pin(kernel::DispatcherReference);

impl dsound::EventRef for Pin {
    fn signal(&mut self) -> Result<(), String> {
        self.0.signal_event().map(drop).map_err(|e| e.to_string())
    }
    fn address(&self) -> Option<u32> {
        Some(self.0.address())
    }
    fn forget(self: Box<Self>) {
        let Pin(pin) = *self;
        pin.forget();
    }
}

impl dsound::Events for KernelEvents {
    fn signal(&mut self, handle: u32) -> Result<(), String> {
        self.0
            .signal_event(handle)
            .map(drop)
            .map_err(|e| e.to_string())
    }
    fn reference(&mut self, handle: u32) -> Option<Result<Box<dyn dsound::EventRef>, String>> {
        Some(
            self.0
                .reference_dispatcher(handle)
                .map(|r| Box::new(Pin(r)) as Box<dyn dsound::EventRef>)
                .map_err(|e| e.to_string()),
        )
    }
    fn adopt(&mut self, address: u32) -> Option<Result<Box<dyn dsound::EventRef>, String>> {
        Some(
            self.0
                .adopt_reference(address)
                .map(|r| Box::new(Pin(r)) as Box<dyn dsound::EventRef>)
                .map_err(|e| e.to_string()),
        )
    }
}

/// The DirectSound HLE and its logical-time output.
pub struct Audio {
    pub hle: Arc<Hle>,
    pub output: dsound::Output,
    /// The kernel periodic source that runs stream callbacks at DPC level
    /// every audio quantum ([`COMPLETION_RATE`]). Removing it brings back
    /// callbacks that wait for the game's next DirectSound call.
    pub completion_dpc: u32,
    /// The native WMA codec, when the image's tables loaded (a machine
    /// snapshot's `wma` section).
    pub wma: Option<Arc<dsound::xwma::WmaHle>>,
    mixer: Arc<Mutex<audio::Mixer>>,
    /// The controller-speaker sound index, while one runs.
    _pad_index: Option<crate::pad_speaker::PadIndex>,
}

/// The controller speaker for [`install_with_sinks`]: its settings (routes,
/// exclusive), its output and the on/off switch every router goes through.
pub struct PadSpeaker {
    pub config: dsound::padroute::PadSpeakerConfig,
    pub side: dsound::SideOutput,
    pub setting: Arc<crate::speaker_setting::SpeakerSetting>,
}

/// The mixer rate [`install`] uses.
const RATE: u64 = 48_000;
/// Logical ticks (100 ns) in which one more `dsound::output::QUANTUM` falls due.
pub const QUANTUM_TICKS: u64 = (dsound::output::QUANTUM * 10_000_000).div_ceil(RATE);
/// One audio quantum per period: the rate of the completion DPC, whose
/// deadlines fall on the output's grid (`Rate::ticks(n)` rounds up, so
/// quantum `n` is due at deadline `n`).
pub const COMPLETION_RATE: kernel::periodic::Rate = kernel::periodic::Rate {
    num: RATE,
    den: dsound::output::QUANTUM,
};
/// How much logical time [`wake_idle`] may add with no thread running in
/// between before the run is a deadlock after all: a looping buffer plays
/// for ever, and must not hide a real one.
pub const IDLE_LIMIT_TICKS: u64 = 10 * 10_000_000;

impl Audio {
    /// The mixer the output renders, for other sound that should go out
    /// with the game's (the movies' voices, movies_glue.rs).
    pub fn mixer(&self) -> Arc<Mutex<audio::Mixer>> {
        Arc::clone(&self.mixer)
    }

    /// Something in the mixer can still finish and signal: a playing buffer
    /// or a stream with queued packets.
    pub fn pending(&self) -> bool {
        self.mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_voices()
            > 0
    }
}

/// Guest-thread slices the machine has scheduled so far. [`wake_idle`]'s
/// budget is reset only when this moves: a slice that only delivered a
/// periodic callback (vblank) ends in `SliceLimit` too, and must not refill
/// the budget while every thread stays blocked.
pub fn guest_slices(m: &kernel::Machine) -> u64 {
    m.schedule().len() as u64 + m.dropped_schedule()
}

/// After `m.run(1)`: whether it ran a guest thread's slice, given
/// [`guest_slices`] from before it. A DPC (vblank, or the completion DPC
/// running a stream callback) is not one: a stream that refills itself from
/// its callback must not keep a deadlocked game looking alive.
pub fn ran_guest_thread(m: &kernel::Machine, before: u64) -> bool {
    guest_slices(m) != before && m.schedule().last() != Some(&kernel::threads::DPC_THREAD_ID)
}

/// The scheduler found no runnable thread and no timed wake. If audio is
/// pending (and `idle` logical ticks added this way since a thread last ran
/// are under [`IDLE_LIMIT_TICKS`]), advance logical time by one audio
/// quantum, render it (completing its packets and signalling their events)
/// and return true so the runner tries again. Between slices, with no
/// guest memory guard held.
pub fn wake_idle(
    a: &Audio,
    k: &kernel::Kernel,
    rt: &runtime::Runtime,
    idle: &mut u64,
) -> Result<bool, String> {
    if *idle >= IDLE_LIMIT_TICKS || !a.pending() {
        return Ok(false);
    }
    k.advance_time(rt, QUANTUM_TICKS)
        .map_err(|e| format!("advance_time: {e:?}"))?;
    *idle += QUANTUM_TICKS;
    advance(a, k)?;
    Ok(true)
}

/// Opens the sink the logical-time output plays into, on the output thread
/// (`audio::output::DeviceSink` is not `Send` on macOS).
pub type SinkOpener = Box<dyn FnOnce() -> Box<dyn audio::sink::AudioSink> + Send>;

/// A boxed sink, so [`SinkOpener`] can pick one at run time.
struct BoxedSink(Box<dyn audio::sink::AudioSink>);

impl audio::sink::AudioSink for BoxedSink {
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
    fn queued_frames(&self) -> usize {
        self.0.queued_frames()
    }
    fn free_frames(&self) -> usize {
        self.0.free_frames()
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        self.0.write(samples)
    }
    fn stats(&self) -> audio::sink::SinkStats {
        self.0.stats()
    }
    fn maintain(&mut self) {
        self.0.maintain()
    }
    fn connected(&self) -> bool {
        self.0.connected()
    }
}

pub fn install(
    rt: &Arc<runtime::Runtime>,
    k: &Arc<kernel::Kernel>,
    xbe: &[u8],
) -> anyhow::Result<Audio> {
    install_with_sink(
        rt,
        k,
        xbe,
        Box::new(|| Box::new(audio::NullSink::new(RATE as u32, RATE as usize / 4))),
    )
}

/// [`install`] playing into the caller's sink (a host's audio device).
/// Pacing stays logical: audio time is the kernel's, so guest-visible audio
/// state is the same as a headless run; a host slower than real time is
/// heard as underruns, never as a different schedule.
pub fn install_with_sink(
    rt: &Arc<runtime::Runtime>,
    k: &Arc<kernel::Kernel>,
    xbe: &[u8],
    open: SinkOpener,
) -> anyhow::Result<Audio> {
    install_with_sinks(rt, k, xbe, open, None)
}

/// [`install_with_sink`] with the controller speaker: a side output for the
/// routed sounds, and the sound index ([`crate::pad_speaker`]) built from
/// every map the game opens. With `pad` `None`, or its device absent, every
/// sound plays on `open`'s sink.
pub fn install_with_sinks(
    rt: &Arc<runtime::Runtime>,
    k: &Arc<kernel::Kernel>,
    xbe: &[u8],
    open: SinkOpener,
    pad: Option<PadSpeaker>,
) -> anyhow::Result<Audio> {
    install_parts(rt, k, Some(xbe), open, pad)
}

/// [`install_with_sink`] without the native WMA codec (it needs the retail
/// image's tables): the game's own codec would run. For tests of the audio
/// path that have no image.
pub fn install_without_wma(
    rt: &Arc<runtime::Runtime>,
    k: &Arc<kernel::Kernel>,
    open: SinkOpener,
) -> anyhow::Result<Audio> {
    install_parts(rt, k, None, open, None)
}

fn install_parts(
    rt: &Arc<runtime::Runtime>,
    k: &Arc<kernel::Kernel>,
    xbe: Option<&[u8]>,
    open: SinkOpener,
    pad: Option<PadSpeaker>,
) -> anyhow::Result<Audio> {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(RATE as u32)));
    let hle = Hle::new(
        DirectSound::new(Arc::clone(&mixer), Layout::default()),
        Box::new(KernelEvents(Arc::clone(k))),
    );
    let mut r = None;
    rt.edit_dispatch(|d| r = Some(dsound::register(d, &hle)));
    r.unwrap()
        .map_err(|e| anyhow::anyhow!("dsound::register: {e:?}"))?;
    let wma = match xbe {
        Some(xbe) => {
            let wma = dsound::xwma::WmaHle::from_xbe(xbe)
                .map_err(|e| anyhow::anyhow!("dsound::xwma: {e:?}"))?;
            let mut r = None;
            rt.edit_dispatch(|d| r = Some(dsound::xwma::register(d, &wma)));
            r.unwrap()
                .map_err(|e| anyhow::anyhow!("dsound::xwma::register: {e:?}"))?;
            Some(wma)
        }
        None => None,
    };
    let (config, side, setting) = match pad {
        Some(p) => (Some(p.config), Some(p.side), Some(p.setting)),
        None => (None, None, None),
    };
    if let Some(s) = &setting {
        let hle = Arc::clone(&hle);
        s.attach_router_sink(move |router| hle.direct_sound().set_pad_router(router));
    }
    let origin = k.clock().counter();
    let output = hle
        .spawn_output(
            Arc::clone(rt),
            move || BoxedSink(open()),
            dsound::OutputConfig {
                period: std::time::Duration::from_millis(5),
                pacing: dsound::Pacing::Logical {
                    origin,
                    max_queued: std::time::Duration::from_millis(250),
                    playout: Some(playout_from_env()),
                },
                halt_when: None,
                side,
            },
        )
        .map_err(|e| anyhow::anyhow!("dsound output: {e:?}"))?;
    // Stream callbacks at DPC level, on the output's grid: the completion
    // DPC's deadlines also stop the idle advance at every quantum, so a
    // callback runs within two quanta of its packet (docs/audio/dsound.md, "The
    // completion DPC").
    anyhow::ensure!(
        k.clock().counter() == origin,
        "logical time moved while the audio output started"
    );
    let completion_dpc = k
        .add_periodic(kernel::periodic::Source {
            name: "dsound completions",
            rate: COMPLETION_RATE,
            // One drain clears a debt: a period with nothing queued calls nothing.
            max_catch_up: 64,
            handler: Box::new(hle.completion_dpc()),
        })
        .map_err(|e| anyhow::anyhow!("dsound completion DPC: {e:?}"))?;
    let _pad_index = config
        .zip(setting)
        .and_then(|(c, s)| crate::pad_speaker::install(k, &c, s));
    Ok(Audio {
        hle,
        output,
        completion_dpc,
        wma,
        mixer,
        _pad_index,
    })
}

/// The jitter buffer a device plays through: `dsound::Playout::default()`,
/// with `H2_AUDIO_LATENCY_MS` (10..=500) as the starting target.
pub fn playout_from_env() -> dsound::Playout {
    playout_from(std::env::var("H2_AUDIO_LATENCY_MS").ok().as_deref())
}

fn playout_from(latency_ms: Option<&str>) -> dsound::Playout {
    let mut p = dsound::Playout::default();
    match latency_ms.map(|v| v.trim().parse::<u32>()) {
        None => {}
        Some(Ok(ms)) if (10..=500).contains(&ms) => {
            p.target_ms = ms;
            p.max_target_ms = p.max_target_ms.max(ms + 2 * p.step_ms);
        }
        Some(_) => eprintln!(
            "audio: H2_AUDIO_LATENCY_MS wants 10..=500; using {} ms",
            p.target_ms
        ),
    }
    p
}

/// `H2_AUDIO_STATS=1`: one line every [`AudioReport::EVERY`] on stderr with
/// what the audio path did in that window (docs/audio/dsound.md, "Audio stats").
pub struct AudioReport {
    last_at: std::time::Instant,
    last: Option<(dsound::OutputStats, audio::StreamHealth)>,
    max_callbacks: usize,
}

impl AudioReport {
    pub const EVERY: std::time::Duration = std::time::Duration::from_secs(5);

    /// A reporter if `H2_AUDIO_STATS` is set (and not `0`).
    pub fn from_env() -> Option<Self> {
        std::env::var("H2_AUDIO_STATS")
            .is_ok_and(|v| v != "0" && !v.is_empty())
            .then(|| Self {
                last_at: std::time::Instant::now(),
                last: None,
                max_callbacks: 0,
            })
    }

    /// Between slices: notes what only shows at a moment, and prints the
    /// line once a window is up.
    pub fn tick(&mut self, a: &Audio) {
        self.max_callbacks = self.max_callbacks.max(a.hle.pending_callbacks());
        let now = std::time::Instant::now();
        let secs = now.duration_since(self.last_at).as_secs_f64();
        if secs < Self::EVERY.as_secs_f64() {
            return;
        }
        self.last_at = now;
        let out = a.output.take_window();
        let health = a
            .mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stream_health();
        eprintln!(
            "{}",
            audio_line(secs, &out, health, self.last.as_ref(), self.max_callbacks)
        );
        self.last = Some((out, health));
        self.max_callbacks = 0;
    }
}

/// One `H2_AUDIO_STATS` line: counters are what changed since `last`.
fn audio_line(
    secs: f64,
    out: &dsound::OutputStats,
    health: audio::StreamHealth,
    last: Option<&(dsound::OutputStats, audio::StreamHealth)>,
    max_callbacks: usize,
) -> String {
    let (po, ph) = last.copied().unwrap_or_default();
    let d = |now: u64, then: u64| now.saturating_sub(then);
    let p = out.playout.unwrap_or_default();
    let mut line = format!(
        "audio: {secs:.1}s fill {:.0}ms ({:.0}-{:.0}) target {:.0}ms trim {:+}ppm{} | dropouts +{} | dev underruns +{} ({} frames) | dropped +{} | late wakes +{} (max gap {:.1}ms) | stream starves +{} ({} frames) unrouted +{} frames | callbacks waiting max {} | rendered {:.0}% of real time",
        p.fill_ms,
        p.min_fill_ms,
        p.max_fill_ms,
        p.target_ms,
        p.trim_ppm,
        if p.primed { "" } else { " (buffering)" },
        d(p.starves, po.playout.unwrap_or_default().starves),
        d(out.sink.underruns, po.sink.underruns),
        d(out.sink.underrun_frames, po.sink.underrun_frames),
        d(out.dropped_frames, po.dropped_frames),
        d(out.late_wakes, po.late_wakes),
        out.max_wake_gap_us as f64 / 1000.0,
        d(health.starves, ph.starves),
        d(health.starve_frames, ph.starve_frames),
        d(health.unrouted_frames, ph.unrouted_frames),
        max_callbacks,
        100.0 * d(out.rendered_frames, po.rendered_frames) as f64 / (secs * RATE as f64),
    );
    if let Some(side) = out.side {
        let ps = po.side.unwrap_or_default();
        line.push_str(&format!(
            " | pad {} underruns +{} dropped +{}",
            if side.live { "live" } else { "off" },
            d(side.sink.underruns, ps.sink.underruns),
            d(side.dropped_frames, ps.dropped_frames),
        ));
    }
    line
}

/// Bring audio time up to the kernel's logical time. Between slices, with
/// no guest memory guard held.
pub fn advance(a: &Audio, k: &kernel::Kernel) -> Result<(), String> {
    a.output
        .advance_to(k.clock().counter())
        .map(drop)
        .map_err(|e| format!("{e:?}"))
}

pub fn describe(a: &Audio) -> String {
    let hle = &a.hle;
    format!(
        "dsound: pending callbacks {}; stops {:?}; signal failures {:?}; output {:?}",
        hle.pending_callbacks(),
        hle.take_stops(),
        hle.take_signal_failures(),
        a.output.stats()
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn audio_voices_reports_the_positioned_bus_and_its_input_stream() {
        use super::voice_levels;
        use audio::dsound::{Source3d, Vec3};
        use serde_json::json;
        use std::sync::Mutex;
        let mut m = audio::Mixer::new(48_000);
        let format = audio::WaveFormat::pcm16(1, 48_000).unwrap();
        let mut bus = audio::MixIn::new(17);
        bus.set_source_3d(Some(Source3d {
            position: Vec3::new(0.0, 3.0, 4.0),
            ..Default::default()
        }));
        bus.set_mix_bins(Some(vec![(6, 0), (10, -1000)]));
        bus.play();
        m.add_mix_in(bus);
        let mut stream = audio::Stream::new(format).unwrap();
        stream.set_mix_bins(Some(vec![(17, -2000)]));
        stream.submit(&[0; 16]).unwrap();
        m.add_stream(stream);
        m.add_mix_in(audio::MixIn::new(18)); // stopped
        let m = Mutex::new(m);
        assert!(voice_levels(&m, json!({"id": 0})).is_err());
        let result = voice_levels(&m, json!({})).unwrap();
        let voices = result["voices"].as_array().unwrap();
        assert_eq!(voices.len(), 2);
        assert_eq!(result["distance_reverb"], false);
        assert_eq!(result["reverb_active"], false);
        assert_eq!(voices[0]["kind"], "stream");
        assert!(voices[0]["distance_units"].is_null());
        assert_eq!(voices[0]["bins"][0]["bin"], voices[1]["input_bin"]);
        assert_eq!(voices[1]["kind"], "mix_in");
        assert_eq!(voices[1]["distance_units"], 5.0);
        assert!((voices[1]["bins"][1]["gain"].as_f64().unwrap() - 10f64.powf(-0.5)).abs() < 1e-6);
    }

    #[test]
    fn latency_override_sets_the_target_and_room_to_grow() {
        let d = dsound::Playout::default();
        assert_eq!(super::playout_from(None), d);
        let p = super::playout_from(Some("200"));
        assert_eq!(p.target_ms, 200);
        assert!(p.max_target_ms > 200);
        assert_eq!(super::playout_from(Some("5")), d, "out of range");
        assert_eq!(super::playout_from(Some("x")), d);
    }

    #[test]
    fn the_stats_line_reports_the_window() {
        let mut then = dsound::OutputStats::default();
        then.sink.underruns = 4;
        let mut now = then;
        now.rendered_frames = 240_000;
        now.sink.underruns = 6;
        now.sink.underrun_frames = 512;
        now.late_wakes = 1;
        now.max_wake_gap_us = 21_500;
        now.playout = Some(dsound::PlayoutStats {
            fill_ms: 48.0,
            min_fill_ms: 30.0,
            max_fill_ms: 70.0,
            target_ms: 75.0,
            primed: true,
            starves: 1,
            trim_ppm: -1200,
        });
        let health = audio::StreamHealth {
            starves: 2,
            starve_frames: 900,
            unrouted_frames: 0,
        };
        let line = super::audio_line(5.0, &now, health, Some(&(then, Default::default())), 3);
        assert_eq!(
            line,
            "audio: 5.0s fill 48ms (30-70) target 75ms trim -1200ppm | dropouts +1 | dev underruns +2 (512 frames) | dropped +0 | late wakes +1 (max gap 21.5ms) | stream starves +2 (900 frames) unrouted +0 frames | callbacks waiting max 3 | rendered 100% of real time"
        );
    }

    #[test]
    fn completion_dpc_deadlines_fall_on_the_output_grid() {
        // kernel::periodic's deadline n: ceil(n * 1e7 * den / num) ticks
        // after the origin; the grid's due frames: floor(ticks * rate / 1e7).
        let r = super::COMPLETION_RATE;
        let q = dsound::output::QUANTUM;
        for n in [1u64, 2, 3, 187, 188, 1_000_003] {
            let ticks = (n * 10_000_000 * r.den).div_ceil(r.num);
            assert_eq!(
                ticks * super::RATE / 10_000_000 / q,
                n,
                "deadline {n} makes exactly quantum {n} due"
            );
        }
    }

    #[test]
    fn one_quantum_tick_step_makes_a_quantum_due() {
        // dsound's due_frames: floor(ticks * rate / 1e7).
        let due = |t: u64| t * super::RATE / 10_000_000;
        for now in [0u64, 1, 53_333, 1_234_567, 99_999_999] {
            assert!(
                due(now + super::QUANTUM_TICKS) >= due(now) + dsound::output::QUANTUM,
                "{now}"
            );
        }
    }
}
