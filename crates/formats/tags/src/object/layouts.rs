//! Field tables for the object family. Every entry records how it was
//! established: `E` engine code (address in the note), `V` structure checked on
//! every instance on the retail disc, `D` a naming or interpretation choice
//! made here. Unlisted bytes are opaque and preserved. Names that are not
//! backed by engine code are `unknown_*`, apart from a few `D`-labelled names
//! whose role is argued in `docs/formats/objects.md`.
//!
//! No community tag definition was used: offsets, strides, classes and names
//! come from the disc and the XBE, see `docs/formats/objects.md`. The weapon
//! aim-assist fields (`weap +0x208` to `+0x218`) are named after what the
//! engine reader does with them (docs/game/aim-assist.md).
use super::{Evidence::*, Field, Kind::*, Layout};

const fn f(
    offset: u32,
    name: &'static str,
    kind: super::Kind,
    evidence: super::Evidence,
    note: &'static str,
) -> Field {
    Field {
        offset,
        name,
        kind,
        evidence,
        note,
    }
}

// ---------------------------------------------------------------- obje

/// `obje` block at `+0x94`, stride `0x18`.
pub static ATTACHMENT: Layout = Layout {
    name: "attachment",
    start: 0,
    end: 0x18,
    fields: &[
        f(
            0x00,
            "type",
            TagRef(&[]),
            Engine,
            "0x000bed2a switches on the class: ligh, lsnd, effe, cont; lens/MGS2/tdtl skipped",
        ),
        f(
            0x08,
            "marker",
            StringId,
            Engine,
            "accessor 0x000b8c40 loads element +8 by attachment index at 0x000b8c8b",
        ),
        f(
            0x0c,
            "unknown_0c",
            U16,
            Engine,
            "0x000bedad passes it to the attachment constructor",
        ),
        f(0x10, "unknown_sid_10", StringId, Disc, ""),
        f(0x14, "unknown_sid_14", StringId, Disc, ""),
    ],
};
/// `obje` block at `+0x9c`, stride 8: one tag reference per element.
pub static WIDGET: Layout = Layout {
    name: "widget",
    start: 0,
    end: 8,
    fields: &[f(
        0x00,
        "type",
        TagRef(&[]),
        Disc,
        "observed classes include clwd and tdtl",
    )],
};
/// `obje` block at `+0x64`, stride `0x20`.
pub static FUNCTION: Layout = Layout {
    name: "function",
    start: 0,
    end: 0x20,
    fields: &[
        f(0x00, "unknown_flags", U32, Disc, ""),
        f(0x04, "unknown_sid_04", StringId, Disc, ""),
        f(
            0x08,
            "output",
            StringId,
            Engine,
            "0x000bac20 loop (stride 0x20) matches element +8 against the requested ID",
        ),
        f(0x0c, "unknown_sid_0c", StringId, Disc, ""),
        f(
            0x14,
            "unknown_block_14",
            OpaqueBlock,
            Disc,
            "nonempty on every element; element size unreviewed",
        ),
    ],
};
/// `obje` block at `+0xac`, stride `0x10`.
pub static CHANGE_COLOR: Layout = Layout {
    name: "change_color",
    start: 0,
    end: 0x10,
    fields: &[
        f(0x00, "unknown_block_00", OpaqueBlock, Disc, ""),
        f(0x08, "unknown_block_08", OpaqueBlock, Disc, ""),
    ],
};
/// `obje` block at `+0xb4`, stride 8.
pub static PREDICTED_RESOURCE: Layout = Layout {
    name: "predicted_resource",
    start: 0,
    end: 8,
    fields: &[
        f(0x00, "unknown_00", U16, Disc, ""),
        f(0x02, "unknown_02", U16, Disc, ""),
        f(
            0x04,
            "tag",
            TagDatum,
            Disc,
            "full salted datum of an existing tag on every disc element",
        ),
    ],
};

pub static OBJE: Layout = Layout {
    name: "obje",
    start: 0,
    end: 0xbc,
    fields: &[
        f(
            0x00,
            "object_type",
            U16,
            Engine,
            "0x000b7b40 indexes the type table 0x00468630 with it",
        ),
        f(0x02, "flags", U16, Engine, "0x000b7b40 tests bit 0"),
        f(
            0x04,
            "bounding_radius",
            F32,
            Engine,
            "0x000be075 multiplies it by the object scale",
        ),
        f(
            0x08,
            "bounding_offset",
            Vec3,
            Engine,
            "0x000bdfa9 transforms it by the root node into the sphere centre",
        ),
        f(
            0x30,
            "default_variant",
            StringId,
            Engine,
            "0x000ba571 passes it to the hlmt variant lookup 0x0016d180",
        ),
        f(
            0x34,
            "model",
            TagRef(&["hlmt"]),
            Engine,
            "0x000b7b40 resolves +0x38 and reads hlmt fields from it",
        ),
        f(0x3c, "unknown_bloc_ref_3c", TagRef(&["bloc"]), Disc, ""),
        f(0x44, "unknown_shad_ref_44", TagRef(&["shad"]), Disc, ""),
        f(
            0x4c,
            "creation_effect",
            TagRef(&["effe"]),
            Engine,
            "0x000b7b40 passes +0x50 to 0x00176780 at the end of creation",
        ),
        f(0x54, "unknown_foot_ref_54", TagRef(&["foot"]), Disc, ""),
        f(
            0x5c,
            "unknown_block_5c",
            OpaqueBlock,
            Disc,
            "at most one element; stride not established",
        ),
        f(
            0x64,
            "functions",
            Block(0x20, &FUNCTION),
            Engine,
            "0x000bac46 steps 0x20",
        ),
        f(
            0x94,
            "attachments",
            Block(0x18, &ATTACHMENT),
            Engine,
            "0x000b8c82 index*0x18; object_new reserves 8 bytes each",
        ),
        f(
            0x9c,
            "widgets",
            Block(8, &WIDGET),
            Disc,
            "name is task vocabulary (D)",
        ),
        f(
            0xa4,
            "unknown_block_a4",
            OpaqueBlock,
            Disc,
            "observed empty",
        ),
        f(
            0xac,
            "change_colors",
            Block(0x10, &CHANGE_COLOR),
            Disc,
            "object_new 0x000b7b40 reserves 0x18 bytes per element (two colours, D)",
        ),
        f(
            0xb4,
            "predicted_resources",
            Block(8, &PREDICTED_RESOURCE),
            Disc,
            "name D",
        ),
    ],
};

// ---------------------------------------------------------------- abstract layers

pub static UNIT: Layout = Layout {
    name: "unit",
    start: 0xbc,
    end: 0x1ec,
    fields: &[
        f(0xc4, "unknown_effe_ref_c4", TagRef(&["effe"]), Disc, ""),
        f(
            0xe8,
            "unknown_block_e8",
            OpaqueBlock,
            Disc,
            "at most one element",
        ),
        f(0x134, "unknown_char_ref_134", TagRef(&["char"]), Disc, ""),
        f(0x164, "unknown_jpt_ref_164", TagRef(&["jpt!"]), Disc, ""),
        f(0x16c, "unknown_jpt_ref_16c", TagRef(&["jpt!"]), Disc, ""),
        f(0x174, "unknown_jpt_ref_174", TagRef(&["jpt!"]), Disc, ""),
        f(0x17c, "unknown_jpt_ref_17c", TagRef(&["jpt!"]), Disc, ""),
        f(0x184, "unknown_jpt_ref_184", TagRef(&["jpt!"]), Disc, ""),
        f(0x18c, "unknown_jpt_ref_18c", TagRef(&["jpt!"]), Disc, ""),
        f(
            0x198,
            "unknown_block_198",
            OpaqueBlock,
            Disc,
            "stride 0x10 on bipd only",
        ),
        f(
            0x1a0,
            "unknown_block_1a0",
            OpaqueBlock,
            Disc,
            "stride 8 from layout",
        ),
        f(0x1a8, "unknown_block_1a8", OpaqueBlock, Disc, ""),
        f(0x1b8, "unknown_block_1b8", OpaqueBlock, Disc, ""),
        f(0x1c0, "unknown_block_1c0", OpaqueBlock, Disc, ""),
        f(
            0x1c8,
            "unknown_block_1c8",
            OpaqueBlock,
            Disc,
            "stride 0xb0 from layout; unit code reads +0x1cc 21 times",
        ),
    ],
};
pub static ITEM: Layout = Layout {
    name: "item",
    start: 0xbc,
    end: 0x12c,
    fields: &[
        f(0xcc, "unknown_sid_cc", StringId, Disc, ""),
        f(0xd0, "unknown_sid_d0", StringId, Disc, ""),
        f(0xd4, "unknown_sid_d4", StringId, Disc, ""),
        f(0xd8, "unknown_sid_d8", StringId, Disc, ""),
        f(0xdc, "unknown_sid_dc", StringId, Disc, ""),
        f(0xe0, "unknown_sid_e0", StringId, Disc, ""),
        f(0xe4, "unknown_sid_e4", StringId, Disc, ""),
        f(0xe8, "unknown_sid_e8", StringId, Disc, ""),
        f(0xec, "unknown_sid_ec", StringId, Disc, ""),
        f(0xf0, "unknown_sid_f0", StringId, Disc, ""),
        f(0xf4, "unknown_foot_ref_f4", TagRef(&["foot"]), Disc, ""),
        f(0xfc, "unknown_snd_ref_fc", TagRef(&["snd!"]), Disc, ""),
        f(
            0x104,
            "unknown_block_104",
            OpaqueBlock,
            Disc,
            "stride 8 from layout",
        ),
        f(0x10c, "unknown_jpt_ref_10c", TagRef(&["jpt!"]), Disc, ""),
        f(0x11c, "unknown_effe_ref_11c", TagRef(&["effe"]), Disc, ""),
        f(0x124, "unknown_effe_ref_124", TagRef(&["effe"]), Disc, ""),
    ],
};
pub static DEVI: Layout = Layout {
    name: "devi",
    start: 0xbc,
    end: 0x11c,
    fields: &[
        f(0xdc, "unknown_snd_ref_dc", TagRef(&["snd!"]), Disc, ""),
        f(0xe4, "unknown_snd_ref_e4", TagRef(&["snd!"]), Disc, ""),
        f(0xec, "unknown_ref_ec", TagRef(&["effe", "snd!"]), Disc, ""),
        f(0xf4, "unknown_ref_f4", TagRef(&["effe", "snd!"]), Disc, ""),
        f(0xfc, "unknown_effe_ref_fc", TagRef(&["effe"]), Disc, ""),
        f(0x104, "unknown_effe_ref_104", TagRef(&["effe"]), Disc, ""),
        f(
            0x110,
            "unknown_ref_110",
            TagRef(&["effe", "snd!"]),
            Disc,
            "",
        ),
    ],
};

// ---------------------------------------------------------------- concrete types

pub static BIPD: Layout = Layout {
    name: "bipd",
    start: 0x1ec,
    end: 0x314,
    fields: &[
        f(
            0x1f0,
            "flags",
            U32,
            Engine,
            "biped_new helper 0x000de5ca tests bit 1",
        ),
        f(0x25c, "unknown_effe_ref_25c", TagRef(&["effe"]), Disc, ""),
        f(0x278, "unknown_sid_278", StringId, Disc, ""),
        f(0x27c, "unknown_sid_27c", StringId, Disc, ""),
        f(
            0x284,
            "unknown_block_284",
            OpaqueBlock,
            Disc,
            "stride 0x80 from layout; same as crea +0xf4",
        ),
        f(
            0x28c,
            "unknown_block_28c",
            OpaqueBlock,
            Disc,
            "stride 0x50 from layout; same as crea +0xfc",
        ),
        f(
            0x294,
            "unknown_block_294",
            OpaqueBlock,
            Disc,
            "stride 0x80 from layout; same as crea +0x104",
        ),
        f(
            0x2f8,
            "unknown_block_2f8",
            OpaqueBlock,
            Disc,
            "stride 4 from layout",
        ),
        f(0x300, "unknown_char_ref_300", TagRef(&["char"]), Disc, ""),
        f(0x308, "unknown_char_ref_308", TagRef(&["char"]), Disc, ""),
    ],
};
pub static VEHI: Layout = Layout {
    name: "vehi",
    start: 0x1ec,
    end: 0x300,
    fields: &[
        f(
            0x1ec,
            "flags",
            U32,
            Engine,
            "vehicle code tests bits, e.g. 0x000efb8c",
        ),
        f(
            0x1f0,
            "vehicle_type",
            U16,
            Engine,
            "vehicle code compares it with 5 and 6 (0x000ef545, 0x000eff0b)",
        ),
        f(
            0x268,
            "unknown_block_268",
            OpaqueBlock,
            Disc,
            "stride 0x44 from layout",
        ),
        f(0x284, "unknown_snd_ref_284", TagRef(&["snd!"]), Disc, ""),
        f(0x28c, "unknown_snd_ref_28c", TagRef(&["snd!"]), Disc, ""),
        f(0x294, "unknown_foot_ref_294", TagRef(&["foot"]), Disc, ""),
        f(0x29c, "unknown_effe_ref_29c", TagRef(&["effe"]), Disc, ""),
        f(0x2a4, "unknown_effe_ref_2a4", TagRef(&["effe"]), Disc, ""),
        f(
            0x2e8,
            "unknown_block_2e8",
            OpaqueBlock,
            Disc,
            "stride 0x4c from layout",
        ),
        f(
            0x2f0,
            "unknown_block_2f0",
            OpaqueBlock,
            Disc,
            "stride 0x4c from layout",
        ),
        f(
            0x2f8,
            "unknown_block_2f8",
            OpaqueBlock,
            Disc,
            "exactly one element on every disc vehicle",
        ),
    ],
};
pub static WEAP: Layout = Layout {
    name: "weap",
    start: 0x12c,
    end: 0x31c,
    fields: &[
        f(0x140, "unknown_effe_ref_140", TagRef(&["effe"]), Disc, ""),
        f(0x148, "unknown_jpt_ref_148", TagRef(&["jpt!"]), Disc, ""),
        f(0x16c, "unknown_effe_ref_16c", TagRef(&["effe"]), Disc, ""),
        f(0x174, "unknown_jpt_ref_174", TagRef(&["jpt!"]), Disc, ""),
        f(0x17c, "unknown_effe_ref_17c", TagRef(&["effe"]), Disc, ""),
        f(0x184, "unknown_jpt_ref_184", TagRef(&["jpt!"]), Disc, ""),
        f(0x18c, "unknown_jpt_ref_18c", TagRef(&["jpt!"]), Disc, ""),
        f(0x194, "unknown_jpt_ref_194", TagRef(&["jpt!"]), Disc, ""),
        f(0x1bc, "unknown_jpt_ref_1bc", TagRef(&["jpt!"]), Disc, ""),
        f(0x1c4, "unknown_jpt_ref_1c4", TagRef(&["jpt!"]), Disc, ""),
        f(0x1cc, "unknown_jpt_ref_1cc", TagRef(&["jpt!"]), Disc, ""),
        f(0x1d4, "unknown_jpt_ref_1d4", TagRef(&["jpt!"]), Disc, ""),
        f(0x1dc, "unknown_jpt_ref_1dc", TagRef(&["jpt!"]), Disc, ""),
        f(0x1e4, "unknown_jpt_ref_1e4", TagRef(&["jpt!"]), Disc, ""),
        f(0x1ec, "unknown_jpt_ref_1ec", TagRef(&["jpt!"]), Disc, ""),
        f(0x1f4, "unknown_jpt_ref_1f4", TagRef(&["jpt!"]), Disc, ""),
        // Weapon aim assist (docs/game/aim-assist.md). The engine's
        // aim-assist parameters (0x001a50a0) read these through the tag
        // entry table; named after that use.
        f(
            0x208,
            "cone_angle",
            F32,
            Engine,
            "radians; 0x001a5137 divides it by the zoom magnification",
        ),
        f(
            0x20c,
            "cone_range",
            F32,
            Engine,
            "world units; 0x001a5153 multiplies it by the zoom magnification",
        ),
        f(
            0x210,
            "pull_angle",
            F32,
            Engine,
            "radians; 0x001a5164 divides it by the zoom magnification",
        ),
        f(
            0x214,
            "pull_range",
            F32,
            Engine,
            "world units; 0x001a5175 multiplies it by the zoom magnification",
        ),
        f(
            0x218,
            "bend_angle",
            F32,
            Engine,
            "radians; 0x001a5186 uses max(this, cone_angle)",
        ),
        f(0x244, "unknown_effe_ref_244", TagRef(&["effe"]), Disc, ""),
        f(0x24c, "unknown_effe_ref_24c", TagRef(&["effe"]), Disc, ""),
        f(0x264, "unknown_snd_ref_264", TagRef(&["snd!"]), Disc, ""),
        f(
            0x26c,
            "unknown_ref_26c",
            TagRef(&["bitm", "snd!"]),
            Disc,
            "",
        ),
        f(
            0x274,
            "unknown_ref_274",
            TagRef(&["bitm", "snd!"]),
            Disc,
            "",
        ),
        f(
            0x2a8,
            "unknown_block_2a8",
            OpaqueBlock,
            Disc,
            "stride 0x10 from layout",
        ),
        f(0x2b0, "unknown_nhdt_ref_2b0", TagRef(&["nhdt"]), Disc, ""),
        f(
            0x2b8,
            "unknown_block_2b8",
            OpaqueBlock,
            Disc,
            "stride 8 from layout",
        ),
        f(
            0x2c0,
            "unknown_block_2c0",
            OpaqueBlock,
            Disc,
            "at most one element",
        ),
        f(
            0x2c8,
            "unknown_block_2c8",
            OpaqueBlock,
            Disc,
            "stride 0x40 from layout",
        ),
        f(
            0x2d0,
            "unknown_block_2d0",
            OpaqueBlock,
            Disc,
            "stride 0xec from layout",
        ),
        f(0x2f0, "unknown_vehi_ref_2f0", TagRef(&["vehi"]), Disc, ""),
        f(0x2f8, "unknown_effe_ref_2f8", TagRef(&["effe"]), Disc, ""),
        f(0x300, "unknown_weap_ref_300", TagRef(&["weap"]), Disc, ""),
    ],
};
pub static EQIP: Layout = Layout {
    name: "eqip",
    start: 0x12c,
    end: 0x13c,
    fields: &[
        f(0x12c, "unknown_12c", U16, Disc, ""),
        f(0x12e, "unknown_12e", U16, Disc, ""),
        f(0x134, "unknown_snd_ref_134", TagRef(&["snd!"]), Disc, ""),
    ],
};
pub static GARB: Layout = Layout {
    name: "garb",
    start: 0x12c,
    end: 0x1d4,
    fields: &[],
};
pub static MACH: Layout = Layout {
    name: "mach",
    start: 0x11c,
    end: 0x134,
    fields: &[
        f(
            0x11c,
            "unknown_11c",
            U16,
            Engine,
            "0x001153ee compares the word with 2",
        ),
        f(
            0x11e,
            "unknown_11e",
            U16,
            Engine,
            "0x001151c5 reads its low byte",
        ),
        f(
            0x120,
            "unknown_120",
            F32,
            Engine,
            "0x0011596e compares a float with it",
        ),
    ],
};
pub static CTRL: Layout = Layout {
    name: "ctrl",
    start: 0x11c,
    end: 0x140,
    fields: &[
        f(
            0x11c,
            "unknown_11c",
            U16,
            Engine,
            "0x0011baa3 sign-extends the word",
        ),
        f(
            0x11e,
            "unknown_11e",
            U16,
            Engine,
            "0x0011b9fd compares the word with 0",
        ),
        f(
            0x120,
            "unknown_120",
            F32,
            Engine,
            "0x0011bace loads the float",
        ),
        f(0x124, "unknown_sid_124", StringId, Disc, ""),
        f(
            0x128,
            "unknown_ref_128",
            TagRef(&[]),
            Disc,
            "null on every disc control",
        ),
        f(
            0x130,
            "unknown_ref_130",
            TagRef(&[]),
            Disc,
            "null on every disc control",
        ),
        f(
            0x138,
            "unknown_ref_138",
            TagRef(&[]),
            Disc,
            "null on every disc control",
        ),
    ],
};
pub static SCEN: Layout = Layout {
    name: "scen",
    start: 0xbc,
    end: 0xc4,
    fields: &[],
};
pub static BLOC: Layout = Layout {
    name: "bloc",
    start: 0xbc,
    end: 0xc0,
    fields: &[],
};
pub static SSCE: Layout = Layout {
    name: "ssce",
    start: 0xbc,
    end: 0xcc,
    fields: &[],
};
pub static CREA: Layout = Layout {
    name: "crea",
    start: 0xbc,
    end: 0x180,
    fields: &[
        f(
            0xf4,
            "unknown_block_f4",
            OpaqueBlock,
            Disc,
            "stride 0x80 from layout; same as bipd +0x284",
        ),
        f(
            0xfc,
            "unknown_block_fc",
            OpaqueBlock,
            Disc,
            "stride 0x50 from layout",
        ),
        f(
            0x104,
            "unknown_block_104",
            OpaqueBlock,
            Disc,
            "stride 0x80 from layout",
        ),
        f(0x168, "unknown_jpt_ref_168", TagRef(&["jpt!"]), Disc, ""),
        f(0x170, "unknown_jpt_ref_170", TagRef(&["jpt!"]), Disc, ""),
    ],
};
pub static PROJ: Layout = Layout {
    name: "proj",
    start: 0xbc,
    end: 0x1a4,
    fields: &[
        f(0xe8, "unknown_effe_ref_e8", TagRef(&["effe"]), Disc, ""),
        f(0xf0, "unknown_effe_ref_f0", TagRef(&["effe"]), Disc, ""),
        f(0xf8, "unknown_effe_ref_f8", TagRef(&["effe"]), Disc, ""),
        f(0x100, "unknown_jpt_ref_100", TagRef(&["jpt!"]), Disc, ""),
        f(0x108, "unknown_jpt_ref_108", TagRef(&["jpt!"]), Disc, ""),
        f(0x110, "unknown_effe_ref_110", TagRef(&["effe"]), Disc, ""),
        f(0x118, "unknown_jpt_ref_118", TagRef(&["jpt!"]), Disc, ""),
        f(0x120, "unknown_snd_ref_120", TagRef(&["snd!"]), Disc, ""),
        f(0x12c, "unknown_jpt_ref_12c", TagRef(&["jpt!"]), Disc, ""),
        f(0x138, "unknown_snd_ref_138", TagRef(&["snd!"]), Disc, ""),
        f(0x140, "unknown_effe_ref_140", TagRef(&["effe"]), Disc, ""),
        f(0x148, "unknown_jpt_ref_148", TagRef(&["jpt!"]), Disc, ""),
        f(0x154, "unknown_jpt_ref_154", TagRef(&["jpt!"]), Disc, ""),
        f(0x15c, "unknown_jpt_ref_15c", TagRef(&["jpt!"]), Disc, ""),
        f(
            0x19c,
            "unknown_block_19c",
            OpaqueBlock,
            Disc,
            "stride 0x58 from layout",
        ),
    ],
};

// ---------------------------------------------------------------- hlmt

pub static VARIANT_PERMUTATION: Layout = Layout {
    name: "variant_permutation",
    start: 0,
    end: 0x20,
    fields: &[
        f(
            0x00,
            "permutation",
            StringId,
            Disc,
            "names a permutation of the hlmt region",
        ),
        f(0x04, "unknown_04", U32, Disc, ""),
        f(
            0x08,
            "probability",
            F32,
            Disc,
            "name D: 1.0 on single-permutation regions",
        ),
        f(
            0x0c,
            "unknown_block_0c",
            OpaqueBlock,
            Disc,
            "stride 0x18 from layout",
        ),
    ],
};
pub static VARIANT_REGION: Layout = Layout {
    name: "variant_region",
    start: 0,
    end: 0x14,
    fields: &[
        f(
            0x00,
            "region",
            StringId,
            Disc,
            "equals the name of hlmt region `model_region`",
        ),
        f(
            0x04,
            "model_region",
            U8,
            Disc,
            "index into hlmt regions (+0x70)",
        ),
        f(0x05, "unknown_05", U8, Disc, ""),
        f(0x06, "unknown_06", I16, Disc, ""),
        f(
            0x08,
            "permutations",
            Block(0x20, &VARIANT_PERMUTATION),
            Disc,
            "stride from layout",
        ),
        f(0x10, "unknown_10", U32, Disc, ""),
    ],
};
pub static VARIANT_OBJECT: Layout = Layout {
    name: "variant_object",
    start: 0,
    end: 0x10,
    fields: &[
        f(
            0x00,
            "parent_marker",
            StringId,
            Disc,
            "names a marker group of the parent's render model (the Warthog's `turret`, the Wraith's `left_minigun`)",
        ),
        f(
            0x04,
            "child_marker",
            StringId,
            Disc,
            "zero on the retail vehicles: the child's origin sits on the marker",
        ),
        f(
            0x08,
            "object",
            TagRef(&[]),
            Disc,
            "observed classes are object types (vehi, weap, scen, mach)",
        ),
    ],
};
pub static VARIANT: Layout = Layout {
    name: "variant",
    start: 0,
    end: 0x38,
    fields: &[
        f(
            0x00,
            "name",
            StringId,
            Engine,
            "0x0016d1b6 compares the requested name with element +0",
        ),
        f(
            0x04,
            "region_map",
            Bytes(16),
            Engine,
            "0x0016d764 reads signed byte +4+region: model region -> variant region",
        ),
        f(
            0x14,
            "regions",
            Block(0x14, &VARIANT_REGION),
            Engine,
            "0x0016d78b reads +0x18, index*0x14",
        ),
        f(
            0x1c,
            "objects",
            Block(0x10, &VARIANT_OBJECT),
            Disc,
            "stride from layout",
        ),
        f(0x2c, "unknown_sid_2c", StringId, Disc, ""),
        f(0x30, "unknown_udlg_ref_30", TagRef(&["udlg"]), Disc, ""),
    ],
};
pub static REGION_PERMUTATION: Layout = Layout {
    name: "region_permutation",
    start: 0,
    end: 8,
    fields: &[
        f(0x00, "name", StringId, Disc, ""),
        f(0x04, "unknown_04", Bytes(4), Disc, ""),
    ],
};
pub static REGION: Layout = Layout {
    name: "region",
    start: 0,
    end: 0x10,
    fields: &[
        f(
            0x00,
            "name",
            StringId,
            Engine,
            "0x0016d200 compares the requested name with element +0",
        ),
        f(0x04, "unknown_04", Bytes(4), Disc, ""),
        f(
            0x08,
            "permutations",
            Block(8, &REGION_PERMUTATION),
            Disc,
            "stride from layout",
        ),
    ],
};
pub static NODE: Layout = Layout {
    name: "node",
    start: 0,
    end: 0x5c,
    fields: &[f(0x00, "name", StringId, Disc, "")],
};

pub static HLMT: Layout = Layout {
    name: "hlmt",
    start: 0,
    end: 0xfc,
    fields: &[
        f(0x00, "render_model", TagRef(&["mode"]), Disc, ""),
        f(
            0x08,
            "collision_model",
            TagRef(&["coll"]),
            Engine,
            "0x000b7b40 sets an object flag when +0xc is not null",
        ),
        f(
            0x10,
            "animation",
            TagRef(&["jmad"]),
            Engine,
            "0x000b7b40 tests +0x14 before animation/physics setup",
        ),
        f(0x18, "unknown_phys_ref_18", TagRef(&["phys"]), Disc, ""),
        f(0x20, "physics_model", TagRef(&["phmo"]), Disc, ""),
        f(
            0x50,
            "variants",
            Block(0x38, &VARIANT),
            Engine,
            "0x0016d1bb steps 0x38 over +0x50/+0x54",
        ),
        f(
            0x58,
            "unknown_block_58",
            OpaqueBlock,
            Disc,
            "stride 0x14 from layout",
        ),
        f(
            0x60,
            "unknown_block_60",
            OpaqueBlock,
            Disc,
            "at most one element; object_new reads its +0xbc",
        ),
        f(
            0x68,
            "unknown_block_68",
            OpaqueBlock,
            Disc,
            "stride 0x1c from layout",
        ),
        f(
            0x70,
            "regions",
            Block(0x10, &REGION),
            Engine,
            "0x0016d205 steps 0x10 over +0x70/+0x74; object_new reserves 10 bytes each",
        ),
        f(
            0x78,
            "nodes",
            Block(0x5c, &NODE),
            Disc,
            "object_new reserves 0x34 bytes per element (D: node matrices)",
        ),
        f(
            0x84,
            "unknown_block_84",
            OpaqueBlock,
            Disc,
            "at most one element",
        ),
        f(0x8c, "unknown_udlg_ref_8c", TagRef(&["udlg"]), Disc, ""),
        f(0x94, "unknown_shad_ref_94", TagRef(&["shad"]), Disc, ""),
        f(0xf0, "unknown_shad_ref_f0", TagRef(&["shad"]), Disc, ""),
        f(0xf8, "unknown_sid_f8", StringId, Disc, ""),
    ],
};

// ---------------------------------------------------------------- collections

pub static COLLECTION_ENTRY: Layout = Layout {
    name: "entry",
    start: 0,
    end: 0x10,
    fields: &[
        f(
            0x00,
            "weight",
            F32,
            Engine,
            "0x0019f200 subtracts it from the random roll",
        ),
        f(
            0x04,
            "object",
            TagRef(&[]),
            Engine,
            "0x0019f219 returns +8 (the datum)",
        ),
        f(
            0x0c,
            "variant",
            StringId,
            Engine,
            "0x0019f21e returns +0xc alongside the datum",
        ),
    ],
};
pub static COLLECTION: Layout = Layout {
    name: "collection",
    start: 0,
    end: 0xc,
    fields: &[f(
        0x00,
        "entries",
        Block(0x10, &COLLECTION_ENTRY),
        Engine,
        "0x0019f20a steps 0x10",
    )],
};
