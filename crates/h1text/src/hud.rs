//! Rebuild Halo 2's legacy HUD message list and translate its controller
//! substitutions. See docs/halo1/text.md for the two engines' readers.

use h1models::Converted;
use map::FourCC;
use tags::defs::{Defs, Record, Val, default_record, to_built};

use crate::{Error, Result};
use crate::events::Events;
use crate::h1::{Hmt, HudMessage, MessagePart};

/// Scenario reference read by the Halo 2 help and objective setters.
pub const SCENARIO_HMT: u32 = 0x208;

/// The converted HUD text tag group.
pub const GROUP: FourCC = FourCC(*b"hmt ");

/// Root size including the converter's runtime fields.
pub const SIZE: u32 = 0xb0;

/// The marker and HUD state after the unmodified legacy tag fields.
pub const MARKER_AT: u32 = 0x18;
/// Value written by the converter, for the runtime hooks to recognize.
pub const MARKER: u32 = 0x4831_4856;
/// Marker of the first mission-text layout, before shared events and panel tints.
pub const LEGACY_MARKER: u32 = 0x4831_4855;
/// Root size of the first mission-text layout.
pub const LEGACY_SIZE: u32 = 0x94;
/// Persistent visibility and blink bits, with the same bit positions as Halo 1.
pub const CONTROL_FLAGS: u32 = 0x1c;
/// The flash record read by the native help draw hook.
pub const HELP_FLASH: u32 = 0x20;
/// The runtime widget classification block.
pub const WIDGET_CONTROLS: u32 = 0x38;

/// Runtime start ticks for the three component flash records.
pub const COMPONENT_START: u32 = 0x88;
/// Widget control record size, including its static colour and flash fields.
pub const WIDGET_CONTROL_SIZE: u32 = 0x24;
/// First appended scenario help row, followed by its row count.
pub const SCENARIO_HELP: u32 = 0x94;
/// First appended shared event row, followed by its row count.
pub const SHARED_MESSAGES: u32 = 0x98;
/// Native event string IDs and the template and argument rows they select.
pub const EVENTS: u32 = 0x9c;
/// A string ID, one template row, and two argument row indices.
pub const EVENT_SIZE: u32 = 0xc;
/// Per-placement argument rows for the scenario's device controls.
pub const CONTROL_PROMPTS: u32 = 0xa4;
/// The shared template row for a control with an authored prompt.
pub const CUSTOM_DEVICE: u32 = 0xac;

/// Hidden health, shield, motion sensor and crosshair, respectively.
/// Each of the first three is followed by its blink bit, as in Halo 1's
/// unit HUD. The crosshair was a separate word there.
pub const HEALTH: u32 = 1;
/// See [`HEALTH`].
pub const SHIELD: u32 = 4;
/// See [`HEALTH`].
pub const SENSOR: u32 = 16;
/// See [`HEALTH`].
pub const CROSSHAIR: u32 = 64;

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

/// A glyph in a mission message expressed as a Halo 2 font character or
/// controller substitution. Halo 2 resolves action substitutions against
/// the current player's bindings at draw time.
pub fn glyph(index: u8) -> Option<u16> {
    Some(match index {
        // Literal buttons and sticks in the Halo 2 font.
        0..=13 => 0xe100 + u16::from(index),
        14 | 15 => 0xe110 + u16::from(index - 14),
        16 | 17 => 0xe10e + u16::from(index - 16),
        // Halo 1's game actions (0xdfc30, table 0x21bfc0) to Halo 2's
        // localized text input actions (0x470204, used by 0x22d2ee).
        18 => 0xe448,      // action / reload
        19 => 0xe44c,      // throw grenade
        20 => 0xe44d,      // fire
        21 => 0xe44b,      // flashlight
        22 | 28 => 0xe446, // jump
        23 => 0xe44a,      // melee
        24 => 0xe449,      // switch weapon
        25 => 0xe447,      // switch grenade
        26 => 0xe451,      // zoom
        27 => 0xe450,      // crouch
        29 => 0xe101,      // B
        30 => 0xe456,      // move stick
        31 => 0xe457,      // look stick
        _ => return None,
    })
}

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
    root.set("unknown_20", Val::Bytes(help_flash.to_vec()))?;
    root.set("help_first", field_u16(source.messages.len())?)?;
    root.set("help_count", field_u16(help.len())?)?;
    root.set("shared_first", field_u16(source.messages.len() + help.len())?)?;
    root.set("shared_count", field_u16(events.entries.len())?)?;
    root.set(
        "custom_device",
        field_u16(events.custom_device.unwrap_or(usize::from(u16::MAX)))?,
    )?;
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
            record.set("unknown_c", Val::Bytes(flash.to_vec()))?;
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

fn field_u16(value: usize) -> Result<Val> {
    u16::try_from(value)
        .map(|value| Val::Int(i64::from(value)))
        .map_err(|_| Error::Invalid("HUD message start exceeds 16 bits".into()))
}
