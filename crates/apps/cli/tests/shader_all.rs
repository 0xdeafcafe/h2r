//! `h2 shader --all <map>` exit status on synthetic maps. Every byte is
//! invented (a two-tag map holding one `stem`); no game data.
use std::path::PathBuf;
use std::process::Command;

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A minimal map whose only shader tag is a `stem` at file `0x1300`
/// (virtual `0x9000_0100`) with the postprocess descriptor `(count, address)`.
fn map(count: u32, address: u32) -> Vec<u8> {
    let mut b = vec![0u8; 0x2000];
    let len = b.len() as u32;
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    for (o, v) in [
        (4, 8),
        (8, len),
        (16, 0x1000),
        (20, 0x200),
        (24, 0x600),
        (28, 0x800),
    ] {
        put(&mut b, o, v);
    }
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    b[0x198..0x19c].copy_from_slice(b"test");
    let names = b"globals\0synthetic\0";
    b[0x900..0x900 + names.len()].copy_from_slice(names);
    for (o, v) in [
        (0x2c0, 2),
        (0x2c4, 0x900),
        (0x2c8, names.len() as u32),
        (0x2cc, 0x880),
        (0x880, 0),
        (0x884, 8),
    ] {
        put(&mut b, o, v);
    }
    for (i, g) in [*b"matg", *b"stem"].into_iter().enumerate() {
        let g = u32::from_be_bytes(g);
        let at = 0x1020 + i * 12;
        put(&mut b, at, g);
        put(&mut b, at + 4, u32::MAX);
        put(&mut b, at + 8, u32::MAX);
        let t = 0x1038 + i * 16;
        put(&mut b, t, g);
        put(&mut b, t + 4, 0xa000_0000 | i as u32);
        put(&mut b, t + 8, 0x9000_0000 + i as u32 * 0x100);
        put(&mut b, t + 12, if i == 0 { 16 } else { 0x60 });
    }
    for (o, v) in [
        (0, 0x8000_0020),
        (4, 2),
        (8, 0x8000_0038),
        (12, u32::MAX),
        (16, 0xa000_0000),
        (24, 2),
        (28, u32::from_be_bytes(*b"tags")),
    ] {
        put(&mut b, 0x1000 + o, v);
    }
    // Null `shad` references at stem +0x40/+0x4c.
    put(&mut b, 0x1300 + 0x44, u32::MAX);
    put(&mut b, 0x1300 + 0x50, u32::MAX);
    put(&mut b, 0x1300 + 0x58, count);
    put(&mut b, 0x1300 + 0x5c, address);
    b
}

fn run_all(name: &str, bytes: &[u8]) -> std::process::Output {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = dir.join(format!("shader-all-{name}-{}.map", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_h2"))
        .args(["shader", "--all"])
        .arg(&path)
        .env_remove("H2_DISC")
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    out
}

fn assert_fails(name: &str, bytes: &[u8], cause: &str) {
    let out = run_all(name, bytes);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{name}: {text}");
    assert!(text.contains("1 failures"), "{name}: {text}");
    assert!(text.contains(cause), "{name}: {text}");
    assert!(!text.contains("panicked"), "{name}: {text}");
}

#[test]
fn valid_map_exits_zero() {
    let out = run_all("valid", &map(1, 0x9000_0200));
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("stem 1 tags"), "{text}");
    assert!(text.contains("0 failures"), "{text}");
}

#[test]
fn over_budget_block_count_exits_one() {
    assert_fails(
        "count",
        &map(1025, 0x9000_0200),
        "block count exceeds the element budget",
    );
}

#[test]
fn wrapping_descriptor_pointer_exits_one_without_panicking() {
    // The alias counter adds block offsets to this pointer; it must not
    // overflow, and the decoder reports the descriptor.
    assert_fails("pointer", &map(1, 0xffff_fff0), "stem synthetic");
}
