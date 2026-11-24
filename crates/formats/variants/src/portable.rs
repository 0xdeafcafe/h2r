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
//! the script index for Slayer scripts with only common settings, otherwise
//! its retail engine, with the script's `settings`, its name and the reference to
//! it. [`from_script`] makes its bundle (docs/game/rules-variants.md, "The
//! record of a script").

use std::collections::BTreeMap;

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

/// The most map variants a game variant suggests.
pub const MAX_MAPS: usize = 8;
/// A suggestion's name, UTF-16 units.
pub const MAX_MAP_NAME_UNITS: usize = 63;

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
    /// The files `source` names with `use` (`rules::lang::modules`), by
    /// name: a script of several files travels as all of them. Only with a
    /// `source`, and with it at most [`MAX_SOURCE`] bytes in all.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modules: BTreeMap<String, String>,
}

/// A map variant the game variant's author made it for
/// (docs/game/forge-map-variants.md, "Who picks"): the base map's
/// SHA-256, the map variant's document id and its name, for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapSuggestion {
    /// The base map's SHA-256, 64 lowercase hex digits.
    pub base: String,
    /// The map variant's document id, 64 lowercase hex digits.
    pub variant: String,
    pub name: String,
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
    /// For people: the script header's type label, or the record's when unscripted.
    pub game_type: String,
    /// The record's game-engine index.
    pub engine: i32,
    /// The 0x130-byte record, lowercase hex.
    pub record_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<ScriptPart>,
    /// Map variants it suggests, at most [`MAX_MAPS`]. A bundle with none
    /// has the bytes it had before the field existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub maps: Vec<MapSuggestion>,
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
    #[error("the record requires a rules script, but the file carries none")]
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
    #[error("the file suggests {0} map variants; at most {MAX_MAPS}")]
    MapsTooMany(usize),
    #[error("a suggested map variant {0}")]
    MapSuggestion(&'static str),
    #[error("the option values: {0}")]
    Options(String),
    #[error("the record is engine {record}; the script's record is engine {script}")]
    Carrier { record: i32, script: u8 },
    #[error("engine {0} has no built-in script")]
    NoBuiltin(i32),
    #[error("the script's record is engine {0}, which has no stock record")]
    NoStock(u8),
    /// A setting of the script's IR the record cannot take: past the
    /// table, on a carrier it does not apply to, or out of its range.
    #[error("the script's settings: {0}")]
    Setting(String),
    #[error("{0}")]
    Edit(String),
    /// An `hx` extra no script setting sets (weapon ids, team size), beside a script.
    #[error("the record's extension block has {0}, which no script setting sets")]
    Extras(&'static str),
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
            maps: Vec::new(),
        });
    }
    let b: Bundle = serde_json::from_str(text).map_err(|e| PortableError::Json(e.to_string()))?;
    b.integrity()?;
    Ok(b)
}

impl Bundle {
    /// A version-2 bundle of `v` named `name`. With a script, the record's
    /// block is set to mode 2 with the script's hash prefix and `options`
    /// (the script's defaults when None), and its extras are the ones the
    /// script's `settings` set, whatever `v`'s were; without one, a record
    /// that names a script is refused. The result passes [`Bundle::check`].
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
        let mut game_type = v.game_type().label();
        let script = match script {
            None => None,
            Some(a) => {
                if a.ir.len() > MAX_IR {
                    return Err(PortableError::ScriptTooLarge("IR"));
                }
                let (ir, _) = rules::verify::load(&a.ir)
                    .map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
                let id = Sha256::of(&a.ir);
                // The script's settings are the only source of the block's
                // extras: the input record's own are not kept.
                game_type = ir.header.game_type.row().label.to_owned();
                let mut e = attached_settings(&ir, &v)?.ext();
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
                    modules: BTreeMap::new(),
                })
            }
        };
        let b = Bundle {
            format: FORMAT.into(),
            version: VERSION,
            name: name.to_string(),
            description: description.to_string(),
            author: None,
            game_type,
            engine: v.game_type().engine(),
            record_hex: hex(v.bytes()),
            script,
            maps: Vec::new(),
        };
        b.check()?;
        Ok(b)
    }

    /// The bundle with the files its script's source names, `modules` by
    /// name (see [`Attach::from_files`]). Refused without a script that
    /// carries its source, or for a file that is not a module name, more
    /// than the language's file count or [`MAX_SOURCE`] bytes in all.
    pub fn with_modules(mut self, modules: BTreeMap<String, String>) -> Result<Bundle, PortableError> {
        match &mut self.script {
            Some(s) if s.source.is_some() => s.modules = modules,
            _ if modules.is_empty() => {}
            _ => return Err(PortableError::ScriptUnexpected),
        }
        self.check()?;
        Ok(self)
    }

    /// The bundle suggesting `maps`: refused for more than [`MAX_MAPS`], a
    /// hash that is not 64 lowercase hex digits, an empty or over-long
    /// name, or the same map variant twice.
    pub fn with_maps(mut self, maps: Vec<MapSuggestion>) -> Result<Bundle, PortableError> {
        self.maps = maps;
        check_maps(&self.maps)?;
        self.check()?;
        Ok(self)
    }

    /// The suggestions for the base map whose SHA-256 is `base`.
    pub fn suggested_for<'a>(
        &'a self,
        base: &'a Sha256,
    ) -> impl Iterator<Item = &'a MapSuggestion> + 'a {
        let base = base.to_string();
        self.maps.iter().filter(move |m| m.base == base)
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
    /// the record's engine or the script index.
    pub fn integrity(&self) -> Result<Checked, PortableError> {
        check_maps(&self.maps)?;
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
            (None, None) if engine == record::SCRIPT_INDEX => {
                return Err(PortableError::ScriptMissing);
            }
            (None, None) => None,
            (Some(_), None) => return Err(PortableError::ScriptMissing),
            (None, Some(_)) => return Err(PortableError::ScriptUnexpected),
            (Some(prefix), Some(s)) => {
                if s.source.as_ref().is_some_and(|src| src.len() > MAX_SOURCE) {
                    return Err(PortableError::ScriptTooLarge("source"));
                }
                check_modules(s)?;
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
                if engine != record::SCRIPT_INDEX && i32::from(decoded.header.engine()) != engine {
                    return Err(PortableError::Carrier {
                        record: engine,
                        script: decoded.header.engine(),
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
            // The block's extras are the script's settings and nothing else.
            if let Some(what) = non_setting_extra(&c.variant.ext()) {
                return Err(PortableError::Extras(what));
            }
            // A script whose own record the game would not keep makes no
            // variant ([`settings_record`]).
            attached_settings(ir, &c.variant)?;
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

/// A bundle's suggestions: at most [`MAX_MAPS`], each with two hashes and
/// a name, none twice.
fn check_maps(maps: &[MapSuggestion]) -> Result<(), PortableError> {
    if maps.len() > MAX_MAPS {
        return Err(PortableError::MapsTooMany(maps.len()));
    }
    let hash = |h: &str| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    for (i, m) in maps.iter().enumerate() {
        if !hash(&m.base) {
            return Err(PortableError::MapSuggestion("has a base that is not a SHA-256"));
        }
        if !hash(&m.variant) {
            return Err(PortableError::MapSuggestion("has an id that is not a SHA-256"));
        }
        if m.name.is_empty() {
            return Err(PortableError::MapSuggestion("has no name"));
        }
        if m.name.encode_utf16().count() > MAX_MAP_NAME_UNITS {
            return Err(PortableError::MapSuggestion("has a name over 63 characters"));
        }
        text_chars(&m.name).map_err(PortableError::MapSuggestion)?;
        if maps[..i].iter().any(|o| o.base == m.base && o.variant == m.variant) {
            return Err(PortableError::MapSuggestion("is listed twice"));
        }
    }
    Ok(())
}

/// The files a bundle's script source names: each a module name, no more
/// than the language allows beside the root, and the source and the files
/// together within [`MAX_SOURCE`]. None without a source.
fn check_modules(s: &ScriptPart) -> Result<(), PortableError> {
    if s.modules.is_empty() {
        return Ok(());
    }
    let Some(src) = &s.source else {
        return Err(PortableError::ScriptUnexpected);
    };
    if s.modules.len() >= rules::lang::modules::MAX_FILES {
        return Err(PortableError::ScriptTooLarge("files"));
    }
    if let Some(bad) = s.modules.keys().find(|n| !rules::lang::modules::is_module_name(n)) {
        return Err(PortableError::ScriptRefused(format!("`{bad}` is not a module name")));
    }
    let total = src.len() + s.modules.values().map(String::len).sum::<usize>();
    if total > MAX_SOURCE {
        return Err(PortableError::ScriptTooLarge("source"));
    }
    Ok(())
}

impl Attach {
    /// [`Attach::from_source`] for a script of several files: `source` is
    /// the root and `modules` the files its `use` lines name, by name.
    /// The IR is that of the files read as one text; [`Bundle::with_modules`]
    /// puts the files in the bundle beside the source.
    pub fn from_files(source: &str, modules: &BTreeMap<String, String>) -> Result<Attach, PortableError> {
        let total = source.len() + modules.values().map(String::len).sum::<usize>();
        if total > MAX_SOURCE {
            return Err(PortableError::ScriptTooLarge("source"));
        }
        let read = |name: &str| modules.get(name).cloned();
        let (ir, _) = rules::ship_files("script.rules", source, &read).map_err(|e| match e {
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

/// The record of a script's own variant: its stock record ([`settings_record`]),
/// the script's `settings` written through
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

/// The stock record with the script's settings, before its name and
/// reference are attached. Scripts that ride Slayer and write no setting
/// in its type area use the script index, whose stock type area is zero.
pub fn settings_record(ir: &rules::Ir) -> Result<GameVariant, PortableError> {
    let carrier = ir.header.engine();
    let ty = GameType::from_engine(i32::from(carrier));
    let stock = if ty == GameType::Slayer && !writes_type_area(ir, carrier) {
        GameType::Script
    } else {
        ty
    };
    settings_on(ir, stock, carrier)
}

/// Test the field a setting writes, including a write of zero. The stock
/// Slayer record has nonzero type defaults even when a script writes none.
fn writes_type_area(ir: &rules::Ir, carrier: u8) -> bool {
    ir.settings.iter()
        .filter_map(|s| rules::settings::row_at(s.index)?.setting(carrier))
        .filter_map(|id| record::field(SettingId(id)))
        .any(|field| record::TYPE_AREA.contains(&field.offset))
}

/// Validate an attachment on its existing record. A script-index record
/// accepts only common settings and extension fields.
fn attached_settings(ir: &rules::Ir, v: &GameVariant) -> Result<GameVariant, PortableError> {
    let engine = if v.game_type() == GameType::Script {
        rules::SCRIPT_INDEX
    } else {
        ir.header.engine()
    };
    settings_on(ir, GameType::from_engine(i32::from(engine)), engine)
}

fn settings_on(ir: &rules::Ir, stock: GameType, carrier: u8) -> Result<GameVariant, PortableError> {
    let mut v = GameVariant::stock(stock).ok_or(PortableError::NoStock(carrier))?;
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

/// The first extra of `e` that no [`rules::settings::TABLE`] row stores:
/// only gravity, damage, shield, health, speed and unlimited grenades have
/// rows.
fn non_setting_extra(e: &ext::Ext) -> Option<&'static str> {
    let d = ext::Ext::default();
    [
        (e.primary != d.primary, "a primary weapon id"),
        (e.secondary != d.secondary, "a secondary weapon id"),
        (e.team_size != d.team_size, "a team size"),
    ]
    .into_iter()
    .find_map(|(set, what)| set.then_some(what))
}

/// Sets the [`rules::settings::TABLE`] row `index` of `v` to `value`: the
/// record setting the row stores on `v`'s carrier, or its `hx` field.
/// Refused as a script's setting is: a row past the table, one that does
/// not apply to the carrier or a value out of its range. The one way an
/// editor writes an extra.
pub fn set_row(v: &mut GameVariant, index: u16, value: i32) -> Result<(), PortableError> {
    let engine = v.game_type().engine();
    let carrier = u8::try_from(engine)
        .map_err(|_| PortableError::Setting(format!("engine {engine} is no carrier")))?;
    let mut e = v.ext();
    write_setting(v, &mut e, carrier, rules::ir::Setting { index, value })?;
    v.set_ext(&e)
        .map_err(|e| PortableError::Edit(e.to_string()))
}

/// Attaches the built-in script of `v`'s engine to a record that names
/// none: mode 2, its prefix and its default options. The block keeps the
/// extras the table has rows for (gravity, damage, shield, health,
/// speed, unlimited grenades) and drops the rest. Returns false and changes
/// nothing for a record that names a script already, one whose block is
/// ignored, and an engine with no built-in script.
pub fn attach_builtin(v: &mut GameVariant) -> Result<bool, PortableError> {
    let named = match v.ext_state() {
        Block::None => false,
        Block::Ext(e) => e.script.mode != Mode::None,
        Block::Ignored(_) => return Ok(false),
    };
    if named {
        return Ok(false);
    }
    let Some(src) = u8::try_from(v.game_type().engine())
        .ok()
        .and_then(rules::builtin::builtin)
    else {
        return Ok(false);
    };
    let ir = rules::compile(src).map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
    let now = v.ext();
    let mut e = ext::Ext {
        gravity: now.gravity,
        damage: now.damage,
        shield: now.shield,
        health: now.health,
        speed: now.speed,
        unlimited_grenades: now.unlimited_grenades,
        ..ext::Ext::default()
    };
    e.script = ext::Script {
        mode: Mode::Rules,
        prefix: ir.id().0[..8].try_into().unwrap(),
        options: ir.default_options(),
    };
    v.set_ext(&e)
        .map_err(|e| PortableError::Edit(e.to_string()))?;
    Ok(true)
}

/// The built-in options a retail record with no script plays with: each
/// type-area row of [`rules::builtin::RETAIL_OPTIONS`] that applies to the
/// record's engine and that `ir` has an option for, read from `v`; every
/// other option takes its default. A value outside its option's range is
/// refused by name; nothing is clamped.
pub fn retail_options(ir: &rules::Ir, v: &GameVariant) -> Result<u32, PortableError> {
    let engine = v.game_type().engine();
    let carrier = u8::try_from(engine)
        .map_err(|_| PortableError::Setting(format!("engine {engine} is no carrier")))?;
    let mut given = Vec::new();
    for &(row_name, option) in rules::builtin::RETAIL_OPTIONS {
        let Some((_, row)) = rules::settings::named(row_name) else {
            continue;
        };
        let Some(id) = row.setting(carrier) else {
            continue;
        };
        if !(0..ir.options.len()).any(|i| ir.option_name(i) == option) {
            continue;
        }
        let value = v
            .get(SettingId(id))
            .ok_or_else(|| PortableError::Setting(format!("{row_name} has no place in the record")))?;
        given.push((option.to_string(), rules::ir::OptionValue::Int(i64::from(value))));
    }
    ir.pack_options(&given)
        .map_err(|e| PortableError::Options(format!("{}: {e}", ir.header.name)))
}

/// The index-6 record a retail record with no script starts as: the
/// built-in script of its type, with the common settings and extras kept,
/// the type area mapped into the script's options ([`retail_options`]) and
/// then zero. `None` for a record that names a script or has an ignored
/// block, and for Slayer, the script index and an engine with no built-in.
/// `v` is not changed: the stored record and the wire keep its bytes
/// (docs/game/game-types-without-carriers.md, "Retail records").
pub fn script_record_of_retail(v: &GameVariant) -> Result<Option<GameVariant>, PortableError> {
    let ty = v.game_type();
    if matches!(ty, GameType::Slayer | GameType::Script | GameType::Other(_)) {
        return Ok(None);
    }
    let mut e = match v.ext_state() {
        Block::None => ext::Ext::in_effect(v.ext_block()),
        Block::Ext(e) if e.script.mode == Mode::None => e,
        _ => return Ok(None),
    };
    let Some(src) = u8::try_from(ty.engine()).ok().and_then(rules::builtin::builtin) else {
        return Ok(None);
    };
    let ir = rules::compile(src).map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
    let options = retail_options(&ir, v)?;
    let mut raw = *v.bytes();
    raw[record::TYPE_AREA].fill(0);
    let mut out = GameVariant::from_bytes(&raw).map_err(|e| PortableError::Edit(e.to_string()))?;
    out.set_game_type(GameType::Script);
    e.script = ext::Script {
        mode: Mode::Rules,
        prefix: ir.id().0[..8].try_into().unwrap(),
        options,
    };
    out.set_ext(&e)
        .map_err(|e| PortableError::Edit(e.to_string()))?;
    Ok(Some(out))
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
            "{} is a {} setting; this script's record is {}'s",
            row.name,
            row.carrier_words(),
            rules::settings::carrier_label(carrier)
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
                ExtField::Speed => e.speed = percent()?,
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
                ExtField::Speed => i32::from(e.speed),
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
        PortScript::Builtin => {
            // A record that names a script is refused: its blob is not at
            // hand, and the engine's built-in is not that script.
            if matches!(v.ext_state(), Block::Ext(e) if e.script.mode == Mode::Rules) {
                return Err(PortableError::ScriptMissing);
            }
            let engine = v.game_type().engine();
            let src = u8::try_from(engine)
                .ok()
                .and_then(rules::builtin::builtin)
                .ok_or(PortableError::NoBuiltin(engine))?;
            Attach::from_source(src)?
        }
        PortScript::Ir(ir) => Attach {
            ir,
            source: None,
            options: None,
        },
        PortScript::Source(src) => Attach::from_source(&src)?,
    };
    let mut attach = attach;
    if !options.is_empty() {
        let (ir, _) = rules::verify::load(&attach.ir)
            .map_err(|e| PortableError::ScriptRefused(e.to_string()))?;
        let bits = ir
            .pack_options(options)
            .map_err(|e| PortableError::Options(e.to_string()))?;
        attach.options = Some(bits);
    }
    Bundle::new(v, &name, description.unwrap_or(""), Some(attach))
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
    fn a_script_index_bundle_files_by_header_and_edits_common_settings() {
        let mut v = GameVariant::stock(GameType::Script).unwrap();
        let score = rules::settings::named("score_limit").unwrap().0;
        set_row(&mut v, score, 11).unwrap();
        let attach = Attach::from_source(rules::builtin::GLITCH_HUNT).unwrap();
        let bundle = Bundle::new(&v, "Unfamiliar name", "", Some(attach)).unwrap();
        assert_eq!(bundle.game_type, rules::types::Type::Research.row().label);
        assert_eq!(bundle.engine, record::SCRIPT_INDEX);
        let checked = read(&bundle.canonical()).unwrap().check().unwrap();
        assert_eq!(checked.variant.game_type(), GameType::Script);
        assert_eq!(checked.variant.get(SettingId(2)), Some(11));
        let header = checked.script.unwrap().1.header;
        assert_eq!(header.game_type, rules::types::Type::Research);

        let attach = Attach::from_source(&carried("ctf", "ctf_flag_type = 1\n")).unwrap();
        assert!(matches!(
            Bundle::new(&v, "CTF", "", Some(attach)),
            Err(PortableError::Setting(_))
        ));
        assert!(matches!(
            Bundle::new(&v, "No script", "", None),
            Err(PortableError::ScriptMissing)
        ));
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
            PortScript::Ir(slayer_ir()),
            PortScript::Source(rules::builtin::SLAYER.to_string()),
        ] {
            let a = port(&slayer(), None, None, script.clone()).unwrap();
            let b = port(&slayer(), None, None, script).unwrap();
            assert_eq!(a.canonical(), b.canonical());
            assert_eq!(a.id(), b.id());
        }
        // An IR alone travels without source.
        let i = port(&slayer(), None, None, PortScript::Ir(slayer_ir())).unwrap();
        assert!(i.script.as_ref().unwrap().source.is_none());
        // A different script text, a different id.
        let b = port(&slayer(), None, None, PortScript::Builtin).unwrap();
        assert_ne!(b.id(), i.id());
        // No port leaves a record without a script.
        for bundle in [&b, &i] {
            assert_eq!(bundle.variant().unwrap().ext().script.mode, Mode::Rules);
        }
    }

    #[test]
    fn a_port_keeps_no_extras_of_the_input_record() {
        let mut v = slayer();
        v.set_ext(&ext::Ext {
            gravity: 50,
            speed: 120,
            team_size: 2,
            ..ext::Ext::default()
        })
        .unwrap();
        let b = port(&v, None, None, PortScript::Builtin).unwrap();
        let e = b.variant().unwrap().ext();
        assert_eq!(e.gravity, 100);
        assert_eq!(e.speed, 100);
        assert_eq!(e.team_size, 0);
        assert_eq!(e.script.mode, Mode::Rules);
        assert!(b.check().is_ok());
        // A bundle with an extra no script setting sets is refused.
        let mut bad = b.variant().unwrap();
        let mut e = bad.ext();
        e.team_size = 2;
        bad.set_ext(&e).unwrap();
        let mut bundle = b.clone();
        bundle.record_hex = hex(bad.bytes());
        assert_eq!(bundle.check().unwrap_err(), PortableError::Extras("a team size"));
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
        // A record that names a script is not given the engine's built-in
        // in its place ...
        let scripted = port(&slayer(), None, None, PortScript::Builtin).unwrap();
        let v = scripted.variant().unwrap();
        assert_eq!(
            port(&v, None, None, PortScript::Builtin).unwrap_err(),
            PortableError::ScriptMissing
        );
        // ... but can be given its script again.
        assert_eq!(
            port(&v, None, None, PortScript::Source(rules::builtin::SLAYER.into()))
                .unwrap()
                .id(),
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
                port(&slayer(), Some(name), None, PortScript::Builtin).unwrap_err(),
                PortableError::Name(want),
                "{name:?}"
            );
        }
        assert!(matches!(
            port(&slayer(), Some(&"n".repeat(32)), None, PortScript::Builtin),
            Err(PortableError::Name(_))
        ));
        assert!(port(&slayer(), Some("Wheeeeee!"), None, PortScript::Builtin).is_ok());
        assert!(matches!(
            port(&slayer(), None, Some(&"d".repeat(151)), PortScript::Builtin),
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
    }

    #[test]
    fn a_script_over_the_budget_is_not_published() {
        let heavy = "rules \"Heavy\" 1\ntype slayer\nglobal var n: int = 0\n\
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
        let plain = Bundle::new(&slayer(), "Fists", "", None).unwrap();
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
            port(
                &port_input(v1.as_bytes()).unwrap().0,
                None,
                None,
                PortScript::Source(rules::builtin::SLAYER.into())
            )
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
type slayer
uses slayer
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
        format!("rules \"V\" 1\ntype {carrier}\nuses {carrier}\nsettings {{\n{lines}}}\n")
    }

    #[test]
    fn a_named_record_takes_its_name_not_the_scripts() {
        let long = "A script name far longer than any record's thirty-one";
        let src = format!("rules \"{long}\" 1\ntype slayer\nuses slayer\nsettings {{\nscore_limit = 10\n}}\n");
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

        let mut want = GameVariant::stock(GameType::Script).unwrap();
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
        let mut stock = GameVariant::stock(GameType::Script).unwrap();
        stock.set_name(&slayer.header.name).unwrap();
        assert_eq!(v.bytes()[..ext::AT], stock.bytes()[..ext::AT]);
        assert_eq!(v.ext().script.options, slayer.pack_options(&given).unwrap());
        assert_eq!(v.ext().gravity, 100);
    }

    /// A retail record of `ty` with each `(row, value)` of its type area set.
    fn retail(ty: GameType, rows: &[(&str, i32)]) -> GameVariant {
        let mut v = GameVariant::stock(ty).unwrap();
        for &(name, value) in rows {
            let (_, row) = rules::settings::named(name).unwrap();
            let id = row.setting(ty.engine() as u8).unwrap();
            v.set(SettingId(id), value).unwrap();
        }
        v
    }

    #[test]
    fn a_retail_record_with_no_script_maps_to_its_builtin_on_the_script_index() {
        // (type, the type-area rows set, the option each reads back as).
        let cases: &[(GameType, &[(&str, i32)], &[(&str, i32)])] = &[
            (
                GameType::King,
                &[("king_moving_hill", 45)],
                &[("move_time", 45)],
            ),
            (
                GameType::Territories,
                &[("territories_count", 5), ("territories_contest_time", 9)],
                &[("in_play", 5), ("contest_time", 9)],
            ),
            (
                GameType::Ctf,
                &[
                    ("ctf_flag_type", 1),
                    ("ctf_reset_time", 12),
                    ("ctf_touch_return", 1),
                    ("ctf_flag_at_home", 1),
                ],
                &[("flag_type", 1), ("reset_time", 12), ("touch_return", 1), ("flag_at_home", 1)],
            ),
            (
                GameType::Assault,
                &[("ctf_flag_type", 1), ("ctf_reset_time", 20), ("ctf_touch_return", 1), ("assault_arm_time", 9)],
                &[("flag_type", 1), ("reset_time", 20), ("touch_return", 1), ("arm_time", 9), ("flag_at_home", 0)],
            ),
            (
                GameType::Oddball,
                &[("oddball_ball_count", 3)],
                &[("ball_count", 3), ("reset_time", 30)],
            ),
            (GameType::Juggernaut, &[], &[]),
        ];
        for &(ty, area, want) in cases {
            let mut v = retail(ty, area);
            v.set(SettingId(16), 3).unwrap();
            let before = v.clone();
            let out = script_record_of_retail(&v).unwrap().expect("a record that maps");
            // The stored record is unchanged.
            assert_eq!(v.bytes()[..], before.bytes()[..], "{ty:?}");
            // The new record is on the script index with a zero type area.
            assert_eq!(out.game_type(), GameType::Script, "{ty:?}");
            assert!(out.bytes()[record::TYPE_AREA].iter().all(|&b| b == 0), "{ty:?}");
            // The common settings and the name read back equal.
            assert_eq!(out.name(), v.name(), "{ty:?}");
            assert_eq!(out.team_play(), v.team_play(), "{ty:?}");
            assert_eq!(out.get(SettingId(16)), Some(3), "{ty:?}");
            for (at, size) in [(0x4c, 4), (0x50, 4), (0x54, 4), (0x78, 4), (0xac, 4)] {
                assert_eq!(out.bytes()[at..at + size], v.bytes()[at..at + size], "{ty:?} +{at:#x}");
            }
            // The script is the type's built-in, and its options are the area.
            let ir = rules::compile(rules::builtin::builtin(ty.engine() as u8).unwrap()).unwrap();
            let script = out.ext().script;
            assert_eq!((script.mode, script.prefix[..].to_vec()), (Mode::Rules, ir.id().0[..8].to_vec()), "{ty:?}");
            for &(name, value) in want {
                assert_eq!(ir.option_value(script.options, name), Some(value), "{ty:?} {name}");
            }
            assert_eq!(script.options, retail_options(&ir, &v).unwrap(), "{ty:?}");
        }
    }

    #[test]
    fn a_retail_record_that_does_not_map_is_left_alone_or_refused_by_name() {
        // Slayer, a record that names a script and an engine with no built-in.
        let slayer = GameVariant::stock(GameType::Slayer).unwrap();
        assert_eq!(script_record_of_retail(&slayer), Ok(None));
        let mut named = retail(GameType::Ctf, &[]);
        assert!(attach_builtin(&mut named).unwrap());
        assert_eq!(script_record_of_retail(&named), Ok(None));
        assert_eq!(script_record_of_retail(&GameVariant::stock(GameType::Script).unwrap()), Ok(None));
        // A flag reset time past the script's option is refused, not clamped.
        let long = retail(GameType::Ctf, &[("ctf_reset_time", 301)]);
        assert!(matches!(script_record_of_retail(&long), Err(PortableError::Options(_))));
    }

    #[test]
    fn the_matchmaking_ctf_and_assault_records_map_their_type_area() {
        for (key, src, engine, want) in [
            (
                "classic_ctf",
                rules::builtin::CTF,
                1,
                &[("flag_at_home", 1), ("touch_return", 1)][..],
            ),
            ("assault", rules::builtin::ASSAULT, 9, &[("flag_type", 1)][..]),
        ] {
            let m = rules::builtin::MATCHMAKING.iter().find(|m| m.key == key).unwrap();
            let record = record_for(&compiled(m.source), &[]).unwrap();
            assert_eq!(record.game_type().engine(), engine, "{key}");
            let ir = compiled(src);
            let options = retail_options(&ir, &record).unwrap();
            for &(name, value) in want {
                assert_eq!(ir.option_value(options, name), Some(value), "{key} {name}");
            }
        }
    }

    #[test]
    fn only_slayer_scripts_without_type_area_writes_move_to_the_script_index() {
        // A category and the game whose rules it uses can differ. An hx
        // extra and a common setting do not need Slayer's type area.
        let research = compiled(
            "rules \"Practice\" 1\ntype research\nuses slayer\nsettings {\ngravity = 50\nlives = 3\n}\n",
        );
        let v = record_for(&research, &[]).unwrap();
        assert!(!writes_type_area(&research, research.header.engine()));
        assert_eq!(v.game_type(), GameType::Script);
        assert!(v.bytes()[record::TYPE_AREA].iter().all(|&b| b == 0));
        assert_eq!(v.ext().gravity, 50);
        assert_eq!(v.get(SettingId(16)), Some(3));
        assert_eq!(v.ext().script.prefix[..], research.id().0[..8]);

        for (setting, writes_area) in [("lives = 3", false), ("ctf_flag_type = 0", true)] {
            let ir = compiled(&carried("ctf", &format!("{setting}\n")));
            assert_eq!(writes_type_area(&ir, ir.header.engine()), writes_area);
            assert_eq!(record_for(&ir, &[]).unwrap().game_type(), GameType::Ctf);
        }
        let party = compiled("rules \"Duel\" 1\ntype party\nuses juggernaut\n");
        assert_eq!(record_for(&party, &[]).unwrap().game_type(), GameType::Juggernaut);

        // Importing an existing Slayer record keeps its engine and type
        // settings. The producer's index choice is only for new records.
        let old = GameVariant::stock(GameType::Slayer).unwrap();
        let attached = Attach::from_source(rules::builtin::SLAYER).unwrap();
        let b = Bundle::new(&old, "Earlier variant", "", Some(attached)).unwrap();
        let back = read(&b.canonical()).unwrap().variant().unwrap();
        assert_eq!(back.game_type(), GameType::Slayer);
        assert_eq!(back.bytes()[record::TYPE_AREA], old.bytes()[record::TYPE_AREA]);
    }

    #[test]
    fn a_script_of_several_files_travels_as_all_of_them() {
        let main = "rules \"Split\" 1\ntype slayer\nuse \"waves.rules\"\non kill(v, k) { if k != none { score(k, bonus()) } }\n";
        let one = "rules \"Split\" 1\ntype slayer\nconst base = 2\nfn bonus() -> int = base + 1\non kill(v, k) { if k != none { score(k, bonus()) } }\n";
        let mods: BTreeMap<String, String> =
            [("waves.rules".to_string(), "const base = 2\nfn bonus() -> int = base + 1\n".to_string())].into();
        let attach = Attach::from_files(main, &mods).unwrap();
        // The IR is the one of the same text in a file.
        assert_eq!(attach.ir, Attach::from_source(one).unwrap().ir);
        let v = GameVariant::stock(GameType::Slayer).unwrap();
        let b = Bundle::new(&v, "Split", "", Some(attach.clone())).unwrap();
        assert!(!b.canonical().contains("modules"), "a one-file script's bytes are unchanged");
        let b = b.with_modules(mods.clone()).unwrap();
        let back = read(&b.canonical()).unwrap();
        assert_eq!(back, b);
        assert_eq!(back.script.as_ref().unwrap().modules, mods);
        // Without the files the root does not compile; a name that is not
        // a module's, a script with no source and too many bytes are refused.
        assert!(matches!(Attach::from_files(main, &BTreeMap::new()), Err(PortableError::ScriptRefused(_))));
        let bad: BTreeMap<String, String> = [("../x.rules".to_string(), String::new())].into();
        assert!(matches!(b.clone().with_modules(bad), Err(PortableError::ScriptRefused(_))));
        let no_source = Bundle::new(&v, "Split", "", Some(Attach { source: None, ..attach })).unwrap();
        assert_eq!(no_source.with_modules(mods).unwrap_err(), PortableError::ScriptUnexpected);
        let big: BTreeMap<String, String> = [("big.rules".to_string(), "x".repeat(MAX_SOURCE))].into();
        assert_eq!(b.with_modules(big).unwrap_err(), PortableError::ScriptTooLarge("source"));
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
                "juggernaut_movement is a Juggernaut setting; this script's record is Slayer's".into()
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
        assert_eq!(a.engine, record::SCRIPT_INDEX);
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
    fn suggestion(n: u8, name: &str) -> MapSuggestion {
        MapSuggestion {
            base: format!("{:064x}", 1),
            variant: format!("{n:064x}"),
            name: name.into(),
        }
    }

    fn plain() -> Bundle {
        Bundle::new(&king(), "Hill", "", None).unwrap()
    }

    #[test]
    fn a_bundle_without_maps_keeps_its_bytes() {
        let b = plain();
        assert!(b.maps.is_empty());
        assert!(!b.canonical().contains("maps"));
        // A file written before the field existed reads, and writes back
        // the same bytes and id.
        let back = read(&b.canonical()).unwrap();
        assert_eq!(back.canonical(), b.canonical());
        assert_eq!(back.id(), b.id());
    }

    #[test]
    fn suggestions_travel_and_change_the_id() {
        let b = plain();
        let with = b.clone().with_maps(vec![suggestion(2, "Castle")]).unwrap();
        assert_ne!(with.id(), b.id());
        let back = read(&with.canonical()).unwrap();
        assert_eq!(back, with);
        let base: Sha256 = format!("{:064x}", 1).parse().unwrap();
        let other: Sha256 = format!("{:064x}", 9).parse().unwrap();
        assert_eq!(back.suggested_for(&base).count(), 1);
        assert_eq!(back.suggested_for(&other).count(), 0);
    }

    #[test]
    fn more_than_eight_suggestions_are_refused() {
        let eight: Vec<_> = (0..8).map(|n| suggestion(n, "Map")).collect();
        assert!(plain().with_maps(eight.clone()).is_ok());
        let mut nine = eight;
        nine.push(suggestion(8, "Map"));
        assert_eq!(plain().with_maps(nine.clone()).unwrap_err(), PortableError::MapsTooMany(9));
        // A file that carries nine is refused when it is read.
        let mut b = plain();
        b.maps = nine;
        assert_eq!(read(&b.canonical()).unwrap_err(), PortableError::MapsTooMany(9));
    }

    #[test]
    fn a_malformed_suggestion_is_refused_by_name() {
        let bad = |m: MapSuggestion| plain().with_maps(vec![m]).unwrap_err();
        let mut m = suggestion(2, "Castle");
        m.base = "abc".into();
        assert!(matches!(bad(m), PortableError::MapSuggestion(w) if w.contains("base")));
        let mut m = suggestion(2, "Castle");
        m.variant = "G".repeat(64);
        assert!(matches!(bad(m), PortableError::MapSuggestion(w) if w.contains("id")));
        assert!(matches!(bad(suggestion(2, "")), PortableError::MapSuggestion(_)));
        assert!(matches!(bad(suggestion(2, &"x".repeat(64))), PortableError::MapSuggestion(_)));
        assert!(matches!(bad(suggestion(2, "a|b")), PortableError::MapSuggestion(_)));
        let twice = plain().with_maps(vec![suggestion(2, "A"), suggestion(2, "B")]);
        assert!(matches!(twice, Err(PortableError::MapSuggestion(w)) if w.contains("twice")));
    }
}
