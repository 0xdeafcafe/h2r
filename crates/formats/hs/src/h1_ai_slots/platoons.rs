//! Static platoon data carried in a HaloScript string. Indices address the
//! converted squad and global tables and are remapped before encoding.

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatoonTrigger {
    pub condition: u16,
    pub squads: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatoonRule {
    pub latch: Option<u16>,
    pub enabled_by: Option<u16>,
    pub maneuver_flags: u16,
    pub members: Vec<u16>,
    pub change: Option<PlatoonTrigger>,
    pub maneuver: Option<PlatoonTrigger>,
    pub orders: Vec<(u16, u16)>,
    pub routes: Vec<(u16, u16)>,
}

pub fn encode_words(out: &mut String, words: impl IntoIterator<Item = u16>) {
    for word in words {
        write!(out, "{word:04x}").expect("write to string");
    }
}

pub fn decode_words(bytes: &[u8]) -> Option<Vec<u16>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    bytes
        .as_chunks::<4>().0.iter()
        .map(|word| u16::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok())
        .collect()
}

impl PlatoonRule {
    pub fn append_to(&self, out: &mut String) {
        encode_words(
            out,
            [
                self.latch.unwrap_or(u16::MAX),
                self.enabled_by.unwrap_or(u16::MAX),
                self.maneuver_flags,
                self.members.len() as u16,
            ],
        );
        encode_words(out, self.members.iter().copied());
        for trigger in [&self.change, &self.maneuver] {
            if let Some(trigger) = trigger {
                encode_words(out, [trigger.condition, trigger.squads.len() as u16]);
                encode_words(out, trigger.squads.iter().copied());
            } else {
                encode_words(out, [0]);
            }
        }
        for pairs in [&self.orders, &self.routes] {
            encode_words(out, [pairs.len() as u16]);
            encode_words(out, pairs.iter().flat_map(|&(a, b)| [a, b]));
        }
    }

    pub fn decode_all(bytes: &[u8]) -> Option<Vec<Self>> {
        let words = decode_words(bytes)?;
        let mut rest = words.as_slice();
        let mut rules = Vec::new();
        while !rest.is_empty() {
            let [latch, enabled_by, maneuver_flags, ..] = rest else {
                return None;
            };
            let latch = (*latch != u16::MAX).then_some(*latch);
            let enabled_by = (*enabled_by != u16::MAX).then_some(*enabled_by);
            let maneuver_flags = *maneuver_flags;
            rest = &rest[3..];
            let members = take_block(&mut rest, 1)?.to_vec();
            let change = take_trigger(&mut rest)?;
            let maneuver = take_trigger(&mut rest)?;
            let mut pairs = || {
                Some(
                    take_block(&mut rest, 2)?
                        .as_chunks::<2>().0.iter()
                        .map(|pair| (pair[0], pair[1]))
                        .collect(),
                )
            };
            rules.push(Self {
                latch,
                enabled_by,
                maneuver_flags,
                members,
                change,
                maneuver,
                orders: pairs()?,
                routes: pairs()?,
            });
        }
        Some(rules)
    }
}

fn take_word(words: &mut &[u16]) -> Option<u16> {
    let (&first, tail) = words.split_first()?;
    *words = tail;
    Some(first)
}

fn take_block<'a>(words: &mut &'a [u16], stride: usize) -> Option<&'a [u16]> {
    let len = usize::from(take_word(words)?) * stride;
    let (head, tail) = words.split_at_checked(len)?;
    *words = tail;
    Some(head)
}

fn take_trigger(words: &mut &[u16]) -> Option<Option<PlatoonTrigger>> {
    let condition = take_word(words)?;
    match condition {
        0 => Some(None),
        1..=9 => Some(Some(PlatoonTrigger {
            condition,
            squads: take_block(words, 1)?.to_vec(),
        })),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platoon_table_round_trips_and_rejects_incomplete_routes() {
        let rule = PlatoonRule {
            latch: Some(4),
            enabled_by: None,
            maneuver_flags: 2,
            members: vec![9],
            change: Some(PlatoonTrigger {
                condition: 7,
                squads: vec![2, 3],
            }),
            maneuver: None,
            orders: vec![(9, 300)],
            routes: vec![(9, 4)],
        };
        let mut out = String::new();
        rule.append_to(&mut out);
        rule.append_to(&mut out);
        assert_eq!(
            PlatoonRule::decode_all(out.as_bytes()),
            Some(vec![rule.clone(), rule])
        );
        assert!(PlatoonRule::decode_all(&out.as_bytes()[..out.len() - 4]).is_none());
    }
}
