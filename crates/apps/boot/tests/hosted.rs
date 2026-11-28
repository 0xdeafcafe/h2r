//! `run_hosted`: the host's input goes to XAPI, the logical-time audio
//! output plays into the host's sink, the between-slices callback runs once
//! per slice with the kernel, and returning false ends the run as
//! `HostStop`.
//!
//! Needs the disc and a built dylib, so it is ignored by default:
//!
//! ```sh
//! H2_XBE=/abs/disc/default.xbe H2_DISC=/abs/disc/halo2.iso \
//! H2_BOOT_DYLIB=$TMPDIR/h2boot/libh2game.dylib \
//!   cargo test -p boot --test hosted -- --ignored
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A sink that counts what the output thread wrote and on which thread it
/// was opened.
struct Counting {
    written: Arc<AtomicU64>,
}

impl audio::AudioSink for Counting {
    fn sample_rate(&self) -> u32 {
        44_100
    }
    fn queued_frames(&self) -> usize {
        0
    }
    fn free_frames(&self) -> usize {
        1 << 16
    }
    fn write(&mut self, samples: &[f32]) -> usize {
        let frames = samples.len() / 2;
        self.written.fetch_add(frames as u64, Ordering::Relaxed);
        frames
    }
    fn stats(&self) -> audio::SinkStats {
        Default::default()
    }
}

#[test]
#[ignore = "needs $H2_XBE, $H2_DISC and $H2_BOOT_DYLIB"]
fn the_host_paces_feeds_and_stops_the_run() {
    let var = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .unwrap_or_else(|| panic!("{k} is not set"))
    };
    let xbe = std::fs::read(var("H2_XBE")).unwrap();
    let dylib = var("H2_BOOT_DYLIB");
    let data = std::env::temp_dir().join(format!("boot-hosted-test-{}", std::process::id()));
    let opts = boot::RunOpts {
        disc: var("H2_DISC"),
        data_root: data.clone(),
        max_slices: 1_000_000,
        xapi: true,
        transfers_per_slice: 50_000_000,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        movies: boot::movies_glue::Movies::Off,
    };
    let (input, keyboard) =
        input::InputManager::with_keyboard_pad(input::bindings::Bindings::default()).unwrap();
    let slices = Arc::new(AtomicU64::new(0));
    let clock = Arc::new(AtomicU64::new(0));
    let opened = Arc::new(AtomicU64::new(0));
    let written = Arc::new(AtomicU64::new(0));
    let main_thread = std::thread::current().id();
    let host = boot::Host {
        gfx: None,
        gfx_view: None,
        presenter: None,
        host_light: false,
        host_bots: false,
        voice_io: None,
        audio_sink: Some(Box::new({
            let (opened, written) = (Arc::clone(&opened), Arc::clone(&written));
            move || -> Box<dyn audio::AudioSink> {
                // DeviceSink is not Send: the sink must be opened on the
                // output thread, not the runner's.
                assert_ne!(std::thread::current().id(), main_thread);
                opened.fetch_add(1, Ordering::Relaxed);
                Box::new(Counting { written })
            }
        })),
        pad_speaker: None,
        debug_menu: None,
        perf_overlay: None,
        cheat_line: None,
        readout: None,
        game_code: None,
        discoverable_agent: false,
        settings: None,
        input: Some(input),
        keyboard: Some(keyboard),
        icons: None,
        trace_limit: Some(16),
        level: None,
        between_slices: Some(Box::new({
            let (slices, clock) = (Arc::clone(&slices), Arc::clone(&clock));
            move |k: &kernel::Kernel| {
                let n = slices.fetch_add(1, Ordering::Relaxed) + 1;
                let now = k.clock().counter();
                assert!(
                    now >= clock.swap(now, Ordering::Relaxed),
                    "the logical clock went backwards"
                );
                n < 400
            }
        })),
        on_vfs: None,
        ..boot::Host::default()
    };
    let r = boot::run_hosted(&xbe, &dylib, &opts, host).unwrap();
    let _ = std::fs::remove_dir_all(&data);
    assert_eq!(r.outcome, "HostStop");
    assert_eq!(slices.load(Ordering::Relaxed), 400);
    let ticks = clock.load(Ordering::Relaxed);
    assert!(ticks > 0, "guest time never moved");
    assert_eq!(opened.load(Ordering::Relaxed), 1);
    // Logical pacing renders on the guest's clock (48 kHz, resampled to the
    // sink's 44.1 kHz); allow for the queue and the last quantum.
    let due = ticks * 44_100 / 10_000_000;
    let got = written.load(Ordering::Relaxed);
    assert!(got > 0 && got <= due + 4096, "wrote {got} frames for {due} due");
}
