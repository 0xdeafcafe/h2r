//! The retail code the Halo 1 health model rests on (docs/halo1/health.md),
//! checked instruction by instruction against the retail XBE. Addresses
//! and instruction shapes only; no bytes are kept.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test health_xbe -- --ignored`

use iced_x86::{Code, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};

struct Image(std::sync::Arc<runtime::Runtime>);

impl Image {
    fn load() -> Self {
        let path = std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe");
        let bytes = std::fs::read(path).unwrap();
        let loaded =
            runtime::load_xbe_with(&bytes, &runtime::loader::LoadOptions::halo2()).unwrap();
        Image(loaded.runtime)
    }
    fn at(&self, addr: u32) -> Instruction {
        let mut buf = [0u8; 16];
        self.0.memory().read_bytes(addr, &mut buf, 1).unwrap();
        let mut d = Decoder::with_ip(32, &buf, addr as u64, DecoderOptions::NONE);
        d.decode()
    }
    fn f32(&self, addr: u32) -> f32 {
        f32::from_bits(self.0.memory().u32(addr).unwrap())
    }
}

/// `mnemonic`, with a memory operand `[base + disp]`.
fn mem(i: &Instruction, mnemonic: Mnemonic, base: Register, disp: u32) {
    assert_eq!(i.mnemonic(), mnemonic, "{:#x}: {:?}", i.ip(), i.code());
    let has = (0..i.op_count()).any(|n| i.op_kind(n) == OpKind::Memory);
    assert!(has, "{:#x}: {:?}", i.ip(), i.code());
    assert_eq!(i.memory_base(), base, "{:#x}: {:?}", i.ip(), i.code());
    assert_eq!(
        i.memory_displacement32(),
        disp,
        "{:#x}: {:?}",
        i.ip(),
        i.code()
    );
}

fn call(i: &Instruction, target: u32) {
    assert_eq!(
        i.mnemonic(),
        Mnemonic::Call,
        "{:#x}: {:?}",
        i.ip(),
        i.code()
    );
    assert_eq!(i.near_branch32(), target, "{:#x}: {:?}", i.ip(), i.code());
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_damage_update_and_its_regeneration() {
    let x = Image::load();
    // OBJECT_DAMAGE_UPDATE: its object_update call site, the stdcall shape.
    call(&x.at(0x000b_c527), 0x000d_5de0);
    assert_eq!(x.at(0x000b_c524).mnemonic(), Mnemonic::Push, "one argument");
    assert_eq!(x.at(0x000d_5de0).code(), Code::Sub_rm32_imm8);
    for ret in [0x000d_6611, 0x000d_664c, 0x000d_6657] {
        let i = x.at(ret);
        assert_eq!(
            (i.mnemonic(), i.immediate16()),
            (Mnemonic::Ret, 4),
            "{ret:#x}: {:?}",
            i.code()
        );
    }
    // The regeneration: target (definition +0x38) above health, rate
    // (definition +0xd4) above 0, body stun timer +0x106 zero, then
    // health += rate * dt, clamped, "recharging" (+0x10b bit 0) set.
    mem(&x.at(0x000d_63e2), Mnemonic::Movss, Register::EAX, 0x38);
    mem(&x.at(0x000d_63e7), Mnemonic::Movss, Register::EAX, 0xd4);
    mem(&x.at(0x000d_63f5), Mnemonic::Comiss, Register::EBP, 0xec);
    mem(&x.at(0x000d_640b), Mnemonic::Mov, Register::EBP, 0x106);
    mem(&x.at(0x000d_6465), Mnemonic::Or, Register::EBP, 0x10b);
    mem(&x.at(0x000d_646c), Mnemonic::Movss, Register::EBP, 0xec);
    assert_eq!(x.at(0x000d_6474).mnemonic(), Mnemonic::Addss);
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_regeneration_is_the_only_rise_in_the_update() {
    let x = Image::load();
    // Every store to the body's health (+0xec) in 0xd5de0..0xd665a: the
    // regeneration's add and its clamp to the target, nothing else.
    let mut stores = Vec::new();
    let mut buf = vec![0u8; 0xd665a - 0xd5de0];
    x.0.memory().read_bytes(0xd5de0, &mut buf, 1).unwrap();
    let mut d = Decoder::with_ip(32, &buf, 0xd5de0, DecoderOptions::NONE);
    for i in &mut d {
        if i.op_count() > 0
            && i.op0_kind() == OpKind::Memory
            && i.memory_displacement32() == 0xec
            && i.mnemonic() == Mnemonic::Movss
        {
            stores.push(i.ip() as u32);
        }
    }
    assert_eq!(stores, [0x000d_647b, 0x000d_6485]);
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn health_packs_are_the_powerup_pickups_health_case() {
    let x = Image::load();
    // The pickup reads the equipment's powerup type (+0x12c) and time
    // (+0x130; zero or less and nothing happens), and type 5 calls the
    // body restore.
    mem(&x.at(0x0015_1eff), Mnemonic::Movss, Register::ESI, 0x130);
    mem(&x.at(0x0015_1f3e), Mnemonic::Mov, Register::ESI, 0x12c);
    let cmp = x.at(0x0015_1f6a);
    assert_eq!(
        (cmp.mnemonic(), cmp.immediate8to16()),
        (Mnemonic::Cmp, 5),
        "{:?}",
        cmp.code()
    );
    call(&x.at(0x0015_1f73), 0x000d_6790);
    // The pickup is reached by touch (0x15102f, not grenades) and by the
    // action (0x14fbc5).
    call(&x.at(0x0015_102f), 0x0015_1ea0);
    call(&x.at(0x0014_fbc5), 0x0015_1ea0);
    // BODY_RESTORE: not when depleted (+0x10a bit 2), only below the
    // constant, which is 1.0; writes the health.
    mem(&x.at(0x000d_67aa), Mnemonic::Mov, Register::EDX, 0x10a);
    mem(&x.at(0x000d_67cb), Mnemonic::Movss, Register::EDX, 0xec);
    assert_eq!(x.f32(0x0045_dbc0), 1.0);
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_hud_has_a_body_input_beside_the_shield() {
    let x = Image::load();
    // The per-user HUD state: body (+0xec) to +0x28, shield (+0xf0) to +0x2c.
    mem(&x.at(0x0013_99d3), Mnemonic::Mov, Register::EAX, 0xec);
    mem(&x.at(0x0013_99d9), Mnemonic::Mov, Register::EBX, 0x28);
    mem(&x.at(0x0013_99dc), Mnemonic::Mov, Register::EAX, 0xf0);
    mem(&x.at(0x0013_99e2), Mnemonic::Mov, Register::EBX, 0x2c);
    // The widget input switch: input 17 reads +0x28, input 16 +0x2c.
    mem(&x.at(0x0013_adba), Mnemonic::Movss, Register::EDX, 0x28);
    mem(&x.at(0x0013_adc1), Mnemonic::Movss, Register::EDX, 0x2c);
}

/// Why the converted HUD's health widget shows and hides with the shield
/// meter (docs/halo1/health.md "HUD"): the HUD render asks HUD_SHOWN first
/// and draws no `nhdt` when it says no (Blind's hook answers there); each
/// `nhdt` draw walks every bitmap widget through one state gate, its
/// inputs and one draw; the shield meter's own code is special type 5.
#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn every_hud_widget_is_drawn_behind_the_same_gates() {
    let x = Image::load();
    // The HUD render 0x13954b: HUD_SHOWN, then out when it says no.
    call(&x.at(0x0013_9554), 0x0013_939b);
    assert_eq!(x.at(0x0013_955b).mnemonic(), Mnemonic::Je);
    assert_eq!(x.at(0x0013_955b).near_branch32(), 0x0013_96ba);
    // ...then the HUD state (0x13992a) and the nhdt draws (0x1394b9).
    call(&x.at(0x0013_95b7), 0x0013_992a);
    call(&x.at(0x0013_95cd), 0x0013_94b9);
    call(&x.at(0x0013_95f5), 0x0013_94b9);
    // HUD_SHOWN reads the campaign Blind skull's byte.
    mem(
        &x.at(0x0013_93e1),
        Mnemonic::And,
        Register::None,
        0x004f_55e2,
    );
    // One nhdt's draw 0x13a532: bitmap widgets (+0x08 count, +0x0c
    // address), each: the state gate on widget +0x08, the inputs from
    // widget +0x04, the draw; stride 0x64.
    mem(&x.at(0x0013_a5d8), Mnemonic::Mov, Register::EDI, 0x08);
    mem(&x.at(0x0013_a5e8), Mnemonic::Mov, Register::EDI, 0x0c);
    mem(&x.at(0x0013_a5fa), Mnemonic::Lea, Register::ESI, 0x08);
    call(&x.at(0x0013_a5fd), 0x0013_ac87);
    mem(&x.at(0x0013_a610), Mnemonic::Lea, Register::ESI, 0x04);
    call(&x.at(0x0013_a617), 0x0013_ad48);
    call(&x.at(0x0013_a625), 0x0022_aff1);
    let i = x.at(0x0013_a62a);
    assert_eq!((i.mnemonic(), i.immediate(1)), (Mnemonic::Add, 0x64));
    // The widget draw switches on the special type (+0x60); type 5 reads
    // the HUD state's shield (+0x2c) itself.
    mem(&x.at(0x0022_b2d0), Mnemonic::Movzx, Register::ECX, 0x60);
    mem(&x.at(0x0022_b411), Mnemonic::Movss, Register::EAX, 0x2c);
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn shield_overflow_bleeds_into_the_body() {
    let x = Image::load();
    // The shield step hands what it did not absorb back in the damage
    // record (+0x1c); the body step reads it.
    mem(&x.at(0x000d_9473), Mnemonic::Movss, Register::EDI, 0x1c);
    mem(&x.at(0x000d_902a), Mnemonic::Movss, Register::ECX, 0x1c);
    call(&x.at(0x000d_80de), 0x000d_9110);
    call(&x.at(0x000d_8184), 0x000d_8cb0);
}
