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

    fn lookup(&mut self, pc: u64) -> Frame {
        let Some(dl) = dladdr(pc) else {
            return Frame::Unknown { pc };
        };
        let loader = self
            .images
            .entry((dl.path.clone(), dl.base))
            .or_insert_with(|| addr2line::Loader::new(&dl.path).ok());
        let mut symbol = dl.symbol;
        // (innermost function name, outermost `.c` location)
        let mut inner: Option<String> = None;
        let mut c_line: Option<(PathBuf, u32)> = None;
        if let Some(l) = loader {
            let probe = pc
                .wrapping_sub(dl.base)
                .wrapping_add(l.relative_address_base());
            if let Some(s) = l.find_symbol(probe) {
                symbol = Some(s.to_owned());
            }
            if let Ok(mut frames) = l.find_frames(probe) {
                while let Ok(Some(f)) = frames.next() {
                    if inner.is_none()
                        && let Some(n) = f.function.as_ref().and_then(|n| n.raw_name().ok())
                    {
                        inner = Some(n.into_owned());
                    }
                    if let Some(loc) = f.location
                        && let (Some(file), Some(line)) = (loc.file, loc.line)
                        && file.ends_with(".c")
                    {
                        // Later frames are outer: the handler's own line
                        // wins over an inlined h2gen.h helper's.
                        c_line = Some((PathBuf::from(file), line));
                    }
                }
            }
        }
        let Some(symbol) = symbol else {
            return Frame::Unknown { pc };
        };
        match guest_entry(&symbol) {
            Some(function) => Frame::Guest {
                function,
                insn: c_line.and_then(|(f, l)| self.insn(&f, l, function)),
            },
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
