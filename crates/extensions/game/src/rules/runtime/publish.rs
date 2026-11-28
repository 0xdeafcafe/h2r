//! What the authority's running script publishes after each tick
//! (docs/game/rules-engine.md, "The host runtime"): its show state to the
//! actor channel, the objective HUDs, the walls and the pickers' cues.
//! Each is sent when it changes, or again on its repeat, and nothing is
//! copied or encoded on a tick that leaves it as it was.

use guest_layout::GuestMemory;

use super::{Runtime, Status};

/// The script's show state as last encoded for the actor channel's `Show`
/// record, with its bytes. The record is offered every tick the authority
/// runs the script (the capture takes it each tick, and a tick with none
/// retracts it), so the bytes are kept and the state is encoded again only
/// when it differs from the one they encode.
#[derive(Default)]
pub(super) struct ShowCue {
    pub(super) last: Option<(::rules::show::State, Vec<u8>)>,
    /// How many times a state was encoded.
    #[cfg(test)]
    pub(super) encodes: usize,
}

impl ShowCue {
    /// The bytes of `state`, encoded only when it is not the last one.
    pub(super) fn bytes(&mut self, state: &::rules::show::State) -> &[u8] {
        if !matches!(&self.last, Some((last, _)) if last == state) {
            #[cfg(test)]
            {
                self.encodes += 1;
            }
            self.last = Some((state.clone(), state.encode()));
        }
        &self.last.as_ref().expect("set above").1
    }

    /// Forgets the last state: nothing is offered, or another program or
    /// authority starts over.
    pub(super) fn forget(&mut self) {
        self.last = None;
    }
}

/// Whether the authority sends the walls cue now: the list changed since
/// the last one (an emptied list included), or it is not empty and the
/// last one is `WALLS_REPEAT` ticks old. Nothing shown and nothing sent is
/// nothing to say.
pub(super) fn walls_due(
    last: Option<&(Vec<(u32, Option<u8>)>, u32)>,
    shown: &[(u32, Option<u8>)],
    tick: u32,
) -> bool {
    match last {
        None => !shown.is_empty(),
        Some((sent, at)) => {
            sent != shown
                || (!shown.is_empty() && tick.wrapping_sub(*at) >= ::rules::show::WALLS_REPEAT)
        }
    }
}

impl Runtime {
    /// Offers the running script's show state to the actor channel's
    /// `Show` record (`crate::rules::world::show::offer_state`), from the
    /// authority that runs the script. A film's run offers none: it has
    /// no channel. Anything else forgets what was offered, so the channel
    /// retracts the last state from its peers.
    pub(super) fn send_show(&mut self) {
        match &self.status {
            Status::Running(r) if r.authority == Some(true) && !self.film => {
                crate::rules::world::show::offer_state(self.show_cue.bytes(r.vm.show()).to_vec());
            }
            _ => {
                self.show_cue.forget();
                crate::rules::world::show::forget_state();
            }
        }
    }

    /// Feeds the objective HUDs from the running script's show state, for
    /// each local user (`crate::rules::world::hud`), and hides them once no
    /// script feeds them. `None` for a game with no guest to read.
    pub(super) fn publish_hud(&mut self, m: Option<&dyn GuestMemory>) {
        let (on, objective) = match (&self.status, m) {
            (Status::Running(r), Some(m)) => (
                crate::rules::world::hud::publish(m, r.vm.ir(), r.vm.show(), &r.zones),
                crate::objective_hud::publish(m, r.vm.ir(), r.vm.ext(), &r.zones),
            ),
            _ => (false, false),
        };
        if self.hud_on && !on {
            crate::rules::world::hud::clear();
        }
        if self.objective_on && !objective {
            crate::objective_hud::clear();
        }
        self.hud_on = on;
        self.objective_on = objective;
    }

    /// Sends the areas the script shows to the clients as a walls cue
    /// (`::rules::show::Cue::Walls`), from the authority that runs the
    /// script. A film's run sends none: it has no channel.
    pub(super) fn send_walls(&mut self, tick: u32) {
        let shown = match &self.status {
            Status::Running(r) if r.authority == Some(true) && !self.film => r.vm.shown_areas(),
            _ => return,
        };
        if walls_due(self.wall_cue.as_ref(), shown, tick) {
            let shown = shown.to_vec();
            crate::rules::world::show::walls(tick, shown.clone());
            self.wall_cue = Some((shown, tick));
        }
    }

    /// Sends the pickers' offers to the clients as `Pick` cues
    /// (`::rules::show::Cue::Pick`), one per picker, from the authority
    /// that runs the script: again when the offers change and every
    /// `::rules::show::PICK_REPEAT` ticks while any stands, so a machine
    /// that joins late has them. A picker taken away goes as an empty
    /// one, which takes the asking on the clients off. A film's run
    /// sends none: it has no channel.
    pub(super) fn send_picks(&mut self, tick: u32) {
        let offers = match &self.status {
            // The loadout pickers are asked through the player's window
            // (`crate::rules::world::choice`), not as cues.
            Status::Running(r) if r.authority == Some(true) && !self.film => r.vm.plain_picks(),
            _ => return,
        };
        let (sent, at) = match self.pick_cue.as_ref() {
            Some((sent, at)) => (Some(sent.as_slice()), *at),
            None => (None, tick),
        };
        let changed = sent != Some(offers.as_slice());
        let repeat = !offers.is_empty() && tick.wrapping_sub(at) >= ::rules::show::PICK_REPEAT;
        if !changed && !repeat {
            return;
        }
        for old in sent.into_iter().flatten() {
            if !offers.iter().any(|o| o.slot == old.slot) {
                crate::rules::world::show::pick_offer(
                    tick,
                    ::rules::vm::PickOffer {
                        slot: old.slot,
                        title: old.title,
                        rows: Vec::new(),
                    },
                );
            }
        }
        for offer in &offers {
            crate::rules::world::show::pick_offer(tick, offer.clone());
        }
        self.pick_cue = Some((offers, tick));
    }

    /// The walls the running script shows in play (`show_area`), in the
    /// order it showed them; none when no script runs
    /// (`crate::rules::world::walls`).
    pub fn walls(&self) -> Vec<crate::forge_glue::markers_world::Wall> {
        match self.running() {
            Some(r) => crate::rules::world::walls::of(r.vm.shown_areas(), &r.zones),
            None => Vec::new(),
        }
    }

    /// Hands the walls to the window's world-line source: after each
    /// tick, and once more with none when the script's end took them off.
    pub(super) fn publish_walls(&mut self) {
        // Nothing shown makes no walls: with none published either, there
        // is nothing to build or say.
        let shows = self
            .running()
            .is_some_and(|r| !r.vm.shown_areas().is_empty());
        if !shows && !self.walled {
            return;
        }
        let walls = self.walls();
        if !walls.is_empty() || self.walled {
            self.walled = !walls.is_empty();
            crate::forge_glue::markers_world::publish_walls(walls);
        }
    }
}
