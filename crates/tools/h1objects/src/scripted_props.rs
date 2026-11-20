//! Native object kinds and Halo 1 physics for the campaign's scripted props.

use h1map::{H1Map, Tag};
use map::build::Block;

use crate::h1::{self, Physics};
use crate::rebuild::{self, BodyWords, MassProperties, SphereSpec};
use crate::{Error, Result};

/// These Halo 1 tags are vehicles in the palette, but have no seats and
/// their scripts address them as generic objects. Halo 2 places the ring
/// as scenery in 00a and its movable crates as blocs in campaign.
pub fn replacement_group(path: &str) -> Option<[u8; 4]> {
    match path {
        "scenery\\halo\\halo" => Some(*b"scen"),
        "cinematics\\props\\b30_falling_box" => Some(*b"bloc"),
        _ => None,
    }
}

/// Crate motion and size from 05a's Covenant crate-space rigid body.
pub(crate) const CRATE_BODY: BodyWords = BodyWords {
    flags: 0,
    motion: 2,
    word_1e: 4,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptedVehicle {
    /// A fixed seat used by unit_open and vehicle_load_magic.
    Seat,
    /// A flier driven by recorded controls or a scripted animation.
    Flyer,
}

impl ScriptedVehicle {
    pub(crate) fn of(path: &str) -> Option<Self> {
        match path {
            "levels\\a10\\devices\\cryotube\\cryotube"
            | "levels\\a10\\devices\\chairs\\chair pilot\\chair pilot"
            | "levels\\a10\\devices\\chairs\\chair pod\\chair pod"
            | "vehicles\\lifepod_docked\\lifepod_docked" => Some(Self::Seat),
            "vehicles\\fighterbomber\\fighterbomber"
            | "vehicles\\lifepod\\lifepod"
            | "vehicles\\lifepod_entry\\lifepod_entry"
            | "scenery\\vehicles\\lifepod_atmosphere_entry\\lifepod_atmosphere_entry" => {
                Some(Self::Flyer)
            }
            _ => None,
        }
    }

    pub(crate) fn flight_tuning(self, b: &[u8]) -> Option<[f32; 11]> {
        if self != Self::Flyer {
            return None;
        }
        let f = |at: usize| f32::from_le_bytes(b[at..at + 4].try_into().expect("vehicle root"));
        let rate = crate::convert::TICKS_PER_SECOND;
        // Halo 2 reads signed forward/reverse limits and slide limits at
        // 0x000f32f5..0x000f3315 and their accelerations at
        // 0x000f3562..0x000f357c. The axis limits are positive magnitudes.
        // Halo 1 gives distances per tick and changes of speed per tick.
        // Turn angles and rates use the same degree fields (0x000eef80).
        Some([
            f(0x2f8) * rate,
            f(0x2fc) * rate,
            f(0x300) * rate * rate,
            f(0x304) * rate * rate,
            f(0x308),
            f(0x30c),
            f(0x314),
            f(0x330) * rate,
            f(0x334) * rate,
            f(0x338) * rate * rate,
            f(0x33c) * rate * rate,
        ])
    }
}

pub(crate) fn mass_properties(h1: &H1Map, tag: &Tag) -> Result<(Physics, MassProperties)> {
    let physics = Physics::read(h1, tag)?;
    let root = h1::tag_bytes(h1, tag, b"phys", h1::PHYS_SIZE)?;
    // Halo 1 stores its tensor and inverse in that order. Both retail
    // flyer pairs store symmetric matrices that multiply to identity.
    let matrices = h1::elements(h1, root, 0x5c, 0x24)?;
    let tensor = matrices
        .first()
        .ok_or_else(|| Error::Invalid(format!("{}: no inertia tensor", tag.name)))?;
    let inertia = std::array::from_fn(|row| {
        std::array::from_fn(|col| {
            let at = (row * 3 + col) * 4;
            f32::from_le_bytes(tensor[at..at + 4].try_into().expect("inertia word"))
        })
    });
    let mass = MassProperties {
        centre: physics.center_of_mass,
        inertia,
    };
    Ok((physics, mass))
}

/// The fighterbomber has no coll. Its 18 source mass points give it
/// native Havok spheres on the fuselage and wings, in the model's rest
/// pose. The compound body has the source's total mass and inertia.
pub(crate) fn mass_point_physics(
    model: &h1_assets::models::h1::Model,
    physics: &Physics,
    mass: &MassProperties,
) -> Result<Block> {
    use crate::physics::Transform;

    let world = h1_assets::models::mode::world_transforms(
        &model
            .nodes
            .iter()
            .map(|n| (n.parent, n.rotation, n.translation))
            .collect::<Vec<_>>(),
    );
    let spheres = physics
        .mass_points
        .iter()
        .filter(|p| p.radius.is_finite() && p.radius > 0.0)
        .map(|p| {
            let (r, t) = world
                .get(p.model_node as usize)
                .copied()
                .unwrap_or((Transform::IDENTITY.r, Transform::IDENTITY.t));
            SphereSpec {
                centre: Transform { r, t }.point(p.position),
                radius: p.radius,
                mass: p.mass,
            }
        })
        .collect::<Vec<_>>();
    rebuild::compound_sphere_physics(&spheres, physics.mass, *mass, "hard_metal_thin_hum")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_aircraft_keep_the_source_axes_and_rates() {
        let mut bytes = vec![0; 0x340];
        for (offset, value) in [
            (0x2f8, 0.5f32),
            (0x300, 0.05),
            (0x308, 30.0),
            (0x30c, -30.0),
            (0x330, 0.1),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let tuning = ScriptedVehicle::Flyer.flight_tuning(&bytes).unwrap();
        assert_eq!(
            [tuning[0], tuning[2], tuning[4], tuning[5], tuning[7]],
            [15.0, 45.0, 30.0, -30.0, 3.0]
        );
        assert!(ScriptedVehicle::Seat.flight_tuning(&bytes).is_none());
    }
}
