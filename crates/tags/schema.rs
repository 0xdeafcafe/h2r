//! Minimal Xbox retail wire schema; public sources were consulted.
//! Provenance and verification limits: docs/map-format.md.
//! Range checks alone do not establish provenance or semantic accuracy.

#[derive(Clone, Copy, Debug)]
pub enum Kind {
    U8,
    U16,
    I16,
    U32,
    F32,
    Enum16,
    Enum32,
    Flags8,
    Flags16,
    StringId,
    TagRef,
    DataRef,
    Block(u32),
    EmptyBlock,
    Bytes(usize),
    U32s(usize),
    F32s(usize),
}
impl Kind {
    pub fn width(self) -> usize {
        match self {
            Self::U8 | Self::Flags8 => 1,
            Self::U16 | Self::I16 | Self::Enum16 | Self::Flags16 => 2,
            Self::U32 | Self::F32 | Self::Enum32 | Self::StringId => 4,
            Self::TagRef | Self::DataRef | Self::Block(_) | Self::EmptyBlock => 8,
            Self::Bytes(n) => n,
            Self::U32s(n) | Self::F32s(n) => n * 4,
        }
    }
}
pub struct Field {
    pub name: &'static str,
    pub offset: usize,
    pub kind: Kind,
    pub choices: &'static [(u32, &'static str)],
    pub element: Option<&'static str>,
}
impl Field {
    fn choices(mut self, values: &'static [(u32, &'static str)]) -> Self {
        self.choices = values;
        self
    }
    fn elements(mut self, name: &'static str) -> Self {
        self.element = Some(name);
        self
    }
}
pub struct Record {
    pub name: &'static str,
    pub size: usize,
    pub fields: Vec<Field>,
}
fn f(name: &'static str, offset: usize, kind: Kind) -> Field {
    Field {
        name,
        offset,
        kind,
        choices: &[],
        element: None,
    }
}
fn record(name: &'static str, size: usize, mut fields: Vec<Field>) -> Record {
    // Coalesce every unreviewed span without reproducing an upstream block table.
    fields.sort_by_key(|f| f.offset);
    let mut end = 0;
    let mut opaque = Vec::new();
    for field in &fields {
        if field.offset > end {
            opaque.push(f(
                Box::leak(format!("opaque_{end:03x}").into_boxed_str()),
                end,
                Kind::Bytes(field.offset - end),
            ));
        }
        end = field.offset + field.kind.width();
    }
    if end < size {
        opaque.push(f(
            Box::leak(format!("opaque_{end:03x}").into_boxed_str()),
            end,
            Kind::Bytes(size - end),
        ));
    }
    fields.extend(opaque);
    Record { name, size, fields }
}
fn empty(fields: &mut Vec<Field>, rows: &[(&'static str, usize)]) {
    fields.extend(rows.iter().map(|&(n, o)| f(n, o, Kind::EmptyBlock)));
}

const PIXEL_FORMATS: &[(u32, &str)] = &[
    (0, "A8"),
    (1, "Y8"),
    (2, "AY8"),
    (3, "A8Y8"),
    (6, "RGB565"),
    (8, "ARGB1555"),
    (9, "ARGB4444"),
    (10, "XRGB8888"),
    (11, "ARGB8888"),
    (14, "DXT1"),
    (15, "DXT3"),
    (16, "DXT5"),
    (17, "P8Bump"),
    (18, "P8"),
    (19, "ARGBF32"),
    (20, "RGBF32"),
    (21, "RGBF16"),
    (22, "V8U8"),
    (23, "G8B8"),
];

pub fn records() -> Vec<Record> {
    use Kind::*;
    // Provenance: Assembly Halo2Xbox/bitm.xml @ fd14ef3c46135e9422966c6a64cd926fb0bd23fb:
    // offset hypotheses. Disc: both block extents and 0x4c prefix; editor scalar meanings
    // are source-only.
    let bitmap = record(
        "BitmapHeader",
        0x4c,
        vec![
            f("kind", 0, Enum16),
            f("encoding_preference", 2, Enum16),
            f("usage", 4, Enum16),
            f("flags", 6, Flags16),
            f("detail_fade", 8, F32),
            f("sharpen", 0xc, F32),
            f("bump_scale", 0x10, F32),
            f("sprite_budget_class", 0x14, Enum16),
            f("sprite_budget", 0x16, U16),
            f("source_width", 0x18, U16),
            f("source_height", 0x1a, U16),
            f("blur", 0x2c, F32),
            f("alpha_bias", 0x30, F32),
            f("requested_mips", 0x34, U16),
            f("sprite_usage", 0x36, Enum16),
            f("sprite_spacing", 0x38, U16),
            f("forced_format", 0x3a, Enum16),
            f("sequences", 0x3c, Block(0x3c)).elements("BitmapSequence"),
            f("images", 0x44, Block(0x74)).elements("BitmapImage"),
        ],
    );
    // Provenance: Same pinned Assembly bitmap; cross-check Entity @
    // 4b4ddc3d0869cf93773869c642dec6624a22fd36 and OpenH2 @
    // d09a123b3dd16821295f380f01fb5639c697899a. Disc: repeated signatures, depth byte,
    // LOD/low-detail ranges. Format/flag meanings remain source-only.
    let image = record(
        "BitmapImage",
        0x74,
        vec![
            f("signature", 0, U32),
            f("width", 4, U16),
            f("height", 6, U16),
            f("depth", 8, U8),
            f("runtime_flags", 9, Flags8),
            f("kind", 0xa, Enum16).choices(&[(0, "texture_2d"), (1, "texture_3d"), (2, "cube")]),
            f("format", 0xc, Enum16).choices(PIXEL_FORMATS),
            f("flags", 0xe, Flags16).choices(&[
                (1, "power_of_two"),
                (2, "compressed"),
                (4, "palettized"),
                (8, "swizzled"),
                (16, "linear"),
            ]),
            f("registration_x", 0x10, I16),
            f("registration_y", 0x12, I16),
            f("mip_count", 0x14, U16),
            f("lod_bias", 0x16, U8),
            f("cache_usage", 0x17, U8),
            f("pixel_offset", 0x18, U32),
            f("lod_pointers", 0x1c, U32s(3)),
            f("lod_sizes", 0x34, U32s(3)),
            f("runtime_id", 0x4c, U32),
            f("low_detail_offset", 0x60, U32),
            f("low_detail_size", 0x64, U32),
            f("low_detail_format", 0x68, Enum16).choices(PIXEL_FORMATS),
            f("low_detail_width", 0x6a, U16),
            f("low_detail_height", 0x6c, U16),
            f("low_detail_depth", 0x6e, U16),
        ],
    );
    // Provenance: Same pinned Assembly bitmap: sequence hypothesis. Disc: 0x3c stride,
    // sprite count/pointer spans. Label/image-range semantics source-only.
    let sequence = record(
        "BitmapSequence",
        0x3c,
        vec![
            f("label", 0, Bytes(32)),
            f("first_image", 0x20, I16),
            f("image_count", 0x22, U16),
            f("sprites", 0x34, Block(0x20)).elements("BitmapSprite"),
        ],
    );
    // Provenance: Same pinned Assembly bitmap: 0x20 stride and coordinate hypotheses. Disc:
    // spans only; coordinates have not been rendered.
    let sprite = record(
        "BitmapSprite",
        0x20,
        vec![
            f("image_index", 0, I16),
            f("bounds", 8, F32s(4)),
            f("registration", 0x18, F32s(2)),
        ],
    );
    // Provenance: Assembly Halo2Xbox/scnr.xml @ fd14ef3c46135e9422966c6a64cd926fb0bd23fb:
    // scalar/reference/data offsets. Disc: reference resolution, byte ranges and zero
    // guards only. All unused child tables removed.
    let mut scenario = vec![
        f("legacy_reference", 0, TagRef),
        f("kind", 0x10, Enum16),
        f("flags", 0x12, Flags16),
        f("north_radians", 0x1c, F32),
        f("editor_data", 0x30, DataRef),
        f("script_syntax", 0x1a8, DataRef),
        f("script_strings", 0x1b0, DataRef),
        f("object_names_text", 0x1f8, TagRef),
        f("chapter_text", 0x200, TagRef),
        f("hud_text", 0x208, TagRef),
        f("object_salts", 0x298, U32s(32)),
        f("sound_effects", 0x320, TagRef),
        f("lighting", 0x338, TagRef),
        f("subtitles", 0x358, TagRef),
        f("territory_text", 0x3a0, TagRef),
        f("objectives", 0x3b8, TagRef),
    ];
    empty(
        &mut scenario,
        &[
            ("unsupported_028", 0x28),
            ("unsupported_180", 0x180),
            ("unsupported_1d0", 0x1d0),
            ("unsupported_268", 0x268),
            ("unsupported_270", 0x270),
            ("unsupported_278", 0x278),
            ("unsupported_280", 0x280),
            ("unsupported_288", 0x288),
            ("unsupported_3c8", 0x3c8),
        ],
    );
    // Provenance: Assembly Halo2/matg.xml @ fd14ef3c46135e9422966c6a64cd926fb0bd23fb:
    // reference/language offsets. Disc: reference resolution and zero guard. Child tables
    // opaque.
    let mut globals = vec![
        f("language", 0xac, Enum32),
        f("multiplayer", 0x168, TagRef),
        f("lighting", 0x180, TagRef),
    ];
    empty(&mut globals, &[("unsupported_0d8", 0xd8)]);
    // Nine embedded locale descriptors are separate fixed records, not tag_blocks.
    // Provenance: Assembly Halo2/matg.xml and H2X_Layouts_Core.xml @
    // fd14ef3c46135e9422966c6a64cd926fb0bd23fb: descriptor hypothesis. Disc: nine fixed
    // slots, file extents; runtime field meanings source-only.
    let locale = record(
        "LocaleDescriptor",
        0x1c,
        vec![
            f("runtime_index", 0, U32),
            f("runtime_data", 4, U32),
            f("count", 8, U32),
            f("data_size", 0xc, U32),
            f("index_file_offset", 0x10, U32),
            f("data_file_offset", 0x14, U32),
            f("loaded", 0x18, U32),
        ],
    );
    // Provenance: Assembly Halo2/sbsp.xml + Xbox override @
    // fd14ef3c46135e9422966c6a64cd926fb0bd23fb: scalar/reference/data offsets. Disc:
    // BSP-space ranges, reference resolution, zero guards. Child tables opaque.
    let mut bsp = vec![
        f("checksum", 8, U32),
        f("vehicle_floor", 0x1c, F32),
        f("vehicle_ceiling", 0x20, F32),
        f("world_bounds", 0x34, F32s(6)),
        f("cluster_data", 0x54, DataRef),
        f("sound_data", 0xe4, DataRef),
        f("vehicle_radius", 0x1c0, F32),
        f("vehicle_center", 0x1c4, F32s(3)),
        f("decorator_reference", 0x1d8, TagRef),
        f("physics_code", 0x1e0, DataRef),
        f("physics_min", 0x1ec, F32s(3)),
        f("physics_max", 0x1f8, F32s(3)),
        f("breakable_physics_code", 0x204, DataRef),
    ];
    empty(
        &mut bsp,
        &[
            ("unsupported_000", 0),
            ("unsupported_024", 0x24),
            ("unsupported_10c", 0x10c),
            ("unsupported_118", 0x118),
            ("unsupported_120", 0x120),
            ("unsupported_128", 0x128),
            ("unsupported_130", 0x130),
        ],
    );
    // Provenance: Assembly Halo2/mode.xml @ fd14ef3c46135e9422966c6a64cd926fb0bd23fb:
    // scalar offsets. Disc: name StringID length/index, zero guards. Child tables opaque.
    let mut model = vec![
        f("name", 0, StringId),
        f("flags", 4, Flags16),
        f("checksum", 8, U32),
        f("lod_groups", 0x3c, Bytes(6)),
        f("node_checksum", 0x44, U32),
        f("camera_cosine", 0x70, F32),
    ];
    empty(
        &mut model,
        &[("unsupported_00c", 0xc), ("unsupported_068", 0x68)],
    );
    vec![
        bitmap,
        image,
        sequence,
        sprite,
        record("ScenarioHeader", 0x3e0, scenario),
        record("GlobalsHeader", 0x284, globals),
        locale,
        record("BspHeader", 0x23c, bsp),
        record("RenderModelHeader", 0x84, model),
    ]
}
