//! The block-out screens (docs/game/forge-blockout.md, "In the monitor"):
//! the object menu's Structure and Natural groups, and a piece's Properties
//! rows (Resize, Size, Material, Reroll). Every row is a value picker, a
//! typed number or an action; none draws arrows.
use forge::piece::{self, NATURAL, SEEDS, STRUCTURE};
use forge::{Material, OpId, PieceDef, Shape};
use forge_shapes::Role;

use super::sheet::{
    Act, Choice, Group, Kind, NumberField, PickSpec, PieceField, Sheet, SheetRow, TypeSpec,
    add_pick_rows,
};

/// What a piece's Properties rows are built from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceFacts {
    pub id: OpId,
    pub def: PieceDef,
    /// The shaders of the map's surfaces, as `content_kit::blockout::palette::survey`
    /// ranks them, for the Material picker after **Dev**.
    pub surfaces: Vec<String>,
}

/// A size as a line says it: "128 by 16 by 128 units".
pub fn size_line(def: &PieceDef) -> String {
    let [x, y, z] = def.size;
    format!("{x} by {y} by {z} units")
}

/// A piece's name in the readout, Properties and the placed objects
/// browser: "Block 64 by 64 by 64", "Boulder 3".
pub fn name(def: &PieceDef) -> String {
    let title = piece::title(def.shape);
    if def.shape.is_natural() {
        format!("{title} {}", u32::from(def.param) + 1)
    } else {
        format!("{title} {}", size_line(def).trim_end_matches(" units"))
    }
}

/// `sheet` (or an empty object menu) with the Structure and Natural groups.
pub fn with_groups(sheet: Option<Sheet>) -> Sheet {
    let mut sheet = sheet.unwrap_or(Sheet {
        kind: Kind::Objects,
        title: "OBJECTS".into(),
        body: String::new(),
        rows: Vec::new(),
        focus: 0,
    });
    let structure = STRUCTURE
        .iter()
        .map(|&shape| {
            (
                piece::title(shape).to_string(),
                format!("Structure. {}.", size_line(&piece::start(shape, 0))),
                Choice::Piece(shape, 0),
            )
        })
        .collect();
    add_pick_rows(&mut sheet, Group::Structure, structure);
    let natural = NATURAL
        .iter()
        .flat_map(|&shape| {
            (0..SEEDS).map(move |seed| {
                (
                    format!("{} {}", piece::title(shape), seed + 1),
                    format!("Natural. {}.", size_line(&piece::start(shape, seed))),
                    Choice::Piece(shape, seed),
                )
            })
        })
        .collect();
    add_pick_rows(&mut sheet, Group::Natural, natural);
    sheet
}

/// The word the Size page's last row goes by, for a shape that has one.
fn param_label(shape: Shape) -> Option<&'static str> {
    match shape {
        Shape::Stairs => Some("Steps"),
        Shape::Cylinder => Some("Sides"),
        Shape::Arch => Some("Voussoirs"),
        s if s.is_natural() => Some("Seed"),
        _ => None,
    }
}

fn typed(label: &str, field: PieceField, value: u32, help: &str) -> SheetRow {
    let value = value.to_string();
    SheetRow::new(
        label,
        Act::Type(TypeSpec {
            field: NumberField::Size(field),
            initial: value.clone(),
        }),
    )
    .value(&value)
    .describe(help)
}

/// The Size page: Width, Depth and Height in units, and Steps, Sides,
/// Voussoirs or the seed where the shape has one. A types a number; Left
/// and Right nudge by the block-out grid.
pub fn size_sheet(def: &PieceDef) -> Sheet {
    const HELP: &str = "Type a number of units, or nudge with Left and Right.";
    let mut rows: Vec<SheetRow> = (0..3)
        .map(|axis| {
            typed(
                piece::axis_name(axis),
                PieceField::Axis(axis),
                u32::from(def.size[axis]),
                HELP,
            )
        })
        .collect();
    if let Some(label) = param_label(def.shape) {
        rows.push(typed(label, PieceField::Param, u32::from(def.param), HELP));
    }
    Sheet {
        kind: Kind::Properties,
        title: "SIZE".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

fn material_name(m: &Material) -> String {
    match m {
        Material::Dev => "Dev".into(),
        Material::Nodraw => "Nodraw".into(),
        Material::Clip => "Clip".into(),
        Material::Map(key) => key.path.rsplit('\\').next().unwrap_or(&key.path).replace('_', " "),
    }
}

/// The Material page: **All faces** and each role, each opening a picker
/// on Dev, the map's surfaces and Nodraw.
pub fn material_sheet(f: &PieceFacts) -> Sheet {
    let mut options = vec![Material::Dev];
    options.extend(
        f.surfaces
            .iter()
            .map(|path| Material::Map(forge_shapes::TagKey::shader(path))),
    );
    options.push(Material::Nodraw);
    let roles = [
        ("All faces", None),
        ("Floor", Some(Role::Floor)),
        ("Wall", Some(Role::Wall)),
        ("Ceiling", Some(Role::Ceiling)),
        ("Trim", Some(Role::Trim)),
    ];
    let rows = roles
        .into_iter()
        .map(|(label, role)| {
            let now = match role {
                Some(role) => Some(f.def.materials.get(role).clone()),
                None => {
                    let first = f.def.materials.get(Role::Floor).clone();
                    Role::ALL
                        .iter()
                        .all(|r| *f.def.materials.get(*r) == first)
                        .then_some(first)
                }
            };
            let shown = now.as_ref().map_or("Mixed".to_string(), material_name);
            let pick = PickSpec {
                title: label.into(),
                values: options.iter().map(material_name).collect(),
                current: now
                    .and_then(|m| options.iter().position(|o| *o == m))
                    .unwrap_or(0),
                details: Vec::new(),
                description: "What the faces are drawn with.".into(),
                choices: options
                    .iter()
                    .map(|m| Choice::Material(f.id, role, m.clone()))
                    .collect(),
                closes: false,
            };
            SheetRow::new(label, Act::Pick(pick))
                .value(&shown)
                .describe("What the faces are drawn with.")
        })
        .collect();
    Sheet {
        kind: Kind::Properties,
        title: "MATERIAL".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// A piece's Properties rows after the doc's: Resize, Size, Material and,
/// for a rock, Reroll.
pub fn piece_rows(f: &PieceFacts) -> Vec<SheetRow> {
    let mut rows = vec![
        SheetRow::new("Resize", Act::Choose(Choice::Resize(f.id)))
            .describe("Aim at a face and push or pull it with the left stick. A writes, B cancels."),
        SheetRow::new("Size...", Act::Dialog(Box::new(size_sheet(&f.def))))
            .value(&size_line(&f.def))
            .describe("Type the size in units."),
        SheetRow::new("Material...", Act::Dialog(Box::new(material_sheet(f))))
            .describe("Dev, one of the map's surfaces or Nodraw, on every face or one role."),
    ];
    if f.def.shape.is_natural() {
        rows.push(
            SheetRow::new("Reroll", Act::Choose(Choice::Reroll(f.id)))
                .describe("Take the next seed: another rock of the same size."),
        );
    }
    rows
}

/// A sample piece for the preview pages and the tests.
pub fn sample() -> PieceFacts {
    PieceFacts {
        id: OpId(7),
        def: piece::start(Shape::Boulder, 2),
        surfaces: vec![r"levels\test\court\shaders\rock_wall".into()],
    }
}
