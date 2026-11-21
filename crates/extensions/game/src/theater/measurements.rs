//! Host-only observations. Measurements never enter the film or its hashes.
use super::{Mode, Session, State};
use agent_proto::theater::{CaptureMeasurement, Measurements, PageWindow};
use runtime::{
    Guest, Runtime, WatchError, WatchId,
    guest::{PAGE_SIZE, RAM_SIZE},
    watch::CONTIGUOUS_BASE,
};
use std::{collections::VecDeque, time::Instant};

const HISTORY: usize = 256;

pub(super) struct Run {
    boot: Instant,
    child: Option<Instant>,
    match_start: Option<(u64, u32, u64)>,
    first_slice_ms: Option<u64>,
    fast_forward_started: Option<(u64, Instant)>,
    fast_forward: Option<(u64, u64)>,
    watches: Vec<Window>,
    captures: VecDeque<CaptureMeasurement>,
    pages: VecDeque<PageWindow>,
    captures_omitted: u64,
    page_windows_omitted: u64,
}

struct Window {
    id: WatchId,
    seconds: u32,
    ticks: u64,
    from: u64,
}

fn keep<T>(rows: &mut VecDeque<T>, omitted: &mut u64, entry: T) {
    if rows.len() == HISTORY {
        rows.pop_front();
        *omitted += 1;
    }
    rows.push_back(entry);
}

fn ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

impl Run {
    pub(super) fn snapshot(&self) -> Measurements {
        Measurements {
            child_to_first_slice_ms: self.first_slice_ms,
            boot_to_match_start_ms: self.match_start.map(|(_, _, elapsed)| elapsed),
            match_start_tick: self.match_start.map(|(tick, _, _)| tick),
            fast_forward_ms: self.fast_forward.map(|(_, elapsed)| elapsed),
            fast_forward_ticks: self.fast_forward.map(|(ticks, _)| ticks),
            captures: self.captures.iter().cloned().collect(),
            page_windows: self.pages.iter().cloned().collect(),
            captures_omitted: self.captures_omitted,
            page_windows_omitted: self.page_windows_omitted,
        }
    }

    pub(super) fn capture(
        &mut self,
        tick: u64,
        slices: u64,
        start: Instant,
        reason: Option<String>,
    ) {
        keep(
            &mut self.captures,
            &mut self.captures_omitted,
            CaptureMeasurement {
                tick,
                slices,
                elapsed_ms: ms(start),
                published: reason.is_none(),
                reason,
            },
        );
    }

    pub(super) fn reached_target(&mut self, tick: u64) {
        if let Some((from, started)) = self.fast_forward_started.take() {
            self.fast_forward = Some((tick.saturating_sub(from), ms(started)));
        }
    }

    fn pages_due(&self, tick: u64) -> bool {
        self.match_start.is_some()
            && (self.watches.is_empty()
                || self
                    .watches
                    .iter()
                    .any(|w| tick >= w.from.saturating_add(w.ticks)))
    }

    fn sample_pages(&mut self, guest: &mut Guest, tick: u64) -> Result<(), WatchError> {
        if self.watches.is_empty() {
            let Some((_, rate, _)) = self.match_start else {
                return Ok(());
            };
            for seconds in [5, 30] {
                let id = guest.subscribe_writes("theater.measurements");
                guest.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32)?;
                self.watches.push(Window {
                    id,
                    seconds,
                    ticks: u64::from(rate) * u64::from(seconds),
                    from: tick,
                });
            }
        }
        for w in &mut self.watches {
            if tick >= w.from.saturating_add(w.ticks) {
                let pages = guest
                    .take_dirty(w.id)?
                    .iter()
                    .map(|range| u64::from(range.len) / PAGE_SIZE as u64)
                    .sum();
                let entry = PageWindow {
                    seconds: w.seconds,
                    from_tick: w.from,
                    to_tick: tick,
                    pages,
                };
                w.from = tick;
                keep(&mut self.pages, &mut self.page_windows_omitted, entry);
            }
        }
        Ok(())
    }
}

impl State {
    pub(super) fn measure_match_start(&mut self, tick: u64) {
        let Some(run) = &mut self.measurements else {
            return;
        };
        if run.match_start.is_some() {
            return;
        }
        let desc = match &self.mode {
            Mode::Record(rec) => rec.meta.descriptor(),
            Mode::Replay(..) => {
                let Some(engine) = &self.engine else { return };
                &engine.track.descriptor
            }
        };
        if desc.match_start_tick == Some(tick) {
            run.match_start = Some((
                tick,
                desc.ticks_per_second.unwrap_or(30).max(1) as u32,
                ms(run.boot),
            ));
        }
    }
}

impl Session {
    pub fn start_measurements(&self, boot: Instant, child: Option<Instant>) {
        self.lock().measurements = Some(Run {
            boot,
            child,
            match_start: None,
            first_slice_ms: None,
            fast_forward_started: None,
            fast_forward: None,
            watches: Vec::new(),
            captures: VecDeque::new(),
            pages: VecDeque::new(),
            captures_omitted: 0,
            page_windows_omitted: 0,
        });
    }

    pub fn measure_first_slice(&self) {
        let mut s = self.lock();
        let (tick, seeking) = (s.tick, s.seeking.is_some());
        if let Some(run) = &mut s.measurements {
            run.first_slice_ms = run.child.take().map(ms);
            if seeking {
                run.fast_forward_started = Some((tick, Instant::now()));
            }
        }
    }

    pub fn measure_recording_pages(&self, rt: &Runtime) -> Result<(), WatchError> {
        let tick = {
            let s = self.lock();
            if !matches!(s.mode, Mode::Record(_))
                || s.complete
                || s.failure.is_some()
                || !s
                    .measurements
                    .as_ref()
                    .is_some_and(|run| run.pages_due(s.tick))
            {
                return Ok(());
            }
            s.tick
        };
        let mut guest = rt.memory();
        if let Some(run) = &mut self.lock().measurements {
            run.sample_pages(&mut guest, tick)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn five_and_thirty_second_windows_count_distinct_pages_independently() {
        const TEST_ADDRESS: u32 = 0x10000;
        let mut guest = Guest::new();
        guest
            .map_zeroed(TEST_ADDRESS, 2 * PAGE_SIZE as u32, Permissions::RW)
            .unwrap();
        let mut run = Run {
            boot: Instant::now(),
            child: None,
            match_start: Some((8, 30, 123)),
            first_slice_ms: None,
            fast_forward_started: None,
            fast_forward: None,
            watches: Vec::new(),
            captures: VecDeque::new(),
            pages: VecDeque::new(),
            captures_omitted: 0,
            page_windows_omitted: 0,
        };
        run.sample_pages(&mut guest, 8).unwrap();
        guest.set_u32(TEST_ADDRESS, 1).unwrap();
        guest.set_u32(TEST_ADDRESS + 4, 2).unwrap();
        assert!(!run.pages_due(157));
        run.sample_pages(&mut guest, 158).unwrap();
        guest
            .set_u32(TEST_ADDRESS + PAGE_SIZE as u32, 3)
            .unwrap();
        for i in 2..=6 {
            run.sample_pages(&mut guest, 8 + i * 150).unwrap();
        }
        let report = run.snapshot();
        assert_eq!(report.match_start_tick, Some(8));
        assert_eq!(report.boot_to_match_start_ms, Some(123));
        assert_eq!(
            report.page_windows[0],
            PageWindow {
                seconds: 5,
                from_tick: 8,
                to_tick: 158,
                pages: 1,
            }
        );
        assert_eq!(report.page_windows[1].pages, 1);
        assert_eq!(report.page_windows[2].pages, 0);
        assert_eq!(
            report.page_windows[6],
            PageWindow {
                seconds: 30,
                from_tick: 8,
                to_tick: 908,
                pages: 2,
            }
        );
    }
}
