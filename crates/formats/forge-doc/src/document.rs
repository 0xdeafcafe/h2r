use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{BlockKind, Limits, MapId, Refusal, Result, Sha256, validate, wire};

/// The `.h2forge` wire version.
pub const FORMAT: u32 = content::forge::DOCUMENT_FORMAT;
/// The maximum wire size of a document in bytes.
pub const MAX_DOC_BYTES: usize = content::forge::MAX_DOCUMENT_BYTES;

pub(crate) const MAX_SOURCES: usize = 64;
pub(crate) const MAX_RECIPES: usize = 1024;
pub(crate) const MAX_IMPORTS: usize = 256;
pub(crate) const MAX_OPS: usize = 16_384;
pub(crate) const MAX_FIELDS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeDoc {
    pub format: u32,
    pub builder: BuilderVersions,
    pub base: MapRef,
    #[serde(deserialize_with = "wire::bounded::<_, _, MAX_SOURCES>")]
    pub sources: Vec<MapRef>,
    pub title: String,
    pub author: String,
    pub description: String,
    #[serde(deserialize_with = "wire::bounded::<_, _, MAX_RECIPES>")]
    pub recipes: Vec<Recipe>,
    #[serde(deserialize_with = "wire::bounded::<_, _, MAX_IMPORTS>")]
    pub imports: Vec<ImportSlot>,
    #[serde(deserialize_with = "wire::bounded::<_, _, MAX_OPS>")]
    pub ops: Vec<Op>,
    /// A draft can be saved before it has a materialised result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expected>,
}

impl ForgeDoc {
    /// Read an untrusted file. Input size is checked before deserialising,
    /// and collection limits are enforced as the arrays are read.
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self> {
        wire::check_size(bytes.len(), "document bytes", MAX_DOC_BYTES)?;
        let doc = serde_json::from_slice(bytes).map_err(wire::json_error)?;
        validate(&doc, limits)?;
        Ok(doc)
    }

    /// Write the canonical file form using the format's default limits.
    pub fn canonical_json(&self) -> Result<String> {
        crate::validate::document(self, &Limits::default())?;
        wire::canonical(self, "document bytes", MAX_DOC_BYTES)
    }

    /// SHA-256 of the canonical document content, omitting `expect`.
    pub fn id(&self) -> Result<Sha256> {
        crate::validate::document(self, &Limits::default())?;
        self.id_validated()
    }

    pub(crate) fn id_validated(&self) -> Result<Sha256> {
        wire::measure(self, "document bytes", MAX_DOC_BYTES)?;
        let value = serde_json::to_value(self).map_err(wire::json_error)?;
        content::forge::document_id(value)
            .map_err(|error| Refusal::invalid("document identity", error))
    }
}

/// Versions that must match the materialiser's generators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderVersions {
    pub forge_build: u32,
    pub pathfind: u32,
    pub bots: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapRef {
    pub map: String,
    pub sha256: Sha256,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub map_id: MapId,
    pub geometry: Sha256,
}

/// Portable tag key in the same form as `tags::repo::TagKey`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagKey {
    pub group: String,
    /// Full tag path with backslashes, without a group extension.
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Recipe {
    Transplant {
        tag: TagKey,
        /// The digest of a map in the document's base or sources.
        source: Sha256,
    },
    Variant {
        tag: TagKey,
        #[serde(deserialize_with = "wire::bounded::<_, _, MAX_FIELDS>")]
        writes: Vec<TagWrite>,
    },
    Generate {
        generator: String,
        #[serde(deserialize_with = "wire::bounded::<_, _, MAX_FIELDS>")]
        parameters: Vec<FieldWrite>,
    },
    Import {
        slot: ImportId,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImportId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSlot {
    pub id: ImportId,
    pub name: String,
    pub sha256: Sha256,
    /// The retail biped for a skin. A prop has no chassis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chassis: Option<TagKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpId(pub u32);

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Op {
    /// A nonzero stable ID. An undo names the ID of the edit it retracts.
    pub id: OpId,
    pub edit: Edit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    Placement(Placement),
    TagField(TagWrite),
    /// A log-only inverse. The folded document has only active edits.
    Undo,
    Mesh,
    Bitmap,
    Forge,
}

impl Edit {
    pub(crate) fn unsupported(&self) -> Refusal {
        let name = match self {
            Self::Mesh => "mesh",
            Self::Bitmap => "bitmap",
            Self::Forge => "Forge",
            _ => return Refusal::invalid("document op", "undo belongs in the edit log"),
        };
        Refusal::Unsupported(name.into())
    }
}

/// The set, append and swap-remove vocabulary of `scenario_edit::Command`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Placement {
    Set {
        #[serde(with = "wire::block")]
        block: BlockKind,
        index: u32,
        write: FieldWrite,
    },
    Add {
        #[serde(with = "wire::block")]
        block: BlockKind,
        /// Copy from this placement in the current block. `None` starts
        /// with the editor's defaults.
        #[serde(skip_serializing_if = "Option::is_none")]
        like: Option<u32>,
        /// Changes to the new entry, as typed fields. The adapter builds
        /// the entry locally and commits the append and its writes together.
        #[serde(deserialize_with = "wire::bounded::<_, _, MAX_FIELDS>")]
        fields: Vec<FieldWrite>,
    },
    Remove {
        #[serde(with = "wire::block")]
        block: BlockKind,
        index: u32,
    },
}

impl Placement {
    pub fn block(&self) -> BlockKind {
        match self {
            Self::Set { block, .. } | Self::Add { block, .. } | Self::Remove { block, .. } => {
                *block
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldWrite {
    pub field: String,
    pub value: Value,
}

/// One `edit.batch` write, addressed by portable tag key and field path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagWrite {
    pub tag: TagKey,
    pub path: String,
    pub value: Value,
    /// The digest of the canonical typed value the author saw. This
    /// expresses a precondition without copying the old value into a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<Sha256>,
}

/// Values an author can set. Tag references are resolved locally. Typed
/// values have no representation for raw spans, resources or address writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(F32),
    Vec2([F32; 2]),
    Vec3([F32; 3]),
    Vec4([F32; 4]),
    Text(String),
    Tag(Option<TagKey>),
}

impl Value {
    /// The digest used for a tag write's `was`.
    pub fn digest(&self) -> Result<Sha256> {
        crate::validate::value(self)?;
        Ok(Sha256::of(
            wire::canonical(self, "value bytes", MAX_DOC_BYTES)?.as_bytes(),
        ))
    }
}

/// A float encoded as exactly eight lowercase hexadecimal digits of its
/// IEEE 754 bit pattern. Equality compares bits, including the sign of zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct F32(pub(crate) u32);

impl F32 {
    pub fn new(value: f32) -> Result<Self> {
        if value.is_finite() {
            Ok(Self(value.to_bits()))
        } else {
            Err(Refusal::invalid("value", "floats must be finite"))
        }
    }

    pub const fn get(self) -> f32 {
        f32::from_bits(self.0)
    }
}
