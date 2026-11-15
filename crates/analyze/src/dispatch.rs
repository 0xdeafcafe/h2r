//! Static indirect-dispatch inventory. All table contents come from the supplied
//! image. Candidate pointer runs are evidence, not a proof of vtable identity.
use crate::{
    Analysis,
    cfg::{CallTarget, CfgBuilder},
    is_code_section,
};
use iced_x86::{FlowControl, Instruction, OpKind};
use std::collections::{BTreeMap, BTreeSet};
use xbe::Xbe;

pub const CAMPAIGN_ROOTS: [u32; 6] = [0x137ca0, 0x11c1b0, 0x209310, 0xb7b40, 0x1c83c0, 0x311100];

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub base: u32,
    /// Address ranges whose references select this table (descriptor arrays
    /// include their pointer array and ancestor descriptors).
    pub spans: Vec<(u32, u32)>,
    /// (slot address, target), including nulls for known layouts.
    pub slots: Vec<(u32, u32)>,
    pub known_layout: bool,
}

fn code(xbe: &Xbe, va: u32) -> bool {
    xbe.section_containing(va).is_some_and(is_code_section)
}

/// Contiguous aligned runs of at least two executable pointers. Nulls break
/// runs: without a schema they cannot establish a table's extent. Known retail
/// layouts below retain their null slots and use exact bounds instead.
pub fn tables(xbe: &Xbe) -> Result<Vec<Table>, xbe::Error> {
    let mut out = Vec::new();
    if xbe.certificate.title_id == 0x4d530064 && xbe.entry == 0x2d0aee {
        // Layout evidence: docs/engine-map.md and docs/haloscript.md. Fail
        // closed as a unit if any required pointer/span is invalid.
        if let Some(known) = retail_tables(xbe) {
            out.extend(known);
        }
    }
    let occupied: BTreeSet<u32> = out
        .iter()
        .flat_map(|t| t.spans.iter())
        .flat_map(|&(base, len)| (0..len).step_by(4).map(move |offset| (base + offset) & !3))
        .collect();
    for section in xbe.sections.iter().filter(|s| !is_code_section(s)) {
        let bytes = xbe.section_bytes(section)?;
        let bytes = &bytes[..bytes.len().min(section.vsize as usize)];
        let mut run = Vec::new();
        let flush = |run: &mut Vec<(u32, u32)>, out: &mut Vec<Table>| {
            if run.len() >= 2
                && !out.iter().any(|t| {
                    run.iter()
                        .all(|(slot, _)| t.slots.iter().any(|(s, _)| s == slot))
                })
            {
                let base = run[0].0;
                out.push(Table {
                    name: format!("pointer-run-{base:08x}"),
                    base,
                    spans: vec![(base, run.len() as u32 * 4)],
                    slots: run.clone(),
                    known_layout: false,
                });
            }
            run.clear();
        };
        for offset in (0..bytes.len().saturating_sub(3)).step_by(4) {
            let slot = section.va + offset as u32;
            let target = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            if !occupied.contains(&slot) && code(xbe, target) {
                run.push((slot, target));
            } else {
                flush(&mut run, &mut out);
            }
        }
        flush(&mut run, &mut out);
    }
    out.sort_by_key(|t| t.base);
    Ok(out)
}

/// Tables with dispatch evidence. Heap receiver provenance and the end of the
/// final adjacent vtable remain conservative; this does not resolve every site.
pub fn catalog(xbe: &Xbe, analysis: &Analysis) -> Result<Vec<Table>, xbe::Error> {
    let code = crate::cfg::CodeView::new(xbe);
    let mut anchors = BTreeSet::new();
    for (base, states, _) in &analysis.claims {
        for (off, state) in states.iter().enumerate() {
            if *state == crate::image::Byte::InsnStart
                && let Some(insn) = code.decode(*base + off as u32)
            {
                anchors.extend(crate::pointer_seeds::dispatch_anchors(&insn));
            }
        }
    }
    Ok(crate::pointer_seeds::select_tables(
        xbe,
        tables(xbe)?,
        anchors,
    ))
}

fn retail_tables(xbe: &Xbe) -> Option<Vec<Table>> {
    let read = |va| xbe.read_u32(va).ok();
    let mut out = Vec::new();
    // Nine columns in 68 records. Each column stays separate in the miss list.
    for column in 0..9 {
        let base = 0x440dd8 + column * 4;
        let slots: Vec<_> = (0..68)
            .map(|i| {
                let slot = base + i * 36;
                Some((slot, read(slot)?))
            })
            .collect::<Option<_>>()?;
        if slots.iter().any(|&(_, t)| t != 0 && !code(xbe, t)) {
            return None;
        }
        out.push(Table {
            name: format!("lifecycle-column-{column}"),
            base,
            spans: slots.iter().map(|&(s, _)| (s, 4)).collect(),
            slots,
            known_layout: true,
        });
    }
    if read(0x44f6bc)? != 911 {
        return None;
    }
    let mut hs = Table {
        name: "haloscript-evaluate".into(),
        base: 0x4744e0,
        spans: vec![(0x4744e0, 911 * 4)],
        slots: Vec::new(),
        known_layout: true,
    };
    for i in 0..911 {
        let descriptor = read(hs.base + i * 4)?;
        let header = xbe.bytes_at(descriptor, 14).ok()?;
        let ty = u16::from_le_bytes([header[0], header[1]]);
        let count = u16::from_le_bytes([header[12], header[13]]) as usize;
        if ty >= 62 || count > 64 {
            return None;
        }
        let params = xbe.bytes_at(descriptor.checked_add(14)?, count * 2).ok()?;
        if params
            .as_chunks::<2>()
            .0
            .iter()
            .any(|p| u16::from_le_bytes([p[0], p[1]]) >= 62)
        {
            return None;
        }
        let slot = descriptor.checked_add(4)?;
        let target = read(slot)?;
        if target != 0 && !code(xbe, target) {
            return None;
        }
        hs.slots.push((slot, target));
        hs.spans.push((descriptor, 14 + count as u32 * 2));
    }
    out.push(hs);
    let mut pending: Vec<u32> = (0..13)
        .map(|i| read(0x468630 + i * 4))
        .collect::<Option<_>>()?;
    let mut seen = BTreeSet::new();
    while let Some(descriptor) = pending.pop() {
        if !seen.insert(descriptor) {
            continue;
        }
        if seen.len() > 32 {
            return None;
        }
        xbe.bytes_at(descriptor, 0xc8).ok()?;
        let mut slots = Vec::new();
        for off in (0x10..0x84).step_by(4) {
            let slot = descriptor.checked_add(off)?;
            let target = read(slot)?;
            if target != 0 && !code(xbe, target) {
                return None;
            }
            slots.push((slot, target));
        }
        let mut terminated = false;
        for off in (0x84..0xc8).step_by(4) {
            let parent = read(descriptor.checked_add(off)?)?;
            if parent == 0 {
                terminated = true;
                break;
            }
            pending.push(parent);
        }
        if !terminated {
            return None;
        }
        out.push(Table {
            name: format!("object-descriptor-{descriptor:08x}"),
            base: descriptor,
            spans: vec![(descriptor, 0xc8), (0x468630, 13 * 4)],
            slots,
            known_layout: true,
        });
    }
    // Additional audited layouts: the indirect caller at 0x2cbae9 indexes
    // records with LEA index*3, then scale 4. Only +0 is a code pointer;
    // the other two words contain packed tuning fields. See coverage docs.
    for (name, base, count, stride) in [
        ("compression-callbacks", 0x44f978, 10, 12),
        ("ui-base-vtable", 0x4576d0, 3, 4),
        ("ui-vtable-004590b8", 0x4590b8, 28, 4),
    ] {
        let slots: Vec<_> = (0..count)
            .map(|i| {
                let slot = base + i * stride;
                Some((slot, read(slot)?))
            })
            .collect::<Option<_>>()?;
        if slots.iter().any(|&(_, target)| !code(xbe, target)) {
            return None;
        }
        out.push(Table {
            name: name.into(),
            base,
            spans: vec![(base, count * stride)],
            slots,
            known_layout: true,
        });
    }
    Some(out)
}

pub(crate) fn references(insn: &Instruction) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..insn.op_count() {
        match insn.op_kind(i) {
            OpKind::Memory => out.push(insn.memory_displacement32()),
            OpKind::Immediate32 => out.push(insn.immediate32()),
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct Site {
    pub owner: u32,
    pub kind: &'static str,
    /// Tables referenced anywhere in the owner; conservative association,
    /// deliberately not a claim of register/heap dataflow resolution.
    pub candidate_tables: BTreeSet<usize>,
}
#[derive(Debug, Default)]
pub struct Coverage {
    pub functions: BTreeSet<u32>,
    pub missing_functions: BTreeSet<u32>,
    pub sites: BTreeMap<u32, Site>,
    pub tables: BTreeSet<usize>,
    pub cfg_errors: Vec<(u32, String)>,
}

/// Conservative closure over direct calls, exiting CFG edges, and every target
/// of a referenced table. Unresolved heap/register dispatch remains in `sites`.
pub fn coverage(xbe: &Xbe, analysis: &Analysis, tables: &[Table], roots: &[u32]) -> Coverage {
    let builder = CfgBuilder::new(analysis, xbe);
    let index = table_index(tables);
    let kernels: BTreeSet<_> = xbe
        .kernel_imports()
        .unwrap_or_default()
        .iter()
        .map(|k| k.slot_va)
        .collect();
    let mut out = Coverage::default();
    let mut pending = roots.to_vec();
    let mut visited = BTreeSet::new();
    while let Some(entry) = pending.pop() {
        if !visited.insert(entry) {
            continue;
        }
        let owner = if analysis.functions.contains_key(&entry) {
            entry
        } else if let Some(('I', Some(owner))) = analysis.claim(entry) {
            owner
        } else {
            out.missing_functions.insert(entry);
            continue;
        };
        if !out.functions.insert(owner) {
            continue;
        }
        let Some(cfg) = builder.build(owner) else {
            out.missing_functions.insert(entry);
            continue;
        };
        out.cfg_errors
            .extend(cfg.errors.iter().map(|e| (owner, format!("{e:?}"))));
        let mut refs = BTreeSet::new();
        let mut sites = Vec::new();
        for block in cfg.blocks.values() {
            for &pc in &block.insns {
                let Some(insn) = builder.code().decode(pc) else {
                    continue;
                };
                refs.extend(references(&insn));
                let kind = match insn.flow_control() {
                    FlowControl::IndirectCall => "call",
                    FlowControl::IndirectBranch => "jump",
                    _ => continue,
                };
                let kind = if analysis.jump_tables.contains_key(&pc) {
                    "switch-jump"
                } else if insn.op0_kind() == OpKind::Memory
                    && insn.memory_base() == iced_x86::Register::None
                    && insn.memory_index() == iced_x86::Register::None
                    && kernels.contains(&insn.memory_displacement32())
                {
                    if kind == "call" {
                        "kernel-call"
                    } else {
                        "kernel-jump"
                    }
                } else {
                    kind
                };
                sites.push((pc, kind));
            }
            pending.extend(
                block
                    .edges
                    .iter()
                    .filter(|e| e.exits)
                    .filter_map(|e| e.target),
            );
        }
        pending.extend(cfg.calls.iter().filter_map(|c| match c.target {
            CallTarget::Direct(t) => Some(t),
            _ => None,
        }));
        let candidates: BTreeSet<_> = refs
            .iter()
            .filter_map(|r| index.get(&(r & !3)))
            .flat_map(|ids| ids.iter().copied())
            .collect();
        for &i in &candidates {
            if out.tables.insert(i) {
                pending.extend(
                    tables[i]
                        .slots
                        .iter()
                        .map(|&(_, t)| t)
                        .filter(|t| code(xbe, *t)),
                );
            }
        }
        for (pc, kind) in sites {
            out.sites.insert(
                pc,
                Site {
                    owner,
                    kind,
                    candidate_tables: candidates.clone(),
                },
            );
        }
    }
    out
}

/// An address-taken label is usable only if a CFG actually exports it. Merely
/// finding an instruction boundary or a return resume does not permit CALL.
pub fn entries(xbe: &Xbe, analysis: &Analysis) -> BTreeSet<u32> {
    let builder = CfgBuilder::new(analysis, xbe);
    let mut out: BTreeSet<_> = analysis.functions.keys().copied().collect();
    for &entry in analysis.functions.keys() {
        if let Some(cfg) = builder.build(entry) {
            out.extend(cfg.interior_entries);
        }
    }
    out
}

/// Index ranges once rather than testing every operand against every table.
pub(crate) fn table_index(tables: &[Table]) -> BTreeMap<u32, BTreeSet<usize>> {
    let mut index: BTreeMap<u32, BTreeSet<usize>> = BTreeMap::new();
    for (i, table) in tables.iter().enumerate() {
        for &(base, len) in &table.spans {
            for offset in (0..len).step_by(4) {
                index.entry((base + offset) & !3).or_default().insert(i);
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put(data: &mut [u8], off: usize, value: u32) {
        data[off..off + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn retail_layout() -> Vec<u8> {
        let mut data = crate::tests::xbe_with(&[0xc3]);
        data.resize(0x41100, 0);
        put(&mut data, 0x128, 0x2d0aee ^ 0xa8fc57ab);
        put(&mut data, 0x208, 0x300000);
        put(&mut data, 0x408, 0x4d530064);
        put(&mut data, 0x158, 0x440000 ^ 0x5b6d40b6);
        put(&mut data, 0x23c, 0x440000);
        put(&mut data, 0x240, 0x40000);
        put(&mut data, 0x248, 0x40000);
        let mut word = |va, value| put(&mut data, (va - 0x440000 + 0x1100) as usize, value);
        word(0x44f6bc, 911);
        for i in 0..10 {
            word(0x44f978 + i * 12, 0x11000);
        }
        for i in 0..3 {
            word(0x4576d0 + i * 4, 0x11000);
        }
        for i in 0..28 {
            word(0x4590b8 + i * 4, 0x11000);
        }
        for i in 0..911 {
            word(0x4744e0 + i * 4, 0x478000);
        }
        word(0x478004, 0x11000);
        for i in 0..13 {
            word(0x468630 + i * 4, 0x478100);
        }
        word(0x478110, 0x11000);
        word(0x478184, 0x478100); // legitimate self entry in ancestry
        data
    }
    #[test]
    fn known_layouts_decode_runtime_slots_and_deduplicate_ancestry() {
        let data = retail_layout();
        let xbe = Xbe::parse(&data).unwrap();
        let ts = tables(&xbe).unwrap();
        let hs = ts.iter().find(|t| t.name == "haloscript-evaluate").unwrap();
        assert_eq!(hs.slots.len(), 911);
        assert_eq!(hs.slots[0], (0x478004, 0x11000));
        assert_eq!(
            ts.iter()
                .filter(|t| t.name.starts_with("object-descriptor"))
                .count(),
            1
        );
        assert_eq!(
            ts.iter()
                .filter(|t| t.name.starts_with("lifecycle"))
                .count(),
            9
        );
    }
    #[test]
    fn strided_layout_does_not_treat_packed_fields_as_callbacks() {
        let mut data = retail_layout();
        put(&mut data, (0x44f97c - 0x440000 + 0x1100) as usize, 0x11040);
        put(&mut data, (0x44f980 - 0x440000 + 0x1100) as usize, 0x11050);
        let xbe = Xbe::parse(&data).unwrap();
        let ts = tables(&xbe).unwrap();
        let table = ts
            .iter()
            .find(|t| t.name == "compression-callbacks")
            .unwrap();
        assert_eq!(table.slots.len(), 10);
        assert!(ts.iter().all(|t| {
            t.slots
                .iter()
                .all(|&(slot, _)| slot != 0x44f97c && slot != 0x44f980)
        }));
    }

    #[test]
    fn malformed_known_layout_is_not_used_as_seed_evidence() {
        for (va, value) in [
            (0x44f6bc, 912),
            (0x478004, 0x12345678),
            (0x478000, 62),
            (0x47800c, 65),
        ] {
            let mut data = retail_layout();
            put(&mut data, (va - 0x440000 + 0x1100) as usize, value);
            let xbe = Xbe::parse(&data).unwrap();
            assert!(tables(&xbe).unwrap().iter().all(|t| !t.known_layout));
        }
    }
    #[test]
    fn no_pointer_runs_cross_sections_or_zero_fill() {
        let mut data = crate::tests::xbe_with(&[0xc3]);
        put(&mut data, 0x11fc, 0x11000);
        let xbe = Xbe::parse(&data).unwrap();
        assert!(tables(&xbe).unwrap().is_empty());
    }
    #[test]
    fn closure_reaches_table_callbacks_and_keeps_unresolved_sites() {
        // mov [ecx], table; call eax; ret. Callback: call ebx; ret.
        let mut code = vec![0xcc; 64];
        code[..9].copy_from_slice(&[0xc7, 1, 0x10, 0x20, 1, 0, 0xff, 0xd0, 0xc3]);
        code[0x20..0x23].copy_from_slice(&[0xff, 0xd3, 0xc3]);
        code[0x30] = 0xc3;
        let mut data = crate::tests::xbe_with(&code);
        put(&mut data, 0x1110, 0x11020);
        put(&mut data, 0x1114, 0x11030);
        let xbe = Xbe::parse(&data).unwrap();
        let a = crate::analyze(&xbe).unwrap();
        let ts = tables(&xbe).unwrap();
        let c = coverage(&xbe, &a, &ts, &[0x11000]);
        assert!(c.functions.contains(&0x11020));
        assert_eq!(c.sites.len(), 2);
        assert!(!c.sites[&0x11006].candidate_tables.is_empty());
        assert!(c.sites[&0x11020].candidate_tables.is_empty());
        assert!(entries(&xbe, &a).contains(&0x11020));
    }
}
