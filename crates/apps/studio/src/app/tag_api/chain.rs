//! The inspector's tag tree, walked over the current TagDoc (including edits).
use map::{FourCC, TagId};
use scenario_edit::{
    BlockKind, Scenario,
    layout::{FieldType, Role},
};
use serde_json::{Value, json};
use tags::registry::{Kind, Layout};

use super::{TagDoc, le};
use crate::app::tag_doc::Access;

pub const DEFAULT_DEPTH: usize = 3;
pub const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 1024;
const MAX_FIELDS: usize = 100_000;
const MAX_BLOCK_DEPTH: usize = 16;

/// A reference from a placement or tag. The path on a tag uses the same
/// spelling as tag.set and tag.read.
struct Link {
    tag: TagId,
    via: String,
    shortcut: Option<&'static str>,
    variant: Option<u32>,
}

pub struct Chain {
    pub root: Option<Node>,
    pub truncated: bool,
}

pub struct Node {
    pub tag: TagId,
    pub via: String,
    pub shortcut: Option<&'static str>,
    pub variant: Option<Value>,
    pub cycle: bool,
    pub children: Vec<Node>,
}

impl Chain {
    pub fn json(&self, doc: &TagDoc) -> Value {
        json!({
            "root": self.root.as_ref().map(|n| n.json(doc)),
            "truncated": self.truncated,
        })
    }
}

impl Node {
    fn json(&self, doc: &TagDoc) -> Value {
        let entry = doc.tag(self.tag);
        json!({
            "tag": format!("{:#010x}", self.tag.0),
            "group": entry.map(|t| tags::defs::fourcc_text(t.group)),
            "name": entry.map(|t| &t.name),
            "via": (!self.via.is_empty()).then_some(&self.via),
            "label": self.shortcut,
            "where": entry.map(|t| if t.elsewhere.is_some() { "shared" } else { "here" }),
            "cache": entry.and_then(|t| t.elsewhere.as_deref()),
            "variant": self.variant,
            "cycle": self.cycle,
            "children": self.children.iter().map(|child| child.json(doc)).collect::<Vec<_>>(),
        })
    }
}

/// Resolve the placement's current palette entry or item reference.
pub fn placement_root(
    scenario: &Scenario,
    kind: BlockKind,
    index: usize,
) -> Option<(TagId, &'static str)> {
    let tag = scenario.object_tag(kind, index)?.id;
    if tag == TagId::NULL {
        return None;
    }
    let def = scenario.block(kind).def;
    let field = def
        .role(Role::PaletteIndex)
        .or_else(|| def.fields.iter().find(|f| f.ty == FieldType::TagRef))?;
    Some((tag, field.name))
}

/// A placement without an object or item collection has no root. For a
/// tag root, `via` is empty. Depth counts references below the root.
pub fn chain(doc: &TagDoc, root: Option<(TagId, &str)>, depth: usize) -> Chain {
    let mut walk = Walk {
        doc,
        nodes_left: MAX_NODES,
        fields_left: MAX_FIELDS,
        truncated: false,
    };
    let root = root.map(|(tag, via)| {
        walk.node(
            Link {
                tag,
                via: via.into(),
                shortcut: None,
                variant: None,
            },
            depth.min(MAX_DEPTH),
            &mut Vec::new(),
        )
    });
    Chain {
        root,
        truncated: walk.truncated,
    }
}

struct Walk<'a> {
    doc: &'a TagDoc,
    nodes_left: usize,
    fields_left: usize,
    truncated: bool,
}

impl Walk<'_> {
    fn node(&mut self, link: Link, depth: usize, ancestors: &mut Vec<TagId>) -> Node {
        self.nodes_left -= 1;
        let cycle = ancestors.contains(&link.tag);
        let variant = link.variant.and_then(|v| self.model_variant(link.tag, v));
        let mut children = Vec::new();
        if !cycle && depth > 0 {
            let refs = self.links(link.tag, link.variant);
            ancestors.push(link.tag);
            for next in refs {
                if self.nodes_left == 0 {
                    self.truncated = true;
                    break;
                }
                children.push(self.node(next, depth - 1, ancestors));
            }
            ancestors.pop();
        }
        Node {
            tag: link.tag,
            via: link.via,
            shortcut: link.shortcut,
            variant,
            cycle,
            children,
        }
    }

    fn links(&mut self, tag: TagId, variant: Option<u32>) -> Vec<Link> {
        let mut refs = Vec::new();
        let doc = self.doc;
        if let Some(t) = doc.tag(tag)
            && !matches!(t.access, Access::None(_))
            && let Some(layout) = doc.layout(tag)
            && layout.size <= t.size
        {
            let default = string_id(self.doc, t.address, layout, "default_variant");
            let variant = variant.filter(|&v| v != 0 && v != u32::MAX).or(default);
            self.element(t.group, t.address, layout, "", variant, &mut refs, 0);
        }
        // Keep the model chain ahead of wide branches like shaders.
        refs.sort_by_key(|r| match r.shortcut {
            Some("Model") => 0,
            Some("Render model") => 1,
            Some(_) => 2,
            None => 3,
        });
        refs
    }

    #[allow(clippy::too_many_arguments)]
    fn element(
        &mut self,
        group: FourCC,
        at: u32,
        layout: &Layout,
        prefix: &str,
        variant: Option<u32>,
        refs: &mut Vec<Link>,
        depth: usize,
    ) {
        if depth > MAX_BLOCK_DEPTH {
            self.truncated = true;
            return;
        }
        for f in &layout.fields {
            if self.fields_left == 0 {
                self.truncated = true;
                return;
            }
            self.fields_left -= 1;
            let Some(address) = at.checked_add(f.offset) else {
                continue;
            };
            match &f.kind {
                Kind::TagRef { .. } | Kind::Datum { .. } => {
                    let Some(bytes) = self.doc.read(address, f.kind.width()) else {
                        continue;
                    };
                    let id = TagId(le(&bytes[bytes.len() - 4..]) as u32);
                    if id == TagId::NULL || id.0 == 0 {
                        continue;
                    }
                    let to = self.doc.tag(id).map(|t| t.group).or_else(|| {
                        (bytes.len() == 8).then(|| FourCC::from_u32(le(&bytes[..4]) as u32))
                    });
                    let shortcut = to.and_then(|to| shortcut(group, to, prefix, &f.name));
                    let variant = match shortcut {
                        Some("Model") => variant,
                        Some("Collection entry") => string_id(self.doc, at, layout, "variant"),
                        _ => None,
                    };
                    refs.push(Link {
                        tag: id,
                        via: format!("{prefix}{}", f.name),
                        shortcut,
                        variant,
                    });
                }
                Kind::Block(child) if child.size > 0 => {
                    let Some((n, start)) = self.doc.block(address, child) else {
                        continue;
                    };
                    for i in 0..n {
                        let Some(at) = start.checked_add(i * child.size) else {
                            break;
                        };
                        let path = format!("{prefix}{}[{i}].", f.name);
                        self.element(group, at, child, &path, variant, refs, depth + 1);
                        if self.fields_left == 0 {
                            return;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn model_variant(&self, tag: TagId, variant: u32) -> Option<Value> {
        let t = self.doc.tag(tag)?;
        if t.group != FourCC(*b"hlmt") {
            return None;
        }
        let variant = if variant == 0 {
            let (at, _, f) = super::resolve(self.doc, tag, "variants[0].name").ok()?;
            le(&self.doc.read(at.checked_add(f.offset)?, 4)?) as u32
        } else {
            variant
        };
        (variant != u32::MAX).then(|| {
            self.doc
                .string(variant)
                .map_or_else(|| json!(format!("{variant:#010x}")), |s| json!(s))
        })
    }
}

fn string_id(doc: &TagDoc, at: u32, layout: &Layout, name: &str) -> Option<u32> {
    let f = layout
        .fields
        .iter()
        .find(|f| f.name == name && matches!(f.kind, Kind::StringId))?;
    Some(le(&doc.read(at.checked_add(f.offset)?, 4)?) as u32)
}

fn shortcut(from: FourCC, to: FourCC, prefix: &str, field: &str) -> Option<&'static str> {
    Some(match (&from.0, &to.0) {
        (b"itmc" | b"vehc", _) if prefix.starts_with("entries[") && field == "object" => {
            "Collection entry"
        }
        (b"hlmt", b"mode") if prefix.is_empty() => "Render model",
        (b"hlmt", b"coll") if prefix.is_empty() => "Collision model",
        (b"hlmt", b"phmo") if prefix.is_empty() => "Physics model",
        (b"hlmt", b"jmad") if prefix.is_empty() => "Animation",
        (b"mode", b"shad") if prefix.starts_with("materials[") => "Material shader",
        (b"shad", b"stem") => "Shader template",
        (b"shad", b"bitm") => "Bitmap",
        (b"effe", _) if prefix.contains(".parts[") => "Effect part",
        (_, b"hlmt") if prefix.is_empty() && field == "model" => "Model",
        (_, _) if prefix.starts_with("attachments[") => "Attachment",
        (_, _) if prefix.starts_with("widgets[") => "Widget",
        (b"weap", b"proj") if prefix.starts_with("barrels[") => "Projectile",
        (b"weap" | b"proj", b"effe") => "Effect",
        (b"weap" | b"proj", b"jpt!") => "Damage effect",
        (b"vehi" | b"bipd", b"weap") if prefix.starts_with("weapons[") => "Weapon",
        (b"vehi", _) if prefix.starts_with("block_1c8[") => "Seat reference",
        _ => return None,
    })
}
