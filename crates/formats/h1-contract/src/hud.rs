//! The converted HUD text tag (`hmt `): the runtime fields the converter
//! appends after Halo 1's legacy fields, and the controller glyphs of a
//! mission message. `h1text` writes them; `boot::h1_hud_text` reads them.
//! See docs/halo1/text.md for the two engines' readers.

use map::FourCC;

/// Scenario reference read by the Halo 2 help and objective setters.
pub const SCENARIO_HMT: u32 = 0x208;

/// The converted HUD text tag group.
pub const GROUP: FourCC = FourCC(*b"hmt ");

/// Root size including the converter's runtime fields.
pub const SIZE: u32 = 0xc8;

/// The marker and HUD state after the unmodified legacy tag fields.
pub const MARKER_AT: u32 = 0x18;
/// Value written by the converter, for the runtime hooks to recognize.
pub const MARKER: u32 = 0x4831_4857;
/// Marker of the first shared event layout.
pub const EVENT_MARKER: u32 = 0x4831_4856;
/// Root size before silhouettes and contextual prompts.
pub const EVENT_LAYOUT_SIZE: u32 = 0xb0;
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
/// Inline ARGB4444 glyphs rebuilt from the source message bitmap.
pub const SILHOUETTES: u32 = 0xb0;
/// Glyph cache key, metrics and pixel data reference.
pub const SILHOUETTE_SIZE: u32 = 0x18;
/// The first code unit reserved for converted HUD sprites.
pub const SILHOUETTE_BASE: u16 = 0xe500;
/// Number of private glyph code units reserved for converted HUD sprites.
pub const SILHOUETTE_LIMIT: u16 = 0x100;
/// Converted object definition datums and their text argument rows.
pub const PROMPT_SUBJECTS: u32 = 0xb8;
/// A definition datum and argument row.
pub const PROMPT_SUBJECT_SIZE: u32 = 8;
/// The shared template row for exiting a vehicle.
pub const EXIT_VEHICLE: u32 = 0xc0;
/// The shared template row for swapping held equipment.
pub const SWAP_POWERUP: u32 = 0xc2;

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
