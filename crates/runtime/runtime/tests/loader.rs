mod common;
use common::*;
use runtime::{
    guest::{Access, FaultReason},
    kernel, load_xbe, loader,
};

#[test]
fn loads_headers_sections_zero_tails_tls_stack_and_imports() {
    let bytes = synthetic_xbe();
    let image = load_xbe(&bytes).unwrap();
    let tls = image.tls.unwrap();
    assert_eq!(tls.raw_start, 0x12088);
    assert_eq!(tls.raw_end, 0x1208c);
    assert_eq!(tls.index_addr, TLS_INDEX);
    assert_eq!(tls.zero_fill, 0x90);
    assert_eq!(image.cpu.eip, ENTRY);
    assert_eq!(image.stack_top - image.stack_bottom, 0x40000);
    assert_eq!(image.cpu.esp, image.stack_top - 4);
    assert_eq!(image.cpu.x87.control, 0x027f);
    let memory = image.runtime.memory();
    assert_eq!(memory.u32(0x10108).unwrap(), 0x1000);
    assert_eq!(memory.u32(ENTRY).unwrap(), 0xcccccccc);
    assert_eq!(memory.u32(ENTRY + 0x100).unwrap(), 0);
    assert_eq!(memory.u32(0x12ffc).unwrap(), 0);
    assert_eq!(memory.u32(TLS_INDEX).unwrap(), 0); // startup arithmetic is not pre-seeded
    assert_eq!(memory.u32(THUNK).unwrap(), kernel::function_token(255));
    assert_eq!(memory.u32(THUNK + 4).unwrap(), image.data_exports[&156]);
    assert_eq!(memory.u32(image.data_exports[&156]).unwrap(), 0);
    assert_eq!(memory.u32(image.data_exports[&164]).unwrap(), 0); // separate guest pointer variable
    assert_eq!(
        memory.u32(image.cpu.esp).unwrap(),
        runtime::exec::HARNESS_EXIT
    );
    assert_eq!(
        memory.u32(kernel::function_token(255)).unwrap_err().reason,
        FaultReason::Unmapped
    );
    assert!(memory.check(ENTRY, 1, Access::Execute, 1).is_ok());
    assert_eq!(
        memory.check(ENTRY, 1, Access::Write, 1).unwrap_err().reason,
        FaultReason::Protection
    );
    assert!(memory.u32(image.stack_bottom - 1).is_err());
}
#[test]
fn malformed_layouts_fail_before_running() {
    for (offset, value) in [
        (0x204, 0xfffffff0u32),
        (0x208, 0x3000),
        (0x130, 0),
        (0x1144, 1),
        (0x1100, 0x8000ffff),
    ] {
        let mut bytes = synthetic_xbe();
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            load_xbe(&bytes).is_err(),
            "accepted corrupt offset {offset:#x}"
        );
    }
    let mut bytes = synthetic_xbe();
    bytes.truncate(0x1155);
    assert!(load_xbe(&bytes).is_err());
}

#[test]
#[ignore = "requires the user's local disc/default.xbe; never copies game data"]
fn real_disc_header_facts_from_lifter_section_9() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../disc/default.xbe");
    let bytes = std::fs::read(path).expect("supply disc/default.xbe locally");
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    assert_eq!(xbe.entry, 0x2d0aee);
    assert_eq!(xbe.base, 0x10000);
    assert_eq!(xbe.size_of_headers, 0x2000);
    assert_eq!(xbe.stack_commit, 0x40000);
    assert_eq!(xbe.tls_addr, 0x411a30);
    let loaded = load_xbe(&bytes).unwrap();
    let tls = loaded.tls.unwrap();
    assert_eq!((tls.raw_start, tls.raw_end), (0, 0));
    assert_eq!(tls.index_addr, 0x480100);
    assert_eq!(tls.callbacks_addr, 0);
    assert_eq!(tls.zero_fill, 0x90);
    assert_eq!(loaded.stack_top - loaded.stack_bottom, 0x40000);
    let memory = loaded.runtime.memory();
    assert_eq!(memory.u32(0x10108).unwrap(), 0x2000);
    assert_eq!(memory.u32(0x10118).unwrap(), 0x10184);
    assert_eq!(memory.u32(0x4115c4).unwrap(), kernel::function_token(255));
    assert_eq!(loaded.data_exports.len(), 17);
    assert_eq!(loaded.tls_data_size, Some(0x94));
    assert!(memory.check(0x379c40, 16, Access::Read, 16).is_ok());
    assert!(
        loaded
            .data_exports
            .values()
            .all(|a| *a >= loader::KERNEL_DATA_BASE)
    );
    for section in &xbe.sections {
        if section.vsize > section.raw_size {
            let mut tail = vec![1; (section.vsize - section.raw_size) as usize];
            memory
                .read_bytes(section.va + section.raw_size, &mut tail, 1)
                .unwrap();
            assert!(tail.iter().all(|b| *b == 0));
        }
    }
}

#[test]
fn tls_directory_cannot_escape_its_header_or_section_file_range() {
    let mut bytes = synthetic_xbe();
    // This address is in headers, but its complete directory crosses their end.
    bytes[0x12c..0x130].copy_from_slice(&0x10ff8u32.to_le_bytes());
    assert!(load_xbe(&bytes).is_err());
    // A directory in an unbacked virtual tail is not a file-backed directory.
    bytes[0x12c..0x130].copy_from_slice(&0x12ff0u32.to_le_bytes());
    assert!(load_xbe(&bytes).is_err());
    // A falsely huge header must fail instead of indexing beyond file bytes.
    bytes[0x108..0x10c].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(load_xbe(&bytes).is_err());
}

fn packed_xbe(flags: u32) -> Vec<u8> {
    let mut bytes = synthetic_xbe();
    let data = 0x11160;
    for (offset, value) in [
        (0x10c, 0x5000),
        (0x208, 0x14c),
        (0x238, flags),
        (0x23c, data),
        (0x240, 0x2000),
        (0x12c, data + 0x40),
        (0x158, data ^ 0x5b6d40b6),
        (0x1140, data + 0x88),
        (0x1144, data + 0x8c),
        (0x1148, 0x121e0),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
#[test]
fn packed_sections_share_pages_and_padding_without_duplicate_mappings() {
    let image = load_xbe(&packed_xbe(3)).unwrap();
    let mut m = image.runtime.memory();
    assert_eq!(m.u128(0x11140).unwrap(), 0); // aligned read includes the gap
    assert_eq!(m.u8(0x1115f).unwrap(), 0);
    assert_eq!(m.u32(0x11160).unwrap(), kernel::function_token(255));
    m.set_u32(0x11ffe, 0x12345678).unwrap(); // both shared pages writable
    assert_eq!(m.u32(0x11ffe).unwrap(), 0x12345678);
    assert!(m.check(0x1115f, 1, Access::Execute, 1).is_ok());
}
#[test]
fn head_tail_read_only_flags_are_applied_per_page() {
    let image = load_xbe(&packed_xbe(3 | 0x10 | 0x20)).unwrap();
    let mut m = image.runtime.memory();
    assert_eq!(
        m.set_u8(0x1115f, 1).unwrap_err().reason,
        FaultReason::Protection
    );
    m.set_u32(0x12ffc, 0x12345678).unwrap();
    assert_eq!(
        m.set_u32(0x12ffe, 0xaabbccdd).unwrap_err().reason,
        FaultReason::Protection
    );
    assert_eq!(m.u32(0x12ffc).unwrap(), 0x12345678);
    assert_eq!(
        m.set_u8(0x13000, 1).unwrap_err().reason,
        FaultReason::Protection
    );
}
#[test]
fn tls_index_is_checked_against_final_page_permissions() {
    let mut bytes = synthetic_xbe();
    bytes[0x238..0x23c].copy_from_slice(&2u32.to_le_bytes());
    assert!(
        matches!(load_xbe(&bytes), Err(loader::LoadError::Fault(f)) if f.reason == FaultReason::Protection)
    );
    let image = load_xbe(&synthetic_xbe()).unwrap();
    assert_eq!(image.tls_data_size, Some(0xa4));
}
#[test]
fn inspecting_a_bss_tls_directory_still_exposes_the_xbe_header() {
    let mut bytes = synthetic_xbe();
    bytes[0x12c..0x130].copy_from_slice(&0x12100u32.to_le_bytes());
    let xbe = xbe::Xbe::parse(&bytes).unwrap();
    assert_eq!(xbe.entry, ENTRY);
    assert_eq!(xbe.tls, None);
    assert!(xbe.tls_directory().is_err());
}

#[test]
fn permission_union_records_each_widened_shared_page() {
    let image = load_xbe(&packed_xbe(3)).unwrap();
    let note = image
        .permission_notes
        .iter()
        .find(|n| n.page == 0x11000)
        .unwrap();
    assert_eq!(note.effective, runtime::guest::Permissions::RWX);
    assert!(
        note.contributors
            .contains(&(".text".into(), runtime::guest::Permissions::RX))
    );
    assert!(
        note.contributors
            .contains(&(".data".into(), runtime::guest::Permissions::RW))
    );
    assert_eq!(image.permission_notes.len(), 1);
}
#[test]
fn permission_union_with_headers_is_also_recorded() {
    let mut bytes = synthetic_xbe();
    for (offset, value) in [
        (0x108, 0x800u32),
        (0x204, 0x10800),
        (0x208, 0x800),
        (0x128, 0x10800 ^ 0xa8fc57ab),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    let image = load_xbe(&bytes).unwrap();
    let note = image
        .permission_notes
        .iter()
        .find(|n| n.page == 0x10000)
        .unwrap();
    assert_eq!(note.effective, runtime::guest::Permissions::RWX);
    assert!(
        note.contributors
            .contains(&("headers".into(), runtime::guest::Permissions::RW))
    );
    assert!(
        note.contributors
            .contains(&(".text".into(), runtime::guest::Permissions::RX))
    );
}

#[test]
fn image_code_cannot_occupy_the_generation_checked_token_namespace() {
    let mut bytes = synthetic_xbe();
    for (offset, value) in [
        (0x10c, 0xafff1000u32),
        (0x204, 0xb0000000),
        (0x128, 0xb0000400 ^ 0xa8fc57ab),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    assert!(matches!(
        load_xbe(&bytes),
        Err(loader::LoadError::Invalid(_))
    ));
}
