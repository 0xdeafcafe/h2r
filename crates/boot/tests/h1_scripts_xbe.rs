//! The retail code the served Halo 1 builtins rest on
//! (docs/halo1/scripts.md, "Serving missing builtins"), checked against the
//! retail XBE: every claimed slot is a discarded stub nobody calls, the
//! shared stub returns through `0x209ae0`, and the return path starts as
//! the hook expects. Addresses and instruction shapes only; no bytes are
//! kept.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test h1_scripts_xbe -- --ignored`

use boot::h1_scripts::addr;
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, Register};

struct Image(std::sync::Arc<runtime::Runtime>);

impl Image {
    fn load() -> Self {
        let path = std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe");
        let bytes = std::fs::read(path).unwrap();
        let loaded =
            runtime::load_xbe_with(&bytes, &runtime::loader::LoadOptions::halo2()).unwrap();
        Image(loaded.runtime)
    }
    fn u32(&self, at: u32) -> u32 {
        self.0.memory().u32(at).unwrap()
    }
    fn u16(&self, at: u32) -> u16 {
        self.0.memory().u16(at).unwrap()
    }
    fn at(&self, addr: u32) -> Instruction {
        let mut buf = [0u8; 16];
        self.0.memory().read_bytes(addr, &mut buf, 1).unwrap();
        let mut d = Decoder::with_ip(32, &buf, addr as u64, DecoderOptions::NONE);
        d.decode()
    }
}

/// Every slot the table claims names the shared discarded stub, and no
/// retail script calls it (its name is absent from the disc's maps, the
/// `h1campaign` disc test checks against this same table).
#[test]
fn claimed_slots_are_discarded_stubs() {
    let xbe = Image::load();
    for (name, slot, family) in h1builtins::claims() {
        let descriptor = xbe.u32(addr::FUNCTION_TABLE + u32::from(slot) * 4);
        let evaluate = xbe.u32(descriptor + 4);
        assert_eq!(
            evaluate, addr::NULL_STUB,
            "{name} ({family}): slot {slot}'s evaluator"
        );
    }
}

/// The shared stub: the thread from the stack, zero returned through
/// `0x209ae0`, three arguments popped.
#[test]
fn the_discarded_stub_returns_zero_through_the_return_path() {
    let xbe = Image::load();
    let i = xbe.at(addr::NULL_STUB);
    assert_eq!(i.mnemonic(), Mnemonic::Mov);
    assert_eq!(i.memory_base(), Register::ESP);
    assert_eq!(i.memory_displacement32(), 8);
    let i = xbe.at(addr::NULL_STUB + 4);
    assert_eq!(i.mnemonic(), Mnemonic::Xor);
    assert_eq!(i.op0_register(), Register::EAX);
    assert_eq!(i.op1_register(), Register::EAX);
    let i = xbe.at(addr::NULL_STUB + 6);
    assert_eq!(i.mnemonic(), Mnemonic::Call);
    assert_eq!(i.near_branch32(), u64::from(addr::RETURN_PATH));
    let i = xbe.at(addr::NULL_STUB + 11);
    assert_eq!(i.mnemonic(), Mnemonic::Ret);
    assert_eq!(i.immediate(1), 0xc);
}

/// The return path masks ECX to the thread datum first, as the hook's
/// thread walk needs.
#[test]
fn the_return_path_takes_the_thread_in_ecx() {
    let xbe = Image::load();
    let i = xbe.at(addr::RETURN_PATH);
    assert_eq!(i.mnemonic(), Mnemonic::And);
    assert_eq!(i.op0_register(), Register::ECX);
    assert_eq!(i.immediate32(), 0xffff);
    // ... and reads the thread array at `0x4F9384` two instructions in.
    let i = xbe.at(addr::RETURN_PATH + 9);
    assert_eq!(i.mnemonic(), Mnemonic::Mov);
    assert_eq!(i.memory_displacement32(), addr::THREADS);
}
