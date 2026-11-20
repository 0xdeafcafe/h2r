//! Object widgets. H2 dispatches ant!, clwd and tdtl through its widget
//! table. MGS2 volumes go through the render-time attachment dispatcher.
use h1map::{Tag, TagRef};
use h1models::Converted;
use map::build::{Block, Fixup};
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use super::Converter;
use crate::{Error, Result, h1};

fn def(group: [u8; 4]) -> &'static StructDef {
    Defs::builtin()
        .group(FourCC(group))
        .expect("widget definition")
}

fn set(record: &mut Record, name: &str, value: Val) -> Result<()> {
    record
        .set(name, value)
        .map_err(|e| Error::Invalid(e.to_string()))
}

fn built(d: &StructDef, record: &Record) -> Result<Block> {
    defs::to_built(d, record, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

fn reference(b: &[u8], at: usize) -> Result<h1map::TagId> {
    Ok(TagRef::read(b, at)
        .ok_or(Error::Truncated("widget dependency"))?
        .id)
}

fn antenna(b: &[u8], vertices: &[&[u8]], bitmap: TagId, physics: TagId) -> Result<Block> {
    // 0x115e90 stores 0x20 bytes per joint after the 0x1c header,
    // including one terminal joint in the 0x2bc-byte runtime instance.
    if vertices.len() > 20 {
        return Err(Error::Invalid(format!(
            "antenna has {} segments (maximum 20)",
            vertices.len()
        )));
    }
    let d = def(*b"ant!");
    let mut r = defs::default_record(d);
    set(&mut r, "marker", Val::text(&h1::name32(b, 0)))?;
    set(&mut r, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap))?;
    set(&mut r, "physics", Val::tag(FourCC(*b"pphy"), physics))?;
    set(&mut r, "unknown_14", Val::Bytes(b[0x40..0xc4].to_vec()))?;
    let vertex = d.child("vertices").expect("antenna vertices");
    let rows = vertices
        .iter()
        .map(|bytes| {
            let mut row = defs::default_record(vertex);
            set(&mut row, "unknown_0", Val::Bytes(bytes.to_vec()))?;
            Ok(row)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut r, "vertices", Val::Block(rows))?;
    built(d, &r)
}

impl Converter<'_> {
    fn widget_bitmap(&mut self, id: h1map::TagId) -> Result<TagId> {
        if id.is_none() {
            return Ok(TagId::NULL);
        }
        let source = self.h1_tag(id)?;
        h1::tag_bytes(self.h1, source, b"bitm", 0)?;
        self.source_bitmap(Some(id))?
            .ok_or_else(|| Error::Invalid(source.name.clone()))
    }

    fn antenna(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        let b = h1::tag_bytes(self.h1, tag, b"ant!", 0xd0)?;
        let vertices = h1::elements(self.h1, b, 0xc4, 0x80)?;
        let bitmap = self.widget_bitmap(reference(b, 0x20)?)?;
        let physics = self
            .source_point_physics(reference(b, 0x30)?)?
            .unwrap_or(TagId::NULL);
        let root = antenna(b, &vertices, bitmap, physics)?;
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"ant!"),
                root,
                raw: vec![],
            },
        )
    }

    pub(super) fn native_widgets(&mut self, owner: &Tag) -> Result<Option<Fixup>> {
        let b = self.h1.tag_data(owner)?;
        let rows = h1::elements(self.h1, b, 0x14c, 0x20)?;
        let object = def(*b"scen").child("object").expect("shared object header");
        let widget = object.child("widgets").expect("object widgets");
        let mut bindings = Vec::new();
        for row in rows {
            let binding = TagRef::read(row, 0).ok_or(Error::Truncated("object widget"))?;
            let id = binding.id;
            if id.is_none() {
                return Err(Error::Invalid(format!(
                    "{} has a null widget row",
                    owner.name
                )));
            }
            let source = self.h1_tag(id)?;
            if source.group != binding.group {
                return Err(Error::Invalid(format!(
                    "{} widget references {} as {}",
                    owner.name, source.name, binding.group
                )));
            }
            let group = source.group.0.to_be_bytes();
            let converted = match &group {
                b"ant!" => self.antenna(source),
                b"mgs2" | b"glw!" => continue, // Bound at its own marker by object_attachments.
                _ => Err(Error::Invalid(format!(
                    "unsupported {} widget on {}",
                    source.group, owner.name
                ))),
            }
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &source.name, e))?;
            let mut binding = defs::default_record(widget);
            set(&mut binding, "widget", Val::tag(FourCC(group), converted))?;
            bindings.push(binding);
        }
        if bindings.is_empty() {
            return Ok(None);
        }
        let mut r = defs::default_record(object);
        set(&mut r, "widgets", Val::Block(bindings))?;
        built(object, &r)?
            .fixups
            .into_iter()
            .find(|f| f.offset == 0x9c)
            .map(Some)
            .ok_or_else(|| Error::Invalid("missing object widget block fixup".into()))
    }
}

fn real(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().expect("checked widget record"))
}

use tags::effects::function::{Function, FunctionType};

fn curve(kind: FunctionType, range: [f32; 2], sets: &[[f32; 3]]) -> Function {
    Function {
        kind,
        flags: u8::from(sets.len() == 2 && sets[0] != sets[1]),
        sub_types: [0; 2],
        range,
        unknown_0c: [0; 8],
        payload: sets
            .iter()
            .flatten()
            .flat_map(|word| word.to_le_bytes())
            .collect(),
    }
}

fn rgb(b: &[u8], at: usize) -> u32 {
    (0..3).fold(0xff, |packed, i| {
        (packed << 8) | (real(b, at + 4 * i).clamp(0.0, 1.0) * 255.0).round() as u32
    })
}

fn light_volume(b: &[u8], frame: &[u8], count: u16, bitmap: TagId) -> Result<Block> {
    if !(2..=256).contains(&count) {
        return Err(Error::Invalid(format!("light volume sample count {count}")));
    }
    if b[0x22] & 3 != 0 || b[0x6c..0x6e] != [0, 0] {
        return Err(Error::Invalid(
            "light volume needs HSV interpolation or a nonzero bitmap image".into(),
        ));
    }
    let d = def(*b"MGS2");
    let mut root = defs::default_record(d);
    set(
        &mut root,
        "distance_fade",
        Val::floats(&[real(b, 0x34), real(b, 0x38)]),
    )?;
    let layer = d.child("layers").expect("MGS2 layers");
    let mut r = defs::default_record(layer);
    set(&mut r, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap))?;
    set(&mut r, "sample_count", Val::Int(count.into()))?;
    // H1 0x97d10..0x97de1 applies the spacing exponent first. The H2
    // input table at +0x60 supplies that coordinate to all four curves.
    let input = layer
        .child("sample_positions")
        .expect("volume sample inputs");
    let mut positions = Vec::new();
    for index in 0..count {
        let x = (f32::from(index) / f32::from(count - 1)).powf(real(frame, 0x14));
        let mut sample = defs::default_record(input);
        set(&mut sample, "input", Val::floats(&[x, x]))?;
        positions.push(sample);
    }
    set(&mut r, "sample_positions", Val::Block(positions))?;
    let power = |range, exponent| curve(FunctionType::Exponent, range, &[[0.0, 1.0, exponent]; 2]);
    let start = real(frame, 0x10);
    let distance = curve(
        FunctionType::Identity,
        [start, start + real(frame, 0x18)],
        &[],
    );
    let radius = power([real(frame, 0x3c), real(frame, 0x40)], real(frame, 0x44));
    let alpha_set = |gain| {
        [
            real(frame, 0x68) * gain,
            real(frame, 0x78) * gain,
            real(frame, 0x8c),
        ]
    };
    let opacity = curve(
        FunctionType::Exponent,
        [0.0, 1.0],
        &[alpha_set(real(b, 0x3c)), alpha_set(real(b, 0x40))],
    );
    // 0x13bca5 lerps the packed colours at +4 and +0x10 in mode 2.
    let mut colour = power([f32::from_bits(rgb(frame, 0x6c)), 0.0], real(frame, 0x88));
    colour.flags = 0x20;
    colour.unknown_0c[4..].copy_from_slice(&rgb(frame, 0x7c).to_le_bytes());
    let angle = curve(FunctionType::Identity, [0.0, 1.0], &[]);
    for (field, f) in [
        ("data_10", distance),
        ("data_18", radius),
        ("data_20", opacity),
        ("data_28", colour),
        ("data_30", angle),
    ] {
        set(&mut r, field, Val::Data(f.to_bytes()))?;
    }
    set(&mut root, "layers", Val::Block(vec![r]))?;
    built(d, &root)
}

impl Converter<'_> {
    pub(super) fn light_volume(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        let b = h1::tag_bytes(self.h1, tag, b"mgs2", 0x14c)?;
        let frames = h1::elements(self.h1, b, 0x120, 0xb0)?;
        let [frame] = frames.as_slice() else {
            return Err(Error::Invalid(format!(
                "{} light-volume frames",
                frames.len()
            )));
        };
        let mut bitmap = reference(b, 0x5c)?;
        if bitmap.is_none() {
            // The source draw takes the rasterizer globals' glow datum at
            // +0x6c for this case (H1 0x64495).
            let mut globals = self
                .h1
                .tags()
                .iter()
                .filter(|t| t.group == h1map::Group::new(b"matg"));
            let tag = globals
                .next()
                .filter(|_| globals.next().is_none())
                .ok_or_else(|| {
                    Error::Invalid("light volume needs one rasterizer globals tag".into())
                })?;
            let rows = h1::elements(self.h1, self.h1.tag_data(tag)?, 0x134, 0x13c)?;
            let [raster] = rows.as_slice() else {
                return Err(Error::Invalid(
                    "light volume needs one rasterizer globals record".into(),
                ));
            };
            bitmap = reference(raster, 0x60)?;
            if bitmap.is_none() {
                return Err(Error::Invalid("missing rasterizer glow bitmap".into()));
            }
        }
        let bitmap = self.widget_bitmap(bitmap)?;
        let count = u16::from_le_bytes([b[0x6e], b[0x6f]]);
        let root = light_volume(b, frame, count, bitmap)?;
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"MGS2"),
                root,
                raw: vec![],
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antenna_relocates_its_marker_and_preserves_each_segment() {
        let mut source = [0; 0xd0];
        source[..5].copy_from_slice(b"whisk");
        source[0x94..0x98].copy_from_slice(&7.25f32.to_le_bytes());
        let first = [0x31; 0x80];
        let last = [0x72; 0x80];
        let result = antenna(&source, &[&first, &last], TagId(11), TagId(12)).unwrap();
        let r = defs::from_built(def(*b"ant!"), &result).unwrap();
        assert_eq!(r.at("marker"), Some(&Val::text("whisk")));
        assert!(matches!(
            r.at("bitmap"),
            Some(Val::Ref {
                target: defs::Target::Id(TagId(11)),
                ..
            })
        ));
        assert!(matches!(
            r.at("physics"),
            Some(Val::Ref {
                target: defs::Target::Id(TagId(12)),
                ..
            })
        ));
        assert_eq!(real(&result.bytes, 0x68), 7.25);
        assert_eq!(
            r.at("vertices[0].unknown_0"),
            Some(&Val::Bytes(first.to_vec()))
        );
        assert_eq!(
            r.at("vertices[1].unknown_0"),
            Some(&Val::Bytes(last.to_vec()))
        );
    }

    #[test]
    fn beam_samples_follow_source_spacing_radius_and_view_dependent_opacity() {
        let mut source = [0; 0x14c];
        let mut frame = [0; 0xb0];
        for (at, val) in [(0x3c, 0.5f32), (0x40, 1.0)] {
            source[at..at + 4].copy_from_slice(&val.to_le_bytes());
        }
        for (at, val) in [
            (0x10, -1.0f32),
            (0x14, 2.0),
            (0x18, 8.0),
            (0x3c, 1.0),
            (0x40, 5.0),
            (0x44, 1.0),
            (0x68, 1.0),
            (0x88, 1.0),
            (0x8c, 2.0),
        ] {
            frame[at..at + 4].copy_from_slice(&val.to_le_bytes());
        }
        let root = light_volume(&source, &frame, 3, TagId(19)).unwrap();
        let r = defs::from_built(def(*b"MGS2"), &root).unwrap();
        assert_eq!(
            r.at("layers[0].sample_positions[1].input"),
            Some(&Val::floats(&[0.25; 2]))
        );
        let evaluate = |field: &str, angle| {
            let Some(Val::Data(bytes)) = r.at(field) else {
                panic!("{field}")
            };
            let f = Function::parse(bytes).unwrap();
            f.range[0] + (f.range[1] - f.range[0]) * f.evaluate(0.25, angle)
        };
        assert_eq!(evaluate("layers[0].data_10", 0.0), 1.0);
        assert_eq!(evaluate("layers[0].data_18", 0.0), 2.0);
        assert_eq!(evaluate("layers[0].data_20", 0.0), 0.46875);
        assert_eq!(evaluate("layers[0].data_20", 1.0), 0.9375);
        assert!(light_volume(&source, &frame, 257, TagId(19)).is_err());
    }
}

impl Converter<'_> {
    pub(super) fn glow(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        let b = h1::tag_bytes(self.h1, tag, b"glw!", 0x154)?;
        let short = |at| u16::from_le_bytes([b[at], b[at + 1]]);
        let count = short(0x20);
        if !(2..=256).contains(&count)
            || short(0x22) != 1
            || short(0x24) != 1
            || b[0x28..0x2c] != 4u32.to_le_bytes()
            || [0x50, 0x60, 0x70, 0x80, 0x9c, 0xb0]
                .into_iter()
                .any(|at| short(at) != 0xffff)
            || real(b, 0x54) != 0.0
            || real(b, 0x74) <= 0.0
        {
            return Err(Error::Invalid(
                "unsupported glow distribution, movement or object output".into(),
            ));
        }
        let id = reference(b, 0x144)?;
        let bitmap = h1map::bitmap::Bitmap::read(self.h1, self.h1_tag(id)?)?;
        let [sequence] = bitmap.sequences.as_slice() else {
            return Err(Error::Invalid("glow needs one sprite sequence".into()));
        };
        let [sprite] = sequence.sprites.as_slice() else {
            return Err(Error::Invalid("glow needs one sprite".into()));
        };
        let [image] = bitmap.images.as_slice() else {
            return Err(Error::Invalid("glow needs one bitmap image".into()));
        };
        if sprite.image != 0 || sprite.bounds != [0., 1., 0., 1.] || image.width != image.height {
            return Err(Error::Invalid(
                "glow needs a square, full-image sprite".into(),
            ));
        }
        let profile = tags::effects::h1_glow::Glow {
            speed: real(b, 0x74),
            twist: real(b, 0x64),
            orbit: [real(b, 0x84), real(b, 0x88)],
            size: [real(b, 0xa0), real(b, 0xa4)],
            colours: [rgb(b, 0xb8), rgb(b, 0xc8)],
            fade: real(b, 0xf8),
            registration: sprite.registration,
        };
        let d = def(*b"MGS2");
        let mut root = defs::default_record(d);
        let mut layer = defs::default_record(d.child("layers").expect("volume layer"));
        set(&mut layer, "sample_count", Val::Int(count.into()))?;
        set(
            &mut layer,
            "bitmap",
            Val::tag(FourCC(*b"bitm"), self.widget_bitmap(id)?),
        )?;
        set(
            &mut layer,
            "unknown_68",
            Val::Bytes(profile.encode().to_vec()),
        )?;
        set(&mut root, "layers", Val::Block(vec![layer]))?;
        let root = built(d, &root)?;
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"MGS2"),
                root,
                raw: vec![],
            },
        )
    }
}
