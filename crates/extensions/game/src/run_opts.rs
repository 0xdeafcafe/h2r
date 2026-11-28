//! What a run is given and what it ends with: the options, the host it
//! runs in, and the report. `boot`'s `run` and `run_hosted` take them.

use std::path::PathBuf;
use std::sync::Arc;

/// Options for `boot::run`.
#[derive(Debug, Clone)]
pub struct RunOpts {
    /// The XISO, mounted as the DVD drive.
    pub disc: PathBuf,
    /// Host root for the hard-disk partitions (E:, cache, utility).
    pub data_root: PathBuf,
    pub max_slices: u64,
    /// Register crates/platform/xapi (`xapi::register`) and pump its input.
    pub xapi: bool,
    pub transfers_per_slice: u64,
    /// Install our kernel stopgaps (stopgaps.rs).
    pub stopgaps: bool,
    /// Kernel-call trace output (TSV), see trace.rs.
    pub trace: Option<PathBuf>,
    /// Override the guest's D3D entries and frame wait (`render_glue`).
    pub d3d: bool,
    /// Register crates/platform/bink-hle: playback of the movies installed in
    /// `movies`, skip mode for the rest (movies_glue.rs).
    pub bink: bool,
    /// Register crates/platform/dsound's overrides (headless mixer).
    pub dsound: bool,
    /// Which movies may play: those `h2 movies` installed and current for
    /// the disc (`movies_glue::Movies`; level boots and headless runs skip
    /// by default, `--no-movies` always).
    pub movies: crate::movies_glue::Movies,
}

/// What a run ended with, for callers and docs.
pub struct RunReport {
    pub outcome: String,
    pub stop: String,
    pub eip: u32,
    pub kernel_log: Vec<String>,
}

/// What the run is hosted in. The default is headless: no renderer,
/// logical-time audio into a `NullSink`, a keyboard pad
/// only `H2_BOOT_INPUT` presses, every kernel call kept in memory, and
/// `max_slices` as the only stop. A windowed host (`h2 play`, crates/apps/app)
/// fills these in; see docs/runtime/BOOT.md "Hosted runs".
#[derive(Default)]
pub struct Host {
    /// Record this boot to a new film file. Falls back to H2_FILM_OUT.
    pub film_out: Option<PathBuf>,
    /// A player session with no `film_out` records into the film library
    /// (`theater::autorecord`). `h2 play` sets it; other hosts leave it off.
    pub film_auto: crate::theater::autorecord::Request,
    /// Local media controls and the offline clip sink belong to separate players.
    pub film_media: Option<crate::theater::Media>,
    pub film_capture: Option<Arc<crate::theater::ClipCapture>>,
    /// Dedicated System Link profile, with no screen or speakers.
    pub host_light: bool,
    /// Opt in to bot support on a light host.
    pub host_bots: bool,
    #[cfg(feature = "gpu")]
    /// The device the renderer is built on (`crate::render_glue::install_game`).
    pub gfx: Option<gfx::GfxContext>,
    #[cfg(feature = "gpu")]
    /// The window the frame owner presents on. With none, no renderer is
    /// built.
    pub presenter: Option<gfx::Presenter>,
    /// Independent desktop observer for re-simulated playerless films.
    #[cfg(feature = "gpu")]
    pub film_view: Option<crate::theater::HostView>,
    #[cfg(feature = "gpu")]
    /// The display backend and the current physical size for Native resolution.
    /// The first backend keeps the guest's render target dimensions.
    pub gfx_view: Option<DisplayView>,
    #[cfg(feature = "gpu")]
    /// The window runs unpaced: every sealed scene must reach the viewer.
    pub unpaced_display: bool,
    #[cfg(feature = "local-io")]
    /// Where the logical-time audio output plays (an audio device), instead
    /// of a `NullSink` (`dsound_glue::install_with_sink`).
    pub audio_sink: Option<crate::dsound_glue::SinkOpener>,
    #[cfg(feature = "local-io")]
    /// Opens the host's microphone and voice output for voice chat
    /// (`voice_glue`, docs/live/live-voice.md). `None`: the communicator
    /// still works when a microphone is chosen, with a silent microphone.
    pub voice_io: Option<voice::io::OpenIo>,
    #[cfg(feature = "local-io")]
    /// The controller speaker (docs/audio/playstation-audio.md): a second sink
    /// for the sounds its routes pick, and the sound index built from each
    /// map the game opens. Needs `audio_sink`'s logical output; ignored
    /// without DirectSound.
    pub pad_speaker: Option<crate::dsound_glue::PadSpeaker>,
    /// Our settings (`setting_defs`), behind the Controller Speaker row on
    /// Controls and the Video screen's rows (`video_screen::model`), and
    /// read by the camera hook (`fov_row`). `None` (headless runs) leaves
    /// the Controls screen and the camera as retail.
    pub settings: Option<Arc<settings::Store>>,
    /// XAPI's input manager (`xapi_glue::register_with`).
    pub input: Option<input::InputManager>,
    /// The keyboard pad `H2_BOOT_INPUT` presses, when `input` is given.
    #[cfg(feature = "desktop-runtime")]
    pub keyboard: Option<input::backend::keyboard::KeyboardMouse>,
    /// Button icons for the device in use (`icons`, docs/input/button-icons.md).
    /// `None` takes `H2_ICONS`; with that unset too, the fonts and the glyph
    /// lookups are left alone.
    #[cfg(feature = "desktop-runtime")]
    pub icons: Option<crate::icons::IconOptions>,
    /// Keep only this many kernel calls in memory (`trace::install_bounded`).
    pub trace_limit: Option<usize>,
    /// Start this campaign level in place of the main menu (`level_boot`,
    /// docs/runtime/level-boot.md). `None` reads `H2_LEVEL` and `H2_DIFFICULTY`.
    pub level: Option<crate::level_boot::Request>,
    /// Receives the kernel clock after servicing a slice and after an audio-only
    /// idle advance, before the scheduler retries. Returning false ends the run
    /// (outcome `HostStop`).
    pub between_slices: Option<BetweenSlices>,
    /// Switch the window to its loading picture before the game rebuilds the
    /// main menu. Headless and exact film runs have no loading picture.
    #[cfg(feature = "gpu")]
    pub return_to_menu: Option<crate::return_to_menu::LoadingScreen>,
    /// Our debug menu (docs/runtime/debug-menu.md): its calls go to the
    /// agent (`H2_AGENT`'s, else a local one with no socket), and it gets
    /// the menu's host methods (`settings.*`, `perf.overlay`, `cheat.*`).
    pub debug_menu: Option<debug_menu::Shared>,
    /// Desktop Theater controller and the film library opener supplied by the host.
    pub theater: Option<(crate::theater_ui::Theater, crate::theater_ui::adapter::OpenLibrary)>,
    /// The isolated player's host-rendered pages and input model.
    pub theater_playback: Option<crate::theater_ui::Theater>,
    /// Open the agent socket in the agents directory when `H2_AGENT` is
    /// unset, so editors and agents find the game
    /// ([`crate::agent_glue::discoverable_socket`]; `H2_AGENT=off` keeps
    /// it closed). `h2 play` sets it.
    pub discoverable_agent: bool,
    /// The host runs the Live service in this process (`h2 play --live-local`).
    pub embedded_live: bool,
    /// The perf overlay's on/off switch, for `perf.overlay`.
    pub perf_overlay: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The watermark's active-cheats line, which the debug menu glue keeps
    /// filled from `cheat.list` (docs/runtime/debug-menu.md).
    pub cheat_line: Option<Arc<std::sync::Mutex<Vec<String>>>>,
    /// The watermark's `full` readout (`H2_WATERMARK=full`): its level
    /// switch and the lines the game fills ([`crate::debug_readout`]).
    pub readout: Option<crate::debug_readout::Link>,
    /// This start's game code (tier, module, background build), for
    /// `game.code` (crate::game_code).
    pub game_code: Option<Arc<crate::game_code::GameCode>>,
    /// Shared with the registered debug-view world-line producer on a windowed run.
    pub debug_views: Option<Arc<agent::debug_views::DebugViews>>,
    /// The window consumer for the paired-editor plate and focus requests.
    pub editor_link: Option<agent::editor::EditorLink>,
    #[cfg(feature = "gpu")]
    #[doc(hidden)]
    pub world_line_sources: Vec<Arc<dyn crate::world_lines::WorldLineSource>>,
    /// Called once with the kernel's VFS when it is attached, before the
    /// game runs: `h2 play` watches U: through it for cloud saves
    /// (`vfs::Vfs::set_change_observer`, docs/live/live-cloud-saves.md).
    pub on_vfs: Option<VfsReady>,
}

impl Host {
    /// Registers a window-overlay producer before boot. Keep the source's
    /// active state in an atomic so inactive sources make no capture calls
    /// or allocations. Capture only reads shared host state and the
    /// render owner's guest view. The window
    /// consumes the owned sample later, even when it re-shows an image.
    #[cfg(feature = "gpu")]
    pub fn add_world_line_source(&mut self, source: Arc<dyn crate::world_lines::WorldLineSource>) {
        self.world_line_sources.push(source);
    }
}

/// [`Host::on_vfs`].
pub type VfsReady = Box<dyn FnOnce(&Arc<std::sync::Mutex<vfs::Vfs>>) + Send>;

/// [`Host::between_slices`]: true to keep running.
pub type BetweenSlices = Box<dyn FnMut(&kernel::Kernel) -> bool>;

/// A live extent in render or display pixels.
pub type NativeSize = Arc<dyn Fn() -> (u32, u32) + Send + Sync>;

/// The host's physical drawable, fitted viewport and fullscreen mode.
pub type NativeTarget = Arc<dyn Fn() -> settings::NativeTarget + Send + Sync>;

#[cfg(feature = "gpu")]
pub struct DisplayView {
    pub native_target: NativeTarget,
    /// Playback uses the viewer's chosen raster while the guest uses its recorded settings.
    pub viewer_output: Option<NativeSize>,
    /// Theater display geometry follows the viewer’s Video setting.
    pub viewer_lod: Option<settings::Setting<u32>>,
    /// Physical presentation rectangle for an internal render resolution.
    pub display_size: Arc<dyn Fn((u32, u32)) -> (u32, u32) + Send + Sync>,
}
