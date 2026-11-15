#![cfg(target_os = "macos")]
use sse_oracle::{Oracle, State, build_runner};

// These test the transport, state isolation, and observable machine behavior.
#[test]
fn hardware_protocol_and_instruction_boundaries() -> anyhow::Result<()> {
    // Rosetta is opt-in: set SSE_ORACLE_ROSETTA=1. Skips cleanly if unavailable.
    if std::env::var_os("SSE_ORACLE_ROSETTA").is_none() || !sse_oracle::rosetta_available() {
        eprintln!("skipped: set SSE_ORACLE_ROSETTA=1 with Rosetta 2 and clang installed");
        return Ok(());
    }
    let root = std::path::PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR"))
        .join(format!("sse-oracle-test-{}", std::process::id()));
    let binary = build_runner(&root)?;
    let mut oracle = Oracle::spawn(&binary)?;
    let mut state = State {
        eax: 0xdeadbeef,
        ecx: 0xcafebabe,
        ..State::default()
    };
    for (i, r) in state.xmm.iter_mut().enumerate() {
        *r = [0x3f800000 + i as u32, 11, 22, 33];
    }
    for (i, r) in state.mm.iter_mut().enumerate() {
        *r = [i as u32, 0xdead0000 + i as u32];
    }
    state.memory = [0x40000000, 44, 55, 66];
    let out = oracle
        .query(&[0xf3, 0x0f, 0x10, 0x07], &state)?
        .state
        .unwrap(); // movss xmm0,[rdi]
    assert_eq!(out.xmm[0], [0x40000000, 0, 0, 0]);
    assert_eq!(&out.xmm[1..], &state.xmm[1..]);
    assert_eq!(out.mm, state.mm);
    assert_eq!((out.eax, out.ecx), (state.eax, state.ecx));
    let out = oracle
        .query(&[0xf3, 0x0f, 0x10, 0xc1], &state)?
        .state
        .unwrap();
    assert_eq!(out.xmm[0], [0x3f800001, 11, 22, 33]);
    let out = oracle.query(&[0x0f, 0x11, 0x07], &state)?.state.unwrap(); // movups [rdi],xmm0
    assert_eq!(out.memory, state.xmm[0]);
    for bytes in [&[0x0f, 0x2b, 0x07][..], &[0x0f, 0xe7, 0x07][..]] {
        let out = oracle.query(bytes, &state)?.state.unwrap();
        if bytes[1] == 0x2b {
            assert_eq!(out.memory, state.xmm[0]);
        } else {
            assert_eq!(&out.memory[..2], &state.mm[0]);
        }
    }
    // Back-to-back instruction changes exercise Rosetta's translated-code cache.
    state.xmm[0] = [0x40000000; 4];
    state.xmm[1] = [0x3f800000; 4];
    for _ in 0..8 {
        let add = oracle
            .query(&[0xf3, 0x0f, 0x58, 0xc1], &state)?
            .state
            .unwrap();
        let sub = oracle
            .query(&[0xf3, 0x0f, 0x5c, 0xc1], &state)?
            .state
            .unwrap();
        assert_eq!(add.xmm[0][0], 0x40400000);
        assert_eq!(sub.xmm[0][0], 0x3f800000);
    }
    state.mm[0] = [0x7fff7fff; 2];
    state.mm[1] = [0x00010001; 2];
    let out = oracle.query(&[0x0f, 0xed, 0xc1], &state)?.state.unwrap(); // paddsw
    assert_eq!(out.mm[0], [0x7fff7fff; 2]);
    for flags in 0..64 {
        state.mxcsr = 0x1f80 | flags;
        let out = oracle.query(&[0x0f, 0x28, 0xc1], &state)?.state.unwrap();
        assert_eq!(out.mxcsr, state.mxcsr);
        assert_eq!(out.eflags & 0x8d5, state.eflags & 0x8d5);
    }
    assert!(oracle.query(&[0x90], &state).is_err());
    assert!(
        oracle
            .query(&[0xf3, 0x0f, 0x58, 0xc1, 0x90], &state)
            .is_err()
    );
    state.mxcsr |= 0x40;
    assert!(oracle.query(&[0x0f, 0x28, 0xc1], &state).is_err());
    drop(oracle);
    // Exercise actual signal capture and recovery using the low-level protocol.
    // UD2 is intentionally excluded from the safe Rust SSE/MMX API.
    use std::{
        io::{Read, Write},
        process::{Command, Stdio},
    };
    let mut child = Command::new("arch")
        .arg("-x86_64")
        .arg(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().unwrap();
    let mut output = child.stdout.take().unwrap();
    for (bytes, traps) in [(&[0x0f, 0x0b][..], true), (&[0x0f, 0x28, 0xc1][..], false)] {
        let mut request = vec![0u8; 244];
        request[..4].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        request[4..4 + bytes.len()].copy_from_slice(bytes);
        request[20 + 192..20 + 196].copy_from_slice(&0x1f80u32.to_le_bytes());
        input.write_all(&request)?;
        input.flush()?;
        let mut response = [0u8; 228];
        output.read_exact(&mut response)?;
        assert_eq!(
            u32::from_le_bytes(response[224..].try_into().unwrap()) != 0,
            traps
        );
    }
    drop(input);
    assert!(child.wait()?.success());
    std::fs::remove_dir_all(root)?;
    Ok(())
}
