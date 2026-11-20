//! Build the weapon interface from the native nhdt definition.
use map::{FourCC, build::Block};
use tags::defs::{self, Defs, Record, StructDef, Val};
use tags::weapon_hud as input;

use super::{TextWidget, WeaponEntry, WeaponEntryKind, Widget};
use crate::{Error, Result};

pub(crate) struct Interface {
    def: &'static StructDef,
    bitmaps: Vec<Record>,
    texts: Vec<Record>,
}

fn set(record: &mut Record, name: &str, value: Val) -> Result<()> {
    record
        .set(name, value)
        .map_err(|e| Error::Invalid(e.to_string()))
}

fn header(
    def: &StructDef,
    name: &str,
    inputs: [u8; 4],
    state: [u8; 20],
    screen: u8,
) -> Result<Record> {
    if screen > 2 {
        return Err(Error::Invalid(format!("weapon HUD screen type {screen}")));
    }
    let mut r = defs::default_record(def);
    set(&mut r, "name", Val::text(name))?;
    for (i, value) in inputs.into_iter().enumerate() {
        set(&mut r, &format!("input_{}", i + 1), Val::Int(value.into()))?;
    }
    for (i, field) in [
        "show_when_a",
        "show_when_b",
        "show_when_c",
        "show_when_d",
        "hide_when_a",
        "hide_when_b",
        "hide_when_c",
        "hide_when_d",
    ]
    .into_iter()
    .enumerate()
    {
        let word = u16::from_le_bytes([state[2 * i], state[2 * i + 1]]);
        let value = if i % 4 == 0 {
            Val::Flags(word.into())
        } else {
            Val::Int(word.into())
        };
        set(&mut r, field, value)?;
    }
    for (i, field) in ["threshold_battery", "threshold_loaded", "threshold_reserve"]
        .into_iter()
        .enumerate()
    {
        set(&mut r, field, Val::Int(state[16 + i].into()))?;
    }
    set(
        &mut r,
        "h1_screen",
        Val::Int((input::SCREEN + screen).into()),
    )?;
    Ok(r)
}

fn offset(record: &mut Record, mode: &str, (x, y): (i16, i16)) -> Result<()> {
    set(
        record,
        &format!("offset_{mode}"),
        Val::List(vec![Val::Int(x.into()), Val::Int(y.into())]),
    )
}

impl Interface {
    pub(crate) fn new() -> Self {
        Self {
            def: Defs::builtin()
                .group(FourCC(*b"nhdt"))
                .expect("nhdt definition"),
            bitmaps: Vec::new(),
            texts: Vec::new(),
        }
    }

    pub(crate) fn bitmap(&mut self, e: &WeaponEntry, w: Widget) -> Result<()> {
        let def = self
            .def
            .child("bitmap_widgets")
            .expect("nhdt bitmap widgets");
        let mut r = header(def, &w.name, w.inputs, w.state, e.screen)?;
        set(&mut r, "anchor", Val::Enum(w.anchor as u32))?;
        set(&mut r, "flags", Val::Flags(w.flags.into()))?;
        set(&mut r, "shader", Val::tag(FourCC(*b"shad"), w.shader))?;
        set(&mut r, "bitmap", Val::tag(FourCC(*b"bitm"), w.bitmap))?;
        let crosshair = matches!(
            e.kind,
            WeaponEntryKind::Crosshair(..) | WeaponEntryKind::Warning(..)
        );
        let half_size = crosshair && e.split_flags & 2 == 0;
        set(&mut r, "h1_half_size", Val::Int(i64::from(half_size)))?;
        set(&mut r, "h1_frame_period", Val::Int(e.frame_period.into()))?;
        for mode in ["full", "half", "quarter"] {
            set(
                &mut r,
                &format!("sequence_{mode}"),
                Val::Int(w.sequence.into()),
            )?;
            offset(&mut r, mode, w.offset)?;
            set(
                &mut r,
                &format!("registration_{mode}"),
                Val::List(w.registration.map(Val::Float).into()),
            )?;
        }
        let warning = matches!(e.kind, WeaponEntryKind::Warning(..));
        if !super::unscaled(e.scale) || crosshair {
            let mut effect = defs::default_record(def.child("effects").expect("bitmap effects"));
            if !super::unscaled(e.scale) {
                // 0x22be12..0x22beaa scales each basis vector separately.
                set(&mut effect, "scale_flags", Val::Flags(1))?;
                for (field, scale) in ["block_10", "block_24"].into_iter().zip(e.scale) {
                    set(
                        &mut effect,
                        field,
                        Val::Data(crate::particles::constant_data(scale, None)),
                    )?;
                }
            }
            if warning {
                let mut profile = Vec::with_capacity(input::WARNING_BYTES);
                for threshold in e.thresholds {
                    profile.extend(threshold.to_le_bytes());
                }
                profile.extend(e.flash);
                set(&mut effect, "block_38", Val::Data(profile))?;
            }
            if let WeaponEntryKind::Crosshair(kind, flags) = e.kind {
                let mut profile = vec![kind as u8, flags as u8];
                profile.extend(e.flash);
                set(&mut effect, "block_4c", Val::Data(profile))?;
            }
            set(&mut r, "effects", Val::Block(vec![effect]))?;
        }
        self.bitmaps.push(r);
        Ok(())
    }

    pub(crate) fn text(&mut self, e: &WeaponEntry, w: TextWidget) -> Result<()> {
        let def = self.def.child("text_widgets").expect("nhdt text widgets");
        let mut r = header(def, &w.name, w.inputs, w.state, e.screen)?;
        set(&mut r, "anchor", Val::Enum(w.anchor as u32))?;
        set(&mut r, "flags", Val::Flags(w.flags.into()))?;
        set(&mut r, "shader", Val::tag(FourCC(*b"shad"), w.shader))?;
        set(&mut r, "text", Val::text(w.string))?;
        set(&mut r, "justification", Val::Enum(w.justification.into()))?;
        for mode in ["full", "half", "quarter"] {
            set(
                &mut r,
                &format!("font_{mode}"),
                Val::Int(super::TEXT_FONT.into()),
            )?;
            offset(&mut r, mode, w.offset)?;
        }
        self.texts.push(r);
        Ok(())
    }

    pub(crate) fn build(self) -> Result<Block> {
        let mut r = defs::default_record(self.def);
        let mut parent = FourCC(*b"nhdt").as_u32().to_le_bytes().to_vec();
        parent.extend(u32::MAX.to_le_bytes());
        set(&mut r, "unknown_0", Val::Bytes(parent))?;
        set(&mut r, "bitmap_widgets", Val::Block(self.bitmaps))?;
        set(&mut r, "text_widgets", Val::Block(self.texts))?;
        defs::to_built(self.def, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
    }
}
