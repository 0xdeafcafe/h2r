//! Halo 1's native pickups exercise Halo 2's own powerup branches at
//! 0x00151f4b (overshield), 0x00151f85 (camouflage) and 0x00151f6a (health).
//! The pickup's source equipment and model chain supply the tag data.

use h1map::Tag;
use h1models::Converted;
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, DefError, Defs, Val};

use super::Converter;
use crate::h1::{self, Equipment};
use crate::{Error, Result, rebuild};

const EQUIPMENT: [u8; 4] = *b"eqip";
const HEADER: &str = "item.obje";
// The words match on the health, overshield and camouflage pairs on the
// two discs. The retail string IDs differ between maps and are interned
// by name when the new tag is installed.
const H1_ITEM_FLAGS: usize = 0x17c;
const H1_SORT_ORDER: usize = 0x180;
const H1_DETONATION_EFFECTS: [(usize, &str); 2] =
    [(0x2e8, "item.detonating_effect"), (0x2f8, "item.detonation_effect")];

fn def_error(err: DefError) -> Error {
    Error::Invalid(err.to_string())
}

impl Converter<'_> {
    pub(super) fn powerup(&mut self, tag: &Tag, e: &Equipment) -> Result<TagId> {
        let b = h1::tag_bytes(self.h1, tag, &EQUIPMENT, h1::EQIP_SIZE)?;
        let mut rep = Self::report(tag, "rebuilt from the eqip definition");
        let mut object = e.object.clone();
        self.halo2_change_colours(&mut object);
        let hlmt = self.model(&mut rep, &object, None, &tag.name, true, None)?;
        let predicted = self.predicted(hlmt);
        let mut root = equipment_values(e, b, hlmt)?;
        let (attachments, creation, functions) = self.object_attachments(b, &e.object, &mut rep)?;
        rebuild::object_effects(&EQUIPMENT, &mut root, HEADER, &attachments, creation)?;
        rebuild::object_functions(&EQUIPMENT, &mut root, HEADER, &functions)?;
        rebuild::predicted_resources(&EQUIPMENT, &mut root, HEADER, &predicted)?;
        rebuild::object_change_colors(&EQUIPMENT, &mut root, HEADER, &object.change_colours)?;

        if let Some(id) = e.pickup_sound {
            let source = self.h1_tag(id)?;
            if let Some(sound) = self.sound(source, &mut rep) {
                root.set("snd_ref_134", Val::tag(FourCC(*b"snd!"), sound))
                    .map_err(def_error)?;
            }
        }
        for (at, field) in H1_DETONATION_EFFECTS {
            if let Some(id) = h1::reference(self.h1, b, at) {
                let effect = self.effect(self.h1_tag(id)?)?;
                root.set(field, Val::tag(FourCC(*b"effe"), effect))
                    .map_err(def_error)?;
            }
        }
        if let Some(foot) = self.h1_foot(b, &mut rep)? {
            root.set("item.obje.foot_ref_54", Val::tag(FourCC(*b"foot"), foot))
                .map_err(def_error)?;
        }

        let root = encode(&root)?;
        self.tree.reports.push(rep);
        Ok(self.tree.push(
            &tag.name,
            Converted {
                group: FourCC(EQUIPMENT),
                root,
                raw: Vec::new(),
            },
        ))
    }
}

fn equipment_values(e: &Equipment, b: &[u8], hlmt: Option<TagId>) -> Result<defs::Record> {
    let d = Defs::builtin().group(FourCC(EQUIPMENT)).expect("eqip");
    let mut r = defs::default_record(d);
    let message = match e.powerup_type {
        2 => "ovs_picked_up",
        3 => "atc_picked_up",
        _ => "",
    };
    // A health pickup with a nonpositive time is rejected before type
    // dispatch at 0x00151f30.
    let time = if e.powerup_type == super::POWERUP_HEALTH && e.powerup_time <= 0.0 {
        1.0
    } else {
        e.powerup_time
    };
    for (field, value) in [
        ("item.obje.object_type", Val::Enum(3)),
        ("item.obje.flags", Val::Flags(e.object.flags.into())),
        (
            "item.obje.bounding_radius",
            Val::Float(e.object.bounding_radius),
        ),
        (
            "item.obje.bounding_offset",
            Val::floats(&e.object.bounding_offset),
        ),
        (
            "item.obje.model",
            Val::tag(FourCC(*b"hlmt"), hlmt.unwrap_or(TagId::NULL)),
        ),
        (
            "item.unknown_bc",
            Val::Bytes(b[H1_ITEM_FLAGS..H1_ITEM_FLAGS + 4].to_vec()),
        ),
        (
            "item.unknown_c0",
            Val::Bytes(b[H1_SORT_ORDER..H1_SORT_ORDER + 4].to_vec()),
        ),
        ("item.string_e0", Val::text(message)),
        ("powerup_type", Val::Int(e.powerup_type.into())),
        ("grenade_type", Val::Int(e.grenade_type.into())),
        ("powerup_time", Val::Float(time)),
    ] {
        r.set(field, value).map_err(def_error)?;
    }
    Ok(r)
}

fn encode(record: &defs::Record) -> Result<Block> {
    let d = Defs::builtin().group(FourCC(EQUIPMENT)).expect("eqip");
    defs::to_built(d, record, &mut |_| None).map_err(def_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerups_write_native_fields_and_source_values_without_a_base_tag() {
        for (kind, time, message) in [
            (2, 37.5f32, "ovs_picked_up"),
            (3, 14.0, "atc_picked_up"),
            (5, 1.0, ""),
        ] {
            let item = Equipment {
                object: h1::Object {
                    flags: 4,
                    bounding_radius: 0.25,
                    acceleration_scale: 1.0,
                    object_type: 3,
                    bounding_offset: [0.0; 3],
                    model: None,
                    animation: None,
                    collision: None,
                    physics: None,
                    modifier_shader: None,
                    creation_effect: None,
                    attachments: Vec::new(),
                    widgets: Vec::new(),
                    change_colours: Vec::new(),
                },
                powerup_type: kind,
                grenade_type: 0,
                powerup_time: time,
                pickup_sound: None,
            };
            let mut h1 = vec![0; h1::EQIP_SIZE];
            h1[H1_SORT_ORDER] = 13;
            let model = TagId(0xe174_0000);
            let record = equipment_values(&item, &h1, Some(model)).unwrap();
            assert_eq!(record.at("item.string_e0"), Some(&Val::text(message)));
            let built = encode(&record).unwrap();
            assert_eq!(&built.bytes[..4], &[3, 0, 4, 0]);
            assert_eq!(built.bytes[0xc0], 13);
            assert_eq!(&built.bytes[0x12c..0x12e], &kind.to_le_bytes());
            assert_eq!(&built.bytes[0x130..0x134], &time.to_le_bytes());
            assert!(built.fixups.iter().any(|f| f.offset == 0x34
                && matches!(f.kind, map::build::FixupKind::TagRef(id) if id == model)));
        }
    }
}
