//! Shadow execution: native ports checked against the lifted original on
//! the states the game reaches (docs/engine/shadow-exec.md).
//!
//! The port installers register every port ([`Shadow::register`]) and run
//! each installed body through [`Shadow::run`]. With nothing turned on that
//! is one relaxed atomic load. The agent API's `shadow.enable` turns checks
//! on for named ports or groups; then each sampled call is [`check`]ed
//! before the port runs on the live Guest as it always does, and
//! `shadow.status` reads the counts and the mismatches.

mod check;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};
use serde::Serialize;

use crate::code::Code;
pub use check::Overlay;
pub use check::{ByteRun, Diff, MAX_RUNS, Outcome, PortFn, TransferDiff, View, check};

/// Mismatches kept; later ones are counted in `mismatches_dropped`.
pub const MAX_MISMATCHES: usize = 64;
/// Distinct skip reasons kept per port; later ones count as `other`.
const MAX_REASONS: usize = 16;
/// Instructions the original may run per check, by default.
pub const DEFAULT_STEP_LIMIT: u64 = 5_000_000;

/// Builds the lifted original when checks are first turned on.
pub type Source = Box<dyn FnMut() -> Result<Box<dyn Code>, String> + Send>;
/// Reads the game's tick from guest memory, for the mismatch record.
pub type TickFn = fn(&Guest) -> Option<u64>;

/// One registered port and what its checks found.
#[derive(Debug, Clone, Serialize)]
pub struct Port {
    /// The port's switch name (`sim.native_unit_pose`).
    pub name: &'static str,
    /// Its group (`sim.native`, `sim.physics`).
    pub group: &'static str,
    /// The point it replaces.
    pub point: &'static str,
    /// The replaced function's address.
    pub function: String,
    #[serde(skip)]
    pub base: u32,
    /// Installed in this run's plan (its `impl.` switch is on).
    pub installed: bool,
    /// Checks on.
    pub on: bool,
    /// Calls through the port since checks were turned on.
    pub calls: u64,
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub skipped: u64,
    /// Why checks were skipped, with counts.
    pub skip_reasons: BTreeMap<String, u64>,
}

/// One mismatch, with what is needed to reproduce it.
#[derive(Debug, Clone, Serialize)]
pub struct Mismatch {
    pub port: &'static str,
    pub group: &'static str,
    pub point: &'static str,
    pub function: String,
    /// The address the port was entered at (the function, or a resume).
    pub pc: String,
    /// The game's tick, when readable.
    pub tick: Option<u64>,
    /// Which call of the port this was, counting from 1 since checks were
    /// turned on.
    pub call: u64,
    #[serde(flatten)]
    pub diff: Diff,
    /// The registers at entry.
    pub entry: BTreeMap<String, String>,
}

/// `shadow.status`.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// Any port's checks are on.
    pub on: bool,
    /// The lifted original is built.
    pub ready: bool,
    /// One call in `every` is checked.
    pub every: u64,
    pub step_limit: u64,
    pub calls: u64,
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub skipped: u64,
    pub ports: Vec<Port>,
    pub mismatches: Vec<Mismatch>,
    pub mismatches_dropped: u64,
}

/// What `shadow.enable` did.
#[derive(Debug, Clone, Serialize)]
pub struct Enabled {
    /// Ports whose checks are now on.
    pub enabled: Vec<&'static str>,
    /// Ports named (directly or by group) that are registered but not
    /// installed in this run: turn their `impl.` switch on to check them.
    pub not_installed: Vec<&'static str>,
    pub every: u64,
    pub step_limit: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Names that are neither a port, a group nor `all`.
    Unknown(Vec<String>),
    /// The lifted original cannot be built here.
    Unavailable(String),
    Invalid(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unknown(names) => write!(f, "no port or group named {}", names.join(", ")),
            Error::Unavailable(why) | Error::Invalid(why) => f.write_str(why),
        }
    }
}

struct State {
    ports: BTreeMap<&'static str, Port>,
    every: u64,
    step_limit: u64,
    code: Option<Box<dyn Code>>,
    source: Option<Source>,
    view: View,
    mismatches: Vec<Mismatch>,
    dropped: u64,
}

/// The registry. One per process ([`SHADOW`]); tests make their own.
pub struct Shadow {
    on: AtomicBool,
    state: Mutex<State>,
}

/// The process's registry, which the port installers and the agent API use.
pub static SHADOW: Shadow = Shadow::new();

impl Default for Shadow {
    fn default() -> Self {
        Self::new()
    }
}

impl Shadow {
    pub const fn new() -> Self {
        Shadow {
            on: AtomicBool::new(false),
            state: Mutex::new(State {
                ports: BTreeMap::new(),
                every: 1,
                step_limit: DEFAULT_STEP_LIMIT,
                code: None,
                source: None,
                view: View::new(),
                mismatches: Vec::new(),
                dropped: 0,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A port the plan knows, installed or not. Registering a name again
    /// (a later plan) updates it and keeps its counts.
    pub fn register(
        &self,
        name: &'static str,
        group: &'static str,
        point: &'static str,
        base: u32,
        installed: bool,
    ) {
        let mut s = self.lock();
        let p = s.ports.entry(name).or_insert_with(|| Port {
            name,
            group,
            point,
            function: format!("{base:#010x}"),
            base,
            installed,
            on: false,
            calls: 0,
            checked: 0,
            matched: 0,
            mismatched: 0,
            skipped: 0,
            skip_reasons: BTreeMap::new(),
        });
        p.installed = installed;
        if !installed {
            p.on = false;
        }
        let any = s.ports.values().any(|p| p.on);
        self.on.store(any, Ordering::Relaxed);
    }

    /// Where the lifted original comes from (boot sets it once the image
    /// is known).
    pub fn set_source(&self, source: Source) {
        self.lock().source = Some(source);
    }

    /// Use `code` as the lifted original now (tests, and a source that has
    /// already run).
    pub fn set_code(&self, code: Box<dyn Code>) {
        self.lock().code = Some(code);
    }

    /// The ports `names` select: a port's name, a group, or `all`.
    fn select(s: &State, names: &[String]) -> Result<Vec<&'static str>, Error> {
        let mut out = Vec::new();
        let mut unknown = Vec::new();
        for n in names {
            let before = out.len();
            for p in s.ports.values() {
                if n == "all" || p.name == n || p.group == n {
                    out.push(p.name);
                }
            }
            if out.len() == before {
                unknown.push(n.clone());
            }
        }
        if !unknown.is_empty() {
            return Err(Error::Unknown(unknown));
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    /// Turn checks on for `names` (ports, groups or `all`), resetting their
    /// counts and mismatches. `every`: check one call in that many (default
    /// 1, or what it was). Builds the lifted original first if needed.
    pub fn enable(
        &self,
        names: &[String],
        every: Option<u64>,
        step_limit: Option<u64>,
    ) -> Result<Enabled, Error> {
        if names.is_empty() {
            return Err(Error::Invalid(
                "name at least one port, group, or all".into(),
            ));
        }
        if every == Some(0) || step_limit == Some(0) {
            return Err(Error::Invalid("every and step_limit are at least 1".into()));
        }
        let mut s = self.lock();
        let selected = Self::select(&s, names)?;
        if s.code.is_none() {
            let source = s.source.as_mut().ok_or_else(|| {
                Error::Unavailable("no image for the lifted original in this process".into())
            })?;
            let code = source().map_err(Error::Unavailable)?;
            s.code = Some(code);
        }
        if let Some(e) = every {
            s.every = e;
        }
        if let Some(l) = step_limit {
            s.step_limit = l;
        }
        let (mut enabled, mut not_installed) = (Vec::new(), Vec::new());
        for name in &selected {
            let p = s.ports.get_mut(name).expect("selected from the map");
            if !p.installed {
                not_installed.push(p.name);
                continue;
            }
            *p = Port {
                on: true,
                calls: 0,
                checked: 0,
                matched: 0,
                mismatched: 0,
                skipped: 0,
                skip_reasons: BTreeMap::new(),
                ..p.clone()
            };
            enabled.push(p.name);
        }
        s.mismatches.retain(|m| !enabled.contains(&m.port));
        self.on
            .store(s.ports.values().any(|p| p.on), Ordering::Relaxed);
        Ok(Enabled {
            enabled,
            not_installed,
            every: s.every,
            step_limit: s.step_limit,
        })
    }

    /// Turn checks off for `names`, or for every port when empty. Counts
    /// stay for `status`. Returns the ports turned off.
    pub fn disable(&self, names: &[String]) -> Result<Vec<&'static str>, Error> {
        let mut s = self.lock();
        let selected = if names.is_empty() {
            s.ports.keys().copied().collect()
        } else {
            Self::select(&s, names)?
        };
        let mut off = Vec::new();
        for name in selected {
            let p = s.ports.get_mut(name).expect("selected from the map");
            if p.on {
                p.on = false;
                off.push(name);
            }
        }
        let any = s.ports.values().any(|p| p.on);
        if !any {
            s.view.clear();
        }
        self.on.store(any, Ordering::Relaxed);
        Ok(off)
    }

    pub fn status(&self) -> Status {
        let s = self.lock();
        let ports: Vec<Port> = s.ports.values().cloned().collect();
        let sum = |f: fn(&Port) -> u64| ports.iter().map(f).sum();
        Status {
            on: ports.iter().any(|p| p.on),
            ready: s.code.is_some(),
            every: s.every,
            step_limit: s.step_limit,
            calls: sum(|p| p.calls),
            checked: sum(|p| p.checked),
            matched: sum(|p| p.matched),
            mismatched: sum(|p| p.mismatched),
            skipped: sum(|p| p.skipped),
            mismatches: s.mismatches.clone(),
            mismatches_dropped: s.dropped,
            ports,
        }
    }

    /// Run the port `name` at `pc` on the live Guest, checking it first
    /// when its checks are on and this call is sampled. The port's answer
    /// on the live Guest is returned either way.
    pub fn run(
        &self,
        name: &'static str,
        cpu: &mut CpuAbi,
        guest: &mut Guest,
        pc: u32,
        tick: TickFn,
        port: &mut PortFn<'_>,
    ) -> Result<TransferAbi, GuestFault> {
        if !self.on.load(Ordering::Relaxed) {
            return port(cpu, guest);
        }
        let entry = *cpu;
        let mut guard = self.lock();
        let s = &mut *guard;
        let (every, limit) = (s.every, s.step_limit);
        let Some(p) = s.ports.get_mut(name).filter(|p| p.on) else {
            drop(guard);
            return port(cpu, guest);
        };
        p.calls += 1;
        let (call, base) = (p.calls, p.base);
        if (call - 1) % every != 0 {
            drop(guard);
            return port(cpu, guest);
        }
        let Some(code) = s.code.as_deref_mut() else {
            drop(guard);
            return port(cpu, guest);
        };
        code.prime(base);
        let (mut outcome, copy) = check(code, &mut s.view, guest, &entry, limit, port);
        // The real call: on the live Guest, as without checks.
        let live = port(cpu, guest);
        if let (Ok(t), Some((copy_cpu, copy_t))) = (&live, copy)
            && (copy_cpu != *cpu || copy_t != *t)
        {
            let mut registers = crate::cpu::diff(
                &crate::cpu::to_interp(cpu),
                &crate::cpu::to_interp(&copy_cpu),
            );
            registers
                .iter_mut()
                .for_each(|r| r.name = format!("{} (live, copy)", r.name));
            outcome = Outcome::Mismatched(Box::new(Diff {
                kind: "nondeterministic",
                summary: format!(
                    "the port answered differently on the live Guest ({}) than on the copy ({})",
                    show_transfer(t),
                    show_transfer(&copy_t)
                ),
                transfer: (copy_t != *t).then(|| TransferDiff {
                    port: show_transfer(t),
                    original: show_transfer(&copy_t),
                }),
                registers,
                memory: Vec::new(),
                memory_bytes: 0,
                steps: 0,
            }));
        }
        Self::record(s, name, pc, call, tick(guest), &entry, outcome);
        live
    }

    fn record(
        s: &mut State,
        name: &'static str,
        pc: u32,
        call: u64,
        tick: Option<u64>,
        entry: &CpuAbi,
        outcome: Outcome,
    ) {
        let Some(p) = s.ports.get_mut(name) else {
            return;
        };
        p.checked += 1;
        match outcome {
            Outcome::Matched { .. } => p.matched += 1,
            Outcome::Skipped(why) => {
                p.checked -= 1;
                p.skipped += 1;
                let key = if p.skip_reasons.len() < MAX_REASONS || p.skip_reasons.contains_key(&why)
                {
                    why
                } else {
                    "other".into()
                };
                *p.skip_reasons.entry(key).or_default() += 1;
            }
            Outcome::Mismatched(diff) => {
                p.mismatched += 1;
                if s.mismatches.len() >= MAX_MISMATCHES {
                    s.dropped += 1;
                    return;
                }
                let m = Mismatch {
                    port: p.name,
                    group: p.group,
                    point: p.point,
                    function: p.function.clone(),
                    pc: format!("{pc:#010x}"),
                    tick,
                    call,
                    diff: *diff,
                    entry: registers(entry),
                };
                s.mismatches.push(m);
            }
        }
    }
}

fn show_transfer(t: &TransferAbi) -> String {
    format!(
        "kind {} to {:#010x} resume {:#010x}",
        t.kind, t.target_pc, t.resume_pc
    )
}

/// Every register of `c` by name, as hex: the entry state of a mismatch.
pub fn registers(c: &CpuAbi) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (n, v) in [
        ("eax", c.eax),
        ("ecx", c.ecx),
        ("edx", c.edx),
        ("ebx", c.ebx),
        ("esp", c.esp),
        ("ebp", c.ebp),
        ("esi", c.esi),
        ("edi", c.edi),
        ("eip", c.eip),
        ("eflags", c.eflags),
        ("eflags_defined", c.eflags_defined),
        ("fs_base", c.fs_base),
        ("mxcsr", c.mxcsr),
    ] {
        out.insert(n.to_string(), format!("{v:#010x}"));
    }
    for (n, v) in [
        ("fpu_control", c.x87.control),
        ("fpu_status", c.x87.status),
        ("fpu_tag", c.x87.tag),
    ] {
        out.insert(n.to_string(), format!("{v:#06x}"));
    }
    for i in 0..8 {
        out.insert(
            format!("xmm{i}"),
            format!("{:#034x}", u128::from_le_bytes(c.xmm[i])),
        );
        out.insert(
            format!("st{i}"),
            c.x87.st[i]
                .iter()
                .rev()
                .map(|b| format!("{b:02x}"))
                .collect(),
        );
    }
    out
}

#[cfg(test)]
mod tests;
