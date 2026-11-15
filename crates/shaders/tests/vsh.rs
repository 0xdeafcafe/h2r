//! Vertex microcode: decoding, disassembly, interpreter semantics, and WGSL
//! validity for hand-built programs.

mod common;

use shaders::vsh::encode::{Inst, Operand as O, program};
use shaders::vsh::{self, IluOp, MacOp, OutReg, Program, SrcReg, interp};

const X: u8 = 8;
const Y: u8 = 4;
const Z: u8 = 2;
const W: u8 = 1;
const XYZW: u8 = 0xF;

/// `c-38`/`c-37` are where the D3D runtime puts the viewport scale/offset.
const VP_SCALE: u8 = 58;
const VP_OFFSET: u8 = 59;

/// World-view-projection (rows in c[96..100]), the standard Xbox epilogue,
/// a colour and a texture coordinate.
fn transform_program() -> Vec<u8> {
    program(&[
        Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(96)), None).to_out(false, OutReg::Pos, X),
        Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(97)), None).to_out(false, OutReg::Pos, Y),
        Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(98)), None).to_out(false, OutReg::Pos, Z),
        Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(99)), None).to_out(false, OutReg::Pos, W),
        // mul oPos.xyz, r12, c-38 : rcc r1.x, r12.w
        Inst::mac(MacOp::Mul, Some(O::r(12)), Some(O::c(VP_SCALE)), None)
            .to_out(false, OutReg::Pos, X | Y | Z)
            .with_ilu(IluOp::Rcc, O::r(12).swz("w"))
            .ilu_to_temp(1, X),
        // mad oPos.xyz, r12, r1.x, c-37
        Inst::mac(
            MacOp::Mad,
            Some(O::r(12)),
            Some(O::r(1).swz("x")),
            Some(O::c(VP_OFFSET)),
        )
        .to_out(false, OutReg::Pos, X | Y | Z),
        Inst::mac(MacOp::Mov, Some(O::v(3)), None, None).to_out(false, OutReg::D0, XYZW),
        Inst::mac(MacOp::Mov, Some(O::v(9)), None, None).to_out(false, OutReg::T(0), X | Y),
    ])
}

fn identity_constants() -> [[f32; 4]; 192] {
    let mut c = [[0.0; 4]; 192];
    for i in 0..4 {
        c[96 + i][i] = 1.0;
    }
    c[VP_SCALE as usize] = [320.0, -240.0, 16_777_215.0, 0.0];
    c[VP_OFFSET as usize] = [320.0, 240.0, 0.0, 0.0];
    c
}

#[test]
fn decodes_hand_assembled_words() {
    // mov oPos, v0 built from the field table by hand, not the encoder:
    // w1: MAC=1 at bit 21, A swizzle xyzw (00 01 10 11) at bits 0-7.
    // w2: A mux = 2 (v) at bit 26. w3: out mask 0xF at 12, to output at 11,
    // address 0, final bit 0.
    let words = [0, (1 << 21) | 0x1B, 2 << 26, (0xF << 12) | (1 << 11) | 1];
    let p = Program::from_words(vsh::ProgramKind::Normal, &words).unwrap();
    let i = &p.instructions[0];
    assert_eq!(i.mac, MacOp::Mov);
    assert_eq!(i.ilu, IluOp::Nop);
    assert_eq!(i.a.reg, SrcReg::Input(0));
    assert_eq!(i.a.swizzle, [0, 1, 2, 3]);
    assert!(i.is_final);
    assert_eq!(vsh::disassemble(&p), "000: mov oPos, v0\n");

    // rcc r1.x, -c[a0.x+60].w : ILU=3 at 25, const 60 at 13, C mux=3, C neg,
    // C swizzle wwww, temp 1, ILU mask x, relative bit.
    let w1 = (3 << 25) | (60 << 13);
    let w2 = (1 << 10) | (0xFF << 2);
    let w3 = (3 << 28) | (1 << 20) | (8 << 16) | (1 << 1) | 1;
    let p = Program::from_words(vsh::ProgramKind::Normal, &[0, w1, w2, w3]).unwrap();
    let i = &p.instructions[0];
    assert_eq!(i.ilu, IluOp::Rcc);
    assert_eq!(
        i.c.reg,
        SrcReg::Const {
            index: 60,
            relative: true
        }
    );
    assert!(i.c.neg);
    assert_eq!(i.ilu_temp, 1);
    assert_eq!(vsh::disassemble(&p), "000: rcc r1.x, -c[a0.x+60].w\n");
}

#[test]
fn c_temp_register_is_split_across_words() {
    // C temp r13 would be invalid; use r6 = 0b0110: high bits 01 in w2[0..2],
    // low bits 10 in w3[30..32]. ILU mov r2, r6.
    let w1 = 1 << 25;
    let w2 = 0b01 | (0x1B << 2);
    let w3 = (0b10 << 30) | (1 << 28) | (2 << 20) | (0xF << 16) | 1;
    let p = Program::from_words(vsh::ProgramKind::Normal, &[0, w1, w2, w3]).unwrap();
    assert_eq!(p.instructions[0].c.reg, SrcReg::Temp(6));
    assert_eq!(vsh::disassemble(&p), "000: mov r2, r6\n");
}

#[test]
fn encoder_round_trips_every_opcode() {
    use MacOp::*;
    for op in [
        Mov, Mul, Add, Mad, Dp3, Dph, Dp4, Dst, Min, Max, Slt, Sge, Arl,
    ] {
        let inst = Inst::mac(
            op,
            Some(O::v(2).swz("yzxw")),
            Some(O::c(100).negated()),
            Some(O::r(3).swz("z")),
        )
        .to_temp(4, X | Z);
        let bytes = program(&[inst]);
        let p = Program::parse(&bytes).unwrap();
        let i = &p.instructions[0];
        assert_eq!(i.mac, op);
        assert_eq!(i.a.reg, SrcReg::Input(2));
        assert_eq!(i.a.swizzle, [1, 2, 0, 3]);
        let (_, rb, rc) = i.mac_reads();
        if rb {
            assert_eq!(
                i.b.reg,
                SrcReg::Const {
                    index: 100,
                    relative: false
                }
            );
            assert!(i.b.neg);
        }
        if rc {
            assert_eq!(i.c.reg, SrcReg::Temp(3));
            assert_eq!(i.c.swizzle, [2; 4]);
        }
    }
    for op in [
        IluOp::Mov,
        IluOp::Rcp,
        IluOp::Rcc,
        IluOp::Rsq,
        IluOp::Exp,
        IluOp::Log,
        IluOp::Lit,
    ] {
        let bytes = program(&[Inst::ilu(op, O::v(1)).ilu_to_temp(5, XYZW)]);
        let p = Program::parse(&bytes).unwrap();
        assert_eq!(p.instructions[0].ilu, op);
        assert_eq!(p.instructions[0].ilu_temp, 5);
    }
}

#[test]
fn header_and_structure_errors() {
    assert!(matches!(
        Program::parse(&[0x78]),
        Err(vsh::Error::Truncated { .. })
    ));
    assert_eq!(
        Program::parse(&[0x79, 0x20, 1, 0]),
        Err(vsh::Error::BadHeader([0x79, 0x20]))
    );
    assert_eq!(
        Program::parse(&[0x78, 0x20, 0, 0]),
        Err(vsh::Error::BadCount(0))
    );
    assert!(matches!(
        Program::parse(&[0x78, 0x20, 2, 0, 0, 0]),
        Err(vsh::Error::Truncated { .. })
    ));
    // A program whose only instruction lacks the final bit.
    let mut bytes = program(&[Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_out(
        false,
        OutReg::Pos,
        XYZW,
    )]);
    bytes[4 + 12] &= !1;
    assert_eq!(Program::parse(&bytes), Err(vsh::Error::NoFinal));
    // MAC opcode 14.
    let words = [0u32, 14 << 21, 0, 1];
    assert!(matches!(
        Program::from_words(vsh::ProgramKind::Normal, &words),
        Err(vsh::Error::BadOpcode { .. })
    ));
    // An operand with mux 0.
    let words = [0u32, 1 << 21, 0, (0xF << 24) | 1];
    assert!(matches!(
        Program::from_words(vsh::ProgramKind::Normal, &words),
        Err(vsh::Error::BadOperand { .. })
    ));
    // Header kinds.
    let mut rw = program(&[Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_out(
        false,
        OutReg::Pos,
        XYZW,
    )]);
    rw[1] = 0x77;
    assert_eq!(
        Program::parse(&rw).unwrap().kind,
        vsh::ProgramKind::ReadWrite
    );
    rw[1] = 0x73;
    assert_eq!(Program::parse(&rw).unwrap().kind, vsh::ProgramKind::State);
    assert!(vsh::translate(&Program::parse(&rw).unwrap(), &Default::default()).is_err());
}

#[test]
fn disassembles_paired_instruction() {
    let p = Program::parse(&transform_program()).unwrap();
    let text = vsh::disassemble(&p);
    let expected = "\
000: dp4 oPos.x, v0, c[96]
001: dp4 oPos.y, v0, c[97]
002: dp4 oPos.z, v0, c[98]
003: dp4 oPos.w, v0, c[99]
004: mul oPos.xyz, r12, c[58] : rcc r1.x, r12.w
005: mad oPos.xyz, r12, r1.x, c[59]
006: mov oD0, v3
007: mov oT0.xy, v9
";
    assert_eq!(text, expected);
    assert_eq!(p.inputs_read(), (1 << 0) | (1 << 3) | (1 << 9));
}

#[test]
fn transform_and_viewport_fixup() {
    let p = Program::parse(&transform_program()).unwrap();
    let mut inputs = [[0.0; 4]; 16];
    inputs[0] = [0.5, 0.5, 0.25, 1.0];
    inputs[3] = [0.1, 0.2, 0.3, 0.4];
    inputs[9] = [0.7, 0.8, 0.9, 1.0];
    let mut c = identity_constants();
    let out = interp::run(&p, &inputs, &mut c);
    let pos = out.get(OutReg::Pos);
    // Screen space: x = 0.5*320+320, y = 0.5*-240+240, z = 0.25 * zmax.
    assert_eq!(pos, [480.0, 120.0, 0.25 * 16_777_215.0, 1.0]);
    assert_eq!(out.get(OutReg::D0), [0.1, 0.2, 0.3, 0.4]);
    // Only xy written; zw keep the (0, 0, 0, 1) default.
    assert_eq!(out.get(OutReg::T(0)), [0.7, 0.8, 0.0, 1.0]);
    let clip = interp::fixup(pos, [640.0, 480.0], 16_777_215.0);
    assert_eq!(clip, [0.5, 0.5, 0.25, 1.0]);

    // With a perspective w the fixup re-multiplies so the GPU's divide lands
    // back on the same pixel.
    let clip = interp::fixup([480.0, 120.0, 0.0, 4.0], [640.0, 480.0], 1.0);
    assert_eq!(clip, [2.0, 2.0, 0.0, 4.0]);
}

#[test]
fn fixup_keeps_z_max_inside_and_clips_past_it() {
    // z = w = the D24 maximum (full-screen passes): clip z is exactly w.
    let m = 16_777_215.0f32;
    let clip = interp::fixup([0.0, 0.0, m, m], [640.0, 480.0], m);
    assert_eq!(clip[2], clip[3]);
    let clip = interp::fixup([0.0, 0.0, 65_535.0, 1.0], [640.0, 480.0], 65_535.0);
    assert_eq!(clip[2], 1.0);
    // Past the maximum it stays beyond w, so the GPU clips it.
    let clip = interp::fixup([0.0, 0.0, m + 32.0, 1.0], [640.0, 480.0], m);
    assert!(clip[2] > clip[3]);
}

#[test]
fn paired_ilu_owns_r1_and_reads_happen_first() {
    // mov r1, v0 : rcp r1.x, v0.y  -- the MAC's r1 write is dropped; the ILU
    // writes r1.x. Then mov r0, r1 to observe.
    let bytes = program(&[
        Inst::mac(MacOp::Mov, Some(O::v(0)), None, None)
            .to_temp(1, XYZW)
            .with_ilu(IluOp::Rcp, O::v(0).swz("y"))
            .ilu_to_temp(1, X),
        Inst::mac(MacOp::Mov, Some(O::r(1)), None, None).to_out(false, OutReg::D0, XYZW),
    ]);
    let p = Program::parse(&bytes).unwrap();
    assert!(!p.instructions[0].mac_writes_temp());
    let mut inputs = [[0.0; 4]; 16];
    inputs[0] = [3.0, 4.0, 5.0, 6.0];
    let out = interp::run(&p, &inputs, &mut [[0.0; 4]; 192]);
    assert_eq!(out.get(OutReg::D0), [0.25, 0.0, 0.0, 0.0]);

    // Swap through the same register: add r2, r2, r3 : mov... both units
    // read pre-instruction r2.
    let bytes = program(&[
        Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_temp(2, XYZW),
        Inst::mac(MacOp::Add, Some(O::r(2)), None, Some(O::r(2)))
            .to_temp(2, XYZW)
            .to_out(false, OutReg::D1, XYZW),
        Inst::mac(MacOp::Mov, Some(O::r(2)), None, None).to_out(false, OutReg::D0, XYZW),
    ]);
    let p = Program::parse(&bytes).unwrap();
    let out = interp::run(&p, &inputs, &mut [[0.0; 4]; 192]);
    assert_eq!(out.get(OutReg::D1), [6.0, 8.0, 10.0, 12.0]);
    assert_eq!(out.get(OutReg::D0), [6.0, 8.0, 10.0, 12.0]);
}

#[test]
fn r12_reads_back_opos() {
    let bytes = program(&[
        Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_out(false, OutReg::Pos, XYZW),
        Inst::mac(MacOp::Add, Some(O::r(12)), None, Some(O::r(12))).to_temp(0, XYZW),
        Inst::mac(MacOp::Mov, Some(O::r(0)), None, None).to_out(false, OutReg::T(1), XYZW),
    ]);
    let p = Program::parse(&bytes).unwrap();
    let mut inputs = [[0.0; 4]; 16];
    inputs[0] = [1.0, 2.0, 3.0, 4.0];
    let out = interp::run(&p, &inputs, &mut [[0.0; 4]; 192]);
    assert_eq!(out.get(OutReg::T(1)), [2.0, 4.0, 6.0, 8.0]);
}

#[test]
fn arl_relative_addressing() {
    // mov a0.x, v1.x ; mov oD0, c[a0.x+10]
    let bytes = program(&[
        Inst::mac(MacOp::Arl, Some(O::v(1).swz("x")), None, None),
        Inst::mac(MacOp::Mov, Some(O::c_rel(10)), None, None).to_out(false, OutReg::D0, XYZW),
    ]);
    let p = Program::parse(&bytes).unwrap();
    assert_eq!(
        vsh::disassemble(&p),
        "000: mov a0.x, v1.x\n001: mov oD0, c[a0.x+10]\n"
    );
    let mut c = [[0.0; 4]; 192];
    for (i, v) in c.iter_mut().enumerate() {
        *v = [i as f32; 4];
    }
    let mut inputs = [[0.0; 4]; 16];
    // 17/255 * 255 lands a hair under 17 in f32; the bias keeps it at 17.
    inputs[1] = [(17.0f32 / 255.0) * 255.0, 0.0, 0.0, 0.0];
    let out = interp::run(&p, &inputs, &mut c.clone());
    assert_eq!(out.get(OutReg::D0), [27.0; 4]);
    // Out-of-range indices clamp.
    inputs[1] = [500.0, 0.0, 0.0, 0.0];
    assert_eq!(
        interp::run(&p, &inputs, &mut c.clone()).get(OutReg::D0),
        [191.0; 4]
    );
    inputs[1] = [-500.0, 0.0, 0.0, 0.0];
    assert_eq!(
        interp::run(&p, &inputs, &mut c.clone()).get(OutReg::D0),
        [0.0; 4]
    );
}

#[test]
fn op_semantics() {
    use interp::{ilu, mac};
    let a = [1.0, 2.0, 3.0, 4.0];
    let b = [5.0, 6.0, 7.0, 8.0];
    let c = [0.5, 0.5, 0.5, 0.5];
    assert_eq!(
        mac(MacOp::Add, a, b, c),
        [1.5, 2.5, 3.5, 4.5],
        "ADD is A + C"
    );
    assert_eq!(mac(MacOp::Mad, a, b, c), [5.5, 12.5, 21.5, 32.5]);
    assert_eq!(mac(MacOp::Dp3, a, b, c), [38.0; 4]);
    assert_eq!(mac(MacOp::Dph, a, b, c), [46.0; 4]);
    assert_eq!(mac(MacOp::Dp4, a, b, c), [70.0; 4]);
    assert_eq!(mac(MacOp::Dst, a, b, c), [1.0, 12.0, 3.0, 8.0]);
    assert_eq!(
        mac(MacOp::Slt, a, [1.0, 1.0, 9.0, 9.0], c),
        [0.0, 0.0, 1.0, 1.0]
    );
    assert_eq!(
        mac(MacOp::Sge, a, [1.0, 1.0, 9.0, 9.0], c),
        [1.0, 1.0, 0.0, 0.0]
    );
    assert_eq!(
        mac(
            MacOp::Mul,
            [0.0, f32::INFINITY, 2.0, f32::NAN],
            [f32::INFINITY, 0.0, 3.0, 0.0],
            c
        ),
        [0.0, 0.0, 6.0, 0.0]
    );
    assert_eq!(ilu(IluOp::Rcp, [4.0; 4]), [0.25; 4]);
    assert_eq!(ilu(IluOp::Rcc, [0.0; 4]), [interp::RCC_MAX; 4]);
    assert_eq!(ilu(IluOp::Rcc, [-1e30; 4]), [-interp::RCC_MIN; 4]);
    assert_eq!(ilu(IluOp::Rsq, [-16.0; 4]), [0.25; 4]);
    assert_eq!(ilu(IluOp::Exp, [2.5; 4]), [4.0, 0.5, 2.5f32.exp2(), 1.0]);
    assert_eq!(ilu(IluOp::Log, [-12.0; 4]), [3.0, 1.5, 12f32.log2(), 1.0]);
    assert_eq!(ilu(IluOp::Log, [0.0; 4])[1], 1.0);
    assert_eq!(
        ilu(IluOp::Lit, [0.5, 0.25, 0.0, 2.0]),
        [1.0, 0.5, 0.0625, 1.0]
    );
    assert_eq!(
        ilu(IluOp::Lit, [-0.5, 0.25, 0.0, 2.0]),
        [1.0, 0.0, 0.0, 1.0]
    );
}

#[test]
fn fog_takes_x_of_the_result() {
    // mad oFog.w, v0.w, c[24].x, c[24].y  (the shape Halo 2 uses)
    let bytes = program(&[Inst::mac(
        MacOp::Mad,
        Some(O::v(0).swz("w")),
        Some(O::c(24).swz("x")),
        Some(O::c(24).swz("y")),
    )
    .to_out(false, OutReg::Fog, W)]);
    let p = Program::parse(&bytes).unwrap();
    let mut c = [[0.0; 4]; 192];
    c[24] = [2.0, 1.0, 0.0, 0.0];
    let mut inputs = [[0.0; 4]; 16];
    inputs[0] = [0.0, 0.0, 0.0, 3.0];
    let out = interp::run(&p, &inputs, &mut c);
    assert_eq!(out.get(OutReg::Fog)[0], 7.0);
}

#[test]
fn constant_writes_stay_local() {
    let mut bytes = program(&[
        Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_const(false, 20, XYZW),
        Inst::mac(MacOp::Mov, Some(O::c(20)), None, None).to_out(false, OutReg::D0, XYZW),
    ]);
    bytes[1] = 0x77;
    let p = Program::parse(&bytes).unwrap();
    assert!(p.writes_constants());
    let mut inputs = [[0.0; 4]; 16];
    inputs[0] = [9.0; 4];
    let mut c = [[0.0; 4]; 192];
    let out = interp::run(&p, &inputs, &mut c);
    assert_eq!(out.get(OutReg::D0), [9.0; 4]);
    assert_eq!(c[20], [9.0; 4]);
    let t = vsh::translate(&p, &Default::default()).unwrap();
    assert!(t.body.contains("var cw = xvs.c;"));
    common::check(&t.module());
}

#[test]
fn unsupplied_inputs_come_from_defaults() {
    let p = Program::parse(&transform_program()).unwrap();
    let opts = vsh::VertexOptions {
        provided_inputs: 1 << 0,
        flat_shading: true,
    };
    let t = vsh::translate(&p, &opts).unwrap();
    assert_eq!(t.locations, 1);
    assert!(t.body.contains("xvs.defaults[3]"));
    assert!(t.body.contains("xvs.defaults[9]"));
    assert!(!t.body.contains("@location(3) v3"));
    let module = common::check(&t.module());
    let ep = module
        .entry_points
        .iter()
        .find(|e| e.name == "vs_main")
        .unwrap();
    assert_eq!(ep.function.arguments.len(), 1);
}

/// Every hand-built program above, plus one exercising every op, must be
/// accepted by naga and lower to MSL.
#[test]
fn generated_wgsl_validates() {
    let mut programs = vec![transform_program()];
    let mut all = Vec::new();
    for op in [
        MacOp::Mov,
        MacOp::Mul,
        MacOp::Add,
        MacOp::Mad,
        MacOp::Dp3,
        MacOp::Dph,
        MacOp::Dp4,
        MacOp::Dst,
        MacOp::Min,
        MacOp::Max,
        MacOp::Slt,
        MacOp::Sge,
    ] {
        all.push(Inst::mac(op, Some(O::v(1)), Some(O::c_rel(30)), Some(O::r(2))).to_temp(2, X | W));
    }
    all.push(Inst::mac(MacOp::Arl, Some(O::v(1).swz("x")), None, None));
    for op in [
        IluOp::Mov,
        IluOp::Rcp,
        IluOp::Rcc,
        IluOp::Rsq,
        IluOp::Exp,
        IluOp::Log,
        IluOp::Lit,
    ] {
        all.push(
            Inst::mac(
                MacOp::Mad,
                Some(O::r(2)),
                Some(O::v(1)),
                Some(O::r(3).swz("wzyx")),
            )
            .to_temp(4, XYZW)
            .with_ilu(op, O::r(3).swz("wzyx"))
            .ilu_to_temp(1, Y | Z)
            .to_out(true, OutReg::T(3), XYZW),
        );
    }
    for r in [
        OutReg::Pos,
        OutReg::D0,
        OutReg::D1,
        OutReg::Fog,
        OutReg::Pts,
        OutReg::B0,
        OutReg::B1,
        OutReg::T(0),
        OutReg::T(1),
        OutReg::T(2),
        OutReg::T(3),
    ] {
        all.push(
            Inst::mac(MacOp::Mov, Some(O::r(4).negated()), None, None).to_out(false, r, X | Z),
        );
    }
    programs.push(program(&all));
    for bytes in programs {
        let p = Program::parse(&bytes).unwrap();
        for flat in [false, true] {
            let t = vsh::translate(
                &p,
                &vsh::VertexOptions {
                    flat_shading: flat,
                    ..Default::default()
                },
            )
            .unwrap();
            common::check(&t.module());
        }
    }
}

/// Regression (review P1): `mov oPos, c[192]` decoded and then indexed past
/// the constant file in the interpreter and in WGSL. It is now rejected.
#[test]
fn absolute_constant_past_c191_is_rejected() {
    let words = [0, 0x0038_001b, 0x0c00_0000, 0x0000_f801];
    assert_eq!(
        Program::from_words(vsh::ProgramKind::Normal, &words),
        Err(vsh::Error::BadConst { index: 0, reg: 192 })
    );
    // c[191] is the last valid one; relative reads may name any base
    // because they are clamped at run time.
    for op in [O::c(191), O::c_rel(255)] {
        let bytes = program(&[Inst::mac(MacOp::Mov, Some(op), None, None).to_out(
            false,
            OutReg::Pos,
            XYZW,
        )]);
        let p = Program::parse(&bytes).unwrap();
        let mut c = [[0.0; 4]; 192];
        c[191] = [7.0; 4];
        assert_eq!(interp::run(&p, &[[0.0; 4]; 16], &mut c).regs[0], [7.0; 4]);
        common::check(&vsh::translate(&p, &Default::default()).unwrap().module());
    }
}

/// Regression (review P1): `mov oPos, v0` whose unused B operand field
/// names r15. Only operands an op uses may be read.
#[test]
fn unused_operand_fields_are_not_read() {
    let words = [0, 0x0020_001b, 0x0801_e800, 0x0000_f801];
    let p = Program::from_words(vsh::ProgramKind::Normal, &words).unwrap();
    assert_eq!(p.instructions[0].b.reg, SrcReg::Temp(15));
    let mut inputs = [[0.0; 4]; 16];
    inputs[0] = [1.0, 2.0, 3.0, 4.0];
    let out = interp::run(&p, &inputs, &mut [[0.0; 4]; 192]);
    assert_eq!(out.regs[0], [1.0, 2.0, 3.0, 4.0]);
    let t = vsh::translate(&p, &Default::default()).unwrap();
    assert!(!t.body.contains("r15"));
    common::check(&t.module());
}

/// Regression (review P1): `arl a0.x, v0.x` with a huge or non-finite
/// `v0.x` overflowed `a0 + index`. Any such value saturates the clamp.
#[test]
fn arl_with_huge_values_saturates() {
    let bytes = program(&[
        Inst::mac(MacOp::Arl, Some(O::v(0).swz("x")), None, None),
        Inst::mac(MacOp::Mov, Some(O::c_rel(5)), None, None).to_out(false, OutReg::D0, XYZW),
    ]);
    let p = Program::parse(&bytes).unwrap();
    let mut c = [[0.0; 4]; 192];
    for (i, v) in c.iter_mut().enumerate() {
        *v = [i as f32; 4];
    }
    for (x, want) in [
        (2_147_483_648.0f32, 191.0),
        (f32::MAX, 191.0),
        (f32::INFINITY, 191.0),
        (-2_147_483_648.0, 0.0),
        (f32::NEG_INFINITY, 0.0),
        (f32::NAN, 5.0),
        (186.0, 191.0),
        (-5.0, 0.0),
    ] {
        let mut inputs = [[0.0; 4]; 16];
        inputs[0][0] = x;
        let got = interp::run(&p, &inputs, &mut c.clone()).get(OutReg::D0);
        assert_eq!(got, [want; 4], "arl {x}");
    }
    assert_eq!(interp::arl(2_147_483_648.0), 1024);
    assert_eq!(interp::arl(f32::NAN), 0);
}

/// Spec-derived RCC/EXP/LOG boundaries (review P1 RCC(+0), P2 LOG/EXP).
#[test]
fn ilu_boundaries_follow_the_spec() {
    let p = common::ilu_boundary_program();
    for case in common::ilu_boundary_cases() {
        let mut inputs = [[0.0; 4]; 16];
        inputs[0] = [case.x, 0.0, 0.0, 0.0];
        let out = interp::run(&p, &inputs, &mut [[0.0; 4]; 192]);
        let rcc = out.get(OutReg::T(0));
        let exp = out.get(OutReg::T(1));
        let log = out.get(OutReg::T(2));
        for k in 0..4 {
            assert!(
                common::close(rcc[k], case.rcc, 1e-6),
                "rcc({:e}) = {rcc:?}",
                case.x
            );
            assert!(
                common::close(exp[k], case.exp[k], 1e-6),
                "exp({:e}) = {exp:?}",
                case.x
            );
            assert!(
                common::close(log[k], case.log[k], 1e-6),
                "log({:e}) = {log:?}",
                case.x
            );
        }
        // The sign of an RCC result is part of the spec.
        assert_eq!(rcc[0].is_sign_negative(), case.rcc.is_sign_negative());
    }
    // The exact IEEE-field values for the case the review named.
    let l = interp::log(f32::from_bits(0x037f_ffff));
    assert_eq!(l[0], -121.0);
    assert_eq!(l[1].to_bits(), 0x3fff_ffff);
}

#[test]
fn close_rejects_mismatched_non_finite_values() {
    use common::close;
    assert!(close(f32::INFINITY, f32::INFINITY, 1e-3));
    assert!(close(f32::NAN, f32::NAN, 1e-3));
    assert!(!close(f32::INFINITY, 1.0, 1e-3));
    assert!(!close(1.0, f32::INFINITY, 1e-3));
    assert!(!close(f32::INFINITY, f32::NEG_INFINITY, 1e-3));
    assert!(!close(f32::NAN, 1.0, 1e-3));
    assert!(!close(f32::INFINITY, f32::NAN, 1e-3));
    assert!(close(1.0e20, 1.000_01e20, 1e-3));
    assert!(!close(1.0e20, 1.1e20, 1e-3));
}
