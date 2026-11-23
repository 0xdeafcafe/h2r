//! `.h2variant`: our file for a game variant outside the game. JSON, so
//! people can read what it is, with the 0x130-byte record as hex as the
//! source of truth (unknown bytes travel too). The game never reads it, so
//! it has no signature (docs/editor/variants.md, "Portable files").
//!
//! Version 2 is a whole game variant: the name and description, who made
//! it, the record, and, when the record's extension block names a rules
//! script (mode 2), the script itself: its IR blob and, optionally, its
//! source. The record carries the 8-byte hash prefix that names the
//! script (docs/game/rules-engine.md, "Carrying a script"); the file
//! carries the blob that prefix names, so it plays with no server.
//! [`Bundle::canonical`] is the file's one byte form and [`Bundle::id`]
//! its SHA-256, so the same variant is the same id everywhere (Live's
//! `/v1/variants/{id}`).
//!
//! Version 1 (name, game type and record only) is still read: [`read`]
//! takes it as a version-2 bundle with no description and no script.
//! [`export`] still writes it, for editors that have no script blob at
//! hand; [`port`] makes a version-2 file from any variant.
//!
//! A script's own variant is [`record_for`]'s record: the stock record of
//! its carrier with the script's `settings`, its name and the reference to
//! it. [`from_script`] makes its bundle (docs/game/rules-variants.md, "The
//! record of a script").

use content::Sha256;
use serde::{Deserialize, Serialize};

use crate::ext::{self, Block, Mode};
use crate::record::{self, GameType, GameVariant, RECORD_SIZE, SettingId};

pub const FORMAT: &str = "h2recomp-game-variant";
/// The newest version this build reads and the one [`port`] writes.
pub const VERSION: u32 = 2;
/// The version [`export`] writes: name, game type and record.
pub const VERSION_1: u32 = 1;
pub const EXTENSION: &str = "h2variant";

/// A description's length, UTF-16 units (the game's clan description
/// keyboard, docs/game/game-options.md section 4).
pub const MAX_DESCRIPTION_UNITS: usize = 150;
/// The rules IR's cap (`rules::ir::MAX_IR_BYTES`).
pub const MAX_IR: usize = rules::ir::MAX_IR_BYTES;
/// The rules source's cap (`rules::lang::MAX_SOURCE_BYTES`).
pub const MAX_SOURCE: usize = rules::lang::MAX_SOURCE_BYTES;

/// A saved-variant file: the record, then its 20-byte signature
/// ([`crate::save::FILE_SIZE`]).
pub const SAVE_FILE: usize = crate::save::FILE_SIZE;
/// A hopper record slot (`0x37c` bytes) ends with the variant record at
/// `+0x24c` (`crates/live/live-proto/src/hopper/variant.rs`).
pub const HOPPER_SLOT: usize = 0x37c;
pub const HOPPER_SLOT_RECORD_AT: usize = 0x24c;

/// Version 1, as written.
#[derive(Debug, Serialize, Deserialize)]
struct File1 {
    format: String,
    version: u32,
    /// For people; the record decides.
    name: String,
    game_type: String,
    record: String,
}

#[derive(Deserialize)]
struct Probe {
    format: String,
    version: u32,
}

/// Who published a variant (Live writes it; a file made offline has none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub xuid: String,
    pub gamertag: String,
}

/// The script a version-2 file carries whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptPart {
    /// The script's `rules "<name>"`.
    pub name: String,
    /// The IR's SHA-256, 64 lowercase hex digits.
    pub hash: String,
    /// The IR blob, standard base64 with padding.
    pub ir_b64: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A version-2 file. Fields serialize in this order; [`Bundle::canonical`]
/// is compact JSON of this struct, so a bundle has one byte form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>,
    /// For people: our label of the record's game type.
    pub game_type: String,
    /// The record's game-engine index.
    pub engine: i32,
    /// The 0x130-byte record, lowercase hex.
    pub record_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<ScriptPart>,
}

/// A bundle that passed [`Bundle::integrity`] or [`Bundle::check`].
#[derive(Debug, Clone)]
pub struct Checked {
    pub variant: GameVariant,
    /// The script's IR blob and its decoded form.
    pub script: Option<(Vec<u8>, rules::Ir)>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortableError {
    #[error("not a game variant file: {0}")]
    Json(String),
    #[error("not a game variant file (format {0:?})")]
    Format(String),
    #[error("variant file version {0} is newer than this editor ({VERSION})")]
    Version(u32),
    #[error("the record is not {RECORD_SIZE:#x} bytes of hex")]
    Record,
    #[error("the file says engine {file}, the record {record}")]
    Engine { file: i32, record: i32 },
    #[error("the file's name is not the record's name")]
    NameMismatch,
    #[error("the name {0}")]
    Name(&'static str),
    #[error("the description {0}")]
    Description(&'static str),
    #[error("the game would not keep the record: {0}")]
    Refused(String),
    #[error("the record's extension block is not ours: {0}")]
    Block(ext::Why),
    #[error("the record names script mode {0}, which this build does not know")]
    Mode(u8),
    #[error("the record names a rules script, but the file carries none")]
    ScriptMissing,
    #[error("the file carries a script, but the record does not name one")]
    ScriptUnexpected,
    #[error("the script's IR is not base64")]
    ScriptEncoding,
    #[error("the script's {0} is too large")]
    ScriptTooLarge(&'static str),
    #[error("the script's IR does not hash to the file's hash")]
    ScriptHash,
    #[error("the record's script prefix does not start the script's hash")]
    ScriptPrefix,
    #[error("the script does not load: {0}")]
    ScriptRefused(String),
    /// Over the VM's per-tick budget or the IR cap (`rules::fits`).
    #[error("the script is too costly to run: {0}")]
    ScriptCost(String),
    #[error("the option values: {0}")]
    Options(String),
    #[error("the record is engine {record}; the script rides engine {script}")]
    Carrier { record: i32, script: u8 },
    #[error("engine {0} has no built-in script")]
    NoBuiltin(i32),
    #[error("the script rides engine {0}, which has no stock record")]
    NoStock(u8),
    /// A setting of the script's IR the record cannot take: past the
    /// table, on a carrier it does not apply to, or out of its range.
    #[error("the script's settings: {0}")]
    Setting(String),
    #[error("{0}")]
    Edit(String),
}

/// Writes a version-1 file: name, game type and record.
pub fn export(v: &GameVariant) -> String {
    let f = File1 {
        format: FORMAT.into(),
        version: VERSION_1,
        name: v.name(),
        game_type: v.game_type().label(),
        record: hex(v.bytes()),
    };
    serde_json::to_string_pretty(&f).expect("plain data serializes")
}

/// The variant in a portable file (version 1 or 2) and what the game's
/// clean-up would say about it (empty: the game takes it as is).
pub fn import(text: &str) -> Result<(GameVariant, Vec<record::Problem>), PortableError> {
    let b = read(text)?;
    let v = b.variant()?;
    let problems = record::refused(&v);
    Ok((v, problems))
}

/// Reads a file of either version. A version-2 file must pass
/// [`Bundle::integrity`]; a version-1 file becomes a bundle with no
/// description and no script, which [`Bundle::check`] refuses if its
/// record names a script.
pub fn read(text: &str) -> Result<Bundle, PortableError> {
    let p: Probe = serde_json::from_str(text).map_err(|e| PortableError::Json(e.to_string()))?;
    if p.format != FORMAT {
        return Err(PortableError::Format(p.format));
    }
    if p.version > VERSION {
        return Err(PortableError::Version(p.version));
    }
    if p.version <= VERSION_1 {
        let f: File1 =
            serde_json::from_str(text).map_err(|e| PortableError::Json(e.to_string()))?;
        let v = GameVariant::from_bytes(&unhex(&f.record)?).map_err(|_| PortableError::Record)?;
        return Ok(Bundle {
            format: FORMAT.into(),
            version: VERSION,
            name: v.name(),
            description: String::new(),
            author: None,
            game_type: v.game_type().label(),
            engine: v.game_type().engine(),
            record_hex: hex(v.bytes()),
            script: None,
        });
    }
    let b: Bundle = serde_json::from_str(text).map_err(|e| PortableError::Json(e.to_string()))?;
    b.integrity()?;
    Ok(b)
}

impl Bundle {
    /// A version-2 bundle of `v` named `name`. With a script, the record's
    /// block is set to mode 2 with the script's hash prefix and `options`
    /// (the script's defaults when None); without one, a record that
    /// names a script is refused. The result passes [`Bundle::check`].
    pub fn new(
        v: &GameVariant,
        name: &str,
        description: &str,
        script: Option<Attach>,
    ) -> Result<Bundle, PortableError> {
        check_name(name)?;
        check_description(description)?;
        let mut v = v.clone();
        v.set_name(name).map_err(|e| PortableError::Edit(e.to_string()))?;
        let script = match script {
            None => None,
            Some(a) => {
                if a.ir.len() > MAX_IR {
                    return Err(PortableError::ScriptTooLarge("IR"));
                }
                let (ir, _) = rules::verify::load(&a.ir)
                    .map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
                let id = Sha256::of(&a.ir);
                let mut e = v.ext();
                e.script = ext::Script {
                    mode: Mode::Rules,
                    prefix: id.0[..8].try_into().unwrap(),
                    options: a.options.unwrap_or_else(|| ir.default_options()),
                };
                v.set_ext(&e).map_err(|e| PortableError::Edit(e.to_string()))?;
                Some(ScriptPart {
                    name: ir.header.name.clone(),
                    hash: id.to_string(),
                    ir_b64: base64_encode(&a.ir),
                    source: a.source,
                })
            }
        };
        let b = Bundle {
            format: FORMAT.into(),
            version: VERSION,
            name: name.to_string(),
            description: description.to_string(),
            author: None,
            game_type: v.game_type().label(),
            engine: v.game_type().engine(),
            record_hex: hex(v.bytes()),
            script,
        };
        b.check()?;
        Ok(b)
    }

    /// The file's one byte form: compact JSON in field order.
    pub fn canonical(&self) -> String {
        serde_json::to_string(self).expect("plain data serializes")
    }

    /// The SHA-256 of [`Bundle::canonical`], lowercase hex.
    pub fn id(&self) -> String {
        Sha256::of(self.canonical().as_bytes()).to_string()
    }

    /// The record.
    pub fn variant(&self) -> Result<GameVariant, PortableError> {
        GameVariant::from_bytes(&unhex(&self.record_hex)?).map_err(|_| PortableError::Record)
    }

    /// The script's IR blob, if the file carries one.
    pub fn ir(&self) -> Result<Option<Vec<u8>>, PortableError> {
        let Some(s) = &self.script else {
            return Ok(None);
        };
        if s.ir_b64.len() > MAX_IR.div_ceil(3) * 4 {
            return Err(PortableError::ScriptTooLarge("IR"));
        }
        base64_decode(&s.ir_b64)
            .map(Some)
            .ok_or(PortableError::ScriptEncoding)
    }

    /// What every version-2 file must hold to: the record decodes, the
    /// engine and name fields match it, and a record that names a rules
    /// script comes with that script, whose IR hashes to its hash, starts
    /// with the record's prefix, loads (`rules::verify::load`) and rides
    /// the record's engine.
    pub fn integrity(&self) -> Result<Checked, PortableError> {
        let v = self.variant()?;
        let engine = v.game_type().engine();
        if self.engine != engine {
            return Err(PortableError::Engine {
                file: self.engine,
                record: engine,
            });
        }
        if self.name != v.name() {
            return Err(PortableError::NameMismatch);
        }
        let named = match v.ext_state() {
            Block::Ext(e) => match e.script.mode {
                Mode::Rules => Some(e.script.prefix),
                Mode::None | Mode::Firefight => None,
                Mode::Unknown(n) => return Err(PortableError::Mode(n)),
            },
            Block::None | Block::Ignored(_) => None,
        };
        let script = match (named, &self.script) {
            (None, None) => None,
            (Some(_), None) => return Err(PortableError::ScriptMissing),
            (None, Some(_)) => return Err(PortableError::ScriptUnexpected),
            (Some(prefix), Some(s)) => {
                if s.source.as_ref().is_some_and(|src| src.len() > MAX_SOURCE) {
                    return Err(PortableError::ScriptTooLarge("source"));
                }
                let ir = self.ir()?.expect("a script is present");
                if ir.len() > MAX_IR {
                    return Err(PortableError::ScriptTooLarge("IR"));
                }
                let id = Sha256::of(&ir);
                if id.to_string() != s.hash {
                    return Err(PortableError::ScriptHash);
                }
                if id.0[..8] != prefix {
                    return Err(PortableError::ScriptPrefix);
                }
                let (decoded, _) = rules::verify::load(&ir)
                    .map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
                if i32::from(decoded.header.carrier) != engine {
                    return Err(PortableError::Carrier {
                        record: engine,
                        script: decoded.header.carrier,
                    });
                }
                Some((ir, decoded))
            }
        };
        Ok(Checked { variant: v, script })
    }

    /// [`Bundle::integrity`], and what we publish: a name and description
    /// that keep the name rules, a record the game keeps as it is and our
    /// own rules allow ([`record::refused`]), an extension block that is
    /// ours or absent, and a script that fits the VM's per-tick budget and
    /// the IR cap (`rules::fits`) and whose settings make a record the
    /// game keeps ([`settings_record`]).
    pub fn check(&self) -> Result<Checked, PortableError> {
        let c = self.integrity()?;
        check_name(&self.name)?;
        check_description(&self.description)?;
        if let Some(p) = record::refused(&c.variant).into_iter().next() {
            return Err(PortableError::Refused(p.what));
        }
        if let Block::Ignored(why) = c.variant.ext_state() {
            return Err(PortableError::Block(why));
        }
        if let Some((_, ir)) = &c.script {
            let report = rules::verify(ir).map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
            rules::fits(ir, &report).map_err(|e| PortableError::ScriptCost(e.to_string()))?;
            // A script whose own record the game would not keep makes no
            // variant ([`settings_record`]).
            settings_record(ir)?;
        }
        Ok(c)
    }
}

/// A script to attach to a bundle: its IR blob, its source if it is to
/// travel too, and the packed option bits (None: the script's defaults).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attach {
    pub ir: Vec<u8>,
    pub source: Option<String>,
    pub options: Option<u32>,
}

impl Attach {
    /// Compiles and verifies `source`, refuses it over the budget
    /// (`rules::ship`), and attaches it with its IR.
    pub fn from_source(source: &str) -> Result<Attach, PortableError> {
        if source.len() > MAX_SOURCE {
            return Err(PortableError::ScriptTooLarge("source"));
        }
        let (ir, _) = rules::ship(source).map_err(|e| match e {
            e @ (rules::BuildError::OverBudget { .. } | rules::BuildError::TooLarge { .. }) => {
                PortableError::ScriptCost(e.to_string())
            }
            e => PortableError::ScriptRefused(e.to_string()),
        })?;
        Ok(Attach {
            ir: ir.encode(),
            source: Some(source.to_string()),
            options: None,
        })
    }
}

/// The name rules for a variant name (docs/game/game-options.md section
/// 4, "The design"): 1 to 31 UTF-16 units, the record's whole field, with
/// no leading or trailing space, no control character, no `|` (the text
/// scanner's escape), none of the scanner's alignment units
/// U+E405..U+E407 and nothing past U+FFFF.
pub fn check_name(name: &str) -> Result<(), PortableError> {
    if name.is_empty() {
        return Err(PortableError::Name("is empty"));
    }
    if name.encode_utf16().count() > record::MAX_NAME_UNITS {
        return Err(PortableError::Name("is longer than 31 characters"));
    }
    if name.starts_with(char::is_whitespace) || name.ends_with(char::is_whitespace) {
        return Err(PortableError::Name("starts or ends with a space"));
    }
    text_chars(name).map_err(PortableError::Name)
}

/// A description: at most [`MAX_DESCRIPTION_UNITS`] UTF-16 units, under
/// the name's character rules; it may be empty.
pub fn check_description(d: &str) -> Result<(), PortableError> {
    if d.encode_utf16().count() > MAX_DESCRIPTION_UNITS {
        return Err(PortableError::Description("is longer than 150 characters"));
    }
    text_chars(d).map_err(PortableError::Description)
}

fn text_chars(s: &str) -> Result<(), &'static str> {
    for c in s.chars() {
        if c.is_control() {
            return Err("has a control character");
        }
        if c == '|' {
            return Err("has a |");
        }
        if ('\u{e405}'..='\u{e407}').contains(&c) {
            return Err("has an alignment character");
        }
        if u32::from(c) > 0xffff {
            return Err("has a character the game cannot hold");
        }
    }
    Ok(())
}

// ---- the record of a script ----

/// The record of a script's own variant: the stock record of its carrier
/// ([`GameVariant::stock`]), the script's `settings` written through
/// [`GameVariant::set`] or into the `hx` block, the script's name, and the
/// script reference (mode 2, the first 8 bytes of [`rules::Ir::id`] and
/// the option bits `options` pack to, [`rules::Ir::pack_options`]; an
/// option left out takes its default). It refuses what the game would not
/// keep ([`record::refused`]) and a name the name rules refuse; nothing
/// falls back to another value. The one builder of a variant from a
/// script (docs/game/rules-variants.md, "The record of a script").
pub fn record_for(
    ir: &rules::Ir,
    options: &[(String, rules::ir::OptionValue)],
) -> Result<GameVariant, PortableError> {
    record_for_named(ir, &ir.header.name, options)
}

/// [`record_for`] with the variant named `name` in place of the script's
/// name: Live's `from-script` names the variant from its request, so a
/// script whose own name is longer than a record's 31 characters still
/// makes a variant there.
pub fn record_for_named(
    ir: &rules::Ir,
    name: &str,
    options: &[(String, rules::ir::OptionValue)],
) -> Result<GameVariant, PortableError> {
    let mut v = settings_record(ir)?;
    check_name(name)?;
    v.set_name(name)
        .map_err(|e| PortableError::Edit(e.to_string()))?;
    let bits = ir
        .pack_options(options)
        .map_err(|e| PortableError::Options(e.to_string()))?;
    let mut e = v.ext();
    e.script = ext::Script {
        mode: Mode::Rules,
        prefix: ir.id().0[..8].try_into().unwrap(),
        options: bits,
    };
    v.set_ext(&e)
        .map_err(|e| PortableError::Edit(e.to_string()))?;
    Ok(v)
}

/// The stock record of the script's carrier with the script's settings
/// written, refused as [`record_for`] refuses it; it keeps the stock name
/// and has no script reference. [`Bundle::check`] and `h2 rules check`
/// hold a script to it.
pub fn settings_record(ir: &rules::Ir) -> Result<GameVariant, PortableError> {
    let carrier = ir.header.carrier;
    let mut v = GameVariant::stock(GameType::from_engine(i32::from(carrier)))
        .ok_or(PortableError::NoStock(carrier))?;
    let mut e = ext::Ext::default();
    for s in &ir.settings {
        write_setting(&mut v, &mut e, carrier, *s)?;
    }
    v.set_ext(&e)
        .map_err(|e| PortableError::Edit(e.to_string()))?;
    if let Some(p) = record::refused(&v).into_iter().next() {
        return Err(PortableError::Refused(p.what));
    }
    Ok(v)
}

/// The carrier's name in a script (`slayer`), for messages.
fn carrier_word(carrier: u8) -> String {
    rules::caps::CARRIERS
        .iter()
        .find(|&&(_, e)| e == carrier)
        .map_or_else(|| format!("engine {carrier}"), |&(name, _)| name.to_string())
}

/// Writes one setting of a script into the record or the block.
fn write_setting(
    v: &mut GameVariant,
    e: &mut ext::Ext,
    carrier: u8,
    s: rules::ir::Setting,
) -> Result<(), PortableError> {
    use rules::settings::{ExtField, Store};
    let refuse = |what: String| Err(PortableError::Setting(what));
    let Some(row) = rules::settings::row_at(s.index) else {
        return refuse(format!("setting {} is past the table", s.index));
    };
    if !row.applies(carrier) {
        return refuse(format!(
            "{} is a {} setting; this script's carrier is {}",
            row.name,
            row.carrier_words(),
            carrier_word(carrier)
        ));
    }
    if !row.in_range(s.value) {
        let (lo, hi) = row.kind.range();
        return refuse(format!("{} takes {lo} to {hi}, not {}", row.name, s.value));
    }
    match row.store {
        Store::Ext(field) => {
            let percent = || {
                u16::try_from(s.value).map_err(|_| PortableError::Setting(row.name.to_string()))
            };
            match field {
                ExtField::Gravity => e.gravity = percent()?,
                ExtField::Damage => e.damage = percent()?,
                ExtField::Shield => e.shield = percent()?,
                ExtField::Health => e.health = percent()?,
                ExtField::UnlimitedGrenades => e.unlimited_grenades = s.value != 0,
            }
        }
        Store::Setting(_) | Store::Score | Store::Paired { .. } => {
            let Some(id) = row.setting(carrier) else {
                return refuse(format!("{} has no record setting on this carrier", row.name));
            };
            v.set(SettingId(id), s.value)
                .map_err(|e| PortableError::Setting(format!("{}: {e}", row.name)))?;
        }
    }
    Ok(())
}

/// The value of a [`rules::settings::TABLE`] row in a record of the
/// carrier with engine index `carrier`: the record setting the row stores
/// there, or its `hx` field. `None` for a row that does not apply.
pub fn setting_value(v: &GameVariant, row: &rules::settings::Row, carrier: u8) -> Option<i32> {
    use rules::settings::{ExtField, Store};
    if !row.applies(carrier) {
        return None;
    }
    match row.store {
        Store::Ext(field) => {
            let e = v.ext();
            Some(match field {
                ExtField::Gravity => i32::from(e.gravity),
                ExtField::Damage => i32::from(e.damage),
                ExtField::Shield => i32::from(e.shield),
                ExtField::Health => i32::from(e.health),
                ExtField::UnlimitedGrenades => i32::from(e.unlimited_grenades),
            })
        }
        Store::Setting(_) | Store::Score | Store::Paired { .. } => {
            v.get(SettingId(row.setting(carrier)?))
        }
    }
}

/// The bundle of a script's own variant: [`record_for`]'s record, the
/// script's name and `about` as the bundle's name and description, and
/// the script part (its IR, and `source` when it is to travel). The same
/// script and options give the same bundle, so the same [`Bundle::id`].
pub fn from_script(
    ir: &rules::Ir,
    source: Option<&str>,
    options: &[(String, rules::ir::OptionValue)],
) -> Result<Bundle, PortableError> {
    let v = record_for(ir, options)?;
    let attach = Attach {
        ir: ir.encode(),
        source: source.map(str::to_string),
        options: Some(v.ext().script.options),
    };
    Bundle::new(&v, &ir.header.name, &ir.header.about, Some(attach))
}

// ---- porting ----

/// What [`port`] does about the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortScript {
    /// Attach the built-in script of the record's engine
    /// (`rules::builtin::builtin`), compiled here, with its default
    /// options. The built-ins read the retail settings from the record by
    /// name, so nothing else is packed.
    Builtin,
    /// No script: a retail variant plays the retail rules. A record that
    /// names a script is refused (its blob is not at hand).
    Keep,
    /// This IR blob.
    Ir(Vec<u8>),
    /// This source, compiled and verified here; it travels with its IR.
    Source(String),
}

/// A variant as a version-2 bundle. `name` and `description` replace the
/// record's name and the empty description. The same inputs give the same
/// bundle, so the same [`Bundle::id`].
pub fn port(
    v: &GameVariant,
    name: Option<&str>,
    description: Option<&str>,
    script: PortScript,
) -> Result<Bundle, PortableError> {
    port_with_options(v, name, description, script, &[])
}

/// [`port`], with option values by name packed against the attached
/// script's options (`rules::Ir::pack_options`); an option left out takes
/// its default. Options with no script attached are refused.
pub fn port_with_options(
    v: &GameVariant,
    name: Option<&str>,
    description: Option<&str>,
    script: PortScript,
    options: &[(String, rules::ir::OptionValue)],
) -> Result<Bundle, PortableError> {
    let name = name.map_or_else(|| v.name(), str::to_string);
    let attach = match script {
        PortScript::Keep => None,
        PortScript::Builtin => {
            let engine = v.game_type().engine();
            let src = u8::try_from(engine)
                .ok()
                .and_then(rules::builtin::builtin)
                .ok_or(PortableError::NoBuiltin(engine))?;
            Some(Attach::from_source(src)?)
        }
        PortScript::Ir(ir) => Some(Attach {
            ir,
            source: None,
            options: None,
        }),
        PortScript::Source(src) => Some(Attach::from_source(&src)?),
    };
    let attach = match attach {
        None if !options.is_empty() => {
            return Err(PortableError::Options("no script is attached to take them".into()));
        }
        Some(mut a) if !options.is_empty() => {
            let (ir, _) = rules::verify::load(&a.ir)
                .map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
            let bits = ir
                .pack_options(options)
                .map_err(|e| PortableError::Options(e.to_string()))?;
            a.options = Some(bits);
            Some(a)
        }
        a => a,
    };
    Bundle::new(v, &name, description.unwrap_or(""), attach)
}

/// The records in a file to port: a raw 0x130-byte record, a saved-variant
/// file (record and signature, [`SAVE_FILE`] bytes or more), a hopper
/// record slot ([`HOPPER_SLOT`] bytes) or a `.h2variant` of either
/// version. Returns the record and the file's description, if it has one.
pub fn port_input(bytes: &[u8]) -> Result<(GameVariant, Option<String>), PortableError> {
    let first = bytes.iter().find(|b| !b.is_ascii_whitespace());
    if first == Some(&b'{') {
        let text = std::str::from_utf8(bytes).map_err(|e| PortableError::Json(e.to_string()))?;
        let b = read(text)?;
        let d = (!b.description.is_empty()).then(|| b.description.clone());
        return Ok((b.variant()?, d));
    }
    let record = match bytes.len() {
        RECORD_SIZE => bytes,
        HOPPER_SLOT => &bytes[HOPPER_SLOT_RECORD_AT..],
        n if n >= SAVE_FILE => &bytes[..RECORD_SIZE],
        _ => return Err(PortableError::Record),
    };
    let v = GameVariant::from_bytes(record).map_err(|_| PortableError::Record)?;
    Ok((v, None))
}

/// A file name for a bundle: the variant name with the characters file
/// systems refuse replaced, then `.h2variant`.
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let clean = clean.trim().trim_matches('.');
    let stem = if clean.is_empty() { "variant" } else { clean };
    format!("{stem}.{EXTENSION}")
}

/// The game type a bundle's record names.
pub fn game_type_of(b: &Bundle) -> GameType {
    GameType::from_engine(b.engine)
}

// ---- hex and base64 ----

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, PortableError> {
    let hex = s.trim();
    if hex.len() != 2 * RECORD_SIZE || !hex.is_ascii() {
        return Err(PortableError::Record);
    }
    (0..RECORD_SIZE)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| PortableError::Record)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let w = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= n {
                out.push(B64[(w >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64 with padding; None for anything else.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.len() % 4 != 0 {
        return None;
    }
    let val = |c: u8| B64.iter().position(|&x| x == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (k, q) in s.chunks(4).enumerate() {
        let last = k + 1 == s.len() / 4;
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut w = 0u32;
        for &c in &q[..4 - pad] {
            w = w << 6 | val(c)?;
        }
        w <<= 6 * pad as u32;
        let bytes = [(w >> 16) as u8, (w >> 8) as u8, w as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{GameType, SettingId};

    #[test]
    fn round_trip_keeps_every_byte() {
        let mut v = GameVariant::blank(GameType::King);
        v.set_name("Hill \u{e006}").unwrap();
        v.set(SettingId(40), 60).unwrap();
        let mut raw = v.to_bytes();
        raw[0x10c] = 0xab; // a byte nobody models
        let v = GameVariant::from_bytes(&raw).unwrap();
        let text = export(&v);
        assert!(text.contains("\"King of the Hill\""));
        let (back, problems) = import(&text).unwrap();
        assert_eq!(back.to_bytes(), raw);
        assert!(problems.is_empty());
    }

    #[test]
    fn the_extension_block_travels_in_a_portable_file() {
        let mut v = GameVariant::blank(GameType::Slayer);
        let e = crate::ext::Ext {
            gravity: 50,
            speed: 150,
            ..Default::default()
        };
        v.set_ext(&e).unwrap();
        let (back, problems) = import(&export(&v)).unwrap();
        assert!(problems.is_empty());
        assert_eq!(back.ext(), e);
        assert_eq!(back, v);
    }

    #[test]
    fn bad_files_are_refused() {
        let good = export(&GameVariant::blank(GameType::Slayer));
        assert!(matches!(import("{"), Err(PortableError::Json(_))));
        assert!(matches!(
            import(&good.replace(FORMAT, "other")),
            Err(PortableError::Format(_))
        ));
        assert_eq!(
            import(&good.replace("\"version\": 1", "\"version\": 9")).unwrap_err(),
            PortableError::Version(9)
        );
        assert_eq!(
            import(&good.replace("\"record\": \"00", "\"record\": \"zz")).unwrap_err(),
            PortableError::Record
        );
        assert_eq!(
            import(&good.replace("\"record\": \"00", "\"record\": \"")).unwrap_err(),
            PortableError::Record
        );
        // A record the game would clean up imports with its problems listed.
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set(SettingId(19), 5).unwrap();
        assert_eq!(import(&export(&v)).unwrap().1.len(), 1);
    }

    /// A record the clean-up keeps: King of the Hill needs nothing in its
    /// type area.
    fn king() -> GameVariant {
        let mut v = GameVariant::blank(GameType::King);
        v.set_name("Hill").unwrap();
        v
    }

    fn slayer() -> GameVariant {
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set_name("Fists").unwrap();
        v
    }

    fn slayer_ir() -> Vec<u8> {
        rules::compile(rules::builtin::SLAYER).unwrap().encode()
    }

    #[test]
    fn base64_round_trips() {
        for n in 0..10 {
            let b: Vec<u8> = (0..n).map(|i| (i * 37 + 1) as u8).collect();
            assert_eq!(base64_decode(&base64_encode(&b)).unwrap(), b);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        assert!(base64_decode("TQ=").is_none());
        assert!(base64_decode("T===").is_none());
        assert!(base64_decode("TQ==TWFu").is_none());
        assert!(base64_decode("TW!u").is_none());
    }

    #[test]
    fn a_scripted_bundle_carries_its_script_whole() {
        let b = port(&slayer(), None, Some("Kills count"), PortScript::Builtin).unwrap();
        assert_eq!(b.version, VERSION);
        assert_eq!(b.engine, 2);
        assert_eq!(b.game_type, "Slayer");
        let s = b.script.as_ref().unwrap();
        assert_eq!(s.name, "Slayer");
        assert_eq!(s.source.as_deref(), Some(rules::builtin::SLAYER));
        let ir = slayer_ir();
        assert_eq!(s.hash, Sha256::of(&ir).to_string());
        assert_eq!(b.ir().unwrap().unwrap(), ir);
        let e = b.variant().unwrap().ext();
        assert_eq!(e.script.mode, Mode::Rules);
        assert_eq!(e.script.prefix, Sha256::of(&ir).0[..8]);
        let decoded = rules::Ir::decode(&ir).unwrap();
        assert_eq!(e.script.options, decoded.default_options());

        // The canonical bytes read back to the same bundle and the same id.
        let back = read(&b.canonical()).unwrap();
        assert_eq!(back, b);
        assert_eq!(back.id(), b.id());
        // Pretty JSON is the same file.
        let pretty = serde_json::to_string_pretty(&b).unwrap();
        assert_eq!(read(&pretty).unwrap().id(), b.id());
        assert!(b.check().unwrap().script.is_some());
    }

    #[test]
    fn porting_is_deterministic() {
        for script in [
            PortScript::Builtin,
            PortScript::Keep,
            PortScript::Ir(slayer_ir()),
            PortScript::Source(rules::builtin::SLAYER.to_string()),
        ] {
            let a = port(&slayer(), None, None, script.clone()).unwrap();
            let b = port(&slayer(), None, None, script).unwrap();
            assert_eq!(a.canonical(), b.canonical());
            assert_eq!(a.id(), b.id());
        }
        // Keep leaves the record as it was and carries no script.
        let k = port(&slayer(), Some("Plain"), None, PortScript::Keep).unwrap();
        assert!(k.script.is_none());
        assert_eq!(k.name, "Plain");
        assert_eq!(k.variant().unwrap().ext_state(), Block::None);
        // An IR alone travels without source.
        let i = port(&slayer(), None, None, PortScript::Ir(slayer_ir())).unwrap();
        assert!(i.script.as_ref().unwrap().source.is_none());
        // A different script, a different id.
        assert_ne!(k.id(), i.id());
    }

    #[test]
    fn port_refusals() {
        // A script that rides another engine.
        assert!(matches!(
            port(&king(), None, None, PortScript::Ir(slayer_ir())),
            Err(PortableError::Carrier { record: 4, script: 2 })
        ));
        assert!(matches!(
            port(&king(), None, None, PortScript::Source("rules".into())),
            Err(PortableError::ScriptRefused(_))
        ));
        assert!(matches!(
            port(&king(), None, None, PortScript::Ir(vec![1, 2, 3])),
            Err(PortableError::ScriptRefused(_))
        ));
        let mut other = GameVariant::blank(GameType::Other(5));
        other.set_name("Race").unwrap();
        assert_eq!(
            port(&other, None, None, PortScript::Builtin).unwrap_err(),
            PortableError::NoBuiltin(5)
        );
        assert!(matches!(
            port(&other, None, None, PortScript::Keep),
            Err(PortableError::Refused(_))
        ));
        // A record that names a script cannot be kept without it.
        let scripted = port(&slayer(), None, None, PortScript::Builtin).unwrap();
        let v = scripted.variant().unwrap();
        assert_eq!(
            port(&v, None, None, PortScript::Keep).unwrap_err(),
            PortableError::ScriptMissing
        );
        // ... but can be given its script again.
        assert_eq!(
            port(&v, None, None, PortScript::Builtin).unwrap().id(),
            scripted.id()
        );
        for (name, want) in [
            ("", "is empty"),
            (" Lead", "starts or ends with a space"),
            ("a|b", "has a |"),
            ("tab\there", "has a control character"),
            ("x\u{e406}", "has an alignment character"),
            ("\u{1f600}", "has a character the game cannot hold"),
        ] {
            assert_eq!(
                port(&slayer(), Some(name), None, PortScript::Keep).unwrap_err(),
                PortableError::Name(want),
                "{name:?}"
            );
        }
        assert!(matches!(
            port(&slayer(), Some(&"n".repeat(32)), None, PortScript::Keep),
            Err(PortableError::Name(_))
        ));
        assert!(port(&slayer(), Some("Wheeeeee!"), None, PortScript::Keep).is_ok());
        assert!(matches!(
            port(&slayer(), None, Some(&"d".repeat(151)), PortScript::Keep),
            Err(PortableError::Description(_))
        ));
    }

    #[test]
    fn options_pack_by_name_against_the_attached_script() {
        use rules::ir::OptionValue;
        let given = vec![
            ("kill_points".to_string(), OptionValue::Int(3)),
            ("suicide_costs".to_string(), OptionValue::Bool(false)),
        ];
        let b = port_with_options(&slayer(), Some("Triple"), Some("Three a kill"), PortScript::Builtin, &given)
            .unwrap();
        assert_eq!(b.name, "Triple");
        assert_eq!(b.description, "Three a kill");
        let ir = rules::Ir::decode(&slayer_ir()).unwrap();
        let options = b.variant().unwrap().ext().script.options;
        assert_eq!(options, ir.pack_options(&given).unwrap());
        assert_ne!(options, ir.default_options());
        assert!(b.check().is_ok());

        let refused = |given: Vec<(String, OptionValue)>, script| {
            port_with_options(&slayer(), None, None, script, &given).unwrap_err()
        };
        assert_eq!(
            refused(vec![("kill_points".into(), OptionValue::Int(9))], PortScript::Builtin),
            PortableError::Options("option \"kill_points\" takes 1..=5".into())
        );
        assert_eq!(
            refused(vec![("nope".into(), OptionValue::Int(1))], PortScript::Builtin),
            PortableError::Options("the script has no option \"nope\"".into())
        );
        assert!(matches!(
            refused(vec![("kill_points".into(), OptionValue::Int(2))], PortScript::Keep),
            PortableError::Options(_)
        ));
    }

    #[test]
    fn a_script_over_the_budget_is_not_published() {
        let heavy = "rules \"Heavy\" 1\ncarrier slayer\nglobal var n: int = 0\n\
                     on kill(v, k) { for each players a { for each players b { for each players c { n = n + 1 } } } }\n";
        // From source or as a blob: a bundle passes `check`, which refuses it.
        assert!(matches!(
            port(&slayer(), None, None, PortScript::Source(heavy.into())),
            Err(PortableError::ScriptCost(_))
        ));
        let ir = rules::compile(heavy).unwrap().encode();
        assert!(matches!(
            port(&slayer(), None, None, PortScript::Ir(ir)),
            Err(PortableError::ScriptCost(_))
        ));
    }

    #[test]
    fn version_2_refusals() {
        let good = port(&slayer(), None, None, PortScript::Builtin).unwrap();
        let edit = |f: &dyn Fn(&mut Bundle)| {
            let mut b = good.clone();
            f(&mut b);
            read(&b.canonical()).unwrap_err()
        };
        assert_eq!(edit(&|b: &mut Bundle| b.engine = 4), PortableError::Engine { file: 4, record: 2 });
        assert_eq!(edit(&|b: &mut Bundle| b.name = "Other".into()), PortableError::NameMismatch);
        assert_eq!(edit(&|b: &mut Bundle| b.script = None), PortableError::ScriptMissing);
        assert_eq!(
            edit(&|b: &mut Bundle| b.script.as_mut().unwrap().ir_b64 = "!!!!".into()),
            PortableError::ScriptEncoding
        );
        assert_eq!(
            edit(&|b: &mut Bundle| b.script.as_mut().unwrap().hash = "00".repeat(32)),
            PortableError::ScriptHash
        );
        // Another script whose hash is right but not the record's prefix.
        let gun = rules::compile(rules::builtin::GUN_GAME).unwrap().encode();
        assert_eq!(
            edit(&|b: &mut Bundle| {
                let s = b.script.as_mut().unwrap();
                s.ir_b64 = base64_encode(&gun);
                s.hash = Sha256::of(&gun).to_string();
            }),
            PortableError::ScriptPrefix
        );
        let plain = port(&slayer(), None, None, PortScript::Keep).unwrap();
        let mut b = plain.clone();
        b.script = good.script.clone();
        assert_eq!(read(&b.canonical()).unwrap_err(), PortableError::ScriptUnexpected);
        let mut b = good.clone();
        b.version = 3;
        assert_eq!(read(&b.canonical()).unwrap_err(), PortableError::Version(3));
        // Integrity holds but publishing refuses a record the game cleans up.
        let mut v = slayer();
        v.set(SettingId(19), 5).unwrap();
        let mut b = plain.clone();
        b.record_hex = hex(v.bytes());
        assert!(read(&b.canonical()).is_ok());
        assert!(matches!(b.check(), Err(PortableError::Refused(_))));
    }

    #[test]
    fn format_1_reads_as_a_bundle() {
        let b = read(&export(&king())).unwrap();
        assert_eq!(b.version, VERSION);
        assert_eq!(b.name, "Hill");
        assert_eq!(b.engine, 4);
        assert!(b.script.is_none());
        assert!(b.check().is_ok());
        // A version-1 file of a scripted record has no blob to carry.
        let scripted = port(&slayer(), None, None, PortScript::Builtin).unwrap();
        let v1 = export(&scripted.variant().unwrap());
        let b = read(&v1).unwrap();
        assert_eq!(b.check().unwrap_err(), PortableError::ScriptMissing);
        assert_eq!(
            port(&port_input(v1.as_bytes()).unwrap().0, None, None, PortScript::Builtin)
                .unwrap()
                .id(),
            scripted.id()
        );
    }

    #[test]
    fn port_inputs() {
        let v = king();
        let raw = v.to_bytes();
        assert_eq!(port_input(&raw).unwrap().0, v);
        let save = crate::save::encode_file(&v, &crate::save::RUNTIME_KEY);
        assert_eq!(port_input(&save).unwrap().0, v);
        let mut slot = vec![0xeeu8; HOPPER_SLOT_RECORD_AT];
        slot.extend_from_slice(&raw);
        assert_eq!(slot.len(), HOPPER_SLOT);
        assert_eq!(port_input(&slot).unwrap().0, v);
        let b = port(&v, None, Some("Up the hill"), PortScript::Builtin).unwrap();
        let (back, d) = port_input(b.canonical().as_bytes()).unwrap();
        assert_eq!(d.as_deref(), Some("Up the hill"));
        assert_eq!(back, b.variant().unwrap());
        assert_eq!(port_input(&raw[..10]).unwrap_err(), PortableError::Record);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Team Slayer"), "Team Slayer.h2variant");
        assert_eq!(file_name("a/b:c"), "a_b_c.h2variant");
        assert_eq!(file_name(".."), "variant.h2variant");
    }

    // ---- the record of a script ----

    const SWAT: &str = "rules \"SWAT\" 1
carrier slayer
scoring carrier
about \"No shields, no motion sensor, battle rifles and magnums.\"
quick score_limit, starting_weapon, secondary_weapon

settings {
    team_play = true
    score_limit = 50
    shields = none
    motion_sensor = false
    starting_weapon = battle_rifle
    secondary_weapon = magnum
    weapons_on_map = none
    grenades_on_map = false
    gravity = 50
    unlimited_grenades = true
}
";

    fn compiled(src: &str) -> rules::Ir {
        rules::compile(src).unwrap_or_else(|e| panic!("{e}"))
    }

    /// A settings-only script of `carrier` named V.
    fn carried(carrier: &str, lines: &str) -> String {
        format!("rules \"V\" 1\ncarrier {carrier}\nscoring carrier\nsettings {{\n{lines}}}\n")
    }

    #[test]
    fn a_named_record_takes_its_name_not_the_scripts() {
        let long = "A script name far longer than any record's thirty-one";
        let src = format!("rules \"{long}\" 1\ncarrier slayer\nscoring carrier\nsettings {{\nscore_limit = 10\n}}\n");
        let ir = compiled(&src);
        assert!(record_for(&ir, &[]).is_err(), "the script's own name is too long");
        let v = record_for_named(&ir, "Short", &[]).unwrap();
        assert_eq!(v.name(), "Short");
        assert_eq!(v.ext().script.prefix[..], ir.id().0[..8]);
        assert!(record_for_named(&ir, "", &[]).is_err());
    }

    #[test]
    fn record_for_is_stock_plus_the_scripts_settings() {
        let ir = compiled(SWAT);
        let got = record_for(&ir, &[]).unwrap();

        let mut want = GameVariant::stock(GameType::Slayer).unwrap();
        for (s, value) in [
            (91, 1),
            (2, 50),
            (19, 1),
            (20, 0),
            (89, 3),
            (94, 4),
            (83, 1),
            (86, 0),
        ] {
            want.set(SettingId(s), value).unwrap();
        }
        want.set_name("SWAT").unwrap();
        want.set_ext(&ext::Ext {
            gravity: 50,
            unlimited_grenades: true,
            script: ext::Script {
                mode: Mode::Rules,
                prefix: ir.id().0[..8].try_into().unwrap(),
                options: ir.default_options(),
            },
            ..ext::Ext::default()
        })
        .unwrap();
        assert_eq!(got, want);
        assert!(record::refused(&got).is_empty());

        // A setting the script leaves out has the stock value.
        assert_eq!(got.get(SettingId(10)), Some(480));
        assert_eq!(got.get(SettingId(17)), Some(5));

        // The carrier's own score setting, and the pair CTF and Assault
        // share by name.
        let jugg = compiled(&carried("juggernaut", "score_limit = 10\njuggernaut_movement = slow\n"));
        let v = record_for(&jugg, &[]).unwrap();
        assert_eq!((v.get(SettingId(7)), v.get(SettingId(96))), (Some(10), Some(0)));
        for (carrier, ty, flag_type) in [("ctf", GameType::Ctf, 50), ("assault", GameType::Assault, 59)] {
            let ir = compiled(&carried(carrier, "ctf_flag_type = 1\nctf_reset_time = 15s\n"));
            let v = record_for(&ir, &[]).unwrap();
            assert_eq!(v.game_type(), ty);
            assert_eq!(v.get(SettingId(flag_type)), Some(1), "{carrier}");
            assert_eq!(v.get(ty.score_setting().unwrap()), Some(3), "{carrier}: stock score");
            assert!(v.team_play(), "{carrier}");
        }

        // A script with no settings is the stock record, named, with its
        // reference; given options pack into the block.
        let slayer = compiled(rules::builtin::SLAYER);
        let given = vec![("kill_points".to_string(), rules::ir::OptionValue::Int(3))];
        let v = record_for(&slayer, &given).unwrap();
        let mut stock = GameVariant::stock(GameType::Slayer).unwrap();
        stock.set_name(&slayer.header.name).unwrap();
        assert_eq!(v.bytes()[..ext::AT], stock.bytes()[..ext::AT]);
        assert_eq!(v.ext().script.options, slayer.pack_options(&given).unwrap());
        assert_eq!(v.ext().gravity, 100);
    }

    #[test]
    fn record_for_refuses_what_the_game_would_not_keep() {
        let refused = |src: &str| record_for(&compiled(src), &[]).unwrap_err();
        assert_eq!(
            refused(&carried("juggernaut", "team_play = true\n")),
            PortableError::Refused("Juggernaut is never team play".into())
        );
        assert_eq!(
            refused(&carried("ctf", "team_play = false\n")),
            PortableError::Refused("this game type is always team play".into())
        );
        // `Bundle::check` holds a carried script to the same rule.
        let jugg = compiled(&carried("juggernaut", "team_play = true\n"));
        let mut v = GameVariant::stock(GameType::Juggernaut).unwrap();
        v.set_name("Jugg").unwrap();
        let attach = Attach {
            ir: jugg.encode(),
            source: None,
            options: None,
        };
        assert_eq!(
            Bundle::new(&v, "Jugg", "", Some(attach)).unwrap_err(),
            PortableError::Refused("Juggernaut is never team play".into())
        );

        // A blob made by hand: what the compiler and `verify::load` refuse,
        // record_for refuses too, with no value put in its place.
        let base = compiled(&carried("slayer", "lives = 3\n"));
        let edited = |f: &dyn Fn(&mut rules::Ir)| {
            let mut ir = base.clone();
            f(&mut ir);
            record_for(&ir, &[]).unwrap_err()
        };
        let at = |name| rules::settings::named(name).unwrap().0;
        assert!(matches!(
            edited(&|ir: &mut rules::Ir| ir.settings[0].index = rules::settings::TABLE.len() as u16),
            PortableError::Setting(_)
        ));
        assert_eq!(
            edited(&|ir: &mut rules::Ir| ir.settings[0].index = at("juggernaut_movement")),
            PortableError::Setting(
                "juggernaut_movement is a Juggernaut setting; this script's carrier is slayer".into()
            )
        );
        assert_eq!(
            edited(&|ir: &mut rules::Ir| {
                ir.settings[0] = rules::ir::Setting {
                    index: at("shields"),
                    value: 3,
                }
            }),
            PortableError::Setting("shields takes 0 to 2, not 3".into())
        );
        assert!(matches!(
            edited(&|ir: &mut rules::Ir| {
                ir.settings[0] = rules::ir::Setting {
                    index: at("gravity"),
                    value: 5,
                }
            }),
            PortableError::Setting(_)
        ));
        assert_eq!(edited(&|ir: &mut rules::Ir| ir.header.carrier = 5), PortableError::NoStock(5));
        assert_eq!(
            edited(&|ir: &mut rules::Ir| ir.header.name = "a|b".into()),
            PortableError::Name("has a |")
        );
        assert!(matches!(
            edited(&|ir: &mut rules::Ir| ir.header.name = "n".repeat(32)),
            PortableError::Name(_)
        ));
        assert!(matches!(
            record_for(
                &base,
                &[("nope".to_string(), rules::ir::OptionValue::Int(1))]
            ),
            Err(PortableError::Options(_))
        ));
    }

    #[test]
    fn from_script_is_the_same_bundle_twice() {
        let ir = compiled(SWAT);
        let a = from_script(&ir, Some(SWAT), &[]).unwrap();
        let b = from_script(&ir, Some(SWAT), &[]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.canonical(), b.canonical());
        assert_eq!(a.id(), b.id());
        assert_eq!(a.name, "SWAT");
        assert_eq!(a.description, ir.header.about);
        assert_eq!(a.engine, 2);
        assert_eq!(a.variant().unwrap(), record_for(&ir, &[]).unwrap());
        let s = a.script.as_ref().unwrap();
        assert_eq!(s.name, "SWAT");
        assert_eq!(s.hash, ir.id().to_string());
        assert_eq!(s.source.as_deref(), Some(SWAT));
        assert_eq!(a.ir().unwrap().unwrap(), ir.encode());
        assert!(a.check().is_ok());
        // It reads back as the same file.
        assert_eq!(read(&a.canonical()).unwrap().id(), a.id());
        // Without its source, another file; with options, another record.
        let bare = from_script(&ir, None, &[]).unwrap();
        assert!(bare.script.as_ref().unwrap().source.is_none());
        assert_ne!(bare.id(), a.id());
        assert_eq!(bare.variant().unwrap(), a.variant().unwrap());
        let slayer = compiled(rules::builtin::SLAYER);
        let given = [("kill_points".to_string(), rules::ir::OptionValue::Int(3))];
        let plain = from_script(&slayer, None, &[]).unwrap();
        let three = from_script(&slayer, None, &given).unwrap();
        assert_ne!(plain.id(), three.id());
        assert_eq!(three.variant().unwrap(), record_for(&slayer, &given).unwrap());
    }

    #[test]
    fn setting_value_reads_the_record_and_the_block() {
        let ir = compiled(SWAT);
        let v = record_for(&ir, &[]).unwrap();
        let row = |name| rules::settings::named(name).unwrap().1;
        assert_eq!(setting_value(&v, row("score_limit"), 2), Some(50));
        assert_eq!(setting_value(&v, row("gravity"), 2), Some(50));
        assert_eq!(setting_value(&v, row("unlimited_grenades"), 2), Some(1));
        assert_eq!(setting_value(&v, row("damage"), 2), Some(100));
        assert_eq!(setting_value(&v, row("time_limit"), 2), Some(480));
        assert_eq!(setting_value(&v, row("juggernaut_movement"), 2), None);
    }
}
