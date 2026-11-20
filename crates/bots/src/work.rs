//! Tick-based scheduling shared by walking, ground drivers and sight.

/// Reaching a slot's phase still counts when the bot missed that tick.
/// Compute the distance from the previous tick so rollover is harmless.
pub(crate) fn phase_due(now: u32, previous: u32, slot: u32, period: u32) -> bool {
    let period = period.max(1);
    let phase = slot % period;
    let at = previous % period;
    let until = if phase > at {
        phase - at
    } else {
        period - (at - phase)
    };
    if now >= previous {
        now - previous >= until
    } else {
        u32::MAX - previous >= until || now >= phase
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PlanSchedule {
    // The tick before the first request. A standing request uses the next
    // slot phase, including when the first request is already on phase.
    pending: Option<u32>,
    urgent: bool,
    attempted: Option<u32>,
}

impl PlanSchedule {
    pub(crate) fn expedite(&mut self) {
        self.urgent = true;
    }

    /// A missing or invalid route needs a plan now. Only a refresh of a
    /// route the bot can still follow waits for its slot.
    pub(crate) fn ready(
        &mut self,
        tick: u32,
        slot: Option<u32>,
        period: u32,
        needs_route: bool,
    ) -> bool {
        if !needs_route {
            if self.attempted == Some(tick) {
                return false;
            }
            if let Some(slot) = slot {
                let previous = *self.pending.get_or_insert(tick.wrapping_sub(1));
                let regular = phase_due(tick, previous, slot, period);
                // Spread reactions to a shared event over two ticks.
                let urgent = self.urgent && (period <= 1 || tick % 2 == slot % 2);
                if !regular && !urgent {
                    return false;
                }
            }
        }
        self.idle();
        self.attempted = Some(tick);
        true
    }

    pub(crate) fn idle(&mut self) {
        self.pending = None;
        self.urgent = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshes_are_staggered_by_slot() {
        let mut bots: [PlanSchedule; 4] = Default::default();
        for tick in 0..16 {
            for (slot, bot) in bots.iter_mut().enumerate() {
                assert_eq!(
                    bot.ready(tick, Some(slot as u32), 4, false),
                    tick % 4 == slot as u32
                );
                assert!(!bot.ready(tick, Some(slot as u32), 4, false));
            }
        }
    }

    #[test]
    fn damage_or_lost_target_can_expedite_a_refresh() {
        let mut bot = PlanSchedule::default();
        assert!(bot.ready(1, Some(1), 8, false));
        bot.expedite();
        assert!(!bot.ready(2, Some(1), 8, false));
        assert!(bot.ready(3, Some(1), 8, false));
        assert!(!bot.ready(3, Some(1), 8, false));
        assert!(!bot.ready(5, Some(1), 8, false));
        assert!(bot.ready(10, Some(1), 8, false)); // missed phase at 9
    }

    #[test]
    fn required_plans_bypass_the_slot_and_the_refresh_limit() {
        let mut bot = PlanSchedule::default();
        assert!(!bot.ready(2, Some(1), 8, false));
        assert!(bot.ready(2, Some(1), 8, true));
        assert!(!bot.ready(2, Some(1), 8, false));
        assert!(bot.ready(2, Some(1), 8, true));
        assert!(bot.ready(3, Some(1), 8, true));
    }

    #[test]
    fn phase_resumes_after_the_tick_counter_wraps() {
        assert!(phase_due(0, u32::MAX, 0, 6));
        assert!(!phase_due(1, u32::MAX, 2, 6));
        assert!(phase_due(2, u32::MAX, 2, 6));
    }
}
