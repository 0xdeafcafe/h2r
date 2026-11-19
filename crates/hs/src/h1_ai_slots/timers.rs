//! The Halo 1 timer registry stores one VM word per affected squad.
//! Two words per 32 squad indices give the membership mask and its rank.

pub const GLOBAL: &str = "h1_ai_timer_state";
pub const BLOCKS: u32 = 12;
pub const SQUADS: u16 = (BLOCKS * 32) as u16;
pub const HEADER: u32 = 1 + 2 * BLOCKS;

pub const TIME_UNITS_PER_SECOND: u32 = 3000;
pub const DURATION: u32 = 0x00ff_ffff;
pub const REVEAL_PLAYERS: u32 = 0x0100_0000;
pub const IMMEDIATE: u32 = 0x0200_0000;
pub const SCRIPT_HELD: u32 = 0x0400_0000;
pub const STARTED: u32 = 0x0800_0000;

pub fn initial(flags: u32, seconds: f32) -> u32 {
    let duration = (seconds * TIME_UNITS_PER_SECOND as f32).round() as u32;
    let mut value = duration.min(DURATION);
    if flags & 0x4 != 0 {
        value |= IMMEDIATE;
    }
    if flags & 0x8 != 0 {
        value |= SCRIPT_HELD;
    }
    if flags & 0x10 != 0 {
        value |= REVEAL_PLAYERS;
    }
    value
}

pub fn build(timers: impl IntoIterator<Item = (u16, u32, f32)>) -> Option<Vec<u32>> {
    let timers: std::collections::BTreeMap<_, _> = timers
        .into_iter()
        .map(|(squad, flags, delay)| (squad, initial(flags, delay)))
        .collect();
    if timers
        .last_key_value()
        .is_some_and(|(&squad, _)| squad >= SQUADS)
    {
        return None;
    }
    let mut words = vec![0; HEADER as usize];
    words[0] = u32::MAX;
    for (squad, value) in timers {
        let block = usize::from(squad / 32) * 2 + 1;
        words[block] |= 1 << (squad % 32);
        words.push(value);
    }
    let mut count = 0;
    for block in words[1..HEADER as usize].chunks_exact_mut(2) {
        block[1] = count;
        count += block[0].count_ones();
    }
    Some(words)
}

pub fn active(value: u32) -> bool {
    value & (SCRIPT_HELD | DURATION) != 0
}

pub fn expire(value: u32) -> u32 {
    value & REVEAL_PLAYERS
}

pub fn advance(value: u32, elapsed: u32, certain_enemy: bool, visible_enemy: bool) -> u32 {
    if !active(value) {
        return value;
    }
    if visible_enemy {
        return expire(value);
    }
    if value & SCRIPT_HELD != 0 {
        return value;
    }
    if value & STARTED == 0 {
        if certain_enemy || value & IMMEDIATE != 0 {
            return value | STARTED;
        }
        return value;
    }
    let remaining = (value & DURATION).saturating_sub(elapsed);
    if remaining == 0 {
        expire(value)
    } else {
        (value & !DURATION) | remaining
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_squads_start_on_alert_and_script_held_squads_wait() {
        let initial = initial(0x10, 0.5);
        assert_eq!(advance(initial, 1000, false, false), initial);
        let running = advance(initial, 1000, true, false);
        assert_eq!(running, initial | STARTED);
        assert_eq!(advance(running, 1500, false, false), REVEAL_PLAYERS);
        let held = SCRIPT_HELD | REVEAL_PLAYERS;
        assert_eq!(advance(held, u32::MAX, true, false), held);
        assert_eq!(advance(held, 0, true, true), REVEAL_PLAYERS);
        assert_eq!(expire(held), REVEAL_PLAYERS);
    }

    #[test]
    fn timer_table_keeps_squad_indices_and_a_compact_row_per_timer() {
        let words = build([(2, 0x8, 0.0), (42, 0x4, 2.0), (370, 0x10, 0.0)]).unwrap();
        assert_eq!(words[1..5], [4, 0, 1 << 10, 1]);
        assert_eq!(words[21..25], [0, 2, 1 << 18, 2]);
        assert_eq!(
            words[HEADER as usize..],
            [SCRIPT_HELD, IMMEDIATE | 6000, REVEAL_PLAYERS]
        );
    }
}
