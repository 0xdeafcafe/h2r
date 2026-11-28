//! The co-op latency line (docs/live/live-coop.md section 15): once a
//! second, on each console of a co-op game over Live, what its part of a
//! button press's trip costs.
//!
//! - A guest: how often updates arrive (min/median/max gap), and the time
//!   from sending its actions to the next update arriving ("input to
//!   update": one way up, the host's wait for its next tick, one way back).
//! - The host: how long a guest's actions wait for the next update to go
//!   out.
//! - Both: the title's datagrams per second, bytes and the largest
//!   datagram (`live_hle::traffic`).
//!
//! The hooks only observe and only while co-op over Live is armed; the
//! line goes to stderr (`coop-live: latency ...`, which `h2 coop test`
//! keeps). `H2_COOP_LATENCY=0` or `H2_FEATURES=-coop-latency` drops it.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use guest_layout::points;

use crate::coop_probe::Arm;

/// The feature id.
pub const FEATURE: &str = "coop-latency";
/// How often the line is printed (ms).
pub const EVERY_MS: u64 = 1_000;

/// Samples in milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Window(Vec<u32>);

impl Window {
    pub fn push(&mut self, ms: u64) {
        self.0.push(ms.min(u64::from(u32::MAX)) as u32);
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Min, median (the upper of two middles) and max.
    pub fn summary(&self) -> Option<(u32, u32, u32)> {
        let mut v = self.0.clone();
        v.sort_unstable();
        Some((*v.first()?, v[v.len() / 2], *v.last()?))
    }
    fn take(&mut self) -> Window {
        std::mem::take(self)
    }
}

fn mmm(w: &Window) -> String {
    match w.summary() {
        Some((a, b, c)) => format!("{a}/{b}/{c} ms"),
        None => "none".into(),
    }
}

/// One console's measurements; the hooks feed it, [`Latency::report`]
/// makes the line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    last_report: Option<u64>,
    // Guest.
    gaps: Window,
    input_to_update: Window,
    last_update_in: Option<u64>,
    actions_out: Option<u64>,
    // Host.
    held: Window,
    actions_in: Option<u64>,
    updates_out: u32,
}

impl Latency {
    /// This console sent its actions (a guest, every tick).
    pub fn actions_sent(&mut self, now: u64) {
        self.actions_out.get_or_insert(now);
    }

    /// An update arrived (a guest).
    pub fn update_received(&mut self, now: u64) {
        if let Some(t) = self.last_update_in.replace(now) {
            self.gaps.push(now.saturating_sub(t));
        }
        if let Some(t) = self.actions_out.take() {
            self.input_to_update.push(now.saturating_sub(t));
        }
    }

    /// A peer's actions arrived (the host).
    pub fn actions_received(&mut self, now: u64) {
        self.actions_in.get_or_insert(now);
    }

    /// An update went out to one peer (the host; one per peer per tick).
    pub fn update_sent(&mut self, now: u64) {
        self.updates_out += 1;
        if let Some(t) = self.actions_in.take() {
            self.held.push(now.saturating_sub(t));
        }
    }

    /// The line, once [`EVERY_MS`] has passed since the last and something
    /// was measured; `net` is the traffic phrase for that span.
    pub fn report(&mut self, now: u64, net: impl FnOnce(f64) -> String) -> Option<String> {
        let since = *self.last_report.get_or_insert(now);
        let span = now.saturating_sub(since);
        if span < EVERY_MS {
            return None;
        }
        self.last_report = Some(now);
        let secs = span as f64 / 1000.0;
        let mut parts = Vec::new();
        if !self.gaps.is_empty() || !self.input_to_update.is_empty() {
            let gaps = self.gaps.take();
            parts.push(format!(
                "guest: {:.0} updates/s, apart {}; input to update {}",
                (gaps.len() as f64 / secs).round(),
                mmm(&gaps),
                mmm(&self.input_to_update.take())
            ));
        }
        if self.updates_out > 0 {
            parts.push(format!(
                "host: {:.0} updates out/s; guest input waited {} for the next update",
                (f64::from(self.updates_out) / secs).round(),
                mmm(&self.held.take())
            ));
            self.updates_out = 0;
        }
        if parts.is_empty() {
            return None;
        }
        Some(format!(
            "coop-live: latency {} (min/median/max); {}",
            parts.join("; "),
            net(secs)
        ))
    }
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// The measurements and the traffic counters at the last line.
#[derive(Default)]
struct Shared {
    latency: Latency,
    traffic: Option<live_hle::traffic::Traffic>,
}

impl Shared {
    fn line(&mut self, now: u64) -> Option<String> {
        let traffic = &mut self.traffic;
        self.latency.report(now, |secs| {
            let t = live_hle::traffic::snapshot();
            let d = t.since(&traffic.unwrap_or_default());
            *traffic = Some(t);
            d.describe(secs)
        })
    }
}

/// Whether `H2_COOP_LATENCY` leaves the line on (`0`, `off`, `false`, `no`
/// turn it off).
pub fn wanted(v: Option<&str>) -> bool {
    !v.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        )
    })
}

/// Every point the feature hooks.
pub static HOOKED: &[&points::Point] = &[
    &points::SIM_ACTIONS_SEND,
    &points::SIM_ACTIONS_RECEIVE,
    &points::SIM_UPDATE_SEND,
    &points::SIM_CATCH_UP,
];

pub struct CoopLatency {
    arm: Arc<Arm>,
    shared: Arc<Mutex<Shared>>,
}

impl hooks::Feature for CoopLatency {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Co-op over Live: a latency line a second (update gaps, input to update, the host's wait, traffic)",
        )
        .doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        type Event = fn(&mut Latency, u64);
        let events: [(&'static points::Point, &'static str, Event); 4] = [
            (
                &points::SIM_ACTIONS_SEND,
                "latency: actions sent",
                Latency::actions_sent,
            ),
            (
                &points::SIM_ACTIONS_RECEIVE,
                "latency: actions received",
                Latency::actions_received,
            ),
            (
                &points::SIM_UPDATE_SEND,
                "latency: update sent",
                Latency::update_sent,
            ),
            (
                &points::SIM_CATCH_UP,
                "latency: update received",
                Latency::update_received,
            ),
        ];
        for (point, what, event) in events {
            let (arm, shared) = (Arc::clone(&self.arm), Arc::clone(&self.shared));
            b.raw()
                .observe_before(point, what, move |_| {
                    if !arm.active() {
                        return;
                    }
                    let now = now_ms();
                    let line = {
                        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                        event(&mut s.latency, now);
                        s.line(now)
                    };
                    if let Some(line) = line {
                        eprintln!("{line}");
                    }
                })
                .optional();
        }
        // docs/runtime/snapshots.md: an observer; its windows, wall-clock
        // times and traffic counters only make the log line.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the feature on the co-op over Live arm, unless `H2_COOP_LATENCY`
/// turns it off.
pub fn install(registry: &mut hooks::Registry, arm: Arc<Arm>) -> anyhow::Result<bool> {
    if !wanted(std::env::var("H2_COOP_LATENCY").ok().as_deref()) {
        return Ok(false);
    }
    registry
        .add(&CoopLatency {
            arm,
            shared: Arc::default(),
        })
        .map_err(|e| anyhow::anyhow!("coop-latency: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(_: f64) -> String {
        "net".into()
    }

    #[test]
    fn a_window_gives_min_median_max() {
        let mut w = Window::default();
        assert_eq!(w.summary(), None);
        for ms in [50, 10, 30, 20] {
            w.push(ms);
        }
        assert_eq!(w.summary(), Some((10, 30, 50)));
        w.push(u64::MAX);
        assert_eq!(w.summary().unwrap().2, u32::MAX);
    }

    #[test]
    fn a_guest_measures_update_gaps_and_input_to_update() {
        let mut l = Latency::default();
        assert_eq!(l.report(0, net), None, "the first call starts the clock");
        // Ticks at 0, 33, 66, ...; actions go out 5 ms into each tick and
        // the update that answers them comes back 40 ms later.
        for k in 0..30u64 {
            let t = k * 33;
            l.actions_sent(t + 5);
            // A second send in the same tick keeps the first time.
            l.actions_sent(t + 6);
            l.update_received(t + 45);
        }
        assert_eq!(l.report(999, net), None, "not a second yet");
        assert_eq!(
            l.report(1_000, net).unwrap(),
            "coop-live: latency guest: 29 updates/s, apart 33/33/33 ms; input to update 40/40/40 ms (min/median/max); net"
        );
        // Nothing new: no line.
        assert_eq!(l.report(2_100, net), None);
    }

    #[test]
    fn the_host_measures_how_long_input_waits_for_the_next_update() {
        let mut l = Latency::default();
        l.report(0, net);
        // Two guests, so two sends per tick; actions arrive 10 ms before
        // the tick's sends.
        for k in 1..=30u64 {
            let t = k * 33;
            l.actions_received(t - 10);
            l.actions_received(t - 5);
            l.update_sent(t);
            l.update_sent(t);
        }
        let line = l.report(1_000, net).unwrap();
        assert_eq!(
            line,
            "coop-live: latency host: 60 updates out/s; guest input waited 10/10/10 ms for the next update (min/median/max); net"
        );
    }

    #[test]
    fn the_traffic_phrase_is_a_rate_over_the_span() {
        let mut s = Shared::default();
        s.latency.report(0, net);
        s.latency.update_received(10);
        s.latency.update_received(43);
        let line = s.line(2_000).unwrap();
        assert!(line.contains("guest: 1 updates/s"), "{line}");
        assert!(line.ends_with(" B"), "{line}");
        assert!(line.contains("datagrams/s"), "{line}");
    }

    #[test]
    fn the_line_can_be_turned_off() {
        assert!(wanted(None));
        assert!(wanted(Some("1")));
        for v in ["0", "off", " False ", "no"] {
            assert!(!wanted(Some(v)), "{v}");
        }
    }

    #[test]
    fn the_feature_is_on_the_plan_with_the_co_op_hooks() {
        let rt = runtime::Runtime::new(
            runtime::Guest::new(),
            runtime::dispatch::Dispatch::default(),
        );
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        crate::coop_probe::install_live(&rt, &mut r, std::path::Path::new("/nonexistent"))
            .unwrap()
            .unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        assert!(text.contains(FEATURE), "{text}");
        for p in HOOKED {
            assert!(text.contains(p.name), "{} missing:\n{text}", p.name);
        }
    }
}
