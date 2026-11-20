//! The converter's side of Halo 1's lens flares, debris and light
//! fixtures' lights: `lens` from the `lens` definition
//! ([`h1convert::lens`]), `garb` from the `garb` definition, and a light
//! fixture's attachments on the machine it becomes. A child of `convert`,
//! for the converter's caches. Evidence: docs/formats/tag-definitions.md,
//! "Lens flares, debris and light fixtures".

use h1map::Tag;
use h1_assets::models::Converted;
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use super::Converter;
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

/// Builds the item and object header from the source garbage tag.
fn garbage(
    obj: &h1::Object,
    source: &[u8],
    hlmt: Option<TagId>,
    predicted: &[crate::rebuild::Predicted],
    refs: &[(String, FourCC, TagId)],
) -> Result<Record> {
    let d = def(b"garb")?;
    let mut r = defs::default_record(d);
    crate::rebuild::object_tuning(&mut r, "item.obje", obj)?;
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
    crate::arsenal::item_words(&mut r, source)?;
    crate::arsenal::refs(&mut r, refs)?;
    Ok(r)
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
        let bitmap = self.source_bitmap(l.bitmap)?;
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

    /// A Halo 1 garbage object (debris an effect throws) from the `garb`
    /// definition, converted once: its model chain as any rebuilt
    /// object's, and its attachments.
    pub fn garbage(&mut self, tag: &Tag) -> Result<TagId> {
        h1::tag_bytes(self.h1, tag, b"garb", 0x308)?;
        self.object(tag)
    }

    pub(super) fn garbage_uncached(&mut self, tag: &Tag) -> Result<TagId> {
        let b = h1::tag_bytes(self.h1, tag, b"garb", 0x308)?;
        let mut object = h1::Object::read(self.h1, b)?;
        self.halo2_change_colours(&mut object);
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        rep.physics = Some("none (garbage has no physics model)".into());
        let hlmt = self.model(&mut rep, &object, &tag.name, None)?;
        let predicted = self.predicted(hlmt);
        let refs = self.item_effect_refs(b, &mut rep)?;
        let mut item = garbage(&object, b, hlmt, &predicted, &refs)?;
        let (attachments, creation, functions) = self.object_attachments(b, &object, &mut rep)?;
        crate::rebuild::object_effects(b"garb", &mut item, "item.obje", &attachments, creation)?;
        crate::rebuild::object_functions(b"garb", &mut item, "item.obje", &functions)?;
        let root = built(def(b"garb")?, &item)?;
        self.tree.reports.push(rep);
        let id = self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(*b"garb"),
                root,
                raw: Vec::new(),
            },
        );
        Ok(id)
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
            texture_override: 0,
            object_type: 4,
            acceleration_scale: 0.0,
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
            secondary_scale: String::new(),
            change_colour: -1,
        }];
        let d = def(b"garb").unwrap();
        let mut source = [0; 0x308];
        source[0x2e0..0x2e4].copy_from_slice(&0.75f32.to_le_bytes());
        let mut item = garbage(&object(), &source, Some(hlmt), &[], &[]).unwrap();
        crate::rebuild::object_effects(b"garb", &mut item, "item.obje", &a, None).unwrap();
        let r = defs::from_built(d, &built(d, &item).unwrap()).unwrap();
        assert_eq!(r.at("item.unknown_114"), Some(&Val::Bytes(0.75f32.to_le_bytes().to_vec())));
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
        let root = light_with_flare(&crate::rebuild::light(&l).unwrap(), lens).unwrap();
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
            secondary_scale: String::new(),
            change_colour: -1,
        }];
        let r = defs::from_built(
            def(b"mach").unwrap(),
            &super::super::attachments_conv::with_attachments(
                *b"mach",
                "devi.obje",
                mach,
                &a,
                None,
                &[],
                &[],
            )
            .unwrap(),
        )
        .unwrap();
        match r.at("devi.obje.attachments") {
            Some(Val::Block(e)) => assert!(names(e[0].at("type"), light)),
            x => panic!("{x:?}"),
        }
    }
}
