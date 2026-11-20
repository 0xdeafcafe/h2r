//! Test results per package: from nextest's JUnit report, from
//! `cargo nextest list --message-format json` (the tests a run skipped),
//! and from `cargo test`'s own output (doctests and the disc steps).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;

use crate::xml::Element;

/// One test that ran.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    pub package: String,
    pub name: String,
    pub secs: f64,
    pub failed: bool,
    /// The failure message and captured output, for a failed test.
    pub output: String,
}

/// A package's line in the summary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackageSummary {
    pub passed: usize,
    pub failed: usize,
    pub skipped: Option<usize>,
    pub slowest: Option<(String, f64)>,
    /// (test, output) for each failed test.
    pub failures: Vec<(String, String)>,
}

pub type Summary = BTreeMap<String, PackageSummary>;

/// The package a nextest binary id belongs to: `live-server`,
/// `live-server::it`, `cli::bin/h2` all name `live-server` or `cli`.
pub fn package_of_binary(binary_id: &str) -> &str {
    binary_id.split("::").next().unwrap_or(binary_id)
}

/// The tests in a nextest JUnit report. Each `<testsuite>` is a test
/// binary; a `<testcase>` with a `<failure>` or `<error>` failed (a test
/// killed by its time limit included). A flaky test that passed on a retry
/// carries only `<flakyFailure>` and counts as passed.
pub fn junit_cases(doc: &Element) -> Vec<Case> {
    let mut suites = Vec::new();
    if doc.name == "testsuite" {
        suites.push(doc);
    }
    doc.descendants_named("testsuite", &mut suites);
    let mut out = Vec::new();
    for suite in suites {
        let package = package_of_binary(suite.attr("name").unwrap_or("?")).to_owned();
        for case in suite.children_named("testcase") {
            let failures: Vec<&Element> =
                case.children.iter().filter(|c| c.name == "failure" || c.name == "error").collect();
            let mut output = String::new();
            if !failures.is_empty() {
                for f in &failures {
                    if let Some(m) = f.attr("message") {
                        let _ = writeln!(output, "{m}");
                    }
                    push_text(&mut output, &f.text);
                    for c in &f.children {
                        push_text(&mut output, &c.text);
                    }
                }
                for c in case.children.iter().filter(|c| c.name == "system-out" || c.name == "system-err") {
                    push_text(&mut output, &c.text);
                }
            }
            out.push(Case {
                package: package.clone(),
                name: case.attr("name").unwrap_or("?").to_owned(),
                secs: case.attr("time").and_then(|t| t.parse().ok()).unwrap_or(0.0),
                failed: !failures.is_empty(),
                output,
            });
        }
    }
    out
}

fn push_text(out: &mut String, text: &str) {
    let t = text.trim_matches('\n');
    if !t.trim().is_empty() {
        out.push_str(t);
        out.push('\n');
    }
}

/// Tests the run left out, per package, from
/// `cargo nextest list --message-format json` with the run's own filters:
/// every test case whose `filter-match` is not `matches` (ignored tests
/// in a normal run, the others in an `--run-ignored ignored-only` run).
pub fn skipped_from_list(list: &Value) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for (binary_id, suite) in list["rust-suites"].as_object().into_iter().flatten() {
        let package = suite["package-name"].as_str().unwrap_or_else(|| package_of_binary(binary_id)).to_owned();
        let n = suite["testcases"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, t)| t["filter-match"]["status"].as_str() != Some("matches"))
            .count();
        *out.entry(package).or_insert(0) += n;
    }
    out
}

/// Adds `cases` (and the skip counts, when known) to `summary`.
pub fn add_cases(summary: &mut Summary, cases: &[Case], skipped: Option<&BTreeMap<String, usize>>) {
    for c in cases {
        let s = summary.entry(c.package.clone()).or_default();
        if c.failed {
            s.failed += 1;
            s.failures.push((c.name.clone(), c.output.clone()));
        } else {
            s.passed += 1;
        }
        if s.slowest.as_ref().is_none_or(|(_, t)| c.secs > *t) {
            s.slowest = Some((c.name.clone(), c.secs));
        }
    }
    if let Some(skipped) = skipped {
        for (package, n) in skipped {
            let s = summary.entry(package.clone()).or_default();
            *s.skipped.get_or_insert(0) += n;
        }
    }
}

/// One block of `cargo test` output: a doctest run or a test binary.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LibtestBlock {
    /// The crate after `Doc-tests`, or the binary after `Running`.
    pub header: String,
    pub doc: bool,
    pub passed: usize,
    pub failed: usize,
    pub ignored: usize,
    /// (test, its captured output) for each failure.
    pub failures: Vec<(String, String)>,
}

/// Reads `cargo test` output with stderr and stdout merged (cargo prints
/// the `Running` and `Doc-tests` headers on stderr, the results on stdout).
pub fn parse_libtest(text: &str) -> Vec<LibtestBlock> {
    let mut blocks: Vec<LibtestBlock> = Vec::new();
    let mut current_output: Option<(String, String)> = None;
    let mut outputs: BTreeMap<String, String> = BTreeMap::new();
    let flush = |cur: &mut Option<(String, String)>, outputs: &mut BTreeMap<String, String>| {
        if let Some((name, out)) = cur.take() {
            outputs.insert(name, out);
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix("Doc-tests ") {
            flush(&mut current_output, &mut outputs);
            blocks.push(LibtestBlock { header: name.trim().to_owned(), doc: true, ..LibtestBlock::default() });
            continue;
        }
        if let Some(rest) = t.strip_prefix("Running ") {
            flush(&mut current_output, &mut outputs);
            blocks.push(LibtestBlock { header: rest.trim().to_owned(), ..LibtestBlock::default() });
            continue;
        }
        if let Some(name) = t.strip_prefix("---- ").and_then(|r| r.strip_suffix(" ----")) {
            flush(&mut current_output, &mut outputs);
            let name = name.strip_suffix(" stdout").unwrap_or(name).to_owned();
            current_output = Some((name, String::new()));
            continue;
        }
        if t == "failures:" || t.starts_with("test result: ") {
            flush(&mut current_output, &mut outputs);
        }
        if let Some((_, out)) = current_output.as_mut() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if blocks.is_empty() {
            blocks.push(LibtestBlock::default());
        }
        let b = blocks.last_mut().expect("pushed above");
        if let Some(name) = t.strip_prefix("test ").and_then(|r| r.strip_suffix(" ... FAILED")) {
            b.failures.push((name.to_owned(), String::new()));
        } else if let Some(r) = t.strip_prefix("test result: ") {
            for part in r.split(';') {
                let words: Vec<&str> = part.split_whitespace().collect();
                let n = |w: &[&str]| w.iter().rev().nth(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
                match words.last() {
                    Some(&"passed") => b.passed += n(&words),
                    Some(&"failed") => b.failed += n(&words),
                    Some(&"ignored") => b.ignored += n(&words),
                    _ => {}
                }
            }
            // Attach captured output now the block's failures are known.
            let outs = std::mem::take(&mut outputs);
            for (name, out) in &mut b.failures {
                if out.is_empty()
                    && let Some(o) = outs.get(name)
                {
                    *out = o.clone();
                }
            }
        }
    }
    blocks
}

/// The first lines worth reading from a failed test's output: from the
/// first panic when there is one, else the start.
pub fn excerpt(output: &str, lines: usize) -> Vec<&str> {
    let all: Vec<&str> = output.lines().collect();
    let start = all.iter().position(|l| l.contains("panicked at")).unwrap_or(0);
    all[start..].iter().take(lines).copied().collect()
}

/// The summary table and the failed tests, as printed.
pub fn render(title: &str, summary: &Summary, ledger: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\n{title}");
    let width = summary.keys().map(String::len).chain(ledger.iter().map(String::len)).max().unwrap_or(5).max(7);
    let _ = writeln!(out, "  {:width$}  {:>6}  {:>6}  {:>7}  slowest", "package", "passed", "failed", "skipped");
    let (mut passed, mut failed, mut skipped) = (0, 0, 0);
    for (package, s) in summary {
        passed += s.passed;
        failed += s.failed;
        skipped += s.skipped.unwrap_or(0);
        let slow = s.slowest.as_ref().map_or(String::new(), |(n, t)| format!("{t:.1}s {n}"));
        let skip = s.skipped.map_or("-".to_owned(), |n| n.to_string());
        let mark = if s.failed > 0 { "!" } else { " " };
        let _ = writeln!(out, "{mark} {package:width$}  {:>6}  {:>6}  {skip:>7}  {slow}", s.passed, s.failed);
    }
    for package in ledger {
        let _ = writeln!(out, "  {package:width$}  passed before with the same inputs (ledger)");
    }
    let _ = writeln!(out, "  {:width$}  {passed:>6}  {failed:>6}  {skipped:>7}", "total");
    for (package, s) in summary {
        for (name, output) in &s.failures {
            let _ = writeln!(out, "\nFAILED {package}: {name}");
            for l in excerpt(output, 12) {
                let _ = writeln!(out, "    {l}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const JUNIT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="5" failures="2" errors="0">
    <testsuite name="tags" tests="2" disabled="0" errors="0" failures="0">
        <testcase name="field::reads" classname="tags" time="0.010"/>
        <testcase name="field::writes" classname="tags" time="1.500"/>
    </testsuite>
    <testsuite name="live-server::it" tests="2" disabled="0" errors="0" failures="1">
        <testcase name="lobby::joins" classname="live-server::it" time="0.200">
            <failure message="test failure" type="test failure">thread 'lobby::joins' panicked at crates/live/live-server/tests/lobby.rs:12:5:
assertion `left == right` failed
  left: 1
 right: 2</failure>
            <system-out>setting up</system-out>
        </testcase>
        <testcase name="lobby::flaky" classname="live-server::it" time="0.300">
            <flakyFailure message="test failure" type="test failure"/>
        </testcase>
    </testsuite>
    <testsuite name="cli::bin/h2" tests="1" disabled="0" errors="0" failures="1">
        <testcase name="hangs" classname="cli::bin/h2" time="300.000">
            <failure type="test timeout"/>
            <system-out>still going</system-out>
        </testcase>
    </testsuite>
</testsuites>"#;

    #[test]
    fn reads_a_junit_report() {
        let cases = junit_cases(&crate::xml::parse(JUNIT).unwrap());
        assert_eq!(cases.len(), 5);
        assert_eq!(cases[0].package, "tags");
        assert!(!cases[0].failed);
        assert_eq!(cases[2].package, "live-server");
        assert!(cases[2].failed);
        assert!(cases[2].output.contains("panicked at crates/live/live-server/tests/lobby.rs"), "{}", cases[2].output);
        assert!(cases[2].output.contains("setting up"));
        assert!(!cases[3].failed, "a flaky pass is a pass");
        assert_eq!(cases[4].package, "cli");
        assert!(cases[4].failed, "a timeout is a failure");
        assert_eq!(cases[4].secs, 300.0);
    }

    #[test]
    fn aggregates_per_package() {
        let cases = junit_cases(&crate::xml::parse(JUNIT).unwrap());
        let list = json!({ "rust-suites": {
            "tags": { "package-name": "tags", "testcases": {
                "field::reads": { "ignored": false, "filter-match": { "status": "matches" } },
                "field::disc": { "ignored": true, "filter-match": { "status": "mismatch", "reason": "ignored" } },
            }},
            "live-server::it": { "package-name": "live-server", "testcases": {} },
        }});
        let skipped = skipped_from_list(&list);
        assert_eq!(skipped["tags"], 1);
        assert_eq!(skipped["live-server"], 0);
        let mut summary = Summary::new();
        add_cases(&mut summary, &cases, Some(&skipped));
        let tags = &summary["tags"];
        assert_eq!((tags.passed, tags.failed, tags.skipped), (2, 0, Some(1)));
        assert_eq!(tags.slowest, Some(("field::writes".to_owned(), 1.5)));
        let live = &summary["live-server"];
        assert_eq!((live.passed, live.failed, live.skipped), (1, 1, Some(0)));
        assert_eq!(live.failures[0].0, "lobby::joins");
        assert_eq!(summary["cli"].skipped, None);

        let text = render("tests", &summary, &["hs".to_owned()]);
        assert!(text.contains("FAILED live-server: lobby::joins"), "{text}");
        assert!(text.contains("    thread 'lobby::joins' panicked at"), "{text}");
        assert!(text.contains("FAILED cli: hangs"), "{text}");
        assert!(text.contains("hs"), "{text}");
        assert!(text.lines().any(|l| l.starts_with("  total") && l.contains(" 3 ") && l.contains(" 2 ")), "{text}");
    }

    #[test]
    fn reads_cargo_test_output() {
        let text = "\
   Compiling lift v0.1.0
     Running tests/oracle.rs (target/debug/deps/oracle-0123456789abcdef)

running 2 tests
test harness_counts ... ok
test lowering_matches ... FAILED

failures:

---- lowering_matches stdout ----
oracle: 10 match
thread 'lowering_matches' panicked at crates/recompiler/lift/tests/oracle.rs:497:5:
encodings differ

failures:
    lowering_matches

test result: FAILED. 1 passed; 1 failed; 3 ignored; 0 measured; 0 filtered out; finished in 0.50s

   Doc-tests tags

running 4 tests
test src/lib.rs - field (line 10) ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
";
        let blocks = parse_libtest(text);
        assert_eq!(blocks.len(), 2);
        let b = &blocks[0];
        assert!(!b.doc);
        assert!(b.header.starts_with("tests/oracle.rs"));
        assert_eq!((b.passed, b.failed, b.ignored), (1, 1, 3));
        assert_eq!(b.failures[0].0, "lowering_matches");
        assert!(b.failures[0].1.contains("encodings differ"), "{:?}", b.failures);
        let d = &blocks[1];
        assert!(d.doc);
        assert_eq!(d.header, "tags");
        assert_eq!((d.passed, d.failed), (4, 0));
    }

    #[test]
    fn excerpts_start_at_the_panic() {
        let out = "line one\nline two\nthread 'x' panicked at a.rs:1:1:\nboom\n";
        assert_eq!(excerpt(out, 2), vec!["thread 'x' panicked at a.rs:1:1:", "boom"]);
        assert_eq!(excerpt("a\nb\nc", 2), vec!["a", "b"]);
    }
}
