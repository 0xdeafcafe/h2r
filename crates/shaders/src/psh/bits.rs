//! The D3D `PS_*` encodings, for building definitions by hand (tests, tools).
#![allow(missing_docs)]

pub const REG_ZERO: u8 = 0x0;
pub const REG_C0: u8 = 0x1;
pub const REG_C1: u8 = 0x2;
pub const REG_FOG: u8 = 0x3;
pub const REG_V0: u8 = 0x4;
pub const REG_V1: u8 = 0x5;
pub const REG_T0: u8 = 0x8;
pub const REG_T1: u8 = 0x9;
pub const REG_T2: u8 = 0xA;
pub const REG_T3: u8 = 0xB;
pub const REG_R0: u8 = 0xC;
pub const REG_R1: u8 = 0xD;
pub const REG_V1R0_SUM: u8 = 0xE;
pub const REG_EF_PROD: u8 = 0xF;

pub const CHAN_RGB: u8 = 0x00;
pub const CHAN_BLUE: u8 = 0x00;
pub const CHAN_ALPHA: u8 = 0x10;

pub const MAP_UNSIGNED_IDENTITY: u8 = 0x00;
pub const MAP_UNSIGNED_INVERT: u8 = 0x20;
pub const MAP_EXPAND_NORMAL: u8 = 0x40;
pub const MAP_EXPAND_NEGATE: u8 = 0x60;
pub const MAP_HALFBIAS_NORMAL: u8 = 0x80;
pub const MAP_HALFBIAS_NEGATE: u8 = 0xA0;
pub const MAP_SIGNED_IDENTITY: u8 = 0xC0;
pub const MAP_SIGNED_NEGATE: u8 = 0xE0;

/// `PS_REGISTER_ONE`.
pub const ONE: u8 = REG_ZERO | MAP_UNSIGNED_INVERT;

pub const OUT_IDENTITY: u32 = 0x00 << 12;
pub const OUT_BIAS: u32 = 0x08 << 12;
pub const OUT_SHIFTLEFT_1: u32 = 0x10 << 12;
pub const OUT_SHIFTLEFT_1_BIAS: u32 = 0x18 << 12;
pub const OUT_SHIFTLEFT_2: u32 = 0x20 << 12;
pub const OUT_SHIFTRIGHT_1: u32 = 0x30 << 12;
pub const OUT_AB_BLUE_TO_ALPHA: u32 = 0x80 << 12;
pub const OUT_CD_BLUE_TO_ALPHA: u32 = 0x40 << 12;
pub const OUT_AB_DOT_PRODUCT: u32 = 0x02 << 12;
pub const OUT_CD_DOT_PRODUCT: u32 = 0x01 << 12;
pub const OUT_AB_CD_MUX: u32 = 0x04 << 12;

pub const COUNT_MUX_MSB: u32 = 0x001 << 8;
pub const COUNT_UNIQUE_C0: u32 = 0x010 << 8;
pub const COUNT_UNIQUE_C1: u32 = 0x100 << 8;

pub const FINAL_CLAMP_SUM: u32 = 0x80;
pub const FINAL_COMPLEMENT_V1: u32 = 0x40;
pub const FINAL_COMPLEMENT_R0: u32 = 0x20;

pub const TEX_NONE: u32 = 0x00;
pub const TEX_PROJECT2D: u32 = 0x01;
pub const TEX_PROJECT3D: u32 = 0x02;
pub const TEX_CUBEMAP: u32 = 0x03;
pub const TEX_PASSTHRU: u32 = 0x04;
pub const TEX_CLIPPLANE: u32 = 0x05;
pub const TEX_BUMPENVMAP: u32 = 0x06;
pub const TEX_BUMPENVMAP_LUM: u32 = 0x07;
pub const TEX_BRDF: u32 = 0x08;
pub const TEX_DOT_ST: u32 = 0x09;
pub const TEX_DOT_ZW: u32 = 0x0A;
pub const TEX_DOT_RFLCT_DIFF: u32 = 0x0B;
pub const TEX_DOT_RFLCT_SPEC: u32 = 0x0C;
pub const TEX_DOT_STR_3D: u32 = 0x0D;
pub const TEX_DOT_STR_CUBE: u32 = 0x0E;
pub const TEX_DPNDNT_AR: u32 = 0x0F;
pub const TEX_DPNDNT_GB: u32 = 0x10;
pub const TEX_DOTPRODUCT: u32 = 0x11;
pub const TEX_DOT_RFLCT_SPEC_CONST: u32 = 0x12;

pub const DOT_ZERO_TO_ONE: u32 = 0;
pub const DOT_MINUS1_TO_1_D3D: u32 = 1;
pub const DOT_MINUS1_TO_1_GL: u32 = 2;
pub const DOT_MINUS1_TO_1: u32 = 3;
pub const DOT_HILO_1: u32 = 4;

/// `PS_COMBINERINPUTS(a, b, c, d)`.
pub fn inputs(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) << 24 | (b as u32) << 16 | (c as u32) << 8 | d as u32
}
/// `PS_COMBINEROUTPUTS(ab, cd, sum, flags)`.
pub fn outputs(ab: u8, cd: u8, sum: u8, flags: u32) -> u32 {
    (ab as u32) << 4 | cd as u32 | (sum as u32) << 8 | flags
}
/// `PS_TEXTUREMODES(t0, t1, t2, t3)`.
pub fn texture_modes(t: [u32; 4]) -> u32 {
    t[0] | t[1] << 5 | t[2] << 10 | t[3] << 15
}
/// `PS_DOTMAPPING(t0, t1, t2, t3)` (t0 ignored).
pub fn dot_mapping(t1: u32, t2: u32, t3: u32) -> u32 {
    t1 | t2 << 4 | t3 << 8
}
/// `PS_INPUTTEXTURE(t0, t1, t2, t3)` (t0, t1 ignored).
pub fn input_texture(t2: u32, t3: u32) -> u32 {
    t2 << 16 | t3 << 20
}
/// `PS_FINALCOMBINERINPUTSEFG(e, f, g, flags)`.
pub fn final_efg(e: u8, f: u8, g: u8, flags: u32) -> u32 {
    (e as u32) << 24 | (f as u32) << 16 | (g as u32) << 8 | flags
}

/// [`crate::psh::PixelOptions::color_sign`] bits: `D3DTSS_COLORSIGN >> 28`
/// (`D3DTSIGN_ASIGNED` is bit 28, `RSIGNED` 29, `GSIGNED` 30, `BSIGNED` 31,
/// the NV2A `SET_TEXTURE_FILTER` sign bits).
pub const SIGN_A: u8 = 0x1;
pub const SIGN_R: u8 = 0x2;
pub const SIGN_G: u8 = 0x4;
pub const SIGN_B: u8 = 0x8;

/// The per-channel sign mask of a `D3DTSS_COLORSIGN` value.
pub fn color_sign(colorsign: u32) -> u8 {
    (colorsign >> 28) as u8
}
