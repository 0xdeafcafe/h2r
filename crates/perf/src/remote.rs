//! Profiles taken where they can't be symbolised: the Xbox app
//! (docs/runtime/xbox.md, "Profiling on the console").
//!
//! On the device, [`Dump::from_raw`] turns the sampler's stacks into
//! module-relative addresses (`h2game.dll+0x1a2b3c`) and counts identical
//! stacks; [`Dump::write`] makes the text file the app leaves in
//! LocalState. On the Mac, [`Dump::parse`] reads it back and
//! [`Symbols`] resolves it against the very binaries that were deployed:
//!
//! * the generated module (`h2game.dll`): its exported `h2gen_module`
//!   entry table names the handler of every guest function, and its
//!   `.pdata` gives each function's extent, so a PC in a handler becomes
//!   that guest function with no PDB and no change to the generator;
//! * the executable (`h2uwp.exe`): the linker map `build.sh` keeps
//!   (`/map`), demangled, plus the executable's `.pdata`.
//!
//! The result is an ordinary [`Profile`], so the tables, folded stacks and
//! JSON are the same as `h2 profile`'s.
//!
//! The dump's format, one record per line:
//!
//! ```text
//! h2prof 1
//! hz 1000
//! wall_ms 20004
//! missed 0
//! mean_pause_ns 31000
//! thread 0 game
//! module 0 h2game.dll 0x7ffb40000000 0x6400000
//! stack 37 0 1 0:1a2b3c 0:1a0000 *:7ffb9c001234
//! ```
//!
//! `stack COUNT THREAD ON_CPU FRAME...`, innermost first; a frame is
//! `MODULE:OFFSET` (hex) or `*:ADDRESS` outside every known module.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::time::Duration;

use crate::profile::{Profile, Stack};
use crate::sample::RawProfile;
use crate::symbolize::{Category, Frame};

/// A loaded image the samples fell in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// File name only (`h2game.dll`).
    pub name: String,
    pub base: u64,
    pub size: u64,
}

/// One frame of a dump.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pc {
    In { module: usize, offset: u64 },
    Abs(u64),
}

/// Identical stacks, counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpStack {
    pub count: u64,
    pub thread: usize,
    pub on_cpu: bool,
    pub frames: Vec<Pc>,
}

/// A profile with module-relative frames.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dump {
    pub hz: u32,
    pub wall: Duration,
    pub missed: u64,
    pub mean_pause_ns: u64,
    pub threads: Vec<String>,
    pub modules: Vec<Module>,
    pub stacks: Vec<DumpStack>,
}

impl Dump {
    /// Counts `raw`'s stacks and maps each frame to a module with
    /// `locate`, which names the image an address is in: its file name,
    /// base and size. It is asked only for addresses outside the modules
    /// already found.
    pub fn from_raw(raw: &RawProfile, mut locate: impl FnMut(u64) -> Option<Module>) -> Dump {
        let mut modules: Vec<Module> = Vec::new();
        let mut counts: HashMap<(usize, bool, Vec<Pc>), u64> = HashMap::new();
        for s in &raw.samples {
            let frames = s
                .frames
                .iter()
                .map(|&pc| {
                    let found = modules
                        .iter()
                        .position(|m| pc >= m.base && pc - m.base < m.size)
                        .or_else(|| {
                            let m = locate(pc).filter(|m| pc >= m.base && pc - m.base < m.size)?;
                            modules.push(m);
                            Some(modules.len() - 1)
                        });
                    match found {
                        Some(i) => Pc::In {
                            module: i,
                            offset: pc - modules[i].base,
                        },
                        None => Pc::Abs(pc),
                    }
                })
                .collect();
            *counts.entry((s.thread, s.on_cpu, frames)).or_default() += 1;
        }
        let mut stacks: Vec<DumpStack> = counts
            .into_iter()
            .map(|((thread, on_cpu, frames), count)| DumpStack {
                count,
                thread,
                on_cpu,
                frames,
            })
            .collect();
        stacks.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.frames.len().cmp(&b.frames.len()))
        });
        Dump {
            hz: raw.hz,
            wall: raw.wall,
            missed: raw.missed,
            mean_pause_ns: raw.mean_pause_ns,
            threads: raw.threads.clone(),
            modules,
            stacks,
        }
    }

    /// Samples in the dump.
    pub fn samples(&self) -> u64 {
        self.stacks.iter().map(|s| s.count).sum()
    }

    pub fn write(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "h2prof 1");
        let _ = writeln!(o, "hz {}", self.hz);
        let _ = writeln!(o, "wall_ms {}", self.wall.as_millis());
        let _ = writeln!(o, "missed {}", self.missed);
        let _ = writeln!(o, "mean_pause_ns {}", self.mean_pause_ns);
        for (i, t) in self.threads.iter().enumerate() {
            let _ = writeln!(o, "thread {i} {}", one_word(t));
        }
        for (i, m) in self.modules.iter().enumerate() {
            let _ = writeln!(
                o,
                "module {i} {} {:#x} {:#x}",
                one_word(&m.name),
                m.base,
                m.size
            );
        }
        for s in &self.stacks {
            let _ = write!(o, "stack {} {} {}", s.count, s.thread, u8::from(s.on_cpu));
            for f in &s.frames {
                let _ = match f {
                    Pc::In { module, offset } => write!(o, " {module}:{offset:x}"),
                    Pc::Abs(a) => write!(o, " *:{a:x}"),
                };
            }
            o.push('\n');
        }
        o
    }

    pub fn parse(text: &str) -> Result<Dump, String> {
        let mut d = Dump::default();
        let mut lines = text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty());
        match lines.next() {
            Some((_, "h2prof 1")) => {}
            _ => return Err("not an h2prof 1 dump (no `h2prof 1` first line)".into()),
        }
        let hex = |s: &str| {
            u64::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
        };
        for (n, line) in lines {
            let bad = |e: String| format!("line {}: {e}: {line}", n + 1);
            let w: Vec<&str> = line.split_whitespace().collect();
            let num = |i: usize| -> Result<u64, String> {
                w.get(i)
                    .ok_or("missing field")?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())
            };
            match w[0] {
                "hz" => d.hz = num(1).map_err(bad)? as u32,
                "wall_ms" => d.wall = Duration::from_millis(num(1).map_err(bad)?),
                "missed" => d.missed = num(1).map_err(bad)?,
                "mean_pause_ns" => d.mean_pause_ns = num(1).map_err(bad)?,
                "thread" => d
                    .threads
                    .push(w.get(2).copied().unwrap_or("thread").to_owned()),
                "module" => {
                    if w.len() != 5 {
                        return Err(bad("expected module INDEX NAME BASE SIZE".into()));
                    }
                    d.modules.push(Module {
                        name: w[2].to_owned(),
                        base: hex(w[3]).map_err(bad)?,
                        size: hex(w[4]).map_err(bad)?,
                    });
                }
                "stack" => {
                    let mut frames = Vec::new();
                    for f in w.iter().skip(4) {
                        let (m, off) =
                            f.split_once(':').ok_or_else(|| bad(format!("frame {f}")))?;
                        frames.push(if m == "*" {
                            Pc::Abs(hex(off).map_err(bad)?)
                        } else {
                            let module: usize = m.parse().map_err(|_| bad(format!("frame {f}")))?;
                            if module >= d.modules.len() {
                                return Err(bad(format!("frame {f}: no module {module}")));
                            }
                            Pc::In {
                                module,
                                offset: hex(off).map_err(bad)?,
                            }
                        });
                    }
                    d.stacks.push(DumpStack {
                        count: num(1).map_err(bad)?,
                        thread: num(2).map_err(bad)? as usize,
                        on_cpu: num(3).map_err(bad)? != 0,
                        frames,
                    });
                }
                _ => {} // later additions
            }
        }
        Ok(d)
    }
}

fn one_word(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("_")
}

/// The image an address is in, from the Windows loader: its file name,
/// base and `SizeOfImage`. Call with the sampled threads running (it takes
/// the loader lock).
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn locate(pc: u64) -> Option<Module> {
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut isize) -> i32;
        fn GetModuleFileNameW(module: isize, name: *mut u16, size: u32) -> u32;
    }
    let mut h = 0isize;
    // SAFETY: with FROM_ADDRESS the "name" is any address; `h` is ours.
    if unsafe { GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, pc as *const u16, &mut h) }
        == 0
        || h == 0
    {
        return None;
    }
    let mut name = [0u16; 520];
    // SAFETY: the buffer holds `name.len()` UTF-16 units.
    let n = unsafe { GetModuleFileNameW(h, name.as_mut_ptr(), name.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&name[..n.min(name.len())]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
    let base = h as u64;
    // SAFETY: a loaded image's headers are mapped and readable at its base.
    let size = unsafe {
        let p = base as *const u8;
        let lfanew = (p.add(0x3c) as *const u32).read_unaligned() as usize;
        (p.add(lfanew + 0x18 + 0x38) as *const u32).read_unaligned() as u64
    };
    Some(Module {
        name: file,
        base,
        size,
    })
}

#[cfg(not(windows))]
pub fn locate(_pc: u64) -> Option<Module> {
    None
}

// ---- the Mac side ----

/// A PE image's headers, as far as symbolising needs them.
#[derive(Debug, Clone, Default)]
pub struct Pe {
    pub image_base: u64,
    sections: Vec<Section>,
    exports: u32,
    exports_size: u32,
    pdata: u32,
    pdata_size: u32,
}

#[derive(Debug, Clone, Copy)]
struct Section {
    rva: u32,
    vsize: u32,
    raw: u32,
    raw_size: u32,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

impl Pe {
    /// Parses a PE32+ (x64) image's headers.
    pub fn parse(b: &[u8]) -> Result<Pe, String> {
        if b.get(..2) != Some(b"MZ") {
            return Err("not a PE image (no MZ)".into());
        }
        let nt = u32_at(b, 0x3c).ok_or("truncated")? as usize;
        if b.get(nt..nt + 4) != Some(b"PE\0\0") {
            return Err("not a PE image (no PE signature)".into());
        }
        let coff = nt + 4;
        let sections = u16_at(b, coff + 2).ok_or("truncated")? as usize;
        let opt_size = u16_at(b, coff + 16).ok_or("truncated")? as usize;
        let opt = coff + 20;
        if u16_at(b, opt) != Some(0x20b) {
            return Err("not a PE32+ image".into());
        }
        let image_base = u64_at(b, opt + 24).ok_or("truncated")?;
        let dirs = opt + 112;
        let dir = |i: usize| {
            (
                u32_at(b, dirs + 8 * i).unwrap_or(0),
                u32_at(b, dirs + 8 * i + 4).unwrap_or(0),
            )
        };
        let (exports, exports_size) = dir(0);
        let (pdata, pdata_size) = dir(3);
        let mut s = Vec::with_capacity(sections);
        let table = opt + opt_size;
        for i in 0..sections {
            let h = table + 40 * i;
            s.push(Section {
                vsize: u32_at(b, h + 8).ok_or("truncated section table")?,
                rva: u32_at(b, h + 12).ok_or("truncated section table")?,
                raw_size: u32_at(b, h + 16).ok_or("truncated section table")?,
                raw: u32_at(b, h + 20).ok_or("truncated section table")?,
            });
        }
        Ok(Pe {
            image_base,
            sections: s,
            exports,
            exports_size,
            pdata,
            pdata_size,
        })
    }

    /// The file offset of `rva`, when it has bytes in the file.
    pub fn offset(&self, rva: u32) -> Option<usize> {
        self.sections.iter().find_map(|s| {
            let into = rva.checked_sub(s.rva)?;
            (into < s.vsize.max(s.raw_size) && into < s.raw_size).then_some((s.raw + into) as usize)
        })
    }

    /// Every function's `[begin, end)` RVAs from `.pdata`, sorted.
    pub fn functions(&self, b: &[u8]) -> Vec<(u32, u32)> {
        let Some(at) = self.offset(self.pdata) else {
            return Vec::new();
        };
        let mut v: Vec<(u32, u32)> = (0..self.pdata_size as usize / 12)
            .filter_map(|i| Some((u32_at(b, at + 12 * i)?, u32_at(b, at + 12 * i + 4)?)))
            .filter(|(a, e)| a < e)
            .collect();
        v.sort_unstable();
        v
    }

    /// The RVA of the export named `name`.
    pub fn export(&self, b: &[u8], name: &str) -> Option<u32> {
        let d = self.offset(self.exports)?;
        let _ = self.exports_size;
        let names = u32_at(b, d + 24)? as usize;
        let functions = self.offset(u32_at(b, d + 28)?)?;
        let name_ptrs = self.offset(u32_at(b, d + 32)?)?;
        let ordinals = self.offset(u32_at(b, d + 36)?)?;
        for i in 0..names {
            let s = self.offset(u32_at(b, name_ptrs + 4 * i)?)?;
            let end = b.get(s..)?.iter().position(|&c| c == 0)? + s;
            if &b[s..end] == name.as_bytes() {
                let ord = u16_at(b, ordinals + 2 * i)? as usize;
                return u32_at(b, functions + 4 * ord);
            }
        }
        None
    }

    /// The u64 at `rva`.
    fn u64_at_rva(&self, b: &[u8], rva: u32) -> Option<u64> {
        u64_at(b, self.offset(rva)?)
    }

    fn u32_at_rva(&self, b: &[u8], rva: u32) -> Option<u32> {
        u32_at(b, self.offset(rva)?)
    }
}

/// The generated module's handlers: each handler's RVA and the guest
/// function it was generated for, from the exported `h2gen_module` table
/// (emit's `registry`: `{abi, entry_count, resume_count, features,
/// entries, resumes}`; each entry `{addr, kind, handler, abi probe}`, 24
/// bytes, with pointers as preferred-base addresses before relocation).
/// A handler named by several function entries is the lowest one's.
pub fn guest_handlers(pe: &Pe, b: &[u8]) -> Result<HashMap<u32, u32>, String> {
    let m = pe
        .export(b, "h2gen_module")
        .ok_or("no h2gen_module export: not a generated module")?;
    let count = pe
        .u32_at_rva(b, m + 4)
        .ok_or("h2gen_module outside the file")?;
    let entries = pe
        .u64_at_rva(b, m + 16)
        .ok_or("h2gen_module outside the file")?;
    let entries = entries
        .checked_sub(pe.image_base)
        .and_then(|r| u32::try_from(r).ok())
        .ok_or("h2gen_module's entries are not in the image")?;
    let mut out: HashMap<u32, u32> = HashMap::new();
    for i in 0..count {
        let row = entries + 24 * i;
        let (Some(addr), Some(kind), Some(handler)) = (
            pe.u32_at_rva(b, row),
            pe.u32_at_rva(b, row + 4),
            pe.u64_at_rva(b, row + 8),
        ) else {
            return Err(format!("entry {i} is outside the file"));
        };
        if kind != 0 || handler < pe.image_base {
            continue;
        }
        let rva = (handler - pe.image_base) as u32;
        let e = out.entry(rva).or_insert(addr);
        *e = (*e).min(addr);
    }
    Ok(out)
}

/// Symbols from an MSVC-style linker map (`lld-link /map`): `Publics by
/// Value` and `Static symbols` lines, `SECTION:OFFSET NAME RVA+BASE ...`,
/// as `(rva, name)` sorted by RVA. Absolute symbols (section 0) are left
/// out.
pub fn parse_map(text: &str) -> Vec<(u32, String)> {
    let mut base = None;
    let mut out = Vec::new();
    let mut in_symbols = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(b) = t.strip_prefix("Preferred load address is ") {
            base = u64::from_str_radix(b.trim(), 16).ok();
            continue;
        }
        if t.contains("Publics by Value") || t.starts_with("Static symbols") {
            in_symbols = true;
            continue;
        }
        if !in_symbols {
            continue;
        }
        let w: Vec<&str> = t.split_whitespace().collect();
        if w.len() < 3 {
            continue;
        }
        let Some((sec, _)) = w[0].split_once(':') else {
            continue;
        };
        if u32::from_str_radix(sec, 16).is_ok_and(|s| s == 0) {
            continue;
        }
        let (Some(base), Ok(va)) = (base, u64::from_str_radix(w[2], 16)) else {
            continue;
        };
        if let Some(rva) = va.checked_sub(base).and_then(|r| u32::try_from(r).ok()) {
            out.push((rva, w[1].to_owned()));
        }
    }
    out.sort();
    out.dedup_by_key(|(r, _)| *r);
    out
}

/// One image's symbols.
#[derive(Debug, Clone, Default)]
pub struct Image {
    /// `.pdata`: each function's `[begin, end)`, sorted.
    functions: Vec<(u32, u32)>,
    /// Handler RVA to guest function entry (the generated module).
    guest: HashMap<u32, u32>,
    /// Handler RVAs, sorted, for leaf handlers that have no `.pdata`.
    handlers: Vec<u32>,
    /// Map symbols, sorted.
    names: Vec<(u32, String)>,
}

impl Image {
    /// From a PE file's bytes: its functions and, for a generated module,
    /// its guest handlers.
    pub fn from_pe(b: &[u8]) -> Result<Image, String> {
        let pe = Pe::parse(b)?;
        let guest = guest_handlers(&pe, b).unwrap_or_default();
        let mut handlers: Vec<u32> = guest.keys().copied().collect();
        handlers.sort_unstable();
        Ok(Image {
            functions: pe.functions(b),
            guest,
            handlers,
            names: Vec::new(),
        })
    }

    pub fn with_map(mut self, map: &str) -> Image {
        self.names = parse_map(map);
        self
    }

    /// Guest functions found.
    pub fn guest_functions(&self) -> usize {
        self.guest.len()
    }

    fn function(&self, rva: u32) -> Option<(u32, u32)> {
        let i = self.functions.partition_point(|f| f.0 <= rva);
        let f = *self.functions.get(i.checked_sub(1)?)?;
        (rva < f.1).then_some(f)
    }

    /// `rva` as a frame of the image called `module`.
    pub fn resolve(&self, module: &str, rva: u32) -> Frame {
        let f = self.function(rva);
        if let Some((begin, _)) = f
            && let Some(&g) = self.guest.get(&begin)
        {
            return Frame::Guest {
                function: g,
                insn: None,
            };
        }
        if f.is_none() && !self.handlers.is_empty() {
            // A leaf handler (no .pdata): the nearest handler below, when
            // no other function starts in between.
            let i = self.handlers.partition_point(|&h| h <= rva);
            if let Some(&h) = i.checked_sub(1).and_then(|i| self.handlers.get(i)) {
                let j = self.functions.partition_point(|f| f.0 <= rva);
                let between = j
                    .checked_sub(1)
                    .and_then(|j| self.functions.get(j))
                    .is_some_and(|f| f.0 > h);
                if !between {
                    return Frame::Guest {
                        function: self.guest[&h],
                        insn: None,
                    };
                }
            }
        }
        let at = f.map_or(rva, |f| f.0);
        // The nearest symbol at or below, unless it starts before the
        // function the PC is in (then it names another function).
        let i = self.names.partition_point(|n| n.0 <= rva);
        if let Some((s, name)) = i.checked_sub(1).and_then(|i| self.names.get(i))
            && f.is_none_or(|f| *s >= f.0)
        {
            return Frame::Host {
                name: demangle(name),
            };
        }
        Frame::Host {
            name: format!("{module}+{at:#x}"),
        }
    }
}

fn demangle(raw: &str) -> String {
    match rustc_demangle::try_demangle(raw) {
        Ok(d) => format!("{d:#}"),
        Err(_) => raw.to_owned(),
    }
}

/// Images by lower-case file name.
#[derive(Debug, Clone, Default)]
pub struct Symbols {
    images: HashMap<String, Image>,
}

impl Symbols {
    pub fn add(&mut self, file_name: &str, image: Image) {
        self.images.insert(file_name.to_ascii_lowercase(), image);
    }

    pub fn image(&self, file_name: &str) -> Option<&Image> {
        self.images.get(&file_name.to_ascii_lowercase())
    }

    /// A dump frame; `call` for return addresses (looked up one byte back,
    /// at the call).
    pub fn frame(&self, dump: &Dump, pc: Pc, call: bool) -> Frame {
        match pc {
            Pc::Abs(a) => Frame::Unknown { pc: a },
            Pc::In { module, offset } => {
                let m = &dump.modules[module];
                let offset = if call {
                    offset.saturating_sub(1)
                } else {
                    offset
                };
                match (self.image(&m.name), u32::try_from(offset)) {
                    (Some(img), Ok(rva)) => img.resolve(&m.name, rva),
                    _ => Frame::Host {
                        name: format!("{}+{offset:#x}", m.name),
                    },
                }
            }
        }
    }

    /// The dump as a [`Profile`]: one stack per sample, charged as
    /// `profile::resolve` charges them (the innermost frame with a
    /// category).
    pub fn resolve(&self, dump: &Dump) -> Profile {
        let mut stacks = Vec::new();
        for s in &dump.stacks {
            let frames: Vec<Frame> = s
                .frames
                .iter()
                .enumerate()
                .map(|(i, &pc)| self.frame(dump, pc, i > 0))
                .collect();
            let (self_index, category) = frames
                .iter()
                .enumerate()
                .find_map(|(i, f)| f.category().map(|c| (i, c)))
                .unwrap_or((0, Category::Other));
            for _ in 0..s.count {
                stacks.push(Stack {
                    thread: s.thread,
                    on_cpu: s.on_cpu,
                    frames: frames.clone(),
                    self_index,
                    category,
                });
            }
        }
        Profile {
            threads: dump.threads.clone(),
            stacks,
            wall: dump.wall,
            hz: dump.hz,
            missed: dump.missed,
            mean_pause_ns: dump.mean_pause_ns,
        }
    }
}

/// Guest function names from a Ghidra export (`disc/ghidra-functions.tsv`:
/// entry, size, min, max, source, name), leaving out Ghidra's
/// placeholders (`FUN_...`), which only repeat the address.
pub fn guest_names(tsv: &str) -> HashMap<u32, String> {
    tsv.lines()
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            let entry = u32::from_str_radix(c.first()?.trim_start_matches("0x"), 16).ok()?;
            let name = c.get(5)?.trim();
            (!name.is_empty() && !name.starts_with("FUN_")).then(|| (entry, name.to_owned()))
        })
        .collect()
}

/// `guest 0x002d21c6` labels with the guest function's name appended.
pub fn name_guest(label: &str, names: &HashMap<u32, String>) -> String {
    let Some(hex) = label.strip_prefix("guest 0x") else {
        return label.to_owned();
    };
    let digits: String = hex.chars().take_while(char::is_ascii_hexdigit).collect();
    match u32::from_str_radix(&digits, 16)
        .ok()
        .and_then(|a| names.get(&a))
    {
        Some(n) => format!("{label} {n}"),
        None => label.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::RawSample;

    fn raw() -> RawProfile {
        let s = |frames: &[u64]| RawSample {
            thread: 0,
            at: Duration::ZERO,
            on_cpu: true,
            frames: frames.to_vec(),
            lr: 0,
        };
        RawProfile {
            threads: vec!["game".into()],
            samples: vec![
                s(&[0x1_0000_1010, 0x2_0000_0500]),
                s(&[0x1_0000_1010, 0x2_0000_0500]),
                s(&[0x1_0000_2000, 0x9999]),
            ],
            wall: Duration::from_millis(3),
            hz: 1000,
            missed: 1,
            mean_pause_ns: 20_000,
        }
    }

    fn locate(pc: u64) -> Option<Module> {
        match pc >> 32 {
            1 => Some(Module {
                name: "h2game.dll".into(),
                base: 0x1_0000_0000,
                size: 0x10_0000,
            }),
            2 => Some(Module {
                name: "h2uwp exe".into(),
                base: 0x2_0000_0000,
                size: 0x1000,
            }),
            _ => None,
        }
    }

    #[test]
    fn dumps_count_stacks_and_round_trip() {
        let mut asked = 0;
        let d = Dump::from_raw(&raw(), |pc| {
            asked += 1;
            locate(pc)
        });
        // Each module is looked up once; the stray address every time.
        assert_eq!(asked, 3);
        assert_eq!(d.samples(), 3);
        assert_eq!(d.stacks[0].count, 2);
        assert_eq!(
            d.stacks[0].frames,
            [
                Pc::In {
                    module: 0,
                    offset: 0x1010
                },
                Pc::In {
                    module: 1,
                    offset: 0x500
                }
            ]
        );
        assert_eq!(d.stacks[1].frames[1], Pc::Abs(0x9999));
        let text = d.write();
        assert!(text.starts_with("h2prof 1\nhz 1000\n"), "{text}");
        assert!(
            text.contains("module 1 h2uwp_exe 0x200000000 0x1000\n"),
            "{text}"
        );
        assert!(text.contains("stack 2 0 1 0:1010 1:500\n"), "{text}");
        let back = Dump::parse(&text).unwrap();
        assert_eq!(back.stacks, d.stacks);
        assert_eq!(back.modules[0], d.modules[0]);
        assert_eq!(
            (back.hz, back.missed, back.wall),
            (1000, 1, Duration::from_millis(3))
        );
        assert!(Dump::parse("hello").is_err());
        assert!(
            Dump::parse("h2prof 1\nstack 1 0 1 4:10").is_err(),
            "no such module"
        );
    }

    /// A minimal PE32+ image: one section holding an export directory
    /// for `h2gen_module`, the module table, two entries and `.pdata`.
    fn module_image() -> Vec<u8> {
        const BASE: u64 = 0x1_8000_0000;
        const SEC_RVA: u32 = 0x1000;
        const SEC_RAW: u32 = 0x400;
        let mut b = vec![0u8; 0x2000];
        let w16 = |b: &mut Vec<u8>, o: usize, v: u16| b[o..o + 2].copy_from_slice(&v.to_le_bytes());
        let w32 = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let w64 = |b: &mut Vec<u8>, o: usize, v: u64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
        b[0..2].copy_from_slice(b"MZ");
        w32(&mut b, 0x3c, 0x80);
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        let coff = 0x84;
        w16(&mut b, coff + 2, 1); // sections
        w16(&mut b, coff + 16, 240); // optional header size
        let opt = coff + 20;
        w16(&mut b, opt, 0x20b);
        w64(&mut b, opt + 24, BASE);
        let dirs = opt + 112;
        // In-section layout (RVAs): exports 0x1000, names 0x1100, table
        // 0x1200, entries 0x1300, pdata 0x1400.
        w32(&mut b, dirs, 0x1000);
        w32(&mut b, dirs + 4, 0x100);
        w32(&mut b, dirs + 24, 0x1400);
        w32(&mut b, dirs + 28, 36);
        let sec = opt + 240;
        w32(&mut b, sec + 8, 0x1000); // vsize
        w32(&mut b, sec + 12, SEC_RVA);
        w32(&mut b, sec + 16, 0x1000); // raw size
        w32(&mut b, sec + 20, SEC_RAW);
        let at = |rva: u32| (rva - SEC_RVA + SEC_RAW) as usize;
        // Export directory: one name.
        let e = at(0x1000);
        w32(&mut b, e + 24, 1);
        w32(&mut b, e + 28, 0x1080); // functions
        w32(&mut b, e + 32, 0x1090); // names
        w32(&mut b, e + 36, 0x10a0); // ordinals
        w32(&mut b, at(0x1080), 0x1200);
        w32(&mut b, at(0x1090), 0x1100);
        w16(&mut b, at(0x10a0), 0);
        b[at(0x1100)..at(0x1100) + 13].copy_from_slice(b"h2gen_module\0");
        // The module table: abi, count 3, resumes 0, features, entries.
        let t = at(0x1200);
        w32(&mut b, t + 4, 3);
        w64(&mut b, t + 16, BASE + 0x1300);
        let row = |b: &mut Vec<u8>, i: usize, addr: u32, kind: u32, handler: u64| {
            let r = at(0x1300) + 24 * i;
            w32(b, r, addr);
            w32(b, r + 4, kind);
            w64(b, r + 8, handler);
        };
        row(&mut b, 0, 0x0001_2000, 0, BASE + 0x5000); // guest fn at 0x12000
        row(&mut b, 1, 0x0001_2040, 1, BASE + 0x5000); // an interior label
        row(&mut b, 2, 0x0003_0000, 0, BASE + 0x6000); // a leaf handler
        // .pdata: the first handler [0x5000, 0x5100), a helper at 0x7000.
        let p = at(0x1400);
        for (i, (s, e)) in [(0x5000u32, 0x5100u32), (0x7000, 0x7040), (0x8000, 0x8010)]
            .into_iter()
            .enumerate()
        {
            w32(&mut b, p + 12 * i, s);
            w32(&mut b, p + 12 * i + 4, e);
        }
        b
    }

    #[test]
    fn the_module_table_names_each_handlers_guest_function() {
        let b = module_image();
        let pe = Pe::parse(&b).unwrap();
        assert_eq!(pe.image_base, 0x1_8000_0000);
        assert_eq!(pe.export(&b, "h2gen_module"), Some(0x1200));
        assert_eq!(pe.export(&b, "nope"), None);
        let g = guest_handlers(&pe, &b).unwrap();
        assert_eq!(
            g.get(&0x5000),
            Some(&0x12000),
            "kind-1 rows don't rename it"
        );
        assert_eq!(g.get(&0x6000), Some(&0x30000));
        let img = Image::from_pe(&b).unwrap();
        assert_eq!(img.guest_functions(), 2);
        let guest = |f| Frame::Guest {
            function: f,
            insn: None,
        };
        assert_eq!(img.resolve("h2game.dll", 0x5080), guest(0x12000));
        // No .pdata there: the leaf handler below it.
        assert_eq!(img.resolve("h2game.dll", 0x6010), guest(0x30000));
        // A function that is no handler (the CRT, a helper).
        assert_eq!(
            img.resolve("h2game.dll", 0x7008),
            Frame::Host {
                name: "h2game.dll+0x7000".into()
            }
        );
        // Past a non-handler function, a gap is not the handler below.
        assert_eq!(
            img.resolve("h2game.dll", 0x7800),
            Frame::Host {
                name: "h2game.dll+0x7800".into()
            }
        );
        assert!(Pe::parse(b"MZ").is_err());
        assert!(Pe::parse(b"nope").is_err());
    }

    #[test]
    fn linker_maps_give_demangled_names() {
        let map = "\
 h2uwp

 Timestamp is 00000000 (Thu Jan  1 00:00:00 1970)

 Preferred load address is 0000000140000000

 Start         Length     Name                   Class
 0001:00000000 00001000H .text                   CODE

  Address         Publics by Value              Rva+Base               Lib:Object

 0000:00000000       __guard_flags              0000000000000000     <absolute>
 0001:00000000       _ZN6kernel5sched4tick17h0123456789abcdefE 0000000140001000 f   kernel.o
 0001:00000100       memcpy                     0000000140001100 f   libvcruntime.lib:memcpy.obj

 Static symbols

 0001:00000200       _ZN7runtime4exec3run17hfedcba9876543210E 0000000140001200 f   runtime.o
";
        let names = parse_map(map);
        assert_eq!(names.len(), 3, "{names:?}");
        assert_eq!(names[0].0, 0x1000);
        let img = Image::default().with_map(map);
        assert_eq!(
            img.resolve("h2uwp.exe", 0x1010),
            Frame::Host {
                name: "kernel::sched::tick".into()
            }
        );
        assert_eq!(
            img.resolve("h2uwp.exe", 0x1104),
            Frame::Host {
                name: "memcpy".into()
            }
        );
        assert_eq!(
            img.resolve("h2uwp.exe", 0x1300),
            Frame::Host {
                name: "runtime::exec::run".into()
            }
        );
        assert_eq!(
            img.resolve("h2uwp.exe", 0x1300).category(),
            Some(Category::Runtime)
        );
    }

    #[test]
    fn a_dump_resolves_into_a_profile_charged_by_category() {
        let b = module_image();
        let mut syms = Symbols::default();
        syms.add("H2GAME.DLL", Image::from_pe(&b).unwrap());
        let d = Dump {
            hz: 1000,
            threads: vec!["game".into()],
            modules: vec![
                Module {
                    name: "h2game.dll".into(),
                    base: 0,
                    size: 0x10000,
                },
                Module {
                    name: "ntdll.dll".into(),
                    base: 0,
                    size: 0x10000,
                },
            ],
            stacks: vec![
                DumpStack {
                    count: 3,
                    thread: 0,
                    on_cpu: true,
                    frames: vec![
                        Pc::In {
                            module: 1,
                            offset: 0x44,
                        },
                        // A return address just past a call at the
                        // handler's last byte still resolves inside it.
                        Pc::In {
                            module: 0,
                            offset: 0x5100,
                        },
                    ],
                },
                DumpStack {
                    count: 1,
                    thread: 0,
                    on_cpu: false,
                    frames: vec![Pc::Abs(0x77)],
                },
            ],
            ..Dump::default()
        };
        let p = syms.resolve(&d);
        assert_eq!(p.stacks.len(), 4);
        assert_eq!(
            p.stacks[0].frames[0],
            Frame::Host {
                name: "ntdll.dll+0x44".into()
            }
        );
        assert_eq!(
            p.stacks[0].frames[1],
            Frame::Guest {
                function: 0x12000,
                insn: None
            }
        );
        assert_eq!(p.stacks[0].category, Category::Guest);
        assert_eq!(p.stacks[3].category, Category::Other);
        let names = guest_names(
            "entry\tsize\tmin\tmax\tsource\tname\n00012000\t1\t0\t0\tANALYSIS\tXAPILIB::XMemAlloc\n00030000\t1\t0\t0\tDEFAULT\tFUN_00030000\n",
        );
        assert_eq!(names.len(), 1);
        assert_eq!(
            name_guest("guest 0x00012000", &names),
            "guest 0x00012000 XAPILIB::XMemAlloc"
        );
        assert_eq!(name_guest("guest 0x00030000", &names), "guest 0x00030000");
        assert_eq!(name_guest("memcpy", &names), "memcpy");
    }
}
