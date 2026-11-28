//! A view's light history (`0x005234e8 + 0x484·view` for the lights,
//! `0x005246f8 + 0x484·view` for the object shadows): up to 32 rows of
//! `0x24` bytes and the count at `+0x480`. The only light state that
//! crosses frames. It stays in guest memory: [`History::decode`] and
//! [`History::encode`] each frame.
//!
//! [`merge`] is `0x0004a7c0` and [`rank_and_fade`] is `0x0004ac30` (with
//! its pair sort `0x0004b160`).

use super::visible::{VISIBLE_BYTES, VisibleLight};
use super::{Refusal, Result, Writes};
use guest_layout::memory::GuestMemory;

/// Rows a history holds.
pub const ROWS: usize = 32;
/// One row.
pub const ROW_BYTES: usize = 0x24;
/// The count's offset.
pub const COUNT: u32 = 0x480;
/// One view's history.
pub const STRIDE: u32 = 0x484;
/// The lights' histories, four views.
pub const LIGHTS: u32 = 0x0052_34e8;
/// The object shadows' histories.
pub const OBJECT_SHADOWS: u32 = 0x0052_46f8;

/// One row of a history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryRow {
    /// `+0..+0x14`: the candidate row last merged, its bytes kept.
    pub visible: VisibleLight,
    /// `+0x14`, f32 bits: the fade's rate.
    pub rate: u32,
    /// `+0x18`, f32 bits: the faded weight.
    pub weight: u32,
    /// `+0x1c`: the render frame it was last a candidate.
    pub seen: u32,
    /// `+0x20`: the render frame it entered.
    pub first: u32,
}

impl HistoryRow {
    pub fn decode(b: &[u8; ROW_BYTES]) -> Self {
        let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        Self {
            visible: VisibleLight::decode(b[..VISIBLE_BYTES].try_into().unwrap()),
            rate: word(0x14),
            weight: word(0x18),
            seen: word(0x1c),
            first: word(0x20),
        }
    }

    pub fn encode(&self) -> [u8; ROW_BYTES] {
        let mut b = [0; ROW_BYTES];
        b[..VISIBLE_BYTES].copy_from_slice(&self.visible.encode());
        for (o, v) in [(0x14, self.rate), (0x18, self.weight), (0x1c, self.seen), (0x20, self.first)] {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }

    fn scalar(bits: u32) -> f32 {
        f32::from_bits(bits)
    }
}

/// A view's history. The guest removes a row by copying the last live row
/// over it, so a slot past the count keeps what was last written there:
/// `slots` holds every slot written since [`History::decode`] (at least
/// the live ones), and the first `count` are live.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub slots: Vec<HistoryRow>,
    pub count: usize,
}

impl History {
    /// The history at `addr`. A count above 32 is [`Refusal::Layout`].
    pub fn decode(m: &dyn GuestMemory, addr: u32) -> Result<Self> {
        let count = m.u32(addr.checked_add(COUNT).ok_or(Refusal::Layout)?)? as usize;
        if count > ROWS {
            return Err(Refusal::Layout);
        }
        let bytes = m.bytes(addr, count * ROW_BYTES)?;
        Ok(Self {
            slots: bytes.chunks_exact(ROW_BYTES).map(|b| HistoryRow::decode(b.try_into().unwrap())).collect(),
            count,
        })
    }

    /// The live rows, in order.
    pub fn rows(&self) -> &[HistoryRow] {
        &self.slots[..self.count]
    }

    pub fn rows_mut(&mut self) -> &mut [HistoryRow] {
        &mut self.slots[..self.count]
    }

    /// Every slot written and the count, back at `addr`.
    pub fn encode(&self, addr: u32, out: &mut Writes) {
        out.array(addr, self.slots.iter().flat_map(|r| r.encode()).collect());
        out.word(addr.wrapping_add(COUNT), self.count as u32);
    }

    /// Whether every float the ranking compares is finite.
    pub fn finite(&self) -> bool {
        self.rows().iter().all(|r| {
            [r.visible.weight, r.visible.priority, r.rate, r.weight].into_iter().all(|b| f32::from_bits(b).is_finite())
        })
    }
}

/// `0x0004a7c0`: update the rows still candidates (by light datum),
/// remove the rows not seen this frame (the last live row copied over
/// each), and append new candidates in candidate order while there is
/// room. The guest's mask of merged candidates uses the low five bits of
/// the ordinal, also past 32.
pub fn merge(history: &mut History, candidates: &[VisibleLight], frame: u32) {
    let mut seen = 0u32;
    for row in history.rows_mut() {
        if let Some(i) = candidates.iter().position(|c| c.light == row.visible.light) {
            row.visible = candidates[i];
            row.seen = frame;
            seen |= 1 << (i % 32);
        }
    }
    let mut i = 0;
    while i < history.count {
        if history.slots[i].seen == frame {
            i += 1;
        } else {
            history.count -= 1;
            let last = history.slots[history.count];
            history.slots[i] = last;
        }
    }
    for (i, candidate) in candidates.iter().enumerate() {
        let bit = 1 << (i % 32);
        if history.count == ROWS {
            break;
        }
        if seen & bit != 0 {
            continue;
        }
        seen |= bit;
        let row = HistoryRow { visible: *candidate, rate: 0, weight: candidate.weight, seen: frame, first: frame };
        let at = history.count;
        if at < history.slots.len() {
            history.slots[at] = row;
        } else {
            history.slots.push(row);
        }
        history.count += 1;
    }
}

/// `0x0004b160`: one pass per row, each over every adjacent pair left to
/// right. The pass counter (`esp+0x10` after its four pushes) and the
/// first pair (`esp+0x14`, `lo + 1`) are separate slots, so every pass
/// starts at the first pair.
pub fn rank_adjacent(rows: &mut [HistoryRow], after: impl Fn(&HistoryRow, &HistoryRow) -> bool) {
    for _ in 0..rows.len() {
        for right in 1..rows.len() {
            let left = right - 1;
            if after(&rows[left], &rows[right]) {
                rows.swap(left, right);
            }
        }
    }
}

fn fade(row: &mut HistoryRow, delta: f32) {
    let rate = HistoryRow::scalar(row.rate) + delta;
    let opacity = HistoryRow::scalar(row.weight) + rate;
    let cap = HistoryRow::scalar(row.visible.weight);
    row.rate = rate.to_bits();
    row.weight = if 0.0 > opacity {
        0
    } else if opacity > cap {
        cap.to_bits()
    } else {
        opacity.to_bits()
    };
}

/// `0x0004ac30`: the local owner's rows first, then the rest by root
/// owner with one row per owner kept, then by priority; the first
/// `limit` rows fade in by `step` and the others fade out. The call site
/// passes the limit and the step.
pub fn rank_and_fade(rows: &mut [HistoryRow], frame: u32, limit: usize, step: f32) {
    let count = rows.len();
    let mut protected = 0;
    for i in 0..count.saturating_sub(1) {
        if rows[i].visible.local_owner != 0 && rows[i].visible.first_person != 0 {
            rows.swap(i, 0);
            protected += 1;
        }
    }
    if protected == 0
        && let Some(i) = rows.iter().position(|row| row.visible.local_owner != 0)
    {
        rows.swap(0, i);
        protected = 1;
    }
    let mut end = count;
    if count > limit {
        let mut i = protected;
        while i < end {
            if rows[i].visible.owner_kind_2 != 0 && rows[i].first == frame {
                rows[i].weight = 0;
                end -= 1;
                rows.swap(i, end);
            } else {
                i += 1;
            }
        }
    }

    rank_adjacent(&mut rows[protected..end], |a, b| (a.visible.root_owner as i32) < (b.visible.root_owner as i32));
    let mut i = protected;
    while i < end {
        let owner = rows[i].visible.root_owner;
        if i != 0 && owner != u32::MAX && owner == rows[i - 1].visible.root_owner && rows[i].visible.keep == 0 {
            rows[i].weight = 0;
            rows[i].rate = (0.0 - step).to_bits();
            end -= 1;
            rows.swap(i, end);
        }
        i += 1;
    }

    rank_adjacent(&mut rows[protected..end], |a, b| {
        HistoryRow::scalar(b.visible.priority) > HistoryRow::scalar(a.visible.priority)
    });
    let visible = limit.min(count);
    for row in rows[visible..].iter_mut().rev() {
        if row.visible.keep == 0 && HistoryRow::scalar(row.weight) > 0.0 {
            fade(row, -step);
        } else {
            row.rate = 0;
        }
    }
    for row in &mut rows[..visible] {
        if HistoryRow::scalar(row.visible.weight) > HistoryRow::scalar(row.weight) {
            fade(row, step);
        } else {
            row.rate = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(light: u32, owner: u32, priority: f32) -> VisibleLight {
        VisibleLight { light, root_owner: owner, weight: 1.0f32.to_bits(), priority: priority.to_bits(), ..Default::default() }
    }

    fn lights(h: &History) -> Vec<u32> {
        h.rows().iter().map(|r| r.visible.light).collect()
    }

    #[test]
    fn visible_lights_keep_membership_and_rank_with_guest_tie_order() {
        let candidates = [candidate(1, u32::MAX, 2.0), candidate(2, u32::MAX, 2.0), candidate(3, u32::MAX, 4.0)];
        let mut history = History::default();
        merge(&mut history, &candidates, 7);
        rank_and_fade(history.rows_mut(), 7, 3, 0.01);
        assert_eq!(lights(&history), [3, 1, 2]);

        merge(&mut history, &candidates[1..], 8);
        assert_eq!(history.count, 2);
        assert_eq!(lights(&history), [3, 2]);
    }

    #[test]
    fn light_rank_starts_every_pass_at_the_first_pair() {
        let mut rows = [HistoryRow::default(); 4];
        for (i, row) in rows.iter_mut().enumerate() {
            row.visible.priority = (i as f32).to_bits();
        }
        rank_adjacent(&mut rows, |a, b| f32::from_bits(b.visible.priority) > f32::from_bits(a.visible.priority));
        assert_eq!(rows.map(|row| f32::from_bits(row.visible.priority)), [3.0, 2.0, 1.0, 0.0]);
    }

    #[test]
    fn visible_light_history_fades_existing_rows_in_place() {
        let candidates =
            [candidate(1, u32::MAX, 4.0), candidate(2, u32::MAX, 3.0), candidate(3, u32::MAX, 2.0), candidate(4, u32::MAX, 1.0)];
        let mut history = History::default();
        merge(&mut history, &candidates, 10);
        rank_and_fade(history.rows_mut(), 10, 3, 0.01);
        assert_eq!(f32::from_bits(history.rows()[3].weight), 0.99);
        merge(&mut history, &candidates, 11);
        rank_and_fade(history.rows_mut(), 11, 3, 0.01);
        assert_eq!(f32::from_bits(history.rows()[3].weight), 0.99f32 - 0.02);
    }

    #[test]
    fn a_history_row_round_trips_its_bytes_with_a_stale_keep_byte() {
        let mut b = [0u8; ROW_BYTES];
        for (i, x) in b.iter_mut().enumerate() {
            *x = (i as u8).wrapping_mul(37).wrapping_add(5);
        }
        b[3] = 0xa7;
        assert_eq!(HistoryRow::decode(&b).encode(), b);
    }

    #[test]
    fn a_history_past_thirty_two_rows_is_refused() {
        let mut m = guest_layout::FlatMemory::new(0x1000, 0x1000);
        m.set_u32(0x1000 + COUNT, 33).unwrap();
        assert_eq!(History::decode(&m, 0x1000), Err(Refusal::Layout));
        m.set_u32(0x1000 + COUNT, 2).unwrap();
        assert_eq!(History::decode(&m, 0x1000).unwrap().rows().len(), 2);
    }
}
