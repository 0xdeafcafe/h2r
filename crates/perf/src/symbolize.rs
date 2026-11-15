//! Host PC to name, and generated code back to the guest.
//!
//! Every generated handler is the C function `h2fn_<entry>` (emit's
//! `c::symbol`), so a PC whose symbol has that name is in the guest
//! function at `<entry>`. The instruction comes from the line tables the
//! default `H2_DEBUG=lines` build keeps: the PC's C source line, then the
//! nearest `/* 0x........ */` instruction marker at or above that line in
//! the same shard (emit's `c.rs` writes one before every instruction). No
//! sidecar is needed while the shard sources and objects exist; with
//! `H2_DEBUG=none`, or once they are deleted, samples still resolve to the
//! guest function, just not the instruction.
//!
//! Resolution must happen while the dylib is loaded: images are found
//! with `dladdr` and read with addr2line (see [`Symbolizer`]). The marker
//! search is clamped to the generated function the PC is in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Where a sample's time is charged.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    /// Generated code (`h2fn_*`).
    Guest,
    /// The runtime: runner, dispatch, memory accessors, x87/SSE helpers,
    /// the generated module's bridge.
    Runtime,
    /// Kernel HLE and the guest scheduler.
    Kernel,
    /// D3D HLE, push-buffer parsing.
    D3d,
    /// gfx, wgpu, Metal.
    Gfx,
    /// The boot driver's fallback interpreter (IR interpreter).
    Fallback,
    /// Other HLE: XAPI, DSound, Bink, audio.
    Hle,
    /// Nothing recognised on the stack (allocator, libc, the harness).
    Other,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Guest,
        Category::Runtime,
        Category::Kernel,
        Category::D3d,
        Category::Gfx,
        Category::Fallback,
        Category::Hle,
        Category::Other,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Category::Guest => "guest",
            Category::Runtime => "runtime",
            Category::Kernel => "kernel",
            Category::D3d => "d3d",
            Category::Gfx => "gfx",
            Category::Fallback => "fallback",
            Category::Hle => "hle",
            Category::Other => "other",
        }
    }

    /// Classify a (demangled) host symbol name; `None` for frames that
    /// should not decide a sample's category (libc, std, allocator).
    pub fn of_symbol(name: &str) -> Option<Category> {
        let n = name.trim_start_matches(['<', '&', '_', ' ']);
        let n = n.strip_prefix("mut ").unwrap_or(n);
        let starts = |p: &[&str]| p.iter().any(|p| n.starts_with(p));
        if n.starts_with("h2fn_") {
            Some(Category::Guest)
        } else if n.contains("fallback::") || starts(&["ir::interp"]) {
            Some(Category::Fallback)
        } else if starts(&["kernel::"]) {
            Some(Category::Kernel)
        } else if starts(&["d3d::"]) {
            Some(Category::D3d)
        } else if starts(&["gfx::", "wgpu", "naga::", "metal::", "objc"])
            || n.contains("MTL")
            || n.contains("AGX")
        {
            Some(Category::Gfx)
        } else if starts(&[
            "runtime::",
            "h2abi_",
            "h2shim_",
            "h2x87",
            "x87::",
            "simd::",
            "h2bridge",
            "h2boot_",
        ]) {
            Some(Category::Runtime)
        } else if starts(&[
            "xapi::",
            "dsound::",
            "bink_hle::",
            "audio::",
            "wma::",
            "movies::",
        ]) {
            Some(Category::Hle)
        } else {
            None
        }
    }
}

/// One resolved frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Frame {
    /// Generated code: the guest function's entry and, when line tables
    /// allow, the guest instruction.
    Guest { function: u32, insn: Option<u32> },
    /// Host code, demangled without the hash.
    Host { name: String },
    /// No symbol.
    Unknown { pc: u64 },
}

impl Frame {
    pub fn category(&self) -> Option<Category> {
        match self {
            Frame::Guest { .. } => Some(Category::Guest),
            Frame::Host { name } => Category::of_symbol(name),
            Frame::Unknown { .. } => None,
        }
    }
    /// The frame's name in folded stacks and tables.
    pub fn label(&self) -> String {
        match self {
            Frame::Guest { function, .. } => format!("guest {function:#010x}"),
            Frame::Host { name } => name.clone(),
            Frame::Unknown { pc } => format!("{pc:#x}"),
        }
    }
    /// Same function (instruction ignored).
    pub fn same_function(&self, other: &Frame) -> bool {
        match (self, other) {
            (Frame::Guest { function: a, .. }, Frame::Guest { function: b, .. }) => a == b,
            (a, b) => a == b,
        }
    }
}

/// `h2fn_002d21c6` (with or without the Mach-O leading underscore) to
/// `0x002d21c6`.
pub fn guest_entry(symbol: &str) -> Option<u32> {
    let s = symbol.trim_start_matches('_').strip_prefix("h2fn_")?;
    let hex: String = s.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (hex.len() == 8).then(|| u32::from_str_radix(&hex, 16).ok())?
}

/// One instruction marker of a C file: the 1-based source line, the guest
/// instruction, and the generated function (`h2fn_<entry>`) whose body
/// holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marker {
    pub line: u32,
    pub pc: u32,
    pub function: u32,
}

/// The instruction markers of one C file, ascending by line. A marker
/// belongs to the last `h2fn_<entry>(` definition line above it; markers
/// before any definition are dropped.
pub fn markers(text: &str) -> Vec<Marker> {
    let mut out = Vec::new();
    let mut function = None;
    for (i, l) in text.lines().enumerate() {
        if !l.starts_with([' ', '\t'])
            && let Some(at) = l.find("h2fn_")
            && let Some(entry) = guest_entry(&l[at..])
            && l[at + 13..].starts_with('(')
        {
            function = Some(entry);
            continue;
        }
        let t = l.trim_start();
        let Some(rest) = t.strip_prefix("/* 0x") else {
            continue;
        };
        let hex = &rest[..rest.len().min(8)];
        if let Some(function) = function
            && hex.len() == 8
            && rest[8..].starts_with(" */")
            && let Ok(pc) = u32::from_str_radix(hex, 16)
        {
            out.push(Marker {
                line: i as u32 + 1,
                pc,
                function,
            });
        }
    }
    out
}

/// The instruction `line` belongs to in generated function `function`:
/// the last marker at or above it, provided it is that function's. A line
/// before the function's first marker (its prologue, the selector switch)
/// has none; it never inherits the previous function's last instruction.
pub fn insn_at(markers: &[Marker], line: u32, function: u32) -> Option<u32> {
    let i = markers.partition_point(|m| m.line <= line);
    let m = markers.get(i.checked_sub(1)?)?;
    (m.function == function).then_some(m.pc)
}

/// Strip the Mach-O underscore and demangle a Rust symbol (no hash).
fn display_name(raw: &str) -> String {
    let n = raw.strip_prefix('_').unwrap_or(raw);
    match rustc_demangle::try_demangle(n) {
        Ok(d) => format!("{d:#}"),
        Err(_) => n.to_owned(),
    }
}

/// What `dladdr` says about a PC: the image's path and load address, and
/// the nearest exported symbol.
struct Dl {
    path: PathBuf,
    base: u64,
    symbol: Option<String>,
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn dladdr(pc: u64) -> Option<Dl> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr only reads the loader's image list; `info` is ours.
    if unsafe { libc::dladdr(pc as *const libc::c_void, &mut info) } == 0
        || info.dli_fname.is_null()
    {
        return None;
    }
    // SAFETY: non-null C strings owned by the loader, valid while the image
    // stays loaded, which it does for this call.
    let (path, symbol) = unsafe {
        (
            PathBuf::from(std::ffi::OsStr::from_bytes(
                CStr::from_ptr(info.dli_fname).to_bytes(),
            )),
            (!info.dli_sname.is_null()).then(|| {
                CStr::from_ptr(info.dli_sname)
                    .to_string_lossy()
                    .into_owned()
            }),
        )
    };
    Some(Dl {
        path,
        base: info.dli_fbase as u64,
        symbol,
    })
}

#[cfg(not(unix))]
fn dladdr(_: u64) -> Option<Dl> {
    None
}

/// A source location: file and line.
pub type Location = (PathBuf, u32);

/// One frame of an [`ImageView`]: raw function name and location.
pub type ViewFrame = (Option<String>, Option<Location>);

/// An image's own view of a PC; see [`Symbolizer::image_view`].
#[derive(Debug, Clone, Default)]
pub struct ImageView {
    pub symbol: Option<String>,
    /// Innermost first.
    pub frames: Vec<ViewFrame>,
}

/// The preferred (link-time) virtual address of `pc`, which is what the
/// image's symbol and DWARF tables are written in. On Mach-O that is
/// `pc - slide`, with the slide the load address of the header minus the
/// preferred `__TEXT` vmaddr: 0 for a dylib, 0x100000000 for an
/// executable. Elsewhere, `pc - base` plus addr2line's relative base
/// (right for PIE ELF and PE).
pub fn preferred_address(pc: u64, base: u64, relative_base: u64) -> u64 {
    match preferred_text(base) {
        Some(text) => pc.wrapping_sub(base.wrapping_sub(text)),
        None => pc.wrapping_sub(base).wrapping_add(relative_base),
    }
}

/// The preferred vmaddr of the `__TEXT` segment of the 64-bit Mach-O image
/// whose header is mapped at `base`.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn preferred_text(base: u64) -> Option<u64> {
    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const LC_SEGMENT_64: u32 = 0x19;
    if base == 0 {
        return None;
    }
    let at = |off: u64| base.wrapping_add(off) as *const u8;
    // SAFETY: `base` is dladdr's `dli_fbase`, the start of a loaded image's
    // mapped Mach-O header; the header and its load commands (sizeofcmds
    // bytes after it) are mapped for as long as the image is loaded. Every
    // read stays inside them.
    unsafe {
        let u32_at = |off: u64| std::ptr::read_unaligned(at(off) as *const u32);
        if u32_at(0) != MH_MAGIC_64 {
            return None;
        }
        let (ncmds, sizeofcmds) = (u32_at(16) as u64, u32_at(20) as u64);
        let (mut off, end) = (32u64, 32 + sizeofcmds);
        for _ in 0..ncmds {
            if off + 8 > end {
                return None;
            }
            let (cmd, size) = (u32_at(off), u32_at(off + 4) as u64);
            if size < 8 || off + size > end {
                return None;
            }
            if cmd == LC_SEGMENT_64 && size >= 72 {
                let name = std::slice::from_raw_parts(at(off + 8), 16);
                if name.starts_with(b"__TEXT\0") {
                    return Some(std::ptr::read_unaligned(at(off + 24) as *const u64));
                }
            }
            off += size;
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn preferred_text(_: u64) -> Option<u64> {
    None
}

/// Resolves PCs and caches the answers; one per profile.
///
/// Images are found with `dladdr`, which asks the dynamic loader at every
/// call, so a dylib loaded after another symbolizer (or after an earlier
/// lookup) is seen. Each image's symbol table and line tables (including
/// macOS debug maps, which point at the objects) are read with addr2line
/// on first use and cached by path and load address.
#[derive(Default)]
pub struct Symbolizer {
    pcs: HashMap<u64, Frame>,
    images: HashMap<(PathBuf, u64), Option<addr2line::Loader>>,
    files: HashMap<PathBuf, Vec<Marker>>,
}

impl Symbolizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the instruction at `pc`. For return addresses pass `pc - 1`
    /// so the call site, not the next statement, is looked up.
    pub fn resolve(&mut self, pc: u64) -> Frame {
        if let Some(f) = self.pcs.get(&pc) {
            return f.clone();
        }
        let frame = self.lookup(pc);
        self.pcs.insert(pc, frame.clone());
        frame
    }

    /// What the image's own tables say about `pc`, without the `dladdr`
    /// symbol fallback: its symbol and its frames, innermost first (raw
    /// function name, source location). For diagnostics and tests.
    pub fn image_view(&mut self, pc: u64) -> Option<ImageView> {
        let dl = dladdr(pc)?;
        Some(self.view(pc, &dl))
    }

    fn view(&mut self, pc: u64, dl: &Dl) -> ImageView {
        let mut v = ImageView::default();
        let loader = self
            .images
            .entry((dl.path.clone(), dl.base))
            .or_insert_with(|| addr2line::Loader::new(&dl.path).ok());
        let Some(l) = loader else {
            return v;
        };
        let probe = preferred_address(pc, dl.base, l.relative_address_base());
        v.symbol = l.find_symbol(probe).map(str::to_owned);
        if let Ok(mut frames) = l.find_frames(probe) {
            while let Ok(Some(f)) = frames.next() {
                let name = f
                    .function
                    .as_ref()
                    .and_then(|n| n.raw_name().ok())
                    .map(|n| n.into_owned());
                let loc = f
                    .location
                    .and_then(|loc| Some((PathBuf::from(loc.file?), loc.line?)));
                v.frames.push((name, loc));
            }
        }
        v
    }

    fn lookup(&mut self, pc: u64) -> Frame {
        let Some(dl) = dladdr(pc) else {
            return Frame::Unknown { pc };
        };
        let v = self.view(pc, &dl);
        let Some(symbol) = v.symbol.or(dl.symbol) else {
            return Frame::Unknown { pc };
        };
        let inner = v.frames.iter().find_map(|(n, _)| n.clone());
        match guest_entry(&symbol) {
            Some(function) => {
                // Later frames are outer: the handler's own `.c` line wins
                // over an inlined h2gen.h helper's.
                let c_line = v.frames.iter().rev().find_map(|(_, loc)| {
                    loc.clone()
                        .filter(|(f, _)| f.extension().is_some_and(|e| e == "c"))
                });
                Frame::Guest {
                    function,
                    insn: c_line.and_then(|(f, l)| self.insn(&f, l, function)),
                }
            }
            None => Frame::Host {
                name: display_name(inner.as_deref().unwrap_or(&symbol)),
            },
        }
    }

    fn insn(&mut self, file: &Path, line: u32, function: u32) -> Option<u32> {
        let m = self.files.entry(file.to_path_buf()).or_insert_with(|| {
            std::fs::read_to_string(file)
                .map(|t| markers(&t))
                .unwrap_or_default()
        });
        insn_at(m, line, function)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_symbols_name_their_guest_entry() {
        assert_eq!(guest_entry("h2fn_002d21c6"), Some(0x002d21c6));
        assert_eq!(guest_entry("_h2fn_00011000"), Some(0x11000));
        assert_eq!(guest_entry("h2fn_123"), None);
        assert_eq!(guest_entry("runtime::runner::Runner::run_from"), None);
    }

    #[test]
    fn source_lines_map_to_the_instruction_above_them() {
        let c = "TransferAbi h2fn_00001000(CpuAbi *cpu) {\n\
                 L_00001000:;\n\
                 \x20   /* 0x00001000 */ {\n\
                 \x20       uint32_t t0 = eax;\n\
                 \x20   }\n\
                 \x20   /* 0x00001003 */ {\n\
                 \x20       H2_XFER(H2_JUMP, H2_ENTRY, 0x2000u, 0);\n\
                 \x20   }\n\
                 }\n\
                 TransferAbi h2fn_00002000(CpuAbi *cpu) {\n\
                 \x20   H2_LOCALS();\n\
                 \x20   /* 0x00002000 */ {\n\
                 \x20   }\n";
        let m = markers(c);
        let mk = |line, pc, function| Marker { line, pc, function };
        assert_eq!(
            m,
            vec![
                mk(3, 0x1000, 0x1000),
                mk(6, 0x1003, 0x1000),
                mk(12, 0x2000, 0x2000)
            ]
        );
        assert_eq!(insn_at(&m, 1, 0x1000), None);
        assert_eq!(insn_at(&m, 3, 0x1000), Some(0x1000));
        assert_eq!(insn_at(&m, 4, 0x1000), Some(0x1000));
        assert_eq!(insn_at(&m, 7, 0x1000), Some(0x1003));
        // The second function's prologue does not inherit 0x1003.
        assert_eq!(insn_at(&m, 10, 0x2000), None);
        assert_eq!(insn_at(&m, 11, 0x2000), None);
        assert_eq!(insn_at(&m, 13, 0x2000), Some(0x2000));
        // A line attributed to the wrong function gives nothing.
        assert_eq!(insn_at(&m, 13, 0x1000), None);
    }

    #[test]
    fn symbols_classify_by_crate() {
        let c = Category::of_symbol;
        assert_eq!(c("h2fn_00001000"), Some(Category::Guest));
        assert_eq!(
            c("runtime::runner::Runner::run_from"),
            Some(Category::Runtime)
        );
        assert_eq!(c("h2abi_v1_read"), Some(Category::Runtime));
        assert_eq!(
            c("<kernel::Kernel as core::ops::Drop>::drop"),
            Some(Category::Kernel)
        );
        assert_eq!(c("kernel::threads::Machine::run"), Some(Category::Kernel));
        assert_eq!(c("d3d::device::Device<B>::drain"), Some(Category::D3d));
        assert_eq!(c("wgpu_core::device::queue::submit"), Some(Category::Gfx));
        assert_eq!(
            c("-[AGXG15XFamilyCommandBuffer commit]"),
            Some(Category::Gfx)
        );
        assert_eq!(c("boot::fallback::trampoline"), Some(Category::Fallback));
        assert_eq!(c("ir::interp::Interp::step"), Some(Category::Fallback));
        assert_eq!(c("xapi::service_input"), Some(Category::Hle));
        assert_eq!(c("platform_memmove"), None);
        assert_eq!(c("std::sys::thread::unix::Thread::new::thread_start"), None);
    }

    /// The test binary is a Mach-O executable (preferred __TEXT at
    /// 0x100000000, unlike a dylib's 0): its own symbol table, not just
    /// dladdr, must find a local function, so the slide is applied right.
    #[cfg(target_os = "macos")]
    #[test]
    fn executables_are_looked_up_at_their_preferred_addresses() {
        #[inline(never)]
        fn exe_marker() -> u64 {
            std::hint::black_box(11)
        }
        let pc = exe_marker as *const () as u64 + 1;
        let dl = dladdr(pc).unwrap();
        assert_eq!(
            preferred_text(dl.base),
            Some(0x1_0000_0000),
            "{}",
            dl.path.display()
        );
        let mut s = Symbolizer::new();
        let v = s.image_view(pc).unwrap();
        let sym = v.symbol.as_deref().map(display_name).unwrap_or_default();
        assert!(sym.contains("exe_marker"), "{sym:?}");
        // With debug info (any but CARGO_PROFILE_TEST_DEBUG=0) the frame
        // names and locations come from DWARF too.
        if let Some((Some(name), Some((file, _)))) = v.frames.first() {
            assert!(display_name(name).contains("exe_marker"), "{name}");
            assert!(file.ends_with("symbolize.rs"), "{}", file.display());
        }
    }

    /// An inlined function inside the executable resolves as the inner
    /// frame (with DWARF): the PC comes from an `adr` in the inlined body.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn inlined_frames_in_the_executable_name_the_inner_function() {
        #[inline(always)]
        fn inlined_pc_marker() -> u64 {
            let pc: u64;
            // SAFETY: reads the address of this instruction; no side effects.
            #[allow(unsafe_code)]
            unsafe {
                std::arch::asm!("adr {}, .", out(reg) pc)
            };
            pc
        }
        #[inline(never)]
        fn outer_pc_marker() -> u64 {
            std::hint::black_box(inlined_pc_marker())
        }
        let pc = outer_pc_marker();
        let mut s = Symbolizer::new();
        let v = s.image_view(pc).unwrap();
        let sym = v.symbol.as_deref().map(display_name).unwrap_or_default();
        assert!(sym.contains("outer_pc_marker"), "{sym:?}");
        if v.frames.iter().any(|(n, _)| n.is_some()) {
            let names: Vec<String> = v
                .frames
                .iter()
                .filter_map(|(n, _)| n.as_deref().map(display_name))
                .collect();
            assert!(names[0].contains("inlined_pc_marker"), "{names:?}");
            match s.resolve(pc) {
                Frame::Host { name } => assert!(name.contains("inlined_pc_marker"), "{name}"),
                f => panic!("{f:?}"),
            }
        }
    }

    /// A Rust function in this test binary resolves to its own name.
    #[test]
    fn resolves_host_functions_in_process() {
        #[inline(never)]
        fn marker_function() -> u64 {
            std::hint::black_box(7)
        }
        let pc = marker_function as *const () as u64;
        let mut s = Symbolizer::new();
        match s.resolve(pc) {
            Frame::Host { name } => assert!(name.contains("marker_function"), "{name}"),
            f => panic!("{f:?}"),
        }
    }
}
