#![cfg(feature = "unicorn")]
use oracle::{flags::*, *};

fn execute(bytes: &[u8], state: CpuState) -> Outcome {
    let o = run(bytes, state, &[]);
    assert_eq!(o.stop, Stop::Completed, "{o:#?}");
    o
}

#[test]
fn xlat_fixtures_read_the_seeded_table() {
    let mut inputs = fixtures::Inputs::new(fixtures::DEFAULT_SEED);
    let encodings = fixtures::encodings();
    for name in ["xlat", "xlat-fs"] {
        let encoding = encodings.iter().find(|e| e.name == name).unwrap();
        // The 25 edge inputs and 12 seeded random inputs of the XLAT sweep.
        for n in 0..37 {
            let edge = (n < fixtures::EDGES.len()).then_some(n);
            let (initial, memory) = inputs.state(edge, encoding);
            let address = initial.ebx + u32::from(initial.eax as u8);
            let (base, bytes) = &memory[0];
            let byte = bytes[(address - *base) as usize];
            let outcome = run(&encoding.bytes, initial.clone(), &memory);
            assert_eq!(outcome.stop, Stop::Completed, "{name} case {n}");
            assert_eq!(outcome.reads, [MemoryRead { address, size: 1 }]);
            assert!(outcome.writes.is_empty());
            let mut expected = initial;
            expected.eax = (expected.eax & !0xff) | u32::from(byte);
            expected.eip += encoding.bytes.len() as u32;
            assert_eq!(outcome.state, expected, "{name} case {n}");
            assert_eq!(outcome.defined_flags, EFLAGS_MASK);
        }
    }
}

// Expected constants below are worked by hand from SDM pseudocode, never
// produced by the lifter or a second implementation of its arithmetic helpers.
#[test]
fn add_carry_zero_and_signed_overflow() {
    let o = execute(
        &[0x01, 0xd8],
        CpuState {
            eax: u32::MAX,
            ebx: 1,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 0);
    assert_eq!(o.state.eflags & ARITHMETIC, CF | PF | AF | ZF);
    let o = execute(
        &[0x01, 0xd8],
        CpuState {
            eax: 0x7fff_ffff,
            ebx: 1,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 0x8000_0000);
    assert_eq!(o.state.eflags & ARITHMETIC, PF | AF | SF | OF);
}

#[test]
fn adc_sbb_and_inc_preserve_or_consume_carry() {
    let s = CpuState {
        eax: u32::MAX,
        eflags: 2 | CF,
        ..CpuState::default()
    };
    let o = execute(&[0x11, 0xd8], s);
    assert_eq!(o.state.eax, 0);
    assert_eq!(o.state.eflags & ARITHMETIC, CF | PF | AF | ZF);
    let s = CpuState {
        eflags: 2 | CF,
        ..CpuState::default()
    };
    let o = execute(&[0x19, 0xd8], s.clone());
    assert_eq!(o.state.eax, u32::MAX);
    assert_eq!(o.state.eflags & ARITHMETIC, CF | PF | AF | SF);
    assert_ne!(execute(&[0x40], s).state.eflags & CF, 0);
}

#[test]
fn multiply_halves_and_defined_flags() {
    let o = execute(
        &[0xf7, 0xe3],
        CpuState {
            eax: u32::MAX,
            ebx: 2,
            ..CpuState::default()
        },
    );
    assert_eq!((o.state.edx, o.state.eax), (1, 0xffff_fffe));
    assert_eq!(o.state.eflags & (CF | OF), CF | OF);
    assert_eq!(o.defined_flags & ARITHMETIC, CF | OF);
    let o = execute(
        &[0x0f, 0xaf, 0xc3],
        CpuState {
            eax: 0x8000_0000,
            ebx: u32::MAX,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 0x8000_0000);
    assert_eq!(o.state.eflags & (CF | OF), CF | OF);
}

#[test]
fn division_faults_and_signed_truncation() {
    for (bytes, eax, edx, ebx) in [
        (&[0xf7, 0xf3][..], 1, 0, 0),
        (&[0xf7, 0xf3], 0, 1, 1),
        (&[0xf7, 0xfb], 0x8000_0000, u32::MAX, u32::MAX),
        (&[0xf6, 0xf3], 256, 0, 1),
    ] {
        let state = CpuState {
            eax,
            edx,
            ebx,
            ..CpuState::default()
        };
        let o = run(bytes, state.clone(), &[]);
        assert_eq!(o.stop, Stop::Fault(Fault::DivideError), "{o:#?}");
        assert_eq!(
            (o.state.eip, o.state.eax, o.state.edx),
            (state.eip, eax, edx)
        );
        assert_eq!(o.defined_flags, EFLAGS_MASK);
    }
    let o = execute(
        &[0xf7, 0xfb],
        CpuState {
            eax: 0xffff_fff9,
            edx: u32::MAX,
            ebx: 3,
            ..CpuState::default()
        },
    );
    assert_eq!((o.state.eax, o.state.edx), (0xffff_fffe, u32::MAX)); // -7/3 = -2 remainder -1
    assert_eq!(o.defined_flags & ARITHMETIC, 0);
}

#[test]
fn shifts_mask_counts_and_preserve_count_zero_flags() {
    for count in [0, 32, 64] {
        let s = CpuState {
            eax: 0x8765_4321,
            ecx: count,
            eflags: 2 | ARITHMETIC,
            ..CpuState::default()
        };
        let o = execute(&[0xd3, 0xe0], s.clone());
        assert_eq!(o.state.eax, s.eax);
        assert_eq!(o.state.eflags, s.eflags);
        assert_eq!(o.defined_flags & ARITHMETIC, ARITHMETIC);
    }
    let o = execute(
        &[0xd3, 0xe0],
        CpuState {
            eax: 0x8000_0001,
            ecx: 33,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 2);
    assert_eq!(o.state.eflags & (CF | OF), CF | OF);
    let o = execute(
        &[0xc0, 0xc0, 8],
        CpuState {
            eax: 0x1234_5681,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 0x1234_5681);
    assert_eq!(o.state.eflags & CF, CF);
}

#[test]
fn bsf_bsr_and_bit_test_validity() {
    let o = execute(
        &[0x0f, 0xbc, 0xc3],
        CpuState {
            ebx: 0x8000_0040,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 6);
    assert_eq!(o.state.eflags & ZF, 0);
    let o = execute(
        &[0x0f, 0xbd, 0xc3],
        CpuState {
            ebx: 0x8000_0040,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 31);
    let o = execute(&[0x0f, 0xbc, 0xc3], CpuState::default());
    assert_ne!(o.state.eflags & ZF, 0);
    assert_eq!(o.defined_gprs[0], 0);
    let o = execute(
        &[0x0f, 0xa3, 0xd8],
        CpuState {
            eax: 1,
            ebx: 32,
            eflags: 2 | ZF,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eflags & (CF | ZF), CF | ZF); // register bit offset modulo 32
}

#[test]
fn partial_writes_conditions_and_sequences() {
    let o = execute(
        &[0xb4, 0x5a, 0x66, 0xb8, 0x34, 0x12],
        CpuState {
            eax: 0xabcd_9876,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 0xabcd_1234);
    assert_eq!(o.instructions.len(), 2);
    let o = execute(
        &[0x0f, 0x94, 0xc4],
        CpuState {
            eax: 0x1234_5678,
            eflags: 2 | ZF,
            ..CpuState::default()
        },
    );
    assert_eq!(o.state.eax, 0x1234_0178);
    for (zf, expected) in [(0, 10), (ZF, 20)] {
        let o = execute(
            &[0x0f, 0x44, 0xc3],
            CpuState {
                eax: 10,
                ebx: 20,
                eflags: 2 | zf,
                ..CpuState::default()
            },
        );
        assert_eq!(o.state.eax, expected);
    }
    let o = run(
        &[0xf7, 0xf3, 0x11, 0xd8],
        CpuState {
            eax: 4,
            ebx: 2,
            ..CpuState::default()
        },
        &[],
    );
    assert_eq!(o.stop, Stop::UndefinedInput); // ADC after successful DIV consumes undefined CF
}

#[test]
fn memory_writes_include_unchanged_stores_and_page_faults() {
    let s = CpuState {
        edi: fixtures::DATA,
        ..CpuState::default()
    };
    let o = run(&[0x89, 0x07], s, &[(fixtures::DATA, vec![0; 4])]);
    assert_eq!(o.stop, Stop::Completed);
    assert_eq!(
        o.writes,
        vec![MemoryWrite {
            address: fixtures::DATA,
            bytes: vec![0; 4]
        }]
    );
    for (op, access) in [(0xa1, Access::Read), (0xa3, Access::Write)] {
        let o = run(&[op, 0, 0, 0, 0x70], CpuState::default(), &[]);
        assert_eq!(
            o.stop,
            Stop::Fault(Fault::PageFault {
                address: 0x7000_0000,
                access,
                protection: false
            })
        );
    }
    let o = run(&[0xa3, 0, 0, 0x10, 0], CpuState::default(), &[]);
    assert_eq!(
        o.stop,
        Stop::Fault(Fault::PageFault {
            address: CODE_ADDRESS,
            access: Access::Write,
            protection: true
        })
    );
    assert!(o.writes.is_empty());
}

#[test]
fn rep_copy_zero_count_and_fault_restart() {
    let data = fixtures::DATA;
    for backwards in [false, true] {
        let offset = if backwards { 2 } else { 0 };
        let s = CpuState {
            ecx: 3,
            esi: data + offset,
            edi: data + 128 + offset,
            eflags: 2 | if backwards { DF } else { 0 },
            ..CpuState::default()
        };
        let o = run(&[0xf3, 0xa4], s, &[(data, vec![1, 2, 3])]);
        assert_eq!(o.stop, Stop::Completed, "{o:#?}");
        assert_eq!(o.state.ecx, 0);
        assert_eq!(
            o.writes,
            vec![MemoryWrite {
                address: data + 128,
                bytes: vec![1, 2, 3]
            }]
        );
    }
    let s = CpuState {
        ecx: 0,
        eflags: 2 | ARITHMETIC,
        ..CpuState::default()
    };
    let o = run(&[0xf3, 0xa7], s.clone(), &[]);
    assert_eq!(o.stop, Stop::Completed);
    assert_eq!(o.state.eflags, s.eflags);
    let s = CpuState {
        ecx: 4,
        esi: data + 4094,
        edi: data + 128,
        ..CpuState::default()
    };
    let o = run(&[0xf3, 0xa4], s, &[(data + 4094, vec![8, 9])]);
    assert_eq!(
        o.stop,
        Stop::Fault(Fault::PageFault {
            address: data + 4096,
            access: Access::Read,
            protection: false
        })
    );
    assert_eq!(
        (o.state.ecx, o.state.esi, o.state.edi, o.state.eip),
        (2, data + 4096, data + 130, CODE_ADDRESS)
    );
    assert_eq!(
        o.writes,
        vec![MemoryWrite {
            address: data + 128,
            bytes: vec![8, 9]
        }]
    );
}

#[test]
fn invalid_opcode_bounds_and_rejected_newer_isa() {
    assert_eq!(
        run(&[0x0f, 0x0b], CpuState::default(), &[]).stop,
        Stop::Fault(Fault::InvalidOpcode)
    );
    assert_eq!(
        run(&[0xeb, 0xfe], CpuState::default(), &[]).stop,
        Stop::StepLimit
    );
    assert_eq!(
        run(&[0xeb, 0x7f], CpuState::default(), &[]).stop,
        Stop::EscapedCode
    );
    assert!(matches!(
        run(&[0x66, 0x0f, 0xef, 0xc0], CpuState::default(), &[]).stop,
        Stop::Unsupported(_)
    ));
    assert!(matches!(
        run(
            &[0x90],
            CpuState {
                mxcsr: 0x1fc0,
                ..CpuState::default()
            },
            &[]
        )
        .stop,
        Stop::InvalidInput(_)
    ));
    assert!(matches!(
        run(&[0x0f], CpuState::default(), &[]).stop,
        Stop::InvalidInput(_)
    ));
}

#[test]
fn replay_public_smoke_corpus_comparing_only_defined_flags_and_gprs() {
    let fixtures: Vec<fixtures::Fixture> =
        serde_json::from_str(include_str!("../fixtures/smoke.json")).unwrap();
    for f in fixtures {
        let actual = run(&f.bytes, f.initial, &f.memory);
        let expected = f.outcome;
        assert_eq!(actual.stop, expected.stop, "{}", f.name);
        assert_eq!(actual.defined_flags, expected.defined_flags, "{}", f.name);
        assert_eq!(
            (actual.state.eflags ^ expected.state.eflags) & actual.defined_flags,
            0,
            "{}",
            f.name
        );
        let a = &actual.state;
        let b = &expected.state;
        for ((a, b), mask) in [a.eax, a.ecx, a.edx, a.ebx, a.esp, a.ebp, a.esi, a.edi]
            .into_iter()
            .zip([b.eax, b.ecx, b.edx, b.ebx, b.esp, b.ebp, b.esi, b.edi])
            .zip(actual.defined_gprs)
        {
            assert_eq!((a ^ b) & mask, 0, "{}", f.name);
        }
        assert_eq!(actual.defined_gprs, expected.defined_gprs, "{}", f.name);
        assert_eq!(a.eip, b.eip, "{}", f.name);
        assert_eq!(a.x87, b.x87, "{}", f.name);
        assert_eq!(a.xmm, b.xmm, "{}", f.name);
        assert_eq!(a.mxcsr, b.mxcsr, "{}", f.name);
        assert_eq!(actual.writes, expected.writes, "{}", f.name);
    }
}

#[test]
fn x87_ext80_and_scalar_sse_preserve_bit_patterns() {
    let s = CpuState {
        x87: X87State {
            control: 0x027f,
            ..X87State::default()
        },
        ..CpuState::default()
    };
    let o = execute(&[0xd9, 0xe8, 0xd9, 0xe8, 0xde, 0xc1], s);
    assert_eq!(o.state.x87.st[0], [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x40]); // ext80 2.0
    assert_eq!(o.state.x87.control, 0x027f);
    let mut s = CpuState::default();
    s.xmm[0] = [0xa5; 16];
    s.xmm[0][..4].copy_from_slice(&1f32.to_le_bytes());
    s.xmm[1][..4].copy_from_slice(&2f32.to_le_bytes());
    let o = execute(&[0xf3, 0x0f, 0x58, 0xc1], s.clone());
    assert_eq!(o.state.xmm[0][..4], 3f32.to_le_bytes());
    assert_eq!(o.state.xmm[0][4..], s.xmm[0][4..]);
    assert_eq!(o.state.mxcsr, 0x1f80);
}

#[test]
fn ext80_prestate_roundtrip_retains_low_significand_bits_and_top() {
    let mut s = CpuState::default();
    s.x87.st[0] = [1, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]; // 1 + 2^-63
    s.x87.status = 3 << 11;
    s.x87.tag = !(3 << (2 * 3)); // physical R3 valid
    s.x87.instruction_pointer = 0x1234;
    s.x87.data_pointer = 0x5678;
    let o = execute(&[0x90], s.clone());
    assert_eq!(o.state.x87, s.x87);
}

#[test]
fn mapping_inputs_are_isolated_and_validated() {
    let s = CpuState {
        edi: fixtures::DATA,
        eax: 0x1234_5678,
        ..CpuState::default()
    };
    let memory = vec![(fixtures::DATA, vec![0; 4])];
    let written = run(&[0x89, 0x07], s.clone(), &memory);
    assert_eq!(written.stop, Stop::Completed);
    let read = run(&[0x8b, 0x07], s.clone(), &memory);
    assert_eq!(read.state.eax, 0); // fresh engine, no leaked previous store
    for mem in [
        vec![(s.eip, vec![0])],
        vec![(fixtures::DATA, vec![0; 2]), (fixtures::DATA + 1, vec![0])],
        vec![(u32::MAX, vec![0; 2])],
    ] {
        assert!(matches!(
            run(&[0x90], s.clone(), &mem).stop,
            Stop::InvalidInput(_)
        ));
    }
    let o = run(&[0x0f, 0x0b], s.clone(), &[]);
    assert_eq!(o.state.eip, s.eip);
}
