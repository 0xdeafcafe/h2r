//! The lobby's Quick Options for a variant whose record names a rules
//! script (docs/game/rules-variants.md, "Quick Options in the lobby";
//! switch [`FEATURE`]).
//!
//! The shared part is [`rows`]: the script's `quick` marks read against
//! the lobby's record, each a label, the current value's text and the
//! values it may take. The pregame panel (`crate::lobby_options`) shows
//! them as "Label: value" lines through [`panel_ids`], in place of the
//! formatter's; `variants.quick` ([`AgentSource`]) reads them. The script
//! is the one the record's block names, when this machine has it
//! (`crate::rules::header::block_script`). A record that names no script
//! has no rows here, and keeps retail's Quick Options.
//!
//! Labels and value texts are ours: a setting's from
//! `rules::settings::TABLE` (`value_label`), an option's from the IR (its
//! label; "On" or "Off", a number, or its enum value's name). A record
//! setting's values are the ones the editor offered for it at the last
//! load of the variant list (`community_variants::last_offered`); before
//! one, it has none. An option's values are its range.
//!
//! The dialog that writes a row (`variants.quick.set`, Game Setup's
//! "Quick Options...") builds on [`QuickRow::mark`] and
//! [`QuickRow::values`].

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};

use ::rules::Ty;
use ::rules::ir::{Ir, Quick};
use agent_proto::{RpcError, code};
use engine::memory::{GuestMemory, MemFault};
use hooks::{BuildError, Builder};
use runtime::guest::Permissions;
use variants::ext::{self, Ext, Mode};
use variants::record::{GameVariant, NAME_AT, RECORD_SIZE, SettingId};

use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::screen::{SLOT_LEN, SlotTexts};

/// The switch (`H2_FEATURES=-variant-quick` keeps retail's rows for a
/// script variant too). Part of `lobby-options`, which installs it.
pub const FEATURE: &str = "variant-quick";

/// The panel's rows: the five texts the pregame panel shows
/// (`lobby_options`'s `PANEL_ROWS`, `rules::ir::MAX_QUICK`).
pub const ROWS: usize = ::rules::ir::MAX_QUICK;

const IDS: Family = ids::family("variant quick");
const _: () = assert!(IDS.len as usize >= ROWS);

static TEXTS: LazyLock<SlotTexts> = LazyLock::new(|| SlotTexts::new(IDS));

/// What a row changes: a record setting, or one of the script's options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A `rules::settings::TABLE` row and the record setting it stores on
    /// the script's carrier.
    Setting { index: u16, setting: u32 },
    /// An index into the script's options (`Ir::options`), whose bits are
    /// in the block's 32 option bits (`Ir::option_offsets`).
    Option { index: u16 },
}

/// One Quick Options row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickRow {
    pub mark: Mark,
    /// The setting's or the option's name in the script.
    pub name: String,
    /// Our label ("Respawn Time"), or the option's.
    pub label: String,
    /// The current value: the record's, or the option's from the bits.
    pub value: i32,
    /// The current value's text ("5 seconds", "Shotgun", "On").
    pub text: String,
    /// The option's description; a setting has none.
    pub help: Option<String>,
    /// The values it may take, each with its text, in the order offered.
    pub values: Vec<(i32, String)>,
}

impl QuickRow {
    /// The panel's line: "Label: value".
    pub fn line(&self) -> String {
        format!("{}: {}", self.label, self.text)
    }
}

/// The block in effect for a record: the store bundle's when the record
/// is built from one of our entries (`game_variants::inject::bundle_block`,
/// as the lobby's start gate reads it), else its own.
pub fn block_of(record: &GameVariant) -> [u8; ext::LEN] {
    let name = &record.bytes()[NAME_AT..NAME_AT + crate::rules::gate::RECORD_NAME_LEN];
    let engine = record.game_type().engine();
    crate::game_variants::inject::bundle_block(name, engine).unwrap_or(*record.ext_block())
}

/// The last script found, by its prefix: the panel asks at every lobby
/// update, and verifying a blob each time is waste.
static LAST: Mutex<Option<([u8; 8], Arc<Ir>)>> = Mutex::new(None);

/// The script the record's block names (mode 2), when this machine has
/// it. `None` for a record with no script, and for one naming a script
/// this machine lacks: both keep retail's Quick Options.
pub fn script_for(record: &GameVariant) -> Option<Arc<Ir>> {
    let block = block_of(record);
    let script = Ext::in_effect(&block).script;
    if script.mode != Mode::Rules {
        return None;
    }
    let last = || LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((prefix, ir)) = last().as_ref()
        && *prefix == script.prefix
    {
        return Some(Arc::clone(ir));
    }
    // Found without the cache held: the lookup takes the store's lock.
    let ir = Arc::new(crate::rules::header::block_script(&block)?);
    *last() = Some((script.prefix, Arc::clone(&ir)));
    Some(ir)
}

/// The rows of `ir`'s marks over `record`, in the script's order, with
/// the values the editor offered at the last list load.
pub fn rows(record: &GameVariant, ir: &Ir) -> Vec<QuickRow> {
    rows_with(
        record,
        ir,
        crate::community_variants::last_offered().as_ref(),
    )
}

/// [`rows`] with the editor's offer given: `None` gives each setting no
/// values (the panel needs none).
pub fn rows_with(
    record: &GameVariant,
    ir: &Ir,
    offered: Option<&HashMap<u32, Vec<i32>>>,
) -> Vec<QuickRow> {
    let bits = Ext::in_effect(&block_of(record)).script.options;
    ir.quick
        .iter()
        .filter_map(|&q| match q {
            Quick::Setting(index) => setting_row(record, ir, index, offered),
            Quick::Option(index) => option_row(ir, index, bits),
        })
        .collect()
}

fn setting_row(
    record: &GameVariant,
    ir: &Ir,
    index: u16,
    offered: Option<&HashMap<u32, Vec<i32>>>,
) -> Option<QuickRow> {
    use ::rules::settings::{row_at, value_label};
    // `verify::load` holds every mark to a row that applies to the
    // carrier, and the table to `record::field`, so neither is missed.
    let row = row_at(index)?;
    let setting = row.setting(ir.header.carrier)?;
    let value = record.get(SettingId(setting))?;
    let mut values: Vec<(i32, String)> = Vec::new();
    for &v in offered.and_then(|o| o.get(&setting)).into_iter().flatten() {
        if !values.iter().any(|(seen, _)| *seen == v) {
            values.push((v, value_label(index, v)));
        }
    }
    Some(QuickRow {
        mark: Mark::Setting { index, setting },
        name: row.name.into(),
        label: row.label.into(),
        value,
        text: value_label(index, value),
        help: None,
        values,
    })
}

fn option_row(ir: &Ir, index: u16, bits: u32) -> Option<QuickRow> {
    let i = usize::from(index);
    let o = ir.options.get(i)?;
    let name = ir.option_name(i).to_string();
    let value = ir.option_value(bits, &name)?;
    let names = ir.option_values(i);
    let text = |v: i32| match o.ty {
        Ty::Bool => (if v != 0 { "On" } else { "Off" }).to_string(),
        Ty::Enum(_) => usize::try_from(v - o.lo)
            .ok()
            .and_then(|p| names.get(p))
            .map_or_else(|| v.to_string(), |n| n.to_string()),
        _ => v.to_string(),
    };
    let string = |s: u16| ir.strings.get(usize::from(s)).cloned();
    Some(QuickRow {
        mark: Mark::Option { index },
        label: string(o.label).unwrap_or_else(|| name.clone()),
        name,
        value,
        text: text(value),
        help: o.description.and_then(string),
        values: (o.lo..=o.hi).map(|v| (v, text(v))).collect(),
    })
}

/// The panel's lines for `record`: `None` when it names no script this
/// machine has (retail's rows stay), else one line a mark, in order (none
/// for a script with no marks).
pub fn lines(record: &GameVariant) -> Option<Vec<String>> {
    let ir = script_for(record)?;
    Some(
        rows_with(record, &ir, None)
            .iter()
            .map(QuickRow::line)
            .collect(),
    )
}

/// Whether the switch is on: [`build`] ran and gave the texts their page.
pub fn installed() -> bool {
    TEXTS.base.load(Ordering::SeqCst) != 0
}

/// Gives the texts a guest page and the locale source that serves them.
/// `lobby_options` calls it when [`FEATURE`] is on.
pub fn build(b: &mut Builder) -> Result<(), BuildError> {
    let len = (IDS.len * SLOT_LEN).next_multiple_of(0x1000);
    let base = b.guest_alloc(len, Permissions::RW)?;
    TEXTS.base.store(base, Ordering::SeqCst);
    b.locale().source("variant quick", |id| TEXTS.address(id));
    Ok(())
}

/// The ids of the panel's lines for the record at `variant` (the panel
/// update's argument), with their texts written: `None` with the switch
/// off, for a record that cannot be read and for one with no script on
/// this machine. Every slot is written again each time, so a snapshot's
/// page never shows another run's text.
pub fn panel_ids(m: &mut dyn GuestMemory, variant: u32) -> Result<Option<Vec<u32>>, MemFault> {
    if !installed() || variant == 0 {
        return Ok(None);
    }
    let mut raw = [0u8; RECORD_SIZE];
    m.read(variant, &mut raw)?;
    let Ok(record) = GameVariant::from_bytes(&raw) else {
        return Ok(None);
    };
    let Some(lines) = lines(&record) else {
        return Ok(None);
    };
    let ids = lines
        .iter()
        .take(ROWS)
        .enumerate()
        .map(|(i, line)| TEXTS.set(i as u32, line))
        .collect();
    TEXTS.rewrite_all();
    TEXTS.write(m)?;
    Ok(Some(ids))
}

// ---- the agent's view ----

/// `variants.quick` (`agent::variants`) over the lobby's variant
/// (`pause_change_map::lobby_variant`).
pub struct AgentSource;

impl agent::variants::Source for AgentSource {
    fn quick(&self, m: &dyn GuestMemory) -> Result<agent::variants::Quick, RpcError> {
        let record = crate::pause_change_map::lobby_variant(m)
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no lobby variant to read"))?;
        Ok(quick_of(
            &record,
            script_for(&record).as_deref(),
            installed(),
        ))
    }
}

/// The wire form of `record`'s Quick Options with `ir` its script;
/// `on`: the switch is on.
pub fn quick_of(record: &GameVariant, ir: Option<&Ir>, on: bool) -> agent::variants::Quick {
    use agent::variants as wire;
    let script = ir.map(|ir| wire::Script {
        name: ir.header.name.clone(),
        hash: ir.id().0.iter().map(|b| format!("{b:02x}")).collect(),
    });
    let rows = ir.map(|ir| rows(record, ir)).unwrap_or_default();
    wire::Quick {
        variant: record.name(),
        engine: record.game_type().engine(),
        shown: on && ir.is_some(),
        script,
        rows: rows
            .into_iter()
            .enumerate()
            .map(|(slot, r)| wire::Row {
                slot: slot as u8,
                kind: match r.mark {
                    Mark::Setting { .. } => wire::Kind::Setting,
                    Mark::Option { .. } => wire::Kind::Option,
                },
                line: r.line(),
                name: r.name,
                label: r.label,
                value: r.value,
                text: r.text,
                help: r.help,
                values: r
                    .values
                    .into_iter()
                    .map(|(value, text)| wire::Choice { value, text })
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::ir::OptionValue;

    fn superbounce() -> (Ir, GameVariant) {
        let ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
        let record = variants::portable::record_for(&ir, &[]).unwrap();
        (ir, record)
    }

    /// Superbounce marks `respawn_time, starting_weapon, shields,
    /// bounce_points, suicide_costs`: three settings read from the record's
    /// fields and two options read from its block's bits, in that order,
    /// with our labels and texts and the values each may take.
    #[test]
    fn rows_read_settings_and_options_from_the_record() {
        let (ir, mut record) = superbounce();
        record.set(SettingId(17), 5).unwrap();
        record.set(SettingId(89), 9).unwrap();
        record.set(SettingId(19), 1).unwrap();
        let mut e = record.ext();
        e.script.options = ir
            .pack_options(&[
                ("bounce_points".into(), OptionValue::Int(3)),
                ("suicide_costs".into(), OptionValue::Bool(false)),
            ])
            .unwrap();
        record.set_ext(&e).unwrap();
        let offered = HashMap::from([
            (17, vec![0, 3, 5, 3, 10]),
            (89, vec![3, 4, 9]),
            (19, vec![0, 1, 2]),
        ]);

        let rows = rows_with(&record, &ir, Some(&offered));
        let lines: Vec<String> = rows.iter().map(QuickRow::line).collect();
        assert_eq!(
            lines,
            [
                "Respawn Time: 5 seconds",
                "Starting Weapon: Shotgun",
                "Shields: None",
                "Bounce points: 3",
                "Suicide penalty: Off",
            ]
        );
        let at = |n: &str| ::rules::settings::named(n).unwrap().0;
        assert_eq!(
            rows.iter().map(|r| r.mark).collect::<Vec<_>>(),
            [
                Mark::Setting {
                    index: at("respawn_time"),
                    setting: 17
                },
                Mark::Setting {
                    index: at("starting_weapon"),
                    setting: 89
                },
                Mark::Setting {
                    index: at("shields"),
                    setting: 19
                },
                Mark::Option { index: 1 },
                Mark::Option { index: 0 },
            ]
        );
        // The editor's values, once each, in its order.
        assert_eq!(
            rows[0].values,
            [
                (0, "0 seconds".to_string()),
                (3, "3 seconds".to_string()),
                (5, "5 seconds".to_string()),
                (10, "10 seconds".to_string())
            ]
        );
        assert_eq!(rows[2].values[2], (2, "Overshields".to_string()));
        // An option's values are its range; its help its description.
        assert_eq!(rows[3].values.len(), 6);
        assert_eq!(rows[3].values[3], (3, "3".to_string()));
        assert_eq!(
            rows[4].values,
            [(0, "Off".to_string()), (1, "On".to_string())]
        );
        assert!(rows[0].help.is_none());
        assert_eq!(
            rows[3].help.as_deref(),
            Some("Points for each superbounce, times its place in the chain")
        );
        // With no offer a setting has no values; the texts stand.
        let bare = rows_with(&record, &ir, None);
        assert!(bare[..3].iter().all(|r| r.values.is_empty()));
        assert_eq!(bare.iter().map(QuickRow::line).collect::<Vec<_>>(), lines);
    }

    /// The record names its script by prefix, so the lines come from the
    /// record alone; the wire form carries the rows by slot.
    #[test]
    fn the_record_finds_its_script_and_the_wire_lists_its_rows() {
        let (ir, record) = superbounce();
        assert_eq!(script_for(&record).unwrap().id(), ir.id());
        assert_eq!(lines(&record).unwrap().len(), ir.quick.len());
        let q = quick_of(&record, Some(&ir), true);
        assert!(q.shown);
        assert_eq!(q.variant, "Superbounce");
        assert_eq!(q.script.as_ref().unwrap().name, "Superbounce");
        assert_eq!(q.rows.len(), 5);
        assert_eq!(q.rows[3].kind, agent::variants::Kind::Option);
        assert_eq!(q.rows[4].slot, 4);
        assert!(!quick_of(&record, Some(&ir), false).shown);
        let none = quick_of(&record, None, true);
        assert!(!none.shown && none.rows.is_empty() && none.script.is_none());
    }
}
