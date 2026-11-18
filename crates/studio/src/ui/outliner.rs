//! The outliner's search and click rules, apart from drawing: what a
//! filter matches and what a click with modifiers selects.
use scenario_edit::BlockKind;

/// Whether an entry shows under `filter`: every word of the filter (any
/// case) is in the block's name, the entry's label (its index and tag
/// name) or `#<index>`. An empty filter shows everything.
pub fn matches(filter: &str, block: &str, label: &str, index: usize) -> bool {
    let hay = format!("{block} {label} #{index}").to_ascii_lowercase();
    filter
        .split_whitespace()
        .all(|w| hay.contains(&w.to_ascii_lowercase()))
}

/// What a click on entry `(kind, index)` selects: Ctrl/Cmd adds or takes
/// it away, Shift takes the run from the primary selection to it (in the
/// same block, the shown entries only), anything else selects it alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Click {
    One((BlockKind, usize)),
    Toggle((BlockKind, usize)),
    Many(Vec<(BlockKind, usize)>),
}

pub fn click(
    entry: (BlockKind, usize),
    toggle: bool,
    range: bool,
    primary: Option<(BlockKind, usize)>,
    shown: &[usize],
) -> Click {
    if toggle {
        return Click::Toggle(entry);
    }
    match primary {
        Some((kind, from)) if range && kind == entry.0 => {
            let (lo, hi) = (from.min(entry.1), from.max(entry.1));
            let mut run: Vec<_> = shown
                .iter()
                .copied()
                .filter(|i| (lo..=hi).contains(i))
                .map(|i| (kind, i))
                .collect();
            // The clicked entry becomes the primary.
            run.retain(|e| *e != entry);
            run.insert(0, entry);
            Click::Many(run)
        }
        _ => Click::One(entry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_must_match_somewhere() {
        assert!(matches("", "Weapons", "0: battle_rifle", 0));
        assert!(matches("rifle", "Weapons", "0: battle_rifle", 0));
        assert!(matches("WEAP rifle", "Weapons", "0: battle_rifle", 0));
        assert!(!matches("weap sniper", "Weapons", "0: battle_rifle", 0));
        assert!(matches("#12", "Netgame flags", "12", 12));
        assert!(!matches("#12", "Netgame flags", "3", 3));
    }

    #[test]
    fn clicks_select_one_toggle_or_take_a_run() {
        let w = |i| (BlockKind::Weapons, i);
        assert_eq!(click(w(3), false, false, Some(w(1)), &[]), Click::One(w(3)));
        assert_eq!(
            click(w(3), true, false, Some(w(1)), &[]),
            Click::Toggle(w(3))
        );
        // Shift: the run between, as shown (a filter hides 2).
        assert_eq!(
            click(w(4), false, true, Some(w(1)), &[0, 1, 3, 4, 5]),
            Click::Many(vec![w(4), w(1), w(3)])
        );
        // Upward too.
        assert_eq!(
            click(w(0), false, true, Some(w(2)), &[0, 1, 2]),
            Click::Many(vec![w(0), w(1), w(2)])
        );
        // Another block, or nothing selected: just the one.
        let s = (BlockKind::Scenery, 0);
        assert_eq!(click(s, false, true, Some(w(1)), &[0]), Click::One(s));
        assert_eq!(click(s, false, true, None, &[0]), Click::One(s));
    }
}
