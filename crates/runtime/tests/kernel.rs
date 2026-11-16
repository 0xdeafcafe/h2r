use runtime::{Event, Runner, abi, kernel, load_xbe};
mod common;

#[test]
fn metadata_covers_the_documented_import_inventory() {
    let document = include_str!("../../../docs/runtime/kernel-imports.md");
    let function_table = document
        .split("## Imported functions (135)")
        .nth(1)
        .unwrap()
        .split("## Imported data (17)")
        .next()
        .unwrap();
    let data_table = document
        .split("## Imported data (17)")
        .nth(1)
        .unwrap()
        .split("## Hard work and why")
        .next()
        .unwrap();
    let mut counts = [0, 0];
    for (table, index) in [(function_table, 0), (data_table, 1)] {
        for row in table.lines() {
            let columns: Vec<_> = row.split('|').map(str::trim).collect();
            let Some(ordinal) = columns.get(1).and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let name = columns[2]
                .strip_prefix('[')
                .unwrap()
                .split(']')
                .next()
                .unwrap();
            if index == 0 {
                let import = kernel::function(ordinal).unwrap();
                assert_eq!(import.name, name);
                let signature = columns[3].trim_matches('`');
                let arguments = signature
                    .split_once('(')
                    .unwrap()
                    .1
                    .rsplit_once(')')
                    .unwrap()
                    .0;
                let words: usize = arguments
                    .split(',')
                    .map(str::trim)
                    .filter(|a| !matches!(*a, "void" | "..." | ""))
                    .map(|a| {
                        if matches!(
                            a.split_whitespace().next().unwrap(),
                            "LARGE_INTEGER" | "ULONGLONG" | "LONGLONG"
                        ) {
                            2
                        } else {
                            1
                        }
                    })
                    .sum();
                let words = if signature.contains("FASTCALL") {
                    words.saturating_sub(2)
                } else {
                    words
                };
                assert_eq!(import.stack_words, words, "stack words for {name}");
                assert_eq!(import.variadic, signature.contains("..."));
            } else {
                assert_eq!(kernel::data(ordinal).unwrap().name, name);
            }
            counts[index] += 1;
        }
    }
    assert_eq!(counts, [135, 17]);
    assert_eq!(kernel::DATA_IMPORTS.len(), 17);
    for (ordinal, words) in [
        (255, 10),
        (86, 0),
        (87, 0),
        (250, 0),
        (149, 4),
        (150, 5),
        (8, 1),
    ] {
        assert_eq!(kernel::function(ordinal).unwrap().stack_words, words);
    }
    assert!(kernel::function(8).unwrap().variadic);
}

#[test]
fn every_function_import_has_a_stop_and_data_has_shared_storage() {
    for import in kernel::FUNCTION_IMPORTS {
        let mut bytes = common::synthetic_xbe();
        bytes[0x1100..0x1104].copy_from_slice(&(0x80000000 | import.ordinal).to_le_bytes());
        let image = load_xbe(&bytes).unwrap();
        let mut cpu = image.cpu;
        // Reserve enough ordinary guest argument words for every signature.
        cpu.esp -= 64;
        let mut ctx = image.context();
        let t = Runner::default().run(&mut cpu, &mut ctx, kernel::function_token(import.ordinal));
        assert_eq!(t.kind, abi::STOP);
        let Some(Event::KernelCall(call)) = ctx.event(t.event_id) else {
            panic!("missing import stop");
        };
        assert_eq!((call.ordinal, call.name), (import.ordinal, import.name));
        assert_eq!(call.stack_args.len(), import.stack_words);
    }
    let image = load_xbe(&common::synthetic_xbe()).unwrap();
    let mut ctx = image.context();
    ctx.memory().set_u32(image.data_exports[&156], 123).unwrap();
    assert_eq!(
        image
            .runtime
            .memory()
            .u32(image.data_exports[&156])
            .unwrap(),
        123
    );
}
