//! Perf: the overlay's switch (`perf.overlay`), the frame-time window
//! (`profile.counters`) in readable units, and a ten second capture
//! (`profile.capture`).
//!
//! `profile.counters` answers `{frames, fps, p50_ms, p99_ms, max_ms,
//! phase_ms {name: ms}, counts {name: per frame}}`; the five biggest
//! phases are listed by name, leaving out frame pacing (the wait for the
//! next frame, not work).

use serde_json::{Value, json};

use super::method::{CAPTURE, COUNTERS, PERF_OVERLAY};
use super::{get, one, problem};
use crate::model::{Answer, Call, Row, Section, Setter, text};

/// The capture's length.
const CAPTURE_MS: u64 = 10_000;
/// Phases shown.
const TOP: usize = 5;

pub fn section() -> Section {
    Section {
        id: "perf".into(),
        title: "Perf".into(),
        about: "the perf overlay, frame times, what a frame spends".into(),
        calls: vec![Call::bare(PERF_OVERLAY), Call::bare(COUNTERS)],
        rows: perf_rows,
    }
}

/// A phase as a reader would say it.
pub(super) fn phase_name(k: &str) -> String {
    match k {
        "cpu" => "Game CPU".into(),
        "decode" => "Push-buffer decode".into(),
        "encode" => "GPU encode".into(),
        "submit" => "GPU submit".into(),
        "gpu-wait" => "GPU wait".into(),
        "pace" => "Frame pacing".into(),
        "render-wait" => "Render thread wait".into(),
        "d3d" => "Direct3D".into(),
        "render" => "Render front end".into(),
        other => super::humanize(other),
    }
}

fn perf_rows(a: &[Answer]) -> Vec<Row> {
    let mut out = Vec::new();
    match get(a, 0) {
        Ok(v) => out.push(
            Row::toggle(
                "Overlay (F3)",
                v["on"].as_bool().unwrap_or(false),
                Setter::new(PERF_OVERLAY, json!({}), "on"),
            )
            .about("the fps and build lines along the bottom"),
        ),
        Err(e) => out.push(problem("Overlay (F3)", e)),
    }
    let counters = get(a, 1);
    match &counters {
        Ok(v) => {
            for (label, key) in [
                ("FPS", "fps"),
                ("Frame time p50 (ms)", "p50_ms"),
                ("Frame time p99 (ms)", "p99_ms"),
                ("Slowest frame (ms)", "max_ms"),
            ] {
                if !v[key].is_null() {
                    out.push(Row::info(label, one(&v[key])));
                }
            }
            out.extend(top_phases(v));
        }
        Err(e) => out.push(problem("Frame times", e.clone())),
    }
    let capture = Row::action("Capture 10 s", Call::new(CAPTURE, json!({"ms": CAPTURE_MS})))
        .on_ok(capture_toast)
        .about("records every frame for ten seconds and writes a CSV; the page waits for it");
    out.push(match &counters {
        Err(e) => capture.disabled(e.clone()),
        Ok(_) => capture,
    });
    out
}

/// The biggest phases, by mean ms a frame.
fn top_phases(v: &Value) -> Vec<Row> {
    let Some(o) = v["phase_ms"].as_object() else {
        return Vec::new();
    };
    let mut phases: Vec<(&String, f64)> = o
        .iter()
        .filter(|(k, _)| k.as_str() != "pace")
        .filter_map(|(k, x)| Some((k, x.as_f64()?)))
        .collect();
    phases.sort_by(|a, b| b.1.total_cmp(&a.1));
    phases
        .into_iter()
        .take(TOP)
        .map(|(k, ms)| Row::info(format!("{} (ms)", phase_name(k)), format!("{ms:.1}")))
        .collect()
}

fn capture_toast(v: &Value) -> String {
    format!(
        "captured {} frames, p99 {} ms",
        text(&v["frames"]),
        one(&v["frame_ms"]["p99"])
    )
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn numbers_come_with_units_and_names() {
        let mut g = Game::default();
        let rows = page(&mut g, "perf");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Overlay (F3)",
                "FPS",
                "Frame time p50 (ms)",
                "Frame time p99 (ms)",
                "Slowest frame (ms)",
                "Game CPU (ms)",
                "GPU wait (ms)",
                "Direct3D (ms)",
                "GPU encode (ms)",
                "Render thread wait (ms)",
                "Capture 10 s",
            ]
        );
        assert_eq!(row(&rows, "FPS").shown(), "30.0");
        assert_eq!(row(&rows, "Frame time p99 (ms)").shown(), "41.3");
        assert_eq!(row(&rows, "Game CPU (ms)").shown(), "20.1");
    }

    #[test]
    fn capture_asks_for_ten_seconds_and_says_what_it_saw() {
        let mut g = Game::default();
        let rows = page(&mut g, "perf");
        let r = row(&rows, "Capture 10 s");
        let Edit::Action(call) = &r.edit else {
            panic!("an action")
        };
        assert_eq!(*call, Call::new("profile.capture", json!({"ms": 10000})));
        let result = g.call(call).unwrap();
        assert_eq!(capture_toast(&result), "captured 300 frames, p99 41.0 ms");
    }

    #[test]
    fn without_frame_stats_the_capture_says_why() {
        let mut g = Game {
            frame_stats: false,
            ..Game::default()
        };
        let rows = page(&mut g, "perf");
        assert_eq!(
            row(&rows, "Capture 10 s").disabled.as_deref(),
            Some("no frames in the frame-stats window yet")
        );
        assert_eq!(row(&rows, "Overlay (F3)").value, json!(true));
    }
}
