//! The secondary map in the H1 effect shader embedded at part +0xb0.
use super::*;
use h1convert::shad::program::glass::ParticleCoordinates;

/// H1 0x10e0ed..0x10e223 selects a random sequence within each
/// successive phase, then clamps to the bitmap's last sequence. A single
/// H2 random range represents these phases when their effective range agrees.
pub(crate) fn maximum_sequences(
    [first, initial, looping, final_count]: [u16; 4],
    total: usize,
) -> Option<(u16, u16)> {
    let last = total.checked_sub(1)?;
    let mut start = usize::from(first);
    let mut chosen = None;
    for count in [initial, looping, final_count]
        .into_iter()
        .filter(|&n| n != 0)
    {
        let end = start + usize::from(count);
        let a = start.min(last);
        let b = (end - 1).min(last);
        let range = (u16::try_from(a).ok()?, u16::try_from(b - a + 1).ok()?);
        if chosen.is_some_and(|previous| previous != range) {
            return None;
        }
        chosen = Some(range);
        start = end;
    }
    chosen
}

#[derive(Clone, Copy)]
pub(crate) struct Mask<T> {
    pub bitmap: T,
    pub coordinates: ParticleCoordinates,
    motion: [Axis; 2],
}

#[derive(Clone, Copy)]
struct Axis {
    waveform: u8,
    period: f32,
    phase: f32,
    amplitude: f32,
}

impl Mask<H1TagId> {
    pub(crate) fn read(map: &H1Map, tag: &Tag) -> Result<Option<Self>> {
        let fail = |why: &str| h1convert::Error::no_rebuild(&map.header().name, &tag.name, why);
        if tag.group != h1map::Group::new(b"part") {
            return Err(fail("missing source effect shader for blend 6").into());
        }
        let d = tag_bytes(map, tag, b"part", H1_PART_SIZE)?;
        let half = |at| u16::from_le_bytes(d[at..at + 2].try_into().unwrap());
        let float = |at| f32::from_le_bytes(d[at..at + 4].try_into().unwrap());
        if half(0xd8) != 0 || half(0xde) != 0 {
            return Err(fail("unsupported blend 6 shader or sampler flags").into());
        }
        let reference = h1map::TagRef::read(d, 0xfc).ok_or_else(|| fail("short secondary map"))?;
        if reference.group != h1map::Group::new(b"bitm") {
            return Err(fail("secondary particle map is not a bitmap reference").into());
        }
        if reference.id.is_none() {
            return Ok(None);
        }
        let t = map
            .tag(reference.id)
            .map_err(|_| fail("invalid secondary bitmap datum"))?;
        if t.group != reference.group || half(0x10e) != 0 || float(0x13c) != 0.0 {
            return Err(fail("unsupported secondary bitmap, sampler or rotation").into());
        }
        let mask = h1map::bitmap::Bitmap::read(map, t).map_err(|e| fail(&e.to_string()))?;
        if !matches!(mask.images.as_slice(), [image] if image.kind == h1map::bitmap::ImageKind::D2)
        {
            return Err(fail(
                "secondary particle map requires image selection or a non-2D sampler",
            )
            .into());
        }
        let coordinates = match half(0x10c) {
            0 => ParticleCoordinates::Sprite,
            1 => ParticleCoordinates::Screen,
            _ => return Err(fail("unknown secondary particle coordinate mode").into()),
        };
        let axis = |at| -> Result<Axis> {
            let period = float(at + 4);
            if half(at) != 0
                || !matches!(half(at + 2), 2 | 6)
                || period <= 0.0
                || !period.is_finite()
            {
                return Err(fail("unknown secondary particle texture animation").into());
            }
            Ok(Axis {
                waveform: half(at + 2) as u8,
                period,
                phase: float(at + 8),
                amplitude: float(at + 12),
            })
        };
        Ok(Some(Self {
            bitmap: reference.id,
            coordinates,
            motion: [axis(0x110)?, axis(0x120)?],
        }))
    }

    pub(crate) fn bind(self, bitmap: (TagId, u16, u16)) -> Mask<(TagId, u16, u16)> {
        Mask {
            bitmap,
            coordinates: self.coordinates,
            motion: self.motion,
        }
    }
}

impl Mask<(TagId, u16, u16)> {
    pub(super) fn bind_postprocess(&self, r: &mut Record) -> Result<()> {
        let d = def(b"prt3")?
            .child("postprocess")
            .expect("prt3 postprocess");
        let mk = |name: &str, fields: &[(&str, Val)]| -> Result<Record> {
            let mut row = defs::default_record(d.child(name).expect("postprocess definition"));
            for (field, value) in fields {
                set(&mut row, field, value.clone())?;
            }
            Ok(row)
        };
        let (bitmap, w, h) = self.bitmap;
        set(r, "bitmaps[1].bitmap", Val::Datum(defs::Target::Id(bitmap)))?;
        set(r, "bitmaps[1].lod_offset", Val::Float(lod_offset(w, h)))?;
        set(r, "parameter_bitmaps[1].bitmap_index", Val::Int(1))?;
        set(r, "lods[0].layer_mask", Val::Int(1 << 15))?;
        set(
            r,
            "implementations[0].vertex_constant_animations",
            Val::Int(1 << 9),
        )?;
        let mut properties = h1convert::shad::TextureScroll {
            scale: [1.0; 2],
            offset: [None; 2],
        }
        .properties();
        for (i, axis) in self.motion.iter().enumerate() {
            // H1 0x56ff0: wave((seconds + phase) / period) * amplitude.
            // Both games interpolate the 1024-entry periodic lookup tables.
            let words = [1.0, axis.phase / axis.period, 0.0, 1.0].repeat(2);
            let function = Function {
                kind: FunctionType::Periodic,
                flags: 0,
                sub_types: [axis.waveform; 2],
                range: [0.0, axis.amplitude],
                unknown_0c: [0; 8],
                payload: words.into_iter().flat_map(f32::to_le_bytes).collect(),
            };
            properties[4 + i] = (axis.period, function.to_bytes());
        }
        let props = properties
            .into_iter()
            .map(|(period, f)| {
                mk(
                    "properties",
                    &[("input_b", Val::Float(period)), ("function", Val::Data(f))],
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let refs = (0..10)
            .map(|i| {
                mk(
                    "property_refs",
                    &[("property", Val::Int(i)), ("component", Val::Int(i))],
                )
            })
            .collect::<Result<Vec<_>>>()?;
        set(r, "properties", Val::Block(props))?;
        set(r, "property_refs", Val::Block(refs))?;
        set(
            r,
            "property_ranges",
            Val::Block(vec![mk(
                "property_ranges",
                &[("property_refs", Val::Int(10 << 9))],
            )?]),
        )?;
        set(
            r,
            "animations",
            Val::Block(vec![mk(
                "animations",
                &[
                    ("target", Val::Int(0xb1)),
                    ("param_1", Val::Int(1)),
                    ("property_range", Val::Int(0)),
                ],
            )?]),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::maximum_sequences;

    #[test]
    fn sprite_phases_clamp_to_the_source_bitmap() {
        assert_eq!(maximum_sequences([0, 1, 1, 0], 1), Some((0, 1)));
        assert_eq!(maximum_sequences([0, 4, 0, 0], 4), Some((0, 4)));
        assert_eq!(maximum_sequences([0, 1, 1, 0], 2), None);
    }
}
