//! Rebuild Halo 2's legacy HUD message list and translate its controller
//! substitutions. See docs/halo1/text.md for the two engines' readers.

use h1models::Converted;
use tags::defs::{Defs, Record, Val, default_record, to_built};

use crate::{Error, Result};
use crate::events::Events;
use crate::h1::{Hmt, HudMessage, MessagePart};

/// The tag layout and glyphs `boot` reads back, kept in `h1-contract`.
pub use h1_contract::hud::*;

const WIDGET_NAMES: &[(&str, u32)] = &[
    ("h1_health_meter", HEALTH),
    ("h1_health_background", HEALTH),
    ("health_meter", HEALTH),
    ("h1_shield_meter", SHIELD),
    ("h1_shield_background", SHIELD),
    ("shield_meter", SHIELD),
    ("h1_motion_background", SENSOR),
    ("h1_motion_foreground", SENSOR),
    ("motion_sensor", SENSOR),
    ("h1_scope", CROSSHAIR),
    ("h1_crosshair", CROSSHAIR),
];

/// Render a mission-localized row while keeping controller substitutions as
/// values the Halo 2 text formatter can resolve.
pub fn localized_text(message: &HudMessage) -> Result<String> {
    let mut out = String::new();
    for part in &message.parts {
        match part {
            MessagePart::Text(text) => out.push_str(text),
            MessagePart::Glyph(index) => {
                let unit = glyph(*index).ok_or_else(|| {
                    Error::Unsupported(format!(
                        "HUD message {:?} requires event parameter {index}",
                        message.name
                    ))
                })?;
                out.push(char::from_u32(u32::from(unit)).expect("BMP glyph"));
            }
        }
    }
    Ok(out)
}

/// Rebuild a level's HUD from the definition, keeping the source lists indexed separately.
pub(crate) fn rebuild(
    source: &Hmt,
    help: &[HudMessage],
    events: &Events,
    help_flash: &[u8; 0x18],
    unit_hud: &[u8],
    string_names: &[Option<String>],
) -> Result<Converted> {
    let def = Defs::builtin().group(GROUP).expect("hmt definition");
    debug_assert_eq!(def.size, SIZE);
    let element = def.child("elements").expect("element definition");
    let message = def.child("messages").expect("message definition");
    let mut text = Vec::new();
    let mut elements: Vec<Record> = Vec::new();
    let mut messages = Vec::new();
    for entry in source.messages.iter().chain(help).chain(&events.entries) {
        let mut record = default_record(message);
        let mut name = vec![0; 0x20];
        if entry.name.len() > name.len() {
            return Err(Error::Invalid("HUD message name exceeds 32 bytes".into()));
        }
        name[..entry.name.len()].copy_from_slice(entry.name.as_bytes());
        record.set("unknown_0", Val::Bytes(name))?;
        record.set("text_start", field_u16(text.len() / 2)?)?;
        record.set("first_element", field_u16(elements.len())?)?;
        record.set(
            "element_count",
            Val::Int(i64::from(u8::try_from(entry.parts.len()).map_err(
                |_| Error::Invalid("HUD message has more than 255 elements".into()),
            )?)),
        )?;
        messages.push(record);
        for part in &entry.parts {
            let (kind, value) = match part {
                MessagePart::Text(run) => {
                    let units = run.encode_utf16().chain([0]);
                    let count = u8::try_from(units.clone().count())
                        .map_err(|_| Error::Invalid("HUD text run exceeds 255 units".into()))?;
                    text.extend(units.flat_map(u16::to_le_bytes));
                    (0, count)
                }
                MessagePart::Glyph(index) => (1, *index),
            };
            let mut record = default_record(element);
            record.set("kind", Val::Int(kind))?;
            record.set("value", Val::Int(i64::from(value)))?;
            elements.push(record);
        }
    }
    let mut root = default_record(def);
    root.set("text", Val::Data(text))?;
    root.set("elements", Val::Block(elements))?;
    root.set("messages", Val::Block(messages))?;
    root.set("converter_marker", Val::Int(i64::from(MARKER)))?;
    set_flash(&mut root, "help_flash", help_flash)?;
    root.set("help_first", field_u16(source.messages.len())?)?;
    root.set("help_count", field_u16(help.len())?)?;
    root.set("shared_first", field_u16(source.messages.len() + help.len())?)?;
    root.set("shared_count", field_u16(events.entries.len())?)?;
    root.set(
        "custom_device",
        field_u16(events.custom_device.unwrap_or(usize::from(u16::MAX)))?,
    )?;
    for (field, index) in [
        ("exit_vehicle", events.exit_vehicle),
        ("swap_powerup", events.swap_powerup),
    ] {
        root.set(field, field_u16(index.unwrap_or(usize::from(u16::MAX)))?)?;
    }
    let sprite_def = def.child("silhouettes").expect("silhouette definition");
    let mut silhouettes = Vec::new();
    for sprite in &events.silhouettes {
        let mut row = default_record(sprite_def);
        row.set("cache_key", Val::Word(sprite.cache_key()))?;
        row.set("width", Val::Int(i64::from(sprite.width)))?;
        row.set("height", Val::Int(i64::from(sprite.height)))?;
        row.set("advance", Val::Int(i64::from(sprite.advance)))?;
        row.set("x_offset", Val::Int(i64::from(sprite.x_offset)))?;
        row.set("y_offset", Val::Int(i64::from(sprite.y_offset)))?;
        row.set("pixels", Val::Data(sprite.pixels.clone()))?;
        silhouettes.push(row);
    }
    root.set("silhouettes", Val::Block(silhouettes))?;
    let subject_def = def.child("prompt_subjects").expect("prompt subject definition");
    let mut subjects = Vec::new();
    for subject in &events.subjects {
        let mut row = default_record(subject_def);
        row.set("argument", field_u16(subject.argument)?)?;
        subjects.push(row);
    }
    root.set("prompt_subjects", Val::Block(subjects))?;
    let mut routes = Vec::new();
    let event_def = def.child("events").expect("event definition");
    for event in &events.routes {
        let mut row = default_record(event_def);
        row.set("key", Val::text(&event.key))?;
        row.set("template", field_u16(event.template)?)?;
        row.set("control", Val::Int(event.is_control() as i64))?;
        for (field, index) in ["first_argument", "second_argument"].into_iter().zip(event.args) {
            row.set(field, field_u16(index.unwrap_or(usize::from(u16::MAX)))?)?;
        }
        routes.push(row);
    }
    root.set("events", Val::Block(routes))?;
    let widget = def.child("widgets").expect("widget definition");
    let mut widgets = Vec::new();
    let mut add_widget = |name: &str, control: u32| -> Result<()> {
        let mut record = default_record(widget);
        record.set("name", Val::String(tags::defs::Str::Text(name.to_owned())))?;
        record.set("control", Val::Int(i64::from(control)))?;
        let flash = match name {
            "h1_health_background" => Some(0x17c),
            "h1_shield_background" => Some(0x8c),
            "h1_motion_background" => Some(0x26c),
            "h1_motion_foreground" => Some(0x2d4),
            _ => None,
        }
        .and_then(|at| unit_hud.get(at + 0x34..at + 0x4c));
        if let Some(flash) = flash {
            record.set("has_colour", Val::Int(1))?;
            set_flash(&mut record, "flash", flash)?;
        }
        widgets.push(record);
        Ok(())
    };
    for &(name, control) in WIDGET_NAMES {
        add_widget(name, control)?;
    }
    for name in string_names.iter().flatten() {
        if name.contains("reticle") || name.contains("crosshair") {
            add_widget(name, CROSSHAIR)?;
        }
    }
    root.set("widgets", Val::Block(widgets))?;
    Ok(Converted {
        group: GROUP,
        root: to_built(def, &root, &mut |_| None)?,
        raw: Vec::new(),
    })
}

/// Fill the `h1_hud_flash` struct at `path` from Halo 1's 0x18-byte flash
/// record, as `boot::h1_hud_text::flash` reads it back.
fn set_flash(record: &mut Record, path: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() != 0x18 {
        return Err(Error::Invalid("HUD flash record is not 0x18 bytes".into()));
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    let half = |at: usize| Val::Int(i64::from(u16::from_le_bytes([bytes[at], bytes[at + 1]])));
    for (name, value) in [
        ("colour", Val::Word(word(0))),
        ("flash_colour", Val::Word(word(4))),
        ("repeat", Val::Float(f32::from_bits(word(8)))),
        ("off", Val::Float(f32::from_bits(word(0xc)))),
        ("count", half(0x10)),
        ("flags", half(0x12)),
        ("on", Val::Float(f32::from_bits(word(0x14)))),
    ] {
        record.set(&format!("{path}.{name}"), value)?;
    }
    Ok(())
}

fn field_u16(value: usize) -> Result<Val> {
    u16::try_from(value)
        .map(|value| Val::Int(i64::from(value)))
        .map_err(|_| Error::Invalid("HUD message start exceeds 16 bits".into()))
}
