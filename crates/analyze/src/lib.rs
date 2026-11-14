//! Function discovery and coverage over an XBE's code sections.
//!
//! Recursive descent from the entry point and direct call targets, plus
//! jump tables, pointers found in data sections, and a scan of what's left
//! for plausible function starts. The result says, per byte of code, whether
//! it was decoded as an instruction, recognised as data or padding, or is
//! still unknown. Shrinking "unknown" is the job of milestone 2.

pub mod compare;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, Instruction, OpKind, Register};
use xbe::{Section, Xbe};

/// Sections that carry the executable flag on Xbox but hold data.
const DATA_SECTIONS: &[&str] = &[
    ".rdata", ".data", ".data1", "DOLBY", "XON_RD", "BINKDATA", "$$XTIMAGE", "$$XSIMAGE",
    "DSPImage", ".XTLID",
];

/// Longest straight-line run a trial decode may take before giving up.
const TRIAL_LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Entry,
    Call,
    DataPointer,
    GapScan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Byte {
    Unknown,
    InsnStart,
    InsnBody,
    Data,
}

struct CodeRegion<'a> {
    name: String,
    va: u32,
    bytes: &'a [u8],
    state: Vec<Byte>,
}

impl CodeRegion<'_> {
    fn contains(&self, va: u32) -> bool {
        va >= self.va && ((va - self.va) as usize) < self.bytes.len()
    }
}

#[derive(Debug, Clone)]
pub struct Function {
    pub start: u32,
    pub source: Source,
    pub section: String,
}

#[derive(Debug, Clone, Default)]
pub struct SectionCoverage {
    pub name: String,
    pub size: usize,
    pub code: usize,
    pub data: usize,
    pub padding: usize,
    pub unknown: usize,
    pub functions: usize,
}

impl SectionCoverage {
    pub fn percent(&self) -> f64 {
        if self.size == 0 {
            return 100.0;
        }
        100.0 * (self.size - self.unknown) as f64 / self.size as f64
    }
}

pub struct Analysis {
    pub functions: BTreeMap<u32, Function>,
    pub sections: Vec<SectionCoverage>,
    pub jump_tables: usize,
    /// Address of a bad decode, or two decodes that disagree about where an
    /// instruction starts.
    pub conflicts: Vec<u32>,
    /// Kernel ordinal -> number of call sites.
    pub kernel_calls: BTreeMap<u32, usize>,
    pub kernel_imports: usize,
    /// Largest unknown runs: (va, len).
    pub gaps: Vec<(u32, usize)>,
    /// Code sections: (va, len, name).
    pub ranges: Vec<(u32, u32, String)>,
}

struct Analyzer<'a> {
    xbe: &'a Xbe<'a>,
    regions: Vec<CodeRegion<'a>>,
    functions: BTreeMap<u32, Function>,
    worklist: Vec<u32>,
    jump_tables: usize,
    conflicts: BTreeSet<u32>,
    thunk_slots: HashMap<u32, u32>,
    kernel_calls: BTreeMap<u32, usize>,
}

pub fn is_code_section(s: &Section) -> bool {
    s.flags.executable() && !DATA_SECTIONS.contains(&s.name.as_str())
}

pub fn analyze(xbe: &Xbe) -> Result<Analysis, xbe::Error> {
    let mut regions = Vec::new();
    for s in xbe.sections.iter().filter(|s| is_code_section(s)) {
        let bytes = xbe.section_bytes(s)?;
        let bytes = &bytes[..bytes.len().min(s.vsize as usize)];
        regions.push(CodeRegion {
            name: s.name.clone(),
            va: s.va,
            bytes,
            state: vec![Byte::Unknown; bytes.len()],
        });
    }
    let imports = xbe.kernel_imports()?;
    let mut a = Analyzer {
        xbe,
        regions,
        functions: BTreeMap::new(),
        worklist: Vec::new(),
        jump_tables: 0,
        conflicts: BTreeSet::new(),
        thunk_slots: imports.iter().map(|k| (k.slot_va, k.ordinal)).collect(),
        kernel_calls: BTreeMap::new(),
    };

    a.add_function(xbe.entry, Source::Entry);
    a.drain();
    a.scan_data_pointers()?;
    a.drain();
    // Each round of gap scanning can expose more call targets and gaps.
    while a.scan_gaps() > 0 {
        a.drain();
    }
    Ok(a.finish(imports.len()))
}

impl<'a> Analyzer<'a> {
    fn region(&self, va: u32) -> Option<usize> {
        self.regions.iter().position(|r| r.contains(va))
    }

    fn state(&self, va: u32) -> Option<Byte> {
        let r = &self.regions[self.region(va)?];
        Some(r.state[(va - r.va) as usize])
    }

    fn add_function(&mut self, va: u32, source: Source) {
        let Some(ri) = self.region(va) else { return };
        if self.functions.contains_key(&va) {
            return;
        }
        if self.state(va) == Some(Byte::InsnBody) || self.state(va) == Some(Byte::Data) {
            self.conflicts.insert(va);
            return;
        }
        let section = self.regions[ri].name.clone();
        self.functions.insert(va, Function { start: va, source, section });
        self.worklist.push(va);
    }

    fn drain(&mut self) {
        while let Some(va) = self.worklist.pop() {
            self.decode_block(va);
        }
    }

    /// Decode straight-line code from `va`, queueing branch targets.
    fn decode_block(&mut self, mut va: u32) {
        let Some(ri) = self.region(va) else { return };
        loop {
            let off = (va - self.regions[ri].va) as usize;
            match self.regions[ri].state.get(off) {
                None => return,
                Some(Byte::InsnStart) => return, // already decoded from here
                Some(Byte::InsnBody | Byte::Data) => {
                    self.conflicts.insert(va);
                    return;
                }
                Some(Byte::Unknown) => {}
            }
            let r = &self.regions[ri];
            let mut dec = Decoder::with_ip(32, &r.bytes[off..], va as u64, DecoderOptions::NONE);
            let insn = dec.decode();
            if insn.code() == Code::INVALID {
                self.conflicts.insert(va);
                return;
            }
            let len = insn.len();
            let r = &mut self.regions[ri];
            if r.state[off..off + len].iter().any(|b| *b != Byte::Unknown) {
                self.conflicts.insert(va);
                return;
            }
            r.state[off] = Byte::InsnStart;
            r.state[off + 1..off + len].fill(Byte::InsnBody);

            match insn.flow_control() {
                FlowControl::Next => {}
                FlowControl::ConditionalBranch => {
                    self.worklist.push(insn.near_branch32());
                }
                FlowControl::UnconditionalBranch => {
                    if insn.op0_kind() == OpKind::NearBranch32 {
                        self.worklist.push(insn.near_branch32());
                    }
                    return;
                }
                FlowControl::Call => {
                    self.add_function(insn.near_branch32(), Source::Call);
                }
                FlowControl::IndirectCall => {
                    self.note_kernel_call(&insn);
                }
                FlowControl::IndirectBranch => {
                    self.note_kernel_call(&insn);
                    self.jump_table(&insn, ri);
                    return;
                }
                FlowControl::Return
                | FlowControl::Interrupt
                | FlowControl::Exception
                | FlowControl::XbeginXabortXend => return,
            }
            va += len as u32;
        }
    }

    fn note_kernel_call(&mut self, insn: &Instruction) {
        if insn.op0_kind() == OpKind::Memory
            && insn.memory_base() == Register::None
            && insn.memory_index() == Register::None
            && let Some(&ord) = self.thunk_slots.get(&insn.memory_displacement32())
        {
            *self.kernel_calls.entry(ord).or_default() += 1;
        }
    }

    /// MSVC switch: `jmp dword ptr [reg*4 + table]`. The table usually sits
    /// in `.text` right after the function, so its entries become data.
    fn jump_table(&mut self, insn: &Instruction, ri: usize) {
        if insn.op0_kind() != OpKind::Memory
            || insn.memory_base() != Register::None
            || insn.memory_index() == Register::None
            || insn.memory_index_scale() != 4
        {
            return;
        }
        let table = insn.memory_displacement32();
        let mut found = false;
        for i in 0..1024u32 {
            let slot = table + i * 4;
            // Stop when the table runs into decoded code.
            if let Some(si) = self.region(slot) {
                let r = &self.regions[si];
                let o = (slot - r.va) as usize;
                if o + 4 > r.state.len() || r.state[o..o + 4].iter().any(|b| *b != Byte::Unknown) {
                    break;
                }
            }
            let Ok(target) = self.xbe.read_u32(slot) else { break };
            if self.region(target) != Some(ri) {
                break;
            }
            if let Some(si) = self.region(slot) {
                let r = &mut self.regions[si];
                let o = (slot - r.va) as usize;
                r.state[o..o + 4].fill(Byte::Data);
            }
            self.worklist.push(target);
            found = true;
        }
        if found {
            self.jump_tables += 1;
        }
    }

    /// Aligned words in data sections that point at a clean function start.
    fn scan_data_pointers(&mut self) -> Result<(), xbe::Error> {
        let xbe = self.xbe;
        let mut candidates = Vec::new();
        for s in xbe.sections.iter().filter(|s| !is_code_section(s)) {
            let bytes = xbe.section_bytes(s)?;
            for (i, w) in bytes.chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes(w.try_into().unwrap());
                if self.region(v).is_some() {
                    candidates.push((s.va + i as u32 * 4, v));
                }
            }
        }
        for (_, target) in candidates {
            if matches!(self.state(target), Some(Byte::Unknown)) && self.trial(target) {
                self.add_function(target, Source::DataPointer);
            }
        }
        Ok(())
    }

    /// Unknown runs, after skipping padding, that decode cleanly to a
    /// terminator get treated as functions. Returns how many were added.
    fn scan_gaps(&mut self) -> usize {
        let mut starts = Vec::new();
        for r in &self.regions {
            let mut i = 0;
            while i < r.state.len() {
                if r.state[i] != Byte::Unknown {
                    i += 1;
                    continue;
                }
                let gap_start = i;
                while i < r.state.len() && r.state[i] == Byte::Unknown {
                    i += 1;
                }
                let mut s = gap_start;
                while s < i && is_padding(r.bytes[s]) {
                    s += 1;
                }
                if s < i {
                    starts.push(r.va + s as u32);
                }
            }
        }
        let mut added = 0;
        for va in starts {
            if self.trial(va) {
                self.add_function(va, Source::GapScan);
                added += 1;
            }
        }
        added
    }

    /// Linear decode from `va` must reach a terminator without an invalid
    /// opcode or running into bytes already claimed.
    fn trial(&self, va: u32) -> bool {
        let Some(ri) = self.region(va) else { return false };
        let r = &self.regions[ri];
        let off = (va - r.va) as usize;
        let mut dec = Decoder::with_ip(32, &r.bytes[off..], va as u64, DecoderOptions::NONE);
        let mut pos = off;
        for _ in 0..TRIAL_LIMIT {
            if !dec.can_decode() {
                return false;
            }
            let insn = dec.decode();
            if insn.code() == Code::INVALID {
                return false;
            }
            let end = pos + insn.len();
            if r.state[pos..end].iter().any(|b| *b != Byte::Unknown) {
                // Falling into already-decoded code at an instruction start
                // is fine (shared tail); anything else is not.
                return r.state[pos] == Byte::InsnStart && pos > off;
            }
            match insn.flow_control() {
                FlowControl::Return | FlowControl::UnconditionalBranch | FlowControl::IndirectBranch => {
                    return true;
                }
                FlowControl::Interrupt | FlowControl::Exception | FlowControl::XbeginXabortXend => {
                    return false;
                }
                _ => {}
            }
            pos = end;
        }
        false
    }

    fn finish(self, kernel_imports: usize) -> Analysis {
        let mut sections = Vec::new();
        let mut gaps = Vec::new();
        for r in &self.regions {
            let mut c = SectionCoverage { name: r.name.clone(), size: r.bytes.len(), ..Default::default() };
            let mut i = 0;
            while i < r.state.len() {
                match r.state[i] {
                    Byte::InsnStart | Byte::InsnBody => c.code += 1,
                    Byte::Data => c.data += 1,
                    Byte::Unknown => {
                        let start = i;
                        while i < r.state.len() && r.state[i] == Byte::Unknown {
                            i += 1;
                        }
                        let run = &r.bytes[start..i];
                        let pad = run.iter().filter(|b| is_padding(**b)).count();
                        if pad == run.len() {
                            c.padding += pad;
                        } else {
                            c.unknown += run.len();
                            gaps.push((r.va + start as u32, run.len()));
                        }
                        continue;
                    }
                }
                i += 1;
            }
            c.functions = self.functions.range(r.va..r.va + r.bytes.len() as u32).count();
            sections.push(c);
        }
        gaps.sort_by(|a, b| b.1.cmp(&a.1));
        gaps.truncate(20);
        Analysis {
            functions: self.functions,
            sections,
            jump_tables: self.jump_tables,
            conflicts: self.conflicts.into_iter().collect(),
            kernel_calls: self.kernel_calls,
            kernel_imports,
            gaps,
            ranges: self.regions.iter().map(|r| (r.va, r.bytes.len() as u32, r.name.clone())).collect(),
        }
    }
}

fn is_padding(b: u8) -> bool {
    matches!(b, 0xCC | 0x90 | 0x00)
}

impl Analysis {
    /// Name of the code section containing `va`.
    pub fn section_of(&self, va: u32) -> Option<&str> {
        self.ranges.iter().find(|(s, len, _)| va >= *s && va - *s < *len).map(|(_, _, n)| n.as_str())
    }

    pub fn total(&self) -> SectionCoverage {
        let mut t = SectionCoverage { name: "total".into(), ..Default::default() };
        for s in &self.sections {
            t.size += s.size;
            t.code += s.code;
            t.data += s.data;
            t.padding += s.padding;
            t.unknown += s.unknown;
            t.functions += s.functions;
        }
        t
    }
}

impl fmt::Display for Analysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:<10} {:>9} {:>9} {:>8} {:>8} {:>9} {:>7} {:>7}",
            "section", "size", "code", "data", "pad", "unknown", "funcs", "known"
        )?;
        for s in self.sections.iter().chain(std::iter::once(&self.total())) {
            writeln!(
                f,
                "{:<10} {:>9} {:>9} {:>8} {:>8} {:>9} {:>7} {:>6.2}%",
                s.name, s.size, s.code, s.data, s.padding, s.unknown, s.functions, s.percent()
            )?;
        }
        let mut by_source: BTreeMap<Source, usize> = BTreeMap::new();
        for func in self.functions.values() {
            *by_source.entry(func.source).or_default() += 1;
        }
        writeln!(f)?;
        writeln!(f, "functions by source: {by_source:?}")?;
        writeln!(f, "jump tables: {}", self.jump_tables)?;
        writeln!(f, "conflicts: {}", self.conflicts.len())?;
        writeln!(
            f,
            "kernel: {} imports, {} called directly from {} sites",
            self.kernel_imports,
            self.kernel_calls.len(),
            self.kernel_calls.values().sum::<usize>()
        )?;
        writeln!(f, "largest unknown runs:")?;
        for (va, len) in &self.gaps {
            writeln!(f, "  {va:#010x} {len:>7} bytes")?;
        }
        Ok(())
    }
}
