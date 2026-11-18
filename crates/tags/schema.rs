//! Minimal Xbox retail wire schema: offsets, types and names established by
//! the engine's code, disc checks and our decoders (noted per record).
//! Verification limits: docs/formats/map-format.md.
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
    // Names here are ours: each kept field's role is established by the
    // engine's code (E) or a check on the disc (D) noted beside it, or by
    // our decoders using it (pixels decoded, models drawn, text read).
    // Bytes nothing of ours explains stay opaque (preserved by encoders).
    //
    // Bitmap root: D (both block extents, the 0x4c prefix). The scalars are
    // the fields the Halo 1 converter (h1bitmaps::convert) fills from the
    // Halo 1 bitmap's field of the same role, offset for offset; converted
    // bitmaps load and draw in game (docs/halo1/bitmaps.md). Their names
    // follow that role; what the Halo 2 engine does with each is not
    // established here.
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
    // Bitmap image: D (repeated signatures, depth byte, LOD and low-detail
    // ranges); size, format, mips and pixel offset are what our decoder
    // reads to produce the pixels (assets::bitmap, every disc bitmap
    // decoded); the rest as for the root.
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
    // Bitmap sequence and sprite: D (strides, sprite spans); fields as for
    // the root, and the sprite bounds are what live art crops by
    // (cli live_art).
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
    let sprite = record(
        "BitmapSprite",
        0x20,
        vec![
            f("image_index", 0, I16),
            f("bounds", 8, F32s(4)),
            f("registration", 0x18, F32s(2)),
        ],
    );
    // Its tag references (D: they resolve on every disc scenario) are
    // labelled by offset.
    let mut scenario = vec![
        f("tagref_000", 0, TagRef),
        f("data_030", 0x30, DataRef),
        f("script_syntax", 0x1a8, DataRef),
        f("script_strings", 0x1b0, DataRef),
        f("tagref_1f8", 0x1f8, TagRef),
        f("tagref_200", 0x200, TagRef),
        f("tagref_208", 0x208, TagRef),
        f("tagref_320", 0x320, TagRef),
        f("tagref_338", 0x338, TagRef),
        f("tagref_358", 0x358, TagRef),
        f("tagref_3a0", 0x3a0, TagRef),
        f("tagref_3b8", 0x3b8, TagRef),
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
    // Globals root: D (zero guard). Child tables opaque.
    let mut globals = vec![
        f("language", 0xac, Enum32),
        f("tagref_168", 0x168, TagRef),
        f("tagref_180", 0x180, TagRef),
    ];
    empty(&mut globals, &[("unsupported_0d8", 0xd8)]);
    // Nine embedded locale descriptors (fixed records, not tag blocks): D
    // (nine slots, file extents); the count, size and offsets are what our
    // text loader reads (localization, checked on every disc language); the
    // two runtime words and the loaded flag are what the engine's loader
    // writes (E `0x1a01c3`, docs/formats/localization.md).
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
    // Structure BSP root: D (BSP-space ranges of its data references, zero
    // guards). Child tables opaque.
    let mut bsp = vec![
        f("data_054", 0x54, DataRef),
        f("data_0e4", 0xe4, DataRef),
        f("tagref_1d8", 0x1d8, TagRef),
        f("data_1e0", 0x1e0, DataRef),
        f("data_204", 0x204, DataRef),
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
    // Render model root: D (the name string ID's length and index). Child
    // tables opaque.
    let mut model = vec![f("name", 0, StringId)];
    empty(
        &mut model,
        &[("unsupported_00c", 0xc), ("unsupported_068", 0x68)],
    );
    let mut all = vec![
        bitmap,
        image,
        sequence,
        sprite,
        record("ScenarioHeader", 0x3e0, scenario),
        record("GlobalsHeader", 0x284, globals),
        locale,
        record("BspHeader", 0x23c, bsp),
        record("RenderModelHeader", 0x84, model),
    ];
    all.extend(model_geometry_records());
    all
}

// ============================================================================
// render_model geometry (mode header, compression, regions, sections, nodes,
// materials). Used only by src/model.rs; raw section blocks are decoded by
// the shared src/geometry.rs. Kept as one self-contained section so other tag
// groups can be added above without touching it.
//
// Evidence (details: docs/formats/map-format.md, "Render-model geometry").
// Every field carries one or more tags:
//   E = read by the engine's own code (XBE disassembly, address given);
//   D = disc evidence, checked on every mode tag of every map by
//       `viewer --all` (strides, counts, signatures, value ranges);
//   R = the role our model decoder gives it, checked by drawing every
//       model of every map (`viewer --all`).
// Names describe that evidence, or are `unknown_<offset>` when there is
// none.
// ============================================================================
fn model_geometry_records() -> Vec<Record> {
    use Kind::*;
    vec![
        // A second view of the 0x84-byte mode header that interprets the
        // geometry blocks; RenderModelHeader keeps the root-only validation.
        // sections: E (0x41076..0x41086: [tag+0x28] + index*0x5c).
        // compression_infos, regions, nodes, materials: R + D (count*stride
        // fits the payload on every tag).
        record(
            "ModeGeometryHeader",
            0x84,
            vec![
                f("name", 0, StringId),
                f("compression_infos", 0x14, Block(0x38)).elements("ModeCompression"),
                f("regions", 0x1c, Block(0x10)).elements("ModeRegion"),
                f("sections", 0x24, Block(0x5c)).elements("ModeSection"),
                f("nodes", 0x48, Block(0x60)).elements("ModeNode"),
                f("materials", 0x60, Block(0x20)).elements("ModeMaterial"),
            ],
        ),
        // R (the bounds compressed positions and texcoords decode into:
        // min/max per axis, u, v, u2, v2) + D (exactly one per model;
        // decoded attributes fall inside it).
        record(
            "ModeCompression",
            0x38,
            vec![
                f("position_bounds", 0, F32s(6)),
                f("texcoord_bounds", 0x18, F32s(4)),
                f("texcoord2_bounds", 0x28, F32s(4)),
            ],
        ),
        // name/permutations: R (pieces are picked by region and
        // permutation). +4/+6: unknown (i16 pairs, often -1/0).
        record(
            "ModeRegion",
            0x10,
            vec![
                f("name", 0, StringId),
                f("unknown_004", 4, I16),
                f("unknown_006", 6, I16),
                f("permutations", 8, Block(0x10)).elements("ModePermutation"),
            ],
        ),
        // R + D (six section indices, each -1 or a valid section; triangle
        // counts rise with the index: levels of detail).
        record(
            "ModePermutation",
            0x10,
            vec![
                f("name", 0, StringId),
                f("lod0_section", 4, I16),
                f("lod1_section", 6, I16),
                f("lod2_section", 8, I16),
                f("lod3_section", 0xa, I16),
                f("lod4_section", 0xc, I16),
                f("lod5_section", 0xe, I16),
            ],
        ),
        // Stride and +0x34: E (+0x34 is a runtime pointer to the loaded
        // section data, zero on disc; the engine reads its parts at [ptr+4]).
        // Counts/classification/flags: D (counts equal the raw block's;
        // classification matches the vertex types).
        // +0x30..+0x38: the runtime section block (count, pointer) the
        // engine's fixup fills; empty on disc (D).
        // +0x38..+0x5c: the shared 0x24-byte geometry block info, decoded by
        // src/geometry.rs (E 0x00223c70; see docs/formats/structure-bsp.md). Owner =
        // this tag and runtime offset = -8 on every section (D).
        record(
            "ModeSection",
            0x5c,
            vec![
                f("unknown_000", 0, U16),
                f("vertex_count", 4, U16),
                f("triangle_count", 6, U16),
                f("part_count", 8, U16),
                f("unknown_014", 0x14, U8),
                // Labels from D: the vertex-buffer types each value occurs with.
                f("geometry_classification", 0x18, Enum16).choices(&[
                    (1, "no_vertex_nodes"),
                    (2, "one_node_per_vertex"),
                    (3, "weighted_nodes"),
                ]),
                // D: bits 0/1 match s16 position/texcoord buffers.
                f("compression_flags", 0x1a, Flags16)
                    .choices(&[(1, "compressed_positions"), (2, "compressed_texcoords")]),
                f("runtime_section_count", 0x30, U32),
                f("runtime_section_data", 0x34, U32),
            ],
        ),
        // name/parent/first_child/next_sibling/translation/orientation: R
        // (the skeleton our decoder poses) + D (links in range). The 14
        // words from +0x28 are not interpreted.
        record(
            "ModeNode",
            0x60,
            vec![
                f("name", 0, StringId),
                f("parent_index", 4, I16),
                f("first_child_index", 6, I16),
                f("next_sibling_index", 8, I16),
                f("translation", 0xc, F32s(3)),
                f("orientation", 0x18, F32s(4)),
                f("unknown_028", 0x28, F32s(14)),
            ],
        ),
        // D: two tag references; the second resolves to `shad` tags (checked
        // by `viewer --all`), the first is null on the disc.
        record(
            "ModeMaterial",
            0x20,
            vec![f("unknown_reference", 0, TagRef), f("shader", 8, TagRef)],
        ),
        // The raw section struct, parts, subparts and vertex-buffer
        // descriptors are decoded by src/geometry.rs (MODEL_SECTION in
        // src/model.rs), shared with structure BSPs.
    ]
}
