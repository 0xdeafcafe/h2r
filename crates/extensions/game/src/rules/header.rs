//! What a script's header asks of the game outside its rules: the
//! superbounce mode, and a row of ours in the game type list
//! (docs/game/rules-engine.md, "Firefight, AI and bounces";
//! docs/game/community-gametypes.md, "Tweakers").

use ::rules::ir::{Header, Ir};
use variants::ext::{Ext, LEN, Mode, Script};

use super::runtime;
use crate::community_variants::Row;

/// The capability a script requires for the superbounce mode: its HUD,
/// its marks over the map and its medals.
pub const SUPERBOUNCE: &str = "hud.superbounce";

/// Whether a script asks for the superbounce mode: its header requires
/// [`SUPERBOUNCE`].
pub fn asks_superbounce(ir: &Ir) -> bool {
    ir.header.requires.iter().any(|c| c == SUPERBOUNCE)
}

/// The row of ours a script's header files it under: `list research`
/// or `list party`.
pub fn list_row(header: &Header) -> Option<Row> {
    Row::of_list(header.list)
}

/// The script a record's prefix names, when this machine has it: a
/// compiled-in one ([`runtime::compiled_in`], ports and examples) or the
/// game-variant store's.
fn named(prefix: [u8; 8]) -> Option<Ir> {
    let blob = match runtime::compiled_in(prefix) {
        Some(b) => b.blob.clone(),
        None => crate::game_variants::store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .script(prefix)?
            .to_vec(),
    };
    ::rules::verify::load(&blob).ok().map(|(ir, _)| ir)
}

/// The header of the script a record's reference names (mode 2), when
/// this machine has it.
fn header_of(script: Script) -> Option<Ir> {
    (script.mode == Mode::Rules).then(|| named(script.prefix)).flatten()
}

/// The row of ours a block naming a rules script (mode 2) is listed
/// under: the script it names is on this machine and its header names one
/// ([`list_row`]). A script this machine lacks is listed under its
/// carrier's game type.
pub fn block_row(block: &[u8; LEN]) -> Option<Row> {
    header_of(Ext::in_effect(block).script).and_then(|ir| list_row(&ir.header))
}

/// Whether a record's script reference names a script that asks for the
/// superbounce mode ([`asks_superbounce`]), when this machine has it. A
/// guest asks of the reference in its host's block, so it needs no
/// running script.
pub fn script_asks_superbounce(script: Script) -> bool {
    header_of(script).is_some_and(|ir| asks_superbounce(&ir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_block(prefix: [u8; 8]) -> [u8; LEN] {
        Ext {
            script: Script {
                mode: Mode::Rules,
                prefix,
                options: 0,
            },
            ..Ext::default()
        }
        .encode()
    }

    fn prefix(b: &runtime::Builtin) -> [u8; 8] {
        let mut p = [0; 8];
        p.copy_from_slice(&b.id.0[..8]);
        p
    }

    /// The superbounce script asks for the mode and lists under Research
    /// and Devolution; no port and no other example does either.
    #[test]
    fn only_the_superbounce_script_asks_for_the_mode_and_a_row() {
        let sb = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
        assert!(asks_superbounce(&sb));
        assert_eq!(list_row(&sb.header), Some(Row::Research));
        for b in runtime::builtins().iter().chain(runtime::examples()) {
            let (ir, _) = ::rules::verify::load(&b.blob).unwrap();
            let is_sb = ir.id() == sb.id();
            assert_eq!(asks_superbounce(&ir), is_sb, "{}", ir.header.name);
            let row = is_sb.then_some(Row::Research);
            assert_eq!(list_row(&ir.header), row, "{}", ir.header.name);
            assert_eq!(block_row(&rules_block(prefix(b))), row, "{}", ir.header.name);
        }
        assert!(runtime::examples().iter().any(|b| b.id == sb.id()));
        // A block with no script, and one naming a script not here.
        assert_eq!(block_row(&[0; LEN]), None);
        assert_eq!(block_row(&rules_block([9; 8])), None);
    }

    /// The built-in Superbounce variant's block turns the mode on through
    /// its script, on any machine that applies the block, the rules
    /// feature on or off.
    #[test]
    fn the_superbounce_block_asks_for_the_mode_through_its_script() {
        let ext = crate::community_variants::superbounce_ext();
        assert!(!ext.superbounce);
        assert!(script_asks_superbounce(ext.script));
        assert!(!script_asks_superbounce(Script::default()));
        let port = Script {
            mode: Mode::Rules,
            prefix: prefix(&runtime::builtins()[0]),
            options: 0,
        };
        assert!(!script_asks_superbounce(port));
        assert!(crate::variant_ext::decide(ext, None, false).superbounce());
        let plain = Ext {
            gravity: 50,
            ..Ext::RETAIL
        };
        assert!(!crate::variant_ext::decide(plain, None, false).superbounce());
    }

    #[test]
    fn the_capability_is_read_from_requires() {
        let mut ir = ::rules::compile(::rules::builtin::SLAYER).unwrap();
        assert!(!asks_superbounce(&ir));
        ir.header.requires.push(SUPERBOUNCE.into());
        assert!(asks_superbounce(&ir));
        assert!(::rules::caps::capability(SUPERBOUNCE, ::rules::caps::API_VERSION).is_some());
    }
}
