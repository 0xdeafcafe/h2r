//! The compiled-in scripts a record can name by its hash prefix: the
//! built-in ports, the examples, the listed community scripts and the
//! default Live lineup, each compiled once
//! (docs/game/rules-engine.md, "The host runtime").

use std::sync::OnceLock;

use ::rules::Sha256;
use ::rules::caps;

/// One built-in script, compiled once: its engine index (an example's
/// carrier), blob and hash.
pub struct Builtin {
    pub engine: u8,
    pub blob: Vec<u8>,
    pub id: Sha256,
}

/// Every built-in script (`rules::builtin`), compiled once. A built-in
/// that does not compile is left out with a line.
pub fn builtins() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for &(name, engine) in caps::CARRIERS {
            let Some(src) = ::rules::builtin::builtin(engine) else {
                continue;
            };
            match ::rules::compile(src) {
                Ok(ir) => out.push(Builtin {
                    engine,
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the built-in {name} script does not compile: {e}"),
            }
        }
        out
    })
}

/// Every example script (`rules::builtin::examples`), compiled once, each
/// with its carrier as `engine`. A record names one by its prefix, as any
/// script; they are not ports, so a record with no mode never runs one.
pub fn examples() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for &(name, src) in ::rules::builtin::examples() {
            match ::rules::compile(src) {
                Ok(ir) => out.push(Builtin {
                    engine: ir.header.engine(),
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the example {name} script does not compile: {e}"),
            }
        }
        out
    })
}

/// Every listed script (`rules::builtin::LISTED`, the built-in list's
/// community variants), compiled once, each with its carrier as
/// `engine`. A built-in entry's record names one by its prefix
/// (`crate::community_variants::write_blocks`), so a game on it, or on a
/// copy of it, runs it (docs/game/rules-variants.md, "Built-in list
/// entries"). Superbounce and Siege are examples too; [`compiled_in`]
/// finds them there first.
pub fn listed() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for b in crate::community_variants::scripts() {
            match ::rules::compile(&b.source) {
                Ok(ir) => out.push(Builtin {
                    engine: ir.header.engine(),
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!("rules: the listed {} script does not compile: {e}", b.name),
            }
        }
        out
    })
}

/// Every script of the default Live lineup
/// (`rules::builtin::MATCHMAKING`), compiled once, each with its carrier as
/// `engine`. A default hopper entry's record names one by its prefix
/// (`live_proto::hopper::defaults`), so a match on it runs it, as the
/// built-in game each `uses`.
pub fn matchmaking() -> &'static [Builtin] {
    static BUILT: OnceLock<Vec<Builtin>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut out = Vec::new();
        for m in ::rules::builtin::MATCHMAKING {
            match ::rules::compile(m.source) {
                Ok(ir) => out.push(Builtin {
                    engine: ir.header.engine(),
                    blob: ir.encode(),
                    id: ir.id(),
                }),
                Err(e) => eprintln!(
                    "rules: the matchmaking {} script does not compile: {e}",
                    m.key
                ),
            }
        }
        out
    })
}

/// The compiled-in script whose hash starts with `prefix`: a port
/// ([`builtins`]), an example ([`examples`]), a listed script
/// ([`listed`]) or a default-lineup script ([`matchmaking`]).
pub fn compiled_in(prefix: [u8; 8]) -> Option<&'static Builtin> {
    builtins()
        .iter()
        .chain(examples())
        .chain(listed())
        .chain(matchmaking())
        .find(|b| b.id.0[..8] == prefix)
}
