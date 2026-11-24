//! Porting variants into self-contained bundles (docs/game/rules-engine.md,
//! "Live game variants in the game"), through `variants::portable::port`:
//!
//! - [`port_saves`]: the player's saved variants (the XAPI save-game
//!   variants under `UDATA\4D530064`, `variants::save::SaveStore`), once per
//!   source, at boot off the game thread. The source saves are only read.
//! - [`port_builtins`]: the retail built-ins and the listed scripts'
//!   variants (`rules::builtin::LISTED`), on request (they are already in
//!   the game's list, so they are not put in the store at boot).

use content::Sha256;
pub use variants::portable::PortScript;
use variants::record::GameVariant;
use variants::save::SaveStore;

use super::bundle::Bundle;
use super::store::Store;

/// `v` as a bundle named `name` with `description`, its script per
/// `script` (`variants::portable::port`), or why not.
pub fn port(
    v: &GameVariant,
    name: &str,
    description: &str,
    script: PortScript,
) -> Result<Bundle, String> {
    variants::portable::port(v, Some(name), Some(description), script).map_err(|e| e.to_string())
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
        let ported = port_saved(store, &s.contents.variant, &name, script)
            .and_then(|b| store.put_ported(&key, b));
        match ported {
            Ok(_) => report.ported += 1,
            Err(why) => report.refused.push((name, why)),
        }
    }
    report
}

/// A renamed copy of a scripted variant already names the blob and its
/// option bits. Use that reference when automatically porting the save.
fn port_saved(store: &Store, v: &GameVariant, name: &str, script: &PortScript) -> Result<Bundle, String> {
    let saved = v.ext().script;
    if saved.mode == variants::ext::Mode::Rules && *script == PortScript::Builtin {
        let blob = crate::rules::runtime::compiled_in(saved.prefix)
            .map(|b| b.blob.as_slice())
            .or_else(|| store.script(saved.prefix))
            .ok_or_else(|| format!("no local rules script with prefix {}", super::hex(&saved.prefix)))?;
        let attach = variants::portable::Attach {
            ir: blob.to_vec(),
            source: None,
            options: Some(saved.options),
        };
        let mut bundle = Bundle::new(v, name, "", Some(attach)).map_err(|e| e.to_string())?;
        // Attaching a new script starts with its defaults. This save already
        // names the script and can carry the player's edits to those extras.
        let saved_hex = super::hex(v.bytes());
        if bundle.record_hex != saved_hex {
            bundle.record_hex = saved_hex;
            bundle.check().map_err(|e| e.to_string())?;
        }
        return Ok(bundle);
    }
    port(v, name, "", script.clone())
}

/// The retail built-ins in `schema` and the listed scripts' variants as
/// bundles (for the CLI). Each is a bundle or its name and why not.
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
    for b in ::rules::builtin::LISTED {
        out.push(listed_bundle(b.source).map_err(|why| (b.name.to_owned(), why)));
    }
    out
}

/// A listed script's bundle: the bundle of its own variant
/// (`variants::portable::from_script`, its record, name and `about`),
/// with its default options. It carries its script whatever `script`
/// [`port_builtins`] was given.
fn listed_bundle(source: &str) -> Result<Bundle, String> {
    let ir = ::rules::compile(source).map_err(|e| e.to_string())?;
    variants::portable::from_script(&ir, None, &[]).map_err(|e| e.to_string())
}
