//! Rust layouts are constant assertions on every compiled target. Clang checks
//! every header field against those values for both Macs, even on arm64 CI.
use runtime::{CpuAbi, TransferAbi, cpu::X87Abi};
use std::{
    fs,
    mem::{align_of, offset_of, size_of},
    process::Command,
};

const _: () = {
    assert!(size_of::<CpuAbi>() == 288 && align_of::<CpuAbi>() == 4);
    assert!(size_of::<X87Abi>() == 100 && align_of::<X87Abi>() == 4);
    assert!(size_of::<TransferAbi>() == 28 && align_of::<TransferAbi>() == 4);
    assert!(offset_of!(CpuAbi, eax) == 8);
    assert!(offset_of!(CpuAbi, eip) == 40);
    assert!(offset_of!(CpuAbi, fs_base) == 44);
    assert!(offset_of!(CpuAbi, eflags) == 48);
    assert!(offset_of!(CpuAbi, eflags_defined) == 52);
    assert!(offset_of!(CpuAbi, x87) == 56);
    assert!(offset_of!(CpuAbi, xmm) == 156);
    assert!(offset_of!(CpuAbi, mxcsr) == 284);
};

#[test]
fn c_header_matches_every_rust_field_on_both_macs() {
    let mut source = String::from("#include \"h2abi.h\"\n");
    macro_rules! check {
        ($ty:ty, $($field:ident),+ $(,)?) => { $( source.push_str(&format!(
            "_Static_assert(offsetof({}, {}) == {}, \"{}.{}\");\n",
            stringify!($ty), stringify!($field), offset_of!($ty, $field), stringify!($ty), stringify!($field))); )+ };
    }
    check!(
        CpuAbi,
        abi_version,
        struct_size,
        eax,
        ecx,
        edx,
        ebx,
        esp,
        ebp,
        esi,
        edi,
        eip,
        fs_base,
        eflags,
        eflags_defined,
        x87,
        xmm,
        mxcsr
    );
    check!(
        X87Abi,
        st,
        control,
        status,
        tag,
        opcode,
        instruction_pointer,
        data_pointer,
        instruction_selector,
        data_selector
    );
    check!(
        TransferAbi,
        abi_version,
        struct_size,
        kind,
        route,
        target_pc,
        resume_pc,
        event_id
    );
    for (name, actual, expected) in [
        ("H2_ABI_VERSION", runtime::cpu::ABI_VERSION, 2),
        ("H2_CALL", runtime::abi::CALL, 1),
        ("H2_JUMP", runtime::abi::JUMP, 2),
        ("H2_RETURN", runtime::abi::RETURN, 3),
        ("H2_FAULT", runtime::abi::FAULT, 4),
        ("H2_STOP", runtime::abi::STOP, 5),
        ("H2_ENTRY", runtime::abi::ENTRY, 0),
        ("H2_RESUME", runtime::abi::RESUME, 1),
        ("H2_RETURN_TARGET", runtime::abi::RETURN_TARGET, 2),
        ("H2_HARNESS_EXIT", runtime::exec::HARNESS_EXIT, 0xffff_fffc),
    ] {
        assert_eq!(actual, expected);
        source.push_str(&format!(
            "_Static_assert({name} == {actual}u, \"{name}\");\n"
        ));
    }
    for (name, op) in [
        ("ADD", runtime::atomic::AtomicOp::Add),
        ("ADC", runtime::atomic::AtomicOp::Adc),
        ("SUB", runtime::atomic::AtomicOp::Sub),
        ("SBB", runtime::atomic::AtomicOp::Sbb),
        ("AND", runtime::atomic::AtomicOp::And),
        ("OR", runtime::atomic::AtomicOp::Or),
        ("XOR", runtime::atomic::AtomicOp::Xor),
        ("INC", runtime::atomic::AtomicOp::Inc),
        ("DEC", runtime::atomic::AtomicOp::Dec),
        ("NEG", runtime::atomic::AtomicOp::Neg),
        ("NOT", runtime::atomic::AtomicOp::Not),
        ("BTS", runtime::atomic::AtomicOp::Bts),
        ("BTR", runtime::atomic::AtomicOp::Btr),
        ("BTC", runtime::atomic::AtomicOp::Btc),
    ] {
        source.push_str(&format!(
            "_Static_assert(H2_ATOMIC_{name} == {}u, \"atomic {name}\");\n",
            op as u32
        ));
    }
    let directory = std::env::temp_dir().join(format!("h2abi-layout-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("layout.c");
    fs::write(&path, source).unwrap();
    for target in ["arm64-apple-macos14", "x86_64-apple-macos10.15"] {
        let output = Command::new("clang")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-target",
                target,
                "-fsyntax-only",
                "-I",
            ])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/include"))
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn default_version_probes_link_in_multiple_generated_objects() {
    let directory = std::env::temp_dir().join(format!("h2abi-modules-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let a = directory.join("a.c");
    let b = directory.join("b.c");
    let main = directory.join("main.c");
    fs::write(&a,"#include \"h2abi.h\"\nH2_DEFINE_GENERATED_ABI()\nuint32_t version_a(void) { return h2gen_abi_version(); }\n").unwrap();
    fs::write(&b,"#include \"h2abi.h\"\nH2_DEFINE_GENERATED_ABI()\nuint32_t version_b(void) { return h2gen_abi_version(); }\n").unwrap();
    fs::write(&main,"#include \"h2abi.h\"\nuint32_t version_a(void); uint32_t version_b(void);\nint main(void) { return (version_a() == H2_ABI_VERSION && version_b() == H2_ABI_VERSION) ? 0 : 1; }\n").unwrap();
    let binary = directory.join("modules");
    let output = Command::new("clang")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/include"))
        .args([&a, &b, &main])
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Command::new(&binary).status().unwrap().success());
    fs::remove_dir_all(directory).unwrap();
}
