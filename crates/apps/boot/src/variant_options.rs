//! Our added game variant choices in the game's own editor
//! (docs/game/game-options.md, "Showing them").
//!
//! Every editor row is a `sily` setting definition with a block of options
//! (`+0x1c` count, `+0x20` address; `0xc` bytes each: default flag, value,
//! label string id). The choices of [`variants::extra::OPTIONS`] are values
//! the record keeps and plays but the retail lists leave out (starting
//! weapon "None", a 1 s respawn, more score, time and lives steps). Before
//! the main menu's built-in variants load (`0x149f88`, the same wrapper as
//! the community variants) each definition with choices of ours gets a
//! copy of its block in our page, the retail options byte for byte with
//! ours merged in value order, and the definition is pointed at it. The
//! game's editor then lists, shows and stores them through its own code.
//!
//! The labels are string ids of our "variant options" family, answered by
//! the community variants' string hook. A definition already pointing into
//! our page (the same tag memory loaded again) is left alone, and a
//! definition the page has no room for keeps its retail block.

use std::collections::HashSet;

use variants::SettingId;
use variants::extra::{self, OPTIONS};

use crate::community_variants::{GOOF_PAGES, Mem, OPTION_SIZE, PAGE_SIZE, SILY_OPTIONS};

/// Our page for the extended option blocks.
pub const BASE: u32 = 0xa0fd_4000;
pub const SIZE: u32 = 0x4000;

/// The labels' string ids: one per [`OPTIONS`] entry.
const IDS: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("variant options");
const _: () = assert!(OPTIONS.len() as u32 <= IDS.len);

/// The label id of [`OPTIONS`]`[k]`.
pub fn label_id(k: usize) -> u32 {
    IDS.id(k as u32)
}

/// The text for one of our label ids.
pub fn text(id: u32) -> Option<String> {
    if !IDS.owns(id) {
        return None;
    }
    OPTIONS
        .get((id - IDS.base) as usize)
        .map(|e| e.label.to_owned())
}

/// What [`extend`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Extended {
    /// Definitions pointed at a merged block.
    pub lists: usize,
    /// Choices of ours added over all of them.
    pub choices: usize,
    /// Definitions that would have needed more room than the page has.
    pub no_room: usize,
}

/// The `sily` definitions the `goof` tag's pages name, each once (the
/// summary pages repeat settings from the others).
fn definitions<M: Mem>(m: &M, goof: u32, tags: u32) -> Option<Vec<u32>> {
    let (pages, at) = (m.u32(goof + GOOF_PAGES)?, m.u32(goof + GOOF_PAGES + 4)?);
    let (mut seen, mut out) = (HashSet::new(), Vec::new());
    for p in 0..pages.min(0x100) {
        let page = at + p * PAGE_SIZE;
        let (n, refs) = (m.u32(page + 8)?, m.u32(page + 0xc)?);
        for r in 0..n.min(0x100) {
            let datum = m.u32(refs + r * 8 + 4)?;
            if datum == u32::MAX {
                continue;
            }
            let sily = m.u32(tags + (datum & 0xffff) * 16 + 8)?;
            if seen.insert(sily) {
                out.push(sily);
            }
        }
    }
    Some(out)
}

/// One definition's merged block: retail options as they are, ours with
/// no default flag. `None` when it has nothing of ours to add.
fn merged_block(id: SettingId, retail: &[u8]) -> Option<(Vec<u8>, usize)> {
    let options: Vec<&[u8]> = retail.chunks_exact(OPTION_SIZE as usize).collect();
    let values: Vec<i32> = options
        .iter()
        .map(|o| i32::from_le_bytes(o[4..8].try_into().unwrap()))
        .collect();
    let merged = extra::merge(id, &values);
    let added = merged.iter().filter(|r| r.is_err()).count();
    if added == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(merged.len() * OPTION_SIZE as usize);
    for r in merged {
        match r {
            Ok(i) => out.extend_from_slice(options[i]),
            Err(k) => {
                out.extend_from_slice(&0u32.to_le_bytes());
                out.extend_from_slice(&OPTIONS[k].value.to_le_bytes());
                out.extend_from_slice(&label_id(k).to_le_bytes());
            }
        }
    }
    Some((out, added))
}

/// Points every definition with choices of ours at a merged copy of its
/// options in `base..base + size`. The retail blocks are never written.
/// `None` is a read failure before anything changed.
pub fn extend<M: Mem>(m: &mut M, goof: u32, tags: u32, base: u32, size: u32) -> Option<Extended> {
    let defs = definitions(m, goof, tags)?;
    let (end, mut at) = (base + size, base);
    let mut done = Extended::default();
    for sily in defs {
        let Some(setting) = m.u32(sily) else { continue };
        if extra::for_setting(SettingId(setting)).next().is_none() {
            continue;
        }
        let (Some(n), Some(old)) = (m.u32(sily + SILY_OPTIONS), m.u32(sily + SILY_OPTIONS + 4))
        else {
            continue;
        };
        if (base..end).contains(&old) || n == 0 || n > 0x100 {
            continue;
        }
        let mut retail = vec![0; (n * OPTION_SIZE) as usize];
        if !m.rd(old, &mut retail) {
            continue;
        }
        let Some((block, added)) = merged_block(SettingId(setting), &retail) else {
            continue;
        };
        if at + block.len() as u32 > end {
            done.no_room += 1;
            continue;
        }
        if !m.wr(at, &block) {
            continue;
        }
        // The block first, then the address and the count that use it.
        let count = (block.len() / OPTION_SIZE as usize) as u32;
        if m.set_u32(sily + SILY_OPTIONS + 4, at) && m.set_u32(sily + SILY_OPTIONS, count) {
            done.lists += 1;
            done.choices += added;
        }
        at += block.len() as u32;
    }
    Some(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake(HashMap<u32, u8>);
    impl Mem for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    const GOOF: u32 = 0x8000_0000;
    const TAGS: u32 = 0x8001_0000;

    fn sily_at(k: usize) -> u32 {
        0x8010_0000 + k as u32 * 0x1000
    }

    /// A goof tag with two pages naming the same definitions (as a summary
    /// page repeats them), each `(setting, [(default, value, label)])`.
    fn world(defs: &[(u32, &[(u32, i32, u32)])]) -> Fake {
        let mut m = Fake::default();
        m.wr(GOOF, &[0; 0x40]);
        let (pages, refs) = (0x8000_2000, 0x8000_3000);
        m.set_u32(GOOF + GOOF_PAGES, 2);
        m.set_u32(GOOF + GOOF_PAGES + 4, pages);
        for p in 0..2 {
            let page = pages + p * PAGE_SIZE;
            m.wr(page, &[0; PAGE_SIZE as usize]);
            m.set_u32(page + 8, defs.len() as u32 + 1);
            m.set_u32(page + 0xc, refs);
        }
        m.set_u32(refs, 0);
        m.set_u32(refs + 4, u32::MAX);
        for (k, (setting, options)) in defs.iter().enumerate() {
            let sily = sily_at(k);
            m.set_u32(refs + (k as u32 + 1) * 8, 0);
            m.set_u32(refs + (k as u32 + 1) * 8 + 4, 0xe000_0000 | (k as u32 + 5));
            m.set_u32(TAGS + (k as u32 + 5) * 16 + 8, sily);
            m.wr(sily, &[0; 0x24]);
            m.set_u32(sily, *setting);
            m.set_u32(sily + SILY_OPTIONS, options.len() as u32);
            m.set_u32(sily + SILY_OPTIONS + 4, sily + 0x100);
            for (j, (d, v, l)) in options.iter().enumerate() {
                let o = sily + 0x100 + j as u32 * OPTION_SIZE;
                m.set_u32(o, *d);
                m.set_u32(o + 4, *v as u32);
                m.set_u32(o + 8, *l);
            }
        }
        m
    }

    fn options(m: &Fake, sily: u32) -> Vec<(u32, i32, u32)> {
        let (n, at) = (
            m.u32(sily + SILY_OPTIONS).unwrap(),
            m.u32(sily + SILY_OPTIONS + 4).unwrap(),
        );
        (0..n)
            .map(|j| {
                let o = at + j * OPTION_SIZE;
                (
                    m.u32(o).unwrap(),
                    m.u32(o + 4).unwrap() as i32,
                    m.u32(o + 8).unwrap(),
                )
            })
            .collect()
    }

    const WEAPON: &[(u32, i32, u32)] = &[
        (1, 0, 0x0700_0001),
        (0, 2, 0x0700_0002),
        (0, 3, 0x0700_0003),
    ];
    const SHIELDS: &[(u32, i32, u32)] = &[(1, 0, 0x0700_0010), (0, 1, 0x0700_0011)];
    const RESPAWN: &[(u32, i32, u32)] = &[
        (0, 3, 0x0700_0020),
        (1, 5, 0x0700_0021),
        (0, 10, 0x0700_0022),
    ];

    #[test]
    fn our_choices_join_the_retail_lists_in_value_order() {
        let mut m = world(&[(89, WEAPON), (19, SHIELDS), (17, RESPAWN)]);
        let retail: Vec<Vec<u8>> = (0..3)
            .map(|k| {
                let mut b = vec![0; 3 * OPTION_SIZE as usize];
                m.rd(sily_at(k) + 0x100, &mut b);
                b
            })
            .collect();
        let done = extend(&mut m, GOOF, TAGS, BASE, SIZE).unwrap();
        // Weapons and respawn extended once each, though two pages name
        // them; shields have nothing of ours.
        assert_eq!(done.lists, 2);
        assert_eq!(done.no_room, 0);

        let none = OPTIONS
            .iter()
            .position(|e| e.setting == 89 && e.value == 1)
            .unwrap();
        assert_eq!(
            options(&m, sily_at(0)),
            [WEAPON[0], (0, 1, label_id(none)), WEAPON[1], WEAPON[2]]
        );
        assert_eq!(text(label_id(none)).as_deref(), Some("None"));
        assert_eq!(options(&m, sily_at(1)), SHIELDS, "untouched");
        let respawn: Vec<i32> = options(&m, sily_at(2)).iter().map(|o| o.1).collect();
        assert_eq!(respawn, [1, 2, 3, 5, 10, 45, 60, 120]);
        assert_eq!(done.choices, 1 + 5);
        // The retail option keeps its default flag and label id.
        assert!(options(&m, sily_at(2)).contains(&RESPAWN[1]));

        // The retail blocks are never written.
        for (k, b) in retail.iter().enumerate() {
            let mut now = vec![0; b.len()];
            m.rd(sily_at(k) + 0x100, &mut now);
            assert_eq!(&now, b);
        }

        // Again (the same tag memory loaded again): nothing changes.
        let before = options(&m, sily_at(0));
        assert_eq!(extend(&mut m, GOOF, TAGS, BASE, SIZE).unwrap().lists, 0);
        assert_eq!(options(&m, sily_at(0)), before);
    }

    #[test]
    fn a_list_the_page_cannot_hold_stays_retail() {
        let mut m = world(&[(89, WEAPON), (17, RESPAWN)]);
        // Room for the weapons' four options only.
        let done = extend(&mut m, GOOF, TAGS, BASE, 4 * OPTION_SIZE).unwrap();
        assert_eq!((done.lists, done.no_room), (1, 1));
        assert_eq!(options(&m, sily_at(1)), RESPAWN);
        // A tag with no pages: nothing to do.
        let mut empty = Fake::default();
        empty.wr(GOOF, &[0; 0x40]);
        assert_eq!(
            extend(&mut empty, GOOF, TAGS, BASE, SIZE),
            Some(Extended::default())
        );
        // A page pointing nowhere is a read failure.
        let mut broken = Fake::default();
        broken.wr(GOOF, &[0; 0x40]);
        broken.set_u32(GOOF + GOOF_PAGES, 1);
        broken.set_u32(GOOF + GOOF_PAGES + 4, 0x9000_0000);
        assert_eq!(extend(&mut broken, GOOF, TAGS, BASE, SIZE), None);
    }

    #[test]
    fn every_added_choice_has_a_label_id_of_ours() {
        for (k, e) in OPTIONS.iter().enumerate() {
            assert_eq!(text(label_id(k)).as_deref(), Some(e.label));
            assert!(!crate::community_variants::is_ours(label_id(k)));
        }
        assert_eq!(text(label_id(OPTIONS.len() - 1) + 1), None);
        assert_eq!(text(crate::community_variants::name_id(0)), None);
    }
}
