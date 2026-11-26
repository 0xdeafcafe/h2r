//! Block-out pieces in a session (docs/game/forge-blockout.md, "In the
//! document" and "Reaching the game"; the session's methods are
//! `session/pieces.rs`).
//!
//! A piece is a crate the document adds with the piece tag and a
//! definition ([`forge_shapes::PieceDef`]). In the world it is one object of
//! the definition's own `bloc` (`forge\blockout\<name>`), made only when the
//! loaded map holds that definition ([`crate::World::piece_loaded`]);
//! otherwise the piece is **pending**: it is in the document and has no
//! object, until the map is rebuilt and the game reports what it made
//! ([`crate::Session::adopt_copy`]).
//!
//! The three states are [`PieceState`]: `Live` (an object of the
//! definition's `bloc` stands), `Slot` (an object of a spare definition
//! rewritten to this one, slice BO10) and `Pending`.
//!
//! A piece's body is fixed where its object was made, so a piece is never
//! moved in the world: a drop deletes the object and makes it again where
//! the document puts it ([`drop`]), and a piece that is held stays where it
//! was while the monitor draws it as a ghost.
use forge_doc::{BlockKind, Endpoint, OpId, TagKey};
use forge_shapes::{Material, Materials, PieceDef, Role, Shape, units};
use settings::{Apply, Choice, Def, Kind, Scope, Tier, Value};

use crate::tool::Extent;
use crate::{Frame, PaletteItem, Session, World};

/// The `bloc` of a definition's tags: what an object of the piece is made
/// from.
pub fn bloc_key(def: &PieceDef) -> TagKey {
    TagKey {
        group: forge_doc::PIECE_GROUP.into(),
        path: def.tag_path(),
    }
}

/// The crate palette entry a definition's object is made from.
pub fn item(def: &PieceDef) -> PaletteItem {
    PaletteItem {
        block: BlockKind::Crates,
        tag: bloc_key(def),
    }
}

/// Whether a piece has an object in the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieceState {
    /// An object of the definition's own `bloc` stands.
    Live,
    /// An object of a spare definition, written to this one (BO10; nothing
    /// in this slice gives it).
    Slot,
    /// In the document, with no object until **Rebuild map**.
    Pending,
}

impl PieceState {
    /// The state as `forge.pieces` spells it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Slot => "slot",
            Self::Pending => "pending",
        }
    }
}

/// The axis-aligned box a piece fills in the world, in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlacedBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One piece of the session.
#[derive(Clone, Debug, PartialEq)]
pub struct PieceInfo {
    /// The op that added the placement; the target of its `Forge::Piece`.
    pub id: OpId,
    /// The placement's index in the crates block after every op.
    pub index: u32,
    pub def: PieceDef,
    /// The definition's canonical name ([`PieceDef::name`]).
    pub name: String,
    pub state: PieceState,
    /// Where the document puts it: its origin, the lowest corner of its
    /// bounds, and its turn.
    pub frame: Frame,
    pub bounds: PlacedBounds,
}

impl PieceInfo {
    /// The endpoint an op names the piece by.
    pub fn target(&self) -> Endpoint {
        Endpoint::Added(self.id)
    }

    /// The piece's bounding sphere in its own axes: the centre of its box
    /// and the distance to a corner.
    pub fn extent(&self) -> Extent {
        let half = half_size(&self.def);
        Extent {
            radius: (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt(),
            offset: half,
        }
    }
}

/// The size of a definition in world units.
pub fn size(def: &PieceDef) -> [f32; 3] {
    def.size.map(units::world)
}

fn half_size(def: &PieceDef) -> [f32; 3] {
    size(def).map(|s| s / 2.0)
}

/// The box `def` fills with its origin at `frame.position`, turned by
/// `frame.rotation` (yaw about z, then pitch about y, then roll about x,
/// applied to the box as roll first). A quarter turn of yaw gives the
/// exact box on the grid.
pub fn bounds(def: &PieceDef, frame: Frame) -> PlacedBounds {
    let [sx, sy, sz] = size(def);
    let [yaw, pitch, roll] = frame.rotation;
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sy_, cy) = yaw.sin_cos();
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for i in 0..8u8 {
        let c = [
            if i & 1 == 0 { 0.0 } else { sx },
            if i & 2 == 0 { 0.0 } else { sy },
            if i & 4 == 0 { 0.0 } else { sz },
        ];
        // Roll about x, pitch about y, yaw about z.
        let a = [c[0], c[1] * cr - c[2] * sr, c[1] * sr + c[2] * cr];
        let b = [a[0] * cp + a[2] * sp, a[1], -a[0] * sp + a[2] * cp];
        let d = [b[0] * cy - b[1] * sy_, b[0] * sy_ + b[1] * cy, b[2]];
        for k in 0..3 {
            let v = frame.position[k] + d[k];
            min[k] = min[k].min(v);
            max[k] = max[k].max(v);
        }
    }
    PlacedBounds { min, max }
}

/// Drop piece `id` at `frame`: the document takes the new position and
/// turn as one undo step (a group goes with it), and the world deletes the
/// object and makes it again there, so its fixed body is built where it
/// lands. A piece with no object (pending) only changes in the document.
pub fn drop(
    session: &mut Session,
    world: &mut impl World,
    tick: u32,
    id: OpId,
    frame: Frame,
) -> Result<(), String> {
    session.move_thing(world, tick, Endpoint::Added(id), frame)
}

/// The setting keys of the monitor's block-out grid and dev view
/// (docs/game/forge-blockout.md, "In the monitor").
pub const GRID_KEY: &str = "forge.blockout.grid";
pub const DEV_VIEW_KEY: &str = "forge.blockout.dev_view";

const fn grid_choice(units: i64, name: &'static str) -> Choice {
    Choice {
        value: units,
        name,
        label: name,
    }
}

const GRID_CHOICES: &[Choice] = &[
    grid_choice(1, "1"),
    grid_choice(2, "2"),
    grid_choice(4, "4"),
    grid_choice(8, "8"),
    grid_choice(16, "16"),
    grid_choice(32, "32"),
    grid_choice(64, "64"),
];

/// The block-out grid in units: rounds a held piece's position and a size
/// being changed.
pub static GRID: Def = Def {
    key: GRID_KEY,
    label: "Block-out grid",
    help: "The grid, in block-out units (a hundredth of a world unit), a held or resized \
           piece rounds to. A power of two, 1 to 64.",
    kind: Kind::Choice(GRID_CHOICES),
    default: Value::Int(units::DEFAULT_GRID as i64),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: None,
};

/// Draw the pieces with their dev textures (the Overlays page's Dev view).
pub static DEV_VIEW: Def = Def {
    key: DEV_VIEW_KEY,
    label: "Dev view",
    help: "Draw every block-out piece with its dev texture, whatever its material. \
           Presentation on this machine only.",
    kind: Kind::Bool,
    default: Value::Bool(false),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: None,
};

/// The Structure group's shapes, in the object menu's order.
pub const STRUCTURE: [Shape; 8] = [
    Shape::Block,
    Shape::Wall,
    Shape::FloorPlate,
    Shape::Ramp,
    Shape::Wedge,
    Shape::Stairs,
    Shape::Cylinder,
    Shape::Arch,
];

/// The Natural group's shapes, each with [`SEEDS`] seeds.
pub const NATURAL: [Shape; 5] = [
    Shape::Boulder,
    Shape::Slab,
    Shape::Cliff,
    Shape::TerrainWedge,
    Shape::Spire,
];

/// How many seeds of each natural shape the object menu lists.
pub const SEEDS: u16 = 8;

/// A shape as the menu names it.
pub fn title(shape: Shape) -> &'static str {
    match shape {
        Shape::Block => "Block",
        Shape::Wall => "Wall",
        Shape::FloorPlate => "Floor plate",
        Shape::Ramp => "Ramp",
        Shape::Wedge => "Wedge",
        Shape::Stairs => "Stairs",
        Shape::Cylinder => "Cylinder",
        Shape::Arch => "Arch",
        Shape::Boulder => "Boulder",
        Shape::Slab => "Slab",
        Shape::Cliff => "Cliff",
        Shape::TerrainWedge => "Terrain wedge",
        Shape::Spire => "Spire",
    }
}

/// The definition the object menu starts `shape` at, every role `Dev`; a
/// natural shape takes `seed`. Each structural start is in
/// `forge_shapes::STARTER`, so it places with no rebuild.
pub fn start(shape: Shape, seed: u16) -> PieceDef {
    let (size, param) = match shape {
        Shape::Block => ([64, 64, 64], 0),
        Shape::Wall => ([128, 16, 128], 0),
        Shape::FloorPlate => ([128, 128, 16], 0),
        Shape::Ramp | Shape::Wedge => ([128, 128, 64], 0),
        Shape::Stairs => ([128, 64, 64], 4),
        Shape::Cylinder => ([64, 64, 128], 16),
        Shape::Arch => ([256, 32, 192], 8),
        Shape::Boulder => ([96, 96, 64], seed),
        Shape::Slab => ([256, 192, 32], seed),
        Shape::Cliff => ([256, 64, 256], seed),
        Shape::TerrainWedge => ([256, 256, 96], seed),
        Shape::Spire => ([64, 64, 256], seed),
    };
    PieceDef::dev(shape, size, param)
}

/// Whether a palette item is a block-out definition's `bloc`.
pub fn is_piece_item(item: &PaletteItem) -> bool {
    item.block == BlockKind::Crates
        && item.tag.group == forge_doc::PIECE_GROUP
        && item.tag.path.starts_with(PIECE_DIR)
}

const PIECE_DIR: &str = "forge\\blockout\\";

/// The names of the definitions the palette holds `bloc`s of: what the
/// loaded map has ([`crate::World::piece_loaded`]).
pub fn loaded_names(palette: &[PaletteItem]) -> std::collections::BTreeSet<String> {
    palette
        .iter()
        .filter(|item| is_piece_item(item))
        .filter_map(|item| item.tag.path.strip_prefix(PIECE_DIR))
        .map(str::to_string)
        .collect()
}

/// The bounding sphere of a definition's box, in its own axes.
pub fn extent(def: &PieceDef) -> Extent {
    let half = half_size(def);
    Extent {
        radius: (half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt(),
        offset: half,
    }
}

/// `v`, a vector in a piece's own axes, in the world for a piece turned by
/// `rotation` ([`bounds`]' turn).
pub fn turned(rotation: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let [yaw, pitch, roll] = rotation;
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let a = [c[0], c[1] * cr - c[2] * sr, c[1] * sr + c[2] * cr];
    let b = [a[0] * cp + a[2] * sp, a[1], -a[0] * sp + a[2] * cp];
    [b[0] * cy - b[1] * sy, b[0] * sy + b[1] * cy, b[2]]
}

/// A face of a piece's box: the axis (0 x, 1 y, 2 z) and whether it is the
/// face at the size (`far`) or at the origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub axis: usize,
    pub far: bool,
}

/// What the axis is called on the Size page.
pub fn axis_name(axis: usize) -> &'static str {
    ["Width", "Depth", "Height"][axis.min(2)]
}

/// The face of the box of `def` at `frame` that the ray from `eye` along
/// `dir` meets first; the face it leaves by when `eye` is inside.
pub fn pick_face(def: &PieceDef, frame: Frame, eye: [f32; 3], dir: [f32; 3]) -> Option<Face> {
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].map(|e| turned(frame.rotation, e));
    let rel: [f32; 3] = std::array::from_fn(|i| eye[i] - frame.position[i]);
    let o: [f32; 3] = std::array::from_fn(|i| dot(rel, axes[i]));
    let d: [f32; 3] = std::array::from_fn(|i| dot(dir, axes[i]));
    let s = size(def);
    let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
    let (mut entered, mut left) = (None, None);
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if o[i] < 0. || o[i] > s[i] {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((0. - o[i]) / d[i], (s[i] - o[i]) / d[i]);
        let rising = t0 < t1;
        let (enter, exit) = if rising { (t0, t1) } else { (t1, t0) };
        if enter > near {
            near = enter;
            entered = Some(Face { axis: i, far: !rising });
        }
        if exit < far {
            far = exit;
            left = Some(Face { axis: i, far: rising });
        }
    }
    if near > far || far < 0. {
        return None;
    }
    if near >= 0. { entered } else { left }
}

/// The size one grid step from `size` in direction `dir`: the next
/// multiple of the grid above (`dir` positive) or below it.
pub fn step_size(size: u16, grid: u16, dir: i32) -> i32 {
    let (s, g) = (i32::from(size), i32::from(grid.max(1)));
    if dir > 0 {
        (s.div_euclid(g) + 1) * g
    } else {
        (s - 1).div_euclid(g) * g
    }
}

/// Move `face` so the piece is `new_size` units along its axis, the
/// opposite face staying where it is: the new definition and origin. A
/// size the definition refuses (past the minimum or the maximum, or one its
/// shape cannot generate) is refused with the check's sentence.
pub fn resize_to(
    def: &PieceDef,
    frame: Frame,
    face: Face,
    new_size: i32,
) -> Result<(PieceDef, Frame), String> {
    let old = i32::from(def.size[face.axis]);
    let mut next = def.clone();
    next.size[face.axis] = new_size.clamp(0, i32::from(u16::MAX)) as u16;
    next.generate().map_err(|e| e.to_string())?;
    let mut origin = frame;
    if !face.far {
        let mut shift = [0.; 3];
        shift[face.axis] = -((new_size - old) as f32) / 100.0;
        let moved = turned(frame.rotation, shift);
        origin.position = std::array::from_fn(|i| frame.position[i] + moved[i]);
    }
    Ok((next, origin))
}

/// [`resize_to`] one grid step in direction `dir`.
pub fn resize_step(
    def: &PieceDef,
    frame: Frame,
    face: Face,
    dir: i32,
    grid: u16,
) -> Result<(PieceDef, Frame), String> {
    resize_to(def, frame, face, step_size(def.size[face.axis], grid, dir))
}

/// The next seed of a natural piece.
pub fn reroll(def: &PieceDef) -> Result<PieceDef, String> {
    if !def.shape.is_natural() {
        return Err("Reroll is for a rock: a Natural piece.".into());
    }
    let mut next = def.clone();
    next.param = next.param.wrapping_add(1);
    Ok(next)
}

/// `def` with `material` on `role`, or on every role with none.
pub fn with_material(def: &PieceDef, role: Option<Role>, material: Material) -> PieceDef {
    let mut next = def.clone();
    next.materials = match role {
        None => Materials::All(material),
        Some(role) => {
            let mut each = match &def.materials {
                Materials::All(m) => [m.clone(), m.clone(), m.clone(), m.clone()],
                Materials::Each(each) => each.clone(),
            };
            each[role.index()] = material;
            if each.iter().all(|m| *m == each[0]) {
                Materials::All(each[0].clone())
            } else {
                Materials::Each(each)
            }
        }
    };
    next
}
