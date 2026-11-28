//! The base look (base map times lightmap) as a translated pass, for a
//! host with no game (the editor; docs/editor/editor.md): the
//! same vertex and pixel translation every material pass takes
//! (`program::compile_pass`, `fog_gpu::FogSet`), so its depth is the one
//! the world layers drawn over it compute for the same vertex.
//!
//! A layer pass tests its depth less-or-equal against the base look's.
//! Drawn by `world.wgsl` (the view-projection product, depth `w / far`),
//! the base look's depth differs from a translated pass's below an ulp:
//! the translated vertex reaches the window through the game's epilogue
//! (`window = clip.xy / w · c[10] + c[11]`), is snapped to the 1/16-pixel
//! grid (`xvs_fixup`) and stores the W-buffer's floored depth
//! (`xps_w_depth`). On a slanted floor that lost a layer pass more than
//! half its pixels. Here the position is [`world_position`], one
//! instruction list: the base look's programs take it, and a program
//! built on it (the editor's invented layer pass) computes the same
//! position, bit for bit, through the same generated code.
//!
//! The images it samples are [`crate::look::HostStage`]s (a material's
//! base map, white), a stage kind of their own that no tag can name.

use std::collections::HashMap;

use map::TagId;
use shaders::psh::bits::{
    CHAN_ALPHA, ONE, REG_T0, REG_T3, TEX_NONE, TEX_PROJECT2D, final_efg, inputs, texture_modes,
};
use shaders::vsh::encode::{Inst, Operand as O, program};
use shaders::vsh::{IluOp, MacOp, OutReg, SrcReg};

use crate::fog_gpu::{FogPass, FogProgram, FogWords};
use crate::look::{HostStage, PassLook, Stage};
use crate::pass_state::PassState;
use crate::program::MODEL_ROWS;

/// The view position, in the layout the game uploads
/// (`program::ViewInputs::from_camera`): `r0 = v · c[0..3]` (`dp4`, x
/// first), `r1.x = rcc r0.w`, `r0.xy *= r1.x`, then the window
/// `oPos.xy = r0.xy · c[10].xy + c[11].xy` (a `mul` then an `add`: an
/// instruction reads one constant) and `oPos.zw = r0.zw`.
/// `position` is the register holding the world position (`v0` for a
/// cluster, the placed `r10` for a placement).
pub fn world_position(position: O) -> Vec<Inst> {
    let mut code: Vec<Inst> = (0..4u8)
        .map(|k| Inst::mac(MacOp::Dp4, Some(position), Some(O::c(k)), None).to_temp(0, 8 >> k))
        .collect();
    code.push(Inst::ilu(IluOp::Rcc, O::r(0).swz("w")).ilu_to_temp(1, 8));
    code.push(Inst::mac(MacOp::Mul, Some(O::r(0)), Some(O::r(1).swz("x")), None).to_temp(0, 8 | 4));
    code.push(
        Inst::mac(MacOp::Mul, Some(O::r(0)), Some(O::c(10).swz("xy")), None).to_temp(0, 8 | 4),
    );
    code.push(
        Inst::mac(MacOp::Add, Some(O::r(0)), None, Some(O::c(11).swz("xy"))).to_out(
            false,
            OutReg::Pos,
            8 | 4,
        ),
    );
    code.push(Inst::mac(MacOp::Mov, Some(O::r(0)), None, None).to_out(false, OutReg::Pos, 2 | 1));
    code
}

/// `code` as a program, or the first instruction the hardware's encoding
/// cannot hold: one that reads two constant registers or two input
/// registers (the encoder's own rule, which it asserts).
pub fn encode(code: &[Inst]) -> Result<Vec<u8>, String> {
    for (n, inst) in code.iter().enumerate() {
        let regs = || {
            [&inst.a, &inst.b, &inst.c]
                .into_iter()
                .flatten()
                .map(|o| o.reg)
        };
        let mut constants = regs().filter_map(|r| match r {
            SrcReg::Const { index, .. } => Some(index),
            _ => None,
        });
        let mut inputs = regs().filter_map(|r| match r {
            SrcReg::Input(v) => Some(v),
            _ => None,
        });
        if let Some(first) = constants.next()
            && let Some(other) = constants.find(|&c| c != first)
        {
            return Err(format!("instruction {n} reads c[{first}] and c[{other}]"));
        }
        if let Some(first) = inputs.next()
            && let Some(other) = inputs.find(|&v| v != first)
        {
            return Err(format!("instruction {n} reads v{first} and v{other}"));
        }
    }
    Ok(program(code))
}

/// A cluster's world classification: [`world_position`] of `v0`, then
/// `extra` (the texture coordinates a pass reads).
pub fn world_program(extra: &[Inst]) -> Result<Vec<u8>, String> {
    let mut code = world_position(O::v(0));
    code.extend_from_slice(extra);
    encode(&code)
}

/// A placement's rigid classification: `v0` decompressed
/// (`· c[170] + c[171]`), placed by the model rows `c[50..52]` (`dph`),
/// then [`world_position`] of it and `extra`.
pub fn rigid_program(extra: &[Inst]) -> Result<Vec<u8>, String> {
    let mut code = vec![
        Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(170)), None).to_temp(10, 0xf),
        Inst::mac(MacOp::Add, Some(O::r(10)), None, Some(O::c(171))).to_temp(10, 0xf),
    ];
    for k in 0..3u8 {
        code.push(
            Inst::mac(
                MacOp::Dph,
                Some(O::r(10)),
                Some(O::c(MODEL_ROWS as u8 + k)),
                None,
            )
            .to_temp(2, 8 >> k),
        );
    }
    code.push(Inst::mac(MacOp::Mov, Some(O::r(10).swz("w")), None, None).to_temp(2, 1));
    code.extend(world_position(O::r(2)));
    code.extend_from_slice(extra);
    encode(&code)
}

/// The base look's texture coordinates: `oT0` the base map's (`v1`),
/// `oT3` the lightmap's (`v6`).
fn base_coordinates() -> [Inst; 2] {
    [
        Inst::mac(MacOp::Mov, Some(O::v(1)), None, None).to_out(false, OutReg::T(0), 0xf),
        Inst::mac(MacOp::Mov, Some(O::v(6)), None, None).to_out(false, OutReg::T(3), 0xf),
    ]
}

/// The stage a material's base look samples its base map through
/// ([`HostStage::BaseMap`]: the host's stage-image lookup answers it with
/// the material's base map in the Lighting mode).
pub fn base_stage(material: u16) -> Stage {
    Stage::Host(HostStage::BaseMap(material))
}

/// The stage an unlit part's lightmap stage samples: white.
pub const WHITE_STAGE: Stage = Stage::Host(HostStage::White);

/// What a part of a base-look set draws: a material's base look, lit
/// (its lightmap at stage 3) or not ([`WHITE_STAGE`] there), or the
/// material's own layer-3 pass (`layers_load::load_layer3`). A set's
/// material slots stand for these through [`SetKeys`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PassKey {
    Base { material: u16, lit: bool },
    Layer3(u16),
}

/// A set's material slots (`fog_gpu::FogSet` keys its materials by
/// `u16`): one per [`PassKey`], dense and in the order first asked, so
/// no two keys share a slot.
#[derive(Clone, Debug, Default)]
pub struct SetKeys {
    slots: HashMap<PassKey, u16>,
    keys: Vec<PassKey>,
}

impl SetKeys {
    /// `key`'s slot, given one when it has none; an error past the
    /// `u16` slots.
    pub fn slot(&mut self, key: PassKey) -> Result<u16, String> {
        if let Some(&slot) = self.slots.get(&key) {
            return Ok(slot);
        }
        let slot = u16::try_from(self.keys.len())
            .map_err(|_| "more pass keys than a set has material slots".to_owned())?;
        self.slots.insert(key, slot);
        self.keys.push(key);
        Ok(slot)
    }

    /// The key `slot` stands for.
    pub fn key(&self, slot: u16) -> Option<PassKey> {
        self.keys.get(usize::from(slot)).copied()
    }
}

/// The base look's pass state: opaque, depth tested less-or-equal and
/// written, nothing culled.
pub fn state() -> PassState {
    PassState {
        blend: None,
        blend_constant: wgpu::Color::BLACK,
        color_writes: wgpu::ColorWrites::ALL,
        depth_write: true,
        depth_compare: wgpu::CompareFunction::LessEqual,
        primitive: wgpu::PrimitiveState::default(),
        alpha_test: None,
        stencil: None,
        depth_offset: None,
    }
}

/// `material`'s base look, lit (the lightmap at stage 3) or not (white
/// there): `t0 · t3`, alpha 1, with both classifications.
pub fn base_program(material: u16, lit: bool) -> Result<FogProgram, String> {
    let pixel = shaders::PixelShaderDef {
        texture_modes: texture_modes([TEX_PROJECT2D, TEX_NONE, TEX_NONE, TEX_PROJECT2D]),
        final_inputs_abcd: inputs(REG_T0, REG_T3, 0, 0),
        final_inputs_efg: final_efg(0, 0, ONE | CHAN_ALPHA, 0),
        ..Default::default()
    }
    .to_bytes();
    let look = PassLook {
        pass: TagId::NULL,
        implementation: 0,
        vertex_shader: TagId::NULL,
        textures: [
            base_stage(material),
            Stage::Empty,
            Stage::Empty,
            if lit { Stage::Lightmap } else { WHITE_STAGE },
        ],
        stage_states: Vec::new(),
        pixel_shader: pixel,
        vertex_constants: Vec::new(),
        vertex_defaults: Vec::new(),
        per_draw_inputs: false,
    };
    let pass = FogPass {
        look,
        state: state(),
        fog: FogWords::default(),
        planar: None,
    };
    let extra = base_coordinates();
    FogProgram::compile_rigid_pass(
        &pass,
        &world_program(&extra)?,
        Some(&rigid_program(&extra)?),
    )
}

/// Every material's base look for `draws` (each part's material and
/// whether it is lit): the programs by [`PassKey::Base`], or the first
/// that does not compile.
pub fn base_programs(
    draws: impl IntoIterator<Item = (u16, bool)>,
) -> Result<HashMap<PassKey, Vec<FogProgram>>, String> {
    let mut out = HashMap::new();
    for (material, lit) in draws {
        let key = PassKey::Base { material, lit };
        if let std::collections::hash_map::Entry::Vacant(slot) = out.entry(key) {
            slot.insert(vec![base_program(material, lit)?]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The base look compiles lit and unlit, and the program the layer
    /// pass shares reads only the view constants a camera gives.
    #[test]
    fn the_base_look_compiles_on_the_shared_position() {
        assert!(
            base_program(0, true).is_ok(),
            "{:?}",
            base_program(0, true).err()
        );
        assert!(
            base_program(3, false).is_ok(),
            "{:?}",
            base_program(3, false).err()
        );
        let code = world_program(&[]).unwrap();
        assert!(rigid_program(&base_coordinates()).is_ok());
        assert_eq!(
            crate::program::ungiven_view_read(&code, &crate::program::CAMERA_GIVEN),
            None
        );
        let unlit_stage = base_program(3, false).unwrap().program.stages[3];
        assert_eq!(unlit_stage, WHITE_STAGE);
        // Not a tag: its own stage kind, bound from the host's lookup.
        assert!(base_stage(7).takes_image() && WHITE_STAGE.takes_image());
        assert!(!Stage::Lightmap.takes_image());
        assert_eq!(
            base_program(7, true).unwrap().program.stages[0],
            Stage::Host(HostStage::BaseMap(7))
        );
        let keys = base_programs([(0, true), (0, false), (0, true)]).unwrap();
        let mut keys: Vec<PassKey> = keys.into_keys().collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                PassKey::Base {
                    material: 0,
                    lit: false
                },
                PassKey::Base {
                    material: 0,
                    lit: true
                }
            ]
        );
    }

    /// Each key has its own slot, dense in the order asked: a material's
    /// lit and unlit looks and its layer-3 pass never share one.
    #[test]
    fn every_pass_key_has_its_own_slot() {
        let mut keys = SetKeys::default();
        let lit = PassKey::Base {
            material: 0x8003,
            lit: true,
        };
        let unlit = PassKey::Base {
            material: 3,
            lit: false,
        };
        assert_eq!(keys.slot(lit), Ok(0));
        assert_eq!(keys.slot(unlit), Ok(1));
        assert_eq!(keys.slot(PassKey::Layer3(3)), Ok(2));
        assert_eq!(keys.slot(lit), Ok(0));
        assert_eq!(keys.key(1), Some(unlit));
        assert_eq!(keys.key(3), None);
    }

    /// An instruction naming two constants (or two inputs) is an error,
    /// not the encoder's panic.
    #[test]
    fn an_instruction_reading_two_constants_does_not_encode() {
        let two =
            Inst::mac(MacOp::Mad, Some(O::r(0)), Some(O::c(10)), Some(O::c(11))).to_temp(0, 0xf);
        assert_eq!(
            encode(&[two]).unwrap_err(),
            "instruction 0 reads c[10] and c[11]"
        );
        let inputs = Inst::mac(MacOp::Add, Some(O::v(0)), None, Some(O::v(1))).to_temp(0, 0xf);
        assert_eq!(
            encode(&[inputs]).unwrap_err(),
            "instruction 0 reads v0 and v1"
        );
        let same = Inst::mac(MacOp::Mul, Some(O::c(4)), Some(O::c(4)), None).to_temp(0, 0xf);
        assert!(encode(&[same]).is_ok());
    }
}
