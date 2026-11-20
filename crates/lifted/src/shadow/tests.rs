//! Shadow checks over synthetic code: a few assembled instructions as the
//! original, Rust closures as the ports, a small Guest as live memory.

use super::*;
use crate::code::Assembled;
use runtime::guest::Permissions;

const CODE: u32 = 0x1000;
const OUT: u32 = 0x2000;
const STACK: u32 = 0x8000;
const RETURN_TO: u32 = 0x5555;

/// `eax = [esp+4] + 1; [OUT] = eax; ret` with no flags written:
/// `mov eax,[esp+4]` `lea eax,[eax+1]` `mov [OUT],eax` `ret`.
fn increment() -> Assembled {
    Assembled::new(
        CODE,
        &[
            0x8b, 0x44, 0x24, 0x04, 0x8d, 0x40, 0x01, 0xa3, 0x00, 0x20, 0x00, 0x00, 0xc3,
        ],
    )
}

fn live() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0, 0x10000, Permissions::RW).unwrap();
    g.write_bytes(STACK, &RETURN_TO.to_le_bytes(), 1).unwrap();
    g.write_bytes(STACK + 4, &41u32.to_le_bytes(), 1).unwrap();
    g
}

fn entry() -> CpuAbi {
    CpuAbi {
        esp: STACK,
        eip: CODE,
        ..CpuAbi::default()
    }
}

fn word(g: &Guest, a: u32) -> u32 {
    let mut b = [0; 4];
    g.read_bytes(a, &mut b, 1).unwrap();
    u32::from_le_bytes(b)
}

/// The port: `[OUT] = arg + stored`, `eax = arg + 1`, return popping
/// `cleanup` argument bytes.
fn port(
    stored: u32,
    eax: u32,
    cleanup: u32,
) -> impl FnMut(&mut CpuAbi, &mut Guest) -> Result<TransferAbi, GuestFault> {
    move |c: &mut CpuAbi, g: &mut Guest| {
        let arg = word(g, c.esp + 4);
        g.write_bytes(OUT, &(arg + stored).to_le_bytes(), 1)?;
        c.eax = arg + eax;
        g.ret(c, cleanup)
    }
}

fn run_check(p: &mut PortFn<'_>) -> (Outcome, Guest) {
    let g = live();
    let (outcome, _) = check(&mut increment(), &mut View::new(), &g, &entry(), 100, p);
    (outcome, g)
}

fn diff(o: Outcome) -> Diff {
    match o {
        Outcome::Mismatched(d) => *d,
        other => panic!("expected a mismatch, got {other:?}"),
    }
}

#[test]
fn a_faithful_port_matches_and_live_memory_is_untouched() {
    let (outcome, g) = run_check(&mut port(1, 1, 0));
    assert_eq!(outcome, Outcome::Matched { steps: 4 });
    assert_eq!(word(&g, OUT), 0, "neither side writes the live Guest");
}

#[test]
fn a_different_store_is_a_memory_mismatch() {
    let d = diff(run_check(&mut port(2, 1, 0)).0);
    assert_eq!(d.kind, "memory");
    assert_eq!(d.memory_bytes, 1);
    assert_eq!(d.memory[0].before, "00");
    assert_eq!(d.memory[0].port, "2b");
    assert_eq!(d.memory[0].original, "2a");
    assert!(
        d.memory[0]
            .virtual_addresses
            .contains(&format!("{OUT:#010x}"))
    );
}

#[test]
fn a_different_register_is_a_register_mismatch() {
    let d = diff(run_check(&mut port(1, 7, 0)).0);
    assert_eq!(d.kind, "registers");
    assert_eq!(d.registers.len(), 1);
    assert_eq!(d.registers[0].name, "eax");
}

#[test]
fn a_wrong_stack_cleanup_never_meets_the_original() {
    let d = diff(run_check(&mut port(1, 1, 4)).0);
    assert_eq!(d.kind, "returned");
    assert!(d.summary.contains("left the frame"), "{}", d.summary);
}

#[test]
fn the_original_reads_its_own_writes() {
    // mov dword [OUT], 5; mov eax, [OUT]; ret
    let mut code = Assembled::new(
        CODE,
        &[
            0xc7, 0x05, 0x00, 0x20, 0x00, 0x00, 5, 0, 0, 0, 0xa1, 0x00, 0x20, 0x00, 0x00, 0xc3,
        ],
    );
    let g = live();
    let mut p = |c: &mut CpuAbi, g: &mut Guest| {
        g.write_bytes(OUT, &5u32.to_le_bytes(), 1)?;
        c.eax = 5;
        g.ret(c, 0)
    };
    let (outcome, _) = check(&mut code, &mut View::new(), &g, &entry(), 100, &mut p);
    assert_eq!(outcome, Outcome::Matched { steps: 3 });
}

#[test]
fn a_port_fault_on_the_copy_skips() {
    let mut p = |_: &mut CpuAbi, g: &mut Guest| {
        g.ret(
            &mut CpuAbi {
                esp: 0xfff0_0000,
                ..CpuAbi::default()
            },
            0,
        )
    };
    let (outcome, _) = run_check(&mut p);
    assert!(
        matches!(outcome, Outcome::Skipped(ref why) if why.contains("faulted")),
        "{outcome:?}"
    );
}

fn registry() -> Shadow {
    let s = Shadow::new();
    s.register("test.inc", "test", "INCREMENT", CODE, true);
    s.register("test.off", "test", "OFF", 0x3000, false);
    s.set_code(Box::new(increment()));
    s
}

fn tick(_: &Guest) -> Option<u64> {
    Some(7)
}

fn call(s: &Shadow, p: &mut PortFn<'_>) -> (CpuAbi, Guest) {
    let mut g = live();
    let mut c = entry();
    s.run("test.inc", &mut c, &mut g, CODE, tick, p).unwrap();
    (c, g)
}

#[test]
fn enable_selects_by_group_and_reports_what_is_not_installed() {
    let s = registry();
    let e = s.enable(&["test".into()], None, None).unwrap();
    assert_eq!(e.enabled, ["test.inc"]);
    assert_eq!(e.not_installed, ["test.off"]);
    assert_eq!(
        s.enable(&["nope".into()], None, None).unwrap_err(),
        Error::Unknown(vec!["nope".into()])
    );
    assert!(s.enable(&[], None, None).is_err());
    assert!(s.status().on);
    assert_eq!(s.disable(&[]).unwrap(), ["test.inc"]);
    assert!(!s.status().on);
}

#[test]
fn enable_without_an_image_is_unavailable() {
    let s = Shadow::new();
    s.register("test.inc", "test", "INCREMENT", CODE, true);
    assert!(matches!(
        s.enable(&["all".into()], None, None),
        Err(Error::Unavailable(_))
    ));
    s.set_source(Box::new(|| Ok(Box::new(increment()) as Box<dyn Code>)));
    assert!(s.enable(&["all".into()], None, None).is_ok());
    assert!(s.status().ready);
}

#[test]
fn checks_count_and_the_live_call_still_runs() {
    let s = registry();
    // Off: the port runs, nothing is counted.
    let (c, g) = call(&s, &mut port(1, 1, 0));
    assert_eq!((c.eax, word(&g, OUT)), (42, 42));
    assert_eq!(s.status().calls, 0);

    s.enable(&["test.inc".into()], None, None).unwrap();
    let (c, g) = call(&s, &mut port(1, 1, 0));
    assert_eq!((c.eax, c.esp, word(&g, OUT)), (42, STACK + 4, 42));
    let st = s.status();
    assert_eq!(
        (st.calls, st.checked, st.matched, st.mismatched),
        (1, 1, 1, 0)
    );

    call(&s, &mut port(2, 1, 0));
    let st = s.status();
    assert_eq!((st.checked, st.mismatched), (2, 1));
    let m = &st.mismatches[0];
    assert_eq!(
        (m.port, m.tick, m.call, m.diff.kind),
        ("test.inc", Some(7), 2, "memory")
    );
    assert_eq!(m.function, "0x00001000");
    assert_eq!(m.entry["esp"], format!("{STACK:#010x}"));
}

#[test]
fn every_samples_calls() {
    let s = registry();
    s.enable(&["all".into()], Some(3), None).unwrap();
    for _ in 0..7 {
        call(&s, &mut port(1, 1, 0));
    }
    let st = s.status();
    assert_eq!((st.calls, st.checked), (7, 3));
}

#[test]
fn a_port_that_answers_differently_live_is_nondeterministic() {
    let s = registry();
    s.enable(&["all".into()], None, None).unwrap();
    let mut n = 0;
    let mut p = |c: &mut CpuAbi, g: &mut Guest| {
        n += 1;
        port(1, n, 0)(c, g)
    };
    call(&s, &mut p);
    let st = s.status();
    assert_eq!(st.mismatched, 1);
    assert_eq!(st.mismatches[0].diff.kind, "nondeterministic");
}

#[test]
fn status_serializes() {
    let s = registry();
    s.enable(&["all".into()], None, None).unwrap();
    call(&s, &mut port(2, 1, 0));
    let v = serde_json::to_value(s.status()).unwrap();
    assert_eq!(v["mismatched"], 1);
    assert_eq!(v["mismatches"][0]["kind"], "memory");
    assert_eq!(v["ports"][0]["name"], "test.inc");
}
