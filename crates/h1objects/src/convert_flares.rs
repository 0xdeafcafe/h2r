//! The converter's side of Halo 1's lens flares, debris and light
//! fixtures' lights: `lens` from the `lens` definition
//! ([`h1convert::lens`]), `garb` from the `garb` definition, and a light
//! fixture's attachments on the machine it becomes. A child of `convert`,
//! for the converter's caches. Evidence: docs/formats/tag-definitions.md,
//! "Lens flares, debris and light fixtures".

use h1map::Tag;
use h1models::Converted;
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use super::{Converter, ObjectReport};
use crate::{Error, Result, h1};

/// `obje +0` for garbage: its index in the engine's object type table
/// (E `0x00468630`, docs/formats/objects.md).
pub const GARBAGE_TYPE: u32 = 4;

fn def(group: &[u8; 4]) -> Result<&'static StructDef> {
    Defs::builtin()
        .group(FourCC(*group))
        .ok_or_else(|| Error::Invalid(format!("no definition for {}", FourCC(*group))))
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v)
        .map_err(|e| Error::Invalid(format!("{path}: {e}")))
}

fn built(d: &StructDef, r: &Record) -> Result<Block> {
    defs::to_built(d, r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

/// The `garb` root for a Halo 1 garbage object: the role's words (the
/// base's garbage, majority per unknown word; none on most bases), the
/// object header (type, bounds, model, predicted resources) and the
/// attachments; everything else the definition's default. No retail
/// template is read.
pub fn garbage(
    obj: &h1::Object,
    hlmt: Option<TagId>,
    predicted: &[crate::rebuild::Predicted],
    role: &[(String, u32)],
    attachments: &[crate::rebuild::AttachmentSpec],
) -> Result<Block> {
    let d = def(b"garb")?;
    let mut r = defs::default_record(d);
    crate::provenance::note_words("", role);
    for (p, w) in role {
        set(&mut r, p, Val::Bytes(w.to_le_bytes().to_vec()))?;
    }
    set(&mut r, "item.obje.object_type", Val::Enum(GARBAGE_TYPE))?;
    set(
        &mut r,
        "item.obje.bounding_radius",
        Val::Float(obj.bounding_radius),
    )?;
    set(
        &mut r,
        "item.obje.bounding_offset",
        Val::floats(&obj.bounding_offset),
    )?;
    set(
        &mut r,
        "item.obje.model",
        Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
    )?;
    crate::rebuild::predicted_resources(b"garb", &mut r, "item.obje", predicted)?;
    crate::rebuild::object_effects(b"garb", &mut r, "item.obje", attachments, None)?;
    built(d, &r)
}

/// `root` (a `group` built by [`crate::rebuild::device`]) with
/// `attachments` in its object header.
pub fn with_attachments(
    group: &[u8; 4],
    root: &Block,
    attachments: &[crate::rebuild::AttachmentSpec],
) -> Result<Block> {
    let d = def(group)?;
    let mut r = defs::from_built(d, root).map_err(|e| Error::Invalid(e.to_string()))?;
    let creation = match r.at("devi.obje.creation_effect") {
        Some(Val::Ref {
            target: defs::Target::Id(id),
            ..
        }) => Some(*id),
        _ => None,
    };
    crate::rebuild::object_effects(group, &mut r, "devi.obje", attachments, creation)?;
    built(d, &r)
}

/// `root` (a `ligh` built by [`crate::rebuild::light`]) naming `lens` as
/// its lens flare (`ligh +0x90`; E `0x000c34a6` reads the datum and
/// submits the flare with the light).
pub fn light_with_flare(root: &Block, lens: TagId) -> Result<Block> {
    let d = def(b"ligh")?;
    let mut r = defs::from_built(d, root).map_err(|e| Error::Invalid(e.to_string()))?;
    set(&mut r, "lens_flare", Val::tag(FourCC(*b"lens"), lens))?;
    built(d, &r)
}

impl Converter<'_> {
    /// A Halo 1 lens flare from the `lens` definition
    /// ([`h1convert::lens::rebuild`]) with its bitmap, converted once.
    pub fn lens_flare(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let l = h1convert::lens::H1Lens::read(self.h1, tag)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let bitmap = l.bitmap.and_then(|b| self.bitmap(b));
        if l.bitmap.is_some() && bitmap.is_none() {
            rep.no_home.push("bitmap: did not convert".into());
        }
        let root = h1convert::lens::rebuild(&l, bitmap)?;
        for n in h1convert::lens::left_out(&l) {
            rep.no_home.push(n);
        }
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"lens"),
                root,
                raw: Vec::new(),
            },
        );
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// The lens flare of a converted light: Halo 1's, converted, or the
    /// reason in `rep`.
    pub(super) fn flare_for_light(&mut self, lens: Option<h1map::TagId>, rep: &mut ObjectReport) -> Option<TagId> {
        let id = lens?;
        let t = self.h1.tag(id).ok()?;
        match self.lens_flare(t) {
            Ok(x) => Some(x),
            Err(e) => {
                rep.no_home.push(format!("lens flare {}: not converted ({e})", t.name));
                None
            }
        }
    }

    /// A Halo 1 garbage object (debris an effect throws) from the `garb`
    /// definition, converted once: its model chain as any rebuilt
    /// object's, and its attachments.
    pub fn garbage(&mut self, tag: &Tag) -> Result<TagId> {
        if let Some(id) = self.done.get(&tag.id.0) {
            return Ok(*id);
        }
        let b = h1::tag_bytes(self.h1, tag, b"garb", h1::OBJE_SIZE)?;
        let object = h1::Object::read(self.h1, b)?;
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.physics = Some("none (garbage has no physics model)".into());
        let hlmt = self.model(&mut rep, &object, None, &tag.name, true)?;
        let predicted = self.predicted(&object)?;
        let role = crate::arsenal::role(self.base, b"garb", &[])?
            .map(|r| r.root)
            .unwrap_or_default();
        let (attachments, _) = self.object_effects(b, &object, &mut rep)?;
        let (root, roles) =
            crate::provenance::capture(|| garbage(&object, hlmt, &predicted, &role, &attachments));
        let root = root?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"garb"),
                root,
                raw: Vec::new(),
            },
        );
        self.tree.note_roles(id, roles);
        self.done.insert(tag.id.0, id);
        Ok(id)
    }

    /// An effect part's debris: Halo 1's `garb`, converted, or the reason
    /// in `rep`.
    pub(super) fn effect_garbage(&mut self, id: h1map::TagId, rep: &mut ObjectReport) -> Option<TagId> {
        let t = self.h1.tag(id).ok()?;
        match self.garbage(t) {
            Ok(x) => Some(x),
            Err(e) => {
                rep.no_home.push(format!("garb {}: not converted ({e})", t.name));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rebuild::AttachmentSpec;

    fn names(v: Option<&Val>, id: TagId) -> bool {
        matches!(v, Some(Val::Ref { target: defs::Target::Id(x), .. }) if *x == id)
    }

    fn object() -> h1::Object {
        h1::Object {
            object_type: 4,
            flags: 0,
            bounding_radius: 0.3,
            bounding_offset: [0.0, 0.0, 0.1],
            model: None,
            animation: None,
            collision: None,
            physics: None,
            modifier_shader: None,
            creation_effect: None,
            attachments: Vec::new(),
            widgets: Vec::new(),
            change_colours: Vec::new(),
        }
    }

    #[test]
    fn rebuilt_garbage_is_a_garb_object_with_its_attachments() {
        let hlmt = TagId(0xe100_0003);
        let light = TagId(0xe100_0004);
        let a = [AttachmentSpec {
            group: *b"ligh",
            tag: light,
            marker: "glow".into(),
            primary_scale: String::new(),
            h1_primary: 0,
        }];
        let d = def(b"garb").unwrap();
        let r = defs::from_built(d, &garbage(&object(), Some(hlmt), &[], &[], &a).unwrap()).unwrap();
        assert_eq!(r.at("item.obje.object_type"), Some(&Val::Enum(GARBAGE_TYPE)));
        assert_eq!(r.at("item.obje.bounding_radius"), Some(&Val::Float(0.3)));
        assert!(names(r.at("item.obje.model"), hlmt));
        match r.at("item.obje.attachments") {
            Some(Val::Block(e)) => {
                assert_eq!(e.len(), 1);
                assert!(names(e[0].at("type"), light));
            }
            x => panic!("{x:?}"),
        }
    }

    #[test]
    fn a_light_names_its_flare_and_a_fixture_its_lights() {
        let l = h1::Light {
            flags: 0,
            radius: 2.0,
            falloff_angle: std::f32::consts::PI,
            cutoff_angle: std::f32::consts::PI,
            colour_lower: [1.0; 4],
            colour_upper: [1.0; 4],
            duration_ticks: 0.0,
            lens_flare: None,
        };
        let lens = TagId(0xe100_0009);
        let root = light_with_flare(&crate::rebuild::light(&l, None).unwrap(), lens).unwrap();
        let r = defs::from_built(def(b"ligh").unwrap(), &root).unwrap();
        assert!(names(r.at("lens_flare"), lens));
        assert_eq!(r.at("radius"), Some(&Val::Float(2.0)));

        let light = TagId(0xe100_000a);
        let mach = defs::to_built(def(b"mach").unwrap(), &defs::default_record(def(b"mach").unwrap()), &mut |_| None).unwrap();
        let a = [AttachmentSpec {
            group: *b"ligh",
            tag: light,
            marker: "light".into(),
            primary_scale: String::new(),
            h1_primary: 0,
        }];
        let r = defs::from_built(def(b"mach").unwrap(), &with_attachments(b"mach", &mach, &a).unwrap()).unwrap();
        match r.at("devi.obje.attachments") {
            Some(Val::Block(e)) => assert!(names(e[0].at("type"), light)),
            x => panic!("{x:?}"),
        }
    }
}
