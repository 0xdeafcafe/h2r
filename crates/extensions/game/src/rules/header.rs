//! What a script's header asks of the game outside its rules: the
//! superbounce mode, and a place under Tweakers in the game type list
//! (docs/game/rules-engine.md, "Firefight, AI and bounces";
//! docs/game/community-gametypes.md, "Tweakers").

use ::rules::ir::{Header, Ir};
use variants::ext::{Ext, LEN, Mode};

use super::runtime;

/// The capability a script requires for the superbounce mode: its HUD,
/// its marks over the map and its medals.
pub const SUPERBOUNCE: &str = "hud.superbounce";

/// Whether a script asks for the superbounce mode: its header requires
/// [`SUPERBOUNCE`]. `rules::caps` has no such capability yet, and the
/// compiler and the verifier refuse a name it lacks, so no script that
/// loads asks for it.
pub fn asks_superbounce(ir: &Ir) -> bool {
    ir.header.requires.iter().any(|c| c == SUPERBOUNCE)
}

/// Whether a script's header files it under Tweakers. The header has no
/// such line yet (`list tweakers`), so none does.
pub fn lists_tweakers(_header: &Header) -> bool {
    false
}

/// The script a record's prefix names, when this machine has it: a
/// built-in ([`runtime::builtins`]) or the game-variant store's.
fn named(prefix: [u8; 8]) -> Option<Ir> {
    let blob = match runtime::builtins().iter().find(|b| b.id.0[..8] == prefix) {
        Some(b) => b.blob.clone(),
        None => crate::game_variants::store()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .script(prefix)?
            .to_vec(),
    };
    ::rules::verify::load(&blob).ok().map(|(ir, _)| ir)
}

/// Whether a block naming a rules script (mode 2) is listed under
/// Tweakers: the script it names is on this machine and its header says
/// so ([`lists_tweakers`]). A script this machine lacks is listed under
/// its carrier's game type.
pub fn block_is_tweaker(block: &[u8; LEN]) -> bool {
    let script = Ext::in_effect(block).script;
    script.mode == Mode::Rules
        && named(script.prefix).is_some_and(|ir| lists_tweakers(&ir.header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use variants::ext::Script;

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

    #[test]
    fn no_script_asks_for_the_superbounce_mode_or_tweakers_yet() {
        for b in runtime::builtins() {
            let (ir, _) = ::rules::verify::load(&b.blob).unwrap();
            assert!(!asks_superbounce(&ir), "engine {}", b.engine);
            assert!(!lists_tweakers(&ir.header), "engine {}", b.engine);
            let mut p = [0; 8];
            p.copy_from_slice(&b.id.0[..8]);
            assert!(!block_is_tweaker(&rules_block(p)), "engine {}", b.engine);
        }
        let sb = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
        assert!(!asks_superbounce(&sb));
        // Requiring the capability is refused while caps lacks it.
        let asking = "rules \"T\" 1\ncarrier slayer\nrequires hud.superbounce\n";
        assert!(::rules::compile(asking).is_err());
        // A block with no script, and one naming a script not here.
        assert!(!block_is_tweaker(&[0; LEN]));
        assert!(!block_is_tweaker(&rules_block([9; 8])));
    }

    #[test]
    fn the_capability_is_read_from_requires() {
        let mut ir = ::rules::compile(::rules::builtin::SUPERBOUNCE).unwrap();
        ir.header.requires.push(SUPERBOUNCE.into());
        assert!(asks_superbounce(&ir));
    }
}
