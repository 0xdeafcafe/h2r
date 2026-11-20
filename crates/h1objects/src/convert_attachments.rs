//! Convert the marker attachments shared by Halo 1's object groups.

use h1map::Tag;
use h1models::Converted;
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Val};

use super::{Converter, ObjectReport};
use crate::functions::{self, FunctionSpec};
use crate::rebuild::{self, AttachmentSpec};
use crate::{Error, Result, h1};

type ObjectAttachments = (Vec<AttachmentSpec>, Option<TagId>, Vec<FunctionSpec>);

impl Converter<'_> {
    pub(super) fn object_attachments(
        &mut self,
        bytes: &[u8],
        obj: &h1::Object,
        rep: &mut ObjectReport,
    ) -> Result<ObjectAttachments> {
        let inputs = h1::FunctionInputs::read(bytes, obj.object_type);
        let mut notes = Vec::new();
        let functions = functions::convert(&h1::functions(self.h1, bytes)?, &inputs, &mut notes);
        let mut out = Vec::new();
        for a in h1::attachments(self.h1, bytes)? {
            let Some(id) = a.tag else { continue };
            let t = self.h1_tag(id)?;
            let mut group = a.group.0.to_be_bytes();
            let result = match &group {
                b"lsnd" => self.looping_sound(t).map(|id| vec![id]),
                b"ligh" => self.light(t).map(|tag| vec![tag]),
                b"effe" => self.effect(t).map(|tag| vec![tag]),
                b"cont" => self.contrail(t).map(|tag| vec![tag]),
                b"pctl" => {
                    group = *b"effe";
                    self.attached_particle_system(t, &a.marker)
                }
                _ => {
                    return Err(h1convert::Error::no_rebuild(
                        &self.h1.header().name,
                        &t.name,
                        format!("unsupported {} attachment", a.group),
                    )
                    .into());
                }
            };
            // A pctl with no active emitters genuinely emits nothing.
            let tags = result
                .map_err(|e| h1convert::Error::no_rebuild(&self.h1.header().name, &t.name, e))?;
            let mut scale = |index, kind| {
                let scale = functions::primary_scale(index, &functions);
                if index != 0 && scale.is_empty() {
                    notes.push(format!(
                        "attachment {} at {:?}: {kind} output {index} has no converted function",
                        t.name, a.marker,
                    ));
                }
                scale
            };
            let primary_scale = scale(a.primary_scale, "primary");
            let secondary_scale = scale(a.secondary_scale, "secondary");
            let colour_slot = h1convert::shad::halo2_change_colour_slot(a.change_colour);
            // Lights forward the signed index to 0x000bad50. An effect's
            // update (0x00179610) skips slot 0, which retail uses for none.
            let change_colour =
                colour_slot.map_or(if group == *b"effe" { 0 } else { -1 }, |slot| slot as i16);
            if group == *b"effe" && colour_slot == Some(0) {
                notes.push(format!(
                    "attachment {} at {:?}: Halo 2 effect updates skip change-colour slot 0",
                    t.name, a.marker
                ));
            }
            out.extend(tags.into_iter().map(|tag| AttachmentSpec {
                group,
                tag,
                marker: a.marker.clone(),
                primary_scale: primary_scale.clone(),
                secondary_scale: secondary_scale.clone(),
                change_colour,
            }));
        }
        let creation = obj
            .creation_effect
            .map(|id| {
                let t = self.h1_tag(id)?;
                self.effect(t)
            })
            .transpose()?;
        rep.no_home.extend(notes);
        Ok((out, creation, functions.into_iter().flatten().collect()))
    }

    pub(super) fn with_object_attachments(
        &mut self,
        group: [u8; 4],
        path: &str,
        root: Block,
        bytes: &[u8],
        obj: &h1::Object,
        rep: &mut ObjectReport,
    ) -> Result<Block> {
        if obj.attachments.is_empty() && obj.creation_effect.is_none() {
            return Ok(root);
        }
        let (attachments, creation, functions) = self.object_attachments(bytes, obj, rep)?;
        with_attachments(
            group,
            path,
            root,
            &attachments,
            creation,
            &functions,
            &obj.change_colours,
        )
    }

    pub(super) fn sound_scenery(&mut self, tag: &Tag) -> Result<TagId> {
        let group = *b"ssce";
        let bytes = h1::tag_bytes(self.h1, tag, &group, h1::OBJE_SIZE)?;
        let mut obj = h1::Object::read(self.h1, bytes)?;
        self.halo2_change_colours(&mut obj);
        let mut rep = Self::report(tag, "none: rebuilt from tags::defs");
        let model = self.model(&mut rep, &obj, &tag.name, None)?;
        let predicted = self.predicted(model);
        let (attachments, creation, functions) = self.object_attachments(bytes, &obj, &mut rep)?;
        let d = Defs::builtin().group(FourCC(group)).expect("ssce");
        let mut r = defs::default_record(d);
        for (path, value) in [
            ("object.object_type", Val::Enum(10)),
            ("object.flags", Val::Flags(obj.flags.into())),
            ("object.bounding_radius", Val::Float(obj.bounding_radius)),
            ("object.bounding_offset", Val::floats(&obj.bounding_offset)),
            (
                "object.model",
                Val::tag(FourCC(*b"hlmt"), model.unwrap_or(TagId::NULL)),
            ),
        ] {
            r.set(path, value)
                .map_err(|e| Error::Invalid(e.to_string()))?;
        }
        rebuild::predicted_resources(&group, &mut r, "object", &predicted)?;
        rebuild::object_effects(&group, &mut r, "object", &attachments, creation)?;
        rebuild::object_functions(&group, &mut r, "object", &functions)?;
        rebuild::object_change_colors(&group, &mut r, "object", &obj.change_colours)?;
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

/// Replace the object fields at the front of a built object. Legacy roots
/// still hold pointers into the base map in other fields.
pub(super) fn with_attachments(
    group: [u8; 4],
    path: &str,
    mut root: Block,
    attachments: &[AttachmentSpec],
    creation: Option<TagId>,
    functions: &[FunctionSpec],
    colours: &[h1::ChangeColour],
) -> Result<Block> {
    let d = Defs::builtin()
        .group(FourCC(group))
        .ok_or_else(|| Error::Invalid(format!("no definition for {}", FourCC(group))))?;
    let mut r = defs::default_record(d);
    rebuild::object_effects(&group, &mut r, path, attachments, creation)?;
    rebuild::object_functions(&group, &mut r, path, functions)?;
    // The arsenal roots do not write change colours. An attachment may
    // use these slots even when the object's shaders have no colour map.
    let write_colours = matches!(&group, b"weap" | b"eqip" | b"proj" | b"garb")
        && attachments.iter().any(|a| match &a.group {
            b"ligh" => a.change_colour >= 0,
            b"effe" => a.change_colour > 0,
            _ => false,
        })
        && !colours.is_empty();
    if write_colours {
        rebuild::object_change_colors(&group, &mut r, path, colours)?;
    }
    let src = defs::to_built(d, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))?;
    let fields = [0x4c, 0x94];
    for at in fields
        .into_iter()
        .chain((!functions.is_empty()).then_some(0x64))
        .chain(write_colours.then_some(0xac))
    {
        root.bytes[at..at + 8].copy_from_slice(&src.bytes[at..at + 8]);
        root.fixups.retain(|f| (f.offset as usize) != at);
        root.fixups.extend(
            src.fixups
                .iter()
                .filter(|f| f.offset as usize == at)
                .cloned(),
        );
    }
    Ok(root)
}
