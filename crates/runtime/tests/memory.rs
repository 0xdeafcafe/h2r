use runtime::guest::{Access, FaultReason, Guest, GuestFault, Permissions, RAM_SIZE};

fn mapped() -> Guest {
    let mut memory = Guest::new();
    memory.map(0x1000, 0x1000, 0, Permissions::RW).unwrap();
    memory
}
#[test]
fn unmapped_protected_wrapping_and_device_accesses_fault() {
    let mut memory = mapped();
    assert_eq!(
        memory.u32(0),
        Err(GuestFault {
            addr: 0,
            len: 4,
            access: Access::Read,
            reason: FaultReason::Unmapped
        })
    );
    assert_eq!(
        memory.u32(u32::MAX).unwrap_err().reason,
        FaultReason::AddressOverflow
    );
    assert_eq!(
        memory.u32(0xfd00_0000).unwrap_err().reason,
        FaultReason::UnsupportedDevice
    );
    assert!(memory.map(0xfd00_0000, 4, 0, Permissions::RW).is_err());
    assert!(
        memory
            .map(0x3000, 4, RAM_SIZE as u32, Permissions::RW)
            .is_err()
    );
    memory.protect(0x1000, Permissions::READ).unwrap();
    assert_eq!(
        memory.set_u32(0x1000, 42).unwrap_err().reason,
        FaultReason::Protection
    );
}
#[test]
fn all_widths_are_little_endian_and_unaligned_by_default() {
    let mut memory = mapped();
    memory.set_u8(0x1001, 0xab).unwrap();
    assert_eq!(memory.u8(0x1001).unwrap(), 0xab);
    memory.set_u16(0x1001, 0xabcd).unwrap();
    assert_eq!(memory.u16(0x1001).unwrap(), 0xabcd);
    memory.set_u32(0x1001, 0x12345678).unwrap();
    assert_eq!(memory.u8(0x1001).unwrap(), 0x78);
    memory.set_u64(0x1001, 0x123456789abcdef0).unwrap();
    assert_eq!(memory.u64(0x1001).unwrap(), 0x123456789abcdef0);
    memory.set_u80(0x1001, [0xab; 10]).unwrap();
    assert_eq!(memory.u80(0x1001).unwrap(), [0xab; 10]);
    memory.set_u128(0x1001, u128::MAX - 1).unwrap();
    assert_eq!(memory.u128(0x1001).unwrap(), u128::MAX - 1);
    assert_eq!(
        memory
            .read_bytes(0x1001, &mut [0; 16], 16)
            .unwrap_err()
            .reason,
        FaultReason::Misaligned
    );
}
#[test]
fn aliases_and_cross_mapping_accesses_preserve_physical_identity() {
    let mut memory = mapped();
    memory.map(0x2000, 0x1000, 0x4000, Permissions::RW).unwrap();
    memory.map(0x8000_1000, 0x1000, 0, Permissions::RW).unwrap();
    memory.set_u32(0x1ffe, 0x12345678).unwrap();
    assert_eq!(memory.u16(0x8000_1ffe).unwrap(), 0x5678);
    assert_eq!(memory.u16(0x2000).unwrap(), 0x1234);
    memory.protect(0x2000, Permissions::READ).unwrap();
    assert!(memory.set_u32(0x1ffe, 0xdeadbeef).is_err());
    assert_eq!(memory.u32(0x1ffe).unwrap(), 0x12345678);
    assert!(memory.set_u32(0x2ffe, 0xdeadbeef).is_err());
    assert_eq!(memory.u16(0x2ffe).unwrap(), 0);
}
