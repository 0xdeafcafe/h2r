//! Modules (`use "file.rules"`): files read as the one text they make,
//! findings with their file, cycles, and the editor's diagnostics over
//! files. Invented scripts only. See `docs/game/rules-engine.md`,
//! "Modules".

use rules::ide::{self, severity};
use rules::lang::modules;
use rules::{compile, compile_files, diagnose, diagnose_files};

const WAVES: &str = "const base = 2\nfn bonus() -> int = base + 1\n";
const MAIN: &str = "rules \"Split\" 1\ntype slayer\nuse \"waves.rules\"\non kill(v, k) { score(k, bonus()) }\n";
/// `MAIN` with `WAVES` where the `use` line is.
const ONE: &str =
    "rules \"Split\" 1\ntype slayer\nconst base = 2\nfn bonus() -> int = base + 1\non kill(v, k) { score(k, bonus()) }\n";

fn files(list: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + use<> {
    let list = list.to_vec();
    move |n| list.iter().find(|(a, _)| *a == n).map(|(_, b)| b.to_string())
}

#[test]
fn two_files_compile_to_the_blob_of_the_same_text_in_one() {
    let read = files(&[("waves.rules", WAVES)]);
    let split = compile_files("main.rules", MAIN, &read).unwrap();
    let one = compile(ONE).unwrap();
    assert_eq!(split.encode(), one.encode());
    assert_eq!(split.id(), one.id());
}

#[test]
fn a_script_with_no_use_is_the_same_with_or_without_files() {
    let none = |_: &str| None;
    assert_eq!(compile_files("m.rules", ONE, &none).unwrap().id(), compile(ONE).unwrap().id());
    assert!(diagnose_files("m.rules", ONE, &none).errors().next().is_none());
}

#[test]
fn a_file_named_by_two_files_is_read_once() {
    let read = files(&[
        ("a.rules", "use \"waves.rules\"\n"),
        ("b.rules", "use \"waves.rules\"\n"),
        ("waves.rules", WAVES),
    ]);
    let src = "rules \"Split\" 1\ntype slayer\nuse \"a.rules\"\nuse \"b.rules\"\non kill(v, k) { score(k, bonus()) }\n";
    assert_eq!(compile_files("m.rules", src, &read).unwrap().id(), compile(ONE).unwrap().id());
}

#[test]
fn a_finding_in_a_module_has_its_file_and_its_line() {
    let bad = "const base = 2\nfn bonus() -> int = nope + 1\n";
    let read = files(&[("waves.rules", bad)]);
    let c = diagnose_files("main.rules", MAIN, &read);
    assert!(c.ir.is_none());
    let d = c.errors().find(|d| d.file.is_some()).expect("an error in the module");
    assert_eq!(d.file.as_deref(), Some("waves.rules"));
    assert_eq!(d.pos.line, 2);
    // A finding in the root has no file, and its own line.
    let root_bad = MAIN.replace("score(k, bonus())", "score(k, nope)");
    let read = files(&[("waves.rules", WAVES)]);
    let c = diagnose_files("main.rules", &root_bad, &read);
    let d = c.errors().next().unwrap();
    assert_eq!((d.file.as_deref(), d.pos.line), (None, 4));
    // The one error a caller of `compile_files` sees names the file.
    let read = files(&[("waves.rules", bad)]);
    let e = compile_files("main.rules", MAIN, &read).unwrap_err();
    assert!(e.message.starts_with("in waves.rules: "), "{e}");
    assert_eq!(e.line, 2);
}

#[test]
fn a_cycle_is_an_error_naming_both_files() {
    let read = files(&[("a.rules", "use \"b.rules\"\n"), ("b.rules", "use \"a.rules\"\n")]);
    let src = "rules \"Split\" 1\ntype slayer\nuse \"a.rules\"\n";
    let c = diagnose_files("main.rules", src, &read);
    assert!(c.ir.is_none());
    let d = c.errors().next().unwrap();
    assert_eq!(d.code, "use");
    assert_eq!(d.file.as_deref(), Some("b.rules"));
    assert!(d.message.contains("a.rules") && d.message.contains("b.rules"), "{}", d.message);
    // A file that names the root is a cycle too.
    let read = files(&[("a.rules", "use \"main.rules\"\n")]);
    let d = diagnose_files("main.rules", src, &read).errors().next().unwrap().clone();
    assert!(d.message.contains("main.rules") && d.message.contains("a.rules"), "{}", d.message);
}

#[test]
fn a_missing_file_and_a_use_with_no_files_are_errors_at_the_use() {
    let c = diagnose_files("main.rules", MAIN, &|_: &str| None);
    let d = c.errors().next().unwrap();
    assert_eq!((d.code, d.pos.line, d.file.as_deref()), ("use", 3, None));
    assert!(d.message.contains("no file named `waves.rules`"));
    let d = diagnose(MAIN).errors().next().unwrap().clone();
    assert_eq!((d.code, d.pos.line), ("use", 3));
}

#[test]
fn the_editor_puts_a_modules_problem_on_the_use_line() {
    let bad = "const base = 2\nfn bonus() -> int = nope + 1\n";
    let read = files(&[("waves.rules", bad)]);
    let list = ide::diagnostics_with_files(MAIN, "main.rules", &read);
    let d = list.iter().find(|d| d.severity == severity::ERROR).expect("an error");
    assert_eq!(d.range.start.line, 2, "the `use` line, 0-based");
    assert!(d.message.starts_with("waves.rules:2:"), "{}", d.message);
    assert!(d.fixes.is_empty());
    // With the module sound, the root has no error and a problem in it
    // keeps its own line.
    let read = files(&[("waves.rules", WAVES)]);
    let list = ide::diagnostics_with_files(MAIN, "main.rules", &read);
    assert!(list.iter().all(|d| d.severity != severity::ERROR), "{list:?}");
    let root_bad = MAIN.replace("score(k, bonus())", "score(k, nope)");
    let list = ide::diagnostics_with_files(&root_bad, "main.rules", &read);
    let d = list.iter().find(|d| d.severity == severity::ERROR).unwrap();
    assert_eq!(d.range.start.line, 3);
    // A missing file and a cycle are on the `use` line too.
    let list = ide::diagnostics_with_files(MAIN, "main.rules", &|_: &str| None);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].range.start.line, 2);
    // With no `use` it is `diagnostics`.
    assert_eq!(ide::diagnostics_with_files(ONE, "m.rules", &|_: &str| None), ide::diagnostics(ONE));
}

#[test]
fn reading_the_files_of_256_kib_is_cheap() {
    // 16 files of 16 KiB: a comment line is one `use`-free line.
    let line = "// a line of a wave table, as long as a real one is\n";
    let body = line.repeat(16 * 1024 / line.len());
    let names: Vec<String> = (0..15).map(|i| format!("f{i}.rules")).collect();
    let root: String = names.iter().map(|n| format!("use \"{n}\"\n")).collect::<String>() + &body;
    let read = |n: &str| names.iter().any(|x| x == n).then(|| body.clone());
    let t = std::time::Instant::now();
    let x = modules::expand("main.rules", &root, &read).unwrap();
    let took = t.elapsed();
    assert_eq!(x.files.len(), 16);
    assert!(x.src.len() <= rules::lang::MAX_SOURCE_BYTES);
    // The bound of the language's own work on 256 KiB is `h2 rules
    // check`'s, measured in release by the coordinator; reading the files
    // is a small part of it.
    #[cfg(not(debug_assertions))]
    assert!(took.as_millis() < 20, "{took:?}");
    let _ = took;
}
