//! The spawn table precedes the ordinary script strings. Its object-tag
//! entries are rooted script atoms, so the normal converter resolves them.

use hs::h1_conversation::spawn::*;

use super::*;

pub(super) fn prefix_len(scopes: &SpeakerScopes) -> usize {
    HEADER
        + BANK_BYTES * scopes.voice_definitions.len()
        + SQUAD_BYTES * scopes.squad_voices.len()
        + 2 * scopes
            .squad_voices
            .iter()
            .map(|s| s.locations.len())
            .sum::<usize>()
}

pub(super) fn build(program: &mut Program<'_, '_>, scopes: &SpeakerScopes) -> Result<u32> {
    let base = program.compiler.out.globals.len() as u32;
    for _ in 0..VOICE_RECORDS * 2 {
        program.global("unit_voice", long(0))?;
    }
    let mut bytes = Vec::with_capacity(prefix_len(scopes));
    for word in [
        0,
        SIGNATURE,
        base,
        scopes.voice_definitions.len() as u32,
        scopes.squad_voices.len() as u32,
    ] {
        bytes.extend(word.to_le_bytes());
    }
    let object_def = program.compiler.h2.type_index("object_definition");
    for bank in &scopes.voice_definitions {
        let ty = object_def
            .ok_or_else(|| invalid("conversation voice banks require object_definition"))?;
        let unit = program.global("voice_unit", Expr::Hook(ty, bank.name.clone()))?;
        bytes.extend(unit.index.to_le_bytes());
        bytes.extend((bank.generic.len() as u16).to_le_bytes());
        for slot in 0..16 {
            bytes.extend(
                bank.generic
                    .get(slot)
                    .copied()
                    .unwrap_or_default()
                    .to_le_bytes(),
            );
        }
    }
    let mut starts = Vec::new();
    for squad in &scopes.squad_voices {
        let first = u16::try_from(starts.len())
            .map_err(|_| invalid("too many voice starting locations"))?;
        let count = u16::try_from(squad.locations.len())
            .map_err(|_| invalid("too many squad voice starting locations"))?;
        for word in [
            squad.encounter,
            squad.leader_rule,
            squad.generic.unwrap_or(NO_BANK),
            first,
            count,
        ] {
            bytes.extend(word.to_le_bytes());
        }
        starts.extend(squad.locations.iter().map(|bank| bank.unwrap_or(NO_BANK)));
    }
    bytes.extend(starts.into_iter().flat_map(u16::to_le_bytes));
    assert_eq!(bytes.len(), prefix_len(scopes));
    program.compiler.out.strings[..bytes.len()].copy_from_slice(&bytes);
    Ok(base)
}
