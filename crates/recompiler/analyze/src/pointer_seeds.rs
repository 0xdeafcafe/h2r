//! Seeds supported by table layouts or constructor stores. Kept separate from
//! jump-table discovery: no switch bounds or switch decoding change here.
use crate::{
    Analyzer, Source,
    dispatch::{Table, references, tables},
    image::Byte,
};
use iced_x86::{Code, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use std::collections::BTreeSet;

/// A constructor storing a table address into an object is stronger evidence
/// than an arbitrary integer read from a data section. Stack locals excluded.
pub(crate) fn stored_table(insn: &Instruction) -> Option<u32> {
    (insn.code() == Code::Mov_rm32_imm32
        && insn.op0_kind() == OpKind::Memory
        && !matches!(
            insn.memory_base(),
            Register::None | Register::ESP | Register::EBP
        )
        && insn.memory_index() == Register::None
        && insn.memory_displacement32() < 0x10000)
        .then(|| insn.immediate32())
}

/// Some retail virtual callbacks are deliberately trapping destructors:
/// install this object's vtable, then INT3. Speculative discovery correctly
/// rejects unreferenced trap fallthrough; a pointer-table slot plus a store
/// back into that same run makes this particular short body explicit code.
pub(crate) fn trap_callback(view: &crate::cfg::CodeView<'_>, target: u32, table: &Table) -> bool {
    let Some(insn) = view.decode(target) else {
        return false;
    };
    let Some(vtable) = stored_table(&insn) else {
        return false;
    };
    table
        .spans
        .iter()
        .any(|&(base, len)| vtable >= base && vtable - base < len)
        && view
            .decode(insn.next_ip32())
            .is_some_and(|i| i.mnemonic() == Mnemonic::Int3)
}

pub(crate) fn seed(a: &mut Analyzer<'_>) -> Result<(), xbe::Error> {
    loop {
        let before = (a.funcs.len(), a.labels.len());
        seed_pass(a)?;
        a.code_refs_to_fixpoint();
        if before == (a.funcs.len(), a.labels.len()) {
            return Ok(());
        }
    }
}

fn seed_pass(a: &mut Analyzer<'_>) -> Result<(), xbe::Error> {
    let tables = tables(a.xbe)?;
    let mut anchors = BTreeSet::new();
    for r in &a.image.regions {
        for (off, state) in r.state.iter().enumerate() {
            if *state != Byte::InsnStart {
                continue;
            }
            let insn = Decoder::with_ip(
                32,
                &r.bytes[off..],
                (r.va + off as u32) as u64,
                DecoderOptions::NONE,
            )
            .decode();
            anchors.extend(dispatch_anchors(&insn));
        }
    }
    let tables = select_tables(a.xbe, tables, anchors);
    let view = crate::cfg::CodeView::new(a.xbe);
    let mut candidates = BTreeSet::new();
    let mut traps = BTreeSet::new();
    let mut bounded = BTreeSet::new();
    for table in &tables {
        for &(_, target) in &table.slots {
            if target == 0 {
                continue;
            }
            candidates.insert(target);
            if table.known_layout {
                bounded.insert(target);
            }
            if trap_callback(&view, target, table) {
                traps.insert(target);
            }
        }
    }
    for target in candidates {
        if traps.contains(&target)
            && a.image.state(target) == Some(Byte::Unknown)
            && !a.blacklist.contains(&target)
            && a.looks_like_start(target)
            && view.decode(target).is_some_and(|insn| {
                a.image.check_insn(target, insn.len(), None).is_ok()
                    && a.image.check_insn(insn.next_ip32(), 1, None).is_ok()
            })
        {
            a.add_function(target, Source::DataPointer, target);
            a.run_certain();
        } else if bounded.contains(&target) || a.image.state(target) == Some(Byte::InsnStart) {
            // A heuristic run can end in pointer-shaped metadata. It may expose
            // an already-decoded entry, but cannot authorize a fresh decode in
            // unknown bytes. Only a validated schema bypasses start heuristics;
            // the corroborated trap case above also requires a plausible start.
            a.try_candidate(target, Source::DataPointer);
        }
    }
    Ok(())
}

/// Addresses used as table bases by stores or indirect control transfers.
pub(crate) fn dispatch_anchors(insn: &Instruction) -> Vec<u32> {
    if let Some(value) = stored_table(insn) {
        return vec![value];
    }
    if matches!(
        insn.flow_control(),
        iced_x86::FlowControl::IndirectCall | iced_x86::FlowControl::IndirectBranch
    ) && insn.op0_kind() == OpKind::Memory
    {
        return references(insn);
    }
    Vec::new()
}

/// Split adjacent vtables at observed bases, excluding unrelated prefix words.
/// The last run's end remains a heuristic and is reported as such.
pub(crate) fn select_tables(
    xbe: &xbe::Xbe,
    raw: Vec<Table>,
    mut anchors: BTreeSet<u32>,
) -> Vec<Table> {
    let view = crate::cfg::CodeView::new(xbe);
    for table in &raw {
        let end = table.base + table.slots.len() as u32 * 4;
        let Some(&first) = anchors.range(table.base..end).next() else {
            continue;
        };
        for &(slot, target) in &table.slots {
            if slot < first {
                continue;
            }
            if trap_callback(&view, target, table) {
                anchors.extend(view.decode(target).and_then(|i| stored_table(&i)));
            }
        }
    }
    let mut out = Vec::new();
    for table in raw {
        if table.known_layout {
            out.push(table);
            continue;
        }
        let end = table.base + table.slots.len() as u32 * 4;
        let starts: Vec<u32> = anchors
            .range(table.base..end)
            .copied()
            .filter(|va| va.is_multiple_of(4))
            .collect();
        for (i, &base) in starts.iter().enumerate() {
            let limit = starts.get(i + 1).copied().unwrap_or(end);
            out.push(Table {
                name: format!("dispatch-table-{base:08x}"),
                base,
                spans: vec![(base, limit - base)],
                slots: table
                    .slots
                    .iter()
                    .copied()
                    .filter(|&(slot, _)| slot >= base && slot < limit)
                    .collect(),
                known_layout: false,
            });
        }
    }
    // A statically referenced singleton is a valid callback cell too.
    for base in anchors {
        if !base.is_multiple_of(4)
            || out.iter().any(|t| {
                t.spans
                    .iter()
                    .any(|&(start, len)| base >= start && base - start < len)
            })
        {
            continue;
        }
        if xbe
            .section_containing(base)
            .is_none_or(crate::is_code_section)
        {
            continue;
        }
        if let Ok(target) = xbe.read_u32(base)
            && xbe
                .section_containing(target)
                .is_some_and(crate::is_code_section)
        {
            out.push(Table {
                name: format!("dispatch-cell-{base:08x}"),
                base,
                spans: vec![(base, 4)],
                slots: vec![(base, target)],
                known_layout: false,
            });
        }
    }
    out.sort_by_key(|t| t.base);
    out
}
#[cfg(test)]
mod tests {
    use crate::{analyze_with_pointer_seeds, tests::xbe_with};
    fn image(constructor: bool, target: u32) -> Vec<u8> {
        let mut code = vec![0xcc; 64];
        if constructor {
            // Synthetic constructor: mov [ecx], table; ret.
            code[..7].copy_from_slice(&[0xc7, 1, 0x10, 0x20, 1, 0, 0xc3]);
        } else {
            code[0] = 0xc3;
        }
        // Synthetic trap callback and ordinary callback, both table members.
        code[0x21..0x28].copy_from_slice(&[0xc7, 1, 0x10, 0x20, 1, 0, 0xcc]);
        code[0x30] = 0xc3;
        let mut data = xbe_with(&code);
        data[0x1110..0x1114].copy_from_slice(&target.to_le_bytes());
        data[0x1114..0x1118].copy_from_slice(&0x11030u32.to_le_bytes());
        data
    }
    // Sol's synthetic fixture: a one-slot vtable followed by metadata.
    // The metadata points into a MOV immediate after a rejected UD2 block.
    fn adjacent_metadata() -> Vec<u8> {
        let mut code = vec![0xcc; 0x80];
        code[..7].copy_from_slice(&[0xc7, 1, 0x10, 0x20, 1, 0, 0xc3]);
        code[0x20] = 0xc3;
        code[0x40..0x48].copy_from_slice(&[0x0f, 0x0b, 0xb8, 0x41, 0xc3, 0x41, 0x41, 0xc3]);
        let mut data = xbe_with(&code);
        data[0x1110..0x1114].copy_from_slice(&0x11020u32.to_le_bytes());
        data[0x1114..0x1118].copy_from_slice(&0x11044u32.to_le_bytes());
        data
    }

    #[test]
    fn heuristic_tail_does_not_seed_metadata_inside_unclaimed_mov() {
        let data = adjacent_metadata();
        let xbe = xbe::Xbe::parse(&data).unwrap();
        for seeded in [false, true] {
            let a = analyze_with_pointer_seeds(&xbe, seeded).unwrap();
            assert_eq!(a.claim(0x11044), Some(('?', None)));
            assert!(!a.functions.contains_key(&0x11044));
            assert!(!a.labels.contains(&0x11044));
            assert!(a.conflicts.is_empty());
        }
    }

    #[test]
    fn final_rejected_candidate_is_still_seeded_as_a_trap_callback() {
        for control in [false, true] {
            let mut data = adjacent_metadata();
            data[0x1040..0x1080].fill(0xcc);
            data[0x1040..0x1047].copy_from_slice(&[0xc7, 1, 0x10, 0x20, 1, 0, 0xcc]);
            data[0x1114..0x1118].copy_from_slice(&0x11040u32.to_le_bytes());
            if control {
                data[0x1060] = 0xc3;
                data[0x1070..0x1072].copy_from_slice(&[0x0f, 0x0b]);
                data[0x1118..0x111c].copy_from_slice(&0x11060u32.to_le_bytes());
            }
            let xbe = xbe::Xbe::parse(&data).unwrap();
            let before = analyze_with_pointer_seeds(&xbe, false).unwrap();
            assert_eq!(before.claim(0x11040), Some(('?', None)));
            let after = analyze_with_pointer_seeds(&xbe, true).unwrap();
            assert!(after.functions.contains_key(&0x11040), "control={control}");
            assert!(!after.functions[&0x11040].returns);
            assert!(after.conflicts.is_empty());
            assert!(after.converged);
        }
    }

    #[test]
    fn seeding_respects_flat_far_jump_instruction_claims() {
        for selector in [crate::FLAT_CS, 0x10] {
            let mut data = adjacent_metadata();
            // After the constructor store, far-jump into the MOV itself.
            data[0x1006..0x100d].copy_from_slice(&[0xea, 0x42, 0x10, 1, 0, selector as u8, 0]);
            let xbe = xbe::Xbe::parse(&data).unwrap();
            for seeded in [false, true] {
                let a = analyze_with_pointer_seeds(&xbe, seeded).unwrap();
                let expected = if selector == crate::FLAT_CS { 'i' } else { '?' };
                assert_eq!(a.claim(0x11044).unwrap().0, expected);
                assert!(!a.functions.contains_key(&0x11044));
                assert!(!a.labels.contains(&0x11044));
                assert!(a.conflicts.is_empty());
            }
        }
    }

    #[test]
    fn observed_table_base_excludes_prefix_words_and_preserves_singletons() {
        let data = image(true, 0x11021);
        let xbe = xbe::Xbe::parse(&data).unwrap();
        let raw = crate::dispatch::tables(&xbe).unwrap();
        let selected = super::select_tables(&xbe, raw, std::collections::BTreeSet::from([0x12014]));
        // The first word is outside the observed table even though it looks
        // like a pointer to decodable code.
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].base, 0x12014);
        assert_eq!(selected[0].slots, [(0x12014, 0x11030)]);
        // A lone referenced callback cell needs no adjacent pointer evidence.
        let mut singleton = data;
        singleton[0x1110..0x1114].fill(0);
        let xbe = xbe::Xbe::parse(&singleton).unwrap();
        let selected = super::select_tables(
            &xbe,
            crate::dispatch::tables(&xbe).unwrap(),
            std::collections::BTreeSet::from([0x12014]),
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].slots, [(0x12014, 0x11030)]);
    }

    #[test]
    fn constructor_table_seeds_trapping_callback_without_changing_its_behavior() {
        let data = image(true, 0x11021);
        let xbe = xbe::Xbe::parse(&data).unwrap();
        let before = analyze_with_pointer_seeds(&xbe, false).unwrap();
        assert!(!before.functions.contains_key(&0x11021));
        let after = analyze_with_pointer_seeds(&xbe, true).unwrap();
        assert!(after.functions.contains_key(&0x11021));
        assert!(!after.functions[&0x11021].returns);
        assert!(after.conflicts.is_empty());
        let cfg = crate::cfg::CfgBuilder::new(&after, &xbe)
            .build(0x11021)
            .unwrap();
        assert!(cfg.errors.is_empty());
        assert_eq!(
            cfg.blocks[&0x11021].edges[0].kind,
            crate::cfg::EdgeKind::Trap
        );
    }
    #[test]
    fn unreferenced_data_run_does_not_authorize_trap_code() {
        let data = image(false, 0x11021);
        let xbe = xbe::Xbe::parse(&data).unwrap();
        assert!(
            !crate::analyze(&xbe)
                .unwrap()
                .functions
                .contains_key(&0x11021)
        );
    }
    #[test]
    fn table_pointer_into_instruction_body_is_not_promoted() {
        let data = image(true, 0x11002);
        let xbe = xbe::Xbe::parse(&data).unwrap();
        let a = crate::analyze(&xbe).unwrap();
        assert!(!a.functions.contains_key(&0x11002));
        assert!(!a.labels.contains(&0x11002));
        assert!(a.conflicts.is_empty());
    }
    #[test]
    fn stack_store_is_not_constructor_evidence() {
        let mut data = image(true, 0x11021);
        // Change mov [ecx] into mov [ebp+0], with a synthetic 8-byte body.
        data[0x1000..0x1008].copy_from_slice(&[0xc7, 0x45, 0, 0x10, 0x20, 1, 0, 0xc3]);
        let xbe = xbe::Xbe::parse(&data).unwrap();
        assert!(
            !crate::analyze(&xbe)
                .unwrap()
                .functions
                .contains_key(&0x11021)
        );
    }
}
