//! Where each placement block lives in the scenario (`scnr`) root, its element
//! stride, and the fields that scenario-edit reads and edits.
//!
//! Evidence labels follow docs/game/forge.md: **E** read from the retail XBE,
//! **V** checked over every scenario on the owner's disc, **C** a public
//! community layout (cross-check only). Every byte of an element that is not
//! listed here is preserved untouched; see docs/editor/scenario-editor.md for the
//! per-field addresses.

/// The scenario blocks the editor and Forge share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlockKind {
    /// The scenery block.
    Scenery,
    /// The vehicles block.
    Vehicles,
    /// The equipment block.
    Equipment,
    /// The weapons block.
    Weapons,
    /// The machines block.
    Machines,
    /// The sound scenery block.
    SoundScenery,
    /// The crate placement block.
    Crates,
    /// The starting locations block.
    StartingLocations,
    /// The netgame flags block.
    NetgameFlags,
    /// The netgame equipment block.
    NetgameEquipment,
    /// The starting equipment block.
    StartingEquipment,
}

impl BlockKind {
    /// The name used in scenario edit scripts and Forge documents.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scenery => "scenery",
            Self::Vehicles => "vehicles",
            Self::Equipment => "equipment",
            Self::Weapons => "weapons",
            Self::Machines => "machines",
            Self::SoundScenery => "sound_scenery",
            Self::Crates => "crates",
            Self::StartingLocations => "starting_locations",
            Self::NetgameFlags => "netgame_flags",
            Self::NetgameEquipment => "netgame_equipment",
            Self::StartingEquipment => "starting_equipment",
        }
    }

    /// Look up a block kind by its script name.
    pub fn from_name(name: &str) -> Option<Self> {
        BLOCKS.iter().map(|b| b.kind).find(|k| k.name() == name)
    }
}

/// A field's storage type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    /// Unsigned byte.
    U8,
    /// Signed 16-bit integer.
    I16,
    /// Unsigned 16-bit integer.
    U16,
    /// Unsigned 32-bit integer.
    U32,
    /// 32-bit float.
    F32,
    /// Three consecutive f32s (a point, or yaw/pitch/roll in radians).
    Vec3,
    /// An 8-byte tag reference: group FourCC (as a little-endian u32), then
    /// the full salted tag ID.
    TagRef,
}
impl FieldType {
    /// The field width in bytes.
    pub const fn size(self) -> u32 {
        match self {
            Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::U32 | Self::F32 => 4,
            Self::Vec3 => 12,
            Self::TagRef => 8,
        }
    }
}

/// What a field means to the editor (drives the viewport and validation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// World position, game units, z up.
    Position,
    /// Yaw, pitch, roll in radians.
    Rotation,
    /// A single yaw angle in radians.
    Facing,
    /// Index into the block's palette; -1 means none.
    PaletteIndex,
    /// Uniform scale; 0 means 1.
    Scale,
    /// Anything else: shown and edited as a number.
    Plain,
}

/// One field of a block element.
#[derive(Clone, Copy, Debug)]
pub struct FieldDef {
    /// The editor field name.
    pub name: &'static str,
    /// Byte offset in the element.
    pub offset: u32,
    /// How the field is stored.
    pub ty: FieldType,
    /// How the editor treats the field.
    pub role: Role,
    /// Whether the editor lets the user change it.
    pub editable: bool,
    /// Evidence label and address or check, shown in the inspector.
    pub evidence: &'static str,
}

const fn f(
    name: &'static str,
    offset: u32,
    ty: FieldType,
    role: Role,
    editable: bool,
    evidence: &'static str,
) -> FieldDef {
    FieldDef {
        name,
        offset,
        ty,
        role,
        editable,
        evidence,
    }
}

/// A block of the scenario root.
#[derive(Debug)]
pub struct BlockDef {
    /// The block this definition describes.
    pub kind: BlockKind,
    /// The editor label.
    pub label: &'static str,
    /// Offset of the (count, address) pair in the `scnr` root.
    pub offset: u32,
    /// The width of one element in bytes.
    pub stride: u32,
    /// Evidence for the element width.
    pub stride_evidence: &'static str,
    /// The palette block, if the element's palette index selects from one.
    pub palette: Option<PaletteDef>,
    /// The typed fields in an element.
    pub fields: &'static [FieldDef],
    /// Why entries may not be swap-removed, or `None` when nothing in the
    /// map refers to them by index (docs/editor/scenario-editor.md §6).
    pub remove_refusal: Option<&'static str>,
    /// Why entries may not be added, or `None`.
    pub add_refusal: Option<&'static str>,
    /// The most entries the engine accepts. Object placements: the
    /// per-type maximums at `0x0044074c`, which `0x000d5640` sums into one
    /// bitmask without checking the index (E). Netgame equipment: the
    /// runtime array at `[0x4e9ae8]+0x7dc` is cleared for 100 entries
    /// (`0x0015acf4`) and the index travels as an i8 (`0x0015ac15`) (E).
    pub max_count: Option<usize>,
    /// The object type (index into the type table `0x00468630`) of an
    /// object placement block: scenario object names name a placement by
    /// (type, index).
    pub object_type: Option<i16>,
}

/// Scenario object names: `(count, address)` at `scnr+0x48`, stride `0x24`
/// (E: `object_create` by name `0x000bb670`, `lea ecx,[eax+eax*8]` on
/// `[scnr+0x4c]` at `0x000bb702`). Each entry names one placement: object
/// type i16 at `+0x20`, placement index i16 at `+0x22` (-1: nothing is
/// created, `0x000bb716`). At most 640 names (`cmp ax,280h`, `0x000bb765`).
pub const OBJECT_NAMES: u32 = 0x48;
/// Width of an object name element.
pub const OBJECT_NAME_STRIDE: u32 = 0x24;
/// Offset of the placement type in an object name element.
pub const OBJECT_NAME_TYPE: usize = 0x20;
/// Offset of the placement index in an object name element.
pub const OBJECT_NAME_PLACEMENT: usize = 0x22;

/// A palette block: elements start with a tag reference.
#[derive(Debug)]
pub struct PaletteDef {
    /// Offset of the palette pair in the scenario root.
    pub offset: u32,
    /// Width of one palette element.
    pub stride: u32,
    /// Tag groups a palette entry may name.
    pub groups: &'static [[u8; 4]],
}

use FieldType::*;
use Role::*;

/// The common head of every scenario object placement record. Offsets are
/// the public scenario object layout (C); live-art.md checked palette index,
/// position, rotation and scale by machines landing on their levels (V), and
/// scenario-edit's disc test checks them on every object block (V).
const OBJECT: &[FieldDef] = &[
    f(
        "palette index",
        0x00,
        I16,
        PaletteIndex,
        true,
        "V: below the palette count on disc; C",
    ),
    f(
        "name index",
        0x02,
        I16,
        Plain,
        false,
        "C: -1 or a scenario object name",
    ),
    f("placement flags", 0x04, U32, Plain, true, "C"),
    f(
        "position",
        0x08,
        Vec3,
        Position,
        true,
        "V: inside the BSP bounds; C",
    ),
    f(
        "rotation",
        0x14,
        Vec3,
        Rotation,
        true,
        "V: yaw/pitch/roll radians (live-art.md); C",
    ),
    f(
        "scale",
        0x20,
        F32,
        Scale,
        true,
        "V: 0 means 1 (live-art.md); C",
    ),
];

/// Object palettes: a tag reference at +0, stride 0x28 (E for the stride
/// via the type table pairing, V for the reference).
const fn palette(offset: u32, groups: &'static [[u8; 4]]) -> Option<PaletteDef> {
    Some(PaletteDef {
        offset,
        stride: 0x28,
        groups,
    })
}

/// Every block, in `scnr` order.
pub static BLOCKS: &[BlockDef] = &[
    BlockDef {
        kind: BlockKind::Scenery,
        label: "Scenery",
        offset: 0x50,
        stride: 0x5c,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0x58, std::slice::from_ref(b"scen")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(2000),
        object_type: Some(6),
    },
    BlockDef {
        kind: BlockKind::Vehicles,
        label: "Vehicles",
        offset: 0x70,
        stride: 0x54,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0x78, std::slice::from_ref(b"vehi")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(256),
        object_type: Some(1),
    },
    BlockDef {
        kind: BlockKind::Equipment,
        label: "Equipment",
        offset: 0x80,
        stride: 0x38,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0x88, std::slice::from_ref(b"eqip")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(256),
        object_type: Some(3),
    },
    BlockDef {
        kind: BlockKind::Weapons,
        label: "Weapons",
        offset: 0x90,
        stride: 0x54,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0x98, std::slice::from_ref(b"weap")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(128),
        object_type: Some(2),
    },
    BlockDef {
        kind: BlockKind::Machines,
        label: "Machines",
        offset: 0xa8,
        stride: 0x48,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0xb0, std::slice::from_ref(b"mach")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(400),
        object_type: Some(7),
    },
    BlockDef {
        kind: BlockKind::SoundScenery,
        label: "Sound scenery",
        offset: 0xd8,
        stride: 0x50,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0xe0, std::slice::from_ref(b"ssce")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(256),
        object_type: Some(10),
    },
    BlockDef {
        kind: BlockKind::Crates,
        label: "Crates",
        offset: 0x328,
        stride: 0x4c,
        stride_evidence: "E: object type table 0x00468630",
        palette: palette(0x330, std::slice::from_ref(b"bloc")),
        fields: OBJECT,
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(1024),
        object_type: Some(11),
    },
    BlockDef {
        kind: BlockKind::StartingLocations,
        label: "Player starting locations",
        offset: 0x100,
        stride: 0x34,
        stride_evidence: "E: 0x0014ef96 imul 0x34, 0x0014bcb8 add 0x34",
        palette: None,
        fields: &[
            f(
                "position",
                0x00,
                Vec3,
                Position,
                true,
                "E: 0x0023cec7..0x0023ced8",
            ),
            f("facing", 0x0c, F32, Facing, true, "E: 0x0023cee1"),
            f("team", 0x10, I16, Plain, true, "V: small values; C"),
            f(
                "player type",
                0x2c,
                I16,
                Plain,
                false,
                "E: 0x0014bcb0 compares with -1",
            ),
        ],
        remove_refusal: None,
        add_refusal: None,
        max_count: None,
        object_type: None,
    },
    BlockDef {
        kind: BlockKind::NetgameFlags,
        label: "Netgame flags",
        offset: 0x118,
        stride: 0x20,
        stride_evidence: "E: 0x0019cbcb shl 5",
        palette: None,
        fields: &[
            f(
                "position",
                0x00,
                Vec3,
                Position,
                true,
                "E: 0x0019cbce..0x0019cbd9",
            ),
            f("facing", 0x0c, F32, Facing, true, "E: 0x0019cf65"),
            f("type", 0x10, U16, Plain, true, "V: small values; C"),
            f("team", 0x12, U16, Plain, true, "V: small values; C"),
            f(
                "identifier",
                0x14,
                I16,
                Plain,
                true,
                "V: teleporter channel (sources pair with destinations of the same value on coagulation, waterworks, beavercreek; team there is 8 or mixed)",
            ),
        ],
        remove_refusal: None,
        add_refusal: None,
        max_count: None,
        object_type: None,
    },
    BlockDef {
        kind: BlockKind::NetgameEquipment,
        label: "Netgame equipment",
        offset: 0x120,
        stride: 0x90,
        stride_evidence: "E: 0x0015aeea (i*9)<<4",
        palette: None,
        fields: &[
            f("flags", 0x00, U32, Plain, true, "E: bit 0 at 0x0015aad0"),
            f(
                "game type 1",
                0x04,
                I16,
                Plain,
                true,
                "E: 0x0015af2b passes +0x04, 4 slots, to the matcher 0x0015a090",
            ),
            f("game type 2", 0x06, I16, Plain, true, "E: slot 2 of 0x0015a090"),
            f("game type 3", 0x08, I16, Plain, true, "E: slot 3 of 0x0015a090"),
            f(
                "game type 4",
                0x0a,
                I16,
                Plain,
                true,
                "E: slot 4 of 0x0015a090 (push 4 at 0x0015af2e)",
            ),
            f(
                "respawn time",
                0x0e,
                I16,
                Plain,
                true,
                "E: 0x0015ac6d, 0 = collection default",
            ),
            f(
                "abandon limit",
                0x10,
                I16,
                Plain,
                true,
                "E: 0x0015af7d / 0x0015b020",
            ),
            f(
                "respawn mode",
                0x12,
                I16,
                Plain,
                true,
                "E: 0x0015b05d: what starts the respawn countdown while the object exists; 0 or 1, any other never",
            ),
            f(
                "type",
                0x14,
                U8,
                Plain,
                true,
                "E: 0x0015b0ef, 0 / 1..7 / 8..9",
            ),
            f(
                "position",
                0x40,
                Vec3,
                Position,
                true,
                "E: 0x0015afa8..0x0015afbc",
            ),
            f(
                "orientation",
                0x4c,
                Vec3,
                Rotation,
                true,
                "E: offset 0x0015aad0; V",
            ),
            f("item", 0x58, TagRef, Plain, true, "E: 0x0015ac83 itmc/vehc"),
        ],
        remove_refusal: None,
        add_refusal: None,
        max_count: Some(100),
        object_type: None,
    },
    BlockDef {
        kind: BlockKind::StartingEquipment,
        label: "Starting equipment",
        offset: 0x128,
        stride: 0x9c,
        stride_evidence: "E: 0x0019efa6 add 0x9c",
        palette: None,
        fields: &[
            f(
                "collection 1",
                0x3c,
                TagRef,
                Plain,
                false,
                "E: ids at 0x0019efe1 loop",
            ),
            f(
                "collection 2",
                0x44,
                TagRef,
                Plain,
                false,
                "E: 0x0019f00b +8",
            ),
            f("collection 3", 0x4c, TagRef, Plain, false, "E"),
            f("collection 4", 0x54, TagRef, Plain, false, "E"),
            f(
                "collection 5",
                0x5c,
                TagRef,
                Plain,
                false,
                "E: loop bound 5",
            ),
        ],
        remove_refusal: None,
        add_refusal: None,
        max_count: None,
        object_type: None,
    },
];

/// Groups a netgame equipment `item` reference may name: a collection, or any
/// placeable object (E: the spawner rolls `itmc`/`vehc`, else places the tag).
pub const ITEM_GROUPS: &[[u8; 4]] = &[
    *b"itmc", *b"vehc", *b"weap", *b"eqip", *b"vehi", *b"scen", *b"bloc", *b"mach",
];

/// Look a block up by kind.
pub fn block(kind: BlockKind) -> &'static BlockDef {
    BLOCKS
        .iter()
        .find(|b| b.kind == kind)
        .expect("every kind has a definition")
}

impl BlockDef {
    /// A field by name.
    pub fn field(&self, name: &str) -> Option<&'static FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
    /// The first field with a role.
    pub fn role(&self, role: Role) -> Option<&'static FieldDef> {
        self.fields.iter().find(|f| f.role == role)
    }
}

/// The values of a netgame equipment entry's `respawn mode` the tick
/// `0x0015ae70` acts on (E: `0x0015b05d`). While the entry's object exists
/// and no countdown runs, mode 0 starts the countdown when the object is of
/// type 2 to 4 with bit 0 or 1 of its byte `+0x12c` set, or lies farther
/// from the entry's position than the distance at `0x0045dbc0`; mode 1
/// starts it when bit 2 of the object's byte `+0x10a` is set; any other
/// value never starts it. An entry without an object, and every entry
/// after the reset `0x0015ad30`, respawns whatever its mode.
pub const RESPAWN_MODES: std::ops::RangeInclusive<i16> = 0..=1;

/// A netgame equipment entry's game-type slots and the engine's matcher
/// `0x0015a090`. Its only callers are the netgame equipment tick
/// (`0x0015af32`) and the starting equipment pass (`0x0019ef91`, slots at
/// the same `+0x04`) (E, a search of `.text`). No reader of a starting
/// location's bytes past `+0x10` but `+0x2c` was found:
/// docs/editor/scenario-editor.md §2.
pub mod game_types {
    /// Offset of the first slot in a netgame equipment entry (E: `lea
    /// edi,[ebp+4]` at `0x0015af2b`). Each slot is an `i16`.
    pub const ITEM_SLOTS: u32 = 0x04;
    /// How many slots the matcher reads (E: `push 4` at `0x0015af2e`).
    pub const SLOT_COUNT: usize = 4;
    /// The codes the engines pass: slot `+0x00` of each engine's vtable
    /// (E, docs/engine/notes/2026-11-21-engine-vtables.md). King of the
    /// Hill 0, CTF 1, Slayer 2, Oddball 3, Juggernaut 7, Territories 8,
    /// Assault 9.
    pub const ENGINE_CODES: [i32; 7] = [0, 1, 2, 3, 7, 8, 9];
    /// A slot that matches every engine (E: `0x0015a0db`).
    pub const ALL: i16 = 12;
    /// Slots that match every engine but codes 1 and 9 (E: `0x0015a0e6`,
    /// `0x0015a0fd`; the two take the same path).
    pub const ALL_BUT_1_AND_9: [i16; 2] = [13, 14];
    /// The highest value the matcher names. Values outside `0..=MAX`, and
    /// 4, 5, 6, 10 and 11, match no engine's code.
    pub const MAX: i16 = 14;

    /// Whether one slot matches: `engine` is the running engine's code, or
    /// `None` when the engine table holds no object for the game, where
    /// only a slot of 0 matches (E: `0x0015a0ae`, `0x0015a126`).
    pub fn slot_matches(slot: i16, engine: Option<i32>) -> bool {
        let Some(code) = engine else {
            return slot == 0;
        };
        let flag = |v: i32| v == 1 || v == 9;
        i32::from(slot) == code
            || (flag(code) && flag(i32::from(slot)))
            || slot == ALL
            || (ALL_BUT_1_AND_9.contains(&slot) && !flag(code))
    }

    /// The first matching slot, as `0x0015a090` returns it; `None` (the
    /// routine's -1) leaves the entry out of the game.
    pub fn first_match(slots: &[i16], engine: Option<i32>) -> Option<usize> {
        slots.iter().position(|&s| slot_matches(s, engine))
    }

    /// The slots in a netgame equipment entry's bytes, or `None` when the
    /// entry is too short.
    pub fn item_slots(entry: &[u8]) -> Option<[i16; SLOT_COUNT]> {
        let at = ITEM_SLOTS as usize;
        let bytes = entry.get(at..at + 2 * SLOT_COUNT)?;
        Some(std::array::from_fn(|k| {
            i16::from_le_bytes([bytes[2 * k], bytes[2 * k + 1]])
        }))
    }
}
