//! A sim match's `[rules]` table as the script block on the game's record
//! (docs/runtime/sim.md, "Rules scripts"): the script a lobby choosing
//! that game type would pick, with the options it would pick.
//!
//! [`block_for`] compiles the script (a built-in by key or name, or a
//! `.rules` file), packs the options by name, and builds the record
//! `variants::portable::record_for` builds for a variant file; its `hx`
//! block is what [`crate::game_variants::inject::set_sim_block`] writes
//! on the match's record. A script that is not built in goes into the
//! game-variant store's memory, where the rules runtime looks a prefix up
//! after the compiled-in ones. Every refusal names what was wrong.

use std::path::Path;

use sim_bench::config::{RuleOption, RulesConfig};

/// The block the match's script gives its record, or why the table cannot
/// be played.
pub fn block_for(rules: &RulesConfig) -> Result<[u8; variants::ext::LEN], String> {
    let ir = compile(rules)?;
    let options = options_of(&ir, rules)?;
    let record = variants::portable::record_for(&ir, &options)
        .map_err(|e| format!("sim: rules: {:?}: {e}", rules.script))?;
    if let Ok(mut store) = crate::game_variants::store().lock() {
        store
            .add_script(ir.encode())
            .map_err(|e| format!("sim: rules: {:?}: {e}", rules.script))?;
    }
    Ok(*record.ext_block())
}

fn compile(rules: &RulesConfig) -> Result<::rules::Ir, String> {
    let word = rules.script.as_str();
    let (source, what) = if rules.is_path() {
        let text = std::fs::read_to_string(Path::new(word))
            .map_err(|e| format!("sim: rules: script {word}: {e}"))?;
        (text, word.to_string())
    } else {
        let s = crate::community_variants::script_named(word).ok_or_else(|| {
            format!(
                "sim: rules: no built-in script {word:?} (a name or key, or a path ending .rules); the built-ins are: {}",
                crate::community_variants::script_keys().join(", ")
            )
        })?;
        (s.source.clone(), s.key.clone())
    };
    ::rules::ship(&source)
        .map(|(ir, _)| ir)
        .map_err(|e| format!("sim: rules: script {what}: {e}"))
}

fn options_of(
    ir: &::rules::Ir,
    rules: &RulesConfig,
) -> Result<Vec<(String, ::rules::ir::OptionValue)>, String> {
    use ::rules::ir::OptionValue;
    let declared: Vec<&str> = (0..ir.options.len()).map(|i| ir.option_name(i)).collect();
    let mut out = Vec::new();
    for (name, v) in &rules.options {
        if !declared.contains(&name.as_str()) {
            return Err(format!(
                "sim: rules: the script {:?} has no option {name:?}; it has: {}",
                rules.script,
                if declared.is_empty() { "none".into() } else { declared.join(", ") }
            ));
        }
        out.push((
            name.clone(),
            match v {
                RuleOption::Bool(b) => OptionValue::Bool(*b),
                RuleOption::Int(n) => OptionValue::Int(*n),
                RuleOption::Name(s) => OptionValue::Name(s.clone()),
            },
        ));
    }
    // A value out of range or an enum name that is not one is named here,
    // not left to the record builder.
    ir.pack_options(&out)
        .map_err(|e| format!("sim: rules: script {:?}: {e}", rules.script))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn table(script: &str, options: &[(&str, RuleOption)]) -> RulesConfig {
        RulesConfig {
            script: script.into(),
            options: options.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>(),
        }
    }

    fn script_of(block: &[u8; variants::ext::LEN]) -> variants::ext::Script {
        variants::ext::Ext::in_effect(block).script
    }

    fn id_of(key: &str) -> [u8; 8] {
        let s = crate::community_variants::script_named(key).unwrap();
        ::rules::compile(&s.source).unwrap().id().0[..8].try_into().unwrap()
    }

    #[test]
    fn a_built_in_by_key_or_name_names_its_script_on_the_record() {
        for word in ["siege", "Siege", "SIEGE"] {
            let block = block_for(&table(word, &[])).unwrap();
            let s = script_of(&block);
            assert_eq!(s.mode, variants::ext::Mode::Rules);
            assert_eq!(s.prefix, id_of("siege"));
        }
        let gun = block_for(&table("Gun Game", &[])).unwrap();
        assert_eq!(script_of(&gun).prefix, id_of("gun_game"));
    }

    #[test]
    fn superbounce_is_the_script_the_old_flag_played() {
        let block = block_for(&table("superbounce", &[])).unwrap();
        let listed = crate::community_variants::listed_named("Superbounce").unwrap();
        assert_eq!(block, listed.block());
    }

    #[test]
    fn options_pack_into_the_blocks_option_bits() {
        let plain = block_for(&table("siege", &[])).unwrap();
        let two = block_for(&table(
            "siege",
            &[("round_count", RuleOption::Int(2)), ("phase_time", RuleOption::Int(60))],
        ))
        .unwrap();
        assert_eq!(script_of(&plain).prefix, script_of(&two).prefix);
        let ir = ::rules::compile(&crate::community_variants::script_named("siege").unwrap().source).unwrap();
        let want = ir
            .pack_options(&[
                ("round_count".into(), ::rules::ir::OptionValue::Int(2)),
                ("phase_time".into(), ::rules::ir::OptionValue::Int(60)),
            ])
            .unwrap();
        assert_eq!(script_of(&two).options, want);
    }

    #[test]
    fn an_unknown_script_is_refused_by_name() {
        let e = block_for(&table("nonesuch", &[])).unwrap_err();
        assert!(e.contains("no built-in script \"nonesuch\""), "{e}");
        assert!(e.contains("siege"), "the built-ins are listed: {e}");
        let e = block_for(&table("/no/such/dir/mine.rules", &[])).unwrap_err();
        assert!(e.contains("script /no/such/dir/mine.rules"), "{e}");
    }

    #[test]
    fn an_unknown_option_or_value_is_refused_by_name() {
        let e = block_for(&table("siege", &[("rounds", RuleOption::Int(2))])).unwrap_err();
        assert!(e.contains("no option \"rounds\""), "{e}");
        assert!(e.contains("round_count"), "the options are listed: {e}");
        let e = block_for(&table("siege", &[("round_count", RuleOption::Int(9999))])).unwrap_err();
        assert!(e.contains("round_count"), "{e}");
        let e = block_for(&table("siege", &[("round_count", RuleOption::Name("many".into()))])).unwrap_err();
        assert!(e.contains("round_count") && e.contains("many"), "{e}");
    }

    #[test]
    fn a_rules_file_compiles_and_joins_the_store() {
        let dir = std::env::temp_dir().join(format!("h2-sim-rules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mine.rules");
        std::fs::write(
            &path,
            "rules \"Mine\" 1\ntype slayer\noption laps: int 1..5 = 2 \"Laps\" \"How many\"\non round_start {\n}\n",
        )
        .unwrap();
        let r = table(path.to_str().unwrap(), &[("laps", RuleOption::Int(4))]);
        let block = block_for(&r);
        std::fs::remove_dir_all(&dir).unwrap();
        let block = block.unwrap();
        let s = script_of(&block);
        assert_eq!(s.mode, variants::ext::Mode::Rules);
        assert!(crate::game_variants::store().lock().unwrap().script(s.prefix).is_some());
    }
}
