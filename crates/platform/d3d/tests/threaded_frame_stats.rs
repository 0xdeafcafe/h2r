//! CPU-only proof that queued backend telemetry reaches the same CSV frame
//! as the device's Swap, including delayed worker completion and reply waits.
mod common;

use std::sync::mpsc;
use std::time::Duration;

use d3d::backend::{DrawOutcome, ResolvedDraw};
use d3d::{Backend, ThreadedBackend, UploadPolicy};
use gfx::texture::DecodedTexture;
use gfx::{Clear, D3DFormat};
use timeline::phase::{self, Counter, Phase};

#[derive(Default)]
struct MeteredBackend {
    next: u32,
    clears: u64,
    gate: Option<mpsc::Receiver<()>>,
}

impl Backend for MeteredBackend {
    type Surface = u32;
    type Texture = u32;

    fn create_surface(&mut self, _: D3DFormat, _: u32, _: u32, _: bool) -> Result<u32, String> {
        self.next += 1;
        Ok(self.next)
    }
    fn destroy_surface(&mut self, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn upload_texture(&mut self, _: &DecodedTexture) -> Result<u32, String> {
        Ok(1)
    }
    fn bc_supported(&self) -> bool {
        false
    }
    fn clear(&mut self, _: u32, _: Option<u32>, _: Clear) -> Result<(), String> {
        if let Some(gate) = self.gate.take() {
            gate.recv().unwrap();
        }
        self.clears += 1;
        {
            let _encode = phase::enter(Phase::Encode);
            for c in Counter::ALL
                .into_iter()
                .filter(|c| *c != Counter::ReadBytes)
            {
                phase::count(c, self.clears);
            }
            {
                use timeline::native::{Metric, Stage, Timer};
                let _prep = Timer::start(Stage::World, Metric::PrepareNs);
                gfx::churn::buffer_created();
                {
                    let _enc = Timer::start(Stage::Objects, Metric::EncodeNs);
                    gfx::churn::upload_bytes(32 * self.clears as usize);
                }
                gfx::churn::upload_bytes(8);
                Stage::World.attempted(1, true, 2);
                Stage::World.invariant(1, true);
            }
            std::thread::sleep(Duration::from_millis(1));
            let _submit = phase::enter(Phase::Submit);
            std::thread::sleep(Duration::from_millis(1));
        }
        let _wait = phase::enter(Phase::Wait);
        std::thread::sleep(Duration::from_millis(1));
        Ok(())
    }
    fn draw(
        &mut self,
        _: u32,
        _: Option<u32>,
        _: &ResolvedDraw,
        _: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        Ok(DrawOutcome::Drawn)
    }
    fn present(&mut self, _: u32, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn copy_surface(&mut self, _: u32, _: u32) -> Result<bool, String> {
        Ok(true)
    }
}

#[test]
fn queued_frame_stats_and_game_thread_waits_reach_csv() {
    let (mut d, mut m) = common::create(
        ThreadedBackend::new(MeteredBackend::default()).unwrap(),
        UploadPolicy::Tracked,
    );
    d.backend.finish().unwrap();
    let (release, blocked) = mpsc::channel();
    d.backend
        .with_inner(move |b| {
            b.clears = 0;
            b.gate = Some(blocked);
        })
        .unwrap();
    let csv = std::env::temp_dir().join(format!("h2-threaded-stats-{}.csv", std::process::id()));
    phase::start(phase::Config {
        csv: Some(csv.clone()),
        ..Default::default()
    })
    .unwrap();
    let first = timeline::frames() + 1;
    d.clear(&mut m, 0, 0, 0xF0, 0xff00_0000, 1.0, 0).unwrap();
    phase::count(Counter::ReadBytes, 123);
    d.swap(&mut m, 0).unwrap();
    // Swap returned while the first worker command is still blocked. No
    // partial row is published with zeros or credited to a later frame.
    assert!(phase::window_summary().is_none());
    release.send(()).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xff00_0000, 1.0, 0).unwrap();
    d.backend.finish().unwrap();
    phase::count(Counter::ReadBytes, 456);
    d.swap(&mut m, 0).unwrap();
    d.backend.finish().unwrap();
    let report = phase::stop().unwrap();
    assert!(!report.contains("incomplete"), "{report}");
    let text = std::fs::read_to_string(&csv).unwrap();
    std::fs::remove_file(&csv).unwrap();
    let mut lines = text.lines();
    let header: Vec<_> = lines.next().unwrap().split(',').collect();
    // The last field is a quoted material report that may contain commas.
    assert_eq!(header.last().copied(), Some("native_world_opaque_materials"));
    let rows: Vec<Vec<_>> = lines
        .map(|line| line.splitn(header.len(), ',').collect())
        .collect();
    assert_eq!(rows.len(), 2, "{text}");
    let col = |name: &str| header.iter().position(|c| *c == name).unwrap();
    let value = |row: &[&str], name: &str| {
        row[col(name)].parse::<u64>().unwrap_or_else(|err| panic!("{name}: {err}: {text}"))
    };
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row.len(), header.len(), "{text}");
        assert_eq!(value(row, "frame"), first + i as u64);
        for counter in [
            "draws",
            "submits",
            "pipelines",
            "shaders",
            "uploads",
            "readbacks",
        ] {
            assert_eq!(value(row, counter), i as u64 + 1, "{counter}: {text}");
        }
        for timer in ["encode_us", "submit_us", "gpu-wait_us"] {
            assert!(value(row, timer) >= 1000, "{timer}: {text}");
        }
        assert_eq!(value(row, "read_bytes"), [123, 456][i]);
        assert_eq!(value(row, "native_world_upload_bytes"), 8);
        assert_eq!(value(row, "native_world_buffers"), 1);
        assert_eq!(value(row, "native_objects_upload_bytes"), (i as u64 + 1) * 32);
        // The backend prepares on the render thread, so its time is charged
        // to render_prepare_ns; prepare_ns is the game thread's alone.
        assert!(value(row, "native_world_render_prepare_ns") > 0);
        assert_eq!(value(row, "native_world_prepare_ns"), 0);
        assert!(value(row, "native_objects_encode_ns") > 0);
        assert_eq!(value(row, "native_world_v1_display_recorded"), 2);
        assert_eq!(value(row, "native_world_v1_display_ranges"), 0);
        assert_eq!(value(row, "native_world_v1_display_invariants"), 1);
        assert_eq!(value(row, "native_world_v1_reference_invariants"), 0);
    }
    assert!(value(&rows[1], "render-wait_us") > 0, "{text}");
    // Game-thread self time still accounts for its whole wall interval;
    // parallel render times are extra observations, not subtracted twice.
    for row in &rows {
        let self_us: u64 = ["cpu_us", "decode_us", "pace_us", "render-wait_us"]
            .map(|c| value(row, c))
            .iter()
            .sum();
        assert!(value(row, "total_us").abs_diff(self_us) <= 4, "{text}");
    }

    // Stopping with a queued boundary reports an incomplete tail, and a
    // later recording must not inherit that old frame's counters.
    let (release, blocked) = mpsc::channel();
    d.backend
        .with_inner(move |b| b.gate = Some(blocked))
        .unwrap();
    phase::start(phase::Config::default()).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xff00_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 0).unwrap();
    assert!(phase::stop().unwrap().contains("1 incomplete"));
    phase::start(phase::Config {
        csv: Some(csv.clone()),
        ..Default::default()
    })
    .unwrap();
    release.send(()).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0xff00_0000, 1.0, 0).unwrap();
    d.swap(&mut m, 0).unwrap();
    d.backend.finish().unwrap();
    let report = phase::stop().unwrap();
    assert!(!report.contains("incomplete"), "{report}");
    let text = std::fs::read_to_string(&csv).unwrap();
    std::fs::remove_file(&csv).unwrap();
    let rows: Vec<_> = text.lines().collect();
    assert_eq!(rows.len(), 2, "{text}");
    let fields: Vec<_> = rows[1].splitn(header.len(), ',').collect();
    assert_eq!(fields.len(), header.len(), "{text}");
    assert_eq!(fields[col("draws")], "4", "old-session work leaked: {text}");
}
