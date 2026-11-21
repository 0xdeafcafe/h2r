//! Porting variants into self-contained bundles (docs/game/rules-engine.md,
//! "Live game variants in the game"):
//!
//! - [`port_saves`]: the player's saved variants (the XAPI save-game
//!   variants under `UDATA\4D530064`, `variants::save::SaveStore`), once per
//!   source, at boot off the game thread. The source saves are only read.
//! - [`port_builtins`]: the retail built-ins and the community variants, on
//!   request (they are already in the game's list, so they are not put in
//!   the store at boot).
//!
//! [`port`] is the store's stand-in for `variants::portable::port`.

use content::Sha256;
use variants::ext::{Mode, Script};
use variants::record::GameVariant;
use variants::save::SaveStore;

use super::bundle::Bundle;
use super::store::Store;

/// What a ported variant's script is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortScript {
    /// The record as it is: a retail record names no script and plays as
    /// its engine's lifted rules.
    Keep,
    /// The built-in script of the record's engine (`rules::builtin`),
    /// named in the record's block and carried in the bundle.
    Builtin,
    /// A given script: its name and IR.
    Given { name: String, ir: Vec<u8> },
}

impl PortScript {
    /// `variants.port.script`'s value: `keep` (0) or `builtin` (1).
    pub fn from_setting(v: i64) -> Self {
        if v == 1 { Self::Builtin } else { Self::Keep }
    }
}

/// The built-in script of engine `engine`: its name and IR.
#[cfg(feature = "rules")]
fn builtin_script(engine: i32) -> Result<(String, Vec<u8>, u32), String> {
    let b = crate::rules::runtime::builtins()
        .iter()
        .find(|b| i32::from(b.engine) == engine)
        .ok_or_else(|| format!("no built-in script rides engine {engine}"))?;
    let (ir, _) = ::rules::verify::load(&b.blob).map_err(|e| format!("the built-in: {e}"))?;
    Ok((ir.header.name.clone(), b.blob.clone(), ir.default_options()))
}

#[cfg(not(feature = "rules"))]
fn builtin_script(_engine: i32) -> Result<(String, Vec<u8>, u32), String> {
    Err("this build has no built-in scripts (the rules feature is not compiled in)".into())
}

/// The default option bits of a given IR.
#[cfg(feature = "rules")]
fn options_of(ir: &[u8]) -> Result<u32, String> {
    ::rules::verify::load(ir)
        .map(|(ir, _)| ir.default_options())
        .map_err(|e| format!("the script does not verify: {e}"))
}

#[cfg(not(feature = "rules"))]
fn options_of(_ir: &[u8]) -> Result<u32, String> {
    Ok(0)
}

/// `record` as a bundle named `name`, its script per `script`. A record
/// that already names a script keeps it under [`PortScript::Builtin`]
/// (only [`PortScript::Given`] replaces one), and then needs its IR given.
pub fn port(
    record: &GameVariant,
    name: &str,
    description: &str,
    script: PortScript,
) -> Result<Bundle, String> {
    let mut v = record.clone();
    let named = v.ext().script.mode == Mode::Rules;
    let attach = match script {
        PortScript::Keep if named => {
            return Err("the record names a script whose IR this port does not have".into());
        }
        PortScript::Keep => None,
        PortScript::Builtin if named => {
            return Err("the record names a script whose IR this port does not have".into());
        }
        PortScript::Builtin => {
            let (n, ir, options) = builtin_script(v.game_type().engine())?;
            Some((n, ir, options))
        }
        PortScript::Given { name, ir } => {
            let options = options_of(&ir)?;
            Some((name, ir, options))
        }
    };
    let Some((script_name, ir, options)) = attach else {
        return Ok(Bundle::new(name, description, v.bytes()));
    };
    let hash = Sha256::of(&ir);
    let mut e = v.ext();
    let mut prefix = [0; 8];
    prefix.copy_from_slice(&hash.0[..8]);
    e.script = Script {
        mode: Mode::Rules,
        prefix,
        options,
    };
    v.set_ext(&e)
        .map_err(|e| format!("the record's block: {e}"))?;
    let mut b = Bundle::new(name, description, v.bytes());
    b.script = Some(super::bundle::BundleScript {
        name: script_name,
        hash: hash.to_string(),
        ir_b64: None,
        source: None,
    });
    Ok(b.with_ir(&ir))
}

/// One run's porting: how many were ported, skipped (already ported) and
/// refused (by name, with why).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub ported: usize,
    pub skipped: usize,
    pub refused: Vec<(String, String)>,
}

impl Report {
    pub fn line(&self) -> String {
        let mut line = format!(
            "game variants: ported {} saved variants, skipped {} already ported, refused {}",
            self.ported,
            self.skipped,
            self.refused.len()
        );
        for (name, why) in &self.refused {
            line.push_str(&format!("; {name}: {why}"));
        }
        line
    }
}

/// A saved variant's source key: the first 16 hex digits of the SHA-256 of
/// its file's record and signature, so an edited save ports again.
pub fn source_key(s: &variants::save::SavedVariant) -> String {
    let mut bytes = s.contents.variant.bytes().to_vec();
    bytes.extend_from_slice(&s.contents.signature);
    Sha256::of(&bytes).to_string()[..16].to_owned()
}

/// Ports every saved variant in `saves` not yet in `store`. Never writes
/// under the save root.
pub fn port_saves(store: &mut Store, saves: &SaveStore, script: &PortScript) -> Report {
    let mut report = Report::default();
    let list = match saves.list() {
        Ok(list) => list,
        Err(e) => {
            report
                .refused
                .push((saves.root.display().to_string(), e.to_string()));
            return report;
        }
    };
    for s in list {
        let key = source_key(&s);
        if store.has_ported(&key) {
            report.skipped += 1;
            continue;
        }
        let name = s.contents.variant.name();
        let ported = port(&s.contents.variant, &name, "", script.clone())
            .and_then(|b| store.put_ported(&key, b));
        match ported {
            Ok(_) => report.ported += 1,
            Err(why) => report.refused.push((name, why)),
        }
    }
    report
}

/// The retail built-ins in `schema` and the community variants as bundles
/// (for the CLI). Each is a bundle or its name and why not.
pub fn port_builtins(
    schema: &variants::Schema,
    script: &PortScript,
) -> Vec<Result<Bundle, (String, String)>> {
    let mut out = Vec::new();
    for b in &schema.builtins {
        out.push(
            schema
                .build(b)
                .map_err(|e| e.to_string())
                .and_then(|v| port(&v, &b.name, "", script.clone()))
                .map_err(|why| (b.name.clone(), why)),
        );
    }
    for c in crate::community_variants::VARIANTS {
        let built = variants::GameType::from_list_index(c.ty as u32)
            .ok_or_else(|| format!("list type {} is unknown", c.ty as u32))
            .and_then(|ty| {
                let mut v = schema.new_variant(ty);
                v.set_name(c.name).map_err(|e| e.to_string())?;
                for (id, value) in c.pairs() {
                    v.set(variants::SettingId(id), value)
                        .map_err(|e| e.to_string())?;
                }
                Ok(v)
            })
            .and_then(|v| port(&v, c.name, c.description, script.clone()))
            .map_err(|why| (c.name.to_owned(), why));
        out.push(built);
    }
    out
}
