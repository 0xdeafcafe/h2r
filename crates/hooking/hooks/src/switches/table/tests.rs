use std::path::{Path, PathBuf};

use super::*;

#[test]
fn the_table_is_within_its_budget_and_names_each_switch_once() {
    assert!(
        TABLE.len() <= BUDGET,
        "{} switches, budget {BUDGET}: delete one, or raise the budget with a reason",
        TABLE.len()
    );
    let mut names: Vec<&str> = TABLE.iter().map(|s| s.name).collect();
    names.sort();
    for w in names.windows(2) {
        assert_ne!(w[0], w[1], "{} is listed twice", w[0]);
    }
    for s in TABLE {
        assert!(s.name.starts_with("impl."), "{}: not an impl.* name", s.name);
        assert!(!s.owner.is_empty() && !s.remove.is_empty(), "{}: owner and removal", s.name);
        assert!(
            s.added.len() == 10 && s.added.starts_with("20"),
            "{}: added {:?} is not a date",
            s.name,
            s.added
        );
    }
}

#[test]
fn rows_parents_and_engine_leaves_are_known_and_nothing_else() {
    assert!(known("impl.render.native"));
    assert!(known("impl.render"), "a parent");
    assert!(known("impl.sim"), "a parent");
    assert!(known("impl.audio"), "a parent of impl.audio.packet-clock");
    let leaf = guest_layout::abi::OVERRIDES[0].name;
    let key = format!("impl.engine.{}.{leaf}", crate::leaves::group(leaf));
    assert!(known(&key), "{key}");
    assert!(!known("impl.engine.tags.no_such_function"));
    assert!(!known("impl.render.nat"), "not a parent at a dot");
    assert!(!known("impl.render.no_such_stage"));
    assert!(!known("impl.ui"));
}

#[test]
fn an_unknown_impl_switch_does_not_parse() {
    let e = crate::Switches::parse("impl.render.native_fgo=off").unwrap_err();
    assert!(e.contains("impl.render.native_fgo"), "{e}");
    assert!(crate::Switches::parse("impl.render.native=off,-impl.sim").is_ok());
    // Stage keys with no reader were deleted from the table: they refuse to parse.
    assert!(crate::Switches::parse("impl.render.native_fog=off").is_err());
    // Feature ids are not in the table: they are registered features.
    assert!(crate::Switches::parse("lockstep,-render-census").is_ok());
}

/// One `impl.*` read found in the sources.
#[derive(Debug)]
struct Read {
    at: String,
    name: String,
    default: Option<bool>,
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "tests" && n != "target") {
                rust_files(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            let name = p.file_name().unwrap().to_string_lossy();
            if name != "tests.rs" && !name.ends_with("_tests.rs") {
                out.push(p);
            }
        }
    }
}

/// The string literal starting at `s` (just past its opening quote).
fn literal(s: &str) -> Option<&str> {
    s.find('"').map(|end| &s[..end])
}

/// `true`/`false` as the next argument after a literal's closing quote.
fn default_after(s: &str) -> Option<bool> {
    let rest = s.strip_prefix(',')?.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn name_ok(name: &str) -> bool {
    !name.ends_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

/// Every `impl.*` name read by non-test code: `"impl.x"` literals (and
/// each switch in a `"a,impl.x=off"` spec), `implementation("x", d)`,
/// `feature("impl.x", d)` and the hand ports' `"sim.native_x"` names.
fn reads() -> Vec<Read> {
    let mut files = Vec::new();
    let Ok(crates) = std::fs::read_dir(crates_dir()) else {
        return Vec::new();
    };
    for c in crates.flatten() {
        rust_files(&c.path().join("src"), &mut files);
    }
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let t = line.trim_start();
            if t.starts_with("#[cfg(test)]") {
                break;
            }
            if t.starts_with("//") {
                continue;
            }
            let at = format!("{}:{}", file.display(), n + 1);
            let mut push = |name: String, default: Option<bool>| {
                if name_ok(&name) {
                    out.push(Read {
                        at: at.clone(),
                        name,
                        default,
                    });
                }
            };
            for (i, _) in line.match_indices("implementation(\"") {
                let s = &line[i + "implementation(\"".len()..];
                if let Some(lit) = literal(s) {
                    push(format!("impl.{lit}"), default_after(&s[lit.len() + 1..]));
                }
            }
            for (i, _) in line.match_indices("\"impl.") {
                let s = &line[i + 1..];
                let Some(lit) = literal(s) else { continue };
                let call = line[..i].ends_with("feature(");
                let default = call.then(|| default_after(&s[lit.len() + 1..])).flatten();
                for item in lit.split(',') {
                    let item = item.trim();
                    let item = item.strip_prefix('-').unwrap_or(item);
                    let name = item.split_once('=').map_or(item, |(n, _)| n);
                    push(name.to_owned(), default);
                }
            }
            for (i, _) in line.match_indices("\"sim.native_") {
                if let Some(lit) = literal(&line[i + 1..]) {
                    push(format!("impl.{lit}"), None);
                }
            }
        }
    }
    out
}

#[test]
fn every_switch_the_code_reads_is_listed_with_its_default() {
    let reads = reads();
    if reads.is_empty() {
        eprintln!("SKIPPED: no sources next to the crate");
        return;
    }
    let mut problems = Vec::new();
    for r in &reads {
        if !known(&r.name) {
            problems.push(format!("{}: {} is not in the switch table", r.at, r.name));
            continue;
        }
        let Some(row) = get(&r.name) else { continue };
        let want = match row.initial {
            Initial::On => Some(true),
            Initial::Off => Some(false),
            Initial::Group => None,
        };
        if let (Some(want), Some(got)) = (want, r.default)
            && want != got
        {
            problems.push(format!(
                "{}: reads {} with default {got}, the table says {want}",
                r.at, r.name
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    // The scan sees the reads it should.
    for name in ["impl.render.native", "impl.sim.native_simd"] {
        assert!(reads.iter().any(|r| r.name == name), "the scan missed {name}");
    }
}

#[test]
fn the_text_lists_every_row() {
    let text = text();
    assert_eq!(text.lines().count(), TABLE.len());
    assert!(text.contains("impl.render.native [on] render"));
}
