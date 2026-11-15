//! Opt-in per-context profiling. Hot paths count; only coarse scopes read time.
//! Counters are observational and never drive scheduling or guest time.
use crate::ExecCtx;
use std::{
    fmt,
    time::{Duration, Instant},
};

/// Inclusive host wall time, not GPU completion time. Nested scopes overlap.
#[derive(Clone, Copy, Debug)]
#[repr(usize)]
pub enum Scope {
    Runner,
    Kernel,
    D3d,
    Gfx,
    Fallback,
}
const SCOPES: [Scope; 5] = [
    Scope::Runner,
    Scope::Kernel,
    Scope::D3d,
    Scope::Gfx,
    Scope::Fallback,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timing {
    pub calls: u64,
    pub elapsed: Duration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub transfers: u64,
    pub runner_safe_points: u64,
    pub loop_safe_points: u64,
    pub memory_accessors: u64,
    pub x87_helpers: u64,
    /// Handler exits that supplied generated counters; zero means those
    /// counters may be unavailable (build generated C with H2_PERF=1).
    pub generated_batches: u64,
    pub scopes: [Timing; 5],
}
impl Profile {
    /// Aggregate contexts, or explicitly collected frame/slice snapshots.
    pub fn merge(&mut self, other: &Self) {
        self.transfers += other.transfers;
        self.runner_safe_points += other.runner_safe_points;
        self.loop_safe_points += other.loop_safe_points;
        self.memory_accessors += other.memory_accessors;
        self.x87_helpers += other.x87_helpers;
        self.generated_batches += other.generated_batches;
        for (a, b) in self.scopes.iter_mut().zip(other.scopes) {
            a.calls += b.calls;
            a.elapsed += b.elapsed;
        }
    }
    pub fn timing(&self, scope: Scope) -> Timing {
        self.scopes[scope as usize]
    }
    /// Record an external host scope (e.g. a graphics backend adapter with
    /// no ExecCtx). Merge this separate profile into the run report once.
    pub fn record(&mut self, scope: Scope, elapsed: Duration) {
        let t = &mut self.scopes[scope as usize];
        t.calls += 1;
        t.elapsed += elapsed;
    }
}
impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "perf: transfers={} safe_points={}/{} (runner/loop) memory_accessors={} x87_helpers={} generated_batches={}",
            self.transfers,
            self.runner_safe_points,
            self.loop_safe_points,
            self.memory_accessors,
            self.x87_helpers,
            self.generated_batches
        )?;
        if self.generated_batches == 0 {
            writeln!(
                f,
                "  loop/x87 counts unavailable unless generated C uses H2_PERF=1"
            )?;
        }
        for scope in SCOPES {
            let t = self.timing(scope);
            if t.calls == 0 {
                writeln!(f, "  {scope:?}: unobserved")?;
            } else {
                writeln!(
                    f,
                    "  {scope:?}: {} calls, {:.3} ms inclusive host time",
                    t.calls,
                    t.elapsed.as_secs_f64() * 1000.0
                )?;
            }
        }
        Ok(())
    }
}
impl ExecCtx {
    /// Enable/reset counters, or disable them. Call between invocations.
    pub fn enable_profiling(&mut self, enabled: bool) {
        self.profile = enabled.then(Profile::default);
    }
    pub fn profile(&self) -> Option<&Profile> {
        self.profile.as_ref()
    }
    /// Drain without disabling, useful when aggregating every scheduler slice.
    pub fn take_profile(&mut self) -> Option<Profile> {
        self.profile.as_mut().map(std::mem::take)
    }
    /// Instrument an HLE service, fallback entry or host graphics submission.
    /// No clock read when disabled. Nested durations are inclusive; do not sum
    /// Runner + Kernel + D3d + Gfx as independent components of a frame.
    pub fn measure<R>(&mut self, scope: Scope, f: impl FnOnce(&mut Self) -> R) -> R {
        let start = self.profile.as_ref().map(|_| Instant::now());
        let result = f(self);
        if let (Some(start), Some(p)) = (start, self.profile.as_mut()) {
            let t = &mut p.scopes[scope as usize];
            t.calls += 1;
            t.elapsed += start.elapsed();
        }
        result
    }
}
