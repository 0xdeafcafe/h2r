use h1map::Tag;
use h1models::Converted;
use map::FourCC;
use tags::defs::{self, Defs, Val};

use super::{Converter, PhysicsKind};
use crate::{Error, Result, h1, rebuild, scripted_props};

impl Converter<'_> {
    pub(super) fn scripted_prop(&mut self, tag: &Tag) -> Result<map::TagId> {
        let group = scripted_props::replacement_group(&tag.name).expect("scripted prop");
        let object_type = if group == *b"bloc" {
            11
        } else {
            rebuild::SCENERY_TYPE
        };
        let b = self.h1.tag_data(tag)?;
        let mut obj = h1::Object::read(self.h1, b)?;
        obj.object_type = object_type as u16;
        self.halo2_change_colours(&mut obj);
        if obj.bounding_radius == 0.0 {
            let source = obj
                .model
                .ok_or_else(|| Error::Invalid(format!("{}: no model", tag.name)))?;
            let (model, _) =
                h1models::model_from_map(self.h1, self.h1_tag(source)?, &Default::default())?;
            let (radius, centre) = rebuild::model_sphere(&model)
                .ok_or_else(|| Error::Invalid(format!("{}: no model bounds", tag.name)))?;
            obj.bounding_radius = radius;
            obj.bounding_offset = centre;
        }

        let mut rep = Self::report(tag, &format!("rebuilt as {group}", group = FourCC(group)));
        let hlmt = self.model(&mut rep, &obj, None, &tag.name, true, None)?;
        if group == *b"bloc"
            && !self.object_physics(&mut rep, hlmt, &obj, &tag.name, PhysicsKind::Crate)?
        {
            return Err(Error::Invalid(format!(
                "{}: no solid crate collision",
                tag.name
            )));
        }
        let d = Defs::builtin()
            .group(FourCC(group))
            .expect("scripted prop definition");
        let mut r = defs::default_record(d);
        for (field, value) in [
            ("object.object_type", Val::Enum(object_type)),
            ("object.flags", Val::Flags(obj.flags.into())),
            ("object.bounding_radius", Val::Float(obj.bounding_radius)),
            ("object.bounding_offset", Val::floats(&obj.bounding_offset)),
            (
                "object.model",
                Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(map::TagId::NULL)),
            ),
        ] {
            r.set(field, value)
                .map_err(|e| Error::Invalid(e.to_string()))?;
        }
        let predicted = self.predicted(hlmt);
        rebuild::predicted_resources(&group, &mut r, "object", &predicted)?;
        rebuild::object_change_colors(&group, &mut r, "object", &obj.change_colours)?;
        let (attachments, creation) = self.object_effects(b, &obj, &mut rep)?;
        rebuild::object_effects(&group, &mut r, "object", &attachments, creation)?;
        let root =
            defs::to_built(d, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))?;
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(group),
                root,
                raw: Vec::new(),
            },
        ))
    }
}
