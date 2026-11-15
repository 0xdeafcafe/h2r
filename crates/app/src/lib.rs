//! The host shell the recompiled game runs inside (see docs/app.md).
//!
//! [`run`] owns the macOS main thread: it opens the window, creates the wgpu
//! device on a Metal surface, opens the audio device, then starts the guest
//! on a worker thread with a [`GuestHost`]. The guest side uses the host
//! for everything platform-shaped:
//!
//! * [`GuestHost::gfx_context`] is the device to build `d3d::GfxBackend` on,
//!   and [`GuestHost::present_hook`] the closure for `GfxBackend::on_present`;
//! * [`GuestHost::take_input`] is the `InputManager` (keyboard, mouse and
//!   pads) to build `xapi::Xapi` on, and [`pump_input`] runs one input frame;
//! * [`GuestHost::mixer`] is the `audio::Mixer` the output device plays, to
//!   build `dsound::DirectSound` on;
//! * [`GuestHost::tick`] paces guest time to the wall clock, blocks while
//!   paused and says when to stop.
//!
//! [`GuestHost::headless`] gives the same interface with no window, so the
//! guest-side code is the same for `h2 run --headless` and tests.

pub mod audio_out;
pub mod frame;
pub mod input_pump;
pub mod keymap;
pub mod lifecycle;
pub mod mailbox;
pub mod pacing;
pub mod scale;
mod shell;
pub mod worker;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gfx::GfxContext;
use gfx::surface::{SurfaceId, SurfaceStore};
use input::InputManager;
use input::backend::keyboard::KeyboardMouse;
use input::backend::scripted::ScriptedInput;
use input::bindings::Bindings;

pub use frame::{Compositor, Filter, FrameError, FrameSink, FrameSource, FrameStats, ShownFrame};
pub use input_pump::{PumpReport, pump_input};
pub use lifecycle::{Control, Lifecycle};
pub use pacing::{PaceAction, PacePolicy, PaceStats, Pacer, VBlank, WindowedRate};
pub use scale::{DisplayAspect, Scaling, Viewport};

/// Shell settings.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Window title (the shell appends status).
    pub title: String,
    /// Initial inner size in logical points.
    pub window_size: (u32, u32),
    /// Start in borderless fullscreen.
    pub fullscreen: bool,
    /// Displayed shape of the guest frame.
    pub aspect: DisplayAspect,
    pub scaling: Scaling,
    pub filter: Filter,
    /// Wait for the display's vblank when presenting (`Fifo`). Off uses
    /// `AutoNoVsync` (tearing allowed); guest pacing is unaffected either way.
    pub vsync: bool,
    /// Pause the guest (and audio) while the window is not focused.
    pub pause_on_focus_loss: bool,
    /// Click to capture the mouse for look; Cmd or focus loss releases it.
    pub mouse_capture: bool,
    /// Add physical controllers (GameController on macOS, gilrs elsewhere).
    pub gamepads: bool,
    /// Keyboard/mouse shares player one with the first physical pad.
    pub keyboard: bool,
    /// Keyboard and mouse bindings (and stick/trigger deadzones).
    pub bindings: Bindings,
    /// Exclusive timeline source, replacing all live backends. Retain a clone
    /// and call advance_to(guest_ticks) before pump_input. Polls never advance it.
    pub scripted_input: Option<ScriptedInput>,
    /// Open the default audio device. Off (or no device) still gives the
    /// guest a mixer, at [`audio_out::FALLBACK_RATE`].
    pub audio: bool,
    pub pacing: PacePolicy,
    /// When the guest thread returns: keep the window up with the last frame
    /// and the outcome in the title (true), or close.
    pub keep_open_on_guest_exit: bool,
    /// How long a close waits for the guest thread to notice the stop request.
    pub shutdown_timeout: Duration,
    /// Print the window's status line to stderr once a second.
    pub log_status: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            title: "h2recomp".into(),
            window_size: (1280, 960),
            fullscreen: false,
            aspect: DisplayAspect::Standard,
            scaling: Scaling::Fit,
            filter: Filter::Linear,
            vsync: true,
            pause_on_focus_loss: false,
            mouse_capture: true,
            gamepads: true,
            keyboard: true,
            bindings: Bindings::default(),
            scripted_input: None,
            audio: true,
            pacing: PacePolicy::default(),
            keep_open_on_guest_exit: true,
            shutdown_timeout: Duration::from_secs(3),
            log_status: false,
        }
    }
}

/// How the guest thread ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuestExit {
    /// The guest function returned `Ok`.
    Returned,
    /// It returned an error (formatted with `{:#}`).
    Failed(String),
    /// It panicked.
    Panicked(String),
    /// It did not stop within `shutdown_timeout` of the close (the process
    /// exiting will end it).
    StillRunning,
    /// The window never came up, so it never started.
    NotStarted,
}

/// What [`run`] returns once the window closes.
#[derive(Clone, Debug)]
pub struct ExitReport {
    pub guest: GuestExit,
    pub frames: FrameStats,
    pub pacing: PaceStats,
}

/// The guest thread's view of the host. `Send`; moved into the guest thread.
pub struct GuestHost {
    gfx: GfxContext,
    sink: FrameSink,
    input: Option<InputManager>,
    keyboard: Option<KeyboardMouse>,
    mixer: Arc<Mutex<audio::Mixer>>,
    sample_rate: u32,
    life: Lifecycle,
    pacer: Pacer,
    epoch: Instant,
    pace_stats: Arc<Mutex<PaceStats>>,
    /// Keeps a headless channel's receiving end alive.
    _source: Option<FrameSource>,
    /// Consumes a headless host's mixer in real time (the window's host
    /// leaves that to the shell's `AudioOut`).
    _discard: Option<audio_out::DiscardRenderer>,
}

impl GuestHost {
    /// A host with no window: frames are accepted and counted, audio goes to
    /// a mixer consumed in real time by a `DiscardRenderer`, input has only the backends `config` enables
    /// (`keyboard` supplies the player-one fallback; `gamepads` reads real pads).
    /// `scripted_input` replaces both with the runner-controlled timeline.
    pub fn headless(config: &AppConfig) -> Result<Self, gfx::GfxError> {
        let ctx = GfxContext::headless()?;
        let (sink, source) = frame::channel(&ctx);
        let (input, keyboard) = build_input(config);
        let mixer = Arc::new(Mutex::new(audio::Mixer::new(audio_out::FALLBACK_RATE)));
        let life = Lifecycle::new();
        Ok(Self {
            gfx: ctx,
            sink,
            input: Some(input),
            keyboard,
            _discard: Some(audio_out::DiscardRenderer::start(
                Arc::clone(&mixer),
                audio_out::FALLBACK_RATE,
                Some(life.clone()),
            )),
            mixer,
            sample_rate: audio_out::FALLBACK_RATE,
            life,
            pacer: Pacer::new(config.pacing),
            epoch: Instant::now(),
            pace_stats: Arc::default(),
            _source: Some(source),
        })
    }

    /// The device every guest GPU object must live on: build
    /// `d3d::GfxBackend::new(host.gfx_context())` from it. Each call returns
    /// another handle to the same instance, adapter, device and queue.
    pub fn gfx_context(&self) -> GfxContext {
        share_context(&self.gfx)
    }

    /// The closure for `d3d::GfxBackend::on_present`. Each call makes a new
    /// sink on the same channel.
    pub fn present_hook(
        &self,
    ) -> impl FnMut(&GfxContext, &SurfaceStore, SurfaceId, u32, Option<&[u8; 768]>) + Send + 'static {
        self.sink.another().into_present_hook()
    }

    /// A sink for frames that are not gfx surfaces (CPU pixels, other views).
    pub fn frame_sink(&self) -> FrameSink {
        self.sink.another()
    }

    /// The input manager, once. Build `xapi::Xapi::new(config, input, storage)`
    /// on it and run [`pump_input`] once per guest frame.
    pub fn take_input(&mut self) -> Option<InputManager> {
        self.input.take()
    }

    /// The keyboard pad's handle, for code that wants to inject keys
    /// (tests, scripted input). The window feeds it already.
    pub fn keyboard(&self) -> Option<&KeyboardMouse> {
        self.keyboard.as_ref()
    }

    /// The mixer the audio device renders: `dsound::DirectSound::new(mixer,
    /// layout)`. Created at the device rate, so no host-side resampling.
    pub fn mixer(&self) -> Arc<Mutex<audio::Mixer>> {
        Arc::clone(&self.mixer)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Stop and pause flags, for checks from inside overrides.
    pub fn lifecycle(&self) -> Lifecycle {
        self.life.clone()
    }

    pub fn should_stop(&self) -> bool {
        self.life.should_stop()
    }

    /// Call between scheduler slices with the kernel's logical clock
    /// (`kernel.clock().counter()`, 100 ns ticks). Blocks while paused,
    /// sleeps while guest time is ahead of wall time, and returns
    /// [`Control::Stop`] once the window has asked the guest to stop.
    pub fn tick(&mut self, guest_ticks: u64) -> Control {
        loop {
            let (waited, c) = self.life.wait_while_paused();
            if c == Control::Stop {
                return c;
            }
            if waited {
                // Wall time ran on while paused; guest time did not.
                self.pacer.reset();
            }
            let action = self.pacer.decide(guest_ticks, self.epoch.elapsed());
            *self.pace_stats.lock().unwrap_or_else(|e| e.into_inner()) = self.pacer.stats();
            if let PaceAction::Sleep(d) = action
                && self.life.sleep(d) == Control::Stop
            {
                return Control::Stop;
            }
            // A pause that arrived during the pacing sleep (which ends early
            // on pause) is honoured before the guest runs again.
            if self.life.is_paused() {
                continue;
            }
            return if self.life.should_stop() {
                Control::Stop
            } else {
                Control::Continue
            };
        }
    }

    /// Pacing counters so far.
    pub fn pace_stats(&self) -> PaceStats {
        self.pacer.stats()
    }

    /// Handoff counters so far.
    pub fn frame_stats(&self) -> FrameStats {
        self.sink.stats()
    }
}

/// Another handle on the same wgpu objects (all wgpu handles are shared
/// references).
pub fn share_context(ctx: &GfxContext) -> GfxContext {
    GfxContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        caps: ctx.caps,
    }
}

/// The input manager the guest gets, and the keyboard handle the window
/// keeps. The keyboard and first physical pad share port 0.
fn build_input(config: &AppConfig) -> (InputManager, Option<KeyboardMouse>) {
    let convert = match config.bindings.convert.validate() {
        Ok(()) => config.bindings.convert,
        Err(e) => {
            eprintln!("app: invalid deadzones in bindings ({e}); using defaults");
            Default::default()
        }
    };
    let mut manager = InputManager::new(convert).expect("validated deadzones");
    if let Some(scripted) = &config.scripted_input {
        manager.add_backend(Box::new(scripted.clone()));
        return (manager, None);
    }
    let physical = config.gamepads.then(gamepad_backend).flatten();
    let keyboard = if config.keyboard {
        let kb = KeyboardMouse::new(config.bindings.clone());
        manager.add_backend(Box::new(input::backend::fallback::KeyboardFallback::new(
            kb.clone(),
            physical,
            convert,
        )));
        Some(kb)
    } else {
        if let Some(physical) = physical {
            manager.add_backend(physical);
        }
        None
    };
    (manager, keyboard)
}

#[cfg(all(feature = "gamepads", target_os = "macos"))]
fn gamepad_backend() -> Option<Box<dyn input::HostBackend>> {
    Some(Box::new(
        input::backend::game_controller::GameControllerPads::new(),
    ))
}

#[cfg(all(feature = "gamepads", not(target_os = "macos")))]
fn gamepad_backend() -> Option<Box<dyn input::HostBackend>> {
    match input::backend::gilrs_pads::GilrsPads::new() {
        Ok(p) => Some(Box::new(p)),
        Err(e) => {
            eprintln!("app: gamepads unavailable: {e}");
            None
        }
    }
}

#[cfg(not(feature = "gamepads"))]
fn gamepad_backend() -> Option<Box<dyn input::HostBackend>> {
    eprintln!("app: built without the `gamepads` feature; keyboard only");
    None
}

/// Opens the window and runs `guest` on a worker thread until the window
/// closes. Must be called on the main thread (macOS UI rule; winit returns
/// an error otherwise). Returns after the window has closed and the guest
/// thread has stopped or timed out.
pub fn run<F>(config: AppConfig, guest: F) -> anyhow::Result<ExitReport>
where
    F: FnOnce(GuestHost) -> anyhow::Result<()> + Send + 'static,
{
    shell::run(config, Box::new(guest))
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use input::PortIndex;
    use input::backend::scripted::Timeline;

    #[test]
    fn timeline_replaces_live_input_and_advances_only_on_runner_clock() {
        let script = Timeline::from_toml(include_str!("../../../docs/input-menu-example.toml"))
            .unwrap()
            .playback();
        let config = AppConfig {
            scripted_input: Some(script.clone()),
            ..Default::default()
        };
        // No GPU, audio device, window or physical controller is needed.
        let (mut input, keyboard) = build_input(&config);
        assert!(keyboard.is_none(), "scripts replace live sources");
        input.sync();
        let h = input.open(PortIndex::new(0).unwrap()).unwrap();
        assert_eq!(input.connected_mask(), 1);
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 0);
        script.advance_to(10_000_000).unwrap();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 255);
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 255);
        script.advance_to(11_000_000).unwrap();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.analog[0], 0);
        script.advance_to(12_000_000).unwrap();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad.buttons.0, 2);
    }

    #[test]
    fn documented_bindings_drive_all_fallback_inputs_and_release_on_focus_loss() {
        let config = AppConfig {
            gamepads: false,
            bindings: Bindings::from_toml(include_str!("../../../docs/input-default.toml"))
                .unwrap(),
            ..Default::default()
        };
        let (mut input, keyboard) = build_input(&config);
        let keyboard = keyboard.unwrap();
        keyboard.key_down("Space");
        keyboard.key_down("W");
        keyboard.mouse_button(input::bindings::MouseButton::Left, true);
        keyboard.mouse_motion(10.0, -10.0);
        input.sync();
        let h = input.open(PortIndex::new(0).unwrap()).unwrap();
        let pad = input.get_state(h).unwrap().gamepad;
        assert_eq!(pad.analog[0], 255);
        assert_eq!(pad.analog[7], 255);
        assert_eq!(pad.thumb_ly, i16::MAX);
        assert!(pad.thumb_rx > 0 && pad.thumb_ry > 0);
        keyboard.release_all();
        input.sync();
        assert_eq!(input.get_state(h).unwrap().gamepad, Default::default());
        assert_eq!(input.capabilities(h).unwrap().output.left_motor, u16::MAX);
    }
}
