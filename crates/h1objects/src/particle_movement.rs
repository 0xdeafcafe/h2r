//! Point physics for attached Halo 1 particle systems.

use h1map::{H1Map, TagId as H1TagId};
use map::{FourCC, build::Block};
use tags::defs::{self, Defs, Record, Val};
use tags::effects::function::{Function, FunctionType};
use tags::effects::h1_attachment::PointPhysics;

use crate::{Error, Result, h1};

/// The source's point-physics tag supplies compiled buoyancy and effective
/// mass, plus the original friction and restitution coefficients.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Halo1Physics {
    flags: u32,
    mass: f32,
    air_buoyancy: f32,
    liquid_buoyancy: f32,
    air_drag: f32,
    liquid_drag: f32,
    friction: f32,
    bounce: f32,
}

impl Halo1Physics {
    pub(crate) fn read(map: &H1Map, id: H1TagId) -> Result<Self> {
        let b = h1::tag_bytes(map, map.tag(id)?, b"pphy", 0x34)?;
        Self::from_bytes(b)
    }

    fn from_bytes(b: &[u8]) -> Result<Self> {
        let word = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"));
        let f = |at| f32::from_bits(word(at));
        let p = Self {
            flags: word(0),
            mass: f(4),
            liquid_buoyancy: f(8),
            air_buoyancy: f(0xc),
            air_drag: f(0x24),
            liquid_drag: f(0x28),
            friction: f(0x2c),
            bounce: f(0x30),
        };
        if [
            p.mass,
            p.liquid_buoyancy,
            p.air_buoyancy,
            p.air_drag,
            p.liquid_drag,
            p.friction,
            p.bounce,
        ]
        .into_iter()
        .any(|v| !v.is_finite())
        {
            return Err(Error::Invalid("non-finite Halo 1 point physics".into()));
        }
        Ok(p)
    }

    /// E (Halo 1) 0x00080886 applies the signed buoyancy times
    /// [0x1fb648] * 30² in world units per second squared.
    fn point_physics(self, type_flags: u32, radius: f32) -> PointPhysics {
        const GRAVITY: f32 = 0.0035651792 * 30.0 * 30.0;
        PointPhysics {
            flags: self.flags,
            particle_type_flags: type_flags,
            mass: self.mass,
            acceleration: [self.air_buoyancy * GRAVITY, self.liquid_buoyancy * GRAVITY],
            drag: [self.air_drag, self.liquid_drag],
            radius,
        }
    }

    /// Build the native collision stage. Its two constants at
    /// 0x002b939f are tangential friction and normal restitution.
    pub(crate) fn movement(self, type_flags: u32, radius: f32) -> Result<Block> {
        let def = Defs::builtin().group(FourCC(*b"pmov")).expect("pmov");
        let stage_def = def.child("movements").expect("pmov stages");
        let parameter_def = stage_def.child("parameters").expect("pmov parameters");
        let mut root = defs::default_record(def);
        let mut stage = defs::default_record(stage_def);
        let mut parameters = Vec::new();
        let words = self.point_physics(type_flags, radius).to_words();
        for (i, (coefficient, extra)) in [self.friction, self.bounce]
            .into_iter()
            .zip(words)
            .enumerate()
        {
            let bytes = Function {
                kind: FunctionType::Constant,
                flags: 0,
                sub_types: [0; 2],
                range: [coefficient; 2],
                unknown_0c: [extra[0].to_le_bytes(), extra[1].to_le_bytes()]
                    .concat()
                    .try_into()
                    .expect("eight bytes"),
                payload: [extra[2].to_le_bytes(), extra[3].to_le_bytes()].concat(),
            }
            .to_bytes();
            let mut parameter = defs::default_record(parameter_def);
            set(&mut parameter, "id", Val::Int(i as i64))?;
            set(&mut parameter, "modifier_input", Val::Int(-1))?;
            set(&mut parameter, "function", Val::Data(bytes))?;
            parameters.push(parameter);
        }

        // 0x002b928a tests bit 0 for structure collision. The extra object
        // mask is selected by the source point-physics flag at 0x0008094c.
        let flags: u32 = 1 | if self.flags & 2 != 0 { 0x10 } else { 0 };
        set(
            &mut root,
            "unknown_8",
            Val::Bytes(flags.to_le_bytes().to_vec()),
        )?;
        set(&mut stage, "kind", Val::Int(1))?;
        set(
            &mut stage,
            "unknown_c",
            Val::Bytes([3u32.to_le_bytes(), [0; 4]].concat()),
        )?;
        set(&mut stage, "parameters", Val::Block(parameters))?;
        set(&mut root, "movements", Val::Block(vec![stage]))?;
        defs::to_built(def, &root, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
    }
}

fn set(r: &mut Record, path: &str, value: Val) -> Result<()> {
    r.set(path, value)
        .map_err(|e| Error::Invalid(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_physics_keeps_the_native_collision_and_extension_coefficients() {
        let mut bytes = [0; 0x34];
        bytes[0] = 0x12;
        for (at, value) in [
            (4, 59.3067_f32),
            (8, 0.999),
            (0xc, 0.375),
            (0x24, 200.0),
            (0x28, 15000.0),
            (0x2c, 0.4),
            (0x30, 0.2),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        let physics = Halo1Physics::from_bytes(&bytes).unwrap();
        let block = physics.movement(0x11, 2.0).unwrap();
        let def = Defs::builtin().group(FourCC(*b"pmov")).unwrap();
        let record = defs::from_built(def, &block).unwrap();
        assert_eq!(record.at("movements[0].kind"), Some(&Val::Int(1)));
        let mut curves = [[0; 4]; 2];
        for (i, curve) in curves.iter_mut().enumerate() {
            let Some(Val::Data(blob)) =
                record.at(&format!("movements[0].parameters[{i}].function"))
            else {
                panic!("missing collision curve");
            };
            let f = Function::parse(blob).unwrap();
            assert_eq!(f.range, [[0.4; 2], [0.2; 2]][i]);
            for (word, bytes) in curve.iter_mut().zip(blob[12..].chunks_exact(4)) {
                *word = u32::from_le_bytes(bytes.try_into().unwrap());
            }
        }
        let decoded = PointPhysics::from_words(curves).unwrap();
        assert_eq!(decoded, physics.point_physics(0x11, 2.0));
        assert_eq!(decoded.drag, [200.0, 15000.0]);
        assert!(decoded.acceleration[0] > 1.2);
    }
}
