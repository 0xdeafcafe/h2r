//! The raw-offset ratchet: guest field offsets belong in this crate, so
//! the crates that read guest structures may not add more hand-written
//! `+ 0x..` or `wrapping_add(0x..)` than they have today
//! (docs/engine/guest-layout.md, review section 3.4).
//!
//! The count is textual (every such addition in a crate's `src`, comment
//! lines aside), so it also counts arithmetic that is not a guest field;
//! that is why each crate has a budget, not zero. When a crate moves
//! readers onto the layouts, lower its budget to the new count in the
//! same commit. Do not raise one: name the field in a layout instead.

use std::path::Path;

/// (crate, most additions allowed).
const BUDGETS: &[(&str, usize)] = &[
    ("agent", 21),
    ("live-telemetry", 101),
    ("bots", 355),
    ("native-render", 273),
    ("sim", 2),
];

fn count_line(line: &str) -> usize {
    if line.trim_start().starts_with("//") {
        return 0;
    }
    let plus = line
        .match_indices("+ 0x")
        .filter(|(i, _)| {
            line[i + 4..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_hexdigit())
        })
        .count();
    plus + line.matches("wrapping_add(0x").count()
}

fn count_dir(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += count_dir(&p);
        } else if p.extension().is_some_and(|x| x == "rs")
            && let Ok(text) = std::fs::read_to_string(&p)
        {
            n += text.lines().map(count_line).sum::<usize>();
        }
    }
    n
}

#[test]
fn the_count_sees_offset_additions_and_skips_comments() {
    assert_eq!(count_line("let p = m.u32(unit + 0x13c)?;"), 1);
    assert_eq!(count_line("a.wrapping_add(0x10) + 0x4 + 0xg"), 2);
    assert_eq!(count_line("    // body + 0x64 is the position"), 0);
    assert_eq!(count_line("let p = Unit::PLAYER.at(unit);"), 0);
}

#[test]
fn readers_add_no_new_raw_offsets() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut over = Vec::new();
    for &(name, budget) in BUDGETS {
        let src = crates.join(name).join("src");
        if !src.is_dir() {
            eprintln!("SKIPPED {name}: no sources at {}", src.display());
            continue;
        }
        let n = count_dir(&src);
        if n > budget {
            over.push(format!(
                "{name}: {n} raw offset additions, budget {budget}: name the field in guest-layout"
            ));
        } else if n < budget {
            eprintln!("{name}: {n} raw offset additions; lower its budget from {budget}");
        }
    }
    assert!(over.is_empty(), "\n{}", over.join("\n"));
}
