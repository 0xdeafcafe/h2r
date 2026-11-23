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
        let ported = port(&s.contents.variant, &name, "", script.clone())
            .and_then(|b| store.put_ported(&key, b));
        match ported {
            Ok(_) => report.ported += 1,
            Err(why) => report.refused.push((name, why)),
        }
    }
    report
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
