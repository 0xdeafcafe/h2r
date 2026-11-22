//! Scenario light placements for the offline comparison. The sphere's fitted
//! coefficient includes the source colour and intensity in lightmap units.
use anyhow::{Context, Result, bail, ensure};
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};
use tags::effects::{
    self, Ctx, Rec,
    light::{Light, LightKind},
};

#[derive(Clone, Debug)]
pub(super) struct PlacedLamp {
    pub name: String,
    pub position: [f32; 3],
    /// Reach in world units. The emitting point itself has zero area.
    pub radius: f32,
}

impl PlacedLamp {
    pub fn load<R: Read + Seek>(map: &mut Map<R>, index: usize, bsp: usize) -> Result<Self> {
        let scenario = map
            .scenario()
            .and_then(|tag| tag.location)
            .context("scenario has no local payload")?;
        let placement = Placement::read(&mut Ctx::new(map), scenario.address, index, bsp)?;
        let tag = map.tag(placement.definition)?;
        let name = tag.name.clone().unwrap_or_else(|| tag.id.to_string());
        let light: Light = effects::read(map, placement.definition)
            .with_context(|| format!("placed:{index} {name}: reading the light definition"))?;
        placement
            .lamp(name, &light)
            .with_context(|| format!("placed:{index}"))
    }

    /// Receiver-to-lamp unit vector, exclusive ray distance, and strength at
    /// that distance. Linear attenuation is the reconstruction hypothesis.
    /// The fit decides whether a level has evidence for it. At the emitter
    /// itself the direction is undefined and the sample contributes zero.
    pub fn sample(&self, receiver: [f32; 3]) -> Option<([f32; 3], f32, f32)> {
        let delta: [f32; 3] = std::array::from_fn(|i| self.position[i] - receiver[i]);
        let distance = delta[0].hypot(delta[1]).hypot(delta[2]);
        if distance > 0.0 && distance < self.radius {
            Some((
                delta.map(|d| d / distance),
                distance,
                1.0 - distance / self.radius,
            ))
        } else {
            None
        }
    }
}

struct Placement {
    definition: TagId,
    position: [f32; 3],
    scale: f32,
    custom_geometry: bool,
    lightmap_mode: u16,
}

impl Placement {
    // The Xbox placement and palette strides are recorded in
    // docs/formats/effects.md. The Gen2 ScenarioLightBlock's common object
    // header and trailing light fields follow the TagTool Gen2 Scenario
    // schema. Its geometry flags and lightmap mode start at +3e and +40.
    fn read(ctx: &mut Ctx<'_>, address: u32, index: usize, bsp: usize) -> Result<Self> {
        let header = ctx.mem.read(address, 0xf8)?;
        let rows = ctx.block(Rec(&header).block(0xe8), 0x6c)?;
        let row = Rec(rows.get(index).with_context(|| {
            format!(
                "placed:{index} outside {} scenario light placements",
                rows.len()
            )
        })?);
        ensure!(
            super::placement_in_bsp(row.0, bsp)?,
            "placed:{index} does not belong to BSP {bsp}"
        );
        let palette = ctx.block(Rec(&header).block(0xf0), 0x28)?;
        let slot = row.i16(0);
        let entry = usize::try_from(slot)
            .ok()
            .and_then(|i| palette.get(i))
            .with_context(|| format!("placed:{index} has invalid light palette index {slot}"))?;
        let reference = Rec(entry).tag_ref(0);
        ensure!(
            !reference.is_null() && reference.group == FourCC(*b"ligh"),
            "placed:{index} has no ligh reference in palette slot {slot}"
        );
        Ok(Self {
            definition: reference.id,
            position: [row.f32(8), row.f32(12), row.f32(16)],
            scale: row.f32(0x20),
            custom_geometry: row.u16(0x3e) & 1 != 0,
            lightmap_mode: row.u16(0x40),
        })
    }

    fn lamp(&self, name: String, light: &Light) -> Result<PlacedLamp> {
        ensure!(!self.custom_geometry, "{name} has a custom placement shape");
        ensure!(
            light.kind == LightKind::Sphere,
            "{name} is a {} light; the local split requires a sphere",
            light.kind.name()
        );
        ensure!(light.bitmap.is_null(), "{name} has a textured light gel");

        // Gen2 Light +a4 is the definition's default lightmap setting.
        // Placement mode 0 inherits it; modes 1..3 specify it directly.
        let mode = match self.lightmap_mode {
            0 => Rec(&light.raw).u16(0xa4),
            override_mode => override_mode - 1,
        };
        match mode {
            0 => bail!("{name} is authored as dynamic-only and has no baked lamp to split"),
            1 | 2 => {}
            other => bail!("{name} has unknown lightmap setting {other}"),
        }

        ensure!(
            self.position.iter().all(|x| x.is_finite()),
            "{name} has a non-finite placement position"
        );
        ensure!(
            self.scale.is_finite() && self.scale >= 0.0,
            "{name} has invalid placement scale {}",
            self.scale
        );
        let scale = if self.scale == 0.0 { 1.0 } else { self.scale };
        let radius = light.radius * scale;
        ensure!(
            radius.is_finite() && radius > 0.0,
            "{name} has invalid sphere range {radius}"
        );
        Ok(PlacedLamp {
            name,
            position: self.position,
            radius,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::effects::{EffectTag, SliceMemory, light::LIGHT_SIZE};

    fn light(radius: f32, mode: u16) -> Light {
        let mut bytes = [0; LIGHT_SIZE];
        bytes[0x18..0x1c].copy_from_slice(&radius.to_le_bytes());
        bytes[0x7c..0x80].copy_from_slice(&u32::MAX.to_le_bytes()); // no gel
        bytes[0xa4..0xa6].copy_from_slice(&mode.to_le_bytes());
        let mut mem = SliceMemory::new(0x8000);
        let root = mem.push(&bytes);
        Light::decode_at(&mut Ctx::new(&mut mem), root).unwrap()
    }

    #[test]
    fn placement_identity_transform_and_lightmap_override() {
        let mut mem = SliceMemory::new(0x4000);
        let mut rows = [0; 2 * 0x6c];
        let row = &mut rows[0x6c..];
        row[..2].copy_from_slice(&1_i16.to_le_bytes());
        for (axis, value) in [3.0_f32, -4.0, 5.0].into_iter().enumerate() {
            row[8 + 4 * axis..12 + 4 * axis].copy_from_slice(&value.to_le_bytes());
        }
        row[0x20..0x24].copy_from_slice(&2_f32.to_le_bytes());
        row[0x40..0x42].copy_from_slice(&3_u16.to_le_bytes()); // lightmaps only
        let at_rows = mem.push(&rows);
        let mut palette = [0; 0x50];
        palette[0x28..0x2c].copy_from_slice(&u32::from_be_bytes(*b"ligh").to_le_bytes());
        palette[0x2c..0x30].copy_from_slice(&17_u32.to_le_bytes());
        let at_palette = mem.push(&palette);
        let mut header = [0; 0xf8];
        for (offset, value) in [(0xe8, 2), (0xec, at_rows), (0xf0, 2), (0xf4, at_palette)] {
            header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let address = mem.push(&header);
        let mut ctx = Ctx::new(&mut mem);
        let mut placement = Placement::read(&mut ctx, address, 1, 0).unwrap();
        assert_eq!(placement.definition, TagId(17));
        let mut def = light(4.0, 0); // the placement overrides dynamic-only
        let lamp = placement.lamp("fixture".into(), &def).unwrap();
        assert_eq!(lamp.position, [3., -4., 5.]);
        assert_eq!(lamp.radius, 8.0);
        assert!(Placement::read(&mut ctx, address, 2, 0).is_err());
        assert!(Placement::read(&mut ctx, address, 1, 1).is_err());
        placement.lightmap_mode = 0;
        assert!(placement.lamp("fixture".into(), &def).is_err());
        placement.lightmap_mode = 2;
        def.radius = 0.0;
        assert!(placement.lamp("fixture".into(), &def).is_err());
        def.kind = LightKind::Frustum;
        assert!(placement.lamp("fixture".into(), &def).is_err());
    }

    #[test]
    fn rays_end_at_the_lamp_and_only_receivers_inside_its_sphere_get_direct_light() {
        let lamp = PlacedLamp {
            name: "fixture".into(),
            position: [0.0, 3.0, 4.0],
            radius: 10.0,
        };
        let (direction, distance, gain) = lamp.sample([0.0; 3]).unwrap();
        assert_eq!(direction, [0.0, 0.6, 0.8]);
        assert_eq!((distance, gain), (5.0, 0.5));
        assert!(lamp.sample([0.0, 3.0, 14.0]).is_none());
        assert!(lamp.sample([0.0, 3.0, -20.0]).is_none());
        assert!(lamp.sample(lamp.position).is_none());
    }

    #[test]
    fn removing_one_red_fixture_preserves_the_blue_fill_and_other_room_light() {
        use super::super::solve::{Source, Vertex, split_vertices};

        // A red lamp three units over a floor. The first receiver is halfway
        // to the radius, the next is in shade and the last is across the room.
        let lamp = PlacedLamp {
            name: "red ceiling lamp".into(),
            position: [0.0, 0.0, 3.0],
            radius: 6.0,
        };
        let floor = |x, retail| Vertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            retail,
        };
        let receivers = [
            floor(0.0, [112, 20, 79]),
            floor(1.0, [12, 20, 79]),
            floor(10.0, [57, 35, 40]),
        ];
        // A strip halfway up the wall casts on receiver 1. A second roof
        // lies beyond the lamp and would falsely shade receiver 0 if rays
        // continued past the source.
        let visible = |origin: [f32; 3], dir: [f32; 3], max_t: f32| {
            let blocked = [(1.5, 0.4..0.7), (4.0, -0.1..0.1)]
                .iter()
                .any(|(z, strip)| {
                    let t = (z - origin[2]) / dir[2];
                    t > 0.0 && t < max_t && strip.contains(&(origin[0] + dir[0] * t))
                });
            [!blocked; 2]
        };
        let split = split_vertices(
            &receivers,
            &[],
            &Source::Placed(lamp),
            &visible,
            [200., 0., 0.],
            1,
        )
        .unwrap();
        let rgb = &split.texels;
        assert_eq!(rgb[0].lamp_off, [12, 20, 79]);
        assert_eq!(rgb[1].lamp_off, [12, 20, 79]);
        assert_eq!(rgb[2].lamp_off, [57, 35, 40]);
        for (sample, vertex) in rgb.iter().zip(receivers) {
            assert_eq!(sample.lamp_on, vertex.retail);
        }
    }
}
