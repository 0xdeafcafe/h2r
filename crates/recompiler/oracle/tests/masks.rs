use iced_x86::{Decoder, DecoderOptions};
use oracle::flags::*;

fn rule(bytes: &[u8], cl: u8) -> FlagEffect {
    let i = Decoder::new(32, bytes, DecoderOptions::NONE).decode();
    assert!(!i.is_invalid());
    effect(&i, cl)
}

// Intel SDM 325383 Vol. 2: the Flags Affected sections for these mnemonics.
#[test]
fn integer_undefined_flags_from_sdm() {
    for (bytes, expected) in [
        (&[0x01, 0xd8][..], 0),
        (&[0x11, 0xd8], 0),
        (&[0x19, 0xd8], 0),
        (&[0x21, 0xd8], AF),
        (&[0xf7, 0xe3], PF | AF | ZF | SF),
        (&[0xf7, 0xeb], PF | AF | ZF | SF),
        (&[0xf7, 0xf3], ARITHMETIC),
        (&[0xf7, 0xfb], ARITHMETIC),
        (&[0x0f, 0xa3, 0xd8], OF | SF | AF | PF),
        (&[0x0f, 0xbc, 0xc3], CF | OF | SF | AF | PF),
    ] {
        assert_eq!(rule(bytes, 0).undefined, expected, "{bytes:02x?}");
    }
}

#[test]
fn shift_count_masks_and_width_boundaries() {
    assert_eq!(
        rule(&[0xc1, 0xe0, 0], 0),
        FlagEffect {
            modified: 0,
            undefined: 0
        }
    );
    assert_eq!(rule(&[0xc1, 0xe0, 1], 0).undefined, AF);
    assert_eq!(rule(&[0xc1, 0xe0, 31], 0).undefined, AF | OF);
    assert_eq!(rule(&[0xc1, 0xe0, 32], 0).modified, 0);
    assert_eq!(rule(&[0xc1, 0xe0, 33], 0).undefined, AF);
    assert_eq!(rule(&[0xd3, 0xe0], 0).modified, 0);
    assert_eq!(rule(&[0xd3, 0xe0], 1).undefined, AF);
    assert_eq!(rule(&[0xd3, 0xe0], 255).undefined, AF | OF);
    assert_eq!(rule(&[0xc0, 0xe0, 8], 0).undefined, AF | OF | CF);
    assert_eq!(rule(&[0xc0, 0xf8, 8], 0).undefined, AF | OF); // SAR CF remains defined
    assert_eq!(rule(&[0x66, 0x0f, 0xa4, 0xd8, 17], 0).undefined, ARITHMETIC);
}

#[test]
fn rotates_and_preserved_flags() {
    assert_eq!(rule(&[0xc0, 0xc0, 8], 0).modified, CF | OF); // ROL AL,8 writes CF
    assert_eq!(rule(&[0xc0, 0xc0, 8], 0).undefined, OF);
    assert_eq!(rule(&[0xc1, 0xd0, 32], 0).modified, 0);
    assert_eq!(rule(&[0xc0, 0xd0, 9], 0).modified, 0); // RCL AL,9 has effective count zero
    assert_eq!(rule(&[0xc0, 0xd0, 10], 0).undefined, OF); // effective 1, masked 10
    assert_eq!(rule(&[0xc1, 0xc8, 1], 0).undefined, 0);
    assert_eq!(rule(&[0x40], 0).modified, ARITHMETIC & !CF); // INC EAX
    assert_eq!(rule(&[0x0f, 0xa3, 0xd8], 0).modified & ZF, 0); // BT preserves ZF
}

#[test]
fn smoke_fixtures_have_independently_checked_edge_results() {
    use oracle::{Fault, Stop, fixtures::Fixture};
    let list: Vec<Fixture> = serde_json::from_str(include_str!("../fixtures/smoke.json")).unwrap();
    assert_eq!(list.len(), 19);
    let named = |name: &str| list.iter().find(|f| f.name == name).unwrap();
    let add = named("add-32");
    assert_eq!((add.initial.eax, add.initial.ebx), (0, 0));
    assert_eq!(add.outcome.state.eax, 0);
    assert_eq!(add.outcome.state.eflags & ARITHMETIC, PF | ZF);
    assert_eq!(
        named("idiv-32").outcome.stop,
        Stop::Fault(Fault::DivideError)
    );
    assert_eq!(named("bsf-32").outcome.state.eflags & ZF, ZF);
    assert_eq!(named("bsf-32").outcome.defined_gprs[0], 0);
    assert_eq!(named("mov-ah").outcome.state.eax, 0x5a00);
    assert_eq!(named("shl-32-imm-32").outcome.state.eax, u32::MAX);
}

#[test]
fn input_generator_is_reproducible_and_contains_requested_families() {
    use oracle::fixtures::*;
    let list = encodings();
    let mut names = std::collections::BTreeSet::new();
    for e in &list {
        assert!(names.insert(&e.name), "duplicate {}", e.name);
        let mut decoder = Decoder::new(32, &e.bytes, DecoderOptions::NONE);
        while decoder.can_decode() {
            assert!(!decoder.decode().is_invalid(), "{}", e.name);
        }
    }
    let mut a = Inputs::new(DEFAULT_SEED);
    let mut b = Inputs::new(DEFAULT_SEED);
    for e in list {
        assert_eq!(a.state(Some(0), &e), b.state(Some(0), &e));
        assert_eq!(a.state(None, &e), b.state(None, &e));
    }
}

#[cfg(not(feature = "unicorn"))]
#[test]
fn execution_without_feature_is_explicitly_unavailable() {
    let result = oracle::run(&[0x90], oracle::CpuState::default(), &[]);
    assert_eq!(result.stop, oracle::Stop::BackendUnavailable);
    assert!(result.instructions.is_empty());
}
