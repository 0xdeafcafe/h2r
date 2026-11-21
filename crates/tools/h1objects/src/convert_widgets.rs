//! Object widgets. H2 dispatches ant!, clwd and tdtl through its widget
//! table. MGS2 volumes go through the render-time attachment dispatcher.
use h1map::{Tag, TagRef};
use h1_assets::models::Converted;
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
        let mut has_flag = false;
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
            if &group == b"flag" {
                if has_flag {
                    return Err(h1convert::Error::no_rebuild(
                        &self.h1.header().name,
                        &source.name,
                        format!("{} has multiple cloth widgets", owner.name),
                    )
                    .into());
                }
                has_flag = true;
                let panels = self.flag(source).map_err(|e| {
                    h1convert::Error::no_rebuild(&self.h1.header().name, &source.name, e)
                })?;
                for id in panels {
                    let mut row = defs::default_record(widget);
                    set(&mut row, "widget", Val::tag(FourCC(*b"clwd"), id))?;
                    bindings.push(row);
                }
                continue;
            }
            let converted = match &group {
                b"ant!" => self.antenna(source),
                b"elec" => self.lightning(source),
                b"mgs2" | b"glw!" => continue, // Bound at its own marker by object_attachments.
                _ => Err(Error::Invalid(format!(
                    "unsupported {} widget on {}",
                    source.group, owner.name
                ))),
            }
            .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &source.name, e))?;
            let mut binding = defs::default_record(widget);
            set(
                &mut binding,
                "widget",
                Val::tag(
                    FourCC(if &group == b"elec" { *b"tdtl" } else { group }),
                    converted,
                ),
            )?;
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

fn short(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn lightning(b: &[u8], markers: &[&[u8]], shader: &[u8], bitmap: TagId) -> Result<Block> {
    use tags::effects::h1_lightning::{Endpoint, MAGIC};
    let bolts = short(b, 2);
    if !(1..=3).contains(&bolts)
        || !(2..=64).contains(&markers.len())
        || short(b, 0x2c) != 0
        || short(b, 0x32) != 0
        || short(b, 0x2e) > 4
        || short(b, 0x30) > 4
    {
        return Err(Error::Invalid(
            "unsupported lightning bolt count, endpoints or modulation".into(),
        ));
    }
    let blend = short(shader, 0x28);
    if !matches!(blend, 2 | 3) || short(shader, 0x24) != 1 || short(shader, 0x2a) != 3 {
        return Err(Error::Invalid(format!(
            "unsupported lightning material {blend}"
        )));
    }
    let mut endpoints = Vec::new();
    let mut depth = 2;
    for marker in markers {
        let subdivisions = short(marker, 0x24);
        if subdivisions > 6 || short(marker, 0x20) & !1 != 0 {
            return Err(Error::Invalid(
                "unsupported lightning subdivision or endpoint flag".into(),
            ));
        }
        depth = depth.max(subdivisions);
        endpoints.push(Endpoint {
            end: short(marker, 0x20) & 1 != 0,
            depth: subdivisions as u8,
            jitter: std::array::from_fn(|i| real(marker, 0x74 + 4 * i)),
            displacement: real(marker, 0x80),
            width: real(marker, 0x84),
            argb: std::array::from_fn(|i| real(marker, 0x88 + 4 * i)),
        });
    }
    let d = def(*b"tdtl");
    let mut r = defs::default_record(d);
    set(
        &mut r,
        "marker",
        Val::text(&h1::name32(markers[0], 0).to_ascii_lowercase()),
    )?;
    let layer = d.child("layers").expect("lightning layers");
    let mut layers = Vec::new();
    for bolt in 0..bolts {
        let mut profile = curve(FunctionType::Constant, [1.; 2], &[]).to_bytes();
        profile[0xc..0x10].copy_from_slice(&MAGIC);
        profile[0x10..0x14].copy_from_slice(&[
            bolt as u8,
            h1convert::shad::halo2_change_colour_slot(short(b, 0x30))
                .map_or(0, |slot| slot as u8 + 1),
            blend as u8,
            short(b, 0x2e) as u8,
        ]);
        profile.extend_from_slice(&(endpoints.len() as u32).to_le_bytes());
        for endpoint in &endpoints {
            endpoint.encode(&mut profile);
        }
        let mut row = defs::default_record(layer);
        set(&mut row, "flags", Val::Flags(1))?;
        set(&mut row, "subdivision", Val::Int((depth - 2).into()))?;
        set(&mut row, "length", Val::Float(1.))?;
        set(&mut row, "copies", Val::Int(1))?;
        set(&mut row, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap))?;
        set(
            &mut row,
            "passes",
            Val::Block(vec![defs::default_record(
                layer.child("passes").expect("lightning pass"),
            )]),
        )?;
        set(&mut row, "displacement", Val::Data(profile))?;
        layers.push(row);
    }
    set(&mut r, "layers", Val::Block(layers))?;
    built(d, &r)
}

impl Converter<'_> {
    pub(super) fn lightning(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(&id) = self.done.get(&tag.id.0) {
            return Ok(id);
        }
        let b = h1::tag_bytes(self.h1, tag, b"elec", 0xb0)?;
        let markers = h1::elements(self.h1, b, 0x98, 0xe4)?;
        let shaders = h1::elements(self.h1, b, 0xa4, 0xb4)?;
        let [shader] = shaders.as_slice() else {
            return Err(Error::Invalid(format!(
                "lightning has {} shaders",
                shaders.len()
            )));
        };
        let id = reference(b, 0x34)?;
        if id.is_none() {
            return Err(Error::Invalid("lightning has no bitmap".into()));
        }
        let bitmap = self.widget_bitmap(id)?;
        let root = lightning(b, &markers, shader, bitmap)?;
        self.cached(
            tag,
            Converted {
                group: FourCC(*b"tdtl"),
                root,
                raw: vec![],
            },
        )
    }
}

struct FlagGrid {
    width: usize,
    height: usize,
    spacing: [f32; 2],
    physics: tags::effects::h1_attachment::PointPhysics,
    contact: [f32; 2],
}

impl FlagGrid {
    fn panel(&self, upper: bool, marker: &str, shader: TagId) -> Result<Block> {
        let seam = (self.height - 1) / 2;
        let w = self.width;
        let h = if upper { self.height - seam } else { seam + 1 };
        if w * h > 128 {
            return Err(Error::Invalid(format!(
                "cloth panel has {} vertices (maximum 128)",
                w * h
            )));
        }
        let d = def(*b"clwd");
        let mut r = defs::default_record(d);
        // 0x1173ae assigns vertex 0 to a single marker. Number both
        // panels from their own mast anchor toward the overlap.
        let source_y = |row: usize| if upper { self.height - 1 - row } else { row };
        set(&mut r, "marker", Val::text(marker))?;
        set(&mut r, "shader", Val::tag(FourCC(*b"shad"), shader))?;
        set(&mut r, "columns", Val::Int(w as i64))?;
        set(&mut r, "rows", Val::Int(h as i64))?;
        set(&mut r, "spacing", Val::floats(&self.spacing))?;
        set(&mut r, "iterations", Val::Int(1))?;
        // The H1 integrator owns these two values on marked cloth panels.
        set(&mut r, "damping", Val::Float(self.contact[0]))?;
        set(&mut r, "wind_perpendicular", Val::Float(self.contact[1]))?;
        let p = self.physics;
        let key = if upper { *b"H1F1" } else { *b"H1F0" };
        let mut profile = key.to_vec();
        for f in [
            p.mass,
            p.acceleration[0],
            p.acceleration[1],
            p.drag[0],
            p.drag[1],
        ] {
            profile.extend_from_slice(&f.to_le_bytes());
        }
        set(&mut r, "unknown_34", Val::Bytes(profile))?;
        let mut vertices = Vec::new();
        for row in 0..h {
            let y = source_y(row);
            for x in 0..w {
                let mut v = defs::default_record(d.child("vertices").expect("cloth vertices"));
                set(
                    &mut v,
                    "position",
                    Val::floats(&[x as f32 * self.spacing[0], 0., y as f32 * self.spacing[1]]),
                )?;
                set(
                    &mut v,
                    "uv",
                    Val::floats(&[
                        x as f32 / (w - 1) as f32,
                        y as f32 / (self.height - 1) as f32,
                    ]),
                )?;
                vertices.push(v);
            }
        }
        set(&mut r, "vertices", Val::Block(vertices))?;
        // Preserve one winding and the full cloth's texture coordinates
        // through both halves, even though their in-memory rows run apart.
        let idx = |x: usize, y: usize| (if upper { y } else { h - 1 - y }) * w + x;
        let mut triangles = Vec::new();
        let mut strip = Vec::new();
        let mut edges = std::collections::BTreeSet::new();
        for y in 0..h - 1 {
            if y > 0 {
                strip.push(idx(0, y));
            }
            for x in 0..w {
                let a = idx(x, y);
                let c = idx(x, y + 1);
                strip.extend([a, c]);
                if x + 1 == w {
                    continue;
                }
                let b = idx(x + 1, y);
                let e = idx(x + 1, y + 1);
                triangles.extend([a, c, b, b, c, e]);
                for (i, j) in [(a, b), (a, c), (c, e), (b, e), (a, e), (b, c)] {
                    edges.insert((i.min(j), i.max(j)));
                }
            }
            strip.push(idx(w - 1, y + 1));
        }
        for (field, indices) in [("triangles", triangles), ("strip", strip)] {
            let mut rows = Vec::new();
            for i in indices {
                let mut row = defs::default_record(d.child(field).expect("cloth indices"));
                set(&mut row, "index", Val::Int(i as i64))?;
                rows.push(row);
            }
            set(&mut r, field, Val::Block(rows))?;
        }
        let mut constraints = Vec::new();
        for (a, b) in edges {
            let dx = (a % w).abs_diff(b % w) as f32 * self.spacing[0];
            let dz = (a / w).abs_diff(b / w) as f32 * self.spacing[1];
            let mut c = defs::default_record(d.child("constraints").expect("cloth constraints"));
            set(&mut c, "a", Val::Int(a as i64))?;
            set(&mut c, "b", Val::Int(b as i64))?;
            set(&mut c, "rest_length", Val::Float(dx.hypot(dz)))?;
            set(&mut c, "weight", Val::Float(1.))?;
            constraints.push(c);
        }
        set(&mut r, "constraints", Val::Block(constraints))?;
        built(d, &r)
    }
}

impl Converter<'_> {
    fn flag(&mut self, tag: &Tag) -> Result<[TagId; 2]> {
        if let Some(ids) = self.flag_panels.get(&tag.id.0) {
            return Ok(*ids);
        }
        let b = h1::tag_bytes(self.h1, tag, b"flag", 0x60)?;
        let width = usize::from(short(b, 0xc));
        let height = usize::from(short(b, 0xe));
        if width < 2 || height < 3 || width * height >= 225 || b[..0xc].iter().any(|v| *v != 0) {
            return Err(Error::Invalid(format!(
                "unsupported cloth grid or flags ({width} by {height})"
            )));
        }
        let rows = h1::elements(self.h1, b, 0x54, 0x34)?;
        let mut anchors = [None, None];
        for row in rows {
            let idx = short(row, 0) as usize;
            let slot = if idx == 0 {
                0
            } else if idx == height - 1 {
                1
            } else {
                return Err(Error::Invalid(format!("unsupported cloth pin {idx}")));
            };
            if anchors[slot].replace(h1::name32(row, 0x14)).is_some() {
                return Err(Error::Invalid(format!("duplicate cloth anchor {idx}")));
            }
        }
        let [Some(lower), Some(upper)] = anchors else {
            return Err(Error::Invalid("cloth needs both mast pins".into()));
        };
        let source_physics =
            crate::particle_movement::Halo1Physics::read(self.h1, reference(b, 0x28)?)?;
        let physics = source_physics.point_physics(0, 0.02);
        if real(b, 0x38) != 0. {
            return Err(Error::Invalid("unsupported cloth wind noise".into()));
        }
        if physics.flags != 0 {
            return Err(Error::Invalid(format!(
                "unsupported cloth point-physics flags {:#x}",
                physics.flags
            )));
        }
        let grid = FlagGrid {
            width,
            height,
            spacing: [real(b, 0x10), real(b, 0x14)],
            physics,
            contact: source_physics.contact(),
        };
        let mut shader = |at| {
            let src = self.h1_tag(reference(b, at)?)?;
            let entry = h1_assets::models::h1::ShaderRef {
                group: src.group,
                tag: src.id,
                permutation: 0,
            };
            self.model_shader(src, &entry, None)
        };
        let red = shader(0x18)?;
        let blue = shader(0x44)?;
        let panel = |root| Converted {
            group: FourCC(*b"clwd"),
            root,
            raw: vec![],
        };
        let bottom = panel(grid.panel(false, &lower, red)?);
        let top = panel(grid.panel(true, &upper, blue)?);
        let ids = [
            self.cached(tag, bottom)?,
            self.tree.push(&format!("{}\\upper", tag.name), top),
        ];
        self.flag_panels.insert(tag.id.0, ids);
        Ok(ids)
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
    #[test]
    fn lightning_passes_keep_each_bolts_endpoint_styles() {
        use tags::effects::h1_lightning::{Endpoint, HEADER, STRIDE};
        let mut root = [0; 0x108];
        root[2] = 2;
        let mut a = [0; 0xe4];
        a[..4].copy_from_slice(b"mast");
        a[0x24] = 3;
        a[0x84..0x88].copy_from_slice(&0.75f32.to_le_bytes());
        let mut b = a;
        b[0x20] = 1;
        b[0x74..0x78].copy_from_slice(&0.25f32.to_le_bytes());
        let mut shader = [0; 0xb4];
        shader[0x24] = 1;
        shader[0x28] = 2;
        shader[0x2a] = 3;
        let root = lightning(&root, &[&a, &b], &shader, TagId(42)).unwrap();
        let r = defs::from_built(def(*b"tdtl"), &root).unwrap();
        assert_eq!(r.at("marker"), Some(&Val::text("mast")));
        for n in 0..2 {
            let field = format!("layers[{n}].displacement");
            let Some(Val::Data(blob)) = r.at(&field) else {
                panic!("{field}")
            };
            assert_eq!(&blob[0x10..0x14], &[n, 0, 2, 0]);
            let end = |i: usize| {
                Endpoint::decode(
                    blob[HEADER + i * STRIDE..HEADER + (i + 1) * STRIDE]
                        .try_into()
                        .unwrap(),
                )
            };
            assert_eq!(end(0).depth, 3);
            assert_eq!(end(0).width, 0.75);
            assert!(end(1).end);
            assert_eq!(end(1).jitter, [0.25, 0., 0.]);
        }
    }

    #[test]
    fn full_flag_grid_keeps_every_source_vertex_and_the_mast_pins() {
        let grid = FlagGrid {
            width: 16,
            height: 13,
            spacing: [0.025, 0.025],
            contact: [0.5, 0.75],
            physics: tags::effects::h1_attachment::PointPhysics {
                flags: 0,
                particle_type_flags: 0,
                mass: 1.,
                acceleration: [-3., 0.],
                drag: [1., 3.],
                radius: 0.02,
            },
        };
        let panel = |upper, marker, shader| {
            defs::from_built(
                def(*b"clwd"),
                &grid.panel(upper, marker, TagId(shader)).unwrap(),
            )
            .unwrap()
        };
        let lo = panel(false, "bottom", 1);
        let hi = panel(true, "top", 2);
        assert_eq!(lo.at("marker"), Some(&Val::text("bottom")));
        assert_eq!(hi.at("marker"), Some(&Val::text("top")));
        assert_eq!(lo.at("vertices[0].uv"), Some(&Val::floats(&[0., 0.])));
        assert_eq!(hi.at("vertices[0].uv"), Some(&Val::floats(&[0., 1.])));
        for p in [&lo, &hi] {
            let Some(Val::Block(vertices)) = p.at("vertices") else {
                panic!("vertices")
            };
            let Some(Val::Block(triangles)) = p.at("triangles") else {
                panic!("triangles")
            };
            assert_eq!(vertices.len(), 112);
            assert_eq!(triangles.len(), 6 * 15 * 6);
        }
        for i in 96..112 {
            assert_eq!(
                lo.at(&format!("vertices[{i}].position")),
                hi.at(&format!("vertices[{i}].position"))
            );
            assert_eq!(
                lo.at(&format!("vertices[{i}].uv")),
                hi.at(&format!("vertices[{i}].uv"))
            );
        }
    }
}
