//! `cargo xtask`: the workspace's test and check runner
//! (docs/dev/build-speed.md, "The gate"; docs/dev/crate-structure.md,
//! "Gates"). See README.md beside this crate.

mod git;
mod ledger;
mod proc;
mod report;
mod workspace;
mod xml;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::report::{Summary, add_cases};
use crate::workspace::{Affected, OutsideInputs, Workspace};

const USAGE: &str = "\
cargo xtask <command> [options]

Commands:
  test       check, then test the packages a branch can have changed:
             one nextest run over all of them, then their doctests
  check      the desktop and host cargo checks, the crate rules, the crate
             table, and the docs check on changed docs
  affected   print the packages a branch can have changed, one per line

Options:
  --base <rev>       compare against the merge base with <rev> (default origin/master)
  -p, --package <p>  these packages instead of the affected ones (repeatable, or a,b)
  --all              every package in the workspace
  --disc             test: also the disc tier (ignored tests, the oracle, the differentials)
  --web              test: also the web app's tests and typecheck (automatic when web/ changed)
  --no-ledger        test: run every package, and record no pass
  --why              affected: say on stderr which file put each package in the set
  --dry-run          print the packages and the exact commands; run nothing
";

#[derive(Debug, Clone, PartialEq)]
struct Opts {
    command: String,
    base: String,
    packages: Vec<String>,
    all: bool,
    disc: bool,
    web: bool,
    no_ledger: bool,
    why: bool,
    dry_run: bool,
}

fn parse_args(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        command: String::new(),
        base: "origin/master".into(),
        packages: Vec::new(),
        all: false,
        disc: false,
        web: false,
        no_ledger: false,
        why: false,
        dry_run: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        let mut value = |name: &str| -> Result<String> {
            match inline.clone().or_else(|| it.next().cloned()) {
                Some(v) => Ok(v),
                None => bail!("{name} needs a value"),
            }
        };
        match flag {
            "--base" => o.base = value("--base")?,
            "-p" | "--package" => {
                o.packages.extend(value("--package")?.split(',').filter(|s| !s.is_empty()).map(str::to_owned))
            }
            "--all" | "--workspace" => o.all = true,
            "--disc" => o.disc = true,
            "--web" => o.web = true,
            "--no-ledger" => o.no_ledger = true,
            "--why" => o.why = true,
            "--dry-run" | "-n" => o.dry_run = true,
            "-h" | "--help" | "help" => o.command = "help".into(),
            f if f.starts_with('-') => bail!("unknown option {f}\n\n{USAGE}"),
            c if o.command.is_empty() => o.command = c.to_owned(),
            c => bail!("unexpected argument {c}\n\n{USAGE}"),
        }
    }
    if o.command.is_empty() {
        o.command = "help".into();
    }
    Ok(o)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse_args(&args).and_then(|o| run(&o)) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("xtask: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

fn run(o: &Opts) -> Result<bool> {
    if o.command == "help" {
        print!("{USAGE}");
        return Ok(true);
    }
    let ws = load_workspace()?;
    match o.command.as_str() {
        "test" => cmd_test(&ws, o),
        "check" => cmd_check(&ws, o),
        "affected" => cmd_affected(&ws, o),
        c => bail!("unknown command {c}\n\n{USAGE}"),
    }
}

fn load_workspace() -> Result<Workspace> {
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.args(["metadata", "--no-deps", "--format-version", "1", "--offline"]);
    let (ok, out) = proc::output(&mut c)?;
    if !ok {
        bail!("cargo metadata failed");
    }
    let meta: serde_json::Value = serde_json::from_str(&out).context("reading cargo metadata")?;
    Workspace::from_metadata(&meta)
}

/// The packages to work on, and what the branch changed.
struct Selection {
    packages: BTreeSet<String>,
    outside: OutsideInputs,
    affected: Option<Affected>,
    files: Vec<String>,
}

fn select(ws: &Workspace, o: &Opts) -> Result<Selection> {
    let outside = ws.outside_inputs(git::outside_grep(&ws.root)?.lines());
    if !o.packages.is_empty() {
        let unknown: Vec<&String> = o.packages.iter().filter(|p| !ws.packages.contains_key(*p)).collect();
        if !unknown.is_empty() {
            bail!("no such package: {}", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
        }
        return Ok(Selection { packages: o.packages.iter().cloned().collect(), outside, affected: None, files: Vec::new() });
    }
    if o.all {
        // The changed files only decide whether the web checks run.
        let files = git::changed_files(&ws.root, &o.base).unwrap_or_default();
        return Ok(Selection { packages: ws.packages.keys().cloned().collect(), outside, affected: None, files });
    }
    let files = git::changed_files(&ws.root, &o.base)?;
    let affected = ws.affected(&files, &outside);
    Ok(Selection { packages: affected.test.clone(), outside, affected: Some(affected), files })
}

fn describe(ws: &Workspace, o: &Opts, sel: &Selection) {
    if !o.packages.is_empty() {
        println!("xtask: {} packages named with -p", sel.packages.len());
        return;
    }
    if o.all {
        println!("xtask: every package ({})", sel.packages.len());
        return;
    }
    let a = sel.affected.as_ref().expect("set without -p or --all");
    println!(
        "xtask: {} files changed against the merge base with {}; {} packages changed, {} of {} to test",
        sel.files.len(),
        o.base,
        a.why.len(),
        a.test.len(),
        ws.packages.len()
    );
    if let Some(g) = &a.global {
        println!("xtask: {g} changed, which can change every package");
    }
    if !a.unmapped.is_empty() {
        let shown: Vec<&str> = a.unmapped.iter().take(8).map(String::as_str).collect();
        let more = if a.unmapped.len() > 8 { format!(", and {} more", a.unmapped.len() - 8) } else { String::new() };
        println!(
            "xtask: {} changed files belong to no package and no package names them (not tested by cargo): {}{more}",
            a.unmapped.len(),
            shown.join(", ")
        );
    }
}

fn cmd_affected(ws: &Workspace, o: &Opts) -> Result<bool> {
    let sel = select(ws, o)?;
    if o.why
        && let Some(a) = &sel.affected
    {
        for (p, files) in &a.why {
            let only = if files.iter().all(|(_, t)| *t) { ", its tests only" } else { "" };
            let more = if files.len() > 1 { format!(" and {} more", files.len() - 1) } else { String::new() };
            eprintln!("changed: {p} ({}{more}){only}", files[0].0);
        }
        if let Some(g) = &a.global {
            eprintln!("changed: every package ({g})");
        }
        for f in &a.unmapped {
            eprintln!("unmapped: {f}");
        }
    }
    for p in &sel.packages {
        println!("{p}");
    }
    Ok(true)
}

/// `-p a -p b ...`, or `--workspace` when that is every package.
fn package_args(ws: &Workspace, packages: &BTreeSet<String>) -> Vec<String> {
    if packages.len() == ws.packages.len() {
        return vec!["--workspace".into()];
    }
    packages.iter().flat_map(|p| ["-p".to_owned(), p.clone()]).collect()
}

/// The desktop and host checks of `cli`. Both build into this tree's
/// one target directory, so they run one after the other: two cargo
/// processes on one target only wait on each other's lock.
fn check_commands(ws: &Workspace) -> Vec<(String, Command)> {
    let mut desktop = proc::cargo(&ws.root);
    desktop.args(["check", "--locked", "-p", "cli"]);
    let mut host = proc::cargo(&ws.root);
    host.args(["check", "--locked", "-p", "cli", "--no-default-features", "--features", "host"]);
    vec![("check cli".into(), desktop), ("check cli, host features".into(), host)]
}

#[derive(Clone, Copy, PartialEq)]
enum Tier {
    Normal,
    Disc,
}

fn nextest_command(ws: &Workspace, packages: &BTreeSet<String>, tier: Tier, list: bool) -> Command {
    let mut c = proc::cargo(&ws.root);
    if list {
        c.args(["nextest", "list", "--locked", "--profile", "xtask", "--message-format", "json"]);
    } else {
        c.args(["nextest", "run", "--locked", "--profile", "xtask", "--no-fail-fast"]);
        c.arg(if tier == Tier::Disc { "--no-tests=fail" } else { "--no-tests=warn" });
    }
    if tier == Tier::Disc {
        c.args(["--run-ignored", "ignored-only", "-E", "not test(/bench|slow/)"]);
    }
    c.args(package_args(ws, packages));
    c
}

fn doc_packages(ws: &Workspace, packages: &BTreeSet<String>) -> BTreeSet<String> {
    packages.iter().filter(|p| ws.packages[*p].doctests).cloned().collect()
}

fn doc_command(ws: &Workspace, packages: &BTreeSet<String>) -> Command {
    let mut c = proc::cargo(&ws.root);
    c.args(["test", "--locked", "--doc"]);
    for p in packages {
        c.args(["-p", p]);
    }
    c
}

/// The disc tier's `cargo test` steps: (label, package, cargo arguments,
/// extra environment). Each runs when its package is being tested.
const DISC_STEPS: &[(&str, &str, &[&str], &[(&str, &str)])] = &[
    ("lift oracle", "lift", &["test", "--locked", "-p", "lift", "--features", "oracle-fixtures", "--test", "oracle"], &[]),
    ("engine differentials", "engine", &["test", "--locked", "-p", "engine", "--features", "differential"], &[]),
    (
        "native-render differentials",
        "native-render",
        &["test", "--locked", "-p", "native-render", "--features", "differential"],
        &[],
    ),
    ("runtime on flat RAM", "runtime", &["test", "--locked", "-p", "runtime"], &[("H2_EMIT_FLAT_RAM", "1"), ("H2_FLAT_RAM", "1")]),
    (
        "emit on flat RAM",
        "emit",
        &[
            "test", "--locked", "-p", "emit", "--test", "fast_copy", "--test", "perf", "--test", "differential", "--test",
            "second_executor",
        ],
        &[("H2_EMIT_FLAT_RAM", "1")],
    ),
];

/// The disc variables to set: each one unset in `env` whose file is in
/// `disc`. `fixtures` are the oracle's `.jsonl` files found there.
fn disc_env(
    disc: Option<&Path>,
    env: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&Path) -> bool,
    fixtures: &[PathBuf],
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(disc) = disc else { return out };
    for (var, file) in [("H2_XBE", "default.xbe"), ("H2_DISC", "halo2.iso"), ("H1_ISO", "halo1.iso")] {
        let path = disc.join(file);
        if env(var).is_none() && exists(&path) {
            out.insert(var.to_owned(), path.to_string_lossy().into_owned());
        }
    }
    if env("H2_ORACLE_FIXTURES").is_none() && !fixtures.is_empty() {
        let joined: Vec<String> = fixtures.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        out.insert("H2_ORACLE_FIXTURES".into(), joined.join(":"));
    }
    out
}

/// The oracle fixture files in `disc/oracle/`, sorted.
fn oracle_fixtures(disc: Option<&Path>) -> Vec<PathBuf> {
    let Some(dir) = disc.map(|d| d.join("oracle")) else { return Vec::new() };
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    out.sort();
    out
}

fn web_commands(ws: &Workspace) -> Vec<(String, Command)> {
    let web = ws.root.join("web");
    let bun = |args: &[&str]| {
        let mut c = Command::new("bun");
        c.current_dir(&web).args(args);
        c
    };
    let mut out = Vec::new();
    if !web.join("node_modules").is_dir() {
        out.push(("web: bun install".to_owned(), bun(&["install", "--frozen-lockfile"])));
    }
    out.push(("web: bun test".to_owned(), bun(&["test"])));
    out.push(("web: typecheck".to_owned(), bun(&["run", "typecheck"])));
    out
}

/// What became of each step, for the final lines.
struct Steps(Vec<(String, &'static str, String)>);

impl Steps {
    fn add(&mut self, label: impl Into<String>, ok: bool, detail: impl Into<String>) {
        self.0.push((label.into(), if ok { "PASS" } else { "FAIL" }, detail.into()));
    }
    fn note(&mut self, label: impl Into<String>, status: &'static str, detail: impl Into<String>) {
        self.0.push((label.into(), status, detail.into()));
    }
    fn ok(&self) -> bool {
        self.0.iter().all(|(_, s, _)| *s != "FAIL")
    }
    fn print(&self) {
        let width = self.0.iter().map(|(l, _, _)| l.len()).max().unwrap_or(0);
        println!();
        for (label, status, detail) in &self.0 {
            let sep = if detail.is_empty() { "" } else { "  " };
            println!("xtask: {label:width$}  {status:10}{sep}{detail}");
        }
        println!("xtask: {}", if self.ok() { "PASSED" } else { "FAILED" });
    }
}

/// One nextest run: its exit status, and the tests it ran and skipped.
struct NextestRun {
    ok: bool,
    cases: Option<Vec<report::Case>>,
    skipped: Option<BTreeMap<String, usize>>,
}

fn run_nextest(ws: &Workspace, packages: &BTreeSet<String>, tier: Tier, env: &BTreeMap<String, String>) -> Result<NextestRun> {
    // The xtask profile's JUnit report (.config/nextest.toml).
    let junit = ws.target_dir.join("nextest/xtask/junit.xml");
    let _ = std::fs::remove_file(&junit);
    let mut c = nextest_command(ws, packages, tier, false);
    c.envs(env);
    let ok = proc::run(&mut c)?;
    let cases = match std::fs::read_to_string(&junit) {
        Ok(text) => Some(report::junit_cases(&xml::parse(&text).with_context(|| format!("reading {}", junit.display()))?)),
        Err(_) => None,
    };
    let mut skipped = None;
    if cases.is_some() {
        let mut l = nextest_command(ws, packages, tier, true);
        l.envs(env);
        if let Ok((true, out)) = proc::output(&mut l)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&out)
        {
            skipped = Some(report::skipped_from_list(&v));
        }
    }
    Ok(NextestRun { ok, cases, skipped })
}

/// Adds a `cargo test` run's blocks to `summary` under `package_of`.
fn add_libtest(summary: &mut Summary, blocks: &[report::LibtestBlock], prefix: &str, package_of: impl Fn(&report::LibtestBlock) -> String) {
    for b in blocks {
        if b.passed + b.failed + b.ignored == 0 && b.failures.is_empty() {
            continue;
        }
        let s = summary.entry(package_of(b)).or_default();
        s.passed += b.passed;
        s.failed += b.failed.max(b.failures.len());
        *s.skipped.get_or_insert(0) += b.ignored;
        for (name, out) in &b.failures {
            s.failures.push((format!("{prefix}{name}"), out.clone()));
        }
    }
}

fn cmd_test(ws: &Workspace, o: &Opts) -> Result<bool> {
    let sel = select(ws, o)?;
    describe(ws, o, &sel);
    let web = o.web || sel.files.iter().any(|f| f.starts_with("web/"));

    // The ledger: packages that passed before with the same inputs.
    let command = "nextest run --profile xtask; test --doc";
    let mut keys = BTreeMap::new();
    let mut ledgered = Vec::new();
    let ledger_dir = ledger::dir();
    let mut to_run = sel.packages.clone();
    if !o.no_ledger && !sel.packages.is_empty() {
        let inputs = ledger::read_inputs(ws, &sel.outside, &sel.packages, command)?;
        for p in &sel.packages {
            let Some(text) = ledger::key_text(ws, &sel.outside, &inputs, p) else { continue };
            let key = ledger::hash(&text);
            if ledger::passed(&ledger_dir, &key) {
                ledgered.push(p.clone());
                to_run.remove(p);
            }
            keys.insert(p.clone(), key);
        }
        if !ledgered.is_empty() {
            println!("xtask: {} packages passed before with the same inputs (ledger {})", ledgered.len(), ledger_dir.display());
        }
    }

    let checks = if sel.packages.contains("cli") { check_commands(ws) } else { Vec::new() };
    let docs = doc_packages(ws, &to_run);
    let disc_dir = git::disc_dir(&ws.root);
    let fixtures = oracle_fixtures(disc_dir.as_deref());
    let disc_vars = disc_env(disc_dir.as_deref(), |v| std::env::var(v).ok(), Path::exists, &fixtures);
    let have = |var: &str| std::env::var_os(var).is_some() || disc_vars.contains_key(var);
    let disc_steps: Vec<_> = DISC_STEPS.iter().filter(|s| o.disc && sel.packages.contains(s.1)).collect();
    let webs = if web { web_commands(ws) } else { Vec::new() };

    if o.dry_run {
        println!("xtask: packages to test ({}):", to_run.len());
        for p in &to_run {
            println!("  {p}");
        }
        println!("xtask: commands:");
        for (_, c) in &checks {
            println!("  {}", proc::show(c));
        }
        if !to_run.is_empty() {
            println!("  {}", proc::show(&nextest_command(ws, &to_run, Tier::Normal, false)));
            println!("  {}", proc::show(&nextest_command(ws, &to_run, Tier::Normal, true)));
        }
        if !docs.is_empty() {
            println!("  {}", proc::show(&doc_command(ws, &docs)));
        }
        if o.disc && !sel.packages.is_empty() {
            let mut c = nextest_command(ws, &sel.packages, Tier::Disc, false);
            c.envs(&disc_vars);
            println!("  {}", proc::show(&c));
            for (_, _, args, env) in &disc_steps {
                let mut c = proc::cargo(&ws.root);
                c.args(*args).envs(&disc_vars).envs(env.iter().copied());
                println!("  {}", proc::show(&c));
            }
            if !have("H2_XBE") {
                println!("xtask: no disc (no default.xbe in disc/ and no H2_XBE): the disc tier would fail");
            }
        }
        for (_, c) in &webs {
            println!("  {}   (beside the Rust tests)", proc::show(c));
        }
        return Ok(true);
    }

    let mut steps = Steps(Vec::new());

    // The web app's checks run beside everything else, their output held
    // until they finish so it does not interleave with cargo's.
    let web_thread = (!webs.is_empty()).then(|| {
        std::thread::spawn(move || {
            let mut results = Vec::new();
            for (label, cmd) in webs {
                let shown = proc::show(&cmd);
                let r = proc::run_merged(cmd, false);
                let failed = !matches!(r, Ok((true, _)));
                results.push((label, shown, r.map_err(|e| format!("{e:#}"))));
                if failed {
                    break;
                }
            }
            results
        })
    });

    if checks.is_empty() {
        steps.note("check cli", "NOT NEEDED", "cli is not affected");
    }
    for (label, mut c) in checks {
        let ok = proc::run(&mut c)?;
        steps.add(label, ok, "");
    }

    let mut summary = Summary::new();
    // Whether every failure of the runs is a failed test the summary names
    // (not a build error): only then are the other packages' passes real.
    let mut explained = true;
    if to_run.is_empty() {
        steps.note("tests", if ledgered.is_empty() { "NOT NEEDED" } else { "LEDGER" }, "no package to run");
    } else {
        let r = run_nextest(ws, &to_run, Tier::Normal, &BTreeMap::new())?;
        match &r.cases {
            Some(cases) => {
                add_cases(&mut summary, cases, r.skipped.as_ref());
                let failed = cases.iter().filter(|c| c.failed).count();
                explained = r.ok || failed > 0;
                let detail = format!("{} tests in {} packages, {failed} failed", cases.len(), to_run.len());
                if !explained {
                    steps.add("tests", false, format!("{detail}; nextest exited nonzero anyway (see above)"));
                } else {
                    steps.add("tests", failed == 0 && r.ok, detail);
                }
            }
            None => {
                explained = false;
                steps.add("tests", false, "nextest ran no tests: a build error, or cargo-nextest is missing (see above)");
            }
        }
    }

    let mut doc_explained = true;
    if !docs.is_empty() {
        let (ok, out) = proc::run_merged(doc_command(ws, &docs), true)?;
        let blocks = report::parse_libtest(&out);
        let by_lib: BTreeMap<String, String> =
            ws.packages.iter().filter_map(|(n, p)| p.lib_name.clone().map(|l| (l, n.clone()))).collect();
        let doc_blocks: Vec<_> = blocks.into_iter().filter(|b| b.doc).collect();
        add_libtest(&mut summary, &doc_blocks, "doc: ", |b| by_lib.get(&b.header).cloned().unwrap_or_else(|| b.header.clone()));
        let failed: usize = doc_blocks.iter().map(|b| b.failed.max(b.failures.len())).sum();
        doc_explained = ok || failed > 0;
        steps.add("doctests", ok, format!("{} packages, {failed} failed", docs.len()));
    }

    print!("{}", report::render("tests, per package", &summary, &ledgered));

    // Record the passes: each package with no failure, from runs whose
    // every failure is accounted for.
    if !o.no_ledger && explained && doc_explained {
        let head = git::git(&ws.root, &["rev-parse", "--short=12", "HEAD"]).unwrap_or_default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        for p in &to_run {
            let Some(key) = keys.get(p) else { continue };
            if summary.get(p).is_none_or(|s| s.failed == 0) {
                let note = format!("{p} passed at unix {now}, commit {}, in {}\n", head.trim(), ws.root.display());
                ledger::record(&ledger_dir, key, &note)?;
            }
        }
    }

    if o.disc {
        disc_tier(ws, &sel.packages, &disc_vars, &disc_steps, &have, &mut steps)?;
    }

    if let Some(t) = web_thread {
        let results = t.join().map_err(|_| anyhow::anyhow!("the web thread panicked"))?;
        for (label, shown, r) in results {
            println!("\n$ {shown}");
            match r {
                Ok((ok, out)) => {
                    print!("{out}");
                    steps.add(label, ok, "");
                }
                Err(e) => steps.add(label, false, e),
            }
        }
    } else if !o.web {
        steps.note("web", "NOT NEEDED", "web/ did not change (--web runs it)");
    }

    steps.print();
    Ok(steps.ok())
}

type DiscStep = (&'static str, &'static str, &'static [&'static str], &'static [(&'static str, &'static str)]);

fn disc_tier(
    ws: &Workspace,
    packages: &BTreeSet<String>,
    disc_vars: &BTreeMap<String, String>,
    disc_steps: &[&DiscStep],
    have: &dyn Fn(&str) -> bool,
    steps: &mut Steps,
) -> Result<()> {
    // An empty package list would be the whole workspace to nextest.
    if packages.is_empty() {
        steps.note("disc tier", "NOT NEEDED", "no package to test");
        return Ok(());
    }
    if !have("H2_XBE") {
        steps.add("disc tier", false, "no disc: put default.xbe (and the ISOs) in disc/, or set H2_XBE");
        return Ok(());
    }
    let mut summary = Summary::new();
    let r = run_nextest(ws, packages, Tier::Disc, disc_vars)?;
    match &r.cases {
        Some(cases) if !cases.is_empty() => {
            add_cases(&mut summary, cases, r.skipped.as_ref());
            let failed = cases.iter().filter(|c| c.failed).count();
            steps.add("disc tests", r.ok && failed == 0, format!("{} tests, {failed} failed", cases.len()));
        }
        _ => steps.add("disc tests", false, "no disc test ran; a disc tier that runs nothing proves nothing"),
    }
    for (label, package, args, env) in disc_steps {
        if *package == "lift" && !have("H2_ORACLE_FIXTURES") {
            steps.add(*label, false, "no fixtures: set H2_ORACLE_FIXTURES, or put the .jsonl files in disc/oracle/");
            continue;
        }
        let mut c = proc::cargo(&ws.root);
        c.args(*args).envs(disc_vars).envs(env.iter().copied());
        let (ok, out) = proc::run_merged(c, true)?;
        let blocks = report::parse_libtest(&out);
        let ran: usize = blocks.iter().map(|b| b.passed + b.failed).sum();
        add_libtest(&mut summary, &blocks, &format!("{label}: "), |_| (*package).to_owned());
        if ran == 0 {
            steps.add(*label, false, "ran no tests");
        } else {
            steps.add(*label, ok, format!("{ran} tests"));
        }
    }
    print!("{}", report::render("disc tier, per package", &summary, &[]));
    Ok(())
}

fn cmd_check(ws: &Workspace, o: &Opts) -> Result<bool> {
    let files = git::changed_files(&ws.root, &o.base)?;
    let docs: Vec<String> = files
        .iter()
        .filter(|f| f.starts_with("docs/") && f.ends_with(".md") && ws.root.join(f).is_file())
        .cloned()
        .collect();
    let python = |args: &[&str]| {
        let mut c = Command::new("python3");
        c.current_dir(&ws.root).args(args);
        c
    };
    let mut cmds = check_commands(ws);
    cmds.push(("crate rules".into(), python(&["scripts/crate-graph.py", "layers", "--check"])));
    cmds.push(("crate table".into(), python(&["scripts/crate-table.py", "--check"])));
    if !docs.is_empty() {
        let mut args = vec!["scripts/docs-check.py"];
        args.extend(docs.iter().map(String::as_str));
        cmds.push((format!("docs check ({} changed)", docs.len()), python(&args)));
    }
    if o.dry_run {
        for (_, c) in &cmds {
            println!("  {}", proc::show(c));
        }
        return Ok(true);
    }
    let mut steps = Steps(Vec::new());
    for (label, mut c) in cmds {
        let ok = proc::run(&mut c)?;
        steps.add(label, ok, "");
    }
    if docs.is_empty() {
        steps.note("docs check", "NOT NEEDED", format!("no doc changed against {}", o.base));
    }
    steps.print();
    Ok(steps.ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_the_command_line() {
        let o = parse_args(&args("test --base master -p a,b --package c --disc --dry-run")).unwrap();
        assert_eq!(o.command, "test");
        assert_eq!(o.base, "master");
        assert_eq!(o.packages, vec!["a", "b", "c"]);
        assert!(o.disc && o.dry_run && !o.web && !o.all);
        let o = parse_args(&args("affected --base=HEAD~3 --why")).unwrap();
        assert_eq!((o.command.as_str(), o.base.as_str(), o.why), ("affected", "HEAD~3", true));
        assert_eq!(parse_args(&[]).unwrap().command, "help");
        assert!(parse_args(&args("test --frobnicate")).is_err());
        assert!(parse_args(&args("test --base")).is_err());
    }

    #[test]
    fn disc_variables_come_from_the_disc_directory_when_unset() {
        let disc = Path::new("/d");
        let fixtures = vec![PathBuf::from("/d/oracle/a.jsonl"), PathBuf::from("/d/oracle/b.jsonl")];
        let env = |v: &str| (v == "H1_ISO").then(|| "/mine/halo1.iso".to_owned());
        let exists = |p: &Path| p != Path::new("/d/halo2.iso");
        let got = disc_env(Some(disc), env, exists, &fixtures);
        assert_eq!(got.get("H2_XBE").map(String::as_str), Some("/d/default.xbe"));
        assert_eq!(got.get("H2_DISC"), None, "no file");
        assert_eq!(got.get("H1_ISO"), None, "already set");
        assert_eq!(got.get("H2_ORACLE_FIXTURES").map(String::as_str), Some("/d/oracle/a.jsonl:/d/oracle/b.jsonl"));
        assert!(disc_env(None, |_| None, |_| true, &fixtures).is_empty());
    }

    #[test]
    fn doctest_blocks_count_per_package() {
        let blocks = vec![
            report::LibtestBlock { header: "live_server".into(), doc: true, passed: 2, ignored: 1, ..Default::default() },
            report::LibtestBlock {
                header: "tags".into(),
                doc: true,
                passed: 1,
                failed: 1,
                failures: vec![("src/lib.rs - f (line 3)".into(), "boom".into())],
                ..Default::default()
            },
            report::LibtestBlock { header: "empty".into(), doc: true, ..Default::default() },
        ];
        let mut summary = Summary::new();
        add_libtest(&mut summary, &blocks, "doc: ", |b| b.header.replace('_', "-"));
        assert_eq!(summary["live-server"].passed, 2);
        assert_eq!(summary["live-server"].skipped, Some(1));
        assert_eq!(summary["tags"].failed, 1);
        assert_eq!(summary["tags"].failures[0].0, "doc: src/lib.rs - f (line 3)");
        assert!(!summary.contains_key("empty"));
    }
}
