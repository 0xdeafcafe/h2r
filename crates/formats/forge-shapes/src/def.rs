//! A piece's definition, its checks and its canonical name
//! (docs/game/forge-blockout.md, "A piece").

use crate::error::{Error, Result};
use crate::units::{MAX, MIN};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// The shapes of the block-out set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Block,
    Wall,
    FloorPlate,
    Ramp,
    Wedge,
    Stairs,
    Cylinder,
    Arch,
    Boulder,
    Slab,
    Cliff,
    TerrainWedge,
    Spire,
}

impl Shape {
    pub const ALL: [Shape; 13] = [
        Shape::Block,
        Shape::Wall,
        Shape::FloorPlate,
        Shape::Ramp,
        Shape::Wedge,
        Shape::Stairs,
        Shape::Cylinder,
        Shape::Arch,
        Shape::Boulder,
        Shape::Slab,
        Shape::Cliff,
        Shape::TerrainWedge,
        Shape::Spire,
    ];

    /// The spelling used in a name and in the document.
    pub fn name(self) -> &'static str {
        match self {
            Shape::Block => "block",
            Shape::Wall => "wall",
            Shape::FloorPlate => "floor_plate",
            Shape::Ramp => "ramp",
            Shape::Wedge => "wedge",
            Shape::Stairs => "stairs",
            Shape::Cylinder => "cylinder",
            Shape::Arch => "arch",
            Shape::Boulder => "boulder",
            Shape::Slab => "slab",
            Shape::Cliff => "cliff",
            Shape::TerrainWedge => "terrain_wedge",
            Shape::Spire => "spire",
        }
    }

    pub fn from_name(name: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Whether the shape is one of the natural set, whose param is a seed.
    pub fn is_natural(self) -> bool {
        matches!(
            self,
            Shape::Boulder | Shape::Slab | Shape::Cliff | Shape::TerrainWedge | Shape::Spire
        )
    }

    /// The words the refusal of a param uses.
    pub fn param_words(self) -> &'static str {
        match self {
            Shape::Stairs => "2 to 32 steps",
            Shape::Cylinder => "8, 12, 16 or 24 sides",
            Shape::Arch => "4 to 12 voussoirs",
            s if s.is_natural() => "any seed",
            _ => "0",
        }
    }

    /// Whether `param` is one the shape takes.
    pub fn param_ok(self, param: u16) -> bool {
        match self {
            Shape::Stairs => (2..=32).contains(&param),
            Shape::Cylinder => matches!(param, 8 | 12 | 16 | 24),
            Shape::Arch => (4..=12).contains(&param),
            s if s.is_natural() => true,
            _ => param == 0,
        }
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A tag, as the document names one: its group and its path with
/// backslashes, without the group's extension.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagKey {
    pub group: String,
    pub path: String,
}

impl TagKey {
    pub fn shader(path: &str) -> TagKey {
        TagKey {
            group: "shad".into(),
            path: path.into(),
        }
    }
}

/// What a face is drawn with.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    /// The dev texture of the role's family.
    Dev,
    /// A shader the base map holds.
    Map(TagKey),
    /// Left out of the render model; still collides.
    Nodraw,
    /// A barrier face: collides, never drawn.
    Clip,
}

/// The materials of a piece: one for every role, or one a role in the order
/// of [`crate::Role::ALL`] (floor, wall, ceiling, trim).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Materials {
    All(Material),
    Each([Material; 4]),
}

impl Materials {
    pub fn get(&self, role: crate::Role) -> &Material {
        match self {
            Materials::All(m) => m,
            Materials::Each(each) => &each[role.index()],
        }
    }

    fn list(&self) -> &[Material] {
        match self {
            Materials::All(m) => std::slice::from_ref(m),
            Materials::Each(each) => each,
        }
    }
}

/// A piece: integers and tag keys, nothing taken from a disc.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PieceDef {
    pub shape: Shape,
    /// x, y, z in units.
    pub size: [u16; 3],
    /// Steps, sides, voussoirs or seed by shape; else 0.
    pub param: u16,
    pub materials: Materials,
}

impl PieceDef {
    /// A definition drawn with the dev textures on every role.
    pub fn dev(shape: Shape, size: [u16; 3], param: u16) -> PieceDef {
        PieceDef {
            shape,
            size,
            param,
            materials: Materials::All(Material::Dev),
        }
    }

    /// Checks everything a definition can be wrong about without
    /// generating it.
    pub fn validate(&self) -> Result<()> {
        for (axis, value) in ['x', 'y', 'z'].into_iter().zip(self.size) {
            if !(MIN..=MAX).contains(&value) {
                return Err(Error::SizeRange { axis, value });
            }
        }
        if !self.shape.param_ok(self.param) {
            return Err(Error::ParamRange {
                shape: self.shape,
                param: self.param,
                allowed: self.shape.param_words(),
            });
        }
        for material in self.materials.list() {
            if let Material::Map(key) = material {
                if key.group != "shad" {
                    return Err(Error::MapNotShader {
                        group: key.group.clone(),
                    });
                }
                if key.path.is_empty() {
                    return Err(Error::EmptyPath);
                }
            }
        }
        Ok(())
    }

    /// The canonical JSON: sorted keys, no spaces, integers only.
    pub fn canonical_json(&self) -> String {
        let materials = match &self.materials {
            Materials::All(m) => format!("{{\"all\":{}}}", material_json(m)),
            Materials::Each(each) => {
                let list: Vec<String> = each.iter().map(material_json).collect();
                format!("{{\"each\":[{}]}}", list.join(","))
            }
        };
        format!(
            "{{\"materials\":{materials},\"param\":{},\"shape\":\"{}\",\"size\":[{},{},{}]}}",
            self.param,
            self.shape.name(),
            self.size[0],
            self.size[1],
            self.size[2]
        )
    }

    /// The first 12 hex digits of the SHA-256 of the canonical JSON.
    pub fn digest12(&self) -> String {
        let sum = Sha256::digest(self.canonical_json().as_bytes());
        sum.iter().take(6).map(|b| format!("{b:02x}")).collect()
    }

    /// `<shape>_<x>x<y>x<z>_<param>_<digest12>`.
    pub fn name(&self) -> String {
        format!(
            "{}_{}x{}x{}_{}_{}",
            self.shape.name(),
            self.size[0],
            self.size[1],
            self.size[2],
            self.param,
            self.digest12()
        )
    }

    /// The tag path of the definition's tags.
    pub fn tag_path(&self) -> String {
        format!("forge\\blockout\\{}", self.name())
    }

    /// Whether `name` is this definition's name.
    pub fn is_named(&self, name: &str) -> bool {
        ParsedName::parse(name).is_ok_and(|p| p.matches(self))
    }
}

fn material_json(m: &Material) -> String {
    match m {
        Material::Dev => "\"dev\"".into(),
        Material::Nodraw => "\"nodraw\"".into(),
        Material::Clip => "\"clip\"".into(),
        Material::Map(key) => format!(
            "{{\"map\":{{\"group\":{},\"path\":{}}}}}",
            json_string(&key.group),
            json_string(&key.path)
        ),
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What a name says; the materials are only in its digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedName {
    pub shape: Shape,
    pub size: [u16; 3],
    pub param: u16,
    pub digest: String,
}

impl ParsedName {
    pub fn parse(name: &str) -> Result<ParsedName> {
        let bad = |why: &'static str| Error::BadName {
            name: name.to_string(),
            why,
        };
        let mut parts = name.rsplitn(4, '_');
        let digest = parts.next().ok_or_else(|| bad("empty"))?;
        let param = parts.next().ok_or_else(|| bad("no param"))?;
        let size = parts.next().ok_or_else(|| bad("no size"))?;
        let shape = parts.next().ok_or_else(|| bad("no shape"))?;
        if digest.len() != 12
            || !digest
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(bad("the digest is not 12 lowercase hex digits"));
        }
        let shape = Shape::from_name(shape).ok_or_else(|| bad("unknown shape"))?;
        let param = param
            .parse::<u16>()
            .map_err(|_| bad("param is not a number"))?;
        let axes: Vec<&str> = size.split('x').collect();
        let [x, y, z] = axes[..] else {
            return Err(bad("size is not <x>x<y>x<z>"));
        };
        let num = |s: &str| s.parse::<u16>().map_err(|_| bad("size is not numbers"));
        Ok(ParsedName {
            shape,
            size: [num(x)?, num(y)?, num(z)?],
            param,
            digest: digest.to_string(),
        })
    }

    pub fn matches(&self, def: &PieceDef) -> bool {
        self.shape == def.shape
            && self.size == def.size
            && self.param == def.param
            && self.digest == def.digest12()
    }
}
