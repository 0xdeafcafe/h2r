//! `H2_EMIT_TARGET=windows` end to end on a Mac: real generated code (guest
//! memory, calls, x87 and SSE, a loop) through the support headers, clang
//! for `x86_64-pc-windows-msvc`, `llvm-lib` and `lld-link`, into a DLL that
//! exports what the hosts look up and nothing else. Nothing here can load
//! the DLL: what runs on Windows is only compile- and link-checked.
//!
//! Needs the xwin splat (`H2_XWIN`), `lld-link`, `llvm-lib` and
//! `llvm-readobj` (platform/windows/env.sh, docs/runtime/windows.md); skipped
//! without them.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use emit::compile::{self, Arch};
use iced_x86::code_asm::*;

const BASE: u32 = 0x0001_1000;
const CALLEE: u32 = 0x0001_2000;
const DATA: u32 = 0x0002_0000;

fn functions() -> Vec<ir::IrFunction> {
    vec![
        common::function(BASE, |a| {
            let mut top = a.create_label();
            a.mov(ecx, 4)?;
            a.set_label(&mut top)?;
            // Guest memory, x87 and SSE on every trip.
            a.mov(eax, dword_ptr(DATA))?;
            a.add(eax, ecx)?;
            a.mov(dword_ptr(DATA), eax)?;
            a.fld(dword_ptr(DATA + 4))?;
            a.fmul(dword_ptr(DATA + 8))?;
            a.fstp(dword_ptr(DATA + 12))?;
            a.movss(xmm0, dword_ptr(DATA + 16))?;
            a.addss(xmm0, dword_ptr(DATA + 20))?;
            a.mulps(xmm0, xmm0)?;
            a.movss(dword_ptr(DATA + 24), xmm0)?;
            a.call(CALLEE as u64)?;
            a.dec(ecx)?;
            a.jnz(top)?;
            a.ret()
        }),
        common::function(CALLEE, |a| {
            a.push(ecx)?;
            a.fld1()?;
            a.fsqrt()?;
            a.fistp(dword_ptr(DATA + 28))?;
            a.pop(ecx)?;
            a.ret()
        }),
    ]
}

/// The tools, or why the test cannot run here.
fn tools() -> Result<(PathBuf, PathBuf), String> {
    compile::xwin_dir().map_err(|e| e.0)?;
    compile::windows_tool("lld-link").map_err(|e| e.0)?;
    compile::windows_tool("llvm-lib").map_err(|e| e.0)?;
    let readobj = compile::windows_tool("llvm-readobj").map_err(|e| e.0)?;
    let cc = compile::compiler().map_err(|e| e.0)?;
    Ok((cc, readobj))
}

fn readobj(tool: &Path, args: &[&str], dll: &Path) -> String {
    let o = Command::new(tool).args(args).arg(dll).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// The names after `Name: ` in llvm-readobj's export or import listing.
fn names(listing: &str) -> Vec<String> {
    let mut v: Vec<String> =
        listing.lines().filter_map(|l| l.trim().strip_prefix("Name: ")).filter(|n| !n.is_empty()).map(str::to_owned).collect();
    v.sort();
    v
}

#[test]
fn a_generated_module_links_into_a_windows_dll_exporting_only_the_host_symbols() {
    let (_cc, readobj_tool) = match tools() {
        Ok(t) => t,
        Err(why) => return common::skip("a_generated_module_links_into_a_windows_dll_exporting_only_the_host_symbols", &why),
    };
    let header = common::abi_header().expect("h2abi.h");
    let owners = emit::plan_with(functions(), emit::opt::Passes::from_env().unwrap());
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("windows-module");
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    emit::write_module_with(&owners, &src, emit::Codegen { accounting: true, ..emit::Codegen::default() }).unwrap();
    compile::install_header(&src, &header).unwrap();
    let b = compile::build(&src, &dir, Arch::X86_64Windows, 2).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(b.arch, Arch::X86_64Windows);
    assert!(b.objects.iter().all(|o| o.to_string_lossy().contains(".x86_64-windows.")), "{:?}", b.objects);
    // The archive is llvm-lib's and holds exactly the objects.
    let mut members = compile::archive_members_for(Arch::X86_64Windows, &b.library).unwrap();
    members.sort();
    let mut want: Vec<String> = b.objects.iter().map(|o| o.file_name().unwrap().to_string_lossy().into_owned()).collect();
    want.sort();
    assert_eq!(members, want);
    // The same build again reuses every object and the archive.
    let again = compile::build(&src, &dir, Arch::X86_64Windows, 2).unwrap();
    assert_eq!((again.compiled, again.reused), (0, b.objects.len()));

    // As emit's host loads it (emit::host::Generated)...
    let emit_dll = dir.join("emit.dll");
    compile::host_dylib(&b, &src, &[("bridge.c", emit::host::BRIDGE)], &emit_dll).unwrap_or_else(|e| panic!("{e}"));
    // ...and as boot does (boot::build): boot's bridge and the x87 bridge
    // compiled apart, exactly as boot::BRIDGE and boot::BRIDGE_X87 are.
    let boot_bridge = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../boot/c/boot_bridge.c")).unwrap();
    let boot_x87 = format!("#include \"h2gen.h\"\n{}", include_str!("../c/bridge_x87.c"));
    let boot_dll = dir.join(Arch::X86_64Windows.module_file_name());
    compile::host_dylib(&b, &src, &[("h2boot_bridge.c", &boot_bridge), ("h2boot_bridge_x87.c", &boot_x87)], &boot_dll)
        .unwrap_or_else(|e| panic!("{e}"));

    for (dll, bridge) in [
        (
            &emit_dll,
            &[
                "h2bridge_install",
                "h2bridge_install_v2",
                "h2bridge_install_v3",
                "h2bridge_layout",
                "h2bridge_layout_v2",
                "h2bridge_layout_v3",
            ][..],
        ),
        (&boot_dll, &["h2boot_install_v3"][..]),
    ] {
        let header = readobj(&readobj_tool, &["--file-headers"], dll);
        assert!(header.contains("IMAGE_FILE_MACHINE_AMD64"), "{header}");
        assert!(header.contains("IMAGE_FILE_DLL"), "{header}");
        // Exactly the marked symbols: a DLL holds at most 65,535 exports,
        // and the retail image has some 18,000 handlers.
        let mut want: Vec<String> = ["h2gen_module", "h2gen_module_abi_version", "h2x87bridge_install"]
            .iter()
            .chain(bridge)
            .map(|s| s.to_string())
            .collect();
        want.sort();
        assert_eq!(names(&readobj(&readobj_tool, &["--coff-exports"], dll)), want, "{}", dll.display());
        // One C runtime, the DLL one the Rust host links too, and the
        // kernel: no static CRT, nothing else.
        let imports = names(&readobj(&readobj_tool, &["--coff-imports"], dll));
        assert!(imports.iter().any(|i| i.eq_ignore_ascii_case("VCRUNTIME140.dll")), "{imports:?}");
        assert!(
            imports.iter().all(|i| {
                let i = i.to_ascii_lowercase();
                i == "kernel32.dll" || i == "vcruntime140.dll" || i.starts_with("api-ms-win-crt-")
            }),
            "{imports:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
