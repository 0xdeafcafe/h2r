//! Raw samples to attributed stacks, and the outputs: folded stacks
//! (`flamegraph.pl`, inferno, speedscope), a top-N table and a JSON
//! summary. [`Session`] wraps the sampler and the timeline for a run.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::sample::{RawProfile, Sampler, ThreadTarget};
use crate::symbolize::{Category, Frame, Symbolizer};

/// One attributed sample.
#[derive(Debug, Clone)]
pub struct Stack {
    pub thread: usize,
    pub on_cpu: bool,
    /// Innermost first.
    pub frames: Vec<Frame>,
    /// The innermost frame with a category (or 0).
    pub self_index: usize,
    pub category: Category,
}

impl Stack {
    /// The frame the sample is charged to.
    pub fn self_frame(&self) -> Option<&Frame> {
        self.frames.get(self.self_index)
    }
    /// The innermost guest frame: the guest function that was running,
    /// whether in its own code or in a service it called.
    pub fn guest(&self) -> Option<&Frame> {
        self.frames
            .iter()
            .find(|f| matches!(f, Frame::Guest { .. }))
    }
}

/// A resolved profile.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    pub threads: Vec<String>,
    pub stacks: Vec<Stack>,
    pub wall: Duration,
    pub hz: u32,
    pub missed: u64,
    pub mean_pause_ns: u64,
}

/// Resolve every frame of `raw`. Call while the profiled dylibs are still
/// loaded.
pub fn resolve(raw: RawProfile, sym: &mut Symbolizer) -> Profile {
    let mut stacks = Vec::with_capacity(raw.samples.len());
    for s in &raw.samples {
        let mut frames: Vec<Frame> = Vec::with_capacity(s.frames.len() + 1);
        for (i, &pc) in s.frames.iter().enumerate() {
            // Return addresses point after the call; look up the call.
            frames.push(sym.resolve(if i == 0 { pc } else { pc.saturating_sub(1) }));
        }
        if s.lr != 0 && !frames.is_empty() {
            // A leaf that has not pushed a frame: LR is its caller, which
            // the frame chain skipped.
            let caller = sym.resolve(s.lr.saturating_sub(1));
            let dup = frames[0].same_function(&caller)
                || frames.get(1).is_some_and(|f| f.same_function(&caller));
            if !dup {
                frames.insert(1, caller);
            }
        }
        let (self_index, category) = frames
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.category().map(|c| (i, c)))
            .unwrap_or((0, Category::Other));
        stacks.push(Stack {
            thread: s.thread,
            on_cpu: s.on_cpu,
            frames,
            self_index,
            category,
        });
    }
    Profile {
        threads: raw.threads,
        stacks,
        wall: raw.wall,
        hz: raw.hz,
        missed: raw.missed,
        mean_pause_ns: raw.mean_pause_ns,
    }
}

/// How to render a profile.
#[derive(Debug, Clone, Copy)]
pub struct Render {
    /// Rows in each top-N table.
    pub top: usize,
    /// Leave out samples of threads that were not running.
    pub on_cpu_only: bool,
    /// Add the guest instruction as a leaf frame under its function.
    pub guest_insns: bool,
    /// Put the category above the thread at the root of each stack.
    pub by_category: bool,
    /// Keep the process and thread start-up frames (dyld, `main`,
    /// `lang_start`, `__rust_begin_short_backtrace` and above) in folded
    /// stacks.
    pub full_stacks: bool,
}

impl Default for Render {
    fn default() -> Self {
        Render {
            top: 25,
            on_cpu_only: false,
            guest_insns: true,
            by_category: false,
            full_stacks: false,
        }
    }
}

/// A count and its share of the counted samples.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub name: String,
    pub category: Option<Category>,
    pub samples: u64,
    pub percent: f64,
}

/// What `profile.json` holds.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub samples: u64,
    pub on_cpu: u64,
    pub wall_secs: f64,
    pub hz: u32,
    pub missed: u64,
    pub mean_pause_us: f64,
    pub threads: Vec<String>,
    pub categories: Vec<Row>,
    /// By the frame each sample is charged to.
    pub self_top: Vec<Row>,
    /// Guest functions, counting time in the services they called.
    pub guest_total: Vec<Row>,
    /// Guest functions, their own code only.
    pub guest_self: Vec<Row>,
    /// Guest instructions (needs line tables).
    pub guest_insns: Vec<Row>,
}

fn top(counts: HashMap<(String, Option<Category>), u64>, n: usize, total: u64) -> Vec<Row> {
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.0.cmp(&b.0.0)));
    v.truncate(n);
    v.into_iter()
        .map(|((name, category), samples)| Row {
            name,
            category,
            samples,
            percent: pct(samples, total),
        })
        .collect()
}

fn pct(n: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}

impl Profile {
    fn counted(&self, r: &Render) -> impl Iterator<Item = &Stack> {
        let cpu = r.on_cpu_only;
        self.stacks.iter().filter(move |s| !cpu || s.on_cpu)
    }

    /// Folded stacks, root first, one `frame;frame;... count` line each.
    pub fn folded(&self, r: &Render) -> String {
        let mut counts: HashMap<String, u64> = HashMap::new();
        for s in self.counted(r) {
            let mut line = String::new();
            if r.by_category {
                line.push_str(s.category.name());
                line.push(';');
            }
            line.push_str(&clean(
                self.threads.get(s.thread).map_or("thread", String::as_str),
            ));
            let frames = if r.full_stacks {
                &s.frames[..]
            } else {
                trimmed(&s.frames)
            };
            for f in frames.iter().rev() {
                line.push(';');
                line.push_str(&clean(&f.label()));
            }
            if r.guest_insns
                && let Some(Frame::Guest { insn: Some(pc), .. }) = s.frames.first()
            {
                let _ = write!(line, ";@{pc:#010x}");
            }
            if !s.on_cpu {
                line.push_str(";[off-cpu]");
            }
            *counts.entry(line).or_default() += 1;
        }
        let mut lines: Vec<_> = counts.into_iter().collect();
        lines.sort();
        let mut out = String::new();
        for (l, n) in lines {
            let _ = writeln!(out, "{l} {n}");
        }
        out
    }

    pub fn summary(&self, r: &Render) -> Summary {
        let stacks: Vec<&Stack> = self.counted(r).collect();
        let total = stacks.len() as u64;
        let mut cats: HashMap<Category, u64> = HashMap::new();
        let mut selfs = HashMap::new();
        let mut gtotal = HashMap::new();
        let mut gself = HashMap::new();
        let mut insns = HashMap::new();
        for s in &stacks {
            *cats.entry(s.category).or_default() += 1;
            if let Some(f) = s.self_frame() {
                *selfs.entry((f.label(), f.category())).or_default() += 1;
            }
            if let Some(g) = s.guest() {
                *gtotal
                    .entry((g.label(), Some(Category::Guest)))
                    .or_default() += 1;
            }
            if let Some(f @ Frame::Guest { insn, .. }) = s.self_frame() {
                *gself.entry((f.label(), Some(Category::Guest))).or_default() += 1;
                if let Some(pc) = insn {
                    *insns
                        .entry((
                            format!("{pc:#010x} in {}", f.label()),
                            Some(Category::Guest),
                        ))
                        .or_default() += 1;
                }
            }
        }
        let mut categories: Vec<Row> = Category::ALL
            .iter()
            .filter_map(|&c| {
                let n = *cats.get(&c)?;
                Some(Row {
                    name: c.name().into(),
                    category: Some(c),
                    samples: n,
                    percent: pct(n, total),
                })
            })
            .collect();
        categories.sort_by_key(|a| std::cmp::Reverse(a.samples));
        Summary {
            samples: total,
            on_cpu: stacks.iter().filter(|s| s.on_cpu).count() as u64,
            wall_secs: self.wall.as_secs_f64(),
            hz: self.hz,
            missed: self.missed,
            mean_pause_us: self.mean_pause_ns as f64 / 1000.0,
            threads: self.threads.clone(),
            categories,
            self_top: top(selfs, r.top, total),
            guest_total: top(gtotal, r.top, total),
            guest_self: top(gself, r.top, total),
            guest_insns: top(insns, r.top, total),
        }
    }

    /// The summary as an aligned text table.
    pub fn table(&self, r: &Render) -> String {
        let s = self.summary(r);
        let mut o = String::new();
        let _ = writeln!(
            o,
            "{} samples ({} on cpu) over {:.2}s at {} Hz on {}; {} missed; mean pause {:.1} us",
            s.samples,
            s.on_cpu,
            s.wall_secs,
            s.hz,
            s.threads.join(", "),
            s.missed,
            s.mean_pause_us
        );
        let mut section = |title: &str, rows: &[Row], cat: bool| {
            if rows.is_empty() {
                return;
            }
            let _ = writeln!(o, "\n{title}");
            for r in rows {
                let c = match (cat, r.category) {
                    (true, Some(c)) => c.name(),
                    (true, None) => "-",
                    (false, _) => "",
                };
                let _ = writeln!(
                    o,
                    "  {:>6.2}% {:>8}  {:<9} {}",
                    r.percent, r.samples, c, r.name
                );
            }
        };
        section("by category", &s.categories, false);
        section("self (charged frame)", &s.self_top, true);
        section(
            "guest functions, including services they called",
            &s.guest_total,
            false,
        );
        section("guest functions, own code", &s.guest_self, false);
        section("guest instructions", &s.guest_insns, false);
        o
    }

    /// Write `profile.folded`, `profile.txt` and `profile.json` into `dir`.
    pub fn write(&self, dir: &Path, r: &Render) -> io::Result<Vec<PathBuf>> {
        std::fs::create_dir_all(dir)?;
        let folded = dir.join("profile.folded");
        std::fs::write(&folded, self.folded(r))?;
        let txt = dir.join("profile.txt");
        std::fs::write(&txt, self.table(r))?;
        let json = dir.join("profile.json");
        std::fs::write(
            &json,
            serde_json::to_vec_pretty(&self.summary(r)).map_err(io::Error::other)?,
        )?;
        Ok(vec![folded, txt, json])
    }
}

/// `frames` (innermost first) without the start-up frames: everything from
/// the outermost `__rust_begin_short_backtrace` (main thread and spawned
/// threads alike) outwards. Stacks without one are kept whole.
fn trimmed(frames: &[Frame]) -> &[Frame] {
    let start = frames.iter().rposition(|f| match f {
        Frame::Host { name } => name.contains("__rust_begin_short_backtrace"),
        _ => false,
    });
    match start {
        Some(i) => &frames[..i],
        None => frames,
    }
}

/// Folded-stack frames may not hold `;` (the separator) or newlines, and a
/// trailing number would read as the count.
fn clean(s: &str) -> String {
    s.replace(';', ",").replace('\n', " ")
}

/// What a [`Session`] does.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Write the sampling profile here (a directory); `None` to skip it.
    pub profile: Option<PathBuf>,
    pub hz: u32,
    pub render: Render,
    /// Write a timeline trace JSON here; `None` to skip it.
    pub timeline: Option<PathBuf>,
    pub timeline_config: timeline::Config,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            profile: None,
            hz: 1000,
            render: Render::default(),
            timeline: None,
            timeline_config: timeline::Config::default(),
        }
    }
}

impl SessionConfig {
    /// From the environment: `H2_PROFILE=<dir>` (and `H2_PROFILE_HZ`,
    /// `H2_PROFILE_TOP`, `H2_PROFILE_CPU=1`), `H2_TIMELINE=<file.json>`
    /// (and `H2_TIMELINE_CATS`, see [`timeline::Config::parse`]). `None`
    /// when neither is set.
    pub fn from_env() -> Result<Option<SessionConfig>, String> {
        let var = |k| std::env::var_os(k).filter(|v| !v.is_empty());
        let profile = var("H2_PROFILE").map(PathBuf::from);
        let tl = var("H2_TIMELINE").map(PathBuf::from);
        if profile.is_none() && tl.is_none() {
            return Ok(None);
        }
        let mut c = SessionConfig {
            profile,
            timeline: tl,
            ..SessionConfig::default()
        };
        let num = |k: &str| -> Result<Option<u32>, String> {
            std::env::var(k)
                .ok()
                .map(|v| v.parse().map_err(|e| format!("{k}={v}: {e}")))
                .transpose()
        };
        if let Some(hz) = num("H2_PROFILE_HZ")? {
            c.hz = hz;
        }
        if let Some(n) = num("H2_PROFILE_TOP")? {
            c.render.top = n as usize;
        }
        c.render.on_cpu_only = std::env::var_os("H2_PROFILE_CPU").is_some_and(|v| v == "1");
        if let Ok(spec) = std::env::var("H2_TIMELINE_CATS") {
            c.timeline_config = timeline::Config::parse(&spec)?;
        }
        Ok(Some(c))
    }
}

/// A profiling and/or timeline run around some code. Start it on the
/// thread to sample (more with [`Session::start_with`]); finish it, or
/// drop it, *before* unloading the generated dylib: resolution needs the
/// code still mapped.
pub struct Session {
    config: SessionConfig,
    sampler: Option<Sampler>,
    finished: bool,
}

/// What a finished session wrote.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub files: Vec<PathBuf>,
    pub profile: Option<Profile>,
    pub timeline_events: usize,
    pub timeline_dropped: u64,
}

impl Session {
    /// Sample the calling thread.
    pub fn start(config: SessionConfig) -> io::Result<Session> {
        Self::start_with(config, vec![ThreadTarget::current("main")])
    }

    /// Sample `targets`.
    /// The sampler, which can fail (unsupported host, no targets, no
    /// thread), starts first: on error nothing is left running, the
    /// timeline included.
    pub fn start_with(config: SessionConfig, targets: Vec<ThreadTarget>) -> io::Result<Session> {
        let sampler = match config.profile {
            Some(_) => Some(Sampler::start(targets, config.hz)?),
            None => None,
        };
        if config.timeline.is_some() {
            timeline::start(config.timeline_config);
        }
        Ok(Session {
            config,
            sampler,
            finished: false,
        })
    }

    /// [`SessionConfig::from_env`], started on the calling thread. Errors
    /// are reported on stderr and give `None`: a bad variable must not
    /// stop the run it was meant to measure.
    pub fn from_env() -> Option<Session> {
        match SessionConfig::from_env() {
            Ok(Some(c)) => match Session::start(c) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("perf: not profiling: {e}");
                    None
                }
            },
            Ok(None) => None,
            Err(e) => {
                eprintln!("perf: not profiling: {e}");
                None
            }
        }
    }

    /// Stop, resolve and write everything.
    pub fn finish(mut self) -> io::Result<Report> {
        self.finish_inner()
    }

    fn finish_inner(&mut self) -> io::Result<Report> {
        self.finished = true;
        let mut report = Report::default();
        let raw = self.sampler.take().map(Sampler::finish);
        if let Some(path) = &self.config.timeline {
            let t = timeline::stop();
            report.timeline_events = t.len();
            report.timeline_dropped = t.dropped;
            if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(d)?;
            }
            t.save(path)?;
            report.files.push(path.clone());
        }
        if let (Some(raw), Some(dir)) = (raw, &self.config.profile) {
            let p = resolve(raw, &mut Symbolizer::new());
            report.files.extend(p.write(dir, &self.config.render)?);
            report.profile = Some(p);
        }
        Ok(report)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        match self.finish_inner() {
            Ok(r) => {
                for f in &r.files {
                    eprintln!("perf: wrote {}", f.display());
                }
            }
            Err(e) => eprintln!("perf: writing the profile failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::RawSample;

    fn stack(frames: Vec<Frame>, on_cpu: bool) -> Stack {
        let (self_index, category) = frames
            .iter()
            .enumerate()
            .find_map(|(i, f)| f.category().map(|c| (i, c)))
            .unwrap_or((0, Category::Other));
        Stack {
            thread: 0,
            on_cpu,
            frames,
            self_index,
            category,
        }
    }

    fn host(n: &str) -> Frame {
        Frame::Host { name: n.into() }
    }

    fn sample_profile() -> Profile {
        let g = |f, i| Frame::Guest {
            function: f,
            insn: Some(i),
        };
        Profile {
            threads: vec!["game".into()],
            stacks: vec![
                stack(
                    vec![
                        g(0x1000, 0x1004),
                        host("runtime::runner::Runner::run_from"),
                        host("main"),
                    ],
                    true,
                ),
                stack(
                    vec![
                        g(0x1000, 0x1004),
                        host("runtime::runner::Runner::run_from"),
                        host("main"),
                    ],
                    true,
                ),
                stack(
                    vec![
                        g(0x1000, 0x1008),
                        host("runtime::runner::Runner::run_from"),
                        host("main"),
                    ],
                    true,
                ),
                stack(
                    vec![
                        host("_platform_memmove"),
                        host("runtime::ffi::h2abi_v1_read"),
                        g(0x1000, 0x1010),
                        host("runtime::runner::Runner::run_from"),
                        host("main"),
                    ],
                    true,
                ),
                stack(
                    vec![host("d3d::device::Device<B>::drain"), host("main")],
                    false,
                ),
            ],
            wall: Duration::from_millis(5),
            hz: 1000,
            ..Profile::default()
        }
    }

    #[test]
    fn samples_are_charged_to_the_innermost_known_frame() {
        let p = sample_profile();
        assert_eq!(
            p.stacks[3].category,
            Category::Runtime,
            "memmove inside a memory read"
        );
        assert_eq!(p.stacks[3].self_index, 1);
        let s = p.summary(&Render::default());
        assert_eq!(s.samples, 5);
        assert_eq!(s.on_cpu, 4);
        let cat = |n: &str| s.categories.iter().find(|r| r.name == n).map(|r| r.samples);
        assert_eq!(cat("guest"), Some(3));
        assert_eq!(cat("runtime"), Some(1));
        assert_eq!(cat("d3d"), Some(1));
        assert_eq!(s.guest_total[0].name, "guest 0x00001000");
        assert_eq!(s.guest_total[0].samples, 4, "includes the read it called");
        assert_eq!(s.guest_self[0].samples, 3);
        assert_eq!(s.guest_insns[0].name, "0x00001004 in guest 0x00001000");
        assert_eq!(s.guest_insns[0].samples, 2);
        let cpu = p.summary(&Render {
            on_cpu_only: true,
            ..Render::default()
        });
        assert_eq!(cpu.samples, 4);
    }

    #[test]
    fn folded_stacks_are_root_first_and_counted() {
        let p = sample_profile();
        let f = p.folded(&Render {
            full_stacks: true,
            ..Render::default()
        });
        assert!(
            f.lines().any(|l| l
                == "game;main;runtime::runner::Runner::run_from;guest 0x00001000;@0x00001004 2"),
            "{f}"
        );
        assert!(
            f.lines()
                .any(|l| l == "game;main;d3d::device::Device<B>::drain;[off-cpu] 1"),
            "{f}"
        );
        let by = p.folded(&Render {
            by_category: true,
            guest_insns: false,
            full_stacks: true,
            ..Render::default()
        });
        assert!(
            by.lines().any(
                |l| l == "guest;game;main;runtime::runner::Runner::run_from;guest 0x00001000 3"
            ),
            "{by}"
        );
        let t = p.table(&Render::default());
        assert!(
            t.contains("by category") && t.contains("guest instructions"),
            "{t}"
        );
    }

    #[test]
    fn start_up_frames_are_trimmed() {
        let s = stack(
            vec![
                Frame::Guest {
                    function: 0x1000,
                    insn: None,
                },
                host("runtime::runner::Runner::run_from"),
                host("std::sys::backtrace::__rust_begin_short_backtrace::<fn()>"),
                host("std::rt::lang_start_internal"),
                Frame::Unknown { pc: 0x18376fe7f },
            ],
            true,
        );
        let p = Profile {
            threads: vec!["main".into()],
            stacks: vec![s],
            ..Profile::default()
        };
        assert_eq!(
            p.folded(&Render::default()),
            "main;runtime::runner::Runner::run_from;guest 0x00001000 1\n"
        );
    }

    /// A sampler that cannot start leaves no timeline recording behind.
    #[test]
    fn a_failed_start_does_not_leave_the_timeline_on() {
        let dir = std::env::temp_dir().join(format!("perf-session-fail-{}", std::process::id()));
        let config = SessionConfig {
            profile: Some(dir.clone()),
            timeline: Some(dir.join("t.json")),
            ..SessionConfig::default()
        };
        assert!(Session::start_with(config, vec![]).is_err());
        assert!(!timeline::recording());
        assert!(!dir.exists());
    }

    #[test]
    fn separators_in_names_are_replaced() {
        assert_eq!(clean("f<[u8; 4]>"), "f<[u8, 4]>");
    }

    /// The link register is inserted only when it names a new caller.
    #[test]
    fn link_register_fills_in_a_frameless_leaf_caller() {
        #[inline(never)]
        fn leaf_marker() -> u64 {
            std::hint::black_box(1)
        }
        #[inline(never)]
        fn caller_marker() -> u64 {
            std::hint::black_box(2)
        }
        let leaf = leaf_marker as *const () as u64;
        let caller = caller_marker as *const () as u64 + 4;
        let raw = RawProfile {
            threads: vec!["t".into()],
            samples: vec![
                RawSample {
                    thread: 0,
                    at: Duration::ZERO,
                    on_cpu: true,
                    frames: vec![leaf],
                    lr: caller + 1,
                },
                RawSample {
                    thread: 0,
                    at: Duration::ZERO,
                    on_cpu: true,
                    frames: vec![leaf],
                    lr: leaf + 1,
                },
            ],
            ..RawProfile::default()
        };
        let p = resolve(raw, &mut Symbolizer::new());
        assert_eq!(p.stacks[0].frames.len(), 2);
        assert!(p.stacks[0].frames[1].label().contains("caller_marker"));
        assert_eq!(
            p.stacks[1].frames.len(),
            1,
            "LR inside the leaf itself is not a caller"
        );
    }
}
